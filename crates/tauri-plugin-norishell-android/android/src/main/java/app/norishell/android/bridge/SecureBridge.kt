package app.norishell.android.bridge

import android.app.Activity
import android.app.Application
import android.content.Intent
import android.os.Bundle
import android.util.Log
import app.tauri.plugin.Invoke
import java.lang.ref.WeakReference
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * Rust-facing half of the Secure Surfaces (`secureSurfaceOpen/Close/Post`).
 *
 * Core decides every surface (label, kind, entry path, protection); this object
 * only hosts them in [SecureSurfaceActivity]. Specs are non-secret and never
 * travel through an Intent: a pending queue hands them to the Activity, which
 * also lets a launch that Android blocked in the background resume when the
 * app returns to the foreground.
 *
 * All members are UI-thread only except [executor] and [notifyDestroyed].
 */
object SecureBridge {
    private const val TAG = "NoriShellSecure"

    const val ORIGIN = "https://secure.norishell.localhost"
    const val SECURE_HOST = "secure.norishell.localhost"
    const val PLUGIN_DOCUMENT_HOST = "norishell-plugin.localhost"
    const val PLUGIN_ISOLATED = "pluginIsolated"

    /** Non-secret surface description produced by Core. */
    data class Spec(
        val label: String,
        val kind: String,
        val path: String,
        val title: String,
        val contentProtected: Boolean,
    )

    /**
     * Kotlin half of the double whitelist: commands each surface kind may send.
     * Mirrors `SURFACE_COMMANDS` in `src-tauri/src/secure_surface.rs`; Core
     * re-checks every command against the surface label prefix.
     */
    val COMMANDS: Map<String, Set<String>> = mapOf(
        "vault" to setOf("secure_vault_get", "secure_vault_submit", "secure_vault_reset", "secure_vault_cancel"),
        "credential" to setOf(
            "secure_credential_get", "secure_credential_submit", "secure_credential_cancel", "secure_vault_open",
        ),
        "sshChallenge" to setOf(
            "secure_ssh_challenge_get", "secure_ssh_challenge_submit", "secure_ssh_challenge_cancel",
        ),
        "backup" to setOf("offline_backup_secure_get", "offline_backup_secure_submit", "offline_backup_secure_cancel"),
        "desktopAuth" to setOf("desktop_prompt_get", "desktop_prompt_decide"),
        "pluginHostApproval" to setOf("plugin_host_approval_get", "plugin_host_approval_decide"),
        "pluginPermission" to setOf("plugin_special_permission_get", "plugin_special_permission_decide"),
        "pluginRemoteApproval" to setOf(
            "plugin_remote_approval_get", "plugin_remote_approval_decide", "plugin_credential_input_submit",
        ),
        "pluginTerminalInput" to setOf("plugin_terminal_input_get", "plugin_terminal_input_decide"),
        "sshSync" to setOf("ssh_sync_secure_prompt_get", "ssh_sync_secure_prompt_decide"),
        PLUGIN_ISOLATED to setOf("plugin_isolated_surface_content", "plugin_isolated_bridge"),
        "toolHostEditor" to TOOL_LIFECYCLE + setOf(
            "host_list", "host_catalog_list", "host_group_list", "host_group_create", "host_tag_list",
            "host_tag_create", "identity_list", "credential_ref_list", "algorithm_policy_catalog_get",
            "host_connection_config_get", "host_configured_create", "host_update", "host_organization_replace",
            "route_plan_replace", "algorithm_policy_replace", "heartbeat_policy_replace",
            "monitoring_policy_replace", "login_automation_replace", "login_automation_confirm",
            "login_automation_secret_create", "login_automation_secret_cancel", "host_create_password_stage",
            "host_create_password_cancel", "identity_create", "private_key_file_import",
            "private_key_file_import_release",
            "credential_transient_prepare", "ssh_connection_test", "vault_status", "secure_vault_open",
        ),
        "toolDesktopEditor" to TOOL_LIFECYCLE + setOf(
            "desktop_profile_list", "desktop_profile_save", "host_list", "identity_list", "credential_ref_list",
            "host_create_password_stage", "host_create_password_cancel", "vault_status", "secure_vault_open",
        ),
        "toolSftpFile" to TOOL_LIFECYCLE + setOf("tool_file_preview", "tool_file_tail", "tool_file_save"),
    )

    // `tool_window_exit_reply` answers Core's application-exit draft check
    // (`tool-window-exit-requested` event posted to the editor surface).
    private val TOOL_LIFECYCLE: Set<String>
        get() = setOf("tool_window_get", "tool_window_close", "tool_window_changed", "tool_window_exit_reply")

    /** Dispatcher threads: a command may block while a nested surface is answered. */
    val executor: ExecutorService = Executors.newCachedThreadPool { runnable ->
        Thread(runnable, "norishell-secure").apply { isDaemon = true }
    }

