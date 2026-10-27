package io.github.st3inberg.tabextend

import java.io.ByteArrayOutputStream
import java.io.DataInputStream
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.nio.charset.StandardCharsets

/**
 * The TabExtend wire protocol, spoken inside TLS.
 *
 * Mirrors `crates/core/src/protocol.rs`; keep the two in step. Every message
 * is `[length: u32 BE][tag: u8][payload]`, `length` counting tag and payload.
 * The host is untrusted too: lengths are checked against per-tag limits
 * before anything is allocated.
 */
object Protocol {
    const val VERSION = 1
    const val MAX_VIDEO_FRAME = 16 * 1024 * 1024
    const val MAX_CONTROL_MESSAGE = 4096
    const val MAX_NAME = 64
    const val PAIRING_TOKEN_LEN = 16
    const val NONCE_LEN = 32
    private const val HEADER_LEN = 4

    private const val HELLO = 1
    private const val WELCOME = 2
    private const val REJECT = 3
    private const val VIDEO = 4
    private const val PING = 5
    private const val PONG = 6
    private const val REQUEST_KEYFRAME = 7
    private const val PAIR_CHALLENGE = 8
    private const val PAIR_REVEAL = 9
    private const val TOUCH = 10
    private const val POINTER_MOVE = 11
    private const val BUTTON = 12
    private const val SCROLL = 13
    private const val KEY = 14

    fun maxLength(tag: Int): Int = when (tag) {
        VIDEO -> 1 + MAX_VIDEO_FRAME + 9
        HELLO, WELCOME, REJECT, PING, PONG, REQUEST_KEYFRAME, PAIR_CHALLENGE, PAIR_REVEAL,
        TOUCH, POINTER_MOVE, BUTTON, SCROLL, KEY -> MAX_CONTROL_MESSAGE
        else -> throw ProtocolException("unknown message tag $tag")
    }

    fun encode(message: Message): ByteArray {
        val body = Writer()
        when (message) {
            is Message.Hello -> {
                body.u8(HELLO)
                body.u16(message.version)
                when (val pairing = message.pairing) {
                    PairingRequest.None -> body.u8(0)
                    is PairingRequest.Token -> {
                        require(pairing.token.size == PAIRING_TOKEN_LEN)
                        body.u8(1)
                        body.bytes(pairing.token)
                    }
                    is PairingRequest.Confirm -> {
                        require(pairing.commitment.size == NONCE_LEN)
                        body.u8(2)
                        body.bytes(pairing.commitment)
                    }
                }
                body.string(message.deviceName.take(MAX_NAME))
                body.u32(message.display.width)
                body.u32(message.display.height)
                body.u32(message.display.densityDpi)
                body.u32(message.display.refreshMillihz)
            }
            is Message.Welcome -> {
                body.u8(WELCOME)
                body.u16(message.version)
                body.u8(1)
                body.u32(message.width)
                body.u32(message.height)
            }
            is Message.Reject -> {
                body.u8(REJECT)
                body.u8(message.reason.code)
                body.string(message.detail)
            }
            is Message.Video -> {
                body.u8(VIDEO)
                body.u64(message.ptsUs)
                body.u8(if (message.keyframe) 1 else 0)
                body.bytes(message.data)
            }
            is Message.Ping -> {
                body.u8(PING)
                body.u32(message.id)
                body.u64(message.sentUs)
            }
            is Message.Pong -> {
                body.u8(PONG)
                body.u32(message.id)
                body.u64(message.sentUs)
            }
            Message.RequestKeyframe -> body.u8(REQUEST_KEYFRAME)
            is Message.PairChallenge -> {
                body.u8(PAIR_CHALLENGE)
                body.bytes(message.nonce)
            }
            is Message.PairReveal -> {
                body.u8(PAIR_REVEAL)
                body.bytes(message.nonce)
            }
            is Message.Touch -> {
                body.u8(TOUCH)
                body.u8(message.slot)
                body.u8(message.phase.code)
                body.f32(message.x)
                body.f32(message.y)
            }
            is Message.PointerMove -> {
                body.u8(POINTER_MOVE)
                body.f32(message.x)
                body.f32(message.y)
            }
            is Message.Button -> {
                body.u8(BUTTON)
                body.u8(message.button.code)
                body.u8(if (message.pressed) 1 else 0)
            }
            is Message.Scroll -> {
                body.u8(SCROLL)
                body.u16(message.horizontal)
                body.u16(message.vertical)
            }
            is Message.Key -> {
                body.u8(KEY)
                body.u16(message.code)
                body.u8(if (message.pressed) 1 else 0)
            }
        }
        val bytes = body.toByteArray()
        return ByteBuffer.allocate(HEADER_LEN + bytes.size).putInt(bytes.size).put(bytes).array()
    }

