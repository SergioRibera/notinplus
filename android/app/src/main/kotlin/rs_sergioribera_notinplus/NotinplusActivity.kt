package rs.sergioribera.notinplus

import android.content.Context
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Bundle
import android.view.MotionEvent
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.androidgamesdk.GameActivity
import dev.istmo.plugins.pen.PenCaptureView
import dev.istmo.plugins.pen.PenFactoryImpl
import dev.istmo.runtime.Bincode
import dev.istmo.runtime.IstmoHost
import dev.istmo.runtime.IstmoRuntime
import dev.istmo.runtime.PenCodecsImpl
import dev.istmo.runtime.PenDispatcher
import java.io.ByteArrayOutputStream

/**
 * GameActivity (androidx.games:games-activity) forwards every
 * MotionEvent through Java `dispatchTouchEvent` /
 * `dispatchGenericMotionEvent` before handing off to the native
 * (winit / freya) side. That gives us a clean Kotlin seam to intercept
 * stylus samples — impossible under NativeActivity, which takes the
 * raw InputQueue and skips the view hierarchy entirely.
 *
 * PenCaptureView is instantiated as a detached observer (not added to
 * the view tree). We call its onTouchEvent / onGenericMotionEvent
 * directly from the dispatch overrides, then delegate to super so
 * GameActivity's native forwarding still delivers the same events to
 * winit / freya on the Rust side.
 *
 * `IstmoHost.onCreate` starts the runtime, loads `libnotinplus.so` and
 * runs the generated `IstmoPluginRegistry` — which auto-registers
 * data-store and file-picker. Pen opts out (`auto_register = false`)
 * because its factory needs the capture view; we wire it manually.
 */
class NotinplusActivity : GameActivity() {

    private lateinit var penView: PenCaptureView

    /**
     * Devices on the same Wi-Fi find each other through UDP broadcast,
     * which Android filters out unless the app holds a multicast lock.
     */
    private var multicastLock: WifiManager.MulticastLock? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        IstmoHost.onCreate(this)
        acquireMulticastLock()

        penView = PenCaptureView(this)
        IstmoRuntime.registerHandler(
            PenDispatcher.PLUGIN_ID,
            PenDispatcher(PenFactoryImpl(penView), PenCodecsImpl()),
        )

        super.onCreate(savedInstanceState)

        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            show(WindowInsetsCompat.Type.systemBars())
            isAppearanceLightStatusBars = true
        }
        installSafeAreaListener()
        publishDeepLink(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        publishDeepLink(intent)
    }

    override fun dispatchTouchEvent(event: MotionEvent): Boolean {
        penView.onTouchEvent(event)
        return super.dispatchTouchEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        penView.onGenericMotionEvent(event)
        return super.dispatchGenericMotionEvent(event)
    }

    private fun installSafeAreaListener() {
        val density = resources.displayMetrics.density
        val root = window.decorView
        ViewCompat.setOnApplyWindowInsetsListener(root) { _, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val ime = insets.getInsets(WindowInsetsCompat.Type.ime())
            val cutout = insets.getInsets(WindowInsetsCompat.Type.displayCutout())
            IstmoRuntime.publishSafeArea(
                bars.top / density, bars.right / density, bars.bottom / density, bars.left / density,
                ime.top / density, ime.right / density, ime.bottom / density, ime.left / density,
                cutout.top / density, cutout.right / density, cutout.bottom / density, cutout.left / density,
            )
            insets
        }
        ViewCompat.requestApplyInsets(root)
    }

    /**
     * Hand a `notein://` URL to the Rust side. Cold-start intents and
     * hot opens go through the same early-event queue, so a link that
     * arrives before the router listens is kept until it does.
     */
    private fun publishDeepLink(intent: Intent?) {
        if (intent?.action != Intent.ACTION_VIEW) return
        val uri = intent.dataString ?: return
        val out = ByteArrayOutputStream()
        Bincode.writeString(out, uri)
        Bincode.writeOption(out, "intent") { o, s -> Bincode.writeString(o, s) }
        Bincode.writeOption(out, System.currentTimeMillis()) { o, ms -> Bincode.writeVarintU64(o, ms) }
        IstmoRuntime.publishEarlyQueue(DEEPLINKS_CHANNEL, DEEPLINKS_CAPACITY, out.toByteArray())
    }

    private fun acquireMulticastLock() {
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager ?: return
        multicastLock = wifi.createMulticastLock("notinplus-discovery").apply {
            setReferenceCounted(false)
            acquire()
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        multicastLock?.takeIf { it.isHeld }?.release()
        multicastLock = null
        IstmoHost.onDestroy(this)
    }

    private companion object {
        const val DEEPLINKS_CHANNEL = "istmo.deeplinks"
        const val DEEPLINKS_CAPACITY = 16
    }
}
