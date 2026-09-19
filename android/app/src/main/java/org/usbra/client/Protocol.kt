package org.usbra.client

import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * USBra wire protocol v1 — Kotlin mirror of `protocol/src/lib.rs` (the
 * authoritative implementation). Framing: 16-byte little-endian header
 * {magic u32, version u16, type u16, length u32, flags u32} + payload.
 */
object Protocol {
    const val MAGIC: Int = 0x5242_5355 // "USBR" bytes, little-endian (u32::from_le_bytes(b"USBR") in Rust)
    const val VERSION: Int = 1
    const val HEADER_LEN = 16
    const val MAX_PAYLOAD = 64 shl 20

    object M {
        const val HELLO = 1
        const val HELLO_OK = 2
        const val CONFIG = 3
        const val DISPLAY_INFO = 4
        const val FRAME = 5
        const val DAMAGE_REGION = 6
        const val PING = 7
        const val PONG = 8
        const val DISCONNECT = 9
        const val RECONNECT = 10
        const val FRAME_ACK = 11
    }

    object Cap {
        const val RAW = 1
        const val ZSTD = 2
    }

    object Codec {
        const val RAW = 0
        const val ZSTD = 1
    }

    object PF {
        const val BGRX = 0
        const val RGBX = 1
        fun name(f: Int) = when (f) {
            BGRX -> "BGRX"
            RGBX -> "RGBX"
            else -> "PF?$f"
        }
    }

    object Reason {
        const val UNKNOWN_SESSION = 1
        const val PROTOCOL = 2
        const val SHUTDOWN = 3
        const val ERROR = 4
    }

    const val MAX_RECTS = 64

    data class Rect(val x: Int, val y: Int, val w: Int, val h: Int) {
        val byteLen: Int get() = w * h * 4
    }

    sealed class Msg {
        abstract val wireType: Int
    }

    /** Client → host: session request and capability negotiation. */
    class Hello(
        val protoVersion: Int,
        val capabilities: Int,
        val screenW: Int,
        val screenH: Int,
        val refreshHint: Int,
        val name: String,
    ) : Msg() { override val wireType = M.HELLO }

    /** Host → client: session accepted. */
    class HelloOk(
        val protoVersion: Int,
        val capabilities: Int,
        val sessionId: Long,
        val maxRects: Int,
    ) : Msg() { override val wireType = M.HELLO_OK }

    /** Host → client: virtual display geometry and native format. */
    class DisplayInfo(
        val width: Int,
        val height: Int,
        val refresh: Int,
        val pixelFormat: Int,
        val flags: Int,
        val name: String,
    ) : Msg() { override val wireType = M.DISPLAY_INFO }

    /** Host → client: authoritative session parameters. */
    class ConfigMsg(
        val codec: Int,
        val pixelFormat: Int,
        val refreshHint: Int,
        val flags: Int,
    ) : Msg() { override val wireType = M.CONFIG }

    class FrameMsg(
        val frameId: Long,
        val timestampNs: Long,
        val codec: Int,
        val pixelFormat: Int,
        val rects: Array<Rect>,
        val payload: ByteArray,
    ) : Msg() {
        override val wireType = M.FRAME

        /** Byte range of rect [i] inside a RAW payload (tightly packed, rect order). */
        fun rectRange(i: Int): IntRange {
            var off = 0
            for (j in 0 until i) off += rects[j].byteLen
            return off until off + rects[i].byteLen
        }
    }

    class DamageRegionMsg(
        val frameId: Long,
        val timestampNs: Long,
        val rects: Array<Rect>,
    ) : Msg() { override val wireType = M.DAMAGE_REGION }

    class Ping(val clientTsNs: Long) : Msg() { override val wireType = M.PING }

    class Pong(val echoedTsNs: Long, val hostTsNs: Long) : Msg() { override val wireType = M.PONG }

    class Disconnect(val reason: Int, val message: String) : Msg() {
        override val wireType = M.DISCONNECT
    }

    class Reconnect(val sessionId: Long, val lastFrameId: Long) : Msg() {
        override val wireType = M.RECONNECT
    }

    class FrameAck(val frameId: Long, val clientTsNs: Long) : Msg() {
        override val wireType = M.FRAME_ACK
    }

    class ProtocolException(message: String) : IOException(message)

    // ------------------------------------------------------------------
    // Writing (client→host messages only; host messages are decoded, not sent)
    // ------------------------------------------------------------------

    fun write(out: OutputStream, msg: Msg) {
        val payload = encodePayload(msg)
        val head = ByteBuffer.allocate(HEADER_LEN).order(ByteOrder.LITTLE_ENDIAN)
        head.putInt(MAGIC)
        head.putShort(VERSION.toShort())
        head.putShort(msg.wireType.toShort())
        head.putInt(payload.size)
        head.putInt(0) // flags
        val all = ByteArray(HEADER_LEN + payload.size)
        System.arraycopy(head.array(), 0, all, 0, HEADER_LEN)
        System.arraycopy(payload, 0, all, HEADER_LEN, payload.size)
        out.write(all)
        out.flush()
    }