    /** Read one message, enforcing the per-tag size limit before allocating. */
    fun read(input: DataInputStream): Message {
        val length = input.readInt().toLong() and 0xffff_ffffL
        val tag = input.readUnsignedByte()
        if (length == 0L || length > maxLength(tag)) {
            throw ProtocolException("message tag $tag has invalid length $length")
        }
        val body = ByteArray(length.toInt())
        body[0] = tag.toByte()
        input.readFully(body, 1, body.size - 1)
        return decode(body)
    }

    /** Decode a message body: the bytes after the length prefix, tag first. */
    fun decode(body: ByteArray): Message {
        val r = Reader(body)
        val tag = r.u8()
        if (body.size > maxLength(tag)) throw ProtocolException("message too long")
        val message = when (tag) {
            HELLO -> {
                val version = r.u16()
                val pairing = when (val flag = r.u8()) {
                    0 -> PairingRequest.None
                    1 -> PairingRequest.Token(r.bytes(PAIRING_TOKEN_LEN))
                    2 -> PairingRequest.Confirm(r.bytes(NONCE_LEN))
                    else -> throw ProtocolException("unknown pairing mode $flag")
                }
                Message.Hello(
                    version, pairing, r.string(MAX_NAME),
                    DisplayInfo(r.u32(), r.u32(), r.u32(), r.u32()),
                )
            }
            WELCOME -> {
                val version = r.u16()
                val codec = r.u8()
                if (codec != 1) throw ProtocolException("unknown codec $codec")
                Message.Welcome(version, r.u32(), r.u32())
            }
            REJECT -> {
                val code = r.u8()
                val reason = RejectReason.entries.firstOrNull { it.code == code }
                    ?: throw ProtocolException("unknown reject reason $code")
                Message.Reject(reason, r.string(MAX_CONTROL_MESSAGE))
            }
            VIDEO -> {
                val pts = r.u64()
                val keyframe = when (val flag = r.u8()) {
                    0 -> false
                    1 -> true
                    else -> throw ProtocolException("unknown keyframe flag $flag")
                }
                Message.Video(pts, keyframe, r.rest())
            }
            PING -> Message.Ping(r.u32(), r.u64())
            PONG -> Message.Pong(r.u32(), r.u64())
            REQUEST_KEYFRAME -> Message.RequestKeyframe
            PAIR_CHALLENGE -> Message.PairChallenge(r.bytes(NONCE_LEN))
            PAIR_REVEAL -> Message.PairReveal(r.bytes(NONCE_LEN))
            TOUCH -> {
                val slot = r.u8()
                val code = r.u8()
                val phase = TouchPhase.entries.firstOrNull { it.code == code }
                    ?: throw ProtocolException("unknown touch phase $code")
                Message.Touch(slot, phase, r.f32(), r.f32())
            }
            POINTER_MOVE -> Message.PointerMove(r.f32(), r.f32())
            BUTTON -> {
                val code = r.u8()
                val button = MouseButton.entries.firstOrNull { it.code == code }
                    ?: throw ProtocolException("unknown mouse button $code")
                Message.Button(button, r.flag())
            }
            SCROLL -> Message.Scroll(r.u16().toShort().toInt(), r.u16().toShort().toInt())
            KEY -> Message.Key(r.u16(), r.flag())
            else -> throw ProtocolException("unknown message tag $tag")
        }
        if (r.remaining() != 0) throw ProtocolException("message has trailing bytes")
        return message
    }

    private class Writer : ByteArrayOutputStream() {
        fun u8(v: Int) = write(v)
        fun u16(v: Int) {
            write(v ushr 8); write(v)
        }
        fun u32(v: Long) {
            for (shift in intArrayOf(24, 16, 8, 0)) write((v ushr shift).toInt())
        }
        fun u64(v: Long) {
            for (shift in 56 downTo 0 step 8) write((v ushr shift).toInt())
        }
        fun f32(v: Float) = u32(java.lang.Float.floatToRawIntBits(v).toLong() and 0xffff_ffffL)
        fun bytes(v: ByteArray) = write(v, 0, v.size)
        fun string(s: String) {
            val bytes = s.toByteArray(StandardCharsets.UTF_8)
            require(bytes.size <= 0xffff)
            u16(bytes.size)
            bytes(bytes)
        }
    }

