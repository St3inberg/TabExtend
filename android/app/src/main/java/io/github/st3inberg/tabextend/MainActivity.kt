package io.github.st3inberg.tabextend

import android.app.Activity
import android.content.ClipboardManager
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.Gravity
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.TextView

/**
 * The tablet's whole UI: a full-screen surface showing the host's virtual
 * monitor, with a status line on top while not streaming.
 */
class MainActivity : Activity(), SurfaceHolder.Callback, StreamSession.Listener {
    private lateinit var hosts: HostStore
    private lateinit var surfaceView: SurfaceView
    private lateinit var status: TextView
    private lateinit var stats: TextView
    private lateinit var pasteButton: Button
    private lateinit var panel: LinearLayout

    private var session: StreamSession? = null
    private var pendingPairing: PairingLink? = null
    private var surfaceReady = false
    private var streamedSize: Pair<Int, Int>? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        hosts = HostStore(this)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        buildLayout()
        handlePairingIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handlePairingIntent(intent)
        restartSession()
    }

    /** On rotation, reconnect so the host recreates the monitor at the new size. */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        val size = windowManager.maximumWindowMetrics.bounds
        if (size.width() != streamedSize?.first || size.height() != streamedSize?.second) {
            restartSession()
        }
    }

    override fun onResume() {
        super.onResume()
        goImmersive()
    }

    private fun buildLayout() {
        surfaceView = SurfaceView(this).apply { holder.addCallback(this@MainActivity) }
        status = TextView(this).apply {
            setTextColor(Color.WHITE)
            textSize = 18f
            gravity = Gravity.CENTER
        }
        pasteButton = Button(this).apply {
            text = getString(R.string.paste_link)
            setOnClickListener { pasteLink() }
        }
        panel = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setBackgroundColor(Color.BLACK)
            setPadding(48, 48, 48, 48)
            addView(status)
            addView(pasteButton, LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = 32 })
        }
        stats = TextView(this).apply {
            setTextColor(Color.argb(200, 255, 255, 255))
            setBackgroundColor(Color.argb(120, 0, 0, 0))
            textSize = 11f
            setPadding(12, 6, 12, 6)
            visibility = View.GONE
        }
        setContentView(FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            addView(surfaceView, FrameLayout.LayoutParams(-1, -1))
            addView(panel, FrameLayout.LayoutParams(-1, -1))
            addView(stats, FrameLayout.LayoutParams(-2, -2, Gravity.TOP or Gravity.START))
        })
    }

    private fun goImmersive() {
        window.insetsController?.let {
            it.hide(WindowInsets.Type.systemBars())
            it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
    }

    private fun handlePairingIntent(intent: Intent?) {
        val uri = intent?.takeIf { it.action == Intent.ACTION_VIEW }?.dataString ?: return
        val link = PairingLink.parse(uri)
        if (link == null) {
            showPanel(getString(R.string.bad_link))
            return
        }
        pendingPairing = link
        showPanel(getString(R.string.pairing, fingerprintText(link.host.fingerprint)))
    }

    private fun pasteLink() {
        val clip = getSystemService(ClipboardManager::class.java)?.primaryClip
        val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this)?.toString()?.trim()
        handlePairingIntent(Intent(Intent.ACTION_VIEW).apply { data = android.net.Uri.parse(text ?: "") })
        restartSession()
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        surfaceReady = true
        restartSession()
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {}

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        surfaceReady = false
        stopSession()
    }

    private fun restartSession() {
        stopSession()
        if (!surfaceReady) return
        val pairing = pendingPairing
        val host = pairing?.host ?: hosts.load()
        if (host == null) {
            showPanel(getString(R.string.not_paired))
            return
        }
        val display = displayInfo()
        val panel = windowManager.maximumWindowMetrics.bounds
        streamedSize = panel.width() to panel.height()
        session = StreamSession(
            host, pairing?.token, deviceName(), display, surfaceView.holder.surface, this,
        ).also { it.start() }
    }

    private fun stopSession() {
        session?.stop()
        session = null
    }

    override fun onDestroy() {
        stopSession()
        super.onDestroy()
    }

    /**
     * What to ask the host for: the panel's shape, at the largest size this
     * tablet's decoder handles at the frame rate the host will send (at most 60).
     */
    private fun displayInfo(): DisplayInfo {
        val bounds = windowManager.maximumWindowMetrics.bounds
        val fps = minOf((display?.refreshRate ?: 60f).toDouble(), MAX_FPS)
        val (width, height) = StreamSize.forDisplay(bounds.width(), bounds.height(), fps)
        val density = resources.displayMetrics.densityDpi.toLong() * width / bounds.width()
        return DisplayInfo(width.toLong(), height.toLong(), density, (fps * 1000).toLong())
    }

    private fun deviceName(): String =
        Settings.Global.getString(contentResolver, Settings.Global.DEVICE_NAME)
            ?.takeIf { it.isNotBlank() } ?: Build.MODEL

    private fun showPanel(text: String) = runOnUiThread {
        status.text = text
        panel.visibility = View.VISIBLE
        stats.visibility = View.GONE
    }

    // StreamSession.Listener: called on the session thread.

    override fun onStatus(text: String) = showPanel(text)

    override fun onStreaming(width: Int, height: Int, decoder: String, lowLatency: Boolean) = runOnUiThread {
        panel.visibility = View.GONE
        stats.visibility = View.VISIBLE
        stats.text = getString(R.string.streaming, width, height, decoder, if (lowLatency) " (low latency)" else "")
    }

    override fun onPaired() = runOnUiThread {
        pendingPairing?.let { hosts.save(it.host) }
        pendingPairing = null
    }

    override fun onStats(fps: Double, mbps: Double) = runOnUiThread {
        stats.text = getString(R.string.stats, fps, mbps)
    }

    override fun onGaveUp(reason: String) {
        runOnUiThread { pendingPairing = null }
        showPanel(reason)
    }

    private companion object {
        const val MAX_FPS = 60.0
    }
}
