package app.norishell.android.bridge

import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.os.SystemClock
import android.provider.OpenableColumns
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.FileNotFoundException
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * Storage Access Framework I/O for user-selected documents.
 *
 * Core receives `content://` URIs from the system document picker and calls
 * `contentUriOpen` to obtain a detached file descriptor. File bytes are then
 * read or written by Rust directly; they never cross JNI as JSON and never
 * reach a WebView.
 *
 * `contentUriOpen` payload `{uri, mode}` where `mode` is `r` (read) or `wt`
 * (write, truncate). Resolves `{fd, displayName, size}`; `size` is -1 when the
 * provider does not report it. The caller owns `fd` and must close it.
 *
 * `contentUriQuery` payload `{uri}` resolves `{displayName, size}` without
 * opening the document (a save target must not be truncated early).
 *
 * Reject codes:
 * - `content_access.invalid_request`: missing/unknown mode or malformed URI
 * - `content_access.uri_rejected`: not a `content` URI, or a provider of this
 *   app (its FileProvider would otherwise expose app-private files)
 * - `content_access.not_found`: provider or document is gone
 * - `content_access.permission_denied`: no grant for this URI
 * - `content_access.open_failed`: anything else
 *
 * No storage permission is requested; access relies on the per-URI grant the
 * picker result carries. Persistable grants are not taken.
 *
 * Held documents: a grant delivered with an activity result belongs to the
 * Activity that received it and is revoked when that Activity is destroyed.
 * Core's own pickers (ContentPicker.kt) receive results in a short-lived host
 * Activity, while Core may only open the document minutes later (after a
 * password prompt, for example). So the host opens the document in the mode
 * Core will need while the grant is alive ([hold]) and keeps that descriptor
 * here; the first matching `contentUriOpen` hands it over, `contentUriQuery`
 * answers from the captured metadata. Held descriptors are closed after
 * [HOLD_TTL_MS] if Core never asks for them.
 */
object ContentUriIo {
    // Providers may block (cloud documents download on open), so opening never
    // runs on the UI thread that delivered the command.
    internal val executor: ExecutorService = Executors.newCachedThreadPool { runnable ->
        Thread(runnable, "NoriShellContentIo").apply { isDaemon = true }
    }

    private class Held(
        val uri: String,
        val mode: String,
        val descriptor: ParcelFileDescriptor,
        val displayName: String?,
        val size: Long,
        val expiresAt: Long,
    )

    private val held = mutableListOf<Held>()
    private val heldLock = Any()
    private val handler = Handler(Looper.getMainLooper())

    /**
     * Opens [uri] in [mode] while the caller still holds the picker grant and
     * keeps the descriptor for Core. Blocks (providers may download); never call
     * it on the UI thread. Returns false when the document cannot be opened.
     */
    fun hold(context: Context, uri: Uri, mode: String): Boolean {
        // Same provider rules as `contentUriOpen`: never this app's own providers.
        val authority = uri.authority
        if (uri.scheme != "content" || authority.isNullOrEmpty()) return false
        val provider = try {
            context.packageManager.resolveContentProvider(authority, 0)
        } catch (error: Exception) {
            null
        }
        if (provider == null || provider.packageName == context.packageName) return false
        val descriptor = try {
            context.contentResolver.openFileDescriptor(uri, mode)
        } catch (error: Exception) {
            null
        } ?: return false
        val (displayName, size) = metadata(context, uri)
        val entry = Held(uri.toString(), mode, descriptor, displayName, size, SystemClock.uptimeMillis() + HOLD_TTL_MS)
        val evicted = synchronized(heldLock) {
            // Only the newest picks matter; older unclaimed ones are closed.
            val dropped = mutableListOf<Held>()
            held.filter { it.uri == entry.uri && it.mode == entry.mode }.forEach {
                held.remove(it)
                dropped.add(it)
            }
            while (held.size >= MAX_HELD) dropped.add(held.removeAt(0))
            held.add(entry)
            dropped
        }
        evicted.forEach { closeQuietly(it.descriptor) }
        handler.postDelayed({ expire() }, HOLD_TTL_MS + 1_000)
        return true
    }

    private fun take(uri: String, mode: String): Held? = synchronized(heldLock) {
        val index = held.indexOfFirst { it.uri == uri && it.mode == mode }
        if (index < 0) null else held.removeAt(index)
    }

    private fun peek(uri: String): Held? = synchronized(heldLock) { held.lastOrNull { it.uri == uri } }

    private fun expire() {
        val now = SystemClock.uptimeMillis()
        val expired = synchronized(heldLock) {
            val gone = held.filter { it.expiresAt <= now }
            held.removeAll(gone)
            gone
        }
        expired.forEach { closeQuietly(it.descriptor) }
    }

