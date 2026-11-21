@file:Suppress("DEPRECATION")

package org.tachys.wallet

import android.Manifest
import android.content.pm.PackageManager
import android.graphics.SurfaceTexture
import android.hardware.Camera
import android.os.Handler
import android.os.Looper
import android.view.TextureView
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.material.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.google.zxing.BinaryBitmap
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

// QR scanner: the old camera API (works from Android 7 up, needs no library) feeding preview frames
// to the ZXing QR reader a few times a second. Reports the first code it reads.

@Composable
fun Scanner(prompt: String, onResult: (String) -> Unit, onCancel: () -> Unit) {
    val ctx = LocalContext.current
    var granted by remember { mutableStateOf(ctx.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    LaunchedEffect(Unit) { if (!granted) ask.launch(Manifest.permission.CAMERA) }

    CardBox {
        Text(prompt, color = Accent, fontSize = 20.sp)
        Spacer(Modifier.height(10.dp))
        if (!granted) {
            Text("The camera is needed to scan a code. Nothing is recorded or sent.", color = Color.White, fontSize = 15.sp)
        } else {
            val done = remember { AtomicBoolean(false) }
            AndroidView(modifier = Modifier.fillMaxWidth().height(320.dp), factory = { c ->
                TextureView(c).apply {
                    surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                        var camera: Camera? = null
                        val worker = Executors.newSingleThreadExecutor()
                        val busy = AtomicBoolean(false)
                        val main = Handler(Looper.getMainLooper())
                        override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                            try {
                                val cam = Camera.open() ?: return
                                camera = cam
                                val p = cam.parameters
                                // a modest frame is plenty for a QR code and cheap to decode on an old phone
                                val size = p.supportedPreviewSizes.minByOrNull { kotlin.math.abs(it.width * it.height - 640 * 480) }
                                if (size != null) p.setPreviewSize(size.width, size.height)
                                if (p.supportedFocusModes?.contains(Camera.Parameters.FOCUS_MODE_CONTINUOUS_PICTURE) == true) p.focusMode = Camera.Parameters.FOCUS_MODE_CONTINUOUS_PICTURE
                                cam.parameters = p
                                cam.setDisplayOrientation(90)
                                cam.setPreviewTexture(st)
                                val ps = cam.parameters.previewSize
                                cam.setPreviewCallback { data, _ ->
                                    if (done.get() || !busy.compareAndSet(false, true)) return@setPreviewCallback
                                    worker.execute {
                                        try {
                                            val src = PlanarYUVLuminanceSource(data, ps.width, ps.height, 0, 0, ps.width, ps.height, false)
                                            val text = QRCodeReader().decode(BinaryBitmap(HybridBinarizer(src))).text
                                            if (text != null && done.compareAndSet(false, true)) main.post { onResult(text) }
                                        } catch (_: Exception) {
                                            // no code in this frame
                                        } finally {
                                            Thread.sleep(150); busy.set(false)
                                        }
                                    }
                                }
                                cam.startPreview()
                            } catch (_: Exception) { }
                        }
                        override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {}
                        override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
                        override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                            camera?.let { it.setPreviewCallback(null); it.stopPreview(); it.release() }
                            camera = null; worker.shutdown(); return true
                        }
                    }
                }
            })
        }
        Spacer(Modifier.height(10.dp))
        Pill("Cancel", Bg, Color.White, Modifier.fillMaxWidth(), onClick = onCancel)
    }
}

/** What a scanned code means for a field that wants an address: plain, or an `ergo:` payment link. */
fun addressFromCode(code: String): String = payRequestFromCode(code).address

/** What a scanned code asks to be paid: an address, and perhaps an amount of ERG and a note. `tokens` = the request also names tokens. */
class PayRequest(val address: String, val erg: String = "", val note: String = "", val tokens: Boolean = false)

/**
 * Reads a bare address, an `ergo:ADDRESS?amount=…` link, or the payment request other Ergo wallets show
 * (`https://explorer.ergoplatform.com/payment-request?address=…&amount=…&description=…`, tokens as `token-ID=…`).
 */
fun payRequestFromCode(code: String): PayRequest {
    val c = code.trim()
    val query = c.substringAfter('?', "")
    val params = query.split('&').filter { it.contains('=') }.associate { it.substringBefore('=') to try { java.net.URLDecoder.decode(it.substringAfter('='), "UTF-8") } catch (e: Exception) { "" } }
    val address = (if (c.startsWith("http://") || c.startsWith("https://")) params["address"] ?: "" else c.removePrefix("ergo:").substringBefore('?')).filter { it.isLetterOrDigit() }
    val erg = (params["amount"] ?: "").filter { it.isDigit() || it == '.' }.takeIf { (it.toBigDecimalOrNull() ?: java.math.BigDecimal.ZERO).signum() > 0 } ?: ""
    return PayRequest(address, erg, (params["description"] ?: "").take(80), params.keys.any { it.startsWith("token-") })
}

/** What a scanned code means for a field that wants a box id: the code itself if it is one (64 hex characters, either
 *  case), and nothing otherwise. A receipt's link form is read by `Receipt.parse`, which gives this its `box`. */
fun boxIdFromCode(code: String): String = code.trim().lowercase().takeIf { it.length == 64 && it.all { c -> c in '0'..'9' || c in 'a'..'f' } } ?: ""

/** What is typed or pasted into a box-id field, as it is typed: its hex characters, at most 64. */
fun boxIdTyped(text: String): String = text.lowercase().filter { it in '0'..'9' || it in 'a'..'f' }.take(64)
