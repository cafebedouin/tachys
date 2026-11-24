package org.tachys.wallet

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import kotlinx.coroutines.delay
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// tachys phone app (spike). Screens follow the Ergo mobile wallet; all wallet logic is in the Rust
// core, which for now is the command-line binary run as a child process.

val Bg = Color(0xFF303030)
val Card = Color(0xFF424242)
val Bar = Color(0xFF121212)
val Accent = Color(0xFFFF4136)
val Dim = Color(0xFF9E9E9E)

/** A token the wallet holds. `reported` = the name came from a node's answer, not from the list shipped with the app. */
class TokenLine(val id: String, val name: String, val decimals: Int, val raw: Long, val reported: Boolean) {
    val amount: String get() = shown(raw, decimals)
}
/** One box a wallet tracks: its id, nanoERG, how many kinds of token, and the blocks it was last proven and first confirmed at. */
class BoxLine(val id: String, val nano: Long, val tokens: Int, val provenAt: Int, val confirmedAt: Int)
/** The layout of `wallet.json` this app reads (the core's `walletfile.rs`). */
const val WALLET_FILE_VERSION = 1
class WalletView(val name: String, val address: String, val erg: String, val boxes: Int, val tokens: List<TokenLine>,
                 val provenAt: Int, val readOnly: Boolean, val expecting: List<String>, val dir: String = "", val addresses: List<String> = listOf(address),
                 /** What became of payments sent from this wallet, by transaction id: "waiting", "confirmed", "spent elsewhere", "coins spent" or "not included", and the height it was settled at. */
                 val sent: Map<String, Pair<String, Int>> = mapOf(),
                 /** The balance exactly, in nanoERG (`erg` is rounded down for show). */
                 val nano: Long = 0L,
                 /** nanoERG in boxes proven only in a block one node has shown: tracked, shown apart, not in the balance and not spent. */
                 val seen: Long = 0L)

/**
 * The phone's own screen lock, used to guard the seed phrases and the app.
 * `open` hands over the password that opens the seed phrases, for one use: (what to tell the user, whether to ask for the
 * screen lock even if it was given a moment ago, then the password or null with the reason).
 * A wallet may instead be sealed with a password of its own, typed when it was made (`Core.own`): the screens of such
 * a wallet are given a Guard whose `open` asks for that password and whose `ask` does not ask for the screen lock.
 */
class Guard(val secure: () -> Boolean, val ask: (String, (Boolean) -> Unit) -> Unit, val open: (String, Boolean, (String?, String) -> Unit) -> Unit)

/** A receipt as shown in a QR code: the box id, and the purpose the payer wrote, if any. */
class Receipt(val box: String, val purpose: String, val tx: String = "") {
    /** With `tx` (hex of a signed transaction) the code carries the payment itself, so the payee can submit it. */
    fun code(): String = when {
        tx.isNotEmpty() -> "tachys:tx?d=" + base64Url(tx.chunked(2).map { it.toInt(16).toByte() }.toByteArray()) + "&box=$box" + if (purpose.isEmpty()) "" else "&p=" + percentEncoded(purpose)
        purpose.isEmpty() -> box
        else -> "tachys:receipt?box=$box&p=" + percentEncoded(purpose)
    }
    companion object {
        fun parse(code: String): Receipt {
            val c = code.trim()
            if (c.startsWith("tachys:tx?")) {
                val q = queryOf(c)
                val bytes = base64UrlBytes(q["d"] ?: "") ?: ByteArray(0)
                return Receipt(boxIdFromCode(q["box"] ?: ""), (q["p"] ?: "").take(80), bytes.joinToString("") { hex2(it) })
            }
            if (c.startsWith("tachys:receipt?")) {
                val q = queryOf(c)
                return Receipt(boxIdFromCode(q["box"] ?: ""), (q["p"] ?: "").take(80))
            }
            return Receipt(boxIdFromCode(c), "")
        }
    }
}

class Core(val on: Platform) {
    private val prefs = on.prefs
    var testnet: Boolean
        get() = prefs.getBoolean("testnet", false)
        set(v) { prefs.put("testnet", v) }
    private val net get() = if (testnet) "testnet" else "mainnet"
    private fun path(name: String) = FileAt(on.files, name).absolutePath
    /** The wallets on this phone for this network: one folder each. */
    fun dirs(): List<String> = (on.files.listFiles() ?: listOf()).filter { (it.name == "st-$net" || it.name.startsWith("st-$net-")) && FileAt(it, "wallet.json").exists() }.map { it.name }.sorted()
    /** The wallet the screens are about. */
    var sel: String = ""
        get() = dirs().let { d -> field.takeIf { it in d } ?: d.firstOrNull() ?: "st-$net" }
    val state get() = path(sel)
    // the agreed chain tip is fetched once and shared by every wallet
    private val chain get() = path("chain-$net")
    private fun seedChain(dir: String) { FileAt(chain).listFiles()?.forEach { f -> if (f.isFile) f.copyTo(FileAt(path(dir), f.name)) } }
    private fun fresh() = "st-$net-${now()}"
    // REST nodes for box proofs, broadcast and token names. "auto": the core finds usable ones (well-known nodes and,
    // if those are too few, nodes their peers advertise) and falls back from one to the next; the user may name one instead
    val defaultNode get() = "auto"
    var node: String
        get() = prefs.getString("node-$net", null)?.takeIf { it.isNotBlank() } ?: defaultNode
        set(v) { prefs.put("node-$net", v.trim().trimEnd('/')) }
    var appLock: Boolean
        get() = prefs.getBoolean("applock", false)
        set(v) { prefs.put("applock", v) }
    private fun payKey(dir: String = sel) = if (dir == "st-$net") "payments-$net" else "payments-$dir"
    private fun seeds(): String = FileAt(on.files, "seeds-$net.txt").also { f ->
        // the list the app ships, written out again whenever it differs: an updated app must not keep the first install's peers for ever
        val shipped = on.asset("seeds-$net.txt")
        if (!f.exists() || !f.readBytes().contentEquals(shipped)) f.writeBytes(shipped)
    }.absolutePath
    // names shipped with the app: trusted. The list is every token whose deepest ErgoDEX pool held 100 ERG or more when it was made (docs/maintaining.md)
    private val shipped: Map<String, Pair<String, Int>> = JSONArray(on.asset("tokens.json").decodeToString()).let { a ->
        (0 until a.length()).associate { a.getJSONObject(it).let { t -> t.getString("id") to (t.getString("name") to t.getInt("decimals")) } }
    }
    // names a node reported once and the core stored: shown with a mark
    private fun reported(dir: String = sel): Map<String, Pair<String, Int>> = try {
        JSONObject(FileAt(path(dir), "tokens.json").readText()).let { o -> o.keys().asSequence().associateWith { k -> o.getJSONObject(k).let { it.optString("name") to it.optInt("decimals") } } }
    } catch (e: Exception) { mapOf() }

    fun run(vararg args: String): String = exec(listOf(), *args)
    /** Runs the core with lines for it to read, so that a password or a seed phrase is never an argument or a file. */
    private fun exec(input: List<String>, vararg args: String): String = execCode(input, *args).second
    /**
     * Asks the core with `--json`: the code it ends with, the one JSON object it writes as its output (null if there is
     * none), and its sentences, which it then writes apart from it. What the app decides on (a tip agreed, boxes proven,
     * a payment taken by a node, a receipt accepted) is read from the object and the code; the sentences are for showing.
     */
    private fun ask(input: List<String>, vararg args: String): Reply = core(input, args.toList() + listOf("--json", "--network", net)).let { reply(it.code, it.out, it.err) }
    /** What the core printed and the code it ended with (-1 if it could not be run at all). */
    private fun execCode(input: List<String>, vararg args: String): Pair<Int, String> = core(input, args.toList() + listOf("--network", net)).let { it.code to it.out + it.err }
    /**
     * One command of the core, run in this process (`Native`). A panic inside the core comes back as its code 70 and the
     * app goes on. If the library is not in this build, or will not load, the answer is -1 and why, as it was when the
     * core was a program that would not start.
     */
    private fun core(input: List<String>, args: List<String>): Ran = runCore(args, input)

    /** Runs a command that needs the seed phrase: `pass` (from the Vault) opens it, and is the first line of the core's input. */
    private fun sealedAsk(pass: String, vararg args: String): Reply = ask(listOf(pass), *args, "--pass-stdin")
    /** Runs a command that needs the seed phrase and answers with its sentences, for showing; whether it worked is in the core's JSON answer. */
    private fun sealed(pass: String, vararg args: String): String = sealedAsk(pass, *args).let { a -> if (a.ok) a.text else failed(a) }
    /** Wallets, on either network, whose seed phrase is still in plain text from before phrases were sealed. */
    fun plain(): List<String> = (on.files.listFiles() ?: listOf()).filter { it.name.startsWith("st-") && try { JSONObject(FileAt(it, "wallet.json").readText()).has("mnemonic") } catch (e: Exception) { false } }.map { it.name }
    /** Seals every plain-text seed phrase. Returns how many are still in plain text. */
    fun sealAll(pass: String): Int {
        plain().forEach { sealed(pass, "wallet", "encrypt", "--state", path(it), "--network", if (it.startsWith("st-testnet")) "testnet" else "mainnet") }
        return plain().size
    }
    // words of letters only, in any of the languages the core takes (Japanese phrases are spaced with U+3000); the core's refusals all carry punctuation
    /**
     * Whether a wallet's seed phrase is sealed with a password of its own, typed by whoever made it, and not with the
     * one the Vault keeps behind the screen lock. Marked by a file beside the wallet file, so it goes when the wallet does.
     */
    fun own(dir: String = sel): Boolean = FileAt(path(dir), OWN).exists()
    private fun mark(dir: String, own: Boolean) { FileAt(path(dir), OWN).let { f -> if (own) f.writeText("") else f.delete() } }
    /** Why the core did not do what was asked, in its words; a password that does not open the phrase is recognised by a field of its answer, not by those words. */
    private fun failed(a: Reply, own: Boolean = own()): String = if (!a.badPassword) a.error else if (own) WRONG else LOST

