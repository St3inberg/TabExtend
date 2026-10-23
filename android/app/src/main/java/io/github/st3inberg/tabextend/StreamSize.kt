package io.github.st3inberg.tabextend

import android.media.MediaCodecInfo
import android.media.MediaCodecList
import android.media.MediaFormat

/**
 * The largest stream this tablet can decode smoothly.
 *
 * Decoders often cannot handle the panel's full resolution at full frame
 * rate (the reference tablet's H.264 decoder tops out at 2560×1440, and at
 * about 1080p for 60 fps). Rather than assume limits, ask the decoder and
 * shrink the stream, keeping the panel's aspect ratio, until it fits. The
 * decoded picture is scaled up to fill the screen.
 */
object StreamSize {
    /** Smallest side worth streaming; below this, give up shrinking. */
    private const val MIN_SIDE = 320

    /**
     * Pure search, testable without Android: the largest even size with the
     * display's aspect ratio for which [supported] is true, or null.
     */
    fun fit(displayWidth: Int, displayHeight: Int, supported: (Int, Int) -> Boolean): Pair<Int, Int>? {
        val aspect = displayHeight.toDouble() / displayWidth
        var width = displayWidth and 1.inv()
        while (true) {
            val height = (width * aspect).toInt() and 1.inv()
            if (width < MIN_SIDE || height < MIN_SIDE) return null
            if (supported(width, height)) return width to height
            width -= 16
        }
    }

    /** The first hardware H.264 decoder, as `MediaCodec.createDecoderByType` would pick. */
    private fun decoderCapabilities(): MediaCodecInfo.VideoCapabilities? {
        val mime = MediaFormat.MIMETYPE_VIDEO_AVC
        return MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos
            .filter { !it.isEncoder && mime in it.supportedTypes.map(String::lowercase) }
            .sortedByDescending { it.isHardwareAccelerated }
            .firstOrNull()
            ?.getCapabilitiesForType(mime)
            ?.videoCapabilities
    }

    /** What to ask the host for, given the panel and a target frame rate. */
    fun forDisplay(width: Int, height: Int, fps: Double): Pair<Int, Int> {
        val caps = decoderCapabilities() ?: return width to height
        return fit(width, height) { w, h ->
            caps.areSizeAndRateSupported(w, h, fps) ||
                caps.isSizeSupported(h, w) && caps.areSizeAndRateSupported(h, w, fps)
        } ?: (width to height)
    }
}
