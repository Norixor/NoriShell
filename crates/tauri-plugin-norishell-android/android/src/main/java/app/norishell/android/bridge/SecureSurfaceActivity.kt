package app.norishell.android.bridge

import android.annotation.SuppressLint
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.View
import android.view.WindowManager
import android.webkit.GeolocationPermissions
import android.webkit.PermissionRequest
import android.webkit.RenderProcessGoneDetail
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.FrameLayout
import androidx.activity.OnBackPressedCallback
import androidx.appcompat.app.AppCompatActivity
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import org.json.JSONObject
import java.io.ByteArrayInputStream

/**
 * Hosts Core-owned Secure Surfaces (secure-*.html, tool-window.html,
 * plugin-isolated.html), each in its own plain WebView that is never managed
 * by Tauri and has no Tauri IPC.
 *
 * - Origin `https://secure.norishell.localhost/`; every byte comes from Core
 *   (`nativeReadAsset`) with Core's CSP. Plugin guest documents
 *   (`https://norishell-plugin.localhost/<token>`) are served only to the
 *   pluginIsolated surface that owns them. Everything else is a 404: no network,
 *   no file or content access, no navigation, no new windows.
 * - The page reaches Kotlin only through the `__norishellSecure` web message
 *   listener, injected for that origin. Each message is bound to the surface it
 *   came from; its label is never taken from the page. Commands outside the
 *   kind's whitelist are rejected here before Core re-checks them.
 * - Surfaces stack (singleTop Activity): the newest is visible, back asks the
 *   page to close (its cancel path), and a destroyed surface is reported to
 *   Core exactly like a closed desktop window.
 * - Secret arguments pass through as an opaque JSON string and are never logged.
 */
class SecureSurfaceActivity : AppCompatActivity() {
    private class Surface(val spec: SecureBridge.Spec, val webView: WebView) {
        var reply: JavaScriptReplyProxy? = null
        val queued = mutableListOf<String>()
    }

