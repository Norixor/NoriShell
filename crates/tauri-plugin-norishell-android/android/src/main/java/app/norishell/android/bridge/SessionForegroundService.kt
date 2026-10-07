package app.norishell.android.bridge

import android.app.ForegroundServiceStartNotAllowedException
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import java.util.Locale
import kotlin.concurrent.thread

/**
 * Foreground service (type `specialUse`) that keeps the process alive while
 * Core reports live resources. Rust's foreground worker drives it through
 * `foregroundUpdate` / `foregroundStop`; the service holds no session state.
 *
 * The "disconnect all and exit" action opens the launcher activity with
 * [EXTRA_ACTION] so the exit confirmation UI is visible; MainActivity forwards
 * it through [handleLaunchIntent].
 */
class SessionForegroundService : Service() {
    data class Counts(val sessions: Int, val tunnels: Int, val sftp: Int, val transfers: Int)

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val counts = Counts(
            sessions = intent?.getIntExtra(KEY_SESSIONS, 0) ?: 0,
            tunnels = intent?.getIntExtra(KEY_TUNNELS, 0) ?: 0,
            sftp = intent?.getIntExtra(KEY_SFTP, 0) ?: 0,
            transfers = intent?.getIntExtra(KEY_TRANSFERS, 0) ?: 0,
        )
        val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE
        } else {
            0
        }
        val text = localized(this, intent?.getStringExtra(KEY_LOCALE))
        ServiceCompat.startForeground(this, NOTIFICATION_ID, buildNotification(this, text, counts), type)
        // Rust re-sends the counts while resources exist; do not resurrect after a kill.
        return START_NOT_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
        dispatch(NoriShellNative.ACTION_TASK_REMOVED)
    }

    companion object {
        private const val TAG = "NoriShellFgs"
        private const val EXIT_IDENTIFIER = "norishell.fgs.exit-all"
        private const val CHANNEL_ID = "norishell.sessions"
        private const val NOTIFICATION_ID = 0x4E5301
        private const val KEY_SESSIONS = "sessions"
        private const val KEY_TUNNELS = "tunnels"
        private const val KEY_SFTP = "sftp"
        private const val KEY_TRANSFERS = "transfers"
        private const val KEY_LOCALE = "locale"

        /** Extra on the launcher intent carrying a [NoriShellNative] action. */
        const val EXTRA_ACTION = "app.norishell.android.extra.ACTION"

        /** Returns null on success or a stable error code. */
        fun update(context: Context, counts: Counts, locale: String): String? {
            val intent = Intent(context, SessionForegroundService::class.java)
                .putExtra(KEY_LOCALE, locale)
                .putExtra(KEY_SESSIONS, counts.sessions)
                .putExtra(KEY_TUNNELS, counts.tunnels)
                .putExtra(KEY_SFTP, counts.sftp)
                .putExtra(KEY_TRANSFERS, counts.transfers)
            return try {
                ensureChannel(localized(context, locale))
                context.startForegroundService(intent)
                null
            } catch (error: Exception) {
                val code = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S &&
                    error is ForegroundServiceStartNotAllowedException
                ) {
                    "foreground.start_not_allowed"
                } else {
                    "foreground.start_failed"
                }
                Log.w(TAG, "foreground service start failed: $code")
                code
            }
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, SessionForegroundService::class.java))
        }

        /**
         * Called for launch intents by MainActivity (cold start) and LaunchTrampolineActivity
         * (app already running). Returns true
         * when the intent carried a bridge action (and consumes the extra).
         */
        @JvmStatic
        fun handleLaunchIntent(intent: Intent?): Boolean {
            val action = intent?.getStringExtra(EXTRA_ACTION) ?: return false
            intent.removeExtra(EXTRA_ACTION)
            if (action != NoriShellNative.ACTION_EXIT_ALL) return false
            dispatch(action)
            return true
        }

        /** Rust callbacks may call back into Kotlin; never run them on the UI thread. */
        private fun dispatch(action: String) {
            thread(name = "norishell-notification-action") {
                try {
                    NoriShellNative.nativeNotificationAction(action)
                } catch (error: Throwable) {
                    Log.w(TAG, "notification action dispatch failed")
                }
            }
        }

        private fun ensureChannel(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java)
            val name = context.getString(R.string.norishell_fgs_channel_name)
            // Re-creating an existing channel only renames it (importance and
            // user changes stay), so a language switch reaches the settings too.
            if (manager.getNotificationChannel(CHANNEL_ID)?.name?.toString() == name) return
            val channel = NotificationChannel(CHANNEL_ID, name, NotificationManager.IMPORTANCE_LOW)
            channel.description = context.getString(R.string.norishell_fgs_channel_description)
            channel.setShowBadge(false)
            manager.createNotificationChannel(channel)
        }

        private fun launcherIntent(context: Context): Intent? =
            context.packageManager.getLaunchIntentForPackage(context.packageName)?.apply {
                flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
            }

        /**
         * Resources in the app language chosen in Settings (`en`, `zh-CN`);
         * `system` or anything else follows the device locale.
         */
        private fun localized(context: Context, locale: String?): Context {
            val chosen = when (locale) {
                "en" -> Locale.ENGLISH
                "zh-CN" -> Locale.SIMPLIFIED_CHINESE
                else -> return context
            }
            val configuration = Configuration(context.resources.configuration)
            configuration.setLocale(chosen)
            return context.createConfigurationContext(configuration)
        }

        /**
         * [text] supplies the localized strings; [context] owns the
         * notification.
         */
        private fun buildNotification(context: Context, text: Context, counts: Counts): Notification {
            ensureChannel(text)
            val immutable = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
            val builder = NotificationCompat.Builder(context, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_stat_norishell)
                .setContentTitle(text.getString(R.string.norishell_fgs_title))
                .setContentText(
                    text.getString(
                        R.string.norishell_fgs_text,
                        counts.sessions, counts.tunnels, counts.sftp, counts.transfers,
                    ),
                )
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setSilent(true)
                .setCategory(NotificationCompat.CATEGORY_SERVICE)
                .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            launcherIntent(context)?.let { open ->
                builder.setContentIntent(PendingIntent.getActivity(context, 0, open, immutable))
                // A distinct identifier makes the exit intent differ from the task's
                // launch intent, so the launcher entry Activity really runs (and
                // routes the action) instead of the task only moving forward.
                val exit = Intent(open).putExtra(EXTRA_ACTION, NoriShellNative.ACTION_EXIT_ALL)
                    .setIdentifier(EXIT_IDENTIFIER)
                builder.addAction(
                    0,
                    text.getString(R.string.norishell_fgs_action_exit),
                    PendingIntent.getActivity(context, 1, exit, immutable),
                )
            }
            return builder.build()
        }
    }
}
