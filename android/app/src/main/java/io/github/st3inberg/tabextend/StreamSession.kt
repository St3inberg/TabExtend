package io.github.st3inberg.tabextend

import android.view.Surface
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.IOException
import java.io.OutputStream
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLSocket

/** Who to connect to, and how this tablet proves it may. */
sealed class Target {
    abstract val address: String
    abstract val port: Int

    /** Paired before: both certificates are pinned. */
    data class Paired(val host: Host) : Target() {
        override val address get() = host.address
        override val port get() = host.port
    }

    /** From a QR code or pairing link: certificate pinned, single-use token. */
    class WithToken(val host: Host, val token: ByteArray) : Target() {
        override val address get() = host.address
        override val port get() = host.port
    }

    /** Found by scanning: pair by comparing a code shown on both screens. */
    data class ByCode(override val address: String, override val port: Int, val name: String) : Target()
}

/**
 * One tablet's connection to its host, on a background thread: connect,
 * say hello, decode video until the connection ends, then reconnect.
 *
 * Gives up (no more retries) only when retrying cannot help: the host
 * does not know this tablet, pairing was refused, or the versions do not
 * match. Pairing by code never retries on its own, since each attempt
 * puts a question on the computer's screen.
 */
class StreamSession(
    @Volatile private var target: Target,
    private val deviceName: String,
    private val display: DisplayInfo,
    private val surface: Surface,
    private val listener: Listener,
) {
    interface Listener {
        fun onStatus(text: String)
        fun onStreaming(width: Int, height: Int, decoder: String, lowLatency: Boolean)
        fun onPaired(host: Host)
        fun onConfirmCode(code: String, computer: String)
        fun onStats(fps: Double, mbps: Double, decodeMs: Double)
        fun onGaveUp(reason: String)
    }

    @Volatile private var running = true
    @Volatile private var socket: SSLSocket? = null
    @Volatile private var streaming = false

    /** Everything sent while streaming goes through here, to one writer thread. */
    private val outbox = LinkedBlockingQueue<Message>(OUTBOX_CAPACITY)
    private val thread = Thread(::run, "tabextend-session")

    fun start() = thread.start()

    /** Queue input for the host; dropped when not streaming or if the link is backed up. */
    fun sendInput(messages: List<Message>) {
        if (!streaming) return
        for (message in messages) outbox.offer(message)
    }

    fun stop() {
        running = false
        runCatching { socket?.close() }
        thread.interrupt()
    }

    private fun run() {
        var backoffMs = 500L
        while (running) {
            val outcome = try {
                connectOnce()
            } catch (e: Exception) {
                if (!running) return
                val why = e.message ?: e.javaClass.simpleName
                if (target is Target.ByCode) {
                    Outcome.GiveUp("Pairing failed: $why")
                } else {
                    listener.onStatus(if (e is IOException) "Connection lost: $why" else "Error: $why")
                    Outcome.Retry
                }
            }
            when (outcome) {
                is Outcome.GiveUp -> {
                    running = false
                    listener.onGaveUp(outcome.reason)
                    return
                }
                Outcome.Streamed -> backoffMs = 500L
                Outcome.Retry -> {}
            }
            try {
                Thread.sleep(backoffMs)
            } catch (e: InterruptedException) {
                return
            }
            backoffMs = (backoffMs * 2).coerceAtMost(MAX_BACKOFF_MS)
            listener.onStatus("Reconnecting to ${target.address}…")
        }
    }

    private sealed class Outcome {
        data object Streamed : Outcome()
        data object Retry : Outcome()
        data class GiveUp(val reason: String) : Outcome()
    }

    private fun connectOnce(): Outcome {
        val current = target
        listener.onStatus("Connecting to ${current.address}…")
        val (tls, hostFingerprint) = when (current) {
            is Target.Paired -> Tls.connect(current.host) to current.host.fingerprint
            is Target.WithToken -> Tls.connect(current.host) to current.host.fingerprint
            is Target.ByCode -> Tls.connectForPairing(current.address, current.port)
        }
        socket = tls
        tls.use {
            val output = BufferedOutputStream(tls.outputStream)
            val input = DataInputStream(BufferedInputStream(tls.inputStream, 256 * 1024))
            val nonce = PairingCode.newNonce()
            val request = when (current) {
                is Target.Paired -> PairingRequest.None
                is Target.WithToken -> PairingRequest.Token(current.token)
                is Target.ByCode -> PairingRequest.Confirm(PairingCode.commitment(nonce))
            }
            send(output, Message.Hello(Protocol.VERSION, request, deviceName, display))

            var first = Protocol.read(input)
            if (current is Target.ByCode && first is Message.PairChallenge) {
                send(output, Message.PairReveal(nonce))
                val code = PairingCode.code(hostFingerprint, Tls.clientFingerprint(), first.nonce, nonce)
                listener.onConfirmCode(PairingCode.format(code), current.name)
                first = Protocol.read(input) // the user has up to 90 s to answer
            }
            val welcome = when (first) {
                is Message.Welcome -> first
                is Message.Reject -> return rejected(first)
                else -> throw ProtocolException("expected welcome, got ${first.javaClass.simpleName}")
            }
            if (current !is Target.Paired) {
                // Paired now; reconnects go by certificate.
                val host = Host(current.address, current.port, hostFingerprint)
                target = Target.Paired(host)
                listener.onPaired(host)
            }
            val width = welcome.width.toInt()
            val height = welcome.height.toInt()
            val decoder = VideoDecoder(width, height, surface)
            outbox.clear()
            val writer = Thread({ writeLoop(output) }, "tabextend-writer").apply { start() }
            streaming = true
            listener.onStreaming(width, height, decoder.name, decoder.lowLatency || decoder.vendorLowLatency.isNotEmpty())
            try {
                stream(input, decoder)
            } finally {
                streaming = false
                writer.interrupt()
                outbox.clear()
                decoder.release()
            }
            return Outcome.Streamed
        }
    }

    /** Write queued messages, flushing whenever the queue runs dry. */
    private fun writeLoop(output: OutputStream) {
        try {
            while (true) {
                val message = outbox.poll(200, TimeUnit.MILLISECONDS) ?: continue
                output.write(Protocol.encode(message))
                if (outbox.isEmpty()) output.flush()
            }
        } catch (e: InterruptedException) {
            return
        } catch (e: IOException) {
            runCatching { socket?.close() } // the reader notices and reconnects
        }
    }

    private fun stream(input: DataInputStream, decoder: VideoDecoder) {
        var waitingForKeyframe = true
        var windowStart = System.nanoTime()
        var windowFrames = decoder.framesRendered
        var windowBytes = 0L
        while (running) {
            when (val message = Protocol.read(input)) {
                is Message.Video -> {
                    windowBytes += message.data.size
                    if (waitingForKeyframe && !message.keyframe) continue
                    if (decoder.submit(message.data, message.ptsUs, message.keyframe)) {
                        waitingForKeyframe = false
                    } else if (!waitingForKeyframe) {
                        waitingForKeyframe = true
                        outbox.offer(Message.RequestKeyframe)
                    }
                }
                is Message.Ping -> outbox.offer(Message.Pong(message.id, message.sentUs))
                is Message.Reject -> throw IOException(message.detail)
                else -> {}
            }
            val now = System.nanoTime()
            val elapsed = (now - windowStart) / 1e9
            if (elapsed >= 1.0) {
                listener.onStats(
                    (decoder.framesRendered - windowFrames) / elapsed,
                    windowBytes * 8 / elapsed / 1e6,
                    decoder.takeAverageDecodeMs(),
                )
                windowStart = now
                windowFrames = decoder.framesRendered
                windowBytes = 0
            }
        }
    }

    private fun rejected(reject: Message.Reject): Outcome = when (reject.reason) {
        RejectReason.NOT_PAIRED, RejectReason.BAD_PAIRING_TOKEN, RejectReason.UNSUPPORTED_VERSION,
        RejectReason.PAIRING_DENIED ->
            Outcome.GiveUp(reject.detail)
        RejectReason.BUSY, RejectReason.CAPTURE_FAILED -> {
            listener.onStatus(reject.detail)
            Outcome.Retry
        }
    }

    private fun send(output: OutputStream, message: Message) {
        output.write(Protocol.encode(message))
        output.flush()
    }

    private companion object {
        const val MAX_BACKOFF_MS = 5000L
        const val OUTBOX_CAPACITY = 1024
    }
}