    private val surfaces = LinkedHashMap<String, Surface>()
    private val mainHandler = Handler(Looper.getMainLooper())
    /** True once this Activity finishes because its last surface closed. */
    private var closedByBridge = false
    private lateinit var container: FrameLayout

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        container = FrameLayout(this)
        container.setBackgroundColor(themeBackground())
        setContentView(container)
        // Same edge-to-edge, cutout, keyboard and bar-appearance handling as
        // the main window, following the app theme.
        SystemBars.install(this, container)
        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() = requestCloseTop()
        })
        for (spec in SecureBridge.attach(this)) addOrRaise(spec)
        if (surfaces.isEmpty()) finishClosed()
    }

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        for (spec in SecureBridge.attach(this)) addOrRaise(spec)
    }

    override fun onConfigurationChanged(newConfig: android.content.res.Configuration) {
        super.onConfigurationChanged(newConfig)
        SystemBars.onConfigurationChanged(this)
    }

    override fun onDestroy() {
        SystemBars.uninstall(this)
        SecureBridge.detach(this)
        if (closedByBridge) {
            for (label in surfaces.keys.toList()) removeSurface(label, finishWhenEmpty = false)
        } else {
            // Torn down by the system: a relaunch for a configuration change the
            // manifest cannot absorb (navigation-mode overlay), or the task being
            // cleared by a launcher re-launch. Keep the prompts alive in Core and
            // show them again; pages reload their state by prompt id.
            val specs = surfaces.values.map { it.spec }
            for (surface in surfaces.values) {
                container.removeView(surface.webView)
                surface.webView.destroy()
            }
            surfaces.clear()
            if (isChangingConfigurations) {
                // The system creates the replacement itself; it attaches these.
                SecureBridge.keepForRecreate(specs)
            } else {
                SecureBridge.restore(specs)
            }
        }
        super.onDestroy()
    }

    private fun finishClosed() {
        closedByBridge = true
        finish()
    }

    /** Opens a new surface, or brings an existing label to the top. */
    fun addOrRaise(spec: SecureBridge.Spec) {
        surfaces.remove(spec.label)?.let { existing ->
            surfaces[spec.label] = existing
            existing.webView.bringToFront()
            refresh()
            return
        }
        if (!WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
            // Without an origin-bound channel the page could not answer; fail closed.
            Log.w(TAG, "web message listener unsupported; surface refused")
            SecureBridge.notifyDestroyed(spec.label)
            if (surfaces.isEmpty()) finishClosed()
            return
        }
        val webView = createWebView(spec)
        val surface = Surface(spec, webView)
        surfaces[spec.label] = surface
        container.addView(
            webView,
            FrameLayout.LayoutParams(FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT),
        )
        refresh()
        webView.loadUrl("${SecureBridge.ORIGIN}/${spec.path}")
    }

    /** Removes a surface and tells Core it is gone. */
    fun removeSurface(label: String, finishWhenEmpty: Boolean = true) {
        val surface = surfaces.remove(label) ?: return
        surface.reply = null
        container.removeView(surface.webView)
        surface.webView.stopLoading()
        surface.webView.destroy()
        SecureBridge.notifyDestroyed(label)
        if (finishWhenEmpty) {
            if (surfaces.isEmpty()) finishClosed() else refresh()
        }
    }

    /** Core -> page event, delivered once the page has said it is ready. */
    fun postEvent(label: String, event: String, payloadJson: String) {
        val surface = surfaces[label] ?: return
        val message = "{\"type\":\"event\",\"event\":${JSONObject.quote(event)},\"payload\":$payloadJson}"
        send(surface, message)
    }

    private fun send(surface: Surface, message: String) {
        val reply = surface.reply
        if (reply == null) surface.queued.add(message) else reply.postMessage(message)
    }

    private fun requestCloseTop() {
        val top = surfaces.values.lastOrNull()
        if (top == null) {
            finishClosed()
            return
        }
        if (top.reply == null) {
            // The page never connected; there is no cancel path to run.
            removeSurface(top.spec.label)
        } else {
            send(top, CLOSE_REQUESTED)
        }
    }

    /** Shows only the newest surface and applies FLAG_SECURE while any surface needs it. */
    private fun refresh() {
        val top = surfaces.values.lastOrNull()
        for (surface in surfaces.values) {
            surface.webView.visibility = if (surface === top) View.VISIBLE else View.GONE
        }
        top?.let { title = it.spec.title }
        if (surfaces.values.any { it.spec.contentProtected }) {
            window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        } else {
            window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
        }
    }

    @SuppressLint("SetJavaScriptEnabled")
    private fun createWebView(spec: SecureBridge.Spec): WebView {
        val webView = WebView(this)
        webView.setBackgroundColor(Color.TRANSPARENT)
        webView.settings.apply {
            javaScriptEnabled = true
            domStorageEnabled = true
            allowFileAccess = false
            allowContentAccess = false
            javaScriptCanOpenWindowsAutomatically = false
            setSupportMultipleWindows(false)
            setGeolocationEnabled(false)
            mediaPlaybackRequiresUserGesture = true
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            cacheMode = WebSettings.LOAD_NO_CACHE
            builtInZoomControls = false
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) safeBrowsingEnabled = false
        }
        webView.setDownloadListener { _, _, _, _, _ -> }
        WebViewCompat.addWebMessageListener(webView, CHANNEL, setOf(SecureBridge.ORIGIN)) {
                view, message, sourceOrigin, isMainFrame, replyProxy ->
            onPageMessage(spec.label, view, message, sourceOrigin, isMainFrame, replyProxy)
        }
        webView.webViewClient = SurfaceClient(spec)
        webView.webChromeClient = object : WebChromeClient() {
            override fun onCreateWindow(view: WebView?, dialog: Boolean, gesture: Boolean, result: android.os.Message?) =
                false

            override fun onShowFileChooser(
                view: WebView?,
                callback: ValueCallback<Array<Uri>>?,
                params: FileChooserParams?,
            ): Boolean {
                callback?.onReceiveValue(null)
                return true
            }

            override fun onGeolocationPermissionsShowPrompt(origin: String?, callback: GeolocationPermissions.Callback?) {
                callback?.invoke(origin, false, false)
            }

            override fun onPermissionRequest(request: PermissionRequest?) {
                request?.deny()
            }
        }
        return webView
    }

    private fun onPageMessage(
        label: String,
        view: WebView,
        message: WebMessageCompat,
        sourceOrigin: Uri,
        isMainFrame: Boolean,
        replyProxy: JavaScriptReplyProxy,
    ) {
        val surface = surfaces[label] ?: return
        if (surface.webView !== view || !isMainFrame || sourceOrigin.toString() != SecureBridge.ORIGIN) return
        val text = message.data ?: return
        val request = try {
            JSONObject(text)
        } catch (error: Exception) {
            return
        }
        when (request.optString("type")) {
            "ready" -> {
                surface.reply = replyProxy
                surface.queued.forEach { replyProxy.postMessage(it) }
                surface.queued.clear()
            }
            "close" -> removeSurface(label)
            "invoke" -> invoke(surface, replyProxy, request)
        }
    }

    private fun invoke(surface: Surface, replyProxy: JavaScriptReplyProxy, request: JSONObject) {
        val id = request.optLong("id", -1)
        if (id < 0) return
        val command = request.optString("cmd")
        if (command !in SecureBridge.COMMANDS[surface.spec.kind].orEmpty()) {
            replyProxy.postMessage(response(id, DENIED))
            return
        }
        // Pre-serialized by the page so values pass through byte for byte.
        val arguments = request.optString("args", "{}")
        val label = surface.spec.label
        SecureBridge.executor.execute {
            val result = try {
                NoriShellNative.nativeSecureDispatch(label, command, arguments)
            } catch (error: Throwable) {
                UNAVAILABLE
            }
            mainHandler.post {
                // Drop the reply when the surface went away meanwhile.
                if (surfaces[label] === surface) replyProxy.postMessage(response(id, result))
            }
        }
    }

    private fun response(id: Long, resultJson: String) = "{\"type\":\"response\",\"id\":$id,\"result\":$resultJson}"

    private fun themeBackground(): Int {
        val value = android.util.TypedValue()
        return if (theme.resolveAttribute(android.R.attr.colorBackground, value, true)) value.data else Color.WHITE
    }

    private inner class SurfaceClient(private val spec: SecureBridge.Spec) : WebViewClient() {
        // Navigation away from the Core-chosen document is never allowed.
        override fun shouldOverrideUrlLoading(view: WebView?, request: WebResourceRequest?) = true

        override fun shouldInterceptRequest(view: WebView?, request: WebResourceRequest?): WebResourceResponse {
            val url = request?.url ?: return notFound()
            if (url.scheme != "https" || url.port != -1) return notFound()
            val framed = when (url.host) {
                SecureBridge.SECURE_HOST -> NoriShellNative.nativeReadAsset(url.path ?: "/")
                SecureBridge.PLUGIN_DOCUMENT_HOST ->
                    if (spec.kind == SecureBridge.PLUGIN_ISOLATED) {
                        NoriShellNative.nativeIsolatedDocument(spec.label, url.path ?: "/")
                    } else {
                        null
                    }
                else -> null
            }
            val asset = NoriShellNative.decodeAsset(framed) ?: return notFound()
            val mime = asset.mimeType.substringBefore(';').trim()
            val textual = mime.startsWith("text/") || mime.contains("javascript") || mime.contains("json") ||
                mime.contains("svg")
            val response = WebResourceResponse(mime, if (textual) "utf-8" else null, ByteArrayInputStream(asset.body))
            response.setStatusCodeAndReasonPhrase(asset.status, if (asset.status == 200) "OK" else "Error")
            response.responseHeaders = asset.headers
            return response
        }

        override fun onRenderProcessGone(view: WebView?, detail: RenderProcessGoneDetail?): Boolean {
            // Only this surface is lost; Core cancels its pending interaction.
            removeSurface(spec.label)
            return true
        }

        private fun notFound() = WebResourceResponse("text/plain", "utf-8", 404, "Not Found", emptyMap(), ByteArrayInputStream(ByteArray(0)))
    }

    companion object {
        private const val TAG = "NoriShellSecure"
        private const val CHANNEL = "__norishellSecure"
        private const val CLOSE_REQUESTED = "{\"type\":\"closeRequested\"}"
        private const val DENIED = "{\"ok\":false,\"error\":\"secure_surface.denied\"}"
        private const val UNAVAILABLE = "{\"ok\":false,\"error\":\"secure_surface.unavailable\"}"
    }
}
