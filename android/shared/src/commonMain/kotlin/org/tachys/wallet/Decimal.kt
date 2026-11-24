package org.tachys.wallet

// Amounts are whole numbers of the smallest unit (nanoERG, or a token's own) and are only ever shown or typed as
// decimals. These do that exactly, on the digits, with no floating point and nothing of any one platform; the tests
// hold each to what java.math.BigDecimal gives.

private fun digitsOf(units: String): Pair<Boolean, String> {
    val negative = units.startsWith("-")
    val digits = units.removePrefix("-").removePrefix("+")
    if (digits.isEmpty() || digits.any { it !in '0'..'9' }) throw NumberFormatException("not a whole number: $units")
    return negative to digits.trimStart('0')
}

/**
 * A whole number of units as a decimal with the point `decimals` places from the right and no needless zeros:
 * `shown("1500000000", 9)` is `1.5`. With `decimals` below zero the number is that many places larger.
 */
fun shown(units: String, decimals: Int): String {
    val (negative, digits) = digitsOf(units)
    if (digits.isEmpty()) return "0"
    val text = if (decimals <= 0) digits + "0".repeat(-decimals) else {
        val padded = digits.padStart(decimals + 1, '0')
        val whole = padded.dropLast(decimals)
        val part = padded.takeLast(decimals).trimEnd('0')
        if (part.isEmpty()) whole else "$whole.$part"
    }
    return if (negative) "-$text" else text
}
fun shown(units: Long, decimals: Int): String = shown(units.toString(), decimals)

/** The same with exactly `places` digits after the point, the rest cut off (never rounded up): a balance for show. */
fun shownFixed(units: Long, decimals: Int, places: Int): String {
    val (negative, digits) = digitsOf(units.toString())
    val padded = digits.padStart(decimals + 1, '0')
    val whole = padded.dropLast(decimals)
    val part = padded.takeLast(decimals).padEnd(places, '0').take(places)
    val text = if (places == 0) whole else "$whole.$part"
    return if (negative && (whole + part).any { it != '0' }) "-$text" else text
}

/** The digits before and after the point of a decimal as typed (`12`, `1.5`, `.5`, `5.`, with or without a sign), or null if it is not one. */
private fun typed(text: String): Triple<Boolean, String, String>? {
    val t = text.trim()
    val negative = t.startsWith("-")
    val body = t.removePrefix("-").removePrefix("+")
    val whole = body.substringBefore('.')
    val part = body.substringAfter('.', "")
    if (body.count { it == '.' } > 1 || (whole + part).isEmpty() || (whole + part).any { it !in '0'..'9' }) return null
    return Triple(negative, whole, part)
}

/**
 * A decimal as typed, in whole units with `decimals` places: digits beyond them are cut off, what is not a number
 * is 0, and a number too large to hold is `Long.MAX_VALUE` (which no wallet has, so it reads as "more than there is").
 */
fun units(text: String?, decimals: Int): Long {
    val (negative, whole, part) = typed(text ?: "") ?: return 0L
    val digits = (whole + part.padEnd(decimals, '0').take(decimals)).trimStart('0')
    if (digits.isEmpty()) return 0L
    val n = if (digits.length > 19) null else digits.toLongOrNull()
    return when { n == null -> Long.MAX_VALUE; negative -> -n; else -> n }
}

/** A decimal as typed that is more than nothing, however small. */
fun positive(text: String): Boolean = typed(text)?.let { (negative, whole, part) -> !negative && (whole + part).any { it != '0' } } ?: false
