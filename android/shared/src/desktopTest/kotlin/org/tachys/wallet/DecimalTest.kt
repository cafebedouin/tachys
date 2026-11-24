package org.tachys.wallet

import java.math.BigDecimal
import java.math.RoundingMode
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Amounts shown and typed, held to what java.math.BigDecimal gives (which is what the app used before it was shared). */
class DecimalTest {
    private val rnd = java.util.Random(11)
    private fun someLong(): Long = when (rnd.nextInt(6)) { 0 -> 0L; 1 -> rnd.nextInt(1000).toLong(); 2 -> Long.MAX_VALUE - rnd.nextInt(5); 3 -> rnd.nextLong() and Long.MAX_VALUE; 4 -> 1_000_000_000L * rnd.nextInt(50); else -> (rnd.nextLong() and Long.MAX_VALUE) % 100_000_000_000L }

    @Test fun anAmountIsShownAsBigDecimalShowedIt() {
        assertEquals("1.5", shown(1_500_000_000L, 9)); assertEquals("0", shown(0L, 9)); assertEquals("0.000000001", shown(1L, 9)); assertEquals("20", shown("20000000000", 9))
        repeat(20000) {
            val n = someLong() * (if (rnd.nextInt(8) == 0) -1 else 1)
            val d = rnd.nextInt(24) - 4
            assertEquals("$n / $d", BigDecimal(n).movePointLeft(d).stripTrailingZeros().toPlainString(), shown(n, d))
        }
        // a token's amount can be given as digits longer than a Long
        assertEquals(BigDecimal("123456789012345678901234567890").movePointLeft(7).stripTrailingZeros().toPlainString(), shown("123456789012345678901234567890", 7))
    }

    @Test fun aBalanceIsCutToSixPlacesAndNeverRoundedUp() {
        assertEquals("20.704714", shownFixed(20_704_714_794L, 9, 6)); assertEquals("0.000000", shownFixed(999L, 9, 6))
        repeat(20000) {
            val n = someLong() * (if (rnd.nextInt(8) == 0) -1 else 1)
            assertEquals("$n", BigDecimal(n).movePointLeft(9).setScale(6, RoundingMode.DOWN).toPlainString(), shownFixed(n, 9, 6))
        }
    }

    private fun was(text: String, decimals: Int): Long = try { (text.toBigDecimalOrNull() ?: BigDecimal.ZERO).movePointRight(decimals).setScale(0, RoundingMode.DOWN).longValueExact() } catch (e: ArithmeticException) { Long.MAX_VALUE }

    @Test fun anAmountTypedBecomesTheUnitsBigDecimalGave() {
        assertEquals(1_500_000_000L, units("1.5", 9)); assertEquals(0L, units("", 9)); assertEquals(0L, units(null, 9)); assertEquals(0L, units(".", 9)); assertEquals(0L, units("1.2.3", 9))
        assertEquals(Long.MAX_VALUE, units("99999999999", 9)); assertEquals(1L, units("0.0000000019", 9))
        for (text in listOf("0", "1", "1.", ".5", "0.5", "007", "12.3400", "9223372036.854775807", "9223372036.854775808", "0.0000000001", "123456789012345678901234567890", "5.000000000999")) for (d in listOf(0, 2, 9, 18)) assertEquals("$text / $d", was(text, d), units(text, d))
        repeat(20000) {
            val whole = if (rnd.nextInt(5) == 0) "" else (rnd.nextLong() and Long.MAX_VALUE).toString().take(1 + rnd.nextInt(19))
            val part = if (rnd.nextInt(3) == 0) "" else "." + (rnd.nextLong() and Long.MAX_VALUE).toString().take(rnd.nextInt(14))
            val text = whole + part
            val d = rnd.nextInt(19)
            if (text.any { it.isDigit() }) assertEquals("$text / $d", was(text, d), units(text, d))
        }
    }

    @Test fun whatIsShownReadsBackAsTheSameUnits() {
        repeat(20000) { val n = someLong(); val d = rnd.nextInt(19); assertEquals("$n / $d", n, units(shown(n, d), d)) }
    }

    @Test fun anAmountAskedForIsMoreThanNothingOrIsNotTaken() {
        assertTrue(positive("1.5")); assertTrue(positive("0.0000000000001")); assertTrue(positive(".5"))
        assertFalse(positive("0")); assertFalse(positive("0.000")); assertFalse(positive("")); assertFalse(positive(".")); assertFalse(positive("-1")); assertFalse(positive("1.2.3"))
    }

    @Test fun whenTheFeeIsPaidInATokenWhatItMayCostIsKeptBackFromWhatCanBeSent() {
        // 0.0001 ERG a unit: the 0.0031 ERG a payment can be short of is 31 units at most
        assertEquals(31L, feeCeiling(100_000L)); assertEquals(104L, leftAfterFee(135L, 100_000L))
        assertEquals(0L, leftAfterFee(31L, 100_000L)); assertEquals(0L, leftAfterFee(20L, 100_000L)); assertEquals(1L, leftAfterFee(32L, 100_000L))
        // a price that does not divide the ERG: rounded up, never short
        assertEquals(2L, feeCeiling(3_099_999L)); assertEquals(1L, feeCeiling(3_100_000L)); assertEquals(4L, feeCeiling(1_000_000L))
        // no price known: nothing is kept
        assertEquals(135L, leftAfterFee(135L, 0L))
    }
}
