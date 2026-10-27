package io.github.st3inberg.tabextend

import android.content.ClipboardManager
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.Gravity
import android.view.KeyEvent
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
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts

/**
 * The tablet's whole UI: a full-screen surface showing the host's virtual
 * monitor, with a panel on top while not streaming: status, pairing
 * buttons, computers found on the network, and the code during pairing.
 */
class MainActivity : ComponentActivity(), SurfaceHolder.Callback, StreamSession.Listener {
    private lateinit var hosts: HostStore
    private lateinit var surfaceView: SurfaceView
    private lateinit var status: TextView
    private lateinit var code: TextView
    private lateinit var stats: TextView
    private lateinit var computers: LinearLayout
    private lateinit var panel: LinearLayout
    private lateinit var discovery: Discovery

    private var session: StreamSession? = null
    private val input = InputTranslator { surfaceView.width to surfaceView.height }
    /** A pairing in progress: from a QR code or link, or a computer picked from the list. */
    private var pendingTarget: Target? = null

    private val scan = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val link = result.data?.dataString ?: return@registerForActivityResult
        handlePairingIntent(Intent(Intent.ACTION_VIEW, android.net.Uri.parse(link)))
        restartSession()
    }
    private var surfaceReady = false
    private var streamedSize: Pair<Int, Int>? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        hosts = HostStore(this)
        discovery = Discovery(this) { found -> runOnUiThread { showComputers(found) } }
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
        surfaceView = SurfaceView(this).apply {
            holder.addCallback(this@MainActivity)
            setOnTouchListener { _, event -> forward(input.onTouch(event)) }
            setOnGenericMotionListener { _, event -> forward(input.onGenericMotion(event)) }
        }
        status = TextView(this).apply {
            setTextColor(Color.WHITE)
            textSize = 18f
            gravity = Gravity.CENTER
        }
        code = TextView(this).apply {
            setTextColor(Color.WHITE)
            textSize = 64f
            gravity = Gravity.CENTER
            letterSpacing = 0.1f
            visibility = View.GONE
        }
        fun button(label: Int, action: () -> Unit) = Button(this).apply {
            text = getString(label)
            setOnClickListener { action() }
        }
        val buttons = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER
            addView(button(R.string.scan_qr) { scan.launch(Intent(this@MainActivity, ScannerActivity::class.java)) })
            addView(button(R.string.find_computers) { findComputers() })
            addView(button(R.string.paste_link) { pasteLink() })
        }
        computers = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
        }
        val wrap = { LinearLayout.LayoutParams(-2, -2).apply { topMargin = 32 } }
        panel = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setBackgroundColor(Color.BLACK)
            setPadding(48, 48, 48, 48)
            addView(status)
            addView(code, wrap())
            addView(buttons, wrap())
            addView(computers, wrap())
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

    /** Send input to the host while streaming; true means the event was used. */
    private fun forward(messages: List<Message>): Boolean {
        val active = session ?: return false
        active.sendInput(messages)
        return true
    }

    /** Keys from a physical keyboard go to the laptop; everything else stays here. */
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        forwardKey(event) || super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
        forwardKey(event) || super.onKeyUp(keyCode, event)

    private fun forwardKey(event: KeyEvent): Boolean {
        if (panel.visibility == View.VISIBLE) return false
        val key = session?.let { input.onKey(event) } ?: return false
        session?.sendInput(listOf(key))
        return true
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
        pendingTarget = Target.WithToken(link.host, link.token)
        showPanel(getString(R.string.pairing, fingerprintText(link.host.fingerprint)))
    }

    private fun findComputers() {
        discovery.start()
        showComputers(emptyList())
    }

    /** One button per computer open for pairing. */
    private fun showComputers(found: List<FoundComputer>) {
        computers.removeAllViews()
        val pairable = found.filter { it.pairing }
        if (pairable.isEmpty()) {
            computers.addView(TextView(this).apply {
                setTextColor(Color.LTGRAY)
                text = getString(R.string.searching)
            })
        }
        for (computer in pairable) {
            computers.addView(Button(this).apply {
                text = computer.name
                setOnClickListener {
                    discovery.stop()
                    computers.removeAllViews()
                    pendingTarget = Target.ByCode(computer.address, computer.port, computer.name)
                    restartSession()
                }
            })
        }
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
        val target = pendingTarget ?: hosts.load()?.let { Target.Paired(it) }
        if (target == null) {
            showPanel(getString(R.string.not_paired))
            return
        }
        val display = displayInfo()
        val panel = windowManager.maximumWindowMetrics.bounds
        streamedSize = panel.width() to panel.height()
        session = StreamSession(
            target, deviceName(), display, surfaceView.holder.surface, this,
        ).also { it.start() }
    }

    private fun stopSession() {
        session?.stop()
        session = null
        input.reset()
    }

    override fun onDestroy() {
        stopSession()
        discovery.stop()
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
        code.visibility = View.GONE
        panel.visibility = View.VISIBLE
        stats.visibility = View.GONE
    }

    // StreamSession.Listener: called on the session thread.

    override fun onStatus(text: String) = showPanel(text)

    override fun onStreaming(width: Int, height: Int, decoder: String, lowLatency: Boolean) = runOnUiThread {
        discovery.stop()
        panel.visibility = View.GONE
        stats.visibility = View.VISIBLE
        stats.text = getString(R.string.streaming, width, height, decoder, if (lowLatency) " (low latency)" else "")
    }

    override fun onPaired(host: Host) = runOnUiThread {
        hosts.save(host)
        pendingTarget = null
    }

    override fun onConfirmCode(code: String, computer: String) = runOnUiThread {
        status.text = getString(R.string.confirm_code, computer)
        this.code.text = code
        this.code.visibility = View.VISIBLE
        panel.visibility = View.VISIBLE
    }

    override fun onStats(fps: Double, mbps: Double, decodeMs: Double) = runOnUiThread {
        stats.text = getString(R.string.stats, fps, mbps, decodeMs)
    }

    override fun onGaveUp(reason: String) {
        runOnUiThread { pendingTarget = null }
        showPanel(reason)
    }

    private companion object {
        const val MAX_FPS = 60.0
    }
}
