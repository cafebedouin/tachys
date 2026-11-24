package org.tachys.wallet

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What the screens read of a wallet file: the balance on a wallet's card, its tokens, what Send lets be sent, and
 * where a remembered payment stands. Read from `vectors/app/wallet-file.json`, which the core's own tests hold to
 * what the core writes and to its sums (`walletfile.rs`), so the app's reading and the core's are of one file.
 */
class ViewTest {
    private val kept: String = generateSequence(File("").absoluteFile) { it.parentFile }.map { File(it, "vectors/app/wallet-file.json") }.first { it.exists() }.readText()
    private fun id(c: Char) = c.toString().repeat(64)
    private fun view(text: String = kept) = walletView(text, "st-mainnet", mapOf(id('a') to ("Named" to 2)), mapOf())

    @Test fun theBalanceIsWhatIsConfirmedAndABoxOnlyOneNodeHasShownIsCountedApart() {
        val w = view()
        // the core's own sums of this file: 3501000001 confirmed, 900000000 beside it
        assertEquals(3_501_000_001L, w.nano); assertEquals(900_000_000L, w.seen)
        assertEquals("3.501000", w.erg)
        assertEquals(4, w.boxes); assertEquals(106, w.provenAt)
        assertEquals("kept for the app's tests", w.name); assertEquals("9abc", w.address); assertEquals(listOf("9abc"), w.addresses)
        assertFalse(w.readOnly)
        assertEquals(listOf(id('e')), w.expecting); assertEquals(listOf(id('f')), w.void)
    }

    @Test fun aTokenIsSummedOverTheConfirmedBoxesAndNamedByTheListTheAppShips() {
        val tokens = view().tokens
        assertEquals(listOf(id('a'), id('b')), tokens.map { it.id })
        // 700 and 300 in confirmed boxes; the 5 in the box only one node has shown are not the wallet's yet
        tokens[0].let { assertEquals(1_000L, it.raw); assertEquals("Named", it.name); assertEquals("10", it.amount); assertFalse(it.reported) }
        // two boxes each holding the most a box can: held at the most this can show, not wrapped round to a negative amount
        tokens[1].let { assertEquals(Long.MAX_VALUE, it.raw); assertTrue(it.reported); assertEquals(0, it.decimals) }
    }

    @Test fun amountsAddedAreHeldAtTheTopAndNeverWrapRound() {
        assertEquals(5L, added(2L, 3L)); assertEquals(-1L, added(2L, -3L))
        assertEquals(Long.MAX_VALUE, added(Long.MAX_VALUE, 1L)); assertEquals(Long.MAX_VALUE, added(Long.MAX_VALUE, Long.MAX_VALUE))
        assertEquals(Long.MIN_VALUE, added(Long.MIN_VALUE, -1L))
        assertEquals(Long.MAX_VALUE, added(Long.MAX_VALUE, 0L))
    }

    @Test fun aWalletFileANewerBuildWroteIsLeftUnread() {
        val newer = kept.replaceFirst("\"version\": 1", "\"version\": 2")
        assertTrue(newer != kept)
        assertTrue(try { view(newer); false } catch (e: IllegalStateException) { true })
        // and one that is not a wallet file at all is no wallet, not an empty one
        assertTrue(try { view("{\"name\": \"cut sh"); false } catch (e: Exception) { true })
    }

    @Test fun aPaymentHeldForThisWalletThatCanNeverConfirmIsNotShownAsReceived() {
        val w = view()
        fun held(box: String) = JSONObject().put("dir", "in").put("state", "promised").put("receipt", box)
        assertEquals("Promised, not on chain yet", paymentStatus(held(id('e')), w))
        // the core took it off the boxes expected and marked it void: that alone used to read as "Received"
        assertTrue(paymentStatus(held(id('f')), w).startsWith("Void"))
        // in a block, and no longer expected or void
        assertEquals("Received", paymentStatus(held(id('1')), w))
        // a receipt checked against the chain was received when it was remembered
        assertEquals("Received", paymentStatus(JSONObject().put("dir", "in").put("state", "sent").put("receipt", id('1')), w))
    }
}
