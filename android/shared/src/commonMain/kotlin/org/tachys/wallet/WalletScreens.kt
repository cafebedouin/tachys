package org.tachys.wallet

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.material.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// One wallet looked into: its balance and payments (Detail, Pending), and the boxes its balance is made of (Boxes).

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
