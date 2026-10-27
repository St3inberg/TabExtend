package io.github.st3inberg.tabextend

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.view.Gravity
import android.widget.FrameLayout
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import com.google.zxing.BinaryBitmap
import com.google.zxing.LuminanceSource
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.ReaderException
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * Scans the QR code `tabextend pair` shows and returns the pairing link.
 * Anything that is not a TabExtend pairing link is ignored.
 */
class ScannerActivity : ComponentActivity() {
    private lateinit var preview: PreviewView
    private lateinit var analysis: ExecutorService
    @Volatile private var done = false

    private val askCamera = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) startCamera() else finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        analysis = Executors.newSingleThreadExecutor()
        preview = PreviewView(this)
        val hint = TextView(this).apply {
            text = getString(R.string.scan_hint)
            setTextColor(Color.WHITE)
            setBackgroundColor(Color.argb(160, 0, 0, 0))
            textSize = 18f
            gravity = Gravity.CENTER
            setPadding(32, 24, 32, 24)
        }
        setContentView(FrameLayout(this).apply {
            addView(preview, FrameLayout.LayoutParams(-1, -1))
            addView(hint, FrameLayout.LayoutParams(-1, -2, Gravity.BOTTOM))
        })
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            startCamera()
        } else {
            askCamera.launch(Manifest.permission.CAMERA)
        }
    }

    private fun startCamera() {
        val future = ProcessCameraProvider.getInstance(this)
        future.addListener({
            val provider = future.get()
            val previewUse = Preview.Builder().build().also { it.surfaceProvider = preview.surfaceProvider }
            val analyzer = ImageAnalysis.Builder()
                .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                .build()
                .also { it.setAnalyzer(analysis, ::analyze) }
            val camera = listOf(CameraSelector.DEFAULT_BACK_CAMERA, CameraSelector.DEFAULT_FRONT_CAMERA)
                .firstOrNull { provider.hasCamera(it) } ?: return@addListener finish()
            provider.unbindAll()
            provider.bindToLifecycle(this, camera, previewUse, analyzer)
        }, mainExecutor)
    }

    private fun analyze(image: ImageProxy) {
        image.use {
            if (done) return
            val plane = it.planes[0]
            val bytes = ByteArray(plane.buffer.remaining()).also { b -> plane.buffer.get(b) }
            val source = PlanarYUVLuminanceSource(
                bytes, plane.rowStride, it.height, 0, 0, it.width, it.height, false,
            )
            // The terminal draws the code light-on-dark; try both ways round.
            val text = decode(source) ?: decode(source.invert()) ?: return
            if (PairingLink.parse(text) == null) return
            done = true
            runOnUiThread {
                setResult(RESULT_OK, Intent().setData(Uri.parse(text)))
                finish()
            }
        }
    }

    private fun decode(source: LuminanceSource): String? = try {
        QRCodeReader().decode(BinaryBitmap(HybridBinarizer(source))).text
    } catch (e: ReaderException) {
        null
    }

    override fun onDestroy() {
        analysis.shutdown()
        super.onDestroy()
    }
}
