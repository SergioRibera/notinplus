package rs.sergioribera.notinplus

import android.app.NativeActivity
import android.os.Bundle
import dev.istmo.runtime.IstmoRuntime

// NativeActivity subclass that boots IstmoRuntime + registers plugin
// dispatchers BEFORE `android_main` fires in the Rust cdylib.
class NotinplusActivity : NativeActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        IstmoRuntime.instance.start(this)

        // TODO: register your plugin dispatchers here — the generated
        // classes live under `rs.sergioribera.notinplus.gen`.
        //
        // IstmoRuntime.instance.registerHandler(
        //     DataStoreDispatcher.PLUGIN_ID,
        //     DataStoreDispatcher(DataStoreBackendImpl(this), DataStoreCodecsImpl()),
        // )

        super.onCreate(savedInstanceState)
    }
}
