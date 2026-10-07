package app.norishell.android.bridge

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import android.util.Base64
import android.util.Log
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * Rust -> Kotlin command surface. Commands are reached only through
 * `PluginHandle::run_mobile_plugin` (the Rust plugin rejects every WebView
 * invoke). Error codes passed to `reject` are stable machine codes.
 *
 * Commands owned by later packages delegate to their own files
 * (SecureBridge, PluginHostConnection, ApkInstaller) so this file stays stable.
 */
@TauriPlugin
class NoriShellPlugin(initial: Activity) : Plugin(initial) {
    /** The live host Activity; replaced when the system relaunches MainActivity. */
    private var activity: Activity = initial

    init {
        val ready = try {
            NoriShellNative.nativeInit(activity.applicationContext)
        } catch (error: UnsatisfiedLinkError) {
            false
        }
        Log.i(TAG, "bridge plugin created; platform verifier ready=$ready")
        NotificationsBridge.install(activity)
    }

    override fun onActivityRecreated(activity: androidx.appcompat.app.AppCompatActivity) {
        this.activity = activity
    }

    override fun load(webView: android.webkit.WebView) {
        NotificationsBridge.handleIntent(activity.intent)
    }

    override fun onNewIntent(intent: android.content.Intent) {
        NotificationsBridge.handleIntent(intent)
    }

    // ---- Vault auto-unlock device key (AndroidKeyStore) ----

    @Command
    fun deviceKeyWrap(invoke: Invoke) = keystore(invoke) { DeviceKeyStore.wrap(it) }

    @Command
    fun deviceKeyUnwrap(invoke: Invoke) = keystore(invoke) { DeviceKeyStore.unwrap(it) }

    @Command
    fun deviceKeyDelete(invoke: Invoke) {
        try {
            DeviceKeyStore.delete()
            invoke.resolve()
        } catch (error: DeviceKeyStore.KeystoreException) {
            invoke.reject("device key delete failed", error.code)
        }
    }

    private fun keystore(invoke: Invoke, operation: (ByteArray) -> ByteArray) {
        val input = try {
            Base64.decode(invoke.getArgs().getString("data"), Base64.NO_WRAP)
        } catch (error: Exception) {
            invoke.reject("invalid payload", "keystore.invalid_payload")
            return
        }
        try {
            val output = operation(input)
            invoke.resolve(JSObject().put("data", Base64.encodeToString(output, Base64.NO_WRAP)))
        } catch (error: DeviceKeyStore.KeystoreException) {
            invoke.reject("keystore operation failed", error.code)
        } finally {
            input.fill(0)
        }
    }

    // ---- Foreground service and notifications ----

    @Command
    fun foregroundUpdate(invoke: Invoke) {
        val args = invoke.getArgs()
        val counts = SessionForegroundService.Counts(
            sessions = args.optInt("sessions", 0),
            tunnels = args.optInt("tunnels", 0),
            sftp = args.optInt("sftp", 0),
            transfers = args.optInt("transfers", 0),
        )
        val locale = args.optString("locale", "system")
        when (val code = SessionForegroundService.update(activity.applicationContext, counts, locale)) {
            null -> invoke.resolve()
            else -> invoke.reject("foreground service update failed", code)
        }
    }

    @Command
    fun foregroundStop(invoke: Invoke) {
        SessionForegroundService.stop(activity.applicationContext)
        invoke.resolve()
    }

    @Command
    fun notificationPermissionRequest(invoke: Invoke) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            invoke.resolve(JSObject().put("granted", true))
            return
        }
        val granted = ContextCompat.checkSelfPermission(
            activity, Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED
        if (!granted) {
            ActivityCompat.requestPermissions(
                activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), NOTIFICATION_REQUEST_CODE,
            )
        }
        invoke.resolve(JSObject().put("granted", granted))
    }

    // Resource notifications (NotificationsBridge.kt).
    @Command
    fun notificationsPermission(invoke: Invoke) = NotificationsBridge.permission(activity, invoke)

    @Command
    fun notificationsRequestPermission(invoke: Invoke) =
        NotificationsBridge.requestPermission(activity, invoke)

    @Command
    fun notificationsPost(invoke: Invoke) = NotificationsBridge.post(activity, invoke)

    // ---- Later packages (delegating stubs; implementations live in their files) ----

    @Command
    fun secureSurfaceOpen(invoke: Invoke) = SecureBridge.open(activity, invoke)

    @Command
    fun secureSurfaceClose(invoke: Invoke) = SecureBridge.close(activity, invoke)

    @Command
    fun secureSurfacePost(invoke: Invoke) = SecureBridge.post(activity, invoke)

    @Command
    fun pluginHostStart(invoke: Invoke) = PluginHostConnection.start(activity, invoke)

    @Command
    fun pluginHostStop(invoke: Invoke) = PluginHostConnection.stop(activity, invoke)

    @Command
    fun apkInstall(invoke: Invoke) = ApkInstaller.install(activity, invoke)

    // Storage Access Framework documents selected by the user (ContentUriIo.kt).
    @Command
    fun contentUriOpen(invoke: Invoke) = ContentUriIo.open(activity, invoke)

    @Command
    fun contentUriQuery(invoke: Invoke) = ContentUriIo.query(activity, invoke)

    // System document picker for Core-initiated picks; works while a Secure
    // Surface covers MainActivity (ContentPicker.kt).
    @Command
    fun contentPick(invoke: Invoke) = ContentPicker.pick(activity, invoke)

    @Command
    fun contentCreate(invoke: Invoke) = ContentPicker.create(activity, invoke)

    // ---- Window chrome ----

    /** `{dark: Boolean, background: "#rrggbb"}`: system bar icons and the color behind the bars. */
    @Command
    fun systemBarsSet(invoke: Invoke) {
        val args = invoke.getArgs()
        val background = SystemBars.parseColor(args.optString("background", ""))
        if (background == null || !args.has("dark")) {
            invoke.reject("invalid system bars style", "system_bars.invalid_payload")
            return
        }
        SystemBars.update(args.optBoolean("dark", false), background)
        invoke.resolve()
    }

    companion object {
        private const val TAG = "NoriShellBridge"
        private const val NOTIFICATION_REQUEST_CODE = 0x4E53
    }
}
