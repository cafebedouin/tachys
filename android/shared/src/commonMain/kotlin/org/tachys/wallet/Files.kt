package org.tachys.wallet

import okio.FileSystem
import okio.Path
import okio.Path.Companion.toPath

/** A file or folder at a full path, with the few things the app does to one, the same on every platform. */
class FileAt private constructor(private val path: Path) {
    constructor(absolutePath: String) : this(absolutePath.toPath())
    constructor(parent: FileAt, child: String) : this(parent.path / child)
    constructor(parent: String, child: String) : this(parent.toPath() / child)
    val absolutePath: String get() = path.toString()
    val name: String get() = path.name
    fun exists(): Boolean = disk.exists(path)
    val isFile: Boolean get() = disk.metadataOrNull(path)?.isRegularFile == true
    val isDirectory: Boolean get() = disk.metadataOrNull(path)?.isDirectory == true
    /** What a folder holds; null if this is not a folder that can be read. */
    fun listFiles(): List<FileAt>? = disk.listOrNull(path)?.map { FileAt(it) }
    fun readBytes(): ByteArray = disk.read(path) { readByteArray() }
    fun readText(): String = disk.read(path) { readUtf8() }
    fun writeBytes(bytes: ByteArray) { disk.write(path) { write(bytes) } }
    fun writeText(text: String) { disk.write(path) { writeUtf8(text) } }
    /** Copies this file's bytes to `target`, over whatever is there. */
    fun copyTo(target: FileAt) { disk.copy(path, target.path) }
    fun mkdirs() { disk.createDirectories(path) }
    fun delete() { disk.delete(path, mustExist = false) }
    fun deleteRecursively() { disk.deleteRecursively(path, mustExist = false) }
    override fun toString() = absolutePath
}
