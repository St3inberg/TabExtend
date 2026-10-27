package io.github.st3inberg.tabextend

import java.security.MessageDigest
import java.security.SecureRandom

/**
 * Pairing by code. Mirrors `commitment` and `confirmation_code` in
 * `crates/core/src/pairing.rs`; keep the domain strings in step.
 *
 * The tablet commits to a secret nonce before it sees the host's, so a
 * machine in the middle cannot steer both screens to the same code: it
 * gets one blind guess in a million. The code covers both certificates and
 * both nonces.
 */
object PairingCode {
    fun newNonce(): ByteArray = ByteArray(Protocol.NONCE_LEN).also { SecureRandom().nextBytes(it) }

    fun commitment(nonce: ByteArray): ByteArray = sha256 {
        update("tabextend commitment v1".toByteArray())
        update(nonce)
    }

    fun code(host: ByteArray, client: ByteArray, hostNonce: ByteArray, clientNonce: ByteArray): Int {
        val digest = sha256 {
            update("tabextend confirmation v1".toByteArray())
            update(host)
            update(client)
            update(hostNonce)
            update(clientNonce)
        }
        val first = (digest[0].toLong() and 0xffL shl 24) or (digest[1].toLong() and 0xffL shl 16) or
            (digest[2].toLong() and 0xffL shl 8) or (digest[3].toLong() and 0xffL)
        return (first % 1_000_000).toInt()
    }

    /** `123 456`, as the computer shows it. */
    fun format(code: Int): String = "%03d %03d".format(code / 1000, code % 1000)

    private fun sha256(block: MessageDigest.() -> Unit): ByteArray =
        MessageDigest.getInstance("SHA-256").apply(block).digest()
}
