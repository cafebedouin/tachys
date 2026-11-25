package org.tachys.wallet

import androidx.compose.ui.graphics.toAwtImage
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.getOrNull
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertCountEquals
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.hasSetTextAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Rule
import org.junit.Test

/**
 * The screens themselves, as a computer shows them, driven with no window and with the core loaded: so they need the
 * library (TACHYS_HOST_CORE) and are skipped without it, as NativeTest is. No network: the only peer listed is a
 * port of this machine that nothing listens on.
 */
class ScreensTest {
    @get:Rule val screen = createComposeRule()

    private class Offline(private val real: Computer) : Platform by real {
        override fun asset(name: String): ByteArray = if (name.startsWith("seeds-")) "127.0.0.1:1\n".toByteArray() else real.asset(name)
    }
    private fun core(name: String): Pair<Core, File> {
        assumeTrue("the core is not on java.library.path (TACHYS_HOST_CORE)", try { Native.run(listOf("version"), listOf()).code == 0 } catch (e: LinkageError) { false })
        val dir = File(System.getProperty("java.io.tmpdir"), "tachys-screens-${ProcessHandle.current().pid()}-$name").also { it.deleteRecursively() }
        return Core(Offline(Computer(dir))) to dir
    }
    private val numbered = Regex("\\d+\\. \\p{L}+")
    private fun words() = screen.onAllNodes(SemanticsMatcher("a numbered word") { n -> n.config.getOrNull(SemanticsProperties.Text)?.any { t -> numbered.matches(t.text) } == true })
    /** A sentence that names a phone. Not a word of the seed phrase: "phone" is one of the 2048, and a wallet made here has it about once in 170. */
    private fun saysPhone() = screen.onAllNodes(SemanticsMatcher("says phone, and is not a numbered word") { n -> n.config.getOrNull(SemanticsProperties.Text)?.any { t -> "phone" in t.text && !numbered.matches(t.text) } == true })
    private fun picture(name: String) { File("build/screens").mkdirs(); javax.imageio.ImageIO.write(screen.onRoot().captureToImage().toAwtImage(), "png", File("build/screens/$name.png")) }

    @Test fun onAComputerAWalletIsMadeWithAPasswordOfItsOwnAndOpenedOnlyByIt() {
        val (core, dir) = core("new")
        try {
            screen.setContent { App(core, "", noLock, false) { } }
            // the sentences name the machine they are on
            screen.onNodeWithText("Makes a new key on this computer and shows you its seed phrase to write down.").assertExists()
            saysPhone().assertCountEquals(0)
            screen.onNodeWithText("New wallet").performClick()
            saysPhone().assertCountEquals(0)
            // no screen lock to choose here: the password fields are simply there
            screen.onAllNodes(hasText("Guarded by")).assertCountEquals(0)
            screen.onAllNodes(hasText("Screen lock")).assertCountEquals(0)
            val fields = screen.onAllNodes(hasSetTextAction())
            fields.assertCountEquals(3)
            fields[0].performTextInput("on a computer")
            fields[1].performTextInput("a password of its own")
            fields[2].performTextInput("a password of its 0wn")
            screen.onNodeWithText("Create").performScrollTo().performClick()
            screen.onNodeWithText("The two passwords are not the same.").assertExists()
            assertTrue("nothing was made", core.dirs().isEmpty())
            picture("new-wallet-refused")
            screen.onNodeWithText("‹  Back").performScrollTo().performClick()
            screen.onNodeWithText("New wallet").performClick()
            // (the name typed is still there; the passwords are not)
            screen.onAllNodes(hasSetTextAction()).let { f -> f[1].performTextInput("a password of its own"); f[2].performTextInput("a password of its own") }
            screen.onNodeWithText("Create").performScrollTo().performClick()
            screen.onNodeWithText("Write these words down").assertExists()
            words().assertCountEquals(12)
            saysPhone().assertCountEquals(0)
            screen.onNodeWithText("I have written them down").performScrollTo().performClick()
            screen.onNodeWithText("on a computer").assertExists()
            picture("wallet-made")
            // on disk: one wallet, sealed, marked as having its own password
            val st = File(dir, core.dirs().single())
            assertTrue(File(st, Core.OWN).exists())
            assertFalse("sealed on disk", File(st, "wallet.json").readText().contains("\"mnemonic\""))
            assertTrue(core.own())
            assertFalse(core.opens("not the password")); assertTrue(core.opens("a password of its own"))
            // its words are asked for with the password, in a dialog; a wrong one is said to be wrong
            screen.onNodeWithText("⚙").performClick()
            screen.onNodeWithText("Display mnemonic").performScrollTo().performClick()
            screen.onNodeWithText("This wallet has a password of its own.").assertExists()
            screen.onAllNodes(hasSetTextAction()).let { f -> f[f.fetchSemanticsNodes().size - 1].performTextInput("not the password") }
            screen.onNodeWithText("Open").performClick()
            screen.onNodeWithText(Core.WRONG).assertExists()
            words().assertCountEquals(0)
            screen.onNodeWithText("Display mnemonic").performScrollTo().performClick()
            screen.onAllNodes(hasSetTextAction()).let { f -> f[f.fetchSemanticsNodes().size - 1].performTextInput("a password of its own") }
            screen.onNodeWithText("Open").performClick()
            words().assertCountEquals(12)
            assertEquals(1, core.dirs().size)
            // with its mark gone the wallet is still one with a password of its own here: a computer has no screen lock
            // to fall back to, so nothing may be asked of one (and Remove still asks for the password)
            assertTrue(File(st, Core.OWN).delete())
            assertTrue(core.own())
        } finally { dir.deleteRecursively() }
    }
}
