package org.tachys.wallet

import android.app.Activity
import android.app.KeyguardManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.Bundle
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.UserNotAuthenticatedException
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.mutableStateOf
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

// The Android app: what a phone gives the screens (`Platform`), the keystore that guards seed phrases behind the
// screen lock, and the activity. The screens and the wallet logic are in the shared module (`shared/`).

/** A phone, as the screens need it. */
class Phone(private val act: Activity) : Platform {
    override val files: FileAt get() = FileAt(act.filesDir.absolutePath)
    override val prefs: Prefs = object : Prefs {
        private val p = act.getSharedPreferences("tachys", Context.MODE_PRIVATE)
        override fun getString(key: String, otherwise: String?) = p.getString(key, otherwise)
        override fun getBoolean(key: String, otherwise: Boolean) = p.getBoolean(key, otherwise)
        override fun put(key: String, value: String) = p.edit().putString(key, value).apply()
        override fun put(key: String, value: Boolean) = p.edit().putBoolean(key, value).apply()
        override fun remove(key: String) = p.edit().remove(key).apply()
    }
    override fun asset(name: String): ByteArray = act.assets.open(name).use { it.readBytes() }
    override val lock = true
    override val device = "phone"
    override fun time(ms: Long): String = android.text.format.DateFormat.format("d MMM, HH:mm", ms).toString()
    override val camera = true
    override fun copy(label: String, text: String) { (act.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText(label, text)) }
    @Composable override fun Back(enabled: Boolean, onBack: () -> Unit) = androidx.activity.compose.BackHandler(enabled, onBack)
    @Composable override fun Secure() {
        DisposableEffect(Unit) {
            act.window?.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
            onDispose { act.window?.clearFlags(WindowManager.LayoutParams.FLAG_SECURE) }
        }
    }
    @Composable override fun Scanner(title: String, onCode: (String) -> Unit, onCancel: () -> Unit) = CameraScanner(title, onCode, onCancel)
}

/**
 * Keeps the password that seals every seed phrase on this phone. The password is random, is never shown, and is stored
 * only wrapped by a key inside the phone's keystore. On a phone with a screen lock that key works only for a few seconds
 * after the screen lock has been given, so the phrases cannot be opened, even by this app, without it. The phone destroys
 * such a key if the screen lock is ever removed; the phrases on the phone are then closed for good and the written words
 * are the way back.
 */
class Vault(ctx: Context) {
    private val prefs = ctx.getSharedPreferences("tachys", Context.MODE_PRIVATE)
    private val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    private fun make(alias: String, guarded: Boolean): SecretKey {
        ks.deleteEntry(alias)
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).setKeySize(256)
        if (guarded) {
            spec.setUserAuthenticationRequired(true)
            if (Build.VERSION.SDK_INT >= 30) spec.setUserAuthenticationParameters(WINDOW, KeyProperties.AUTH_DEVICE_CREDENTIAL or KeyProperties.AUTH_BIOMETRIC_STRONG)
            else @Suppress("DEPRECATION") spec.setUserAuthenticationValidityDurationSeconds(WINDOW)
        }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply { init(spec.build()) }.generateKey()
    }
    private fun wrap(key: SecretKey, pass: String): String {
        val c = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key) }
        return android.util.Base64.encodeToString(c.iv + c.doFinal(pass.toByteArray()), android.util.Base64.NO_WRAP)
    }
    private fun unwrap(key: SecretKey, stored: String): String {
        val b = android.util.Base64.decode(stored, android.util.Base64.NO_WRAP)
        val c = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, b, 0, 12)) }
        return String(c.doFinal(b, 12, b.size - 12))
    }
    private fun store(wrapped: String, alias: String) { if (!prefs.edit().putString("vault", wrapped).putString("vault-key", alias).commit()) throw IllegalStateException("could not save the wrapped password") }
    /**
     * The password. Throws UserNotAuthenticatedException when the screen lock has to be given first, and
     * KeyPermanentlyInvalidatedException when the keystore key is gone. `secure` = the phone has a screen lock now.
     */
    fun pass(secure: Boolean): String {
        val stored = prefs.getString("vault", null)
        if (stored == null) {
            // first use: a new password, saved (wrapped) before anything is sealed with it
            val alias = if (secure) GUARDED else OPEN
            val pass = ByteArray(32).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
            store(wrap(make(alias, secure), pass), alias)
            return pass
        }
        val alias = prefs.getString("vault-key", OPEN) ?: OPEN
        val key = ks.getKey(alias, null) as? SecretKey ?: throw KeyPermanentlyInvalidatedException("the keystore key is gone")
        val pass = unwrap(key, stored)
        // made when the phone had no screen lock, and it has one now: move the password under a key that needs the lock
        if (alias == OPEN && secure) { store(wrap(make(GUARDED, true), pass), GUARDED); ks.deleteEntry(OPEN) }
        return pass
    }
    /** After the keystore key is gone: start again with a new password. What the old one sealed stays closed. */
    fun forget() { prefs.edit().remove("vault").remove("vault-key").commit(); ks.deleteEntry(GUARDED); ks.deleteEntry(OPEN) }
    companion object {
        const val GUARDED = "tachys-seed-guarded"; const val OPEN = "tachys-seed-open"
        /** Seconds the keystore key stays usable after the screen lock is given: long enough to use it once, on a slow phone. */
        const val WINDOW = 15
    }
}

