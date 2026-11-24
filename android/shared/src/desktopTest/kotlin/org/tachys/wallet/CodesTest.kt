package org.tachys.wallet

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The coding of receipts and scanned links. A receipt made by one build must read on another, and on the builds from
 * before the screens were shared, which used Android's `Uri.encode`, `Uri.getQueryParameter` and `Base64` (URL_SAFE,
 * no padding, no wrapping): what those give for a purpose and a payment is written out here, and Java's own base64 is
 * asked for the same bytes. (What is done with a % that starts no byte is this code's own choice.)
 */
class CodesTest {
    @Test fun aPurposeIsCodedAsAndroidCodedIt() {
        assertEquals("rent%20for%20May", percentEncoded("rent for May"))
        assertEquals("a_-!.~'()*z", percentEncoded("a_-!.~'()*z"))
        assertEquals("%26%3D%3F%23%2F%2B%25", percentEncoded("&=?#/+%"))
        assertEquals("caf%C3%A9%20%E2%82%AC", percentEncoded("café €"))
    }

    @Test fun aCodedPurposeReadsBackAndAStrayPercentIsKept() {
        for (text in listOf("rent for May", "&=?#/+%", "café €", "100% sure", "", "日本語")) assertEquals(text, percentDecoded(percentEncoded(text)))
        assertEquals("a b", percentDecoded("a+b"))
        assertEquals("100% sure", percentDecoded("100% sure"))
        assertEquals("%zz and %4", percentDecoded("%zz and %4"))
        assertEquals("% 1", percentDecoded("%+1"))
    }

    @Test fun aQueryGivesEachParameterDecodedAndTheFirstOfANameGivenTwice() {
        val q = queryOf("tachys:receipt?box=ab&p=rent%20for+May&p=second&odd")
        assertEquals("ab", q["box"]); assertEquals("rent for May", q["p"]); assertNull(q["odd"]); assertEquals(2, q.size)
        assertEquals(0, queryOf("no query here").size)
    }

    @Test fun bytesCrossAsBase64ForLinksWithNoPaddingAndReadBackPaddedOrNot() {
        val rnd = java.util.Random(7)
        for (n in 0..70) {
            val bytes = ByteArray(n).also { rnd.nextBytes(it) }
            val ours = base64Url(bytes)
            assertEquals(java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(bytes), ours)
            assertArrayEquals(bytes, base64UrlBytes(ours))
            assertArrayEquals(bytes, base64UrlBytes(java.util.Base64.getUrlEncoder().encodeToString(bytes)))
        }
        assertNull("not that alphabet", base64UrlBytes("ab+/"))
        assertNull("a length no bytes give", base64UrlBytes("abcde"))
    }

    @Test fun aReceiptReadsBackWithItsPurposeAndItsPayment() {
        val box = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        assertEquals(box, Receipt(box, "").code())
        val plain = Receipt.parse(Receipt(box, "rent for May & June").code())
        assertEquals(box, plain.box); assertEquals("rent for May & June", plain.purpose); assertEquals("", plain.tx)
        assertEquals("tachys:receipt?box=$box&p=rent%20for%20May", Receipt(box, "rent for May").code())
        val tx = "00ff10a7c3"
        val carried = Receipt.parse(Receipt(box, "café", tx).code())
        assertEquals(box, carried.box); assertEquals("café", carried.purpose); assertEquals(tx, carried.tx)
        assertEquals("tachys:tx?d=AP8Qp8M&box=$box", Receipt(box, "", tx).code())
        assertEquals("", Receipt.parse("tachys:receipt?box=nonsense").box)
    }

    @Test fun aPaymentRequestIsReadFromABareAddressALinkAndAnotherWalletsPage() {
        assertEquals("9abc", payRequestFromCode(" 9abc ").address)
        payRequestFromCode("ergo:9abc?amount=1.5&description=two%20words").let { assertEquals("9abc", it.address); assertEquals("1.5", it.erg); assertEquals("two words", it.note) }
        payRequestFromCode("https://explorer.ergoplatform.com/payment-request?address=9abc&amount=0&token-aa=3").let { assertEquals("9abc", it.address); assertEquals("", it.erg); assertEquals(true, it.tokens) }
    }

    @Test fun aPaymentLinkThatCannotBeReadGivesNoAmountAndNoNote() {
        // a % that starts no byte: the digits around it must not run together into an amount (1%2 is not 12)
        payRequestFromCode("ergo:9abc?amount=1%2").let { assertEquals("9abc", it.address); assertEquals("", it.erg) }
        payRequestFromCode("ergo:9abc?amount=1%zz5&description=fine").let { assertEquals("", it.erg); assertEquals("fine", it.note) }
        // bytes that are not text
        assertEquals("", payRequestFromCode("ergo:9abc?amount=2&description=%FF%FE").note)
        assertEquals("2", payRequestFromCode("ergo:9abc?amount=2&description=%FF%FE").erg)
        // and what Java's own decoder refused or took, this refuses or takes
        for (v in listOf("1%2", "%", "%zz", "a%2", "%FF", "%C3%A9", "a+b", "1.5", "", "%E2%82%AC", "100%25")) {
            val java = try { java.net.URLDecoder.decode(v, "UTF-8").takeIf { '\uFFFD' !in it } } catch (e: Exception) { null }
            assertEquals(v, java, percentDecodedStrictly(v))
        }
    }
}
