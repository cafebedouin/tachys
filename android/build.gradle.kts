plugins {
    id("com.android.application") version "9.5.1" apply false
    // Kotlin itself comes with the Android plugin (9.0 and later); only the Compose compiler is named, and its version is Kotlin's
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.21" apply false
}
