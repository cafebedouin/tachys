package org.tachys.wallet

// What the screens show of a wallet is read from its file, as the core's last sync left it: the balance on a wallet's
// card, its tokens, and the most the Send screen lets be sent. It is read here and nowhere else, from the file's text
// alone, so that it can be held to wallet files the core writes (`ViewTest`).

/** Two amounts added, held at the most a Long holds and never wrapped round, as the core adds them (`saturating_add`). */
fun added(a: Long, b: Long): Long {
    val sum = a + b
    return if (((a xor sum) and (b xor sum)) < 0L) (if (a >= 0L) Long.MAX_VALUE else Long.MIN_VALUE) else sum
}

/**
 * A wallet file as the screens show it. The balance and the tokens count only boxes confirmed under a tip the peers
 * agree on; a box only one node has shown is counted apart (`seen`). `shipped` are the token names the app carries,
 * `reported` the ones a node gave. Throws if the text is not a wallet file, or is one a newer build wrote.
 */
fun walletView(text: String, dir: String, shipped: Map<String, Pair<String, Int>>, reported: Map<String, Pair<String, Int>>): WalletView {
    val w = JSONObject(text)
    if (w.optInt("version", 0) > WALLET_FILE_VERSION) throw IllegalStateException("newer wallet file")
    val boxes = w.getJSONObject("boxes")
    var nano = 0L; var proven = 0; var seen = 0L
    val sums = LinkedHashMap<String, Long>()
    for (k in boxes.keys()) {
        val b = boxes.getJSONObject(k)
        if (b.isNull("confirmed_at")) { seen = added(seen, b.getLong("value")); continue }
        nano = added(nano, b.getLong("value")); proven = maxOf(proven, b.optInt("proven_at"))
        val a = b.optJSONArray("assets") ?: JSONArray()
        for (i in 0 until a.length()) a.getJSONObject(i).let { sums[it.getString("id")] = added(sums[it.getString("id")] ?: 0L, it.getLong("amount")) }
    }
    val tokens = sums.map { (id, amt) ->
        val s = shipped[id]; val r = reported[id]
        when {
            s != null -> TokenLine(id, s.first, s.second, amt, false)
            r != null && r.first.isNotEmpty() -> TokenLine(id, r.first, r.second, amt, true)
            else -> TokenLine(id, id.take(10) + "…", 0, amt, true)
        }
    }.sortedBy { it.reported }
    fun list(name: String) = (w.optJSONArray(name) ?: JSONArray()).let { a -> (0 until a.length()).map { a.getString(it) } }
    return WalletView(w.optString("name", "Wallet"), w.getString("address"),
        shownFixed(nano, 9, 6), boxes.length(), tokens, proven,
        w.optBoolean("watch", false), list("expect"), dir,
        list("address_list").takeIf { it.isNotEmpty() } ?: listOf(w.getString("address")),
        ((w.optJSONArray("pending") ?: JSONArray()).let { a -> (0 until a.length()).map { a.getJSONObject(it).optString("tx") to ("waiting" to 0) } } +
         (w.optJSONArray("done") ?: JSONArray()).let { a -> (0 until a.length()).map { a.getJSONObject(it).let { d -> d.optString("tx") to (d.optString("result") to d.optInt("seen_at")) } } }).toMap(), nano, seen, list("void"))
}

/**
 * Where a payment the app remembers stands, for a wallet as its file now has it: signed but not sent, sent but not yet
 * in a block, or done. `p` is one entry of the app's own list (`Core.remember`).
 */
fun paymentStatus(p: JSONObject, w: WalletView): String = when {
    // a payment held for this wallet that can never confirm is not one received: the core marks its box void, and
    // takes it off the boxes expected, which alone used to read here as "Received"
    p.optString("dir", "out") != "out" -> when {
        p.optString("state") != "promised" -> "Received"
        p.optString("receipt") in w.void -> "Void: its coins were spent elsewhere, so it was never paid"
        p.optString("receipt") in w.expecting -> "Promised, not on chain yet"
        else -> "Received"
    }
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