    fun hasWallet() = dirs().isNotEmpty()
    /** Each of these answers `OK`, or the reason it did not happen. */
    fun watch(address: String, name: String): String {
        if (views().any { address in it.addresses }) return "that address is already a wallet on this $device"
        val d = fresh(); seedChain(d)
        val a = ask(listOf(), "wallet", "watch", "--state", path(d), "--address", address, "--name", name.ifBlank { "Wallet" })
        if (a.ok) sel = d else FileAt(path(d)).deleteRecursively()
        return if (a.ok) OK else a.error
    }
    /** `own`: `pass` is a password typed for this wallet, not the Vault's. */
    fun create(name: String, pass: String, own: Boolean = false): String {
        val d = fresh(); seedChain(d)
        val a = sealedAsk(pass, "wallet", "new", "--state", path(d), "--name", name.ifBlank { "Wallet" })
        if (a.ok) { mark(d, own); sel = d } else FileAt(path(d)).deleteRecursively()
        return if (a.ok) OK else failed(a, own)
    }
    /** A seed phrase becomes a wallet; if it is the phrase of a read-only wallet already here, that wallet gets its key. */
    fun restore(phrase: String, name: String, pass: String, own: Boolean = false): String {
        val words = phrase.trim().lowercase().split(Regex("[\\s\u3000]+")).filter { it.isNotEmpty() }
        if (words.size !in listOf(12, 15, 18, 21, 24)) return "a seed phrase has 12, 15, 18, 21 or 24 words; this has ${words.size}"
        val line = words.joinToString(" ")
        // the core checks the words and the phrase's checksum: a slip would otherwise open an empty wallet at another address
        val derived = ask(listOf(line), "wallet", "derive", "--mnemonic-file", "-")
        if (!derived.ok) return derived.error.removePrefix("seed phrase not accepted: ").replaceFirstChar { c -> c.uppercase() } + "."
        val address = derivedAddress(derived) ?: return "could not derive a key from that phrase"
        val same = views().firstOrNull { it.address == address }
        // a full wallet of this phrase that this password does not open (the phone's key went, its own password is
        // forgotten, or it is to be guarded the other way from now on) is put back by the phrase, sealed with this password
        val closed = same != null && !same.readOnly && sealedAsk(pass, "wallet", "mnemonic", "--state", path(same.dir)).badPassword
        if (same != null && !same.readOnly && !closed) return "that wallet is already on this $device (${same.name})"
        val d = same?.dir ?: fresh()
        if (same == null) seedChain(d)
        // the wallet file is written first and its boxes looked for after (by the next sync); `restored` says the file is there
        val a = ask(listOf(pass, line), "wallet", "restore", "--state", path(d), "--mnemonic-file", "-", "--embed", "--name", name.ifBlank { "Wallet" }, *(if (closed) arrayOf("--reseal") else arrayOf()), "--pass-stdin")
        val done = restored(a)
        if (done) { mark(d, own); sel = d } else if (same == null) FileAt(path(d)).deleteRecursively()
        return if (done) OK else failed(a, own)
    }
    fun rename(name: String) = run("wallet", "name", "--state", state, "--name", name.ifBlank { "Wallet" })
    fun addAddress(pass: String) = sealed(pass, "wallet", "address", "--state", state, "--add")
    /** The extended public key, or "! " and the reason there is none. */
    fun pubkey(pass: String): String = sealedAsk(pass, "wallet", "pubkey", "--state", state).let { a -> textOf(a, "base58") ?: ("! " + failed(a)) }
    /** The seed phrase, or "! " and the reason it cannot be shown. */
    fun mnemonic(pass: String): String = sealedAsk(pass, "wallet", "mnemonic", "--state", state).let { a -> textOf(a, "mnemonic") ?: ("! " + failed(a)) }
    /** Whether this password opens the wallet's seed phrase. */
    fun opens(pass: String): Boolean = textOf(sealedAsk(pass, "wallet", "mnemonic", "--state", state), "mnemonic") != null
    fun remove() { val d = sel; FileAt(path(d)).deleteRecursively(); prefs.remove(payKey(d)) }
    fun tips(): Boolean = locked(this) { tipsNow() }
    private fun tipsNow(): Boolean {
        FileAt(chain).mkdirs()
        // agreed: the core ended with success (it exits 1 when no tip is agreed on, or the chain could not be saved) and its answer says so
        val a = if (testnet) ask(listOf(), "tips", "--peers", "2", "--tries", "8", "--quorum", "2", "--state", chain, "--seeds", seeds())
            else ask(listOf(), "tips", "--peers", "4", "--tries", "24", "--state", chain, "--seeds", seeds())
        val ok = tipAgreed(a)
        // the nodes are found once here, against the chain just agreed on, and the list goes to every wallet with the chain
        if (ok && node == "auto") run("nodes", "--state", chain)
        if (ok) dirs().forEach { seedChain(it) }
        return ok
    }
    fun sync(): String {
        if (!tips()) return "no agreed tip"
        var ok = true
        for (d in dirs()) {
            flush(d)
            val proven = ask(listOf(), "wallet", "status", "--state", path(d), "--node", node, "--reconcile").ok
            // a token never seen before: ask a node for its name once; the core stores the answer
            val have = reported(d)
            view(d)?.tokens?.filter { it.id !in shipped && it.id !in have }?.take(5)?.forEach { run("token", "--state", path(d), "--node", node, "--id", it.id) }
            if (!proven) ok = false
        }
        return if (ok) "ok" else "tip agreed; boxes not proven"
    }
    /** Checks a receipt. The answer is the core's JSON object (its `checked` list says what the proof showed for each box), or a sentence if there is none. */
    fun receive(ids: String): String { if (!tips()) return "no agreed tip"; return ask(listOf(), "wallet", "receive", "--state", state, "--node", node, "--ids", ids).let { it.json?.toString() ?: it.error } }
    /** Sends now if the network answers; otherwise signs the payment and leaves it waiting (the answer then starts with QUEUED). */
    fun send(to: String, nano: Long, tokens: List<Pair<TokenLine, Long>>, feeToken: TokenLine?, feeMax: Long, pass: String): String {
        // ERG if any was asked for (with none, the core gives the tokens' box the least it may hold); the first token, then the others
        val what = (if (nano > 0) arrayOf("--nanoerg", nano.toString()) else arrayOf()) +
            tokens.take(1).flatMap { (t, raw) -> listOf("--token", t.id, "--token-amount", raw.toString()) } +
            tokens.drop(1).flatMap { (t, raw) -> listOf("--also-token", "${t.id}:$raw") }
        // the fee in a token being sent (Babel fees): needs the network, to find and prove an offer
        // `feeMax` is the most token units the screen said the fee could cost; the core signs nothing above it
        val babel = if (feeToken != null) arrayOf("--babel", feeToken.id, "--babel-max", feeMax.toString()) else arrayOf()
        if (tips()) return sent(sealedAsk(pass, "wallet", "send", "--state", state, "--node", node, "--to", to, *what, *babel, "--broadcast"))
        if (babel.isNotEmpty()) return JSONObject().put("state", "refused").put("error", "paying the fee in tokens needs the network; there is none right now").toString()
        FileAt(state, "outbox").mkdirs()
        return sent(sealedAsk(pass, "wallet", "send", "--state", state, "--to", to, *what, "--parcel-out", FileAt(state, "outbox/${now()}.json").absolutePath))
    }
    /**
     * What became of a payment or a signed request, as one object for the screens, every part of it from a field of the
     * core's answer: `state` is "sent" (a node took it), "queued" (signed, waiting for network), "unsure" (a node was
     * sent it and did not answer: it may have gone, it is kept as waiting, and it must not be paid again on a guess) or
     * "refused"; with `tx`, `receipt`, `change`, `fee_token_units` (-1: the fee was paid in ERG), `tx_bytes` and `error`.
     */
    private fun sent(a: Reply): String = sentAnswer(a, failed(a).removePrefix("send: ")).toString()
    /** Keep a payer's signed transaction and expect the boxes that pay this wallet. */
    /** The core's answer as JSON text: `promised` (the box ids that pay this wallet), `nanoerg`, `tokens`; or `error`. */
    fun promise(txHex: String): String = ask(listOf(), "wallet", "promise", "--state", state, "--tx", txHex).let { a -> if (a.ok) a.json.toString() else JSONObject().put("error", a.error).toString() }
    private fun waiting(dir: String): Int = FileAt(path(dir), "outbox").listFiles()?.size ?: 0
    /** Sends whatever was signed while offline. Called on every sync that reaches the network. */
    private fun flush(dir: String) {
        if (waiting(dir) == 0) return
        // the core keeps each payment's file until a proof shows it in a block, so "sent" is read from its answer, not from the folder being empty
        val a = ask(listOf(), "outbox", "--state", path(dir), "--rest", node)
        if (a.ok && (a.json?.optInt("sent") ?: 0) > 0) prefs.put(payKey(dir), JSONArray(payments(dir).map { if (it.optString("state") == "queued") it.put("state", "sent") else it }).toString())
    }
    /** A signing request: what it does, in the wallet's own words; with `pass` (which opens the seed phrase), sign it and send it. */
    fun request(code: String, pass: String?): String {
        if (!tips()) return "request refused: no agreed chain tip; sync first"
        run("wallet", "status", "--state", state, "--node", node)
        // read: what the request does, as lines for the screen, written here from the fields of the core's answer
        if (pass == null) return ask(listOf(), "wallet", "request", "--state", state, "--node", node, "--code", code).let { a ->
            if (a.ok && a.json != null) JSONObject().put("lines", JSONArray(requestLines(a.json))).toString() else JSONObject().put("error", a.error.removePrefix("request refused: ")).toString()
        }
        return sent(sealedAsk(pass, "wallet", "request", "--state", state, "--node", node, "--code", code, "--sign"))
    }
    private fun erg(nano: Long, sign: Boolean = false) = (if (sign && nano >= 0) "+" else "") + shown(nano, 9) + " ERG"
    /** What a signing request does, from the core's fields: amounts in ERG, tokens by name where the name is known. */
    private fun requestLines(j: JSONObject): List<String> {
        fun tokens(o: JSONObject?): Map<String, Long> = o?.optJSONObject("tokens")?.let { t -> t.keys().asSequence().associateWith { t.optLong(it) } } ?: mapOf()
        fun side(name: String) = j.optJSONObject(name).let { o -> erg(o?.optLong("nanoerg") ?: 0L) + tokens(o).entries.joinToString("") { (id, n) -> ", " + tokenText("", n.toString(), id) } }
        val (give, get) = tokens(j.optJSONObject("give")) to tokens(j.optJSONObject("get"))
        val net = (give.keys + get.keys).map { it to ((get[it] ?: 0L) - (give[it] ?: 0L)) }.filter { it.second != 0L }
        val n = j.optJSONObject("inputs")
        return listOf("request: ${n?.optInt("all") ?: 0} inputs (${n?.optInt("yours") ?: 0} yours, ${n?.optInt("others") ?: 0} proven others)",
            "you give: " + side("give"), "you get back: " + side("get"), "others put in: " + side("others_put_in"), "others receive: " + side("others_receive"),
            "network fee: " + erg(j.optLong("fee_nanoerg")),
            "net for you: " + erg(j.optLong("net_nanoerg"), true) + net.joinToString("") { (id, d) -> ", " + tokenText(if (d > 0) "+" else "-", kotlin.math.abs(d).toString(), id) })
    }
    /** "N of token ID" as an amount and a name, where the name is known. */
    fun tokenText(sign: String, raw: String, id: String): String {
        val s = shipped[id]
        val r = reported()[id]?.takeIf { it.first.isNotEmpty() }
        fun amount(decimals: Int) = sign + shown(raw, decimals)
        return when {
            s != null -> amount(s.second) + " " + s.first
            // anyone can mint a token and call it SigUSD: a reported name is marked and shown with the start of its id
            r != null -> amount(r.second) + " " + r.first + "* (" + id.take(8) + "…)"
            else -> "$sign$raw of token ${id.take(10)}…"
        }
    }
    /** The best proven offer of ERG for a token (Babel fees) that holds enough to be used: nanoERG per smallest token
     *  unit, 0 if there is none, or UNPROVEN if the offers could not be checked just now. The proof is against the node's tip, so a tip one block old is refreshed once. */
    fun babelPrice(token: String): Long {
        fun offers() = ask(listOf(), "wallet", "babel-list", "--state", state, "--node", node, "--token", token)
        var a = offers()
        // the proof is against the node's tip: if it did not check, the tip is refreshed once and the offers asked for again
        if (!a.ok && tips()) a = offers()
        // still not checked (the node and the peers a block apart, or no node): that is not "no offer"
        if (!a.ok) return UNPROVEN
        val list = a.json?.takeIf { a.ok }?.optJSONArray("offers") ?: JSONArray()
        // an offer too small to cover the missing ERG cannot be the one used, so its price is not the one to quote
        return (0 until list.length()).map { list.getJSONObject(it) }.filter { it.optLong("available_nanoerg") >= FEE_SHORT_MAX }.map { it.optLong("nanoerg_per_unit") }.filter { it > 0 }.maxOrNull() ?: 0L
    }
    fun qr(text: String): List<String> = run("qr", text).lines().filter { it.isNotEmpty() && it.all { c -> c == '0' || c == '1' } }

