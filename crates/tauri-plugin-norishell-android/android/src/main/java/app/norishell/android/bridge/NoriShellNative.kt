package app.norishell.android.bridge

import android.content.Context
import org.json.JSONObject
import java.nio.ByteBuffer

/**
 * Kotlin -> Rust entry points exported by the `tauri-plugin-norishell-android`
 * crate (and, for [nativeRunPluginHost], by the app crate). All functions are
 * synchronous; except [nativeInit] they must be called off the UI thread when
 * the Rust callback may call back into Kotlin through the plugin.
 *
 * The native library is loaded by Tauri's generated `Rust.kt` in the app
 * process. Isolated processes (PluginHostService) must load
 * `norishell_desktop` themselves before calling [nativeRunPluginHost].
 */
object NoriShellNative {
    /** Initializes rustls-platform-verifier; returns false when it failed. */
    @JvmStatic external fun nativeInit(context: Context): Boolean

    /** Secure surface IPC. Returns the response JSON produced by Rust. */
    @JvmStatic external fun nativeSecureDispatch(label: String, command: String, payload: String): String

    /** Framed asset for `https://secure.norishell.localhost/<path>`, or null. */
    @JvmStatic external fun nativeReadAsset(path: String): ByteArray?

    /** Framed plugin-isolated document for a surface, or null. */
    @JvmStatic external fun nativeIsolatedDocument(label: String, path: String): ByteArray?

    /** A Kotlin-hosted surface was destroyed. */
    @JvmStatic external fun nativeSurfaceDestroyed(label: String)

    /** Notification / service action, e.g. [ACTION_EXIT_ALL]. */
    @JvmStatic external fun nativeNotificationAction(action: String)

    /**
     * Resource notification event (NotificationsBridge): `foreground` /
     * `background` (id empty; UI thread allowed) or `click` with the event id
     * (call off the UI thread).
     */
    @JvmStatic external fun nativeNotificationEvent(event: String, id: String)

    /**
     * Runs the plugin host frame loop over [fd] until it ends; returns the exit
     * code. Exported by the app crate (`src-tauri/src/android/plugin_host_jni.rs`).
     */
    @JvmStatic external fun nativeRunPluginHost(fd: Int): Int

    const val ACTION_EXIT_ALL = "exitAll"
    const val ACTION_TASK_REMOVED = "taskRemoved"

    data class Asset(
        val status: Int,
        val mimeType: String,
        val headers: Map<String, String>,
        val body: ByteArray,
    )

    /** Decodes `u32 BE header length | header JSON | body` produced by Rust. */
    @JvmStatic
    fun decodeAsset(framed: ByteArray?): Asset? {
        if (framed == null || framed.size < 4) return null
        val headerLength = ByteBuffer.wrap(framed, 0, 4).int
        if (headerLength < 0 || headerLength > framed.size - 4) return null
        val header = JSONObject(String(framed, 4, headerLength, Charsets.UTF_8))
        val headerMap = mutableMapOf<String, String>()
        val headers = header.optJSONObject("headers")
        headers?.keys()?.forEach { key -> headerMap[key] = headers.getString(key) }
        return Asset(
            status = header.optInt("status", 200),
            mimeType = header.optString("mimeType", "application/octet-stream"),
            headers = headerMap,
            body = framed.copyOfRange(4 + headerLength, framed.size),
        )
    }
}
