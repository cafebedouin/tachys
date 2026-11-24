package org.tachys.wallet

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
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

// The pieces the screens are made of: the top bar, a card, a button, a field, a QR code, a wallet's card, the words
// of a seed phrase and the prompt for a password.

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

/** A small button beside a field ("Max"). */
@Composable
fun Small(text: String, onClick: () -> Unit) {
    Box(Modifier.height(44.dp).widthIn(min = 64.dp).border(1.dp, Dim, RoundedCornerShape(8.dp)).clickable { onClick() }.padding(horizontal = 12.dp), contentAlignment = Alignment.Center) {
        Text(text, color = Color.White, fontSize = 15.sp)
    }
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
