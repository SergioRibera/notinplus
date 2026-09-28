import org.gradle.api.tasks.Exec
import java.net.URI

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    // Builds the Rust crate for every ABI, links every istmo plugin's
    // `native/android/` sources + AndroidManifest fragments, and applies
    // `[app]` identity (id / version / minSdk / label / icon) from
    // `istmo.toml`. Fetched from Maven Central via `pluginManagement`.
    id("io.github.sergioribera.istmo")
}

android {
    namespace  = "rs.sergioribera.notinplus"
    compileSdk = 34

    defaultConfig {
        // `applicationId`, `versionCode`, `versionName` and `minSdk`
        // come from `istmo.toml` `[app]` + `[min_versions]` — the
        // gradle plugin injects them and warns on manual overrides.
        targetSdk = 36
        ndk { abiFilters += setOf("arm64-v8a") }
    }

    signingConfigs {
        getByName("debug") {
            storeFile     = file(System.getenv("ANDROID_DEBUG_KEYSTORE")
                ?: "${System.getProperty("user.home")}/.android/debug.keystore")
            storePassword = "android"
            keyAlias      = "androiddebugkey"
            keyPassword   = "android"
        }
    }

    buildTypes {
        getByName("debug")   { signingConfig = signingConfigs.getByName("debug") }
        getByName("release") { isMinifyEnabled = false }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    packaging { jniLibs { useLegacyPackaging = false } }
}

// -----------------------------------------------------------------
// pdfium shipping. `freya-pdf` loads pdfium at runtime via
// `Pdfium::bind_to_system_library`, which delegates to `dlopen` on
// `libpdfium.so`. Android's dynamic linker searches the app's
// `nativeLibraryDir` before the system paths, so dropping the
// prebuilt `libpdfium.so` alongside our Rust `.so`s is enough for the
// runtime to pick it up.
//
// The istmo gradle plugin registers `build/istmo/jniLibs/<buildType>/<abi>`
// as a jniLibs srcDir for the compiled Rust cdylib; we add our own
// build-type-agnostic staging dir alongside it for pdfium.
// -----------------------------------------------------------------
// Pinned to match the ABI baseline `pdfium-render 0.9` binds against
// (`pdfium_latest = pdfium_7881`). Bump this in lock-step with any
// `pdfium-render` upgrade — the bindgen'd struct layouts and function
// signatures must match the shipped binary or FPDF_* calls corrupt
// allocator state.
val pdfiumRelease   = "chromium/7881"
val pdfiumBaseUrl   = "https://github.com/bblanchon/pdfium-binaries/releases/download/$pdfiumRelease"
val pdfiumAbiSuffix = mapOf(
    "arm64-v8a"   to "arm64",
    "armeabi-v7a" to "arm",
    "x86_64"      to "x64",
    "x86"         to "x86",
)
val pdfiumCacheDir       = layout.buildDirectory.dir("pdfium-cache")
val pdfiumJniLibsDir     = layout.buildDirectory.dir("pdfium-jniLibs")
val pdfiumStageTaskNames = mutableListOf<String>()

for (abi in android.defaultConfig.ndk.abiFilters) {
    val suffix   = pdfiumAbiSuffix[abi] ?: continue
    val tgzName  = "pdfium-android-$suffix.tgz"
    val tgzUrl   = "$pdfiumBaseUrl/$tgzName"
    val tgzFile  = pdfiumCacheDir.map { it.file(tgzName) }
    val unpacked = pdfiumCacheDir.map { it.dir("pdfium-android-$suffix") }
    val stagedSo = pdfiumJniLibsDir.map { it.dir(abi).file("libpdfium.so") }

    val downloadTask = tasks.register("downloadPdfium_$suffix") {
        group   = "istmo"
        outputs.file(tgzFile)
        doLast {
            val dst = tgzFile.get().asFile
            if (dst.exists() && dst.length() > 0) return@doLast
            dst.parentFile.mkdirs()
            logger.lifecycle("Fetching $tgzUrl")
            URI(tgzUrl).toURL().openStream().use { input ->
                dst.outputStream().use { output -> input.copyTo(output) }
            }
        }
    }

    val extractTask = tasks.register("extractPdfium_$suffix", Exec::class) {
        group      = "istmo"
        dependsOn(downloadTask)
        inputs.file(tgzFile)
        outputs.dir(unpacked)
        doFirst { unpacked.get().asFile.mkdirs() }
        workingDir = pdfiumCacheDir.get().asFile
        commandLine("tar", "-xzf", tgzFile.get().asFile.absolutePath,
                    "-C", unpacked.get().asFile.absolutePath)
    }

    val stageTask = tasks.register("stagePdfium_$suffix") {
        group      = "istmo"
        dependsOn(extractTask)
        outputs.file(stagedSo)
        doLast {
            val src = unpacked.get().asFile.resolve("lib/libpdfium.so")
            require(src.exists()) { "libpdfium.so not found under ${src.parentFile}" }
            val dst = stagedSo.get().asFile
            dst.parentFile.mkdirs()
            src.copyTo(dst, overwrite = true)
        }
    }
    pdfiumStageTaskNames.add(stageTask.name)
}

android.sourceSets["main"].jniLibs.srcDir(pdfiumJniLibsDir)

afterEvaluate {
    tasks.matching { it.name.matches(Regex("merge.*JniLibFolders")) }
        .configureEach { pdfiumStageTaskNames.forEach { dependsOn(it) } }
}

dependencies {
    // Runtime AAR published to Maven Central. Version tracks the
    // gradle plugin above — bump in lock-step.
    implementation("io.github.sergioribera:istmo-runtime:0.1.1-alpha.3")
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    // GameActivity dispatches MotionEvents through Java first, so the
    // Kotlin `dispatchTouchEvent` override in NotinplusActivity can
    // spy on stylus samples before winit's native side consumes the
    // raw InputQueue. Required by the `android-game-activity` feature
    // enabled on winit in Cargo.toml.
    implementation("androidx.games:games-activity:4.4.0")
}
