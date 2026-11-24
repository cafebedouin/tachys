package org.tachys.wallet

import androidx.compose.foundation.layout.*
import androidx.compose.material.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// The Sign screen: a signing request from another site, said in the wallet's own words, and signed if told to.

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
