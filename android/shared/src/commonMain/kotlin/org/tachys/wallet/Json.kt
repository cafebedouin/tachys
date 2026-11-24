package org.tachys.wallet

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// The core answers in JSON, and the app keeps a little of its own in JSON. These two classes read and write it the
// same on every platform, with the few methods the app uses, named as org.json names them (which is what the app
// was written against, and what a phone has built in). A missing or null field read with `opt…` is the fallback, never
// a failure; `get…` fails if there is nothing to give. Whole numbers are kept exactly, as Long.

private fun fromElement(e: JsonElement): Any? = when (e) {
    is JsonNull -> null
    is JsonObject -> JSONObject().also { o -> e.forEach { (k, v) -> o.fields[k] = fromElement(v) } }
    is JsonArray -> JSONArray().also { a -> e.forEach { a.items.add(fromElement(it)) } }
    is JsonPrimitive -> if (e.isString) e.content else when (e.content) {
        "true" -> true
        "false" -> false
        else -> e.content.toLongOrNull() ?: e.content.toDouble()
    }
}
private fun toElement(v: Any?): JsonElement = when (v) {
    null -> JsonNull
    is JSONObject -> JsonObject(v.fields.mapValues { toElement(it.value) })
    is JSONArray -> JsonArray(v.items.map { toElement(it) })
    is String -> JsonPrimitive(v)
    is Boolean -> JsonPrimitive(v)
    is Number -> JsonPrimitive(v)
    else -> JsonPrimitive(v.toString())
}
private fun text(v: Any?): String? = when (v) { null -> null; is JSONObject, is JSONArray -> v.toString(); else -> v.toString() }
private fun long(v: Any?): Long? = when (v) { is Long -> v; is Number -> v.toLong(); is String -> v.toLongOrNull() ?: v.toDoubleOrNull()?.toLong(); else -> null }
private fun bool(v: Any?): Boolean? = when (v) { is Boolean -> v; is String -> when (v.lowercase()) { "true" -> true; "false" -> false; else -> null }; else -> null }
private fun missing(what: String): Nothing = throw IllegalArgumentException("no $what in this JSON")

class JSONObject() {
    internal val fields = LinkedHashMap<String, Any?>()
    /** Fails if `text` is not one JSON object. */
    constructor(text: String) : this() {
        val e = try { Json.parseToJsonElement(text) } catch (e: Exception) { throw IllegalArgumentException("not JSON: ${e.message}") }
        (e as? JsonObject ?: throw IllegalArgumentException("not a JSON object")).forEach { (k, v) -> fields[k] = fromElement(v) }
    }
    fun has(key: String) = fields.containsKey(key)
    fun isNull(key: String) = fields[key] == null
    fun keys(): Iterator<String> = fields.keys.toList().iterator()
    fun length() = fields.size
    /** A null value takes the field away. */
    fun put(key: String, value: Any?): JSONObject { if (value == null) fields.remove(key) else fields[key] = if (value is Int) value.toLong() else value; return this }
    fun remove(key: String) { fields.remove(key) }
    fun optString(key: String, otherwise: String = ""): String = text(fields[key]) ?: otherwise
    fun optLong(key: String, otherwise: Long = 0L): Long = long(fields[key]) ?: otherwise
    fun optInt(key: String, otherwise: Int = 0): Int = long(fields[key])?.toInt() ?: otherwise
    fun optBoolean(key: String, otherwise: Boolean = false): Boolean = bool(fields[key]) ?: otherwise
    fun optJSONObject(key: String): JSONObject? = fields[key] as? JSONObject
    fun optJSONArray(key: String): JSONArray? = fields[key] as? JSONArray
    fun getString(key: String): String = text(fields[key]) ?: missing(key)
    fun getLong(key: String): Long = long(fields[key]) ?: missing(key)
    fun getInt(key: String): Int = getLong(key).toInt()
    fun getBoolean(key: String): Boolean = bool(fields[key]) ?: missing(key)
    fun getJSONObject(key: String): JSONObject = optJSONObject(key) ?: missing(key)
    override fun toString(): String = toElement(this).toString()
}

class JSONArray() {
    internal val items = ArrayList<Any?>()
    /** Fails if `text` is not one JSON array. */
    constructor(text: String) : this() {
        val e = try { Json.parseToJsonElement(text) } catch (e: Exception) { throw IllegalArgumentException("not JSON: ${e.message}") }
        (e as? JsonArray ?: throw IllegalArgumentException("not a JSON array")).forEach { items.add(fromElement(it)) }
    }
    constructor(of: Collection<Any?>) : this() { of.forEach { put(it) } }
    fun length() = items.size
    fun put(value: Any?): JSONArray { items.add(if (value is Int) value.toLong() else value); return this }
    fun optString(index: Int, otherwise: String = ""): String = text(items.getOrNull(index)) ?: otherwise
    fun getString(index: Int): String = text(items.getOrNull(index)) ?: missing("item $index")
    fun getJSONObject(index: Int): JSONObject = items.getOrNull(index) as? JSONObject ?: missing("object at $index")
    override fun toString(): String = toElement(this).toString()
}