    /** The last ten payments made or accepted on this phone on this network, newest first: local history, asked of nobody. */
    fun payments(dir: String = sel): List<JSONObject> = try { JSONArray(prefs.getString(payKey(dir), "[]") ?: "[]").let { a -> (0 until a.length()).map { a.getJSONObject(it) } } } catch (e: Exception) { listOf() }
    fun remember(direction: String, what: String, other: String, receipt: String, purpose: String, change: String = "", state: String = "sent", tx: String = "") {
        val all = listOf(JSONObject().put("tx", tx).put("dir", direction).put("what", what).put("other", other).put("receipt", receipt).put("purpose", purpose)
            .put("change", change).put("state", state).put("at", now())) + payments()
        prefs.put(payKey(), JSONArray(all.take(10)).toString())
    }

    /** Where a payment stands: signed but not sent, sent but not yet in a block, or done. */
    fun status(p: JSONObject, w: WalletView): String = when {
        p.optString("dir", "out") != "out" -> if (p.optString("state") == "promised" && p.optString("receipt") in w.expecting) "Promised, not on chain yet" else "Received"
        p.optString("state") == "queued" -> "Waiting for network"
        // the core's own verdict, read off the same proof as the balance
        w.sent[p.optString("tx")] != null -> w.sent.getValue(p.optString("tx")).let { (result, at) -> when (result) {
            "waiting" -> "Waiting to confirm"
            "confirmed" -> "Confirmed · at least ${maxOf(1, w.provenAt - at + 1)} confirmation" + if (w.provenAt - at + 1 > 1) "s" else ""
            "spent elsewhere" -> "Failed: its coins were spent by another payment"
            // its coins are gone and so is everything it created: confirmed and since spent by the payee, or spent by something else; the proof cannot tell which
            "coins spent" -> "Its coins are spent, but what it paid is no longer there to prove. Check with the payee before paying again"
            else -> "Not included in a block: its coins are back in this wallet"
        } }
        p.optString("change").isNotEmpty() && p.optString("change") in w.expecting -> "Waiting to confirm"
        // no word from the core on it any more (it keeps the last twenty): not something to call confirmed
        else -> "Sent · no longer tracked"
    }
    fun unfinished(status: String) = status.startsWith("Waiting") || status.startsWith("Promised")
    fun pending(w: WalletView): List<JSONObject> = payments().filter { unfinished(status(it, w)) }

