package app.norishell.android.bridge

import android.app.Activity
import android.content.res.Configuration
import android.graphics.Color
import android.os.Build
import android.view.View
import android.view.WindowManager
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import java.util.WeakHashMap

/**
 * Edge-to-edge window handling shared by MainActivity and SecureSurfaceActivity.
 *
 * Android 15+ always lays apps out edge to edge and ignores `adjustResize`, and
 * the WebView reports `env(safe-area-inset-*)` as 0. So the native container of
 * each WebView is padded by the system bars, the display cutout and the soft
 * keyboard: the WebView itself shrinks, the page never sits under a bar, and
 * its `visualViewport` / ResizeObserver refit xterm when the keyboard opens.
 * The insets are consumed at the container, so the WebView never adds its own
 * safe-area padding on top.
 *
 * Bar icon contrast and the color behind the bars follow the APP theme the
 * frontend resolved (Settings light/dark/system), sent through `systemBarsSet`;
 * until the first call they follow the system night mode.
 *
 * UI thread only.
 */
object SystemBars {
    private data class Style(val dark: Boolean, val background: Int)

    private var style: Style? = null
    private val installed = WeakHashMap<Activity, View>()
    private val lastNavigation = WeakHashMap<Activity, androidx.core.graphics.Insets>()
    private val imeObservers = WeakHashMap<Activity, (Boolean) -> Unit>()
    private val lastImeVisible = WeakHashMap<Activity, Boolean>()

    /**
     * Lays [activity] out edge to edge and pads [container] by every inset.
     *
     * [onImeVisibilityChanged] receives the soft keyboard's visibility whenever it
     * changes (true once the keyboard takes bottom space). The WebView cannot tell a
     * keyboard from any other viewport shrink (rotation, multi-window), so the
     * frontend hides its bottom navigation on this native fact only.
     */
    fun install(activity: Activity, container: View, onImeVisibilityChanged: ((Boolean) -> Unit)? = null) {
        if (onImeVisibilityChanged != null) imeObservers[activity] = onImeVisibilityChanged else imeObservers.remove(activity)
        lastImeVisible.remove(activity)
        val window = activity.window
        WindowCompat.setDecorFitsSystemWindows(window, false)
        @Suppress("DEPRECATION")
        run {
            // No-ops on Android 15+, where bars are always transparent.
            window.statusBarColor = Color.TRANSPARENT
            window.navigationBarColor = Color.TRANSPARENT
        }
        // The container color sits behind the bars; a system scrim would
        // fight the icon appearance chosen below.
        window.isStatusBarContrastEnforced = false
        window.isNavigationBarContrastEnforced = false
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            window.attributes = window.attributes.apply {
                layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            }
        } else {
            window.attributes = window.attributes.apply {
                layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
            }
        }
        ViewCompat.setOnApplyWindowInsetsListener(container) { view, insets ->
            // A combined query yields the largest inset per side, so the open
            // keyboard replaces (never adds to) the navigation bar padding.
            val area = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or
                    WindowInsetsCompat.Type.ime(),
            )
            view.setPadding(area.left, area.top, area.right, area.bottom)
            // A navigation-mode switch (or rotation) replaces the navigation bar
            // and SystemUI draws it with default icons; re-sending an unchanged
            // appearance is a no-op, so 3-button icons stayed invisible on a light
            // app theme until the app left the foreground. Flip and restore the
            // appearance whenever the navigation bar geometry changes.
            val navigation = insets.getInsets(WindowInsetsCompat.Type.navigationBars())
            if (lastNavigation[activity] != navigation) {
                lastNavigation[activity] = navigation
                refresh(activity, view)
            } else {
                apply(activity, view)
            }
            // The keyboard counts as visible once it occupies bottom space: that is the
            // moment the WebView shrinks and the frontend layout must follow.
            val imeVisible = insets.getInsets(WindowInsetsCompat.Type.ime()).bottom > 0
            if (lastImeVisible[activity] != imeVisible) {
                lastImeVisible[activity] = imeVisible
                imeObservers[activity]?.invoke(imeVisible)
            }
            WindowInsetsCompat.CONSUMED
        }
        installed[activity] = container
        apply(activity, container)
        ViewCompat.requestApplyInsets(container)
    }

    fun uninstall(activity: Activity) {
        installed.remove(activity)
        lastNavigation.remove(activity)
        imeObservers.remove(activity)
        lastImeVisible.remove(activity)
    }

    /** Forces SystemUI to re-read the bar appearance (see the insets listener). */
    private fun refresh(activity: Activity, container: View) {
        if (activity.isDestroyed) return
        val dark = style?.dark ?: systemNight(activity)
        val controller = WindowCompat.getInsetsController(activity.window, activity.window.decorView)
        controller.isAppearanceLightStatusBars = dark
        controller.isAppearanceLightNavigationBars = dark
        container.post { apply(activity, container) }
    }

    /** Applies the app theme to every installed window and remembers it for later ones. */
    fun update(dark: Boolean, background: Int) {
        style = Style(dark, background)
        for ((activity, container) in installed.entries.toList()) apply(activity, container)
    }

    /** Re-evaluates the system-night fallback after a uiMode change. */
    fun onConfigurationChanged(activity: Activity) {
        installed[activity]?.let { refresh(activity, it) }
    }

    private fun apply(activity: Activity, container: View) {
        if (activity.isDestroyed) return
        val current = style
        val dark = current?.dark ?: systemNight(activity)
        val controller = WindowCompat.getInsetsController(activity.window, activity.window.decorView)
        controller.isAppearanceLightStatusBars = !dark
        controller.isAppearanceLightNavigationBars = !dark
        if (current != null) {
            container.setBackgroundColor(current.background)
            activity.window.decorView.setBackgroundColor(current.background)
        }
    }

    private fun systemNight(activity: Activity) =
        (activity.resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
            Configuration.UI_MODE_NIGHT_YES

    /** Parses `#rrggbb`; anything else is refused. */
    fun parseColor(text: String?): Int? {
        if (text == null || !COLOR.matches(text)) return null
        return Color.parseColor(text)
    }

    private val COLOR = Regex("^#[0-9a-fA-F]{6}$")
}
