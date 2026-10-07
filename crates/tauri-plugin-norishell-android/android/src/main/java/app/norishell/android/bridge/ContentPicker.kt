package app.norishell.android.bridge

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.lang.ref.WeakReference

/**
 * Core-initiated system document pickers: `contentPick` (ACTION_OPEN_DOCUMENT)
 * and `contentCreate` (ACTION_CREATE_DOCUMENT, the save dialog).
 *
 * The dialog plugin starts the picker from MainActivity and its result is held
 * by MainActivity's result registry until MainActivity is started again. When a
 * Secure Surface or tool window covers MainActivity that never happens, so a pick
 * started from there spun forever. Here the picker is launched by
 * [ContentPickerActivity], a translucent Activity placed on top of whatever is in
 * front (main WebView or a surface); it is resumed again as soon as the picker
 * closes, so the result always arrives, and finishing it reveals the surface that
 * asked.
 *
 * - One pick at a time (`content_access.pick_busy`).
 * - Time bounded: after [TIMEOUT_MS] the picker is closed and the pick fails
 *   with `content_access.pick_timeout`.
 * - Resolves `{uri: String|null}`; `null` means the user cancelled or the
 *   picker went away without a result. The result grant belongs to the host
 *   Activity, so the document is opened before the host finishes and handed to
 *   Core through ContentUriIo's held documents (no persistable permission).
 *   Fails with `content_access.open_failed` when that open fails. The URI is
 *   never logged.
 *
 * UI thread only.
 */
object ContentPicker {
    private const val TAG = "NoriShellPicker"
    private const val TIMEOUT_MS = 5 * 60 * 1000L
    internal const val EXTRA_TOKEN = "app.norishell.android.extra.PICK_TOKEN"
    internal const val EXTRA_MIME_TYPES = "app.norishell.android.extra.PICK_MIME_TYPES"
    internal const val EXTRA_SUGGESTED_NAME = "app.norishell.android.extra.PICK_SUGGESTED_NAME"

    private class Pending(val token: Long, val invoke: Invoke) {
        var activity: WeakReference<ContentPickerActivity>? = null
    }

    private val handler = Handler(Looper.getMainLooper())
    private var pending: Pending? = null
    private var nextToken = 1L

    /** `{mimeTypes: [String]}`: opens an existing document. */
    fun pick(activity: Activity, invoke: Invoke) {
        if (pending != null) {
            invoke.reject("a document pick is already open", "content_access.pick_busy")
            return
        }
        val requested = invoke.getArgs().optJSONArray("mimeTypes")
        val mimeTypes = buildList {
            if (requested != null) {
                for (index in 0 until minOf(requested.length(), 16)) {
                    val type = requested.optString(index)
                    if (MIME.matches(type)) add(type)
                }
            }
        }.ifEmpty { listOf("*/*") }
        launch(activity, invoke, mimeTypes, suggestedName = null)
    }

    /**
     * `{mimeType: String, suggestedName: String}`: the system save dialog. The
     * provider creates the (empty) document when the user confirms; Core then
     * writes it through `contentUriOpen` mode `wt`.
     */
    fun create(activity: Activity, invoke: Invoke) {
        if (pending != null) {
            invoke.reject("a document pick is already open", "content_access.pick_busy")
            return
        }
        val args = invoke.getArgs()
        val mimeType = args.optString("mimeType", "")
        val name = args.optString("suggestedName", "")
        if (!MIME.matches(mimeType) || mimeType.contains('*') || !validName(name)) {
            invoke.reject("invalid save request", "content_access.invalid_request")
            return
        }
        launch(activity, invoke, listOf(mimeType), suggestedName = name)
    }

    private fun validName(name: String) =
        name.isNotEmpty() && name.length <= 200 && name != "." && name != ".." &&
            name.none { it == '/' || it == '\\' || it.isISOControl() }

    private fun launch(activity: Activity, invoke: Invoke, mimeTypes: List<String>, suggestedName: String?) {
        val token = nextToken++
        val request = Pending(token, invoke)
        pending = request
        handler.postDelayed({ timeOut(token) }, request, TIMEOUT_MS)
        try {
            val intent = Intent(activity, ContentPickerActivity::class.java)
                .putExtra(EXTRA_TOKEN, token)
                .putExtra(EXTRA_MIME_TYPES, mimeTypes.toTypedArray())
            if (suggestedName != null) intent.putExtra(EXTRA_SUGGESTED_NAME, suggestedName)
            activity.startActivity(intent)
        } catch (error: Exception) {
            Log.w(TAG, "picker host activity failed to start")
            finish(token, error = "content_access.picker_unavailable")
        }
    }