    private fun closeQuietly(descriptor: ParcelFileDescriptor) {
        try {
            descriptor.close()
        } catch (ignored: Exception) {
        }
    }

    fun open(context: Context, invoke: Invoke) {
        val args = invoke.getArgs()
        val uriText = args.optString("uri", "")
        val mode = args.optString("mode", "")
        if (uriText.isEmpty() || uriText.length > MAX_URI_LENGTH || (mode != "r" && mode != "wt")) {
            invoke.reject("invalid content request", "content_access.invalid_request")
            return
        }
        take(uriText, mode)?.let { entry ->
            resolveDescriptor(invoke, entry.descriptor, entry.displayName, entry.size)
            return
        }
        val application = context.applicationContext
        executor.execute { openOnWorker(application, invoke, uriText, mode) }
    }

    fun query(context: Context, invoke: Invoke) {
        val uriText = invoke.getArgs().optString("uri", "")
        if (uriText.isEmpty() || uriText.length > MAX_URI_LENGTH) {
            invoke.reject("invalid content request", "content_access.invalid_request")
            return
        }
        val application = context.applicationContext
        executor.execute {
            val uri = validatedUri(application, invoke, uriText) ?: return@execute
            val cached = peek(uriText)
            val (displayName, size) = if (cached != null) {
                Pair(cached.displayName, cached.size)
            } else {
                metadata(application, uri)
            }
            val result = JSObject()
            result.put("displayName", displayName)
            result.put("size", size)
            invoke.resolve(result)
        }
    }

    /** Parses and vets the URI; rejects `invoke` and returns null when refused. */
    private fun validatedUri(context: Context, invoke: Invoke, uriText: String): Uri? {
        val uri = try {
            Uri.parse(uriText)
        } catch (error: Exception) {
            invoke.reject("invalid content uri", "content_access.invalid_request")
            return null
        }
        val authority = uri.authority
        if (uri.scheme != "content" || authority.isNullOrEmpty()) {
            invoke.reject("content uri rejected", "content_access.uri_rejected")
            return null
        }
        val provider = try {
            context.packageManager.resolveContentProvider(authority, 0)
        } catch (error: Exception) {
            null
        }
        if (provider == null) {
            invoke.reject("content provider missing", "content_access.not_found")
            return null
        }
        if (provider.packageName == context.packageName) {
            invoke.reject("content uri rejected", "content_access.uri_rejected")
            return null
        }
        return uri
    }

    private fun metadata(context: Context, uri: Uri): Pair<String?, Long> {
        var displayName: String? = null
        var size = -1L
        try {
            context.contentResolver.query(
                uri,
                arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE),
                null,
                null,
                null,
            )?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (nameIndex >= 0 && !cursor.isNull(nameIndex)) {
                        displayName = cursor.getString(nameIndex)
                    }
                    val sizeIndex = cursor.getColumnIndex(OpenableColumns.SIZE)
                    if (sizeIndex >= 0 && !cursor.isNull(sizeIndex)) {
                        size = cursor.getLong(sizeIndex)
                    }
                }
            }
        } catch (error: Exception) {
            // Metadata is optional; the descriptor itself is authoritative.
        }
        return Pair(displayName, size)
    }

    private fun openOnWorker(context: Context, invoke: Invoke, uriText: String, mode: String) {
        val uri = validatedUri(context, invoke, uriText) ?: return
        val descriptor: ParcelFileDescriptor? = try {
            context.contentResolver.openFileDescriptor(uri, mode)
        } catch (error: FileNotFoundException) {
            invoke.reject("content document missing", "content_access.not_found")
            return
        } catch (error: SecurityException) {
            invoke.reject("content access denied", "content_access.permission_denied")
            return
        } catch (error: Exception) {
            invoke.reject("content open failed", "content_access.open_failed")
            return
        }
        if (descriptor == null) {
            invoke.reject("content open failed", "content_access.open_failed")
            return
        }
        val (displayName, size) = metadata(context, uri)
        resolveDescriptor(invoke, descriptor, displayName, size)
    }

    private fun resolveDescriptor(invoke: Invoke, descriptor: ParcelFileDescriptor, displayName: String?, size: Long) {
        val fd = try {
            descriptor.detachFd()
        } catch (error: Exception) {
            try {
                descriptor.close()
            } catch (ignored: Exception) {
            }
            invoke.reject("content open failed", "content_access.open_failed")
            return
        }
        val result = JSObject()
        result.put("fd", fd)
        result.put("displayName", displayName)
        result.put("size", size)
        invoke.resolve(result)
    }

    private const val MAX_URI_LENGTH = 4096
    private const val MAX_HELD = 4
    /** Long enough for a password prompt between the pick and Core's open. */
    private const val HOLD_TTL_MS = 15 * 60 * 1000L
}
