package io.github.st3inberg.tabextend

import android.content.ClipboardManager
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Color
import android.net.Uri
import android.net.wifi.WifiManager
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
import android.widget.FrameLayout
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.ui.platform.ComposeView
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen

/**
 * The whole app: a full-screen surface for the computer's screen, with the
 * Compose screens (pairing, home, settings) on top while not streaming.
 */
class MainActivity : ComponentActivity(), SurfaceHolder.Callback, StreamSession.Listener, Actions {
    private lateinit var hosts: HostStore
    private lateinit var settings: AppSettings
    private lateinit var surfaceView: SurfaceView
    private lateinit var stats: TextView
    private lateinit var screens: ComposeView
    private lateinit var discovery: Discovery
    private val state = UiState()
    /** Wi-Fi power saving delays packets by up to ~100 ms; Android honours this lock only while we are on screen. */
    private var wifiLock: WifiManager.WifiLock? = null

    private var session: StreamSession? = null
    private val input = InputTranslator { surfaceView.width to surfaceView.height }
    /** A pairing in progress: from a QR code or link, or a computer picked from the list. */
    private var pendingTarget: Target? = null
    private var pendingName: String? = null
    private var surfaceReady = false
    private var streamedSize: Pair<Int, Int>? = null

    private val scan = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val link = result.data?.dataString ?: return@registerForActivityResult
        handlePairingIntent(Intent(Intent.ACTION_VIEW, Uri.parse(link)))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        // The icon zooms and fades into the first screen.
        installSplashScreen().setOnExitAnimationListener { splash ->
            splash.iconView.animate().scaleX(1.4f).scaleY(1.4f).setDuration(SPLASH_EXIT_MS).start()
            splash.view.animate().alpha(0f).setDuration(SPLASH_EXIT_MS)
                .withEndAction { splash.remove() }.start()
        }
        super.onCreate(savedInstanceState)
        hosts = HostStore(this)
        settings = AppSettings(this)
        discovery = Discovery(this) { found ->
            runOnUiThread {
                state.computers = found
                followMovedHost(found)
            }
        }
        state.playSound = settings.playSound
        state.showStats = settings.showStats
        state.version = packageManager.getPackageInfo(packageName, 0).versionName ?: ""
        state.computerName = hosts.load()?.let { hosts.name() }
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        buildLayout()
        showScreen(if (state.computerName != null) Screen.Home else Screen.Pairing)
        handlePairingIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handlePairingIntent(intent)
    }

    /** On rotation, reconnect so the host recreates the monitor at the new size. */
    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        val size = windowManager.maximumWindowMetrics.bounds
        if (session != null && (size.width() != streamedSize?.first || size.height() != streamedSize?.second)) {
            restartSession()
        }
    }

    private fun buildLayout() {
        surfaceView = SurfaceView(this).apply {
            holder.addCallback(this@MainActivity)
            setOnTouchListener { _, event -> forward(input.onTouch(event)) }
            setOnGenericMotionListener { _, event -> forward(input.onGenericMotion(event)) }
        }
        stats = TextView(this).apply {
            setTextColor(Color.argb(200, 255, 255, 255))
            setBackgroundColor(Color.argb(120, 0, 0, 0))
            textSize = 11f
            setPadding(12, 6, 12, 6)
            visibility = View.GONE
        }
        screens = ComposeView(this).apply {
            setContent { TabExtendTheme { App(state, this@MainActivity) } }
        }
        setContentView(FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            addView(surfaceView, FrameLayout.LayoutParams(-1, -1))
            addView(stats, FrameLayout.LayoutParams(-2, -2, Gravity.TOP or Gravity.START))
            addView(screens, FrameLayout.LayoutParams(-1, -1))
        })
    }

    /** Full screen only while streaming; normal system bars everywhere else. */
    private fun showScreen(screen: Screen) {
        state.screen = screen
        val streaming = screen == Screen.Streaming
        screens.visibility = if (streaming) View.GONE else View.VISIBLE
        stats.visibility = if (streaming && state.showStats) View.VISIBLE else View.GONE
        window.insetsController?.let {
            if (streaming) {
                it.hide(WindowInsets.Type.systemBars())
                it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            } else {
                it.show(WindowInsets.Type.systemBars())
            }
        }
        if (screen == Screen.Pairing && state.code == null && pendingTarget == null) {
            findComputers()
        } else if (screen != Screen.Pairing) {
            discovery.stop()
            state.searching = false
        }
    }

    /** Send input to the host while streaming; true means the event was used. */
    private fun forward(messages: List<Message>): Boolean {
        val active = session?.takeIf { state.screen == Screen.Streaming } ?: return false
        active.sendInput(messages)
        return true
    }

    /** Keys from a physical keyboard go to the computer while streaming. */
    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        forwardKey(event) || super.onKeyDown(keyCode, event)

    override fun onKeyUp(keyCode: Int, event: KeyEvent): Boolean =
        forwardKey(event) || super.onKeyUp(keyCode, event)

    private fun forwardKey(event: KeyEvent): Boolean {
        if (state.screen != Screen.Streaming) return false
        val key = session?.let { input.onKey(event) } ?: return false
        session?.sendInput(listOf(key))
        return true
    }

    private fun handlePairingIntent(intent: Intent?) {
        val uri = intent?.takeIf { it.action == Intent.ACTION_VIEW }?.dataString ?: return
        val link = PairingLink.parse(uri)
        if (link == null) {
            state.error = getString(R.string.bad_link)
            showScreen(Screen.Pairing)
            return
        }
        startPairing(Target.WithToken(link.host, link.token), null)
    }

    private fun startPairing(target: Target, name: String?) {
        discovery.stop()
        state.searching = false
        state.error = null
        state.code = null
        pendingTarget = target
        pendingName = name
        state.status = getString(R.string.connecting, name ?: target.address)
        restartSession()
    }

    // Actions from the screens.

    override fun scanQr() = scan.launch(Intent(this, ScannerActivity::class.java))

    override fun findComputers() {
        state.error = null
        state.searching = true
        discovery.start()
    }

    override fun pasteLink() {
        val clip = getSystemService(ClipboardManager::class.java)?.primaryClip
        val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(this)?.toString()?.trim()
        handlePairingIntent(Intent(Intent.ACTION_VIEW, Uri.parse(text ?: "")))
    }

    override fun pick(computer: FoundComputer) =
        startPairing(Target.ByCode(computer.address, computer.port, computer.name), computer.name)

    override fun retry() {
        state.error = null
        restartSession()
    }

    override fun openSettings() = showScreen(Screen.Settings)

    override fun back() {
        state.code = null
        state.searching = false
        discovery.stop()
        showScreen(if (state.computerName != null) Screen.Home else Screen.Pairing)
    }

    override fun pairAnother() {
        state.error = null
        showScreen(Screen.Pairing)
    }

    override fun forget() {
        stopSession()
        hosts.clear()
        state.computerName = null
        state.error = null
        showScreen(Screen.Pairing)
    }

    override fun setPlaySound(on: Boolean) {
        settings.playSound = on
        state.playSound = on
    }

    override fun setShowStats(on: Boolean) {
        settings.showStats = on
        state.showStats = on
    }

    override fun openSource() {
        startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(getString(R.string.source_url))))
    }

    // The video surface and the session.

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
        val target = pendingTarget ?: hosts.load()?.let { Target.Paired(it) } ?: return
        state.busy = true
        val display = displayInfo()
        val panel = windowManager.maximumWindowMetrics.bounds
        streamedSize = panel.width() to panel.height()
        session = StreamSession(
            target, deviceName(), display, surfaceView.holder.surface, settings.playSound, this,
        ).also { it.start() }
    }

    private fun stopSession() {
        state.waiting = false
        session?.stop()
        session = null
        input.reset()
        state.busy = false
    }

    override fun onResume() {
        super.onResume()
        wifiLock = wifiLock ?: getSystemService(WifiManager::class.java)
            ?.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "TabExtend")
            ?.apply { setReferenceCounted(false) }
        wifiLock?.acquire()
        if (state.searching) discovery.start()
    }

    override fun onPause() {
        wifiLock?.release()
        discovery.stop()
        super.onPause()
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

    // StreamSession.Listener: called on the session thread.

    override fun onStatus(text: String) = runOnUiThread {
        // Quiet retries while waiting for the computer: keep the calm waiting state.
        if (state.waiting) return@runOnUiThread
        state.status = text
        state.busy = true
        if (state.screen == Screen.Streaming) {
            showScreen(if (state.computerName != null) Screen.Home else Screen.Pairing)
        }
    }

    override fun onWaiting(text: String) = runOnUiThread {
        state.waiting = true
        state.busy = false
        state.error = null
        state.status = text
        // The computer may have a new address: look for it by its announced id.
        if (pendingTarget == null) discovery.start()
    }

    /** The paired computer shows up at a new address: save it and connect there. */
    private fun followMovedHost(found: List<FoundComputer>) {
        if (!state.waiting || pendingTarget != null) return
        val host = hosts.load() ?: return
        val moved = relocated(host, found) ?: return
        val name = hosts.name()?.takeIf { it != host.address } ?: moved.address
        hosts.save(moved, name)
        state.computerName = name
        restartSession()
    }

    override fun onStreaming(width: Int, height: Int, decoder: String, lowLatency: Boolean) = runOnUiThread {
        state.waiting = false
        state.code = null
        state.error = null
        stats.text = getString(R.string.streaming, width, height, decoder, if (lowLatency) " (low latency)" else "")
        showScreen(Screen.Streaming)
    }

    override fun onPaired(host: Host) = runOnUiThread {
        hosts.save(host, pendingName)
        state.computerName = hosts.name()
        pendingTarget = null
        pendingName = null
    }

    override fun onConfirmCode(code: String, computer: String) = runOnUiThread {
        state.code = code
        state.codeComputer = computer
        showScreen(Screen.Pairing)
    }

    override fun onStats(fps: Double, mbps: Double, decodeMs: Double) = runOnUiThread {
        stats.text = getString(R.string.stats, fps, mbps, decodeMs)
    }

    override fun onGaveUp(reason: String) = runOnUiThread {
        pendingTarget = null
        pendingName = null
        state.code = null
        state.busy = false
        state.error = reason
        showScreen(if (state.computerName != null) Screen.Home else Screen.Pairing)
    }

    private companion object {
        const val MAX_FPS = 60.0
        const val SPLASH_EXIT_MS = 300L
    }
}
