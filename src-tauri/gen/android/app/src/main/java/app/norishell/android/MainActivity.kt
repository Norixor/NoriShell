package app.norishell.android

import android.content.Intent
import android.content.res.Configuration
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import app.norishell.android.bridge.SessionForegroundService
import app.norishell.android.bridge.SystemBars

class MainActivity : TauriActivity() {
  private val mainHandler = Handler(Looper.getMainLooper())
  private var webView: WebView? = null
  private var backPending = false

  /**
   * Back order: the frontend closes its topmost overlay
   * (`window.__norishellHandleBack()` returns `true`), else WebView history,
   * else the task moves to the background. The activity is never finished by
   * back, so the process and its live sessions survive.
   */
  private val backCallback = object : OnBackPressedCallback(true) {
    override fun handleOnBackPressed() = onBack()
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    running = true
    onBackPressedDispatcher.addCallback(this, backCallback)
    // A relaunch (configuration change the manifest cannot absorb, e.g. a
    // navigation-mode overlay switch) keeps the process, Core and wry; the
    // vendored Tauri PluginManager moves its launchers and plugins to this
    // instance during super.onCreate.
    // The relaunch replays the original launch intent: never re-run its action
    // (it may be "disconnect all and exit").
    if (savedInstanceState == null) SessionForegroundService.handleLaunchIntent(intent)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    this.webView = webView
    // wry makes the WebView the content view; padding the content frame by the
    // system bars, cutout and keyboard shrinks the WebView instead of drawing
    // under them (Android 15+ ignores adjustResize and reports no safe-area
    // insets). The frame exists before the WebView is attached to it.
    SystemBars.install(this, findViewById<View>(android.R.id.content)) { visible -> publishImeVisibility(visible) }
    // The dispatcher runs the newest callback first. Tauri's AppPlugin adds its
    // own history callback before the WebView exists; re-adding ours puts the
    // overlay check in front of it.
    backCallback.remove()
    onBackPressedDispatcher.addCallback(this, backCallback)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    SessionForegroundService.handleLaunchIntent(intent)
  }

  override fun onConfigurationChanged(newConfig: Configuration) {
    super.onConfigurationChanged(newConfig)
    SystemBars.onConfigurationChanged(this)
  }

  override fun onDestroy() {
    SystemBars.uninstall(this)
    running = false
    mainHandler.removeCallbacksAndMessages(null)
    // On a relaunch wry builds a fresh WebView for the new Activity but keeps
    // the old one alive, detached: a second copy of the frontend would keep
    // running against Core. Destroy it once wry has seen the teardown.
    val stale = if (isChangingConfigurations) webView else null
    webView = null
    super.onDestroy()
    stale?.let { view ->
      (view.parent as? ViewGroup)?.removeView(view)
      view.destroy()
    }
  }

  private fun onBack() {
    val view = webView
    if (view == null) {
      moveTaskToBack(true)
      return
    }
    // A press while the frontend is still answering is dropped, not queued:
    // it must not skip past an overlay the first press is closing.
    if (backPending) return
    backPending = true
    var settled = false
    val settle = { consumed: Boolean ->
      if (!settled) {
        settled = true
        backPending = false
        mainHandler.removeCallbacksAndMessages(BACK_TIMEOUT_TOKEN)
        if (!consumed && !isFinishing && !isDestroyed) fallBack(view)
      }
    }
    mainHandler.postAtTime({ settle(false) }, BACK_TIMEOUT_TOKEN,
      android.os.SystemClock.uptimeMillis() + BACK_TIMEOUT_MS)
    view.evaluateJavascript(HANDLE_BACK_SCRIPT) { result -> settle(result == "true") }
  }

  /**
   * Mirrors the soft keyboard's visibility into the page: `html[data-nvx-ime="visible"]`
   * plus a `norishell:ime-visibility` event (`detail.visible`). The page cannot derive it
   * from its viewport alone; the bottom navigation hides on it so the content and the
   * terminal helper keys sit directly on the keyboard. A reload drops the attribute, and
   * the next inset change (the keyboard closes with the reload) publishes it again.
   */
  private fun publishImeVisibility(visible: Boolean) {
    val view = webView ?: return
    if (isFinishing || isDestroyed) return
    val flag = if (visible) "true" else "false"
    view.evaluateJavascript(
      "(function(){var r=document.documentElement;if(!r)return;" +
        "if($flag)r.setAttribute('data-nvx-ime','visible');else r.removeAttribute('data-nvx-ime');" +
        "window.dispatchEvent(new CustomEvent('norishell:ime-visibility',{detail:{visible:$flag}}));})()",
      null,
    )
  }

  private fun fallBack(view: WebView) {
    if (view.canGoBack()) {
      view.goBack()
    } else {
      moveTaskToBack(true)
    }
  }

  companion object {
    /** Whether a MainActivity (and with it the loaded Core) lives in this process. */
    @Volatile
    @JvmStatic
    var running = false
      private set

    private const val BACK_TIMEOUT_MS = 300L
    private val BACK_TIMEOUT_TOKEN = Any()
    // Only a synchronous boolean `true` consumes the press; a missing
    // function, a throw or any other value falls through.
    private const val HANDLE_BACK_SCRIPT =
      "(function(){try{return typeof window.__norishellHandleBack==='function'" +
        "&&window.__norishellHandleBack()===true}catch(e){return false}})()"
  }
}