    private val pending = LinkedHashMap<String, Spec>()
    private var host: WeakReference<SecureSurfaceActivity>? = null
    private var resumed: WeakReference<Activity>? = null
    private var lifecycleRegistered = false

    fun open(activity: Activity, invoke: Invoke) {
        val spec = parseSpec(invoke)
        if (spec == null) {
            invoke.reject("invalid secure surface", "secure_surface.invalid_spec")
            return
        }
        registerLifecycle(activity)
        val current = liveHost()
        if (current != null) {
            current.addOrRaise(spec)
        } else {
            pending[spec.label] = spec
            launch(activity)
        }
        invoke.resolve()
    }

    fun close(activity: Activity, invoke: Invoke) {
        val label = invoke.getArgs().optString("label")
        if (pending.remove(label) != null) notifyDestroyed(label)
        liveHost()?.removeSurface(label)
        invoke.resolve()
    }

    fun post(activity: Activity, invoke: Invoke) {
        val args = invoke.getArgs()
        val label = args.optString("label")
        val event = args.optString("event")
        if (event.isEmpty()) {
            invoke.reject("missing event", "secure_surface.invalid_event")
            return
        }
        val payload = args.opt("payload")?.toString() ?: "null"
        liveHost()?.postEvent(label, event, payload)
        invoke.resolve()
    }

    /** Called by the Activity; returns and clears specs waiting for a host. */
    internal fun attach(activity: SecureSurfaceActivity): List<Spec> {
        host = WeakReference(activity)
        val specs = pending.values.toList()
        pending.clear()
        return specs
    }

    internal fun detach(activity: SecureSurfaceActivity) {
        if (host?.get() === activity) host = null
    }

    /**
     * The host Activity was destroyed by the system, not by a close: e.g. the
     * launcher icon re-launches the singleTask MainActivity, which clears every
     * Activity above it. Core still waits for these prompts, so they are shown
     * again (reloaded from Core by their prompt id) instead of being lost or
     * silently cancelled. Core's own expiry still bounds them.
     */
    internal fun restore(specs: List<Spec>) {
        for (spec in specs) pending[spec.label] = spec
        if (pending.isEmpty() || liveHost() != null) return
        resumed?.get()?.takeIf { !it.isFinishing && !it.isDestroyed }?.let { launch(it) }
    }

    /** The host is being relaunched by the system: hand its specs to the replacement without starting another. */
    internal fun keepForRecreate(specs: List<Spec>) {
        for (spec in specs) pending[spec.label] = spec
    }

    /** Tells Core the surface is gone. Never blocks the UI thread. */
    internal fun notifyDestroyed(label: String) {
        executor.execute {
            try {
                NoriShellNative.nativeSurfaceDestroyed(label)
            } catch (error: UnsatisfiedLinkError) {
                Log.w(TAG, "surface destroyed notification unavailable")
            }
        }
    }

    private fun liveHost(): SecureSurfaceActivity? =
        host?.get()?.takeIf { !it.isFinishing && !it.isDestroyed }

    private fun launch(from: Activity) {
        try {
            from.startActivity(Intent(from, SecureSurfaceActivity::class.java))
        } catch (error: RuntimeException) {
            // Kept pending; the next foreground resume retries.
            Log.w(TAG, "secure surface launch deferred")
        }
    }

    /**
     * Android may block an Activity start while the app is in the background.
     * Retry from the next resumed app Activity so a prompt is never lost.
     */
    private fun registerLifecycle(activity: Activity) {
        if (lifecycleRegistered) return
        lifecycleRegistered = true
        activity.application.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityResumed(activity: Activity) {
                if (activity is SecureSurfaceActivity) return
                resumed = WeakReference(activity)
                if (pending.isNotEmpty() && liveHost() == null) launch(activity)
            }

            override fun onActivityPaused(activity: Activity) {
                if (resumed?.get() === activity) resumed = null
            }

            override fun onActivityCreated(created: Activity, state: Bundle?) = Unit
            override fun onActivityStarted(started: Activity) = Unit
            override fun onActivityStopped(stopped: Activity) = Unit
            override fun onActivitySaveInstanceState(saved: Activity, state: Bundle) = Unit
            override fun onActivityDestroyed(destroyed: Activity) = Unit
        })
    }

    private fun parseSpec(invoke: Invoke): Spec? {
        val args = invoke.getArgs()
        val label = args.optString("label")
        val kind = args.optString("kind")
        val path = args.optString("path")
        if (label.isEmpty() || label.length > 256 || kind !in COMMANDS) return null
        // Relative entry document only; the origin is fixed here.
        if (path.isEmpty() || path.startsWith("/") || path.contains("://") || path.contains("..")) return null
        return Spec(
            label = label,
            kind = kind,
            path = path,
            title = args.optString("title", "NoriShell"),
            contentProtected = args.optBoolean("contentProtected", true),
        )
    }
}
