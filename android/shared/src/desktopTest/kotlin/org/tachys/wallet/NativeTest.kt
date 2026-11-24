package org.tachys.wallet

import java.io.File
import java.nio.ByteBuffer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

/**
 * The door to the core. How lists and answers cross is tested always. The core itself is called, through the same JNI
 * entry the phone uses, when the build machine has the library and says where (`docs/android.md`):
 *
 *     cargo build -p tachys-c --features jni
 *     TACHYS_HOST_CORE=$PWD/target/debug ./gradlew testDebugUnitTest      (from android/)
 *
 * Without `TACHYS_HOST_CORE` those tests are skipped, not passed. No network; files only under the temporary directory.
 */
class NativeTest {
    private fun answer(code: Int, out: String, err: String): ByteArray = out.toByteArray().let { o -> ByteBuffer.allocate(8 + o.size + err.toByteArray().size).putInt(code).putInt(o.size).put(o).put(err.toByteArray()).array() }

    @Test fun aListCrossesAsItsStringsEachEndedByAZeroByte() {
        assertEquals(0, packed(listOf()).size)
        assertEquals(listOf<Byte>(0), packed(listOf("")).toList())
        assertEquals("tachys\u0000qr\u0000\u0000".toByteArray().toList(), packed(listOf("tachys", "qr", "")).toList())
        // UTF-8 as it is, not the JNI's own: a character outside the first plane is four bytes
        assertEquals(listOf(0xF0, 0x9F, 0x94, 0x91, 0).map { it.toByte() }, packed(listOf("🔑")).toList())
        assertTrue(try { packed(listOf("two\u0000arguments")); false } catch (e: IllegalArgumentException) { true })
    }

    @Test fun anAnswerCrossesBackAsItsCodeItsOutputAndItsSentences() {
        val a = unpacked(answer(1, "{\"ok\":false}\n", "the reason\n"))
        assertEquals(Triple(1, "{\"ok\":false}\n", "the reason\n"), Triple(a.code, a.out, a.err))
        val b = unpacked(answer(0, "", "合言葉"))
        assertEquals(Triple(0, "", "合言葉"), Triple(b.code, b.out, b.err))
        // what is not an answer is no answer, never a guess at one
        assertEquals(-1, unpacked(null).code)
        assertEquals(-1, unpacked(ByteArray(7)).code)
        assertEquals(-1, unpacked(ByteBuffer.allocate(9).putInt(0).putInt(2).array()).code)
        assertEquals(-1, unpacked(ByteBuffer.allocate(8).putInt(0).putInt(-1).array()).code)
        assertFalse(reply(-1, "", unpacked(null).err).ok)
    }

    private fun withCore() = assumeTrue("TACHYS_HOST_CORE is not set: the core is not called", System.getenv("TACHYS_HOST_CORE") != null)
    private fun dir(name: String) = File(System.getProperty("java.io.tmpdir"), "tachys-app-${ProcessHandle.current().pid()}-$name").also { it.deleteRecursively(); it.mkdirs() }
    private fun ask(input: List<String>, vararg args: String): Reply = Native.run(args.toList() + "--json", input).let { reply(it.code, it.out, it.err) }

    @Test fun theCoreAnswersInThisProcess() {
        withCore()
        val v = ask(listOf(), "version", "--network", "testnet")
        assertTrue(v.ok); assertEquals("testnet", v.json!!.getString("network"))
        // as the app draws a QR code: rows of 0 and 1, with no --json
        val rows = Native.run(listOf("qr", "an address"), listOf()).let { it.out + it.err }.lines().filter { it.isNotEmpty() && it.all { c -> c == '0' || c == '1' } }
        assertTrue(rows.size > 20 && rows.all { it.length == rows.size })
        // misuse is an answer, and the process goes on
        val bad = ask(listOf(), "wallet", "bogus")
        assertEquals(2, bad.code); assertFalse(bad.ok)
        assertTrue(ask(listOf(), "version").ok)
    }

    @Test fun aPasswordAndAPhraseCrossAsLinesAndAWrongPasswordIsSaidInAField() {
        withCore()
        val (st, other) = dir("sealed") to dir("restored")
        try {
            assertTrue(ask(listOf("合言葉 one"), "wallet", "new", "--state", st.path, "--pass-stdin").ok)
            assertFalse("sealed on disk", File(st, "wallet.json").readText().contains("\"mnemonic\""))
            val wrong = ask(listOf("wrong"), "wallet", "mnemonic", "--state", st.path, "--pass-stdin")
            assertTrue(wrong.badPassword); assertFalse(wrong.ok)
            val phrase = textOf(ask(listOf("合言葉 one"), "wallet", "mnemonic", "--state", st.path, "--pass-stdin"), "mnemonic")!!
            assertEquals(12, phrase.split(" ").size)
            // as the app restores: the phrase alone to derive, then the password and the phrase as two lines
            val address = derivedAddress(ask(listOf(phrase), "wallet", "derive", "--mnemonic-file", "-"))!!
            assertEquals(JSONObject(File(st, "wallet.json").readText()).getString("address"), address)
            assertTrue(restored(ask(listOf("another", phrase), "wallet", "restore", "--state", other.path, "--mnemonic-file", "-", "--embed", "--pass-stdin")))
            assertEquals(phrase, textOf(ask(listOf("another"), "wallet", "mnemonic", "--state", other.path, "--pass-stdin"), "mnemonic"))
        } finally { st.deleteRecursively(); other.deleteRecursively() }
    }

    @Test fun callsFromSeveralThreadsEachGetTheirOwnNetworkAndAnswer() {
        withCore()
        val got = java.util.concurrent.ConcurrentHashMap<Int, List<String>>()
        val threads = (0 until 6).map { i -> Thread { got[i] = (0 until 10).map { ask(listOf(), "version", "--network", if (i % 2 == 0) "mainnet" else "testnet").json?.optString("network") ?: "none" } }.also { it.start() } }
        threads.forEach { it.join() }
        for (i in 0 until 6) assertEquals(List(10) { if (i % 2 == 0) "mainnet" else "testnet" }, got[i])
    }
}
