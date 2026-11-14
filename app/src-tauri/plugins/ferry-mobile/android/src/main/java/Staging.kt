package com.xenvaultinteractive.ferry.mobile

import android.content.ContentResolver
import android.net.Uri
import android.os.StatFs
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import java.io.File
import java.io.IOException

/**
 * Copies files and folders the user picked (or shared to Ferry from another app) into Ferry's
 * own storage, where Ferry can serve them. Android hands them out as `content://` URIs, which
 * may stop working once the activity that received them is gone, so they're copied right away.
 */
internal class Staging(private val resolver: ContentResolver) {
    /** A file to copy: where it comes from and its path below the destination folder. */
    private class Entry(val uri: Uri, val path: String, val size: Long)

    /** One picked item: a file, or a folder with everything inside it. */
    private class Item(val name: String, val isFolder: Boolean, val files: List<Entry>)

    class Result(val paths: List<String>, val skipped: Int)

    /**
     * Copy every item into `dest`, which must exist and be empty. Reports progress as
     * (bytes copied, bytes in total). Fails before copying anything when there isn't room.
     */
    fun copy(uris: List<Uri>, dest: File, progress: (Long, Long) -> Unit): Result {
        val items = uris.map(::describe)
        var skipped = 0
        val total = items.sumOf { item -> item.files.sumOf { it.size.coerceAtLeast(0) } }
        val free = StatFs(dest.path).availableBytes
        if (total + SPACE_MARGIN > free) {
            throw IOException(
                "There isn't enough free space on this phone: sharing needs ${human(total)}, " +
                    "and ${human(free)} is free."
            )
        }
        var done = 0L
        progress(done, total)
        val taken = mutableSetOf<String>()
        val paths = items.mapNotNull { item ->
            if (item.files.isEmpty()) {
                skipped++
                return@mapNotNull null
            }
            // Two picked items may have the same name; the second becomes "name (2)".
            val top = unique(item.name, taken)
            val root = File(dest, top)
            for (entry in item.files) {
                val target = if (item.isFolder) File(root, entry.path) else root
                target.parentFile?.mkdirs()
                val input = resolver.openInputStream(entry.uri)
                    ?: throw IOException("Couldn't read ${entry.path}")
                input.use { source ->
                    target.outputStream().use { sink ->
                        val buffer = ByteArray(1 shl 16)
                        var lastReport = 0L
                        while (true) {
                            val n = source.read(buffer)
                            if (n < 0) break
                            sink.write(buffer, 0, n)
                            done += n
                            val now = System.nanoTime()
                            if (now - lastReport > REPORT_EVERY_NS) {
                                lastReport = now
                                progress(done, maxOf(total, done))
                            }
                        }
                    }
                }
            }
            root.path
        }
        progress(done, maxOf(total, done))
        if (paths.isEmpty()) throw IOException("There's nothing to share: the selection has no files.")
        return Result(paths, skipped)
    }

    private fun describe(uri: Uri): Item =
        if (DocumentsContract.isTreeUri(uri)) {
            val rootId = DocumentsContract.getTreeDocumentId(uri)
            val name = query(DocumentsContract.buildDocumentUriUsingTree(uri, rootId))?.first ?: "Folder"
            val files = mutableListOf<Entry>()
            walk(uri, rootId, "", files)
            Item(clean(name), true, files)
        } else {
            val (name, size) = query(uri) ?: (uri.lastPathSegment to -1L)
            Item(clean(name ?: "file"), false, listOf(Entry(uri, clean(name ?: "file"), size)))
        }

    /** Display name and size of a single document, if the provider says. */
    private fun query(uri: Uri): Pair<String?, Long>? =
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
            ?.use { c ->
                if (!c.moveToFirst()) return@use null
                val name = c.getString(0)
                val size = if (c.isNull(1)) -1L else c.getLong(1)
                name to size
            }

    /** Every file below a folder of a picked tree, with paths relative to the picked folder. */
    private fun walk(tree: Uri, docId: String, prefix: String, out: MutableList<Entry>) {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId)
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        resolver.query(children, columns, null, null, null)?.use { c ->
            while (c.moveToNext()) {
                val id = c.getString(0)
                val name = clean(c.getString(1) ?: id)
                val path = if (prefix.isEmpty()) name else "$prefix/$name"
                if (c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                    walk(tree, id, path, out)
                } else {
                    val size = if (c.isNull(3)) -1L else c.getLong(3)
                    out += Entry(DocumentsContract.buildDocumentUriUsingTree(tree, id), path, size)
                }
            }
        }
    }

    companion object {
        /** Room left over after copying, so sharing never fills the phone completely. */
        private const val SPACE_MARGIN = 64L * 1024 * 1024
        private const val REPORT_EVERY_NS = 250_000_000L

        /** A name that is safe as one path component. */
        fun clean(name: String): String {
            val cleaned = name.map { if (it == '/' || it == '\\' || it.code < 32) '_' else it }
                .joinToString("").trim()
            return when (cleaned) {
                "", ".", ".." -> "file"
                else -> cleaned
            }
        }

        fun unique(name: String, taken: MutableSet<String>): String {
            var candidate = name
            var n = 2
            while (!taken.add(candidate.lowercase())) {
                val dot = name.lastIndexOf('.').takeIf { it > 0 } ?: name.length
                candidate = "${name.substring(0, dot)} ($n)${name.substring(dot)}"
                n++
            }
            return candidate
        }

        private fun human(bytes: Long): String {
            val units = arrayOf("B", "KB", "MB", "GB", "TB")
            var value = bytes.toDouble()
            var unit = 0
            while (value >= 1024 && unit < units.size - 1) {
                value /= 1024
                unit++
            }
            return if (unit == 0) "$bytes B" else String.format(java.util.Locale.ROOT, if (value < 10) "%.1f %s" else "%.0f %s", value, units[unit])
        }
    }
}
