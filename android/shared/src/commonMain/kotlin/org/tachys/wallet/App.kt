package org.tachys.wallet

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.layout.*
import kotlinx.coroutines.delay
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// The tachys app's screens, for a phone and a computer alike. They follow the Ergo mobile wallet; all wallet logic
// is in the Rust core, which is a library loaded into this process and called one command at a time (`runCore`).

val Bg = Color(0xFF303030)

val Card = Color(0xFF424242)

val Bar = Color(0xFF121212)

val Accent = Color(0xFFFF4136)

val Dim = Color(0xFF9E9E9E)

enum class Screen { List, Detail, Receive, Send, Pending, Boxes, Settings, Request, Add, Config, Scan }

@Composable
fun App(core: Core, opened: String, guard: Guard, locked: Boolean, unlock: () -> Unit) {
    device = core.on.device
    CompositionLocalProvider(LocalPlatform provides core.on) { Screens(core, opened, guard, locked, unlock) }
}

@Composable
private fun Screens(core: Core, opened: String, guard: Guard, locked: Boolean, unlock: () -> Unit) {
    var wallets by remember { mutableStateOf(core.views()) }
    var sel by remember { mutableStateOf(core.sel) }
    var status by remember { mutableStateOf(if (core.hasWallet()) "not synced yet" else "no wallet") }
    var screen by remember { mutableStateOf(if (opened.isNotEmpty()) Screen.Request else Screen.List) }
    var requestCode by remember { mutableStateOf(opened) }
    var payTo by remember { mutableStateOf(PayRequest("")) } // what a scanned code asked to be paid; fills the Send screen
    var busy by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    fun refresh() { wallets = core.views(); sel = core.sel }
    fun pick(dir: String) { core.sel = dir; sel = core.sel }
    fun io(label: String, work: () -> String, done: (String) -> Unit = {}) {
        // taken before the work is queued, not inside it: two taps in a row must not both get through
        if (busy) return
        busy = true
        scope.launch { status = label; val r = withContext(background) { work() }; refresh(); busy = false; done(r) }
    }
    fun sync() = io("syncing…", { core.sync() }) { status = if (it == "ok") "synced just now" else it }
    LaunchedEffect(Unit) { sync() }
    // seed phrases kept in plain text by an earlier build are sealed as soon as the screen lock allows
    var plain by remember { mutableStateOf(core.plain().size) }
    fun sealOld() = guard.open("Encrypt the seed phrases on this $device", false) { pass, why ->
        // sealing writes the wallet files a sync also writes: it waits for the work under way and holds the screens' one
        // turn while it runs, as a sync or a payment does, and is not started beside them
        if (pass == null) status = why else scope.launch {
            while (busy) delay(200)
            busy = true
            try { plain = withContext(background) { core.sealAll(pass) } } finally { busy = false }
            refresh()
        }
    }
    LaunchedEffect(locked) { if (!locked && plain > 0) sealOld() }
    // a wallet sealed with a password of its own is opened by asking for that password, here, and never by the screen lock
    var asking by remember { mutableStateOf<Pair<String, (String?, String) -> Unit>?>(null) }
    val keyed = remember(guard) { Guard(guard.secure,
        { title, done -> if (core.own()) asking = title to { pass, _ -> done(pass != null && core.opens(pass)) } else guard.ask(title, done) },
        { title, always, done -> if (core.own()) asking = title to done else guard.open(title, always, done) }) }
    asking?.let { (title, done) -> if (!locked) PasswordPrompt(title) { pass -> asking = null; if (pass == null) done(null, "The wallet's password was not given, so the seed phrase stays closed.") else done(pass, "") } }
    BackHandler(enabled = screen != Screen.List && !locked) { screen = if (screen == Screen.Pending || screen == Screen.Boxes) Screen.Detail else Screen.List }

    if (locked) {
        Column(Modifier.fillMaxSize().background(Bar).systemBarsPadding().background(Bg), verticalArrangement = Arrangement.Center) {
            CardBox {
                Text("tachys is locked", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
                Text("Unlock it with this $device's screen lock.", color = Dim, fontSize = 14.sp)
                Spacer(Modifier.height(12.dp))
                Pill("Unlock", Accent, Color.Black, Modifier.fillMaxWidth(), onClick = unlock)
            }
        }
        return
    }
    // focusable itself, so that the focus a field gives up rests here and not on the first field of the form, which would bring the keyboard back
    Column(Modifier.fillMaxSize().background(Bar).systemBarsPadding().imePadding().background(Bg).focusable()) {
        val w = wallets.firstOrNull { it.dir == sel }
        val net = if (core.testnet) " · testnet" else ""
        when (screen) {
            Screen.List -> Header("tachys$net", "$status · tap to sync", "+", { screen = Screen.Add }) { sync() }
            Screen.Settings -> Header("‹  Settings", "tap to go back") { screen = Screen.List }
            Screen.Add -> Header("‹  Add a wallet", "tap to go back") { screen = Screen.List }
            Screen.Pending -> Header("‹  Pending", "tap to go back") { screen = Screen.Detail }
            Screen.Boxes -> Header("‹  Boxes", "tap to go back") { screen = Screen.Detail }
            Screen.Scan -> Header("‹  Scan", "tap to go back") { screen = Screen.List }
            Screen.Config -> Header("‹  " + (w?.name ?: "Wallet") + " · settings", "tap to go back") { screen = Screen.List }
            else -> Header("‹  " + (w?.name ?: "Wallet"), "tap to go back") { screen = Screen.List }
        }
        Column(Modifier.weight(1f).verticalScroll(key(screen) { rememberScrollState() })) {
            Spacer(Modifier.height(8.dp))
            if (screen == Screen.List) remember(wallets) { core.unreadable() }.forEach { CardBox { Text(it, color = Accent, fontSize = 14.sp) } }
            when {
                screen == Screen.Settings -> Settings(core, guard) { refresh(); status = if (core.hasWallet()) "not synced yet" else "no wallet"; screen = Screen.List; sync() }
                // scan first, choose the wallet after: a signing request goes to Sign, an address or payment request to Send
                screen == Screen.Scan && w != null -> Scanner("Scan an address, a payment request or a request to sign", { code ->
                    if (code.trim().startsWith("ergopay")) { requestCode = code.trim(); screen = Screen.Request }
                    else payRequestFromCode(code).let { r -> if (r.address.isEmpty()) { status = "that code holds no address"; screen = Screen.List } else { payTo = r; screen = Screen.Send } }
                }, { screen = Screen.List })
                screen == Screen.Add || w == null -> AddWallet(core, guard) { refresh(); status = "not synced yet"; screen = Screen.List; sync() }
                screen == Screen.Config -> key(w.dir) { Config(core, w, keyed, { refresh() }) { refresh(); screen = Screen.List } }
                screen == Screen.Pending -> Pending(core, w)
                screen == Screen.Boxes -> Boxes(core, w, busy) { show -> keyed.open("Consolidate ${w.name}", true) { pass, why -> if (pass == null) show(why) else io("consolidating…", { core.consolidate(pass) }) { r -> status = "synced just now"; show(r) } } }
                screen == Screen.Detail -> Detail(core, w, { screen = Screen.Boxes }) { screen = Screen.Pending }
                screen == Screen.Receive -> Receive(core, w, busy) { ids, show -> io("checking receipt…", { core.receive(ids) }) { r -> status = "synced just now"; show(r) } }
                screen == Screen.Request -> {
                    // with several wallets, say which one is being asked to sign; tap to step through them
                    if (wallets.size > 1) CardBox({ pick(wallets[(wallets.indexOfFirst { it.dir == sel } + 1) % wallets.size].dir) }) {
                        Text("Signing with", color = Dim, fontSize = 13.sp)
                        Text(w.name + (if (w.readOnly) "  (read-only)" else "") + "  ▾", color = Color.White, fontSize = 18.sp, fontWeight = FontWeight.Bold)
                    }
                    key(w.dir) { Request(core, requestCode, busy, { code -> requestCode = code }) { code, sign, show ->
                        if (!sign) io("reading request…", { core.request(code, null) }) { r -> status = "synced just now"; show(r) }
                        else keyed.open("Sign with ${w.name}", true) { pass, why -> if (pass == null) show("request refused: $why") else io("signing…", { core.request(code, pass) }) { r -> status = "synced just now"; show(r) } }
                    } }
                }
                screen == Screen.Send -> {
                    // with several wallets, say which one pays; tap to step through them
                    if (wallets.size > 1) CardBox({ pick(wallets[(wallets.indexOfFirst { it.dir == sel } + 1) % wallets.size].dir) }) {
                        Text("Paying from", color = Dim, fontSize = 13.sp)
                        Text(w.name + (if (w.readOnly) "  (read-only)" else "") + "  ▾", color = Color.White, fontSize = 18.sp, fontWeight = FontWeight.Bold)
                    }
                    Send(core, w, busy, payTo) { to, nano, tokens, feeToken, feeMax, show ->
                    keyed.open("Send from ${w.name}", true) { pass, why -> if (pass == null) show(why) else io("sending…", { core.send(to, nano, tokens, feeToken, feeMax, pass) }) { r -> status = "synced just now"; show(r) } }
                    }
                }
                else -> {
                    if (plain > 0) CardBox({ sealOld() }) {
                        Text(if (plain == 1) "One wallet's seed phrase is not encrypted yet" else "$plain wallets' seed phrases are not encrypted yet", color = Accent, fontSize = 18.sp, fontWeight = FontWeight.Bold)
                        Text("Tap to encrypt, so that using or showing a seed phrase needs this $device's screen lock.", color = Dim, fontSize = 14.sp)
                    }
                    wallets.forEach { x ->
                    WalletCard(x, onOpen = { pick(x.dir); screen = Screen.Detail }, onReceive = { pick(x.dir); screen = Screen.Receive }, onSend = { pick(x.dir); payTo = PayRequest(""); screen = Screen.Send }, onConfig = { pick(x.dir); screen = Screen.Config })
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth().background(Accent).padding(vertical = 14.dp), horizontalArrangement = Arrangement.SpaceEvenly) {
            Text("Wallets", color = Color.Black, fontSize = 14.sp, modifier = Modifier.clickable { screen = Screen.List })
            if (wallets.isNotEmpty() && core.on.camera) Text("Scan", color = Color.Black, fontSize = 14.sp, modifier = Modifier.clickable { screen = Screen.Scan })
            Text("Settings", color = Color.Black, fontSize = 14.sp, modifier = Modifier.clickable { screen = Screen.Settings })
        }
    }
}

/** What this machine is called in the sentences the screens show ("phone", "computer"): the platform's word, set when the
 *  screens start. Only the screens read it; `Core`, which can be asked before any screen is drawn, reads `on.device`. */
var device = "phone"