    /** The selected wallet's boxes as the last sync left them, largest first. */
    fun boxes(dir: String = sel): List<BoxLine> = try {
        val b = JSONObject(FileAt(path(dir), "wallet.json").readText()).getJSONObject("boxes")
        b.keys().asSequence().map { id -> b.getJSONObject(id).let { BoxLine(id, it.getLong("value"), it.optJSONArray("assets")?.length() ?: 0, it.optInt("proven_at"), it.optInt("confirmed_at")) } }.sortedByDescending { it.nano }.toList()
    } catch (e: Exception) { listOf() }
    /** Spends every box of the wallet (up to sixty) into one box back to itself. Answers as `send` does. */
    fun consolidate(pass: String): String {
        if (!tips()) return JSONObject().put("state", "refused").put("error", "consolidating needs the network; there is none right now").toString()
        run("wallet", "status", "--state", state, "--node", node)
        return sent(sealedAsk(pass, "wallet", "consolidate", "--state", state, "--node", node, "--broadcast"))
    }
    fun views(): List<WalletView> = dirs().mapNotNull { view(it) }
    /** Wallet folders whose file this app cannot read, each with why: said on the first screen, not left out in silence. */
    fun unreadable(): List<String> = dirs().filter { view(it) == null }.map { d ->
        val version = try { JSONObject(FileAt(path(d), "wallet.json").readText()).optInt("version", 0) } catch (e: Exception) { -1 }
        if (version > WALLET_FILE_VERSION) "A wallet on this $device ($d) was written by a newer version of tachys (wallet file version $version; this app reads up to $WALLET_FILE_VERSION). It is not shown, and it has not been touched. Update the app to use it."
        else "A wallet file on this $device ($d) cannot be read: it is damaged or cut short. It is not shown, and it has not been touched. It may still hold a seed phrase."
    }
    fun wallet(): WalletView? = view(sel)
    fun view(dir: String): WalletView? = try {
        val w = JSONObject(FileAt(path(dir), "wallet.json").readText())
        if (w.optInt("version", 0) > WALLET_FILE_VERSION) throw IllegalStateException("newer wallet file")
        val boxes = w.getJSONObject("boxes")
        var nano = 0L; var proven = 0; var seen = 0L
        val sums = LinkedHashMap<String, Long>()
        for (k in boxes.keys()) {
            val b = boxes.getJSONObject(k)
            // the balance is what is confirmed under a tip the peers agree on; a box only one node has shown is counted apart
            if (b.isNull("confirmed_at")) { seen += b.getLong("value"); continue }
            nano += b.getLong("value"); proven = maxOf(proven, b.optInt("proven_at"))
            val a = b.optJSONArray("assets") ?: JSONArray()
            for (i in 0 until a.length()) a.getJSONObject(i).let { sums[it.getString("id")] = (sums[it.getString("id")] ?: 0L) + it.getLong("amount") }
        }
        val rep = reported(dir)
        val tokens = sums.map { (id, amt) ->
            val s = shipped[id]; val r = rep[id]
            when {
                s != null -> TokenLine(id, s.first, s.second, amt, false)
                r != null && r.first.isNotEmpty() -> TokenLine(id, r.first, r.second, amt, true)
                else -> TokenLine(id, id.take(10) + "…", 0, amt, true)
            }
        }.sortedBy { it.reported }
        WalletView(w.optString("name", "Wallet"), w.getString("address"),
            shownFixed(nano, 9, 6), boxes.length(), tokens, proven,
            w.optBoolean("watch", false), (w.optJSONArray("expect") ?: JSONArray()).let { a -> (0 until a.length()).map { a.getString(it) } }, dir,
            w.optJSONArray("address_list")?.let { a -> (0 until a.length()).map { a.getString(it) } }?.takeIf { it.isNotEmpty() } ?: listOf(w.getString("address")),
            ((w.optJSONArray("pending") ?: JSONArray()).let { a -> (0 until a.length()).map { a.getJSONObject(it).optString("tx") to ("waiting" to 0) } } +
             (w.optJSONArray("done") ?: JSONArray()).let { a -> (0 until a.length()).map { a.getJSONObject(it).let { d -> d.optString("tx") to (d.optString("result") to d.optInt("seen_at")) } } }).toMap(), nano, seen)
    } catch (e: Exception) { null }
    companion object {
        /** What a command that did what it was asked answers. */
        const val OK = "ok"
        /** The file that marks a wallet sealed with a password of its own. The core does not read it. */
        /** `babelPrice`: the offers could not be checked against the agreed tip; asking again may find one. */
        const val UNPROVEN = -2L
        const val OWN = "own-password"
        const val WRONG = "That is not this wallet's password."
        val LOST get() = "This wallet's seed phrase can no longer be opened on this $device: the key that guarded it is gone, which happens when the $device's screen lock is removed. Restore the wallet from its written seed phrase."
    }
}

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
        if (pass == null) status = why else scope.launch { plain = withContext(background) { core.sealAll(pass) }; refresh() }
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

/** What this machine is called in the sentences shown ("phone", "computer"): the platform's word, set when the screens start. */
var device = "phone"

/** The top bar. `action` puts a second tap target at its right-hand end (the "+" on the wallet list). */
@Composable
fun Header(title: String, sub: String, action: String? = null, onAction: () -> Unit = {}, onClick: () -> Unit = {}) {
    val click by rememberUpdatedState(onClick); val act by rememberUpdatedState(onAction)
    Row(Modifier.fillMaxWidth().background(Bar), verticalAlignment = Alignment.CenterVertically) {
        // tap targets that take no keyboard focus, so a hardware Enter cannot trigger them
        Column(Modifier.weight(1f).pointerInput(Unit) { detectTapGestures { click() } }.padding(horizontal = 20.dp, vertical = 14.dp)) {
            Text(title, color = Color.White, fontSize = 22.sp, fontWeight = FontWeight.Bold)
            Text(sub, color = Dim, fontSize = 15.sp)
        }
        if (action != null) Box(Modifier.size(64.dp).pointerInput(Unit) { detectTapGestures { act() } }, contentAlignment = Alignment.Center) {
            Text(action, color = Color.White, fontSize = 34.sp)
        }
    }
}

@Composable
fun CardBox(onClick: (() -> Unit)? = null, content: @Composable ColumnScope.() -> Unit) {
    Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp).fillMaxWidth().background(Card, RoundedCornerShape(10.dp))
        .let { if (onClick != null) it.clickable { onClick() } else it }.padding(18.dp), content = content)
}

@Composable
fun Erg(amount: String, size: Int) {
    val (whole, frac) = amount.split(".").let { it[0] to it.getOrElse(1) { "" } }
    Row(verticalAlignment = Alignment.Bottom) {
        Text(whole, color = Color.White, fontSize = size.sp, fontWeight = FontWeight.Bold)
        Text(frac, color = Color.White, fontSize = (size * 0.6).sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(bottom = (size * 0.28).dp, start = 2.dp))
        Text(" ERG", color = Color.White, fontSize = size.sp, fontWeight = FontWeight.Bold)
    }
}

@Composable
fun Pill(text: String, bg: Color, fg: Color, modifier: Modifier = Modifier, outline: Boolean = false, onClick: () -> Unit = {}) {
    Box(modifier.height(48.dp).let { if (outline) it.border(1.dp, Color.White, RoundedCornerShape(12.dp)) else it.background(bg, RoundedCornerShape(12.dp)) }
        .clickable { onClick() }, contentAlignment = Alignment.Center) { Text(text, color = fg, fontSize = 18.sp, fontWeight = FontWeight.Bold) }
}

/** A text field; `onScan` puts a QR button at its right-hand end. */
@Composable
fun Field(value: String, hint: String, numeric: Boolean = false, words: Boolean = false, secret: Boolean = false, onScan: (() -> Unit)? = null, onChange: (String) -> Unit) {
    val focus = LocalFocusManager.current // "Done" on the keyboard closes it
    val keys = LocalSoftwareKeyboardController.current // (said outright: the focus given up can land on the form's first field, which would bring the keyboard straight back)
    Row(Modifier.fillMaxWidth().background(Bg, RoundedCornerShape(8.dp)), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.weight(1f).padding(12.dp)) {
            if (value.isEmpty()) Text(hint, color = Dim, fontSize = 16.sp)
            // addresses, ids and amounts are not words: no autocorrect, no suggestions
            BasicTextField(value, onChange, textStyle = TextStyle(color = Color.White, fontSize = 16.sp), cursorBrush = SolidColor(Accent), modifier = Modifier.fillMaxWidth(), singleLine = true,
                visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
                keyboardOptions = KeyboardOptions(autoCorrectEnabled = words, keyboardType = if (numeric) KeyboardType.Decimal else if (words) KeyboardType.Text else KeyboardType.Password, imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { focus.clearFocus(); keys?.hide() }))
        }
        if (onScan != null && LocalPlatform.current.camera) QrButton { focus.clearFocus(); onScan() }
    }
}

/** A small QR-code glyph: three corner squares and a few dots. */
@Composable
fun QrButton(onClick: () -> Unit) {
    Box(Modifier.size(48.dp).clickable { onClick() }, contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(24.dp)) {
            val u = size.width / 7f
            fun finder(x: Float, y: Float) { drawRect(Color.White, Offset(x + u / 2, y + u / 2), Size(2 * u, 2 * u), style = Stroke(u)); drawRect(Color.White, Offset(x + u, y + u), Size(u, u)) }
            finder(0f, 0f); finder(4 * u, 0f); finder(0f, 4 * u)
            listOf(4 to 4, 6 to 4, 5 to 5, 4 to 6, 6 to 6).forEach { (x, y) -> drawRect(Color.White, Offset(x * u, y * u), Size(u, u)) }
        }
    }
}

@Composable
fun Qr(rows: List<String>) {
    if (rows.isEmpty()) return
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(240.dp).background(Color.White).padding(12.dp)) {
            val cell = size.width / rows.size
            rows.forEachIndexed { y, row -> row.forEachIndexed { x, c -> if (c == '1') drawRect(Color.Black, Offset(x * cell, y * cell), Size(cell + 0.5f, cell + 0.5f)) } }
        }
    }
}

const val REPORTED_NOTE = "* The name and its decimal places are what a node reported, not something proven."
/** The most ERG a payment from this app can be short of: the fee, the payee's box and the change box. */
const val FEE_SHORT_MAX = 3_100_000L
/** The most token units a fee paid in a token can cost at `price` nanoERG per unit (rounded up, and safe for any price). */
fun feeCeiling(price: Long): Long = if (price <= 0L) 0L else (FEE_SHORT_MAX - 1) / price + 1
/** Of `held` units of a token, what can be sent when the fee is paid in it at `price` nanoERG a unit: the rest is kept for the fee. */
fun leftAfterFee(held: Long, price: Long): Long = (held - feeCeiling(price)).coerceAtLeast(0L)
/** Did the core say a node took the transaction? Its own line says so; nothing a node writes can be on such a line. */
/** "No tokens", "1 token", "5 tokens". */
fun tokenCount(n: Int) = when (n) { 0 -> "No tokens"; 1 -> "1 token"; else -> "$n tokens" }

@Composable
fun TokenRow(t: TokenLine, size: Int, centered: Boolean = false) {
    Row(if (centered) Modifier.fillMaxWidth().padding(vertical = 5.dp) else Modifier, horizontalArrangement = if (centered) Arrangement.Center else Arrangement.Start,
        verticalAlignment = Alignment.CenterVertically) {
        Text(t.amount + " ", color = Color.White, fontSize = size.sp, fontWeight = FontWeight.Bold)
        Text(t.name + if (t.reported) "*" else "", color = Color.White, fontSize = size.sp)
    }
}

