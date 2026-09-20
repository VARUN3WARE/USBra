package org.usbra.client

import android.content.Context
import android.opengl.GLES20
import android.opengl.GLSurfaceView
import android.opengl.Matrix
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer
import javax.microedition.khronos.egl.EGLConfig
import javax.microedition.khronos.opengles.GL10

/**
 * GLSurfaceView + GLES2 renderer that maintains one full-display RGBA texture
 * and applies each FRAME's damage rects with `glTexSubImage2D` — the upload
 * path stays proportional to the *damage*, not the display (M5).
 * RENDERMODE_WHEN_DIRTY: we draw only when a frame (or geometry change) arrives.
 */
class UsbraSurfaceView(context: Context) : GLSurfaceView(context) {
    private val renderer = UsbraRenderer(this)

    init {
        setEGLContextClientVersion(2)
        setRenderer(renderer)
        renderMode = RENDERMODE_WHEN_DIRTY
        preserveEGLContextOnPause = true
    }

    fun onDisplay(info: Protocol.DisplayInfo) = renderer.setDisplay(info)
    fun submit(frame: Protocol.FrameMsg) = renderer.submit(frame)
}

private class UsbraRenderer(private val view: GLSurfaceView) : GLSurfaceView.Renderer {

    // ---- shared with client thread (guarded by `lock`) ----
    private val lock = Any()
    private val pending = ArrayDeque<Protocol.FrameMsg>()
    private var displayW = 0
    private var displayH = 0
    private var displayPf = Protocol.PF.BGRX

    // ---- GL thread only ----
    private var program = 0
    private var uMvp = 0
    private var uTex = 0
    private var uSwizzle = 0
    private var aPos = 0
    private var aUv = 0
    private var tex = 0
    private var texAllocW = 0
    private var texAllocH = 0
    private var surfaceW = 1
    private var surfaceH = 1
    private val mvp = FloatArray(16)
    /** Reused across uploads so we don't allocate a DirectByteBuffer per rect. */
    private var uploadBuf: ByteBuffer? = null

    /** Clips: two triangles forming a full clip (x, y) quad. */
    private val quad = floatArrayOf(-1f, -1f, 1f, -1f, -1f, 1f, 1f, 1f)
    /** Texture coords: framebuffer row 0 (display top) maps to clip-space top. */
    private val texCoords = floatArrayOf(0f, 1f, 1f, 1f, 0f, 0f, 1f, 0f)
    private lateinit var quadBuf: FloatBuffer
    private lateinit var uvBuf: FloatBuffer

    // ---- client thread API ----

    fun setDisplay(info: Protocol.DisplayInfo) {
        synchronized(lock) {
            displayW = info.width
            displayH = info.height
            displayPf = info.pixelFormat
            pending.clear() // stale frames from the old geometry
        }
        view.requestRender()
    }

    fun submit(frame: Protocol.FrameMsg) {
        synchronized(lock) {
            val isFull = displayW > 0 &&
                frame.rects.size == 1 &&
                frame.rects[0].x == 0 &&
                frame.rects[0].y == 0 &&
                frame.rects[0].w == displayW &&
                frame.rects[0].h == displayH
            // A full frame resyncs the texture — drop stale damage ahead of it.
            if (isFull) pending.clear()
            pending.addLast(frame)
            // Soft cap: with tile-damage payloads this stays small; avoid multi-second lag.
            while (pending.size > 8) pending.removeFirst()
        }
        view.requestRender()
    }

    // ---- GLSurfaceView.Renderer ----