    private class Reader(private val data: ByteArray) {
        private var pos = 0
        fun remaining() = data.size - pos
        private fun need(n: Int) {
            if (remaining() < n) throw ProtocolException("message ended early")
        }
        fun u8(): Int {
            need(1); return data[pos++].toInt() and 0xff
        }
        fun u16(): Int = (u8() shl 8) or u8()
        fun f32(): Float = java.lang.Float.intBitsToFloat(u32().toInt())
        fun flag(): Boolean = when (val v = u8()) {
            0 -> false
            1 -> true
            else -> throw ProtocolException("unknown flag value $v")
        }
        fun u32(): Long {
            need(4)
            var v = 0L
            repeat(4) { v = (v shl 8) or (data[pos++].toLong() and 0xff) }
            return v
        }
        fun u64(): Long {
            need(8)
            var v = 0L
            repeat(8) { v = (v shl 8) or (data[pos++].toLong() and 0xff) }
            return v
        }
        fun bytes(n: Int): ByteArray {
            need(n)
            return data.copyOfRange(pos, pos + n).also { pos += n }
        }
        fun string(max: Int): String {
            val len = u16()
            if (len > max) throw ProtocolException("string is too long")
            val decoder = StandardCharsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
            return try {
                decoder.decode(ByteBuffer.wrap(bytes(len))).toString()
            } catch (e: CharacterCodingException) {
                throw ProtocolException("string is not UTF-8")
            }
        }
        fun rest(): ByteArray = bytes(remaining())
    }
}

class ProtocolException(message: String) : IOException(message)

/** Integers the protocol sends as u32 are held in Long so all values fit. */
data class DisplayInfo(
    val width: Long,
    val height: Long,
    val densityDpi: Long,
    val refreshMillihz: Long,
)

/** How a tablet asks to be trusted, if it is not paired yet. */
sealed class PairingRequest {
    data object None : PairingRequest()

    /** The single-use token from the host's QR code. */
    class Token(val token: ByteArray) : PairingRequest()

    /** Pairing by code: a commitment to the tablet's secret nonce. See [PairingCode]. */
    class Confirm(val commitment: ByteArray) : PairingRequest()
}

enum class TouchPhase(val code: Int) { DOWN(0), MOVE(1), UP(2), CANCEL(3) }

enum class MouseButton(val code: Int) { LEFT(1), MIDDLE(2), RIGHT(3), BACK(4), FORWARD(5) }

enum class RejectReason(val code: Int) {
    UNSUPPORTED_VERSION(1),
    NOT_PAIRED(2),
    BAD_PAIRING_TOKEN(3),
    BUSY(4),
    CAPTURE_FAILED(5),
    PAIRING_DENIED(6),
}

sealed class Message {
    class Hello(
        val version: Int,
        val pairing: PairingRequest,
        val deviceName: String,
        val display: DisplayInfo,
    ) : Message()

    /** Host → tablet during code pairing: the host's random nonce. */
    class PairChallenge(val nonce: ByteArray) : Message()

    /** Tablet → host after the challenge: the nonce it committed to. */
    class PairReveal(val nonce: ByteArray) : Message()

    data class Welcome(val version: Int, val width: Long, val height: Long) : Message()
    data class Reject(val reason: RejectReason, val detail: String) : Message()
    class Video(val ptsUs: Long, val keyframe: Boolean, val data: ByteArray) : Message()
    data class Ping(val id: Long, val sentUs: Long) : Message()
    data class Pong(val id: Long, val sentUs: Long) : Message()
    data object RequestKeyframe : Message()

    /** Positions are fractions of the stream, 0.0–1.0. */
    data class Touch(val slot: Int, val phase: TouchPhase, val x: Float, val y: Float) : Message()
    data class PointerMove(val x: Float, val y: Float) : Message()
    data class Button(val button: MouseButton, val pressed: Boolean) : Message()

    /** Wheel steps; positive is down and right. */
    data class Scroll(val horizontal: Int, val vertical: Int) : Message()

    /** A Linux evdev key code: what Android reports as the scan code. */
    data class Key(val code: Int, val pressed: Boolean) : Message()
}
