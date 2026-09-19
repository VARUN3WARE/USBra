package org.usbra.client

import android.os.Build
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.IOException
import java.net.InetSocketAddress
import java.net.Socket

/**
 * Network client with auto-reconnect. Runs its own thread; delivers parsed
 * messages through [Events]. Loopback-only by design: the adb reverse USB
 * tunnel makes `127.0.0.1` on the phone reach the host (docs/03-usb-transport.md).
 */
class UsbraClient(
    private val host: String,
    private val port: Int,
    private val events: Events,
) {
    interface Events {
        fun onSession(sessionId: Long, display: Protocol.DisplayInfo, config: Protocol.ConfigMsg)
        fun onFrame(frame: Protocol.FrameMsg)
        fun onRtt(rttMs: Double)
        fun onStatus(status: String)
        fun onBytes(n: Int)
    }

    @Volatile
    var running = true
        private set

    @Volatile
    private var socket: Socket? = null

    private var sessionId: Long = 0
    private var lastFrameId: Long = 0
    private var ackEnabled = false
    private var screenW = 1080
    private var screenH = 1920
    private var refreshHint = 60

    /** Real screen size for HELLO — set before [start]. */
    fun configureScreen(w: Int, h: Int) {
        screenW = w
        screenH = h
    }

    fun start(): Thread {
        val t = Thread { runLoop() }
        t.name = "usbra-client"
        t.start()
        return t
    }

    fun stop() {
        running = false
        try {
            socket?.close()
        } catch (_: IOException) {
        }
    }

    private fun runLoop() {
        var backoffMs = 250L
        while (running) {
            try {
                connectAndPump()
                backoffMs = 250L
            } catch (e: IOException) {
                if (!running) break
                events.onStatus("reconnecting… (${e.message})")
                try {
                    Thread.sleep(backoffMs)
                } catch (_: InterruptedException) {
                    return
                }
                backoffMs = (backoffMs * 2).coerceAtMost(2000L)
            }
        }
    }

    private fun deviceName(): String = "${Build.MANUFACTURER} ${Build.MODEL}".take(255)

    private fun connectAndPump() {
        val sock = Socket()
        socket = sock
        try {
            sock.tcpNoDelay = true
            sock.connect(InetSocketAddress(host, port), 2000)
            val input = BufferedInputStream(sock.getInputStream(), 64 shl 10)
            val output = BufferedOutputStream(sock.getOutputStream(), 64 shl 10)

            // ---- HELLO (fresh) or RECONNECT (resuming a known session) ----
            synchronized(output) {
                if (sessionId != 0L) {
                    Protocol.write(output, Protocol.Reconnect(sessionId, lastFrameId))
                } else {
                    Protocol.write(
                        output,
                        Protocol.Hello(
                            Protocol.VERSION,
                            Protocol.Cap.RAW,
                            screenW,
                            screenH,
                            refreshHint,
                            deviceName(),
                        ),
                    )
                }
            }

            // ---- handshake replies: HELLO_OK + DISPLAY_INFO + CONFIG ----
            var helloOk: Protocol.HelloOk? = null
            var display: Protocol.DisplayInfo? = null
            var config: Protocol.ConfigMsg? = null
            while (helloOk == null || display == null || config == null) {
                when (val m = Protocol.read(input) ?: throw Protocol.ProtocolException("closed during handshake")) {
                    is Protocol.HelloOk -> helloOk = m
                    is Protocol.DisplayInfo -> display = m
                    is Protocol.ConfigMsg -> config = m
                    is Protocol.Disconnect -> {
                        if (m.reason == Protocol.Reason.UNKNOWN_SESSION) {
                            // Host forgot us (restart); fall back to fresh HELLO next round.
                            sessionId = 0
                        }
                        throw Protocol.ProtocolException("host: ${m.message}")
                    }
                    else -> throw Protocol.ProtocolException("unexpected during handshake")
                }
            }
            sessionId = helloOk.sessionId
            ackEnabled = (config.flags and 1) != 0 // Config.FLAG_FRAME_ACK
            events.onSession(sessionId, display!!, config!!)
            events.onStatus("connected — ${display.width}×${display.height}@${display.refresh}Hz ${Protocol.PF.name(display.pixelFormat)}")

            // ---- RTT probe thread (0.5 s cadence) ----
            val pinger = Thread {
                while (running && sock.isConnected) {
                    try {
                        Thread.sleep(500)
                    } catch (_: InterruptedException) {
                        break
                    }
                    try {
                        synchronized(output) {
                            Protocol.write(output, Protocol.Ping(System.nanoTime()))
                        }
                    } catch (_: IOException) {
                        break
                    }
                }
            }.also {
                it.isDaemon = true
                it.name = "usbra-ping"
                it.start()
            }

            // ---- pump: frames, pongs, disconnects ----
            while (running) {
                val m = Protocol.read(input) ?: break // clean EOF
                when (m) {
                    is Protocol.FrameMsg -> {
                        lastFrameId = m.frameId
                        events.onBytes(m.payload.size + Protocol.HEADER_LEN + m.rects.size * 8)
                        events.onFrame(m)
                        if (ackEnabled) {
                            synchronized(output) {
                                Protocol.write(output, Protocol.FrameAck(m.frameId, System.nanoTime()))
                            }
                        }
                    }
                    is Protocol.Pong -> events.onRtt((System.nanoTime() - m.echoedTsNs) / 1e6)
                    is Protocol.Disconnect -> throw Protocol.ProtocolException("host: ${m.message}")
                    else -> {} // DAMAGE_REGION etc. tolerated, no-op in v1
                }
            }
        } finally {
            try {
                sock.close()
            } catch (_: IOException) {
            }
            if (running) events.onStatus("disconnected")
        }
    }
}