    override fun onSurfaceCreated(gl: GL10?, config: EGLConfig?) {
        program = buildProgram()
        aPos = GLES20.glGetAttribLocation(program, "aPos")
        aUv = GLES20.glGetAttribLocation(program, "aUv")
        uMvp = GLES20.glGetUniformLocation(program, "uMvp")
        uTex = GLES20.glGetUniformLocation(program, "uTex")
        uSwizzle = GLES20.glGetUniformLocation(program, "uSwizzle")
        quadBuf = direct(quad)
        uvBuf = direct(texCoords)

        val t = IntArray(1)
        GLES20.glGenTextures(1, t, 0)
        tex = t[0]
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, tex)
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR)
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR)
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE)
        GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE)
        texAllocW = 0 // force (re)allocation on first frame
        texAllocH = 0
        GLES20.glDisable(GLES20.GL_DEPTH_TEST)
    }

    override fun onSurfaceChanged(gl: GL10?, width: Int, height: Int) {
        GLES20.glViewport(0, 0, width, height)
        surfaceW = width
        surfaceH = height
        view.requestRender()
    }

    override fun onDrawFrame(gl: GL10?) {
        GLES20.glClearColor(0f, 0f, 0f, 1f)
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)

        var frames: List<Protocol.FrameMsg>
        var w: Int
        var h: Int
        var pf: Int
        synchronized(lock) {
            w = displayW
            h = displayH
            pf = displayPf
            frames = pending.toList()
            pending.clear()
        }
        if (w == 0 || h == 0) return // no display config yet

        // (Re)allocate the texture on geometry change (e.g. after reconnect).
        if (texAllocW != w || texAllocH != h) {
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, tex)
            GLES20.glTexImage2D(
                GLES20.GL_TEXTURE_2D, 0, GLES20.GL_RGBA, w, h, 0,
                GLES20.GL_RGBA, GLES20.GL_UNSIGNED_BYTE, null as ByteBuffer?,
            )
            texAllocW = w
            texAllocH = h
        } else {
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, tex)
        }

        // Upload damage: one glTexSubImage2D per rect — bytes moved ∝ damage.
        for (f in frames) {
            if (f.pixelFormat != pf) continue // stale format; full frame will follow
            for (i in f.rects.indices) {
                val r = f.rects[i]
                if (r.w == 0 || r.h == 0) continue
                if (r.x + r.w > w || r.y + r.h > h) continue // defensive; host is trusted but cheap to check
                val range = f.rectRange(i)
                val len = range.last - range.first + 1
                val buf = ensureUploadBuf(len)
                buf.clear()
                buf.put(f.payload, range.first, len)
                buf.position(0)
                buf.limit(len)
                GLES20.glTexSubImage2D(
                    GLES20.GL_TEXTURE_2D, 0, r.x, r.y, r.w, r.h,
                    GLES20.GL_RGBA, GLES20.GL_UNSIGNED_BYTE, buf,
                )
            }
        }
        if (texAllocW == 0) return // texture just allocated, nothing uploaded yet

        // Fill the landscape phone screen (cover); slight crop beats tiny letterbox.
        val scale = maxOf(surfaceW.toFloat() / w, surfaceH.toFloat() / h)
        val halfW = w * scale / surfaceW
        val halfH = h * scale / surfaceH
        Matrix.orthoM(mvp, 0, -halfW, halfW, -halfH, halfH, -1f, 1f)

        GLES20.glUseProgram(program)
        GLES20.glActiveTexture(GLES20.GL_TEXTURE0)
        GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, tex)
        GLES20.glUniform1i(uTex, 0)
        GLES20.glUniformMatrix4fv(uMvp, 1, false, mvp, 0)
        // BGRX memory order (DRM XRGB8888) needs a channel swizzle in the shader.
        GLES20.glUniform1f(uSwizzle, if (pf == Protocol.PF.BGRX) 1f else 0f)

        GLES20.glEnableVertexAttribArray(aPos)
        GLES20.glVertexAttribPointer(aPos, 2, GLES20.GL_FLOAT, false, 0, quadBuf)
        GLES20.glEnableVertexAttribArray(aUv)
        GLES20.glVertexAttribPointer(aUv, 2, GLES20.GL_FLOAT, false, 0, uvBuf)
        GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4)
        GLES20.glDisableVertexAttribArray(aPos)
        GLES20.glDisableVertexAttribArray(aUv)
    }

    // ---- helpers ----

    private fun ensureUploadBuf(minBytes: Int): ByteBuffer {
        val cur = uploadBuf
        if (cur != null && cur.capacity() >= minBytes) return cur
        // Grow with headroom so full-frame 720p (≈3.6 MiB) doesn't churn.
        val cap = maxOf(minBytes, 1280 * 720 * 4)
        val buf = ByteBuffer.allocateDirect(cap).order(ByteOrder.nativeOrder())
        uploadBuf = buf
        return buf
    }

    private fun direct(f: FloatArray): FloatBuffer =
        ByteBuffer.allocateDirect(f.size * 4)
            .order(ByteOrder.nativeOrder())
            .asFloatBuffer()
            .apply { put(f); position(0) }

    private fun buildProgram(): Int {
        val vs = """
            attribute vec2 aPos;
            attribute vec2 aUv;
            uniform mat4 uMvp;
            varying vec2 vUv;
            void main() {
                vUv = aUv;
                gl_Position = uMvp * vec4(aPos, 0.0, 1.0);
            }
        """.trimIndent()
        // uSwizzle=1: framebuffer bytes are B,G,R,X (DRM XRGB8888).
        val fs = """
            precision mediump float;
            varying vec2 vUv;
            uniform sampler2D uTex;
            uniform float uSwizzle;
            void main() {
                vec4 c = texture2D(uTex, vUv);
                if (uSwizzle > 0.5) {
                    c = vec4(c.b, c.g, c.r, c.a);
                }
                gl_FragColor = vec4(c.rgb, 1.0);
            }
        """.trimIndent()
        val p = GLES20.glCreateProgram()
        GLES20.glAttachShader(p, compile(GLES20.GL_VERTEX_SHADER, vs))
        GLES20.glAttachShader(p, compile(GLES20.GL_FRAGMENT_SHADER, fs))
        GLES20.glLinkProgram(p)
        val status = IntArray(1)
        GLES20.glGetProgramiv(p, GLES20.GL_LINK_STATUS, status, 0)
        check(status[0] == GLES20.GL_TRUE) { "program link failed: ${GLES20.glGetProgramInfoLog(p)}" }
        return p
    }

    private fun compile(type: Int, src: String): Int {
        val s = GLES20.glCreateShader(type)
        GLES20.glShaderSource(s, src)
        GLES20.glCompileShader(s)
        val status = IntArray(1)
        GLES20.glGetShaderiv(s, GLES20.GL_COMPILE_STATUS, status, 0)
        check(status[0] == GLES20.GL_TRUE) { "shader compile failed: ${GLES20.glGetShaderInfoLog(s)}" }
        return s
    }
}
