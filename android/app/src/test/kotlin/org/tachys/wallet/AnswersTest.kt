package org.tachys.wallet

import java.io.File
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What the app makes of the core's answers, run on the answers kept in `vectors/app/`. The core's own tests make each
 * of those answers again and fail if a field of one is renamed, retyped or gone (`crates/tachys-cli/tests/cli.rs`), so
 * these two tests together hold the app and the core to one layout. Run with `./gradlew testDebugUnitTest`.
 */
class AnswersTest {
    /** An answer kept in the repository, as the app would have it from a run of the core. */
    private fun kept(name: String): Reply {
        val file = generateSequence(File("").absoluteFile) { it.parentFile }.map { File(it, "vectors/app/$name.json") }.first { it.exists() }
        val o = JSONObject(file.readText())
        return reply(o.getInt("code"), o.getJSONObject("answer").toString(), "a sentence\nthe last sentence")
    }
    private val id = Regex("[0-9a-f]{64}")

    @Test fun aPaymentANodeTookIsSent() {
        val a = kept("send-sent")
        assertTrue(a.ok)
        assertEquals("sent", sentState(a))
        val s = sentAnswer(a, "not shown")
        assertTrue(id.matches(s.getString("tx"))); assertTrue(id.matches(s.getString("receipt"))); assertTrue(id.matches(s.getString("change")))
        assertEquals(-1L, s.getLong("fee_token_units"))
        assertEquals("", s.getString("error"))
    }

    @Test fun aPaymentSignedWithNoNetworkIsQueuedWithItsBytes() {
        val a = kept("send-queued")
        assertEquals("queued", sentState(a))
        val s = sentAnswer(a, "not shown")
        assertTrue(id.matches(s.getString("tx"))); assertTrue(id.matches(s.getString("receipt")))
        assertTrue(s.getString("tx_bytes").length > 100)
        assertEquals("", s.getString("error"))
    }

    @Test fun aPaymentANodeRefusedIsRefusedWithTheReason() {
        val a = kept("send-refused")
        assertFalse(a.ok)
        assertEquals("refused", sentState(a))
        assertTrue(a.error.startsWith("send: node refused (400)"))
        assertEquals("the reason", sentAnswer(a, "the reason").getString("error"))
    }

    @Test fun aPaymentANodeDidNotAnswerForIsUnsureAndNeverSentOrRefused() {
        val a = kept("send-unsure")
        assertFalse(a.ok)
        assertEquals("unsure", sentState(a))
        // it was signed and may have gone out: the screens are given its id, to wait on, with the reason
        val s = sentAnswer(a, "the reason")
        assertTrue(id.matches(s.getString("tx")))
        assertEquals("the reason", s.getString("error"))
    }

    @Test fun aPhraseLeadsToAnAddress() {
        val a = kept("derive")
        assertTrue(a.ok)
        assertTrue(derivedAddress(a)!!.startsWith("9"))
    }

    @Test fun aWrongPasswordIsToldFromEveryOtherFailure() {
        val a = kept("bad-password")
        assertFalse(a.ok)
        assertTrue(a.badPassword)
        assertNull(textOf(a, "mnemonic"))
        assertFalse(kept("send-refused").badPassword)
    }

    @Test fun anAnswerIsOkOnlyWithExitCodeZeroItsOwnOkAndTheLayoutThisAppReads() {
        val good = """{"ok":true,"v":1,"exit":0,"command":"tips","verdict":"agreed"}"""
        assertTrue(reply(0, good, "").ok)
        assertFalse(reply(1, good, "").ok)
        assertFalse(reply(0, good.replace("\"ok\":true", "\"ok\":false"), "").ok)
        assertFalse("a layout this app does not read", reply(0, good.replace("\"v\":1", "\"v\":2"), "").ok)
        // no object at all (the core could not be run): not ok, and the reason is its last sentence
        val none = reply(0, "not json", "first\nthe reason\n")
        assertFalse(none.ok); assertNull(none.json)
        assertEquals("the reason", none.error)
        assertEquals("no answer", reply(-1, "", "").error)
        // the object is the last line that starts one, whatever came before it
        assertTrue(reply(0, "something before it\n$good\n", "").ok)
    }

    // (not an answer of the core's: `tips` needs peers, so no answer of it is kept in vectors/app/)
    @Test fun aTipIsAgreedOnlyWhenTheCoreSaysSoAndEndedWell() {
        val agreed = """{"ok":true,"v":1,"exit":0,"command":"tips","verdict":"agreed"}"""
        assertTrue(tipAgreed(reply(0, agreed, "")))
        assertFalse(tipAgreed(reply(1, agreed, "")))
        assertFalse(tipAgreed(reply(0, agreed.replace("agreed", "no quorum"), "")))
        assertFalse(tipAgreed(reply(0, "", "verdict: agreed")))
    }

    @Test fun aPasswordForAWalletIsLongEnoughTypedTheSameTwiceAndCountedAsTheCoreReadsIt() {
        assertNull(passwordFault("correct horse", "correct horse"))
        assertNotNull("too short", passwordFault("seven77", "seven77"))
        assertNull(passwordFault("eight888", "eight888"))
        assertNotNull("not the same", passwordFault("correct horse", "correct house"))
        // the core drops the spaces around the line it reads: they neither count nor differ
        assertNotNull("spaces do not make it long enough", passwordFault("  short  ", "  short  "))
        assertNull(passwordFault(" correct horse", "correct horse "))
        assertNotNull("two lines", passwordFault("correct\nhorse", "correct\nhorse"))
    }
}
