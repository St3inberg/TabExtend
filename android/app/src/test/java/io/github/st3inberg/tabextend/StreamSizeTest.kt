package io.github.st3inberg.tabextend

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class StreamSizeTest {
    @Test
    fun keepsTheFullSizeWhenTheDecoderCanTakeIt() {
        assertEquals(2560 to 1600, StreamSize.fit(2560, 1600) { _, _ -> true })
    }

    @Test
    fun shrinksToTheDecodersLimitKeepingAspectRatio() {
        // Like the reference tablet: about 1080p worth of 16×16 blocks at 60 fps.
        val maxBlocks = 8160
        val (w, h) = StreamSize.fit(2560, 1600) { w, h -> (w / 16) * (h / 16) <= maxBlocks }!!
        assertEquals(1600.0 / 2560, h.toDouble() / w, 0.01)
        assert((w / 16) * (h / 16) <= maxBlocks)
        assert(w > 1700) { "should be close to the limit, got ${w}x$h" }
    }

    @Test
    fun respectsAMaximumWidth() {
        assertEquals(2560 to 1600, StreamSize.fit(2560, 1600) { w, _ -> w <= 2560 })
        val (w, _) = StreamSize.fit(2560, 1600) { w, h -> w <= 2560 && h <= 1440 }!!
        assert(w <= 2304)
    }

    @Test
    fun sizesAreEven() {
        val (w, h) = StreamSize.fit(2561, 1601) { w, _ -> w < 2000 }!!
        assertEquals(0, w % 2)
        assertEquals(0, h % 2)
    }

    @Test
    fun portraitWorks() {
        val (w, h) = StreamSize.fit(1600, 2560) { w, h -> w * h <= 1_000_000 }!!
        assert(h > w)
    }

    @Test
    fun givesUpOnUselessDecoders() {
        assertNull(StreamSize.fit(2560, 1600) { _, _ -> false })
    }
}
