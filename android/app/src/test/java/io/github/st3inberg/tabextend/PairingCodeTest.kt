package io.github.st3inberg.tabextend

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.DataInputStream

class PairingCodeTest {
    private fun bytes(value: Int, size: Int = 32) = ByteArray(size) { value.toByte() }

    @Test
    fun matchesTheRustVectors() {
        // Same vectors as `confirmation_code_known_vector` in crates/core/src/pairing.rs.
        val code = PairingCode.code(bytes(1), bytes(2), bytes(3), bytes(4))
        assertEquals("778 062", PairingCode.format(code))
        assertEquals(
            "0db0815fb87772ab13658c18214590e48ab9889ab1aa2f09db39ec051393bb7d",
            hex(PairingCode.commitment(bytes(5))),
        )
    }

    @Test
    fun codeDependsOnEveryInput() {
        val reference = PairingCode.code(bytes(1), bytes(2), bytes(3), bytes(4))
        assertNotEquals(reference, PairingCode.code(bytes(9), bytes(2), bytes(3), bytes(4)))
        assertNotEquals(reference, PairingCode.code(bytes(1), bytes(9), bytes(3), bytes(4)))
        assertNotEquals(reference, PairingCode.code(bytes(1), bytes(2), bytes(9), bytes(4)))
        assertNotEquals(reference, PairingCode.code(bytes(1), bytes(2), bytes(3), bytes(9)))
    }

    @Test
    fun formatAlwaysShowsSixDigits() {
        assertEquals("000 042", PairingCode.format(42))
        assertEquals("999 999", PairingCode.format(999_999))
    }

    @Test
    fun confirmHelloAndExchangeRoundTrip() {
        val commitment = bytes(9)
        val hello = Message.Hello(1, PairingRequest.Confirm(commitment), "tab", DisplayInfo(1, 2, 3, 4))
        val decoded = Protocol.read(DataInputStream(ByteArrayInputStream(Protocol.encode(hello)))) as Message.Hello
        assertArrayEquals(commitment, (decoded.pairing as PairingRequest.Confirm).commitment)
        for (message in listOf(Message.PairChallenge(bytes(1)), Message.PairReveal(bytes(2)))) {
            val again = Protocol.read(DataInputStream(ByteArrayInputStream(Protocol.encode(message))))
            assertEquals(message.javaClass, again.javaClass)
        }
    }
}
