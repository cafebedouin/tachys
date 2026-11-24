package org.tachys.wallet

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Window
import androidx.compose.ui.window.application
import androidx.compose.ui.window.rememberWindowState
import java.awt.Toolkit
import java.awt.datatransfer.StringSelection
import java.io.File

// The app on a computer (Linux, Windows, macOS): the same screens in a window. It loads the core as the phone does,
// through the JNI entry of the library `tachys_c` (crates/tachys-c, built with `--features jni`), found on
// java.library.path. A computer gives no screen lock to this app, so every wallet here has a password of its own,
// and there is no camera: codes are pasted.

/** A computer, as the screens need it. `dir`: where its wallets and settings are kept. */
class Computer(dir: File) : Platform {
    private val home: File = dir.also { d ->
        if (!d.isDirectory && !d.mkdirs()) error("could not make ${d.path}")
        // the owner's alone, where the system has such a thing: the folder holds wallet files
        try { java.nio.file.Files.setPosixFilePermissions(d.toPath(), java.nio.file.attribute.PosixFilePermissions.fromString("rwx------")) } catch (e: Exception) { }
    }
    override val files = FileAt(home.absolutePath)
    /** Settings as one JSON object in `settings.json`, written whole on each change (to a file beside it first, then moved over it). */
    override val prefs: Prefs = object : Prefs {
        private val file = File(home, "settings.json")
        private val all: JSONObject = try { JSONObject(file.readText()) } catch (e: Exception) { JSONObject() }
        @Synchronized private fun save() { val t = File(home, "settings.json.new"); t.writeText(all.toString()); java.nio.file.Files.move(t.toPath(), file.toPath(), java.nio.file.StandardCopyOption.REPLACE_EXISTING) }
        @Synchronized override fun getString(key: String, otherwise: String?): String? = if (all.has(key)) all.optString(key) else otherwise
        @Synchronized override fun getBoolean(key: String, otherwise: Boolean): Boolean = all.optBoolean(key, otherwise)
        @Synchronized override fun put(key: String, value: String) { all.put(key, value); save() }
        @Synchronized override fun put(key: String, value: Boolean) { all.put(key, value); save() }
        @Synchronized override fun remove(key: String) { all.remove(key); save() }
    }
    override fun asset(name: String): ByteArray = (Computer::class.java.getResourceAsStream("/$name") ?: error("$name is not in this build")).use { it.readBytes() }
    override val lock = false
    override val device = "computer"
    override fun time(ms: Long): String = java.text.SimpleDateFormat("d MMM, HH:mm", java.util.Locale.getDefault()).format(java.util.Date(ms))
    override val camera = false
    override fun copy(label: String, text: String) { Toolkit.getDefaultToolkit().systemClipboard.setContents(StringSelection(text), null) }
    @Composable override fun Back(enabled: Boolean, onBack: () -> Unit) { }
    @Composable override fun Secure() { }
    @Composable override fun Scanner(title: String, onCode: (String) -> Unit, onCancel: () -> Unit) { }
    companion object {
        /** TACHYS_APP_DIR if set; otherwise the place this system keeps a program's data, in a folder `tachys-app`. */
        fun home(): File {
            System.getenv("TACHYS_APP_DIR")?.takeIf { it.isNotBlank() }?.let { return File(it) }
            val user = System.getProperty("user.home")
            val os = System.getProperty("os.name").lowercase()
            return when {
                os.contains("win") -> File(System.getenv("APPDATA") ?: "$user\\AppData\\Roaming", "tachys-app")
                os.contains("mac") -> File(user, "Library/Application Support/tachys-app")
                else -> File(System.getenv("XDG_DATA_HOME")?.takeIf { it.isNotBlank() } ?: "$user/.local/share", "tachys-app")
            }
        }
    }
}

/** No screen lock is asked for here: nothing is guarded by one. */
val noLock = Guard({ false }, { _, done -> done(true) }, { _, _, done -> done(null, "This computer keeps no key for the app: a wallet here has a password of its own.") })

fun main() = application {
    val core = remember { Core(Computer(Computer.home())) }
    Window(onCloseRequest = ::exitApplication, title = "tachys", state = rememberWindowState(width = 430.dp, height = 880.dp)) {
        App(core, "", noLock, false) { }
    }
}
