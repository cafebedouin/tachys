plugins {
    id("com.android.application") version "9.5.1" apply false
    // the shared module: Kotlin for several platforms, as an Android library and a program for the JVM, with Compose Multiplatform
    id("com.android.kotlin.multiplatform.library") version "9.5.1" apply false
    id("org.jetbrains.kotlin.multiplatform") version "2.4.21" apply false
    id("org.jetbrains.compose") version "1.12.2" apply false
    // Kotlin itself comes with the Android plugin (9.0 and later); the Compose compiler's version is Kotlin's
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.21" apply false
}
