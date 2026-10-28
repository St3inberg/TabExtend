package io.github.st3inberg.tabextend

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RelocateTest {
    private val fingerprint = ByteArray(32) { 0xab.toByte() }
    private val host = Host("192.168.1.10", 47420, fingerprint)

    private fun found(id: String, address: String, port: Int = 47420) =
        FoundComputer("fedora", address, port, id, pairing = false)

    @Test
    fun idMatchesTheHostsAnnouncement() {
        // Same value as the host_id test in crates/host/src/mdns.rs.
        assertEquals("abababababababab", hostId(fingerprint))
    }

    @Test
    fun followsTheHostToANewAddress() {
        val moved = relocated(host, listOf(found("1111111111111111", "192.168.1.5"), found("abababababababab", "192.168.1.23")))
        assertEquals(Host("192.168.1.23", 47420, fingerprint), moved)
    }

    @Test
    fun followsANewPort() {
        assertEquals(47421, relocated(host, listOf(found("abababababababab", "192.168.1.10", 47421)))?.port)
    }

    @Test
    fun nothingToDoWhenTheAddressIsTheSame() {
        assertNull(relocated(host, listOf(found("abababababababab", "192.168.1.10"))))
    }

    @Test
    fun ignoresOtherComputers() {
        assertNull(relocated(host, listOf(found("1111111111111111", "192.168.1.23"))))
    }
}
