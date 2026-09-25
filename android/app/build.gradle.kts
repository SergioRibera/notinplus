import org.gradle.api.tasks.Exec

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    // Auto-discovers `native/android/` from every istmo plugin the
    // notinplus crate depends on and injects the directory into this
    // module's main Kotlin source set — no manual copy, no `srcDirs`.
    id("dev.istmo.plugin-loader")
}

// The plugin loader walks up from `project.rootDir` looking for a
// Cargo workspace; notinplus is a single-crate repo so we point it at
// the sibling istmo checkout explicitly.
istmo {
    workspaceRoot.set(file("../../../istmo"))
}

android {
    namespace  = "rs.sergioribera.notinplus"
    compileSdk = 34

    defaultConfig {
        applicationId = "rs.sergioribera.notinplus"
        minSdk        = 26
        targetSdk     = 36
        versionCode   = 1
        versionName   = "0.1.0"
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
        getByName("release") {
            isMinifyEnabled = false
            
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    packaging { jniLibs { useLegacyPackaging = false } }
}

android.sourceSets["main"].jniLibs.setSrcDirs(
    listOf(layout.buildDirectory.dir("rustJniLibs").get().asFile),
)

val abiToRustTarget = mapOf(
    "arm64-v8a"    to "aarch64-linux-android",
    "armeabi-v7a"  to "armv7-linux-androideabi",
    "x86_64"       to "x86_64-linux-android",
    "x86"          to "i686-linux-android",
)

val cargoRoot: File = project.rootDir.resolve("..").normalize()
val rustJniLibsDir = layout.buildDirectory.dir("rustJniLibs")

val cargoStageTaskNames = mutableListOf<String>()

fun cargoLib(crate: String, libName: String = crate.replace('-', '_')) {
    val soName = "lib$libName.so"
    val abis = android.defaultConfig.ndk.abiFilters
    for (abi in abis) {
        val rustTarget = abiToRustTarget[abi] ?: error("no rust target for '$abi'")
        val suffix     = "${libName}_${abi.replace('-', '_')}"
        val cargoSo    = cargoRoot.resolve("target/$rustTarget/release/$soName")
        val stagedSo   = rustJniLibsDir.map { it.dir(abi).file(soName) }

        val cargoTask = tasks.register("cargoBuild_$suffix", Exec::class) {
            group       = "istmo"
            workingDir  = cargoRoot
            commandLine("cargo", "build", "--release",
                        "--target", rustTarget, "-p", crate)
            outputs.file(cargoSo)
        }
        val stageTask = tasks.register("stageRustLib_$suffix") {
            group      = "istmo"
            dependsOn(cargoTask)
            inputs.file(cargoSo)
            outputs.file(stagedSo)
            doLast {
                val dst = stagedSo.get().asFile
                dst.parentFile.mkdirs()
                cargoSo.copyTo(dst, overwrite = true)
            }
        }
        cargoStageTaskNames.add(stageTask.name)
    }
}

afterEvaluate {
    tasks.matching { it.name.matches(Regex("merge.*JniLibFolders")) }
        .configureEach { cargoStageTaskNames.forEach { dependsOn(it) } }
}

cargoLib("notinplus")

dependencies {
    // Runtime substitution wired in `settings.gradle.kts` — the
    // includeBuild call replaces this coordinate with the local
    // `runtime/android` project. Version is a placeholder.
    implementation("dev.istmo:istmo-runtime")
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