    internal fun isPending(token: Long) = pending?.token == token

    internal fun attach(token: Long, activity: ContentPickerActivity) {
        pending?.takeIf { it.token == token }?.activity = WeakReference(activity)
    }

    /** Completes the pick once; later calls for the same token are ignored. */
    internal fun finish(token: Long, uri: String? = null, error: String? = null) {
        val request = pending ?: return
        if (request.token != token) return
        pending = null
        handler.removeCallbacksAndMessages(request)
        if (error != null) {
            request.invoke.reject("document pick failed", error)
        } else {
            request.invoke.resolve(JSObject().put("uri", uri))
        }
    }

    private fun timeOut(token: Long) {
        val request = pending ?: return
        if (request.token != token) return
        Log.w(TAG, "document pick timed out")
        val host = request.activity?.get()
        finish(token, error = "content_access.pick_timeout")
        host?.closePicker()
    }

    private val MIME = Regex("^[a-z*][a-z0-9.+*-]*/[a-zA-Z0-9.+*-]+$")
}

/** Translucent host that owns the picker request and receives its result. */
class ContentPickerActivity : Activity() {
    private var token = 0L

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        token = intent.getLongExtra(ContentPicker.EXTRA_TOKEN, 0L)
        if (!ContentPicker.isPending(token)) {
            finish()
            return
        }
        ContentPicker.attach(token, this)
        // Recreated with the picker still open: its result comes to this instance.
        if (savedInstanceState != null) return
        val mimeTypes = intent.getStringArrayExtra(ContentPicker.EXTRA_MIME_TYPES) ?: arrayOf("*/*")
        val suggestedName = intent.getStringExtra(ContentPicker.EXTRA_SUGGESTED_NAME)
        val open = if (suggestedName != null) {
            Intent(Intent.ACTION_CREATE_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType(mimeTypes[0])
                .putExtra(Intent.EXTRA_TITLE, suggestedName)
        } else {
            Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType(if (mimeTypes.size == 1) mimeTypes[0] else "*/*")
                .also { if (mimeTypes.size > 1) it.putExtra(Intent.EXTRA_MIME_TYPES, mimeTypes) }
        }
        try {
            @Suppress("DEPRECATION")
            startActivityForResult(open, REQUEST_CODE)
        } catch (error: ActivityNotFoundException) {
            ContentPicker.finish(token, error = "content_access.picker_unavailable")
            finish()
        }
    }

    @Deprecated("Activity result API of the platform Activity")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST_CODE) return
        val uri = if (resultCode == RESULT_OK) data?.data?.takeIf { it.scheme == "content" } else null
        if (uri == null || !ContentPicker.isPending(token)) {
            ContentPicker.finish(token)
            close()
            return
        }
        // The grant dies with this Activity: open the document now, in the mode
        // Core will use, and only then finish (ContentUriIo held documents).
        val mode = if (intent.hasExtra(ContentPicker.EXTRA_SUGGESTED_NAME)) "wt" else "r"
        val application = applicationContext
        ContentUriIo.executor.execute {
            val held = ContentUriIo.hold(application, uri, mode)
            runOnUiThread {
                if (held) {
                    ContentPicker.finish(token, uri = uri.toString())
                } else {
                    ContentPicker.finish(token, error = "content_access.open_failed")
                }
                close()
            }
        }
    }

    private fun close() {
        finish()
        @Suppress("DEPRECATION")
        overridePendingTransition(0, 0)
    }

    override fun onDestroy() {
        // Gone without a result (finished, or torn down by the system while
        // not recreating): the pick ends as a cancel.
        if (!isChangingConfigurations) ContentPicker.finish(token)
        super.onDestroy()
    }

    internal fun closePicker() {
        @Suppress("DEPRECATION")
        finishActivity(REQUEST_CODE)
        finish()
    }

    private companion object {
        const val REQUEST_CODE = 0x4E50
    }
}
