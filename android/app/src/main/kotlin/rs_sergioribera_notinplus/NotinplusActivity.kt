package rs.sergioribera.notinplus

import android.os.Bundle
import android.view.MotionEvent
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.androidgamesdk.GameActivity
import dev.istmo.plugins.pen.PenCaptureView
import dev.istmo.plugins.pen.PenFactoryImpl
import dev.istmo.runtime.IstmoPluginRegistry
import dev.istmo.runtime.IstmoRuntime
import dev.istmo.runtime.PenCodecsImpl
import dev.istmo.runtime.PenDispatcher

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
 */
class NotinplusActivity : GameActivity() {

    private lateinit var penView: PenCaptureView

    override fun onCreate(savedInstanceState: Bundle?) {
        // Load the `notinplus` cdylib and start the istmo runtime before
        // GameActivity's super.onCreate spawns android_main — otherwise
        // Rust code could try to talk to a runtime that isn't up yet.
        check(IstmoRuntime.start("notinplus")) { "IstmoRuntime.start() failed" }

        // Auto-register every plugin whose `istmo.toml` declares
        // `auto_register = true`. Pen opts out (bespoke construction);
        // wire it manually below.
        IstmoPluginRegistry.registerAll(applicationContext)

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

    override fun onDestroy() {
        super.onDestroy()
        IstmoRuntime.shutdown()
    }
}
