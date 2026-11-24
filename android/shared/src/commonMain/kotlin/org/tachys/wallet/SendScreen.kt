package org.tachys.wallet

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import kotlinx.coroutines.delay
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.withContext

// The Send screen: an address, ERG and tokens, the fee in ERG or in a token, and what became of the payment.

/** The most ERG a payment from this app can be short of: the fee, the payee's box and the change box. */
const val FEE_SHORT_MAX = 3_100_000L

/** The most token units a fee paid in a token can cost at `price` nanoERG per unit (rounded up, and safe for any price). */
fun feeCeiling(price: Long): Long = if (price <= 0L) 0L else (FEE_SHORT_MAX - 1) / price + 1

/** Of `held` units of a token, what can be sent when the fee is paid in it at `price` nanoERG a unit: the rest is kept for the fee. */
fun leftAfterFee(held: Long, price: Long): Long = (held - feeCeiling(price)).coerceAtLeast(0L)

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
    // (of the wallet that paid: gone when another is chosen to pay from)
    var result by remember(w.dir) { mutableStateOf("") }
    var receiptQr by remember(w.dir) { mutableStateOf(listOf<String>()) }
    var receipt by remember(w.dir) { mutableStateOf("") }
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
    // the price is the price of the token the fee is paid in: asked for when the fee is switched to a token, and again
    // if that token becomes another (the first one taken off the list with another still on it)
    val feeId = feeToken?.id
    LaunchedEffect(feeId) {
        offer = -1L
        if (feeId != null) offer = withContext(background) { core.babelPrice(feeId) }
    }
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
