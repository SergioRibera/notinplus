pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

// Composite build: picks up local edits to the istmo runtime (sibling
// repo at `../../istmo/runtime/android`) without having to publish a
// new Maven artifact for every change. The explicit
// `dependencySubstitution` is required because the included build
// does not set `project.group` on its root module — Vanniktech's
// `mavenPublishing { coordinates(...) }` only tags the publication,
// not the project itself, so Gradle's default group+artifact
// substitution would miss. Drop this block once the upstream alpha
// is bumped on Maven Central.
val istmoRuntimePath = rootDir.resolve("../../istmo/runtime/android")
if (istmoRuntimePath.resolve("settings.gradle.kts").exists()) {
    includeBuild(istmoRuntimePath) {
        dependencySubstitution {
            substitute(module("io.github.sergioribera:istmo-runtime"))
                .using(project(":"))
        }
    }
}

rootProject.name = "notinplus"
include(":app")
