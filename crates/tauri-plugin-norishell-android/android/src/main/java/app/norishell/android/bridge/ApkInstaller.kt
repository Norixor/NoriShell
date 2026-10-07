package app.norishell.android.bridge

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.io.File

/**
 * Installs a verified APK through a PackageInstaller session with the system
 * confirmation dialog. One Rust-facing command, `apkInstall`, multiplexed by
 * `action` (the bridge keeps a single delegate per package):
 *
 * - `status`       -> `{canInstall}` ("install unknown apps" granted for this app)
 * - `openSettings` -> opens the per-app "install unknown apps" screen
 * - `install`      -> `{path}` streams the APK into a session and commits it;
 *                     resolves once committed, the outcome is polled via `result`
 * - `result`       -> `{state, code?}`; state is one of `none`, `committing`,
 *                     `awaiting_user`, `installed`, `cancelled`, `blocked`, `failed`
 *
 * Reject codes: `update.permission_required`, `update.invalid_payload`,
 * `update.invalid_path`, `update.session_failed`. Failed results carry
 * `update.install_*` codes.
 */
object ApkInstaller {
    private const val TAG = "NoriShellBridge"
    private const val ACTION_RESULT = "app.norishell.android.action.APK_INSTALL_RESULT"
    private const val UPDATE_DIRECTORY = "update"

    private val lock = Any()
    private var state = "none"
    private var code: String? = null

    fun install(activity: Activity, invoke: Invoke) {
        when (invoke.getArgs().optString("action", "")) {
            "status" -> invoke.resolve(
                JSObject().put("canInstall", activity.packageManager.canRequestPackageInstalls()),
            )
            "openSettings" -> openSettings(activity, invoke)
            "install" -> startInstall(activity, invoke)
            "result" -> synchronized(lock) {
                val result = JSObject().put("state", state)
                code?.let { result.put("code", it) }
                invoke.resolve(result)
            }
            else -> invoke.reject("unknown action", "update.invalid_payload")
        }
    }

    private fun openSettings(activity: Activity, invoke: Invoke) {
        val intent = Intent(
            Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
            Uri.parse("package:${activity.packageName}"),
        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            activity.startActivity(intent)
            invoke.resolve()
        } catch (error: Exception) {
            invoke.reject("settings unavailable", "update.settings_unavailable")
        }
    }

    private fun startInstall(activity: Activity, invoke: Invoke) {
        val path = invoke.getArgs().optString("path", "")
        if (path.isEmpty()) {
            invoke.reject("missing path", "update.invalid_payload")
            return
        }
        // Only a file Rust placed in the app's update cache may be installed.
        val allowedRoot = File(activity.cacheDir, UPDATE_DIRECTORY).canonicalFile
        val file = File(path).canonicalFile
        if (file.parentFile != allowedRoot || !file.isFile) {
            invoke.reject("path outside update cache", "update.invalid_path")
            return
        }
        if (!activity.packageManager.canRequestPackageInstalls()) {
            invoke.reject("install permission required", "update.permission_required")
            return
        }
        synchronized(lock) {
            state = "committing"
            code = null
        }
        // Streaming a large APK must not block the UI thread this command runs on.
        Thread {
            try {
                commit(activity.applicationContext, file)
                invoke.resolve(JSObject().put("state", "committing"))
            } catch (error: Exception) {
                Log.w(TAG, "APK session failed: ${error.javaClass.simpleName}")
                record("failed", "update.session_failed")
                invoke.reject("install session failed", "update.session_failed")
            }
        }.start()
    }

    private fun commit(context: Context, file: File) {
        val installer = context.packageManager.packageInstaller
        // A stale session from an earlier attempt would hold the same storage.
        installer.mySessions.forEach { runCatching { installer.abandonSession(it.sessionId) } }
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        params.setSize(file.length())
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_REQUIRED)
        }
        val sessionId = installer.createSession(params)
        installer.openSession(sessionId).use { session ->
            try {
                file.inputStream().use { input ->
                    session.openWrite("norishell.apk", 0, file.length()).use { output ->
                        input.copyTo(output)
                        session.fsync(output)
                    }
                }
                val intent = Intent(context, ApkInstallResultReceiver::class.java)
                    .setPackage(context.packageName)
                    .setAction(ACTION_RESULT)
                // The installer adds status extras, so the PendingIntent must be mutable.
                val pending = PendingIntent.getBroadcast(
                    context, sessionId, intent,
                    PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
                )
                session.commit(pending.intentSender)
            } catch (error: Exception) {
                session.abandon()
                throw error
            }
        }
    }

    internal fun record(newState: String, newCode: String? = null) {
        synchronized(lock) {
            state = newState
            code = newCode
        }
    }

    internal fun handleResult(context: Context, intent: Intent) {
        when (val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                @Suppress("DEPRECATION")
                val confirm = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                }
                if (confirm == null) {
                    record("failed", "update.install_failed")
                    return
                }
                record("awaiting_user")
                try {
                    context.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                } catch (error: Exception) {
                    Log.w(TAG, "install confirmation could not be shown: ${error.javaClass.simpleName}")
                    record("failed", "update.confirmation_unavailable")
                }
            }
            PackageInstaller.STATUS_SUCCESS -> record("installed")
            PackageInstaller.STATUS_FAILURE_ABORTED -> record("cancelled")
            PackageInstaller.STATUS_FAILURE_BLOCKED -> record("blocked", "update.install_blocked")
            PackageInstaller.STATUS_FAILURE_CONFLICT -> record("failed", "update.install_conflict")
            PackageInstaller.STATUS_FAILURE_INCOMPATIBLE -> record("failed", "update.install_incompatible")
            PackageInstaller.STATUS_FAILURE_INVALID -> record("failed", "update.install_invalid")
            PackageInstaller.STATUS_FAILURE_STORAGE -> record("failed", "update.install_storage")
            else -> record("failed", "update.install_failed_$status")
        }
    }
}

class ApkInstallResultReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) = ApkInstaller.handleResult(context, intent)
}
