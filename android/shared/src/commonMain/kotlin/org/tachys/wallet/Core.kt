package org.tachys.wallet


// The wallet logic of the app, and what it hands the screens: wallets as folders, each command of the Rust core as a
// method (`Core`), and the app's own short list of payments. Nothing here draws.

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
                 val seen: Long = 0L,
                 /** Boxes promised to this wallet whose payment can never confirm (a coin it spends is gone, or it was refused for good). */
                 val void: List<String> = listOf())

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
    fun own(dir: String = sel): Boolean = !on.lock || FileAt(path(dir), OWN).exists() // (where there is no screen lock every wallet is one, marked or not: nothing else could guard it)
    /** Whether the mark is now as asked. */
    private fun mark(dir: String, own: Boolean): Boolean = FileAt(path(dir), OWN).let { f -> try { if (own) f.writeText("") else f.delete(); f.exists() == own } catch (e: Exception) { false } }
    /** Why the core did not do what was asked, in its words; a password that does not open the phrase is recognised by a field of its answer, not by those words. */
    private fun failed(a: Reply, own: Boolean = own()): String = if (!a.badPassword) a.error else if (own) WRONG else lost(on.device)

    fun hasWallet() = dirs().isNotEmpty()
    /** Each of these answers `OK`, or the reason it did not happen. */
    fun watch(address: String, name: String): String {
        if (views().any { address in it.addresses }) return "that address is already a wallet on this ${on.device}"
        val d = fresh(); seedChain(d)
        val a = ask(listOf(), "wallet", "watch", "--state", path(d), "--address", address, "--name", name.ifBlank { "Wallet" })
        if (a.ok) sel = d else FileAt(path(d)).deleteRecursively()
        return if (a.ok) OK else a.error
    }
    /** `own`: `pass` is a password typed for this wallet, not the Vault's. */
    fun create(name: String, pass: String, own: Boolean = false): String {
        val d = fresh(); seedChain(d)
        val a = sealedAsk(pass, "wallet", "new", "--state", path(d), "--name", name.ifBlank { "Wallet" })
        // a wallet whose guard could not be recorded would be asked for the wrong secret from then on: it is not kept
        if (a.ok && !mark(d, own)) { FileAt(path(d)).deleteRecursively(); return "could not record how this wallet is guarded, so it was not kept" }
        if (a.ok) sel = d else FileAt(path(d)).deleteRecursively()
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
        if (same != null && !same.readOnly && !closed) return "that wallet is already on this ${on.device} (${same.name})"
        val d = same?.dir ?: fresh()
        if (same == null) seedChain(d)
        // the wallet file is written first and its boxes looked for after (by the next sync); `restored` says the file is there
        val a = ask(listOf(pass, line), "wallet", "restore", "--state", path(d), "--mnemonic-file", "-", "--embed", "--name", name.ifBlank { "Wallet" }, *(if (closed) arrayOf("--reseal") else arrayOf()), "--pass-stdin")
        val done = restored(a)
        if (done && !mark(d, own)) { if (same == null) FileAt(path(d)).deleteRecursively(); return "could not record how this wallet is guarded" + if (same == null) ", so it was not kept" else "; restore it again" }
        // sealed again over a wallet that was here: the mark says what guards it only if the new password does open it
        if (done && closed && !opens(pass, d)) return "the wallet was not sealed again with this password; it is as it was"
        if (done) sel = d else if (same == null) FileAt(path(d)).deleteRecursively()
        return if (done) OK else failed(a, own)
    }
    fun rename(name: String) = run("wallet", "name", "--state", state, "--name", name.ifBlank { "Wallet" })
    fun addAddress(pass: String) = sealed(pass, "wallet", "address", "--state", state, "--add")
    /** The extended public key, or "! " and the reason there is none. */
    fun pubkey(pass: String): String = sealedAsk(pass, "wallet", "pubkey", "--state", state).let { a -> textOf(a, "base58") ?: ("! " + failed(a)) }
    /** The seed phrase, or "! " and the reason it cannot be shown. */
    fun mnemonic(pass: String): String = sealedAsk(pass, "wallet", "mnemonic", "--state", state).let { a -> textOf(a, "mnemonic") ?: ("! " + failed(a)) }
    /** Whether this password opens the wallet's seed phrase. */
    fun opens(pass: String, dir: String = sel): Boolean = textOf(sealedAsk(pass, "wallet", "mnemonic", "--state", path(dir)), "mnemonic") != null
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

    /** Where a payment stands: signed but not sent, sent but not yet in a block, or done (`paymentStatus`). */
    fun status(p: JSONObject, w: WalletView): String = paymentStatus(p, w)
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
        if (version > WALLET_FILE_VERSION) "A wallet on this ${on.device} ($d) was written by a newer version of tachys (wallet file version $version; this app reads up to $WALLET_FILE_VERSION). It is not shown, and it has not been touched. Update the app to use it."
        else "A wallet file on this ${on.device} ($d) cannot be read: it is damaged or cut short. It is not shown, and it has not been touched. It may still hold a seed phrase."
    }
    fun wallet(): WalletView? = view(sel)
    /** A wallet as its file has it (`walletView`), with the names of its tokens; null if the file is not one this app reads. */
    fun view(dir: String): WalletView? = try { walletView(FileAt(path(dir), "wallet.json").readText(), dir, shipped, reported(dir)) } catch (e: Exception) { null }
    /** The core's own version, as it says it (`tachys version`); empty if it could not be asked. */
    val version: String by lazy { ask(listOf(), "version").let { a -> if (a.ok) a.json?.optString("version") ?: "" else "" } }
    companion object {
        /** What a command that did what it was asked answers. */
        const val OK = "ok"
        /** The file that marks a wallet sealed with a password of its own. The core does not read it. */
        /** `babelPrice`: the offers could not be checked against the agreed tip; asking again may find one. */
        const val UNPROVEN = -2L
        const val OWN = "own-password"
        const val WRONG = "That is not this wallet's password."
        fun lost(device: String) = "This wallet's seed phrase can no longer be opened on this $device: the key that guarded it is gone, which happens when the $device's screen lock is removed. Restore the wallet from its written seed phrase."
        val LOST get() = lost(device)
    }
}
