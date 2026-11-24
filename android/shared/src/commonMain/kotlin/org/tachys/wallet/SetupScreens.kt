package org.tachys.wallet

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// Making and keeping wallets: a new, restored or read-only wallet (AddWallet), one wallet's own settings (Config),
// and the app's (Settings).

/** The "+" page: a new wallet, a wallet restored from its seed phrase, or a read-only wallet. */
@Composable
fun AddWallet(core: Core, guard: Guard, onDone: () -> Unit) {
    var mode by remember { mutableStateOf("") }
    var name by remember { mutableStateOf("") }
    var address by remember { mutableStateOf("") }
    var phrase by remember { mutableStateOf("") }
    var words by remember { mutableStateOf("") }
    var msg by remember { mutableStateOf("") }
    val lock = LocalPlatform.current.lock // where there is no screen lock to guard a phrase, a wallet always has a password of its own
    var own by remember { mutableStateOf(!lock) } // sealed with a password typed here, not behind the screen lock
    var pass1 by remember { mutableStateOf("") }
    var pass2 by remember { mutableStateOf("") }
    var scanning by remember { mutableStateOf(false) }
    if (scanning) { Scanner("Scan an address to watch", { address = addressFromCode(it); scanning = false }, { scanning = false }); return }
    @Composable fun NameField() { Field(name, "wallet name (optional)", words = true) { name = it.take(40) }; Spacer(Modifier.height(8.dp)) }
    // back from a form to the three choices, by the line at its top or the phone's back button. A phrase half typed is
    // dropped on the way out. Not from the words to write down: those are seen through to the end.
    val leave = { mode = ""; msg = ""; phrase = ""; address = ""; own = !lock; pass1 = ""; pass2 = "" }
    // what guards the seed phrase: the phone's screen lock, or a password for this wallet alone
    @Composable fun GuardChoice() {
        if (lock) {
            Text("Guarded by", color = Dim, fontSize = 13.sp)
            Spacer(Modifier.height(4.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Pill("Screen lock", if (!own) Color.White else Bg, if (!own) Color.Black else Color.White, Modifier.weight(1f)) { own = false; msg = "" }
                Pill("Password", if (own) Color.White else Bg, if (own) Color.Black else Color.White, Modifier.weight(1f)) { own = true; msg = "" }
            }
            Spacer(Modifier.height(8.dp))
        }
        if (!own) SealNote(guard) else {
            Field(pass1, "password for this wallet", secret = true) { pass1 = it }
            Spacer(Modifier.height(8.dp))
            Field(pass2, "the same password again", secret = true) { pass2 = it }
            Spacer(Modifier.height(6.dp))
            Text("The seed phrase is kept encrypted with this password, and whoever has the password and this $device can spend from the wallet and read its words: the $device's screen lock is not asked. A short or guessable password can be found by anyone who copies the wallet's file. If it is forgotten, the written words are the only way back.", color = Dim, fontSize = 13.sp)
        }
    }
    // the password to seal with: the one typed, or the Vault's once the screen lock allows
    fun sealWith(title: String, then: (String) -> Unit) {
        if (!own) guard.open(title, false) { pass, why -> if (pass == null) msg = why else then(pass) }
        else passwordFault(pass1, pass2).let { fault -> if (fault != null) msg = fault else then(pass1.trim()) }
    }
    BackHandler(enabled = mode == "new" || mode == "restore" || mode == "watch") { leave() }
    // the words to write down: the back button does nothing here; the only way on is "I have written them down"
    BackHandler(enabled = mode == "words") { }
    @Composable fun BackLine() { Text("‹  Back", color = Accent, fontSize = 16.sp, modifier = Modifier.fillMaxWidth().clickable { leave() }.padding(bottom = 10.dp)) }
    when (mode) {
        "" -> {
            CardBox({ mode = "new"; msg = "" }) {
                Text("New wallet", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
                Text("Makes a new key on this $device and shows you its seed phrase to write down.", color = Dim, fontSize = 14.sp)
            }
            CardBox({ mode = "restore"; msg = "" }) {
                Text("Restore wallet", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
                Text("Enter a seed phrase you already have. If it belongs to a read-only wallet on this $device, that wallet becomes a full one.", color = Dim, fontSize = 14.sp)
            }
            CardBox({ mode = "watch"; msg = "" }) {
                Text("Read-only wallet", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
                Text("Watch an address. No key is on this $device, so nothing can be spent from here.", color = Dim, fontSize = 14.sp)
            }
        }
        "new" -> CardBox {
            BackLine()
            Text("New wallet", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(8.dp))
            NameField()
            GuardChoice()
            Spacer(Modifier.height(10.dp))
            // why it did not happen is said here, by the button: under the card it would be off the screen
            if (msg.isNotEmpty()) { Text(msg, color = Accent, fontSize = 15.sp); Spacer(Modifier.height(10.dp)) }
            Pill("Create", Accent, Color.Black, Modifier.fillMaxWidth()) {
                sealWith("Create a wallet") { pass ->
                    val r = core.create(name, pass, own)
                    val shown = if (r == Core.OK) core.mnemonic(pass) else "! $r"
                    if (!shown.startsWith("! ")) { words = shown; msg = ""; pass1 = ""; pass2 = ""; mode = "words" } else msg = shown.drop(2)
                }
            }
        }
        "words" -> CardBox {
            Text("Write these words down", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
            Text("In this order, on paper. They are the wallet: anyone who has them can spend from it, and without them a lost $device means lost coins.", color = Dim, fontSize = 14.sp)
            Spacer(Modifier.height(10.dp))
            SecretWords(words)
            Spacer(Modifier.height(12.dp))
            Pill("I have written them down", Accent, Color.Black, Modifier.fillMaxWidth()) { words = ""; onDone() }
        }
        "restore" -> CardBox {
            Secure() // a phrase being typed is as secret as one being shown
            BackLine()
            Text("Restore wallet", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(8.dp))
            NameField()
            Field(phrase, "seed phrase, words separated by spaces") { phrase = it }
            Text("Only the wallet's current coins are looked up, not its history.", color = Dim, fontSize = 13.sp)
            Spacer(Modifier.height(8.dp))
            GuardChoice()
            Spacer(Modifier.height(10.dp))
            // why it did not happen is said here, by the button: under the card it would be off the screen
            if (msg.isNotEmpty()) { Text(msg, color = Accent, fontSize = 15.sp); Spacer(Modifier.height(10.dp)) }
            Pill("Restore", Accent, Color.Black, Modifier.fillMaxWidth()) {
                sealWith("Restore a wallet") { pass ->
                    val r = core.restore(phrase, name, pass, own)
                    if (r == Core.OK) { phrase = ""; pass1 = ""; pass2 = ""; onDone() } else msg = r
                }
            }
        }
        "watch" -> CardBox {
            BackLine()
            Text("Read-only wallet", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(8.dp))
            NameField()
            Field(address, "address", onScan = { scanning = true }) { address = it.filter { c -> c.isLetterOrDigit() } }
            Spacer(Modifier.height(10.dp))
            Pill("Add", Color.White, Color.Black, Modifier.fillMaxWidth()) { val r = core.watch(address, name); if (r == Core.OK) onDone() else msg = r }
        }
    }
    if (msg.isNotEmpty() && mode != "new" && mode != "restore") CardBox { Text(msg, color = Color.White, fontSize = 15.sp) }
}

/** One wallet's own settings (the gear on its card). */
@Composable
fun Config(core: Core, w: WalletView, guard: Guard, onChanged: () -> Unit, onRemoved: () -> Unit) {
    var name by remember { mutableStateOf(w.name) }
    var pub by remember { mutableStateOf("") }
    var words by remember { mutableStateOf("") }
    var msg by remember { mutableStateOf("") }
    var removing by remember { mutableStateOf(false) }
    // a new address and a new name are written to the wallet file, which one command at a time writes: each waits its
    // turn behind a sync of this wallet, off the thread that draws
    val scope = rememberCoroutineScope()
    CardBox {
        Text(if (w.addresses.size > 1) "Wallet addresses" else "Wallet address", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        w.addresses.forEachIndexed { i, a ->
            Spacer(Modifier.height(6.dp))
            if (w.addresses.size > 1) Text(if (i == 0) "Main address" else "Address ${i + 1}", color = Dim, fontSize = 13.sp)
            SelectionContainer { Text(a, color = Color.White, fontSize = 14.sp) }
        }
        if (!w.readOnly) {
            Spacer(Modifier.height(10.dp))
            Pill("Add an address", Bg, Color.White, Modifier.fillMaxWidth()) { guard.open("Add an address to ${w.name}", false) { pass, why -> if (pass == null) msg = why else scope.launch { val r = withContext(background) { core.addAddress(pass) }; msg = if (r.startsWith("address ")) "" else r.lines().lastOrNull { it.isNotBlank() } ?: "failed"; onChanged() } } }
            Text("Every address belongs to the same seed phrase. The balance counts them all; change returns to the main address.", color = Dim, fontSize = 12.sp)
        }
    }
    CardBox {
        Text("Descriptive wallet name", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(8.dp))
        Field(name, "wallet name", words = true) { name = it.take(40) }
        Spacer(Modifier.height(10.dp))
        Pill("Save name", Bg, Color.White, Modifier.fillMaxWidth()) { scope.launch { withContext(background) { core.rename(name) }; onChanged() } }
    }
    if (!w.readOnly) CardBox {
        Pill(if (pub.isEmpty()) "Display public key" else "Hide public key", Bg, Color.White, Modifier.fillMaxWidth()) {
            // made from the seed phrase, so it needs the phrase opened
            if (pub.isNotEmpty()) pub = "" else guard.open("Show the public key of ${w.name}", false) { pass, why -> if (pass == null) msg = why else core.pubkey(pass).let { r -> if (r.startsWith("! ")) msg = r.drop(2) else { pub = r; msg = "" } } }
        }
        if (pub.isNotEmpty()) {
            Text("Lets another wallet see every address of this one without being able to spend. Share it only with someone who may see your balance.", color = Dim, fontSize = 13.sp)
            Spacer(Modifier.height(8.dp)); Qr(remember(pub) { core.qr(pub) }); Spacer(Modifier.height(8.dp))
            SelectionContainer { Text(pub, color = Color.White, fontSize = 13.sp) }
        }
    }
    if (!w.readOnly) CardBox {
        Pill(if (words.isEmpty()) "Display mnemonic" else "Hide mnemonic", Bg, Color.White, Modifier.fillMaxWidth()) {
            if (words.isNotEmpty()) words = "" else guard.open("Show the seed phrase of ${w.name}", true) { pass, why -> if (pass == null) msg = why else core.mnemonic(pass).let { r -> if (!r.startsWith("! ")) { words = r; msg = "" } else msg = r.drop(2) } }
        }
        if (words.isNotEmpty()) {
            Text(if (core.own()) "Anyone who sees these words can spend from this wallet. On this $device they are kept encrypted and open with this wallet's own password." else if (guard.secure()) "Anyone who sees these words can spend from this wallet. On this $device they are kept encrypted and open only with its screen lock." else "This $device has no screen lock, so nothing guards these words. Anyone who sees them can spend from this wallet.", color = Dim, fontSize = 13.sp)
            Spacer(Modifier.height(8.dp)); SecretWords(words)
        }
    }
    CardBox {
        if (!removing) Pill("Remove wallet", Bg, Color.White, Modifier.fillMaxWidth()) { removing = true }
        else {
            Text(if (w.readOnly) "Remove this read-only wallet from the $device?" else "Remove this wallet and its key from the $device? Without the seed phrase written down, its coins are lost for good.", color = Color.White, fontSize = 15.sp)
            Spacer(Modifier.height(10.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Pill("Keep", Color.White, Color.Black, Modifier.weight(1f)) { removing = false }
                Pill("Remove", Accent, Color.Black, Modifier.weight(1f)) { if (w.readOnly) { core.remove(); onRemoved() } else guard.ask("Remove ${w.name}") { ok -> if (ok) { core.remove(); onRemoved() } else if (core.own()) { removing = false; msg = "The wallet's password was not given, or is not the right one, so the wallet stays." } } }
            }
        }
    }
    // said at the foot of the page, which is brought into view: the button that caused it may be a screen above
    val foot = remember { BringIntoViewRequester() }
    if (msg.isNotEmpty()) Box(Modifier.bringIntoViewRequester(foot)) {
        CardBox { Text(msg, color = Color.White, fontSize = 15.sp) }
        LaunchedEffect(msg) { foot.bringIntoView() }
    }
}

@Composable
fun Settings(core: Core, guard: Guard, onChanged: () -> Unit) {
    var lock by remember { mutableStateOf(core.appLock) }
    var url by remember { mutableStateOf(core.node) }
    var msg by remember { mutableStateOf("") }
    CardBox {
        Text("Network", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("Each network has its own wallets on this $device.", color = Dim, fontSize = 13.sp)
        Spacer(Modifier.height(10.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Pill("Mainnet", if (!core.testnet) Accent else Bg, if (!core.testnet) Color.Black else Color.White, Modifier.weight(1f)) { if (core.testnet) { core.testnet = false; onChanged() } }
            Pill("Testnet", if (core.testnet) Accent else Bg, if (core.testnet) Color.Black else Color.White, Modifier.weight(1f)) { if (!core.testnet) { core.testnet = true; onChanged() } }
        }
    }
    if (LocalPlatform.current.lock) CardBox {
        Text("App lock", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("Ask for this $device's screen lock (PIN, pattern, password, or fingerprint where the $device offers it) each time the app is opened.", color = Dim, fontSize = 13.sp)
        Spacer(Modifier.height(10.dp))
        Pill(if (lock) "On · tap to turn off" else "Off · tap to turn on", if (lock) Accent else Bg, if (lock) Color.Black else Color.White, Modifier.fillMaxWidth()) {
            if (!guard.secure()) msg = "Set a screen lock on the $device first; the app lock uses it."
            else guard.ask(if (lock) "Turn off the app lock" else "Turn on the app lock") { ok -> if (ok) { lock = !lock; core.appLock = lock; msg = "" } }
        }
    }
    CardBox {
        Text("Node connection", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("The node asked for box proofs, sending and token names on this network. What it says about your coins is checked against the chain tip the peers agree on; it is trusted only for token names. This build uses no separate explorer. With \"auto\" the wallet finds nodes itself: well-known ones first, others that their peers advertise only if those are too few, moving on to the next when one does not answer. Every node it uses sees the addresses it looks up.", color = Dim, fontSize = 13.sp)
        Spacer(Modifier.height(8.dp))
        Field(url, "auto, or http://host:port") { url = it.trim() }
        Text("To use one node only, your own at home say, write its address as http://HOST:PORT, for example http://192.168.1.20:9053 (the node's REST port; https:// if it has a certificate). The wallet then asks that node and no other. The node needs the UTXO set and the extra index (ergo.node.extraIndex = true in its configuration), and this $device has to be able to reach it.", color = Dim, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
        Spacer(Modifier.height(10.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Pill("Save", Color.White, Color.Black, Modifier.weight(1f)) { if (url.startsWith("http://") || url.startsWith("https://") || url == "auto") { core.node = url; url = core.node; msg = "Saved. Used from the next sync." } else msg = "A node address starts with http:// or https:// (or write auto)" }
            Pill("Default", Bg, Color.White, Modifier.weight(1f)) { core.node = ""; url = core.node; msg = "Back to the default: nodes are found automatically." }
        }
    }
    if (msg.isNotEmpty()) CardBox { Text(msg, color = Color.White, fontSize = 15.sp) }
    CardBox {
        Text("About", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("tachys${core.version.let { if (it.isEmpty()) "" else " $it" }}, a research build. The chain tip comes from peers and is checked for work; balances are proven, not reported. Box proofs, sending and token names use one node's REST port in this build. A token name marked \"as reported\" is what that node said, not something proven.", color = Dim, fontSize = 13.sp)
    }
}
