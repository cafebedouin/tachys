package org.tachys.wallet

import kotlinx.coroutines.CoroutineDispatcher

// The three things the wallet logic needs that each kind of platform does its own way (the JVM's are in
// src/jvmShared, for Android and a computer alike; an iPhone's would be in src/iosMain).

/** What one run of the core gave: the code it ended with, what it wrote as its answer, and its sentences. */
class Ran(val code: Int, val out: String, val err: String)

/**
 * One run of the core, in this process: `args` as on its command line, `input` the lines it reads where the program
 * would read its input (a password, then a seed phrase), so that neither is ever an argument or a file. A core that
 * cannot be reached at all is an answer too (code -1 and the reason), never a failure thrown.
 */
expect fun runCore(args: List<String>, input: List<String>): Ran

/** Where work that waits (on the network, on a slow key) is done, off the thread that draws. */
expect val background: CoroutineDispatcher

/** Runs `block` with no other `locked` block on the same `lock` running beside it. */
expect fun <T> locked(lock: Any, block: () -> T): T

/** This machine's files. */
expect val disk: okio.FileSystem
