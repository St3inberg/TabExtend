package io.github.st3inberg.tabextend

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.DataInputStream

class InputTest {
    @Test
    fun touchMatchesTheRustWireFormat() {
        // Same vector as `known_bytes_for_touch` in crates/core/src/protocol.rs.
        val expected = byteArrayOf(0, 0, 0, 11, 10, 1, 0, 0x3f, 0, 0, 0, 0x3f, 0x80.toByte(), 0, 0)
        assertArrayEquals(expected, Protocol.encode(Message.Touch(1, TouchPhase.DOWN, 0.5f, 1.0f)))
    }

    @Test
    fun inputMessagesRoundTrip() {
        for (message in listOf(
            Message.Touch(9, TouchPhase.CANCEL, 0.25f, 0f),
            Message.PointerMove(1f, 0.5f),
            Message.Button(MouseButton.RIGHT, true),
            Message.Scroll(-2, 3),
            Message.Key(30, false),
        )) {
            val decoded = Protocol.read(DataInputStream(ByteArrayInputStream(Protocol.encode(message))))
            assertEquals(message, decoded)
        }
    }

    @Test
    fun slotsAreSmallReusedAndBounded() {
        val slots = SlotTable(2)
        assertEquals(0, slots.acquire(1000))
        assertEquals(1, slots.acquire(7))
        assertNull("only two slots", slots.acquire(8))
        assertEquals(0, slots.acquire(1000)) // same finger, same slot
        assertEquals(0, slots.release(1000))
        assertEquals(0, slots.acquire(8)) // freed slot is reused
        assertEquals(listOf(0, 1), slots.releaseAll().sorted())
        assertNull(slots.slotOf(7))
    }
}
