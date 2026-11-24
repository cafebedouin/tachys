package org.tachys.wallet

import kotlinx.cinterop.ByteVar
import kotlinx.cinterop.CPointerVar
import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.allocArray
import kotlinx.cinterop.convert
import kotlinx.cinterop.cstr
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.pointed
import kotlinx.cinterop.set
import kotlinx.cinterop.toKString
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.IO
import org.tachys.core.tachys_call
import org.tachys.core.tachys_free
import platform.Foundation.NSRecursiveLock

// NEVER COMPILED. The project is developed on Linux, where nothing for an iPhone can be built, so this file is a
// first draft of what the wallet logic needs of iOS, written from the documentation and not once run through a
// compiler. Expect to correct it. What else an iPhone app needs is in docs/platforms.md.

/** The core through its C interface (tachys.h): strings in, an answer out, freed by the core. */
@OptIn(ExperimentalForeignApi::class)
actual fun runCore(args: List<String>, input: List<String>): Ran = memScoped {
    val a = allocArray<CPointerVar<ByteVar>>(args.size)
    args.forEachIndexed { i, s -> a[i] = s.cstr.ptr }
    val b = allocArray<CPointerVar<ByteVar>>(input.size)
    input.forEachIndexed { i, s -> b[i] = s.cstr.ptr }
    val answer = tachys_call(a, args.size.convert(), b, input.size.convert()) ?: return@memScoped Ran(-1, "", "error: the core gave no answer")
    try { answer.pointed.let { Ran(it.code, it.out?.toKString() ?: "", it.err?.toKString() ?: "") } } finally { tachys_free(answer) }
}

actual val background: CoroutineDispatcher get() = Dispatchers.IO

private val one = NSRecursiveLock()
/** One lock for every `locked` block: the app has one use of it (agreeing on a tip). */
actual fun <T> locked(lock: Any, block: () -> T): T { one.lock(); try { return block() } finally { one.unlock() } }

actual val disk: okio.FileSystem get() = okio.FileSystem.SYSTEM
