package org.tachys.wallet

import org.json.JSONObject

// What the app makes of the core's answers. Nothing of Android is used here, so that the tests can run every reading
// on the build machine against the answers the core is held to (`vectors/app/`, which the core's own tests check it
// still gives). What the app decides a tip, a restore or a payment on is read here.

/** The core's answer to a `--json` command. `ok`: it ended with success and its object, in the layout this app reads, says so. */
class Reply(val code: Int, val json: JSONObject?, val text: String) {
    val ok get() = code == 0 && json != null && json.optBoolean("ok") && json.optInt("v") == 1
    /** Why it failed, in the core's words. */
    val error get() = json?.optString("error")?.takeIf { it.isNotEmpty() } ?: text.lines().lastOrNull { it.isNotBlank() } ?: "no answer"
    /** A password came with the command and does not open the seed phrase. */
    val badPassword get() = json?.optBoolean("bad_password") == true
}

/**
 * One run of the core with `--json`, as a Reply: the code it ended with, what it wrote to its output (its one JSON
 * object is the last line that starts one; null if there is none) and its sentences, which it wrote apart from it.
 */
fun reply(code: Int, out: String, said: String): Reply {
    val json = try { out.lines().lastOrNull { it.startsWith("{") }?.let { JSONObject(it) } } catch (e: Exception) { null }
    return Reply(code, json, said.trim())
}

/** `tips`: the peers agreed on a tip, and the chain was saved (the core exits 1 otherwise). */
fun tipAgreed(a: Reply): Boolean = a.ok && a.json?.optString("verdict") == "agreed"

/** `wallet derive`: the address the phrase leads to, or null. */
fun derivedAddress(a: Reply): String? = a.json?.optString("address")?.takeIf { it.isNotEmpty() }

/** `wallet restore`: the wallet file is written (its boxes are looked for by the next sync). */
fun restored(a: Reply): Boolean = a.json?.optString("restored")?.isNotEmpty() == true

/** A text field of an answer that succeeded, or null: `wallet pubkey` (`base58`), `wallet mnemonic` (`mnemonic`). */
fun textOf(a: Reply, field: String): String? = a.json?.optString(field)?.takeIf { a.ok && it.isNotEmpty() }

/**
 * What became of a payment or a signed request: "sent" (a node took it), "queued" (signed, waiting for network),
 * "unsure" (a node was sent it and did not answer: it may have gone, it is kept as waiting, and it must not be paid
 * again on a guess) or "refused".
 */
fun sentState(a: Reply): String {
    val j = a.json
    return when {
        j?.optBoolean("outcome_unknown") == true -> "unsure"
        a.ok && j?.optBoolean("signed") == true && (j.optJSONObject("broadcast")?.optBoolean("accepted") == true || j.optString("sent_via").isNotEmpty()) -> "sent"
        a.ok && j?.optBoolean("signed") == true && j.optString("parcel").isNotEmpty() -> "queued"
        else -> "refused"
    }
}

/**
 * The same as one object for the screens, every part of it from a field of the core's answer: `state` (`sentState`),
 * with `tx`, `receipt`, `change`, `fee_token_units` (-1: the fee was paid in ERG), `tx_bytes` and `error` (`why`, if
 * it was neither sent nor queued).
 */
fun sentAnswer(a: Reply, why: String): JSONObject {
    val j = a.json
    val state = sentState(a)
    return JSONObject().put("state", state).put("tx", j?.optString("tx") ?: "").put("receipt", j?.optJSONArray("receipt")?.optString(0) ?: "")
        .put("change", j?.optJSONArray("change")?.optString(0) ?: "").put("fee_token_units", j?.optLong("fee_token_units", -1L) ?: -1L)
        .put("tx_bytes", j?.optString("tx_bytes") ?: "").put("error", if (state == "sent" || state == "queued") "" else why)
}

/** The fewest characters a password typed for one wallet may have. */
const val PASSWORD_MIN = 8

/**
 * What is wrong with a password typed twice to seal one wallet, or null. The core reads a password as a line and
 * without the spaces around it, so that is how it is counted and compared here.
 */
fun passwordFault(first: String, again: String): String? = when {
    first.any { it == '\n' || it == '\r' } -> "A password is one line."
    first.trim().length < PASSWORD_MIN -> "A password for a wallet has at least $PASSWORD_MIN characters."
    first.trim() != again.trim() -> "The two passwords are not the same."
    else -> null
}
