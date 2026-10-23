package io.github.st3inberg.tabextend

import android.content.Context

/** A host to connect to: where it is and the certificate it must present. */
data class Host(val address: String, val port: Int, val fingerprint: ByteArray) {
    override fun equals(other: Any?) = other is Host && address == other.address &&
        port == other.port && fingerprint.contentEquals(other.fingerprint)

    override fun hashCode() = 31 * (31 * address.hashCode() + port) + fingerprint.contentHashCode()
}

/**
 * What the host's QR code carries. Mirrors `PairingLink` in
 * `crates/core/src/pairing.rs`, including what counts as malformed.
 */
data class PairingLink(val host: Host, val token: ByteArray) {
    companion object {
        private const val SCHEME = "tabextend://pair?"

        fun parse(uri: String): PairingLink? {
            if (!uri.startsWith(SCHEME)) return null
            val fields = mutableMapOf<String, String>()
            for (pair in uri.removePrefix(SCHEME).split('&')) {
                val eq = pair.indexOf('=')
                if (eq < 0) return null
                val key = pair.substring(0, eq)
                if (key !in setOf("host", "port", "fp", "token")) continue
                if (fields.put(key, pair.substring(eq + 1)) != null) return null
            }
            val address = fields["host"]?.takeIf { h ->
                h.isNotEmpty() && h.all { it.isLetterOrDigit() && it.code < 128 || it in ".:-[]" }
            } ?: return null
            val port = fields["port"]?.toIntOrNull()?.takeIf { it in 1..65535 } ?: return null
            val fingerprint = fields["fp"]?.let(::unhex)?.takeIf { it.size == 32 } ?: return null
            val token = fields["token"]?.let(::unhex)
                ?.takeIf { it.size == Protocol.PAIRING_TOKEN_LEN } ?: return null
            return PairingLink(Host(address, port, fingerprint), token)
        }
    }
}

fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }

fun fingerprintText(bytes: ByteArray) = bytes.joinToString(":") { "%02x".format(it) }

fun unhex(text: String): ByteArray? {
    if (text.length % 2 != 0) return null
    return ByteArray(text.length / 2) { i ->
        text.substring(2 * i, 2 * i + 2).toIntOrNull(16)?.toByte() ?: return null
    }
}

/** The one host this tablet is paired with. */
class HostStore(context: Context) {
    private val prefs = context.getSharedPreferences("host", Context.MODE_PRIVATE)

    fun load(): Host? {
        val address = prefs.getString("address", null) ?: return null
        val fingerprint = prefs.getString("fingerprint", null)?.let(::unhex) ?: return null
        return Host(address, prefs.getInt("port", 0), fingerprint)
    }

    fun save(host: Host) {
        prefs.edit()
            .putString("address", host.address)
            .putInt("port", host.port)
            .putString("fingerprint", hex(host.fingerprint))
            .apply()
    }

    fun clear() = prefs.edit().clear().apply()
}
