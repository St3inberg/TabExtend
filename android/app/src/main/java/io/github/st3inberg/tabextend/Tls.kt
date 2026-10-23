package io.github.st3inberg.tabextend

import android.annotation.SuppressLint
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.net.InetSocketAddress
import java.net.Socket
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.Principal
import java.security.PrivateKey
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.security.spec.ECGenParameterSpec
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509TrustManager
import javax.security.auth.x500.X500Principal

/**
 * TLS to the host: TLS 1.3 only, the host pinned by certificate fingerprint,
 * this tablet identified by a key in Android Keystore.
 *
 * Keystore generates the key inside secure hardware where available and
 * issues a self-signed certificate for it. The private key never leaves the
 * device; the host remembers the certificate's fingerprint when pairing.
 */
object Tls {
    private const val KEY_ALIAS = "tabextend-client"
    private const val CONNECT_TIMEOUT_MS = 5000

    private fun keyStore(): KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    private fun ensureClientKey() {
        if (keyStore().containsAlias(KEY_ALIAS)) return
        val spec = KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256, KeyProperties.DIGEST_SHA384, KeyProperties.DIGEST_NONE)
            .setCertificateSubject(X500Principal("CN=TabExtend tablet"))
            .build()
        KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore").run {
            initialize(spec)
            generateKeyPair()
        }
    }

    /** SHA-256 of this tablet's certificate, as the host shows it. */
    fun clientFingerprint(): ByteArray {
        ensureClientKey()
        val cert = keyStore().getCertificate(KEY_ALIAS)
        return MessageDigest.getInstance("SHA-256").digest(cert.encoded)
    }

    fun connect(host: Host): SSLSocket {
        ensureClientKey()
        val context = SSLContext.getInstance("TLSv1.3").apply {
            init(arrayOf(KeystoreKey(keyStore())), arrayOf(PinnedHost(host.fingerprint)), null)
        }
        val tcp = Socket().apply {
            tcpNoDelay = true
            connect(InetSocketAddress(host.address.trim('[', ']'), host.port), CONNECT_TIMEOUT_MS)
        }
        val socket = context.socketFactory.createSocket(tcp, host.address, host.port, true) as SSLSocket
        socket.enabledProtocols = arrayOf("TLSv1.3")
        socket.sslParameters = socket.sslParameters.apply {
            applicationProtocols = arrayOf("tabextend/1")
        }
        socket.startHandshake()
        return socket
    }

    /** Offers the one Keystore key as this tablet's client certificate. */
    private class KeystoreKey(private val store: KeyStore) : X509ExtendedKeyManager() {
        override fun chooseClientAlias(types: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?) =
            KEY_ALIAS

        override fun chooseEngineClientAlias(types: Array<out String>?, issuers: Array<out Principal>?, engine: SSLEngine?) =
            KEY_ALIAS

        override fun getCertificateChain(alias: String?): Array<X509Certificate> =
            arrayOf(store.getCertificate(KEY_ALIAS) as X509Certificate)

        override fun getPrivateKey(alias: String?): PrivateKey = store.getKey(KEY_ALIAS, null) as PrivateKey

        override fun getClientAliases(type: String?, issuers: Array<out Principal>?) = arrayOf(KEY_ALIAS)

        override fun getServerAliases(type: String?, issuers: Array<out Principal>?): Array<String>? = null

        override fun chooseServerAlias(type: String?, issuers: Array<out Principal>?, socket: Socket?): String? = null
    }

    /**
     * Accepts exactly one certificate: the one whose fingerprint was in the
     * QR code. Lint warns about custom trust managers because they often
     * accept everything; this one is stricter than the default, not looser.
     */
    @SuppressLint("CustomX509TrustManager")
    private class PinnedHost(private val pin: ByteArray) : X509TrustManager {
        override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String) {
            val leaf = chain.firstOrNull() ?: throw CertificateException("host sent no certificate")
            val actual = MessageDigest.getInstance("SHA-256").digest(leaf.encoded)
            if (!MessageDigest.isEqual(actual, pin)) {
                throw CertificateException("host certificate does not match the paired fingerprint")
            }
        }

        override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String) =
            throw CertificateException("not a server")

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }
}
