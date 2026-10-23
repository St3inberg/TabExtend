package io.github.st3inberg.tabextend

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Test

class PairingLinkTest {
    private val good = "tabextend://pair?host=10.134.124.2&port=47420&fp=" + "ab".repeat(32) +
        "&token=" + "01".repeat(16)

    @Test
    fun parsesTheHostsLink() {
        val link = PairingLink.parse(good)!!
        assertEquals("10.134.124.2", link.host.address)
        assertEquals(47420, link.host.port)
        assertEquals(32, link.host.fingerprint.size)
        assertEquals(16, link.token.size)
    }

    @Test
    fun acceptsIpv6AndIgnoresUnknownKeys() {
        val link = PairingLink.parse(good.replace("10.134.124.2", "[fe80::1]") + "&v=2")
        assertEquals("[fe80::1]", link!!.host.address)
    }

    @Test
    fun rejectsTheSameMalformedLinksAsTheHost() {
        // Mirrors `malformed_links_are_rejected` in crates/core/src/pairing.rs.
        for (bad in listOf(
            good.replace("tabextend://", "https://"),
            good.replace("port=47420", "port=0"),
            good.replace("port=47420", "port=99999"),
            good.replace("&fp=", "&fp=zz"),
            good.replace("&token=", "&token=0"),
            good.replace("10.134.124.2", "evil host/"),
            good.replace("10.134.124.2", ""),
            "$good&token=00",
            good.replace("&fp=", "&nofp="),
        )) {
            assertNull("accepted $bad", PairingLink.parse(bad))
        }
        assertNotNull(PairingLink.parse(good))
    }

    @Test
    fun fingerprintTextMatchesTheHostsFormat() {
        assertEquals("e3:e3", fingerprintText(byteArrayOf(0xe3.toByte(), 0xe3.toByte())))
    }
}
