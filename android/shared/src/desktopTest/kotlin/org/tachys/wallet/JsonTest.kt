package org.tachys.wallet

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The app's own reading of JSON (Json.kt), held to org.json, which the app was written against: every field of every
 * answer of the core kept in `vectors/app/` is read both ways, by every method the app uses.
 */
class JsonTest {
    private fun same(ours: JSONObject, theirs: org.json.JSONObject, where: String) {
        assertEquals(where, theirs.keySet(), ours.keys().asSequence().toSet())
        for (k in theirs.keySet()) {
            val at = "$where.$k"
            assertEquals(at, theirs.isNull(k), ours.isNull(k))
            assertEquals(at, theirs.optLong(k, -7L), ours.optLong(k, -7L))
            assertEquals(at, theirs.optInt(k, -7), ours.optInt(k, -7))
            assertEquals(at, theirs.optBoolean(k, false), ours.optBoolean(k, false)); assertEquals(at, theirs.optBoolean(k, true), ours.optBoolean(k, true))
            val o = theirs.optJSONObject(k); val a = theirs.optJSONArray(k)
            assertEquals(at, o == null, ours.optJSONObject(k) == null); assertEquals(at, a == null, ours.optJSONArray(k) == null)
            if (o != null) same(ours.getJSONObject(k), o, at)
            else if (a != null) same(ours.optJSONArray(k)!!, a, at)
            else assertEquals(at, theirs.optString(k, "none"), ours.optString(k, "none"))
            // the readings that fail when there is nothing to give: where org.json gives a value, the same value
            fun <T> got(f: () -> T): T? = try { f() } catch (e: Exception) { null }
            got { theirs.getString(k) }?.let { assertEquals(at, it, ours.getString(k)) }
            got { theirs.getLong(k) }?.let { assertEquals(at, it, ours.getLong(k)) }
            got { theirs.getInt(k) }?.let { assertEquals(at, it, ours.getInt(k)) }
            got { theirs.getBoolean(k) }?.let { assertEquals(at, it, ours.getBoolean(k)) }
        }
    }
    private fun same(ours: JSONArray, theirs: org.json.JSONArray, where: String) {
        assertEquals(where, theirs.length(), ours.length())
        for (i in 0 until theirs.length()) {
            val o = theirs.optJSONObject(i)
            if (o != null) same(ours.getJSONObject(i), o, "$where[$i]") else if (theirs.optJSONArray(i) == null) assertEquals("$where[$i]", theirs.optString(i, "none"), ours.optString(i, "none"))
        }
    }

    @Test fun everyAnswerOfTheCoreKeptForTheAppReadsAsOrgJsonReadIt() {
        val dir = generateSequence(File("").absoluteFile) { it.parentFile }.map { File(it, "vectors/app") }.first { it.isDirectory }
        val files = dir.listFiles()!!.filter { it.name.endsWith(".json") }
        assertTrue("answers to read", files.size >= 3)
        for (f in files) { val text = f.readText(); same(JSONObject(text), org.json.JSONObject(text), f.name) }
    }

    @Test fun whatIsWrittenReadsBackTheSameWithLongNumbersExact() {
        val made = JSONObject().put("s", "a \"quoted\" line\nand a tab\t€").put("n", Long.MAX_VALUE).put("i", 7).put("b", true).put("gone", null)
            .put("o", JSONObject().put("deep", JSONArray(listOf("x", 2L, JSONObject().put("k", false)))))
        assertFalse(made.has("gone"))
        val back = JSONObject(made.toString())
        same(back, org.json.JSONObject(made.toString()), "made")
        assertEquals(Long.MAX_VALUE, back.getLong("n")); assertEquals(7, back.getInt("i")); assertEquals("a \"quoted\" line\nand a tab\t€", back.getString("s"))
        assertEquals("x", back.getJSONObject("o").optJSONArray("deep")!!.getString(0))
        assertEquals(2, JSONArray("[{\"a\":1},{\"a\":2}]").length())
    }

    @Test fun aMissingFieldIsTheFallbackAndWhatIsNotJsonIsRefused() {
        val o = JSONObject("{\"here\":1,\"nothing\":null}")
        assertEquals("", o.optString("absent")); assertEquals(0L, o.optLong("absent")); assertFalse(o.optBoolean("absent")); assertNull(o.optJSONObject("absent")); assertNull(o.optJSONArray("here"))
        assertEquals("", o.optString("nothing")); assertTrue(o.isNull("nothing")); assertTrue(o.has("nothing"))
        for (bad in listOf("", "not json", "[1,2]", "{\"cut\":")) assertTrue(bad, try { JSONObject(bad); false } catch (e: Exception) { true })
        assertTrue(try { JSONArray("{}"); false } catch (e: Exception) { true })
        assertTrue(try { o.getString("absent"); false } catch (e: Exception) { true })
    }
}
