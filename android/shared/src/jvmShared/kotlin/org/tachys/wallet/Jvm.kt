package org.tachys.wallet

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers

// What the wallet logic needs of a JVM, for Android and a computer alike.

actual fun runCore(args: List<String>, input: List<String>): Ran =
    try { Native.run(args, input) } catch (e: Exception) { Ran(-1, "", "error: ${e.message}") } catch (e: LinkageError) { Ran(-1, "", "error: the core is not in this build (${e.message})") }

actual val background: CoroutineDispatcher get() = Dispatchers.IO

actual fun <T> locked(lock: Any, block: () -> T): T = synchronized(lock, block)

actual val disk: okio.FileSystem get() = okio.FileSystem.SYSTEM
