package me.edfork.workspace.clip

import android.content.ClipDescription
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.res.AssetFileDescriptor
import android.database.Cursor
import android.database.MatrixCursor
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Binder
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.os.Process
import android.provider.OpenableColumns
import java.io.File
import java.io.FileNotFoundException
import java.util.Locale
import java.util.UUID

/**
 * Persistent exported ContentProvider for clipboard images synced from PC.
 *
 * Keeps recent clipboard image files in the app's own cache directory and serves exact
 * byte lengths via `OpenableColumns.SIZE` and `AssetFileDescriptor(pfd, 0, file.length())`
 * so apps such as WeChat never read `0B` or race against short-lived temp processes.
 */
class ClipboardImageProvider : ContentProvider() {

    override fun onCreate(): Boolean = true

    override fun call(method: String, arg: String?, extras: Bundle?): Bundle {
        enforcePrivilegedWriter()
        val ctx = context ?: return Bundle().apply {
            putBoolean("ok", false)
            putString("error", "ContentProvider context unavailable")
        }
        return when (method) {
            METHOD_PREPARE -> {
                val normalizedMime = normalizeMime(arg ?: extras?.getString(KEY_MIME))
                val ext = extensionForMime(normalizedMime)
                val dir = cacheRootDir(ctx)
                if (!dir.exists()) {
                    dir.mkdirs()
                }
                pruneOldImages(dir, MAX_CACHED_IMAGES - 1)
                val fileName = "clip_${System.currentTimeMillis()}_${UUID.randomUUID().toString().take(8)}.$ext"
                val target = File(dir, fileName)
                if (!target.exists()) {
                    target.createNewFile()
                }
                val contentUri = Uri.Builder()
                    .scheme("content")
                    .authority(AUTHORITY)
                    .appendPath(PATH_SEGMENT_IMAGE)
                    .appendPath(fileName)
                    .build()
                for (pkg in arrayOf("android", "com.android.shell")) {
                    runCatching {
                        ctx.grantUriPermission(pkg, contentUri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    }
                }
                Bundle().apply {
                    putBoolean("ok", true)
                    putString("uri", contentUri.toString())
                    putString("file_name", fileName)
                    putString(KEY_MIME, normalizedMime)
                }
            }
            else -> Bundle().apply {
                putBoolean("ok", false)
                putString("error", "Unknown method: $method")
            }
        }
    }

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? {
        val file = resolveImageFile(uri) ?: return null
        if (!file.isFile) return null
        val mime = mimeForFileName(file.name)
        val size = file.length()
        val (width, height) = readImageBounds(file)

        val cols = if (projection.isNullOrEmpty()) {
            arrayOf(
                OpenableColumns.DISPLAY_NAME,
                OpenableColumns.SIZE,
                COL_MIME_TYPE,
                COL_WIDTH,
                COL_HEIGHT,
            )
        } else {
            @Suppress("UNCHECKED_CAST")
            projection as Array<String>
        }

        val cursor = MatrixCursor(cols, 1)
        val row = Array<Any?>(cols.size) { idx ->
            when (cols[idx]) {
                OpenableColumns.DISPLAY_NAME, "title" -> file.name
                OpenableColumns.SIZE -> size
                COL_MIME_TYPE -> mime
                COL_WIDTH -> width
                COL_HEIGHT -> height
                COL_DATA -> null // Force streaming via ContentResolver FD rather than raw path
                else -> null
            }
        }
        cursor.addRow(row)
        return cursor
    }

    override fun getType(uri: Uri): String? {
        val name = parseValidImageFileName(uri.path) ?: return null
        return mimeForFileName(name)
    }

    override fun getStreamTypes(uri: Uri, mimeTypeFilter: String): Array<String>? {
        val mime = getType(uri) ?: return null
        return if (ClipDescription.compareMimeTypes(mime, mimeTypeFilter)) {
            arrayOf(mime)
        } else {
            null
        }
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        val file = resolveImageFile(uri)
            ?: throw FileNotFoundException("Invalid clipboard image URI: $uri")
        val lowerMode = mode.lowercase(Locale.ROOT)
        val isWriteMode = lowerMode.contains('w') || lowerMode.contains('t') || lowerMode.contains('+')
        if (isWriteMode) {
            enforcePrivilegedWriter()
            file.parentFile?.mkdirs()
            return ParcelFileDescriptor.open(file, ParcelFileDescriptor.parseMode(mode))
        }
        if (!file.isFile) {
            throw FileNotFoundException("Clipboard image does not exist: ${file.name}")
        }
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun openAssetFile(uri: Uri, mode: String): AssetFileDescriptor {
        val lowerMode = mode.lowercase(Locale.ROOT)
        val isWriteMode = lowerMode.contains('w') || lowerMode.contains('t') || lowerMode.contains('+')
        val pfd = openFile(uri, mode)
        if (isWriteMode) {
            return AssetFileDescriptor(pfd, 0L, AssetFileDescriptor.UNKNOWN_LENGTH)
        }
        val file = resolveImageFile(uri)
            ?: throw FileNotFoundException("Invalid clipboard image URI: $uri")
        return AssetFileDescriptor(pfd, 0L, file.length())
    }

    override fun openTypedAssetFile(
        uri: Uri,
        mimeTypeFilter: String,
        opts: Bundle?,
    ): AssetFileDescriptor {
        val mime = getType(uri) ?: throw FileNotFoundException("Unknown image URI: $uri")
        if (!ClipDescription.compareMimeTypes(mime, mimeTypeFilter)) {
            throw FileNotFoundException("MIME filter $mimeTypeFilter does not match $mime")
        }
        return openAssetFile(uri, "r")
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int = 0

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int = 0

    private fun resolveImageFile(uri: Uri): File? {
        val ctx = context ?: return null
        val fileName = parseValidImageFileName(uri.path) ?: return null
        return File(cacheRootDir(ctx), fileName)
    }

    private fun enforcePrivilegedWriter() {
        val uid = Binder.getCallingUid()
        if (uid != Process.ROOT_UID &&
            uid != Process.SYSTEM_UID &&
            uid != Process.SHELL_UID &&
            uid != Process.myUid()
        ) {
            throw SecurityException("Unauthorized clipboard writer uid=$uid")
        }
    }

    private fun cacheRootDir(ctx: Context): File {
        val storageCtx = ctx.createDeviceProtectedStorageContext() ?: ctx
        return File(storageCtx.cacheDir, CACHE_SUBDIR)
    }

    private fun pruneOldImages(dir: File, keepCount: Int) {
        val files = dir.listFiles()?.filter { it.isFile }?.sortedByDescending { it.lastModified() }
            ?: return
        if (files.size <= keepCount) return
        files.drop(keepCount).forEach { it.delete() }
    }

    private fun readImageBounds(file: File): Pair<Int, Int> {
        return try {
            val opts = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(file.absolutePath, opts)
            opts.outWidth.coerceAtLeast(0) to opts.outHeight.coerceAtLeast(0)
        } catch (_: Throwable) {
            0 to 0
        }
    }

    companion object {
        const val AUTHORITY = "me.edfork.workspace.clipprovider"
        const val METHOD_PREPARE = "prepare"
        const val KEY_MIME = "mime"
        private const val CACHE_SUBDIR = "clip_images"
        private const val PATH_SEGMENT_IMAGE = "image"
        private const val MAX_CACHED_IMAGES = 8
        private const val COL_MIME_TYPE = "mime_type"
        private const val COL_WIDTH = "width"
        private const val COL_HEIGHT = "height"
        private const val COL_DATA = "_data"

        private val SAFE_IMAGE_NAME_REGEX = Regex("^[A-Za-z0-9._-]+\\.(png|jpe?g|webp|gif)$", RegexOption.IGNORE_CASE)

        /**
         * Extracts and validates the image file name from a provider path `/image/<name>`.
         * Rejects traversal, nested paths, or unsupported extensions.
         */
        fun parseValidImageFileName(uriPath: String?): String? {
            if (uriPath.isNullOrBlank()) return null
            val prefix = "/$PATH_SEGMENT_IMAGE/"
            if (!uriPath.startsWith(prefix)) return null
            val fileName = uriPath.removePrefix(prefix)
            if (fileName.contains('/') || fileName.contains('\\') || fileName.contains("..")) {
                return null
            }
            return if (SAFE_IMAGE_NAME_REGEX.matches(fileName)) fileName else null
        }

        /** Maps a validated image file name extension to its canonical MIME type. */
        fun mimeForFileName(fileName: String): String {
            val lower = fileName.lowercase(Locale.ROOT)
            return when {
                lower.endsWith(".png") -> "image/png"
                lower.endsWith(".webp") -> "image/webp"
                lower.endsWith(".gif") -> "image/gif"
                else -> "image/jpeg"
            }
        }

        private fun normalizeMime(mime: String?): String {
            return when (mime?.lowercase(Locale.ROOT)?.trim()) {
                "image/png" -> "image/png"
                "image/webp" -> "image/webp"
                "image/gif" -> "image/gif"
                else -> "image/jpeg"
            }
        }

        private fun extensionForMime(mime: String): String {
            return when (mime) {
                "image/png" -> "png"
                "image/webp" -> "webp"
                "image/gif" -> "gif"
                else -> "jpg"
            }
        }
    }
}
