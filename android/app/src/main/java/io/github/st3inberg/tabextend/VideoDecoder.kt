package io.github.st3inberg.tabextend

import android.media.MediaCodec
import android.media.MediaCodecInfo.CodecCapabilities
import android.media.MediaFormat
import android.view.Surface

/**
 * Hardware H.264 decoding straight onto a Surface.
 *
 * Asks for the decoder's low-latency mode where it has one, so it outputs
 * each frame as soon as it is decoded instead of buffering for smoothness.
 */
class VideoDecoder(width: Int, height: Int, surface: Surface) {
    private val codec = MediaCodec.createDecoderByType(MIME)
    private val info = MediaCodec.BufferInfo()
    val lowLatency: Boolean
    val name: String = codec.name

    var framesRendered = 0L
        private set

    init {
        val caps = codec.codecInfo.getCapabilitiesForType(MIME)
        lowLatency = caps.isFeatureSupported(CodecCapabilities.FEATURE_LowLatency)
        val format = MediaFormat.createVideoFormat(MIME, width, height).apply {
            if (lowLatency) setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
            setInteger(MediaFormat.KEY_PRIORITY, 0) // realtime
        }
        codec.configure(format, surface, null, 0)
        codec.start()
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
        codec.queueInputBuffer(index, 0, data.size, ptsUs, if (keyframe) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0)
        drain()
        return true
    }

    /** Render every frame that is ready, immediately. */
    private fun drain() {
        while (true) {
            val index = codec.dequeueOutputBuffer(info, 0)
            when {
                index >= 0 -> {
                    codec.releaseOutputBuffer(index, true)
                    framesRendered++
                }
                index == MediaCodec.INFO_TRY_AGAIN_LATER -> return
                else -> continue // format or buffer change
            }
        }
    }

    fun release() {
        runCatching { codec.stop() }
        codec.release()
    }

    private companion object {
        const val MIME = MediaFormat.MIMETYPE_VIDEO_AVC
        const val INPUT_TIMEOUT_US = 20_000L
    }
}
