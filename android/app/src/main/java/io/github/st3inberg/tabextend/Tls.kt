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

    /** Connect to a paired host; only its pinned certificate is accepted. */
    fun connect(host: Host): SSLSocket = open(host.address, host.port, PinnedHost(host.fingerprint))

    /**
     * Connect to a host found by scanning, whose certificate is not known
     * yet. The certificate is recorded, not trusted: its fingerprint goes
     * into the pairing code, so a swapped certificate shows up as codes that
     * do not match. Returns the socket and the host's fingerprint.
     */
    fun connectForPairing(address: String, port: Int): Pair<SSLSocket, ByteArray> {
        val recorder = RecordingHost()
        val socket = open(address, port, recorder)
        return socket to (recorder.fingerprint ?: throw CertificateException("host sent no certificate"))
    }

    private fun open(address: String, port: Int, trust: X509TrustManager): SSLSocket {
        ensureClientKey()
        val context = SSLContext.getInstance("TLSv1.3").apply {
            init(arrayOf(KeystoreKey(keyStore())), arrayOf(trust), null)
        }
        val tcp = Socket().apply {
            tcpNoDelay = true
            connect(InetSocketAddress(address.trim('[', ']'), port), CONNECT_TIMEOUT_MS)
        }
        val socket = context.socketFactory.createSocket(tcp, address, port, true) as SSLSocket
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
     * Records the host's certificate for pairing by code. Lint warns about
     * trust managers that accept anything; here the fingerprint is checked
     * by the user, through the code shown on both screens, before the host
     * is trusted, and only pinned hosts are used afterwards.
     */
    @SuppressLint("CustomX509TrustManager", "TrustAllX509TrustManager")
    private class RecordingHost : X509TrustManager {
        @Volatile var fingerprint: ByteArray? = null

        override fun checkServerTrusted(chain: Array<out X509Certificate>, authType: String) {
            val leaf = chain.firstOrNull() ?: throw CertificateException("host sent no certificate")
            fingerprint = MessageDigest.getInstance("SHA-256").digest(leaf.encoded)
        }

        override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String) =
            throw CertificateException("not a server")

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
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
