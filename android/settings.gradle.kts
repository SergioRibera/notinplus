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
        // istmo-runtime is published to GitHub Packages. Provide creds via
        // env vars (CI: GITHUB_ACTOR / GITHUB_TOKEN) or gradle.properties.
        maven {
            url = uri("https://maven.pkg.github.com/SergioRibera/istmo")
            credentials {
                username = System.getenv("GITHUB_ACTOR")
                    ?: providers.gradleProperty("gpr.user").orNull
                password = System.getenv("GITHUB_TOKEN")
                    ?: providers.gradleProperty("gpr.key").orNull
            }
        }
    }
}

rootProject.name = "notinplus"
include(":app")
