package app.norishell.android.bridge

import android.app.Activity
import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Build
import android.os.IBinder
import android.os.Parcel
import android.os.ParcelFileDescriptor
import android.util.Log
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * App-process side of the isolated Plugin Host ([PluginHostService]).
 *
 * `pluginHostStart {instanceId, fd}` binds a new isolated process with
 * `bindIsolatedService` (unique instance name per plugin instance), passes a dup
 * of [fd] (one end of a Rust socketpair) over Binder and resolves `{pid}` only
 * after the service took the channel, so Rust may close its copy afterwards.
 *
 * `pluginHostStop {instanceId}` unbinds (the system kills an unbound isolated
 * process), waits up to [DEATH_WAIT_MS] for Binder death and resolves
 * `{alive, reason}`. It is idempotent: a confirmed exit is remembered, so it also
 * serves as the status query for a host whose channel already closed. `reason`
 * is a stable code; `low_memory` means the system reclaimed the process.
 *
 * Both commands arrive on the UI thread; blocking work runs on [worker].
 */
object PluginHostConnection {
    private const val TAG = "NoriShellPluginHost"
    private const val START_TIMEOUT_MS = 10_000L
    private const val DEATH_WAIT_MS = 2_000L
    private const val EXIT_INFO_WAIT_MS = 600L
    private const val EXIT_INFO_POLL_MS = 100L
    private const val MAX_REMEMBERED_EXITS = 64
    private val INSTANCE_ID = Regex("^[A-Za-z0-9_-]{1,64}$")