    private fun encodePayload(m: Msg): ByteArray {
        val b = ByteBuffer.allocate(296).order(ByteOrder.LITTLE_ENDIAN) // worst case HELLO
        when (m) {
            is Hello -> {
                b.putShort(m.protoVersion.toShort())
                b.putInt(m.capabilities)
                b.putShort(m.screenW.toShort())
                b.putShort(m.screenH.toShort())
                b.putShort(m.refreshHint.toShort())
                putStr(b, m.name)
            }
            is Ping -> b.putLong(m.clientTsNs)
            is Disconnect -> {
                b.putShort(m.reason.toShort())
                putStr(b, m.message)
            }
            is Reconnect -> {
                b.putLong(m.sessionId)
                b.putLong(m.lastFrameId)
            }
            is FrameAck -> {
                b.putLong(m.frameId)
                b.putLong(m.clientTsNs)
            }
            is HelloOk, is DisplayInfo, is ConfigMsg, is FrameMsg, is DamageRegionMsg, is Pong ->
                throw ProtocolException("host→client message is never sent by the client")
        }
        return b.array().copyOf(b.position())
    }

    private fun putStr(b: ByteBuffer, s: String) {
        val bytes = s.toByteArray(Charsets.UTF_8)
        require(bytes.size <= 255) { "protocol string too long: $s" }
        b.put(bytes.size.toByte())
        b.put(bytes)
    }

    // ------------------------------------------------------------------
    // Reading
    // ------------------------------------------------------------------

    /** Read one message; null on clean EOF between messages. */
    fun read(input: InputStream): Msg? {
        val h = readFully(input, HEADER_LEN) ?: return null
        val hb = ByteBuffer.wrap(h).order(ByteOrder.LITTLE_ENDIAN)
        val magic = hb.int
        if (magic != MAGIC) {
            throw ProtocolException("bad magic 0x${Integer.toHexString(magic)} (not USBra)")
        }
        val version = hb.short.toInt()
        if (version > VERSION) throw ProtocolException("unsupported protocol version $version")
        val type = hb.short.toInt()
        val len = hb.int
        if (len < 0 || len > MAX_PAYLOAD) throw ProtocolException("payload too large: $len")
        val payload = if (len == 0) {
            ByteArray(0)
        } else {
            readFully(input, len) ?: throw EOFException("closed mid-payload")
        }
        return decode(type, payload)
    }

    private fun readFully(input: InputStream, n: Int): ByteArray? {
        val out = ByteArray(n)
        var off = 0
        while (off < n) {
            val r = input.read(out, off, n - off)
            if (r < 0) {
                return if (off == 0) null else throw EOFException("closed mid-message")
            }
            off += r
        }
        return out
    }

    private fun decode(type: Int, p: ByteArray): Msg {
        val b = ByteBuffer.wrap(p).order(ByteOrder.LITTLE_ENDIAN)

        fun u8(): Int = b.get().toInt() and 0xFF
        fun u16(): Int = b.short.toInt() and 0xFFFF
        fun u32(): Int = b.int
        fun u64(): Long = b.long
        fun str(): String {
            val n = u8()
            if (b.remaining() < n) throw ProtocolException("truncated string")
            val bytes = ByteArray(n)
            b.get(bytes)
            return String(bytes, Charsets.UTF_8)
        }
        fun rect(): Rect {
            val x = u16(); val y = u16(); val w = u16(); val h = u16()
            return Rect(x, y, w, h)
        }

        val m: Msg = when (type) {
            M.HELLO -> Hello(u16(), u32(), u16(), u16(), u16(), str())
            M.HELLO_OK -> {
                val v = u16(); val caps = u32(); val sid = u64(); val maxR = u16(); u16()
                HelloOk(v, caps, sid, maxR)
            }
            M.CONFIG -> ConfigMsg(u8(), u8(), u16(), u16())
            M.DISPLAY_INFO -> DisplayInfo(u16(), u16(), u16(), u8(), u8(), str())
            M.FRAME -> {
                val id = u64(); val ts = u64(); val codec = u8(); val pf = u8()
                val n = u16(); b.short // reserved
                if (n > MAX_RECTS) throw ProtocolException("too many rects: $n")
                val rects = Array(n) { rect() }
                val payload = ByteArray(b.remaining())
                b.get(payload)
                val f = FrameMsg(id, ts, codec, pf, rects, payload)
                if (f.codec != Codec.RAW) {
                    throw ProtocolException("codec ${f.codec} not supported yet (client offered RAW)")
                }
                val expect = rects.sumOf { it.byteLen }
                if (expect != payload.size) {
                    throw ProtocolException("payload ${payload.size}B != rect packing ${expect}B")
                }
                f
            }
            M.DAMAGE_REGION -> {
                val id = u64(); val ts = u64()
                val n = u16(); b.short
                if (n > MAX_RECTS) throw ProtocolException("too many rects: $n")
                DamageRegionMsg(id, ts, Array(n) { rect() })
            }
            M.PING -> Ping(u64())
            M.PONG -> Pong(u64(), u64())
            M.DISCONNECT -> Disconnect(u16(), str())
            M.RECONNECT -> Reconnect(u64(), u64())
            M.FRAME_ACK -> FrameAck(u64(), u64())
            else -> throw ProtocolException("unknown message type $type")
        }
        if (b.hasRemaining()) throw ProtocolException("trailing bytes after message")
        return m
    }
}
