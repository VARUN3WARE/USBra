package org.usbra.client

import android.app.Activity
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.TextView

/**
 * USBra client UI: fullscreen GLES surface + a small stats overlay (fps, RTT,
 * bandwidth — docs/07-benchmarking.md). Connects to 127.0.0.1:8899, which the
 * adb reverse tunnel maps to the host over USB.
 *
 * Optional intent extras: `port` (int), `host` (string) — handy for testing:
 *   adb shell am start -n org.usbra.client/.MainActivity --ei port 9000
 */
class MainActivity : Activity() {

    private lateinit var surface: UsbraSurfaceView
    private lateinit var statsView: TextView
    private var client: UsbraClient? = null
    private val mainHandler = Handler(Looper.getMainLooper())

    // ---- stats accumulators (client thread writes, UI thread reads) ----
    @Volatile private var displayLine = "waiting for display…"
    @Volatile private var statusLine = "connecting…"
    private var framesTotal = 0L
    private var framesWindow = 0L
    private var bytesTotal = 0L
    private var bytesAtLastTick = 0L
    private var fps = 0.0
    private var rttMs = 0.0
    private var lastTickNs = System.nanoTime()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        surface = UsbraSurfaceView(this)
        statsView = TextView(this).apply {
            setTextColor(Color.WHITE)
            setBackgroundColor(0x88000000.toInt())
            textSize = 12f
            typeface = Typeface.MONOSPACE
            setPadding(16, 24, 16, 16)
        }
        val root = FrameLayout(this)
        root.addView(surface, FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT))
        root.addView(statsView, FrameLayout.LayoutParams(FrameLayout.LayoutParams.WRAP_CONTENT, FrameLayout.LayoutParams.WRAP_CONTENT, Gravity.TOP or Gravity.START))
        setContentView(root)

        val port = intent.getIntExtra("port", 8899)
        val host = intent.getStringExtra("host") ?: "127.0.0.1"
        val dm = resources.displayMetrics

        val c = UsbraClient(host, port, object : UsbraClient.Events {
            override fun onSession(sessionId: Long, display: Protocol.DisplayInfo, config: Protocol.ConfigMsg) {
                displayLine = "${display.name} ${display.width}×${display.height}@${display.refresh}Hz " +
                    "${Protocol.PF.name(display.pixelFormat)}"
                surface.onDisplay(display)
            }

            override fun onFrame(frame: Protocol.FrameMsg) {
                framesTotal++
                framesWindow++
                surface.submit(frame)
            }

            override fun onRtt(rttMs: Double) {
                this@MainActivity.rttMs = rttMs
            }

            override fun onStatus(status: String) {
                statusLine = status
            }

            override fun onBytes(n: Int) {
                bytesTotal += n
            }
        })
        c.configureScreen(dm.widthPixels, dm.heightPixels)
        client = c
        c.start()

        tickStats()
    }

    /** 500 ms UI refresh; keeps the GL thread free of bookkeeping. */
    private fun tickStats() {
        mainHandler.postDelayed({
            val now = System.nanoTime()
            val dt = (now - lastTickNs) / 1e9
            if (dt >= 0.25) {
                fps = framesWindow / dt
                framesWindow = 0
                lastTickNs = now
            }
            val mbps = if (dt > 0) (bytesTotal - bytesAtLastTick) / dt / (1024.0 * 1024.0) else 0.0
            bytesAtLastTick = bytesTotal
            statsView.text =
                "USBra · $displayLine\n" +
                "fps %3.0f   rtt %5.1f ms   %6.2f MB/s\n".format(fps, rttMs, mbps) +
                "frames $framesTotal   $statusLine"
            tickStats()
        }, 500)
    }

    override fun onDestroy() {
        client?.stop()
        super.onDestroy()
    }
}