    private val worker = Executors.newCachedThreadPool { runnable ->
        Thread(runnable, "norishell-plugin-host-binder").apply { isDaemon = true }
    }
    private val timer = Executors.newSingleThreadScheduledExecutor { runnable ->
        Thread(runnable, "norishell-plugin-host-timer").apply { isDaemon = true }
    }
    private val instances = ConcurrentHashMap<String, Instance>()
    private val exits = object : LinkedHashMap<String, String>() {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, String>?) =
            size > MAX_REMEMBERED_EXITS
    }

    private class Instance(val id: String, val context: Context) : IBinder.DeathRecipient {
        val settled = AtomicBoolean(false)
        val unbound = AtomicBoolean(false)
        val death = CountDownLatch(1)
        @Volatile var pid = 0
        @Volatile var connected = false
        lateinit var connection: ServiceConnection

        override fun binderDied() = death.countDown()

        fun unbind() {
            if (unbound.compareAndSet(false, true)) {
                try {
                    context.unbindService(connection)
                } catch (error: IllegalArgumentException) {
                    // Already unbound by the system.
                }
            }
        }
    }

    fun start(activity: Activity, invoke: Invoke) {
        val args = invoke.getArgs()
        val id = args.optString("instanceId", "")
        val fd = args.optInt("fd", -1)
        if (!INSTANCE_ID.matches(id) || fd < 0) {
            invoke.reject("invalid plugin host start payload", "plugin_host.invalid_payload")
            return
        }
        // Duplicates the caller's descriptor now; Rust closes its copy once
        // this command settles.
        val channel = try {
            ParcelFileDescriptor.fromFd(fd)
        } catch (error: Exception) {
            invoke.reject("plugin host channel unavailable", "plugin_host.invalid_channel")
            return
        }
        val context = activity.applicationContext
        val instance = Instance(id, context)
        if (instances.putIfAbsent(id, instance) != null) {
            channel.close()
            invoke.reject("plugin host instance already exists", "plugin_host.duplicate_instance")
            return
        }

        fun fail(code: String) {
            if (!instance.settled.compareAndSet(false, true)) return
            Log.w(TAG, "plugin host start failed: $code")
            instance.unbind()
            instances.remove(id, instance)
            closeQuietly(channel)
            invoke.reject("plugin host start failed", code)
        }

        instance.connection = object : ServiceConnection {
            override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
                if (service == null || instance.settled.get()) {
                    fail("plugin_host.start_failed")
                    return
                }
                val pid = transferChannel(service, channel)
                closeQuietly(channel)
                if (pid <= 0) {
                    fail("plugin_host.start_failed")
                    return
                }
                instance.pid = pid
                instance.connected = true
                try {
                    service.linkToDeath(instance, 0)
                } catch (error: Exception) {
                    instance.death.countDown()
                }
                if (instance.settled.compareAndSet(false, true)) {
                    invoke.resolve(JSObject().put("pid", pid))
                }
            }

            override fun onServiceDisconnected(name: ComponentName?) = instance.death.countDown()

            override fun onBindingDied(name: ComponentName?) {
                instance.death.countDown()
                fail("plugin_host.start_failed")
            }

            override fun onNullBinding(name: ComponentName?) = fail("plugin_host.start_failed")
        }

        val bound = try {
            context.bindIsolatedService(
                Intent(context, PluginHostService::class.java),
                Context.BIND_AUTO_CREATE,
                "plugin_host_$id",
                worker,
                instance.connection,
            )
        } catch (error: Exception) {
            false
        }
        if (!bound) {
            // A failed bind still has to be released.
            fail("plugin_host.bind_failed")
            return
        }
        timer.schedule({ fail("plugin_host.start_timeout") }, START_TIMEOUT_MS, TimeUnit.MILLISECONDS)
    }

    fun stop(activity: Activity, invoke: Invoke) {
        val id = invoke.getArgs().optString("instanceId", "")
        if (!INSTANCE_ID.matches(id)) {
            invoke.reject("invalid plugin host stop payload", "plugin_host.invalid_payload")
            return
        }
        val context = activity.applicationContext
        worker.execute {
            val instance = instances[id]
            if (instance == null) {
                val reason = synchronized(exits) { exits[id] } ?: "not_found"
                invoke.resolve(JSObject().put("alive", false).put("reason", reason))
                return@execute
            }
            instance.unbind()
            // A process that never connected has no Binder to watch; unbinding
            // ends its bind and its own channel deadline ends the process.
            val dead = !instance.connected ||
                instance.death.await(DEATH_WAIT_MS, TimeUnit.MILLISECONDS)
            if (!dead) {
                Log.w(TAG, "plugin host death not confirmed")
                invoke.resolve(JSObject().put("alive", true).put("reason", "unknown"))
                return@execute
            }
            val reason = if (instance.connected) exitReason(context, instance.pid) else "never_started"
            instances.remove(id, instance)
            synchronized(exits) { exits[id] = reason }
            invoke.resolve(JSObject().put("alive", false).put("reason", reason))
        }
    }

    private fun transferChannel(service: IBinder, channel: ParcelFileDescriptor): Int {
        val data = Parcel.obtain()
        val reply = Parcel.obtain()
        return try {
            data.writeInterfaceToken(PluginHostService.DESCRIPTOR)
            data.writeFileDescriptor(channel.fileDescriptor)
            service.transact(PluginHostService.TX_RUN, data, reply, 0)
            reply.readException()
            reply.readInt()
        } catch (error: Exception) {
            0
        } finally {
            data.recycle()
            reply.recycle()
        }
    }

    /** Stable exit classification; ApplicationExitInfo is API 30+. */
    private fun exitReason(context: Context, pid: Int): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return "unknown"
        val manager = context.getSystemService(ActivityManager::class.java) ?: return "unknown"
        val deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(EXIT_INFO_WAIT_MS)
        while (true) {
            val info = try {
                manager.getHistoricalProcessExitReasons(context.packageName, pid, 1).firstOrNull()
            } catch (error: Exception) {
                null
            }
            if (info != null) {
                return when (info.reason) {
                    ApplicationExitInfo.REASON_LOW_MEMORY -> "low_memory"
                    ApplicationExitInfo.REASON_EXIT_SELF -> "exited"
                    ApplicationExitInfo.REASON_SIGNALED -> "signaled"
                    ApplicationExitInfo.REASON_CRASH,
                    ApplicationExitInfo.REASON_CRASH_NATIVE -> "crashed"
                    ApplicationExitInfo.REASON_ANR -> "anr"
                    ApplicationExitInfo.REASON_EXCESSIVE_RESOURCE_USAGE -> "resource_usage"
                    else -> "other"
                }
            }
            if (System.nanoTime() >= deadline) return "unknown"
            Thread.sleep(EXIT_INFO_POLL_MS)
        }
    }

    private fun closeQuietly(channel: ParcelFileDescriptor) {
        try {
            channel.close()
        } catch (error: Exception) {
            // Already closed.
        }
    }
}
