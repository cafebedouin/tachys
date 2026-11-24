package org.tachys.wallet

import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf

/** The few settings the app keeps beside its wallets (the network chosen, a node named, the payments made), by name. */
interface Prefs {
    fun getString(key: String, otherwise: String?): String?
    fun getBoolean(key: String, otherwise: Boolean): Boolean
    fun put(key: String, value: String)
    fun put(key: String, value: Boolean)
    fun remove(key: String)
}

/**
 * What the screens need of the machine they run on, and nothing else of it: the screens and the wallet logic are the
 * same on every platform, and each platform gives one of these (a phone: `MainActivity.kt`; a computer: `Main.kt`).
 */
interface Platform {
    /** The directory the wallets live in, one folder each. */
    val files: FileAt
    val prefs: Prefs
    /** A file shipped with the app (`seeds-mainnet.txt`, `tokens.json`). */
    fun asset(name: String): ByteArray
    /** There is a screen lock that can guard seed phrases and the app. Without one every wallet has a password of its own. */
    val lock: Boolean
    /** There is a camera to scan codes with. */
    val camera: Boolean
    /** What this machine is called in a sentence: "phone", "computer". */
    val device: String
    /** A moment (milliseconds since 1970) as the day and time here, for a list: "3 Nov, 14:05". */
    fun time(ms: Long): String
    /** Puts text where the user can paste it from. */
    fun copy(label: String, text: String)
    /** While part of the screen: the back button (where there is one) does `onBack` if `enabled`. */
    @Composable fun Back(enabled: Boolean, onBack: () -> Unit)
    /** While part of the screen: the screen cannot be captured or shown among recent apps (where that can be asked). */
    @Composable fun Secure()
    /** The camera reading a QR code. Never shown where `camera` is false. */
    @Composable fun Scanner(title: String, onCode: (String) -> Unit, onCancel: () -> Unit)
}

/** The platform the screens are on; `App` provides it. */
val LocalPlatform = staticCompositionLocalOf<Platform> { error("no platform given") }

@Composable fun BackHandler(enabled: Boolean = true, onBack: () -> Unit) = LocalPlatform.current.Back(enabled, onBack)
@Composable fun Scanner(title: String, onCode: (String) -> Unit, onCancel: () -> Unit) = LocalPlatform.current.Scanner(title, onCode, onCancel)

/** The moment now, in milliseconds since 1970. */
@OptIn(kotlin.time.ExperimentalTime::class)
fun now(): Long = kotlin.time.Clock.System.now().toEpochMilliseconds()
