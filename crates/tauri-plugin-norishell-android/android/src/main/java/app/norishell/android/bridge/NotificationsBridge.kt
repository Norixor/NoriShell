package app.norishell.android.bridge

import android.Manifest
import android.app.Activity
import android.app.Application
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.core.app.ActivityCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.util.Collections
import java.util.IdentityHashMap
import kotlin.concurrent.thread

/**
 * Resource notifications (unexpected disconnect, failed forward, transfer
 * completed/failed) for Rust's `NativeNotificationService`.
 *
 * - Rust owns every decision (preferences, pause, rate limit, background-only
 *   rule, click records). Kotlin only reports permission, posts the localized
 *   non-secret text Rust sends, and reports taps by the host-owned event id.
 * - Foreground signal: an Activity lifecycle callback tracks resumed
 *   Activities of the app process (MainActivity and Kotlin surfaces) and
 *   stores the result in Rust (`nativeNotificationEvent("foreground"|"background")`).
 *   tao window focus is always false on Android and cannot be used.
 * - Tap: the content intent reopens the launcher Activity with [EXTRA_EVENT_ID];
 *   [handleIntent] consumes it and calls Rust off the UI thread. Rust ignores
 *   ids it did not issue in this process.
 *
 * Commands run on the UI thread (Tauri plugin commands).
 */
object NotificationsBridge {
    private const val TAG = "NoriShellNotify"
    private const val ALERTS_CHANNEL = "norishell.alerts"
    private const val TRANSFERS_CHANNEL = "norishell.transfers"
    /** Every resource notification shares this id; the event id is the tag. */
    private const val RESOURCE_NOTIFICATION_ID = 0x4E5302
    private const val PERMISSION_REQUEST_CODE = 0x4E54
    private const val PREFS = "norishell.notifications"
    private const val PREF_REQUESTED = "permissionRequested"
    private const val EXTRA_EVENT_ID = "app.norishell.android.extra.NOTIFICATION_EVENT"
    private const val ID_PREFIX = "norishell-notification:"
    private val EVENT_ID = Regex("^[A-Za-z0-9_-]{1,128}$")

    // Permission request polling: no visible system dialog within this window
    // means Android answered without asking (already granted or permanently denied).
    private const val POLL_MS = 200L
    private const val NO_DIALOG_MS = 1500L
    private const val REQUEST_LIMIT_MS = 60_000L

    private val main = Handler(Looper.getMainLooper())
    // UI thread only.
    private val resumed = Collections.newSetFromMap(IdentityHashMap<Activity, Boolean>())
    private var installed = false
    private var pendingRequest: PendingRequest? = null

    private class PendingRequest(val invoke: Invoke, val startedAt: Long) {
        var sawPause = false
    }

