pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
    // Local checkout of the istmo repo provides the plugin loader
    // (`dev.istmo.plugin-loader`) that auto-injects every declared
    // plugin's `native/android/` sources into this app's source set.
    includeBuild("../../istmo/runtime/gradle-plugin")
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "notinplus"
include(":app")

// Runtime AAR pulled from the same istmo checkout — no maven creds
// required. Swap this for a maven coordinate once a release is tagged.
includeBuild("../../istmo/runtime/android") {
    dependencySubstitution {
        substitute(module("dev.istmo:istmo-runtime"))
            .using(project(":"))
    }
}