@Composable
fun WalletCard(w: WalletView, onOpen: () -> Unit, onReceive: () -> Unit, onSend: () -> Unit, onConfig: () -> Unit) {
    CardBox(onOpen) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(w.name + if (w.readOnly) "  (read-only)" else "", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            // this wallet's own settings: name, addresses, public key, seed phrase
            Box(Modifier.size(44.dp).clickable { onConfig() }, contentAlignment = Alignment.Center) { Text("⚙", color = Color.White, fontSize = 24.sp) }
        }
        Erg(w.erg, 30)
        Text(when {
            w.provenAt > 0 -> "proven at height ${w.provenAt} · " + (if (w.boxes == 1) "1 box" else "${w.boxes} boxes")
            w.seen > 0 -> "nothing confirmed yet"
            else -> "no boxes yet" }, color = Dim, fontSize = 13.sp)
        // coins seen only in a block one node has shown are not in the balance, but they are there: say so, never "no boxes"
        if (w.seen > 0) Text("+ " + shown(w.seen, 9) + " ERG seen in a block only one node has shown yet: not counted and not spent until the peers reach it", color = Dim, fontSize = 13.sp)
        if (w.tokens.isNotEmpty()) {
            Spacer(Modifier.height(10.dp))
            Text(tokenCount(w.tokens.size), color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.Bold)
            w.tokens.take(4).forEach { TokenRow(it, 18) }
            if (w.tokens.size > 4) Text("+${w.tokens.size - 4} more token" + (if (w.tokens.size - 4 == 1) "" else "s"), color = Color.White, fontSize = 18.sp)
            if (w.tokens.take(4).any { it.reported }) Text(REPORTED_NOTE, color = Dim, fontSize = 12.sp, modifier = Modifier.padding(top = 4.dp))
        }
        Spacer(Modifier.height(14.dp))
        Pill("Details", Color.Transparent, Color.White, Modifier.fillMaxWidth(), outline = true, onClick = onOpen)
        Spacer(Modifier.height(10.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Pill("Receive", Color.White, Color.Black, Modifier.weight(1f), onClick = onReceive)
            Pill("Send", Accent, Color.Black, Modifier.weight(1f), onClick = onSend)
        }
    }
}

/** Amounts in the core's answer, made readable: nanoERG as ERG. */
fun readable(line: String): String = Regex("([+-]?)(\\d+) nanoERG").replace(line) { m -> m.groupValues[1] + shown(m.groupValues[2], 9) + " ERG" }