class MainActivity : ComponentActivity() {
    private lateinit var core: Core
    private lateinit var vault: Vault
    private val locked = mutableStateOf(false)
    private var asking = false
    private var after: ((Boolean) -> Unit)? = null
    private val confirm = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { r -> asking = false; after?.invoke(r.resultCode == RESULT_OK); after = null }
    private fun secure() = (getSystemService(KEYGUARD_SERVICE) as KeyguardManager).isDeviceSecure
    /** Asks for the phone's own screen lock (PIN, pattern, password, or fingerprint where the phone offers it). */
    @Suppress("DEPRECATION")
    private fun ask(title: String, done: (Boolean) -> Unit) {
        val i = if (secure()) (getSystemService(KEYGUARD_SERVICE) as KeyguardManager).createConfirmDeviceCredentialIntent(title, "tachys") else null
        if (i == null) { done(true); return } // a phone with no screen lock has nothing to ask
        after = done; asking = true; confirm.launch(i)
    }
    /**
     * The password that opens the seed phrases, for one use. The keystore gives it up only within a few seconds of the
     * screen lock being given, so this asks for the lock: every time if `always`, otherwise only when the keystore wants it.
     */
    private fun open(title: String, always: Boolean, done: (String?, String) -> Unit) {
        val declined = "The phone's screen lock was not given, so the seed phrase stays closed."
        fun take(mayAsk: Boolean) {
            try { done(vault.pass(secure()), "") }
            catch (e: UserNotAuthenticatedException) {
                if (mayAsk) ask(title) { ok -> if (ok) take(false) else done(null, declined) }
                else done(null, "The phone's keystore did not accept that unlock. Try again with the PIN, pattern or password.")
            }
            catch (e: KeyPermanentlyInvalidatedException) { vault.forget(); done(null, Core.LOST) }
            catch (e: Exception) { done(null, "The phone's keystore failed: ${e.message ?: e.javaClass.simpleName}") }
        }
        if (always && secure()) ask(title) { ok -> if (ok) take(false) else done(null, declined) } else take(true)
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        core = Core(Phone(this))
        vault = Vault(this)
        // Test hooks (adb: --ez testnet true, --es watch ADDRESS --es name NAME), in a debug build only. This activity is
        // exported, so in a release build any other app could otherwise switch the network or plant the first wallet.
        if ((applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0) {
            if (intent.hasExtra("testnet")) core.testnet = intent.getBooleanExtra("testnet", false)
            intent.getStringExtra("watch")?.let { if (!core.hasWallet()) core.watch(it, intent.getStringExtra("name") ?: "Wallet") }
        }
        // a signing request that opened the app (an ergopay: link)
        val request = intent.data?.takeIf { it.scheme == "ergopay" }?.toString() ?: ""
        locked.value = core.appLock && secure()
        setContent { App(core, request, Guard(::secure, ::ask, ::open), locked.value) { ask("Unlock tachys") { if (it) locked.value = false } } }
    }
    override fun onStop() {
        super.onStop()
        // leaving the app locks it; our own screen-lock question is not leaving
        if (core.appLock && !asking && secure()) locked.value = true
    }
}

