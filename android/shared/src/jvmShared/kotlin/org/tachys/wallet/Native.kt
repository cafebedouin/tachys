package org.tachys.wallet

// The core, in this process: the Rust library `crates/tachys-c` (libtachys_c.so), whose one call is the `tachys`
// program's command line. Nothing of Android is used here, so the tests run it on the build machine too.


/**
 * A list of strings as it crosses to the core: UTF-8, each ended by a 0 byte, so an empty list is no bytes. Only arrays
 * of bytes cross: the JNI's own strings are a modified UTF-8, and an array can be wiped where a String cannot.
 */
fun packed(items: List<String>): ByteArray {
    // the program refused such an argument too: it cannot be told from the end of one
    require(items.none { '\u0000' in it }) { "a 0 character in an argument or an input line" }
    val o = java.io.ByteArrayOutputStream()
    items.forEach { val b = it.toByteArray(Charsets.UTF_8); o.write(b); o.write(0); b.fill(0) }
    return o.toByteArray()
}

/** The core's answer as it crosses back: the code and the length of `out` (two big-endian 32-bit numbers), `out`, then `err`. */
fun unpacked(b: ByteArray?): Ran {
    if (b == null || b.size < 8) return Ran(-1, "", "error: the core gave no answer")
    val head = java.nio.ByteBuffer.wrap(b)
    val code = head.int; val n = head.int
    if (n < 0 || n > b.size - 8) return Ran(-1, "", "error: the core's answer is cut short")
    return Ran(code, String(b, 8, n, Charsets.UTF_8), String(b, 8 + n, b.size - 8 - n, Charsets.UTF_8))
}

object Native {
    init {
        // a packaged app for a computer carries the library among its resources; otherwise it is found as the system finds libraries (a phone's own, or java.library.path)
        val packed = System.getProperty("compose.application.resources.dir")?.let { java.io.File(it, System.mapLibraryName("tachys_c")) }
        if (packed != null && packed.exists()) System.load(packed.absolutePath) else System.loadLibrary("tachys_c")
    }
    /** `Java_org_tachys_wallet_Native_call` in `crates/tachys-c/src/jni.rs`. */
    @JvmStatic private external fun call(args: ByteArray, input: ByteArray): ByteArray?

    /**
     * Runs one command of the core and waits for it to end. `args` are the program's arguments after its name; `input`
     * are the lines it reads where the program would read its input (a password, then a seed phrase), so that neither
     * is ever an argument or a file. May be called from several threads at once: each call has its own answer and its
     * own network, and a quick one (a QR code to draw) does not wait for a slow one (a sync).
     */
    fun run(args: List<String>, input: List<String>): Ran {
        val lines = packed(input)
        val answer = try { call(packed(listOf("tachys") + args), lines) } finally { lines.fill(0) }
        try { return unpacked(answer) } finally { answer?.fill(0) }
    }
}