    /** Called from the plugin constructor (UI thread) with the hosting Activity. */
    fun install(activity: Activity) {
        if (installed) return
        installed = true
        activity.application.registerActivityLifecycleCallbacks(object :
            Application.ActivityLifecycleCallbacks {
            override fun onActivityResumed(activity: Activity) {
                resumed.add(activity)
                publishForeground()
            }

            override fun onActivityPaused(activity: Activity) {
                resumed.remove(activity)
                pendingRequest?.sawPause = true
                publishForeground()
            }

            override fun onActivityDestroyed(activity: Activity) {
                if (resumed.remove(activity)) publishForeground()
            }

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}
            override fun onActivityStarted(activity: Activity) {}
            override fun onActivityStopped(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
        })
        // The plugin may be created after MainActivity already resumed.
        if (activity is LifecycleOwner &&
            activity.lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)
        ) {
            resumed.add(activity)
        }
        publishForeground()
    }

    private fun publishForeground() {
        val event = if (resumed.isEmpty()) "background" else "foreground"
        try {
            // Only an atomic store in Rust; safe on the UI thread.
            NoriShellNative.nativeNotificationEvent(event, "")
        } catch (error: Throwable) {
            Log.w(TAG, "foreground signal failed")
        }
    }

    /** Consumes a notification tap delivered to the launcher Activity. */
    fun handleIntent(intent: Intent?): Boolean {
        val id = intent?.getStringExtra(EXTRA_EVENT_ID) ?: return false
        intent.removeExtra(EXTRA_EVENT_ID)
        if (!EVENT_ID.matches(id)) return false
        // The Rust click path may call back into Kotlin; never on the UI thread.
        thread(name = "norishell-notification-click") {
            try {
                NoriShellNative.nativeNotificationEvent("click", id)
            } catch (error: Throwable) {
                Log.w(TAG, "notification click dispatch failed")
            }
        }
        return true
    }

    // ---- Commands ----

    fun permission(activity: Activity, invoke: Invoke) {
        invoke.resolve(JSObject().put("permission", permissionState(activity)))
    }

    /**
     * Shows the POST_NOTIFICATIONS dialog (Android 13+) when it can still be
     * shown and resolves with the state after the user answered. Older
     * releases have no runtime prompt: the current state is returned.
     */
    fun requestPermission(activity: Activity, invoke: Invoke) {
        val current = permissionState(activity)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU || current != "notDetermined") {
            invoke.resolve(JSObject().put("permission", current))
            return
        }
        if (pendingRequest != null) {
            invoke.reject("permission request in progress", "notifications.busy")
            return
        }
        prefs(activity).edit().putBoolean(PREF_REQUESTED, true).apply()
        val request = PendingRequest(invoke, SystemClock.uptimeMillis())
        pendingRequest = request
        ActivityCompat.requestPermissions(
            activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), PERMISSION_REQUEST_CODE,
        )
        main.postDelayed({ pollRequest(activity, request) }, POLL_MS)
    }

    private fun pollRequest(activity: Activity, request: PendingRequest) {
        if (pendingRequest !== request) return
        val elapsed = SystemClock.uptimeMillis() - request.startedAt
        val granted = hasPostPermission(activity)
        val answered = request.sawPause && resumed.isNotEmpty()
        val noDialog = !request.sawPause && elapsed >= NO_DIALOG_MS
        if (granted || answered || noDialog || elapsed >= REQUEST_LIMIT_MS) {
            pendingRequest = null
            request.invoke.resolve(JSObject().put("permission", permissionState(activity)))
            return
        }
        main.postDelayed({ pollRequest(activity, request) }, POLL_MS)
    }

    /** `{id, title, body, channel: "alerts"|"transfers"}` -> `{}`. */
    fun post(activity: Activity, invoke: Invoke) {
        val args = invoke.getArgs()
        val id = args.optString("id", "")
        val title = args.optString("title", "")
        val body = args.optString("body", "")
        val channel = when (args.optString("channel", "")) {
            "alerts" -> ALERTS_CHANNEL
            "transfers" -> TRANSFERS_CHANNEL
            else -> null
        }
        if (!EVENT_ID.matches(id) || title.isEmpty() || channel == null) {
            invoke.reject("invalid notification", "notifications.invalid_content")
            return
        }
        val context = activity.applicationContext
        if (permissionState(activity) != "granted") {
            invoke.reject("notifications not permitted", "notifications.permission_denied")
            return
        }
        try {
            val manager = context.getSystemService(NotificationManager::class.java)
            ensureChannels(context, manager)
            if (manager.getNotificationChannel(channel)?.importance == NotificationManager.IMPORTANCE_NONE) {
                invoke.reject("notification channel disabled", "notifications.channel_disabled")
                return
            }
            val builder = NotificationCompat.Builder(context, channel)
                .setSmallIcon(R.drawable.ic_stat_norishell)
                .setContentTitle(title)
                .setContentText(body)
                .setStyle(NotificationCompat.BigTextStyle().bigText(body))
                .setAutoCancel(true)
                .setCategory(NotificationCompat.CATEGORY_STATUS)
                .setPriority(NotificationCompat.PRIORITY_DEFAULT)
            contentIntent(context, id)?.let { builder.setContentIntent(it) }
            NotificationManagerCompat.from(context).notify(id, RESOURCE_NOTIFICATION_ID, builder.build())
            invoke.resolve()
        } catch (error: SecurityException) {
            invoke.reject("notifications not permitted", "notifications.permission_denied")
        } catch (error: Exception) {
            Log.w(TAG, "notification post failed")
            invoke.reject("notification post failed", "notifications.dispatch_failed")
        }
    }

    // ---- Helpers ----

    /** "granted" | "denied" | "notDetermined" */
    private fun permissionState(activity: Activity): String {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && !hasPostPermission(activity)) {
            // Android does not expose "never asked". After our own request,
            // no rationale means the system will not show the dialog again.
            val requested = prefs(activity).getBoolean(PREF_REQUESTED, false)
            val askable = ActivityCompat.shouldShowRequestPermissionRationale(
                activity, Manifest.permission.POST_NOTIFICATIONS,
            )
            return if (requested && !askable) "denied" else "notDetermined"
        }
        return if (NotificationManagerCompat.from(activity).areNotificationsEnabled()) "granted" else "denied"
    }

    private fun hasPostPermission(context: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED

    private fun prefs(context: Context) =
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun ensureChannels(context: Context, manager: NotificationManager) {
        if (manager.getNotificationChannel(ALERTS_CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    ALERTS_CHANNEL,
                    context.getString(R.string.norishell_alerts_channel_name),
                    NotificationManager.IMPORTANCE_DEFAULT,
                ).apply { description = context.getString(R.string.norishell_alerts_channel_description) },
            )
        }
        if (manager.getNotificationChannel(TRANSFERS_CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(
                    TRANSFERS_CHANNEL,
                    context.getString(R.string.norishell_transfers_channel_name),
                    NotificationManager.IMPORTANCE_DEFAULT,
                ).apply { description = context.getString(R.string.norishell_transfers_channel_description) },
            )
        }
    }

    private fun contentIntent(context: Context, id: String): PendingIntent? {
        val open = context.packageManager.getLaunchIntentForPackage(context.packageName) ?: return null
        open.flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
        open.putExtra(EXTRA_EVENT_ID, id)
        // A distinct identifier keeps each notification's PendingIntent (and its
        // extra) separate; extras alone do not distinguish PendingIntents.
        open.identifier = ID_PREFIX + id
        return PendingIntent.getActivity(
            context, 0, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    }
}
