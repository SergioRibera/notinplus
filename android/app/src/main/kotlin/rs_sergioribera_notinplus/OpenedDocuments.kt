package rs.sergioribera.notinplus

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.util.Log
import androidx.core.content.IntentCompat
import dev.istmo.runtime.Bincode
import dev.istmo.runtime.IstmoRuntime
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.UUID
import kotlin.concurrent.thread

/**
 * Files other apps hand to NotIn Plus (`ACTION_VIEW` from a file
 * manager, `ACTION_SEND` from a share sheet) go to the Rust side on the
 * `istmo.documents` early-event queue, so one that arrives before the UI
 * listens is kept until it does.
 *
 * Rust reads plain paths, not `content://` URIs: each file is copied
 * into the cache first. The copy is what the app opens; changes are not
 * written back to the original, and copies older than a day are removed
 * on the next open.
 */
object OpenedDocuments {
    private const val TAG = "OpenedDocuments"
    private const val CHANNEL = "istmo.documents"
    private const val CAPACITY = 32
    private const val CACHE_DIR = "opened"
    private const val MAX_AGE_MS = 24L * 60 * 60 * 1000

    /** Ceiling on a copied file, so a huge share cannot fill the cache. */
    private const val MAX_BYTES = 512L * 1024 * 1024

    // Wire discriminants of `istmo_plugins::DocumentOrigin`.
    const val ORIGIN_LAUNCH = 0
    const val ORIGIN_RUNNING = 1
    const val ORIGIN_SHARE = 2

    // `istmo_plugins::DocumentLocation::Path`.
    private const val LOCATION_PATH = 0

    /** Whether `intent` carries a document (not a `notein://` link). */
    fun handles(intent: Intent?): Boolean {
        if (intent == null) return false
        return when (intent.action) {
            Intent.ACTION_VIEW -> intent.data?.scheme.let { it == "content" || it == "file" }
            Intent.ACTION_SEND -> streamOf(intent) != null
            else -> false
        }
    }

    /** Copy the document of `intent` off the main thread and publish it. */
    fun publish(context: Context, intent: Intent, origin: Int) {
        val uri = (if (intent.action == Intent.ACTION_SEND) streamOf(intent) else intent.data) ?: return
        val shareOrigin = if (intent.action == Intent.ACTION_SEND) ORIGIN_SHARE else origin
        val app = context.applicationContext
        thread(name = "opened-documents") {
            try {
                val root = File(app.cacheDir, CACHE_DIR)
                prune(root)
                val name = safeName(displayName(app, uri))
                val mime = intent.type ?: app.contentResolver.getType(uri)
                val copy = File(File(root, UUID.randomUUID().toString()).apply { mkdirs() }, name)
                copyLimited(app, uri, copy)
                IstmoRuntime.publishEarlyQueue(CHANNEL, CAPACITY, encode(copy, name, mime, shareOrigin))
            } catch (err: Exception) {
                Log.w(TAG, "could not open $uri", err)
            }
        }
    }

    private fun streamOf(intent: Intent): Uri? =
        IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java)

    private fun displayName(context: Context, uri: Uri): String {
        if (uri.scheme == "content") {
            context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
                ?.use { cursor ->
                    if (cursor.moveToFirst()) {
                        cursor.getString(0)?.let { return it }
                    }
                }
        }
        return uri.lastPathSegment ?: "document"
    }

    /** A provider's display name is untrusted: keep it a single file name. */
    private fun safeName(raw: String): String {
        val name = raw.replace('/', '_').replace('\\', '_').replace('\u0000', '_').trimStart('.')
        return name.ifBlank { "document" }.take(200)
    }

    private fun copyLimited(context: Context, uri: Uri, target: File) {
        val input = context.contentResolver.openInputStream(uri) ?: error("no stream for $uri")
        input.use { src ->
            target.outputStream().use { dst ->
                val buffer = ByteArray(64 * 1024)
                var total = 0L
                while (true) {
                    val read = src.read(buffer)
                    if (read < 0) break
                    total += read
                    if (total > MAX_BYTES) {
                        target.delete()
                        error("file larger than $MAX_BYTES bytes")
                    }
                    dst.write(buffer, 0, read)
                }
            }
        }
    }

    private fun prune(root: File) {
        val cutoff = System.currentTimeMillis() - MAX_AGE_MS
        root.listFiles()?.filter { it.lastModified() < cutoff }?.forEach { it.deleteRecursively() }
    }

    /** `istmo_plugins::OpenedDocument`, field by field. */
    private fun encode(file: File, name: String, mime: String?, origin: Int): ByteArray {
        val out = ByteArrayOutputStream()
        Bincode.writeEnumDiscriminant(out, LOCATION_PATH)
        Bincode.writeString(out, file.absolutePath)
        Bincode.writeOption(out, name) { o, s -> Bincode.writeString(o, s) }
        Bincode.writeOption(out, mime) { o, s -> Bincode.writeString(o, s) }
        Bincode.writeEnumDiscriminant(out, origin)
        Bincode.writeOption(out, null as String?) { o, s -> Bincode.writeString(o, s) }
        return out.toByteArray()
    }
}
