package io.github.st3inberg.tabextend

import java.io.ByteArrayInputStream
import java.io.DataInputStream
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class ProtocolTest {
    private val display = DisplayInfo(2560, 1600, 240, 90_000)

    private fun roundTrip(message: Message): Message {
        val encoded = Protocol.encode(message)
        return Protocol.read(DataInputStream(ByteArrayInputStream(encoded)))
    }

    @Test
    fun pingMatchesTheRustWireFormat() {
        // Same vector as `known_bytes_for_ping` in crates/core/src/protocol.rs.
        val expected = byteArrayOf(0, 0, 0, 13, 5, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2)
        assertArrayEquals(expected, Protocol.encode(Message.Ping(1, 2)))
    }

    @Test
    fun controlMessagesRoundTrip() {
        for (message in listOf(
            Message.Welcome(1, 1280, 800),
            Message.Reject(RejectReason.NOT_PAIRED, "pair first ✓"),
            Message.Ping(4_000_000_000, Long.MAX_VALUE),
            Message.Pong(9, 123),
            Message.RequestKeyframe,
        )) {
            assertEquals(message, roundTrip(message))
        }
    }

    @Test
    fun helloRoundTripsWithAndWithoutToken() {
        val token = ByteArray(16) { 7 }
        val hello = roundTrip(Message.Hello(1, token, "Idea Tab", display)) as Message.Hello
        assertArrayEquals(token, hello.pairingToken)
        assertEquals("Idea Tab", hello.deviceName)
        assertEquals(display, hello.display)
        val plain = roundTrip(Message.Hello(1, null, "", display)) as Message.Hello
        assertNull(plain.pairingToken)
    }

    @Test
    fun videoRoundTrips() {
        val data = byteArrayOf(0, 0, 0, 1, 0x67, 0x42)
        val video = roundTrip(Message.Video(-1, true, data)) as Message.Video
        assertTrue(video.keyframe)
        assertEquals(-1L, video.ptsUs) // u64::MAX on the wire
        assertArrayEquals(data, video.data)
    }

    @Test
    fun longDeviceNamesAreCutToTheLimit() {
        val hello = roundTrip(Message.Hello(1, null, "x".repeat(200), display)) as Message.Hello
        assertEquals(Protocol.MAX_NAME, hello.deviceName.length)
    }

    @Test
    fun oversizedLengthIsRefusedBeforeReading() {
        // Claims a 1 GiB welcome; only the header is present.
        val bytes = byteArrayOf(0x40, 0, 0, 0, 2)
        assertThrows(ProtocolException::class.java) {
            Protocol.read(DataInputStream(ByteArrayInputStream(bytes)))
        }
    }

    @Test
    fun everyTruncationIsAnError() {
        val body = Protocol.encode(Message.Welcome(1, 1280, 800)).copyOfRange(4, 16)
        for (cut in 0 until body.size) {
            assertThrows(Exception::class.java) { Protocol.decode(body.copyOf(cut)) }
        }
    }

    @Test
    fun malformedValuesAreRejected() {
        val unknownTag = byteArrayOf(99)
        val badCodec = byteArrayOf(2, 0, 1, 9, 0, 0, 0, 1, 0, 0, 0, 1)
        val trailing = byteArrayOf(7, 0)
        val badUtf8 = byteArrayOf(3, 2, 0, 2, 0xff.toByte(), 0xfe.toByte())
        for (body in listOf(unknownTag, badCodec, trailing, badUtf8)) {
            assertThrows(ProtocolException::class.java) { Protocol.decode(body) }
        }
    }
}
