package io.github.st3inberg.tabextend

import android.media.MediaCodec
import android.media.MediaCodecInfo.CodecCapabilities
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.Surface

/**
 * Hardware H.264 decoding straight onto a Surface.
 *
 * Asks for the decoder's low-latency mode where it has one, so it outputs
 * each frame as soon as it is decoded instead of buffering for smoothness.
 * A separate thread shows decoded frames the moment they are ready; waiting
 * for the next input instead would hold a finished frame back whenever
 * frames arrive unevenly.
 */
class VideoDecoder(width: Int, height: Int, surface: Surface) {
    private val codec = MediaCodec.createDecoderByType(MIME)
    private val info = MediaCodec.BufferInfo()
    val lowLatency: Boolean
    val name: String = codec.name

    /** Vendor low-latency switches this decoder offered and that were turned on. */
    var vendorLowLatency: List<String> = emptyList()
        private set

    @Volatile var framesRendered = 0L
        private set

    /** When each frame went in, by presentation time, to time its way out. Guarded by itself. */
    private val submittedAt = HashMap<Long, Long>()
    private var decodeNanosTotal = 0L
    private var decodeCount = 0

    @Volatile private var running = true
    private val output: Thread

    init {
        val caps = codec.codecInfo.getCapabilitiesForType(MIME)
        lowLatency = caps.isFeatureSupported(CodecCapabilities.FEATURE_LowLatency)
        val format = MediaFormat.createVideoFormat(MIME, width, height).apply {
            // Always ask: some decoders (MediaTek's c2.mtk.avc.decoder) honour
            // it without advertising the feature, and others ignore it.
            setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0) // realtime
        }
        codec.configure(format, surface, null, 0)
        vendorLowLatency = enableVendorLowLatency()
        codec.start()
        output = Thread({ while (running) showReadyFrames() }, "tabextend-decoder-out").apply { start() }
    }

    /**
     * Some decoders (e.g. MediaTek's) lack the standard low-latency feature
     * but offer their own switch. Ask the decoder for its vendor parameters
     * and turn on any low-latency one, instead of hardcoding vendor names.
     */
    private fun enableVendorLowLatency(): List<String> {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return emptyList()
        val names = runCatching { codec.supportedVendorParameters }.getOrDefault(emptyList())
            .filter { it.replace("-", "").replace("_", "").lowercase().contains("lowlatency") }
        if (names.isEmpty()) return emptyList()
        val params = Bundle().apply { names.forEach { putInt(it, 1) } }
        return runCatching { codec.setParameters(params); names }
            .onFailure { Log.w(TAG, "vendor low-latency parameters rejected: $names", it) }
            .getOrDefault(emptyList())
            .also { Log.i(TAG, "vendor low-latency enabled: $it") }
    }

    /** Average time from queueing a frame to it being ready to show, then reset. */
    fun takeAverageDecodeMs(): Double = synchronized(submittedAt) {
        val average = if (decodeCount == 0) 0.0 else decodeNanosTotal / decodeCount / 1e6
        decodeNanosTotal = 0
        decodeCount = 0
        average
    }

    /**
     * Queue one access unit. Returns false if the decoder had no free input
     * buffer in time; the frame is dropped and the caller should ask the host
     * for a keyframe, since later frames depend on this one.
     */
    fun submit(data: ByteArray, ptsUs: Long, keyframe: Boolean): Boolean {
        val index = codec.dequeueInputBuffer(INPUT_TIMEOUT_US)
        if (index < 0) return false
        val buffer = codec.getInputBuffer(index) ?: return false
        if (buffer.capacity() < data.size) {
            codec.queueInputBuffer(index, 0, 0, ptsUs, 0)
            return false
        }
        buffer.clear()
        buffer.put(data)
        synchronized(submittedAt) {
            if (submittedAt.size > MAX_TRACKED) submittedAt.clear()
            submittedAt[ptsUs] = System.nanoTime()
        }
        codec.queueInputBuffer(index, 0, data.size, ptsUs, if (keyframe) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0)
        return true
    }

    /** Wait briefly for a decoded frame and show it at once. Runs on the output thread. */
    private fun showReadyFrames() {
        val index = try {
            codec.dequeueOutputBuffer(info, OUTPUT_TIMEOUT_US)
        } catch (e: IllegalStateException) {
            return // released while waiting
        }
        if (index < 0) return // nothing yet, or a format or buffer change
        synchronized(submittedAt) {
            submittedAt.remove(info.presentationTimeUs)?.let {
                decodeNanosTotal += System.nanoTime() - it
                decodeCount++
            }
        }
        codec.releaseOutputBuffer(index, true)
        framesRendered++
    }

    fun release() {
        running = false
        output.join(OUTPUT_TIMEOUT_US / 1000 * 4)
        runCatching { codec.stop() }
        codec.release()
    }

    private companion object {
        const val MIME = MediaFormat.MIMETYPE_VIDEO_AVC
        const val INPUT_TIMEOUT_US = 20_000L
        const val OUTPUT_TIMEOUT_US = 10_000L
        const val MAX_TRACKED = 256
        const val TAG = "TabExtendDecoder"
    }
}
