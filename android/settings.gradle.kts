pluginManagement { repositories { google(); mavenCentral(); gradlePluginPortal() } }
dependencyResolutionManagement { repositories { google(); mavenCentral() } }
rootProject.name = "tachys-android"
// shared: the screens and the wallet logic, for every platform, and the app for a computer. app: the Android app around them.
include(":shared", ":app")