/** A signing request (ErgoPay): scan it, read what it does in the wallet's own words, then sign or walk away. */
@Composable
fun Request(core: Core, code: String, busy: Boolean, onCode: (String) -> Unit, ask: (String, Boolean, (String) -> Unit) -> Unit) {
    var scanning by remember { mutableStateOf(code.isEmpty()) }
    // keyed to the request: a new one scanned after a refusal is read afresh, not left behind the old answer
    var summary by remember(code) { mutableStateOf(listOf<String>()) }
    var problem by remember(code) { mutableStateOf("") }
    var done by remember(code) { mutableStateOf("") }
    fun read(c: String) = ask(c, false) { r ->
        val j = try { JSONObject(r) } catch (e: Exception) { null }
        val lines = j?.optJSONArray("lines")?.let { a -> (0 until a.length()).map { a.getString(it) } } ?: listOf()
        if (lines.isNotEmpty()) { summary = lines; problem = "" }
        else { summary = listOf(); problem = j?.optString("error")?.takeIf { it.isNotEmpty() } ?: r.lines().lastOrNull { it.isNotBlank() } ?: "could not read the request" }
    }
    // read once the app is free: a request that arrives while a sync runs (the app syncs as it opens) was dropped, and the
    // screen said "Reading…" for ever (found on the Note 2)
    LaunchedEffect(code, busy) { if (code.isNotEmpty() && summary.isEmpty() && problem.isEmpty() && !busy) read(code) }
    if (scanning) { Scanner("Scan a request to sign", { scanning = false; onCode(it.trim()) }, { scanning = false }); return }
    CardBox {
        Text("Request to sign", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("Someone else built this transaction. What follows is this wallet's own reading of it, not their description.", color = Dim, fontSize = 13.sp)
        Spacer(Modifier.height(8.dp))
        when {
            done.isNotEmpty() -> Text(done, color = Color.White, fontSize = 16.sp)
            problem.isNotEmpty() -> Text("Refused: $problem", color = Color.White, fontSize = 16.sp)
            summary.isEmpty() -> Text(if (code.isEmpty()) "Nothing scanned." else "Reading…", color = Color.White, fontSize = 16.sp)
            else -> {
                summary.forEach { l ->
                    val strong = l.startsWith("net for you")
                    Text(l.replaceFirstChar { it.uppercase() }, color = if (strong) Accent else Color.White, fontSize = if (strong) 18.sp else 15.sp, fontWeight = if (strong) FontWeight.Bold else FontWeight.Normal)
                }
                if (summary.any { it.contains("* (") }) Text(REPORTED_NOTE + " Check the id against one you trust.", color = Dim, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
                Spacer(Modifier.height(12.dp))
                Pill(if (busy) "Signing…" else "Sign and send", Accent, Color.Black, Modifier.fillMaxWidth()) {
                    ask(code, true) { r ->
                        val j = try { JSONObject(r) } catch (e: Exception) { null }
                        done = when (j?.optString("state")) {
                            "sent" -> {
                                // kept in the phone's own history, with the wallet's reading of what it cost
                                core.remember("out", "a signed request (" + (summary.firstOrNull { it.startsWith("net for you") } ?: "") + ")", "", "", "", tx = j.optString("tx"))
                                "Signed and sent. It is recorded under History on the Details screen."
                            }
                            "unsure" -> {
                                core.remember("out", "a signed request (" + (summary.firstOrNull { it.startsWith("net for you") } ?: "") + ")", "", "", "", tx = j.optString("tx"))
                                "Signed. The node did not answer, so it may or may not have gone out. It is under Pending and the next sync will settle it. Do not sign it again until then."
                            }
                            else -> "Not sent: " + (j?.optString("error")?.takeIf { it.isNotEmpty() } ?: r.lines().lastOrNull { it.isNotBlank() } ?: "unknown")
                        }
                    }
                }
            }
        }
        Spacer(Modifier.height(10.dp))
        if (LocalPlatform.current.camera) Pill("Scan another", Bg, Color.White, Modifier.fillMaxWidth()) { summary = listOf(); problem = ""; done = ""; scanning = true }
    }
}

/** One payment from the phone's own record, with where it stands. Tap a sent one for its receipt. */
@Composable
fun PaymentRow(core: Core, w: WalletView, p: JSONObject) {
    var open by remember { mutableStateOf(false) }
    val out = p.optString("dir", "out") == "out"
    val status = core.status(p, w)
    Column(Modifier.fillMaxWidth().clickable { open = !open }.padding(vertical = 8.dp)) {
        val other = p.optString("other")
        // one Text in two styles, not two side by side: beside a long first one, the second was squeezed into a column a
        // letter wide and the row grew as tall as that column
        Text(androidx.compose.ui.text.buildAnnotatedString {
            pushStyle(androidx.compose.ui.text.SpanStyle(fontWeight = FontWeight.Bold)); append(if (out) "Sent ${p.optString("what")} " else "Received "); pop()
            append(if (out) "to " + other.take(8) + "…" + other.takeLast(6) else "by receipt " + p.optString("receipt").take(10) + "…")
        }, color = Color.White, fontSize = 17.sp)
        if (p.optString("purpose").isNotEmpty()) Text("Purpose: " + p.optString("purpose"), color = Color.White, fontSize = 14.sp)
        Text(androidx.compose.ui.text.buildAnnotatedString {
            pushStyle(androidx.compose.ui.text.SpanStyle(fontWeight = FontWeight.Bold, color = if (core.unfinished(status)) Accent else Dim)); append(status); pop()
            append("  ·  " + LocalPlatform.current.time(p.optLong("at")))
        }, color = Dim, fontSize = 13.sp)
        if (open && out) { Spacer(Modifier.height(8.dp)); Qr(remember(p.optString("receipt")) { core.qr(p.optString("receipt")) }) }
    }
}

@Composable
fun Detail(core: Core, w: WalletView, onBoxes: () -> Unit, onPending: () -> Unit) {
    val all = remember(w) { core.payments() }
    val waiting = all.filter { core.unfinished(core.status(it, w)) }
    CardBox {
        Text("Show content for", color = Color.White, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text(if (w.addresses.size > 1) "All ${w.addresses.size} addresses" else "Main address", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        w.addresses.forEach { a -> Text(a.take(12) + "…" + a.takeLast(8), color = Dim, fontSize = 14.sp) }
    }
    CardBox {
        Text("Balance", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Erg(w.erg, 30)
        Text("every box proven against the peers' agreed tip", color = Dim, fontSize = 13.sp)
        if (w.seen > 0) Text("+ " + shown(w.seen, 9) + " ERG seen in a block only one node has shown yet: not counted and not spent until the peers reach it", color = Accent, fontSize = 13.sp)
        Spacer(Modifier.height(10.dp))
        // payments not finished yet: tap to see them
        Row(Modifier.fillMaxWidth().clickable { onPending() }.padding(vertical = 6.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            Text("Pending", color = Color.White, fontSize = 18.sp, fontWeight = FontWeight.Bold)
            Text(if (waiting.isEmpty()) "none  ›" else "${waiting.size}  ›", color = if (waiting.isEmpty()) Dim else Accent, fontSize = 18.sp, fontWeight = FontWeight.Bold)
        }
        // the boxes the balance is made of: tap to see them, and to make them one
        Row(Modifier.fillMaxWidth().clickable { onBoxes() }.padding(vertical = 6.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            Text("Boxes", color = Color.White, fontSize = 18.sp, fontWeight = FontWeight.Bold)
            Text(if (w.boxes == 0) "none  ›" else "${w.boxes}  ›", color = if (w.boxes == 0) Dim else Accent, fontSize = 18.sp, fontWeight = FontWeight.Bold)
        }
    }
    if (w.tokens.isNotEmpty()) CardBox {
        Row { Text("${w.tokens.size} ", color = Color.White, fontSize = 22.sp, fontWeight = FontWeight.Bold); Text(if (w.tokens.size == 1) "token" else "tokens", color = Accent, fontSize = 22.sp, fontWeight = FontWeight.Bold) }
        Spacer(Modifier.height(6.dp))
        w.tokens.forEach { TokenRow(it, 19, centered = true) }
        if (w.tokens.any { it.reported }) Text(REPORTED_NOTE, color = Dim, fontSize = 12.sp, modifier = Modifier.fillMaxWidth().padding(top = 6.dp), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
    }
    CardBox {
        Text("History", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("The last ten finished payments made or accepted on this $device, kept here and asked of nobody. Tap a sent one for its receipt.", color = Dim, fontSize = 13.sp)
        val finished = all.filter { !core.unfinished(core.status(it, w)) }
        if (finished.isEmpty()) Text("None yet.", color = Color.White, fontSize = 15.sp)
        finished.forEach { PaymentRow(core, w, it) }
    }
}

@Composable
fun Receive(core: Core, w: WalletView, busy: Boolean, check: (String, (String) -> Unit) -> Unit) {
    var which by remember(w.dir) { mutableStateOf(0) }
    val shown = w.addresses.getOrElse(which) { w.address }
    val qr = remember(shown) { core.qr(shown) }
    var receipt by remember { mutableStateOf("") }
    var purpose by remember { mutableStateOf("") }
    var result by remember { mutableStateOf("") }
    var scanning by remember { mutableStateOf(false) }
    val focus = LocalFocusManager.current
    fun run(id: String, why: String) = check(id) { r ->
        // what the core's proof showed for this box (its JSON answer): the state, the block, and how many agreed blocks deep it is (a lower bound)
        val j = try { JSONObject(r) } catch (e: Exception) { null }
        val box = j?.takeIf { it.optBoolean("ok") && it.optInt("v") == 1 }?.optJSONArray("checked")?.let { a -> (0 until a.length()).map { a.getJSONObject(it) }.firstOrNull { it.optString("id") == id } }
        val state = box?.optString("state")
        val conf = box?.optInt("confirmations") ?: 0
        val at = box?.optInt("height")?.toString() ?: "?"
        result = when {
            state == "proven" && conf >= 1 -> {
                val known = core.payments().any { it.optString("dir") == "in" && it.optString("receipt") == id }
                if (!known) core.remember("in", "payment", "", id, why)
                (if (known) "Already in this wallet" else "Accepted") + ": proven unspent at block $at under the tip the peers agree on, and it pays this wallet. Confirmations: at least $conf." +
                    (if (conf < 3) " For a large payment, check again after a few more blocks." else "") + if (why.isNotEmpty()) "\nPurpose: $why" else ""
            }
            state == "seen" || state == "proven" -> "Seen, not confirmed yet. It is in block $at, which only this wallet's node has shown so far. Check again in a few minutes, once the peers have reached that block."
            state == "other_address" -> "Refused: that box pays someone else."
            state == "spent" -> "This wallet was paid that box and has since spent it."
            state == "absent" || state == "expected" -> "Not on chain yet, or not a real receipt. A payment takes a block to confirm: try again shortly."
            else -> j?.optString("error")?.takeIf { it.isNotEmpty() } ?: r.lines().lastOrNull { it.isNotBlank() } ?: "no answer"
        }
    }
    if (scanning) { Scanner("Scan the payer's receipt", {
        val rc = Receipt.parse(it); receipt = rc.box; purpose = rc.purpose; scanning = false
        if (rc.tx.isNotEmpty()) {
            // an offline payment: keep the signed transaction; it is a promise until a sync proves it
            val r = core.promise(rc.tx)
            val j = try { JSONObject(r) } catch (e: Exception) { null }
            val box = j?.optJSONArray("promised")?.optString(0) ?: ""
            result = if (box.length != 64) (j?.optString("error")?.takeIf { it.isNotEmpty() } ?: "not a payment to this wallet") else {
                val erg = shown(j!!.optLong("nanoerg"), 9)
                val more = when (j.optInt("tokens")) { 0 -> ""; 1 -> " and a token"; else -> " and tokens" }
                core.remember("in", "$erg ERG$more", "", box, rc.purpose, state = "promised")
                "Promised: $erg ERG$more. Not on chain yet, so not yours yet: the payer could still spend these coins elsewhere. This $device will submit it on its next sync and add it once proven." + if (rc.purpose.isNotEmpty()) "\nPurpose: ${rc.purpose}" else ""
            }
        } else if (rc.box.length == 64) run(rc.box, rc.purpose)
        else result = "That code is not a receipt: a receipt is a box id (64 hex characters) or a tachys receipt link."
    }, { scanning = false }); return }
    CardBox {
        Text("Receive to", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        if (w.addresses.size > 1) Text((if (which == 0) "Main address" else "Address ${which + 1}") + " of ${w.addresses.size}  ▾  tap to change", color = Color.White, fontSize = 15.sp,
            modifier = Modifier.clickable { which = (which + 1) % w.addresses.size }.padding(vertical = 6.dp))
        Spacer(Modifier.height(10.dp))
        Qr(qr)
        Spacer(Modifier.height(10.dp))
        // the address as text, to select, and a button that puts it on the clipboard: for a faucet, an exchange, another app
        val on = LocalPlatform.current
        var copied by remember(shown) { mutableStateOf(false) }
        SelectionContainer { Text(shown, color = Color.White, fontSize = 14.sp) }
        Spacer(Modifier.height(10.dp))
        // on a line of its own, full width: beside the address the text took the whole row and left the button no room
        Pill(if (copied) "Copied to the clipboard" else "Copy address", if (copied) Bg else Accent, if (copied) Color.White else Color.Black, Modifier.fillMaxWidth()) { on.copy("Ergo address", shown); copied = true }
    }
    CardBox {
        Text("Have a receipt?", color = Color.White, fontSize = 18.sp, fontWeight = FontWeight.Bold)
        Text("The payer gives you a box id. It is accepted only if proven unspent, under a tip the peers agree on, and paying this address.", color = Dim, fontSize = 13.sp)
        Spacer(Modifier.height(8.dp))
        Field(receipt, "box id", onScan = { scanning = true }) { receipt = boxIdTyped(it); purpose = "" }
        Spacer(Modifier.height(10.dp))
        Pill(if (busy) "Checking…" else "Check receipt", Color.White, Color.Black, Modifier.fillMaxWidth()) { focus.clearFocus(); if (receipt.length == 64) run(receipt, purpose) }
        if (result.isNotEmpty()) { Spacer(Modifier.height(8.dp)); Text(result, color = Color.White, fontSize = 15.sp) }
    }
}

/** A small button beside a field ("Max"). */
@Composable
fun Small(text: String, onClick: () -> Unit) {
    Box(Modifier.height(44.dp).widthIn(min = 64.dp).border(1.dp, Dim, RoundedCornerShape(8.dp)).clickable { onClick() }.padding(horizontal = 12.dp), contentAlignment = Alignment.Center) {
        Text(text, color = Color.White, fontSize = 15.sp)
    }
}

/** One thing being sent: its name, an amount field with Max beside it, and what is available. `onRemove` puts an ✕ by the name. */
@Composable
fun AmountRow(name: String, value: String, available: String, over: Boolean, onMax: () -> Unit, onRemove: (() -> Unit)? = null, onChange: (String) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(name, color = Color.White, fontSize = 16.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        if (onRemove != null) Text("✕", color = Dim, fontSize = 18.sp, modifier = Modifier.clickable { onRemove() }.padding(horizontal = 10.dp, vertical = 4.dp))
    }
    Spacer(Modifier.height(4.dp))
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.weight(1f)) { Field(value, "amount", numeric = true) { onChange(it.filter { c -> c.isDigit() || c == '.' }) } }
        Small("Max", onMax)
    }
    Text(if (over) "More than the $available available" else "Available: $available", color = if (over) Accent else Dim, fontSize = 12.sp, modifier = Modifier.padding(top = 2.dp))
}

/** `send` takes: recipient, nanoERG (0 = only what the tokens' box must hold), the tokens with raw amounts, the token to pay the fee in (or null), and where to show the answer. */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
fun Send(core: Core, w: WalletView, busy: Boolean, asked: PayRequest, send: (String, Long, List<Pair<TokenLine, Long>>, TokenLine?, Long, (String) -> Unit) -> Unit) {
    var feeInToken by remember(w.dir) { mutableStateOf(false) }
    var offer by remember(w.dir) { mutableStateOf(-1L) } // nanoERG per token unit of the best offer; -1 = not asked yet
    val scope = rememberCoroutineScope()
    var to by remember(asked) { mutableStateOf(asked.address) }
    var erg by remember(asked) { mutableStateOf(asked.erg) }
    var purpose by remember(asked) { mutableStateOf(asked.note) }
    var alsoTokens by remember(asked) { mutableStateOf(asked.tokens) }
    // the tokens chosen to go with this payment, in the order chosen, and the amount typed for each
    val chosen = remember(asked, w.dir) { mutableStateListOf<String>() }
    val amounts = remember(asked, w.dir) { mutableStateMapOf<String, String>() }
    var adding by remember(w.dir) { mutableStateOf(false) }
    var result by remember { mutableStateOf("") }
    var receiptQr by remember { mutableStateOf(listOf<String>()) }
    var receipt by remember { mutableStateOf("") }
    var scanning by remember { mutableStateOf(false) }
    val focus = LocalFocusManager.current
    if (scanning) { Scanner("Scan the recipient's address", { payRequestFromCode(it).let { r -> to = r.address; if (r.erg.isNotEmpty()) erg = r.erg; if (r.note.isNotEmpty()) purpose = r.note; alsoTokens = r.tokens }; scanning = false }, { scanning = false }); return }
    fun label(t: TokenLine) = t.name + if (t.reported) "*" else ""
    fun raw(t: TokenLine): Long = units(amounts[t.id], t.decimals)
    val picked = chosen.mapNotNull { id -> w.tokens.firstOrNull { it.id == id } }
    val rest = w.tokens.filter { it.id !in chosen }
    val nano: Long = units(erg, 9)
    // What may leave: everything but the fee, and, if any token stays behind, the 0.001 ERG the box holding it must keep.
    val staying = w.tokens.any { t -> t.id !in chosen || raw(t) < t.raw }
    val spare = (w.nano - 1_100_000L - if (staying) 1_000_000L else 0L).coerceAtLeast(0L)
    val short = w.nano < 2_100_000L
    val feeToken = picked.firstOrNull()?.takeIf { short && feeInToken }
    // the fee switched to a token, or its price just learnt, with more of that token typed than the fee leaves: the amount
    // comes down to what can be sent, whichever was done first
    LaunchedEffect(offer, feeInToken, short) {
        val t = picked.firstOrNull()
        if (t != null && short && feeInToken && offer > 0 && raw(t) > leftAfterFee(t.raw, offer)) amounts[t.id] = shown(leftAfterFee(t.raw, offer), t.decimals)
    }
    CardBox {
        Text("Send from ${w.name}", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        if (w.readOnly) { Text("This wallet is read-only: it has an address but no key.", color = Color.White, fontSize = 16.sp); return@CardBox }
        Spacer(Modifier.height(10.dp))
        Field(to, "recipient address", onScan = { scanning = true }) { to = it.filter { c -> c.isLetterOrDigit() } }
        if (alsoTokens) Text("The scanned request also asks for tokens. They are not filled in here: add them below.", color = Color.White, fontSize = 13.sp, modifier = Modifier.padding(top = 6.dp))
        AmountRow("ERG", erg, w.erg + " ERG", nano > w.nano, onMax = { erg = shown(spare, 9) }) { erg = it }
        picked.forEach { t ->
            // of the token the fee is paid in, what the fee may cost is kept back: it is not there to send
            val kept = if (short && feeInToken && offer > 0 && t.id == picked.firstOrNull()?.id) t.raw - leftAfterFee(t.raw, offer) else 0L
            val free = t.raw - kept
            AmountRow(label(t), amounts[t.id] ?: "", if (kept > 0) shown(free, t.decimals) + " (" + shown(kept, t.decimals) + " kept for the fee)" else t.amount, raw(t) > free, onMax = { amounts[t.id] = shown(free, t.decimals) }, onRemove = { chosen.remove(t.id); amounts.remove(t.id); if (chosen.isEmpty()) feeInToken = false }) { amounts[t.id] = it }
        }
        if (picked.isNotEmpty() && nano == 0L) Text("Tokens travel in a box that must hold some ERG: 0.001 ERG goes with them.", color = Dim, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
        if (rest.isNotEmpty()) {
            Spacer(Modifier.height(12.dp))
            Pill(if (adding) "Choose a token  ▴" else if (picked.isEmpty()) "Add a token  ▾" else "Add another token  ▾", Bg, Color.White, Modifier.fillMaxWidth()) { focus.clearFocus(); adding = !adding }
            // the list opens in place, under the button, and closes when a token is taken from it
            if (adding) Column(Modifier.fillMaxWidth().padding(top = 4.dp).background(Bg, RoundedCornerShape(12.dp))) {
                rest.forEach { t ->
                    Row(Modifier.fillMaxWidth().clickable { chosen.add(t.id); adding = false }.padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(label(t), color = Color.White, fontSize = 16.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Text(t.amount, color = Dim, fontSize = 15.sp, modifier = Modifier.padding(start = 12.dp))
                    }
                }
            }
        }
        if (w.tokens.any { it.reported } && (picked.any { it.reported } || adding)) Text(REPORTED_NOTE, color = Dim, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
        // no ERG for the fee? pay it in a token being sent, from someone's standing offer (Babel fees).
        // Offered only when the wallet is short of ERG: with enough ERG the fee is simply paid in ERG.
        val first = picked.firstOrNull()
        if (short && first != null) {
            Spacer(Modifier.height(12.dp))
            Pill(if (feeInToken) "Fee: paid in ${first.name}  ▾" else "Fee: 0.0011 ERG  ▾", Bg, Color.White, Modifier.fillMaxWidth()) {
                feeInToken = !feeInToken
                // look up the price before anything is sent
                if (feeInToken) { offer = -1L; scope.launch { offer = withContext(background) { core.babelPrice(first.id) } } }
            }
            if (feeInToken) Text(when {
                offer == Core.UNPROVEN -> "The offers for ${first.name} could not be checked against the chain just now: the node and the peers were a block apart, or no node answered. Tap the fee button twice to ask again."
                offer < 0 -> "Looking for an offer…"
                offer == 0L -> "Nobody is offering ERG for ${first.name} right now, so the fee cannot be paid in it."
                // the ERG a payment is short of is at most the fee, the payee's box and the change box: 0.0031 ERG
                else -> "For a wallet without enough ERG: the missing ERG comes from an offer on the chain. Best offer: 1 ${first.name} buys " + shown(offer, 9 - first.decimals) + " ERG, so this costs at most " + shown(feeCeiling(offer), first.decimals) + " ${first.name}. Nothing is signed above that; the result shows the exact amount."
            }, color = Dim, fontSize = 12.sp)
        } else if (!short) Text("Network fee: 0.0011 ERG", color = Dim, fontSize = 12.sp, modifier = Modifier.padding(top = 10.dp))
        Spacer(Modifier.height(12.dp))
        Field(purpose, "purpose (optional)", words = true) { purpose = it.take(80) }
        Text("Like the memo line on a cheque. It travels with the receipt and is not written to the chain.", color = Dim, fontSize = 12.sp)
        Spacer(Modifier.height(12.dp))
        Pill(if (busy) "Sending…" else "Send", Accent, Color.Black, Modifier.fillMaxWidth()) {
            focus.clearFocus()
            val sending = picked.map { it to raw(it) }
            val what = (listOf(erg.takeIf { nano > 0 }?.let { "$it ERG" }) + sending.map { (t, _) -> "${amounts[t.id]} ${t.name}" }).filterNotNull().joinToString(" + ")
            when {
                to.isEmpty() -> result = "Enter or scan the recipient's address."
                nano == 0L && sending.isEmpty() -> result = "Enter an amount of ERG, or add a token."
                sending.any { it.second <= 0L } -> result = "Enter an amount for each token, or remove it with ✕."
                nano > w.nano || sending.any { (t, r) -> r > t.raw } -> result = "That is more than this wallet holds."
                feeToken != null && offer <= 0L -> result = "The fee's price in ${feeToken.name} is not known yet. Wait for it to show above, or switch the fee back to ERG."
                // the fee comes out of the same token: what is sent must leave what the fee may cost
                feeToken != null && sending.first { it.first.id == feeToken.id }.second > feeToken.raw - feeCeiling(offer) -> result =
                    if (feeToken.raw <= feeCeiling(offer)) "The fee may cost up to ${shown(feeCeiling(offer), feeToken.decimals)} ${feeToken.name}, and this wallet has ${feeToken.amount}: not enough to pay the fee in it."
                    else "The fee is paid in ${feeToken.name} too and may cost up to ${shown(feeCeiling(offer), feeToken.decimals)}. Send at most ${shown(feeToken.raw - feeCeiling(offer), feeToken.decimals)} ${feeToken.name}, so that enough is left for it."
                else -> send(to, nano, sending, feeToken, if (feeToken != null) feeCeiling(offer) else 0L) { r ->
                    val j = try { JSONObject(r) } catch (e: Exception) { null }
                    val state = j?.optString("state") ?: "refused"
                    val feeUnits = j?.optLong("fee_token_units", -1L) ?: -1L
                    val feeText = if (feeUnits >= 0 && feeToken != null) "Fee paid in tokens: " + shown(feeUnits, feeToken.decimals) + " ${feeToken.name}." else "Fee 0.0011 ERG."
                    val rid = j?.optString("receipt") ?: ""
                    val change = j?.optString("change")?.takeIf { it.length == 64 } ?: ""
                    if (state == "sent" || state == "queued" || state == "unsure") {
                        // offline, the code carries the signed payment itself (if it fits a QR code), so the payee can submit it too
                        val txHex = if (state == "queued") j?.optString("tx_bytes")?.takeIf { it.length in 2..2400 } ?: "" else ""
                        val rc = Receipt(rid, purpose.trim(), txHex)
                        result = when (state) {
                            "queued" -> "Signed as a promise: there is no network right now. It is under Pending and goes out on the next sync, from this $device or the payee's. Until it is in a block it is a promise, like a cheque, not a payment. Let the payee scan this:"
                            "unsure" -> "Signed $what, but the node did not answer, so it may or may not have gone out. It is under Pending: the next sync will send it again if it did not arrive, and say what became of it. Do not pay again until then. The payee's receipt, if it did go:"
                            else -> "Sent $what. $feeText Give the payee this receipt (you can find it again under Details):"
                        }
                        receipt = rid; receiptQr = core.qr(rc.code())
                        core.remember("out", what, to, rc.code(), rc.purpose, change, if (state == "queued") "queued" else "sent", j?.optString("tx") ?: "")
                    } else { result = j?.optString("error")?.takeIf { it.isNotEmpty() } ?: r.lines().lastOrNull { it.isNotBlank() } ?: "not sent"; receipt = ""; receiptQr = listOf() }
                }
            }
        }
    }
    // the answer appears under the form, which can be below the edge of a small screen: scroll to it
    val bring = remember { BringIntoViewRequester() }
    LaunchedEffect(result) { if (result.isNotEmpty()) { delay(150); bring.bringIntoView() } }
    if (result.isNotEmpty()) Box(Modifier.bringIntoViewRequester(bring)) { CardBox {
        Text(result, color = Color.White, fontSize = 16.sp)
        if (receipt.isNotEmpty()) { Spacer(Modifier.height(10.dp)); Qr(receiptQr); Spacer(Modifier.height(8.dp)); SelectionContainer { Text(receipt, color = Dim, fontSize = 12.sp) } }
    } }
}

/** While this is part of the screen, the screen cannot be captured or shown in the recent-apps list. */
@Composable
fun Secure() = LocalPlatform.current.Secure()

/** A seed phrase on screen, numbered. While it shows, the screen cannot be captured or shown in the recent-apps list. */
@Composable
fun SecretWords(phrase: String) {
    Secure()
    phrase.split(" ").filter { it.isNotEmpty() }.chunked(3).forEachIndexed { r, row ->
        Row(Modifier.fillMaxWidth().padding(vertical = 4.dp)) { row.forEachIndexed { i, word -> Text("${r * 3 + i + 1}. $word", color = Color.White, fontSize = 16.sp, modifier = Modifier.weight(1f)) } }
    }
}

/** How the seed phrase is kept on this phone, said where a phrase is about to be stored. */
@Composable
fun SealNote(guard: Guard) {
    Text(if (guard.secure()) "The seed phrase is kept encrypted on this $device and opens only with the $device's screen lock. If the screen lock is ever removed, the $device can no longer open it, and the written words are the only way back."
         else "This $device has no screen lock. The seed phrase is kept encrypted, but anyone holding the $device can use it. Set a screen lock and it will guard the wallet from then on.", color = Dim, fontSize = 13.sp)
}

/** Asks for the password of a wallet that has one of its own, over whatever screen wanted it. `done(null)`: not given. */
@Composable
fun PasswordPrompt(title: String, done: (String?) -> Unit) {
    var pass by remember { mutableStateOf("") }
    Dialog(onDismissRequest = { done(null) }) {
        Column(Modifier.fillMaxWidth().background(Card, RoundedCornerShape(12.dp)).padding(16.dp)) {
            Text(title, color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
            Text("This wallet has a password of its own.", color = Dim, fontSize = 14.sp)
            Spacer(Modifier.height(10.dp))
            Field(pass, "wallet password", secret = true) { pass = it }
            Spacer(Modifier.height(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Pill("Cancel", Color.White, Color.Black, Modifier.weight(1f)) { done(null) }
                Pill("Open", Accent, Color.Black, Modifier.weight(1f)) { if (pass.isNotBlank()) done(pass.trim()) }
            }
        }
    }
}

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
    CardBox {
        Text(if (w.addresses.size > 1) "Wallet addresses" else "Wallet address", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        w.addresses.forEachIndexed { i, a ->
            Spacer(Modifier.height(6.dp))
            if (w.addresses.size > 1) Text(if (i == 0) "Main address" else "Address ${i + 1}", color = Dim, fontSize = 13.sp)
            SelectionContainer { Text(a, color = Color.White, fontSize = 14.sp) }
        }
        if (!w.readOnly) {
            Spacer(Modifier.height(10.dp))
            Pill("Add an address", Bg, Color.White, Modifier.fillMaxWidth()) { guard.open("Add an address to ${w.name}", false) { pass, why -> if (pass == null) msg = why else { val r = core.addAddress(pass); msg = if (r.startsWith("address ")) "" else r.lines().lastOrNull { it.isNotBlank() } ?: "failed"; onChanged() } } }
            Text("Every address belongs to the same seed phrase. The balance counts them all; change returns to the main address.", color = Dim, fontSize = 12.sp)
        }
    }
    CardBox {
        Text("Descriptive wallet name", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Spacer(Modifier.height(8.dp))
        Field(name, "wallet name", words = true) { name = it.take(40) }
        Spacer(Modifier.height(10.dp))
        Pill("Save name", Bg, Color.White, Modifier.fillMaxWidth()) { core.rename(name); onChanged() }
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
        Text("tachys 0.0.1, a research build. The chain tip comes from peers and is checked for work; balances are proven, not reported. Box proofs, sending and token names use one node's REST port in this build. A token name marked \"as reported\" is what that node said, not something proven.", color = Dim, fontSize = 13.sp)
    }
}

/** The boxes a wallet's balance is made of, and the one thing to do with them: make them one. `consolidate` hands back where to show the answer. */
@Composable
fun Boxes(core: Core, w: WalletView, busy: Boolean, consolidate: ((String) -> Unit) -> Unit) {
    val boxes = remember(w) { core.boxes() }
    var result by remember { mutableStateOf("") }
    CardBox {
        Text("Boxes", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("Coins on Ergo sit in boxes: each payment to this wallet makes one, and a payment from it spends some and makes a new one for the change. Every box here was proven unspent against the peers' agreed tip. Many small boxes make every sync and payment larger; consolidating spends them all into one box back to this wallet, for one network fee (0.0011 ERG).", color = Dim, fontSize = 13.sp)
        Spacer(Modifier.height(10.dp))
        when {
            w.readOnly -> Text("A read-only wallet holds no key, so it cannot consolidate.", color = Color.White, fontSize = 14.sp)
            boxes.size < 2 -> Text(if (boxes.isEmpty()) "No boxes yet." else "One box: nothing to consolidate.", color = Color.White, fontSize = 14.sp)
            else -> Pill(if (busy) "Consolidating…" else "Consolidate ${minOf(boxes.size, 60)} boxes into one", Accent, Color.Black, Modifier.fillMaxWidth()) {
                consolidate { r ->
                    val j = try { JSONObject(r) } catch (e: Exception) { null }
                    result = when (j?.optString("state")) {
                        "sent" -> { core.remember("out", "${minOf(boxes.size, 60)} boxes into one", w.address, "", "consolidation", tx = j.optString("tx")); "Sent. The new box shows here once it is in a block; until then these boxes are held for it (see Pending)." }
                        "unsure" -> { core.remember("out", "${minOf(boxes.size, 60)} boxes into one", w.address, "", "consolidation", tx = j.optString("tx")); "Signed, but the node did not answer, so it may or may not have gone out. The next sync will settle it. Do not consolidate again until then." }
                        else -> j?.optString("error")?.takeIf { it.isNotEmpty() } ?: r.lines().lastOrNull { it.isNotBlank() } ?: "not sent"
                    }
                }
            }
        }
        if (result.isNotEmpty()) { Spacer(Modifier.height(8.dp)); Text(result, color = Color.White, fontSize = 14.sp) }
    }
    CardBox {
        Text(when (boxes.size) { 0 -> "No boxes"; 1 -> "1 box"; else -> "${boxes.size} boxes" }, color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        boxes.forEach { b ->
            Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
                Text(shown(b.nano, 9) + " ERG" + when (b.tokens) { 0 -> ""; 1 -> " · 1 token"; else -> " · ${b.tokens} tokens" }, color = Color.White, fontSize = 17.sp, fontWeight = FontWeight.Bold)
                Text(b.id.take(12) + "…" + b.id.takeLast(8) + "  ·  " + (if (b.confirmedAt > 0) "confirmed at block ${b.confirmedAt}" else "seen, not confirmed yet"), color = Dim, fontSize = 13.sp)
            }
        }
    }
}

@Composable
fun Pending(core: Core, w: WalletView) {
    val waiting = remember(w) { core.pending(w) }
    CardBox {
        Text("Pending payments", color = Accent, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text("\"Waiting for network\": signed on this $device, not sent yet; it goes out on the next sync that reaches the network. \"Waiting to confirm\": sent, not in a block yet.", color = Dim, fontSize = 13.sp)
        if (waiting.isEmpty()) { Spacer(Modifier.height(6.dp)); Text("Nothing pending. Every payment from this $device has confirmed.", color = Color.White, fontSize = 15.sp) }
        waiting.forEach { PaymentRow(core, w, it) }
    }
}
