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
    private const val HEADER_LEN = 4

    private const val HELLO = 1
    private const val WELCOME = 2
    private const val REJECT = 3
    private const val VIDEO = 4
    private const val PING = 5
    private const val PONG = 6
    private const val REQUEST_KEYFRAME = 7

    fun maxLength(tag: Int): Int = when (tag) {
        VIDEO -> 1 + MAX_VIDEO_FRAME + 9
        HELLO, WELCOME, REJECT, PING, PONG, REQUEST_KEYFRAME -> MAX_CONTROL_MESSAGE
        else -> throw ProtocolException("unknown message tag $tag")
    }

    fun encode(message: Message): ByteArray {
        val body = Writer()
        when (message) {
            is Message.Hello -> {
                body.u8(HELLO)
                body.u16(message.version)
                val token = message.pairingToken
                if (token == null) {
                    body.u8(0)
                } else {
                    require(token.size == PAIRING_TOKEN_LEN)
                    body.u8(1)
                    body.bytes(token)
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
                val token = when (val flag = r.u8()) {
                    0 -> null
                    1 -> r.bytes(PAIRING_TOKEN_LEN)
                    else -> throw ProtocolException("unknown token flag $flag")
                }
                Message.Hello(
                    version, token, r.string(MAX_NAME),
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

enum class RejectReason(val code: Int) {
    UNSUPPORTED_VERSION(1),
    NOT_PAIRED(2),
    BAD_PAIRING_TOKEN(3),
    BUSY(4),
    CAPTURE_FAILED(5),
}

sealed class Message {
    class Hello(
        val version: Int,
        val pairingToken: ByteArray?,
        val deviceName: String,
        val display: DisplayInfo,
    ) : Message()

    data class Welcome(val version: Int, val width: Long, val height: Long) : Message()
    data class Reject(val reason: RejectReason, val detail: String) : Message()
    class Video(val ptsUs: Long, val keyframe: Boolean, val data: ByteArray) : Message()
    data class Ping(val id: Long, val sentUs: Long) : Message()
    data class Pong(val id: Long, val sentUs: Long) : Message()
    data object RequestKeyframe : Message()
}
