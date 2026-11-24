package org.tachys.wallet

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

/**
 * The app's wallet logic (`Core`) with the core loaded and no screen drawn: wallets made, named, sealed and read back
 * from the files the core writes. Needs the library (TACHYS_HOST_CORE), and is skipped without it, as NativeTest is.
 * No network: nothing here asks for a tip. Files only under the temporary directory.
 */
class CoreTest {
    private fun core(name: String): Pair<Core, File> {
        assumeTrue("the core is not on java.library.path (TACHYS_HOST_CORE)", try { Native.run(listOf("version"), listOf()).code == 0 } catch (e: LinkageError) { false })
        val dir = File(System.getProperty("java.io.tmpdir"), "tachys-core-${ProcessHandle.current().pid()}-$name").also { it.deleteRecursively() }
        return Core(Computer(dir)) to dir
    }
    private val pass = "a password of its own"

    @Test fun theAppSaysTheVersionTheCoreSays() {
        val (core, dir) = core("version")
        try {
            assertTrue(core.version, Regex("\\d+\\.\\d+\\.\\d+").matches(core.version))
            assertTrue(Native.run(listOf("version"), listOf()).let { it.out + it.err }.contains("tachys ${core.version} "))
        } finally { dir.deleteRecursively() }
    }

    @Test fun aWalletIsMadeSealedNamedAndGivenAnAddressAndReadBackFromItsFile() {
        val (core, dir) = core("made")
        try {
            assertFalse(core.hasWallet())
            assertEquals(Core.OK, core.create("first", pass, own = true))
            val w = core.wallet()!!
            assertEquals("first", w.name); assertFalse(w.readOnly); assertEquals(listOf(w.address), w.addresses)
            assertEquals(0L, w.nano); assertEquals("0.000000", w.erg); assertEquals(0, w.boxes)
            // sealed from the start: nothing in plain text, the words open with the password and only with it
            assertEquals(listOf<String>(), core.plain())
            assertEquals(12, core.mnemonic(pass).split(" ").size)
            assertEquals("! " + Core.WRONG, core.mnemonic("not the password"))
            assertTrue(core.opens(pass)); assertFalse(core.opens("not the password"))
            // a name and an address are written to the file, and read back from it
            core.rename("renamed")
            assertTrue(core.addAddress(pass).startsWith("address 1: "))
            core.wallet()!!.let { assertEquals("renamed", it.name); assertEquals(2, it.addresses.size); assertEquals(w.address, it.addresses[0]) }
            assertEquals(Core.WRONG, core.addAddress("not the password").lines().last { it.isNotBlank() })
            assertEquals(2, core.wallet()!!.addresses.size)
        } finally { dir.deleteRecursively() }
    }

    @Test fun aWalletAlreadyHereIsNotAddedAgainAndTheSentenceNamesThisMachine() {
        val (core, dir) = core("twice")
        try {
            assertEquals(Core.OK, core.create("first", pass, own = true))
            val w = core.wallet()!!
            val words = core.mnemonic(pass)
            // asked before any screen is drawn: the machine's name is the platform's, not the one the screens are told
            assertEquals("that address is already a wallet on this computer", core.watch(w.address, "again"))
            assertEquals("that wallet is already on this computer (first)", core.restore(words, "again", pass, own = true))
            assertEquals(1, core.views().size)
            // a phrase with a word wrong is said to be so, and makes no wallet
            val slip = words.split(" ").toMutableList().also { it[3] = "notaword" }.joinToString(" ")
            assertTrue(core.restore(slip, "slip", pass, own = true).let { it != Core.OK && it.isNotEmpty() })
            assertTrue(core.restore("one two three", "short", pass, own = true).startsWith("a seed phrase has 12, 15, 18, 21 or 24 words"))
            assertEquals(1, core.views().size)
        } finally { dir.deleteRecursively() }
    }

    @Test fun aReadOnlyWalletBecomesAFullOneWhenItsPhraseIsRestored() {
        val (core, dir) = core("watch")
        try {
            assertEquals(Core.OK, core.create("made", pass, own = true))
            val (address, words) = core.wallet()!!.address to core.mnemonic(pass)
            core.remove()
            assertFalse(core.hasWallet())
            assertEquals(Core.OK, core.watch(address, "watched"))
            core.wallet()!!.let { assertTrue(it.readOnly); assertEquals(address, it.address) }
            assertTrue(core.mnemonic(pass).startsWith("! "))
            assertEquals(Core.OK, core.restore(words, "whole again", pass, own = true))
            assertEquals(1, core.views().size)
            core.wallet()!!.let { assertFalse(it.readOnly); assertEquals(address, it.address) }
            assertEquals(words, core.mnemonic(pass))
        } finally { dir.deleteRecursively() }
    }

    @Test fun seedPhrasesLeftInPlainTextAreSealedAndTheWalletIsOtherwiseAsItWas() {
        val (core, dir) = core("plain")
        try {
            // a wallet as a build from before phrases were sealed left it: made by the core with no password
            val old = File(dir, "st-mainnet-1").also { it.mkdirs() }
            assertEquals(0, Native.run(listOf("wallet", "new", "--state", old.path, "--name", "old"), listOf()).code)
            val before = JSONObject(File(old, "wallet.json").readText())
            assertTrue(before.has("mnemonic"))
            assertEquals(listOf("st-mainnet-1"), core.plain())
            assertEquals(0, core.sealAll(pass))
            assertEquals(listOf<String>(), core.plain())
            val after = JSONObject(File(old, "wallet.json").readText())
            assertFalse(after.has("mnemonic")); assertNotNull(after.optJSONObject("secret"))
            assertEquals(before.getString("address"), after.getString("address")); assertEquals("old", after.getString("name"))
            // and the words it opens to are the words that were there
            assertEquals(before.getString("mnemonic"), core.mnemonic(pass))
        } finally { dir.deleteRecursively() }
    }
}
