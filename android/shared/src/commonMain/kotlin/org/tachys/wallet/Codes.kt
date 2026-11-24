package org.tachys.wallet

// What a scanned or pasted code means, and the small pieces of a link's coding the receipts use. Nothing of any one
// platform is used here: a receipt made on one must read on another, and on a build from before the screens were shared
// (which coded these with Android's own Uri and Base64; the tests hold the same bytes).

/** What a scanned code means for a field that wants an address: plain, or an `ergo:` payment link. */
fun addressFromCode(code: String): String = payRequestFromCode(code).address

/** What a scanned code asks to be paid: an address, and perhaps an amount of ERG and a note. `tokens` = the request also names tokens. */
class PayRequest(val address: String, val erg: String = "", val note: String = "", val tokens: Boolean = false)

/**
 * Reads a bare address, an `ergo:ADDRESS?amount=…` link, or the payment request other Ergo wallets show
 * (`https://explorer.ergoplatform.com/payment-request?address=…&amount=…&description=…`, tokens as `token-ID=…`).
 */
fun payRequestFromCode(code: String): PayRequest {
    val c = code.trim()
    val query = c.substringAfter('?', "")
    val params = query.split('&').filter { it.contains('=') }.associate { it.substringBefore('=') to percentDecoded(it.substringAfter('=')) }
    val address = (if (c.startsWith("http://") || c.startsWith("https://")) params["address"] ?: "" else c.removePrefix("ergo:").substringBefore('?')).filter { it.isLetterOrDigit() }
    val erg = (params["amount"] ?: "").filter { it.isDigit() || it == '.' }.takeIf { positive(it) } ?: ""
    return PayRequest(address, erg, (params["description"] ?: "").take(80), params.keys.any { it.startsWith("token-") })
}

/** What a scanned code means for a field that wants a box id: the code itself if it is one (64 hex characters, either
 *  case), and nothing otherwise. A receipt's link form is read by `Receipt.parse`, which gives this its `box`. */
fun boxIdFromCode(code: String): String = code.trim().lowercase().takeIf { it.length == 64 && it.all { c -> c in '0'..'9' || c in 'a'..'f' } } ?: ""

/** What is typed or pasted into a box-id field, as it is typed: its hex characters, at most 64. */
fun boxIdTyped(text: String): String = text.lowercase().filter { it in '0'..'9' || it in 'a'..'f' }.take(64)

/**
 * A link's value with everything but letters, digits and `_-!.~'()*` written as %XX of its UTF-8 bytes (a space is
 * %20): what a receipt's purpose is carried as.
 */
fun percentEncoded(text: String): String = buildString {
    for (b in text.encodeToByteArray()) {
        val c = (b.toInt() and 0xff).toChar()
        if (c in 'A'..'Z' || c in 'a'..'z' || c in '0'..'9' || c in "_-!.~'()*") append(c) else append('%').append(hex2(b).uppercase())
    }
}

/** The reverse, as a link's query is read: %XX are bytes of UTF-8, `+` is a space, and a % that starts no byte stays as it is. */
fun percentDecoded(text: String): String {
    val out = ArrayList<Byte>()
    var i = 0
    fun hex(at: Int) = at < text.length && (text[at] in '0'..'9' || text[at] in 'a'..'f' || text[at] in 'A'..'F')
    while (i < text.length) {
        val c = text[i]
        when {
            c == '%' && hex(i + 1) && hex(i + 2) -> { out.add(text.substring(i + 1, i + 3).toInt(16).toByte()); i += 3 }
            c == '+' -> { out.add(' '.code.toByte()); i++ }
            else -> { out.addAll(c.toString().encodeToByteArray().asList()); i++ }
        }
    }
    return out.toByteArray().decodeToString()
}

/** A byte as two hex digits, lower case. */
fun hex2(b: Byte): String = (b.toInt() and 0xff).toString(16).padStart(2, '0')

/** The parameters after a link's `?`, each decoded; of a name given twice, the first. */
fun queryOf(link: String): Map<String, String> {
    val found = LinkedHashMap<String, String>()
    link.substringAfter('?', "").substringBefore('#').split('&').filter { it.contains('=') }.forEach { part -> found.getOrPut(percentDecoded(part.substringBefore('='))) { percentDecoded(part.substringAfter('=')) } }
    return found
}

private const val B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"

/** Bytes as base64 in the alphabet for links (`-` and `_`), on one line, with no padding. */
fun base64Url(bytes: ByteArray): String = buildString {
    var i = 0
    while (i < bytes.size) {
        val n = minOf(3, bytes.size - i)
        var v = 0
        for (k in 0 until 3) v = (v shl 8) or (if (k < n) bytes[i + k].toInt() and 0xff else 0)
        append(B64[(v shr 18) and 63]).append(B64[(v shr 12) and 63])
        if (n > 1) append(B64[(v shr 6) and 63])
        if (n > 2) append(B64[v and 63])
        i += 3
    }
}

/** The reverse, padded or not; null if it is not base64 of that alphabet. */
fun base64UrlBytes(text: String): ByteArray? {
    val t = text.trim().trimEnd('=')
    if (t.length % 4 == 1 || t.any { it !in B64 }) return null
    val out = ArrayList<Byte>()
    var i = 0
    while (i < t.length) {
        val n = minOf(4, t.length - i)
        var v = 0
        for (k in 0 until 4) v = (v shl 6) or (if (k < n) B64.indexOf(t[i + k]) else 0)
        out.add((v shr 16).toByte())
        if (n > 2) out.add((v shr 8).toByte())
        if (n > 3) out.add(v.toByte())
        i += 4
    }
    return out.toByteArray()
}
