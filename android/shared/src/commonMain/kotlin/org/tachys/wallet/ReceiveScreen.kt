package org.tachys.wallet

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// The Receive screen: the wallet's address as a code, and a payer's receipt checked or a signed payment kept as a promise.

@Composable
fun Receive(core: Core, w: WalletView, busy: Boolean, check: (String, (String) -> Unit) -> Unit) {
    var which by remember(w.dir) { mutableStateOf(0) }
    val shown = w.addresses.getOrElse(which) { w.address }
    val qr = remember(shown) { core.qr(shown) }
    var receipt by remember { mutableStateOf("") }
    var purpose by remember { mutableStateOf("") }
    var result by remember { mutableStateOf("") }
    var scanning by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
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
        if (rc.tx.isNotEmpty()) scope.launch {
            // an offline payment: keep the signed transaction; it is a promise until a sync proves it
            // (it writes the wallet file, so it waits its turn behind a sync of this wallet: off the thread that draws)
            val r = withContext(background) { core.promise(rc.tx) }
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
