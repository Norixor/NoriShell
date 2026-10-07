package app.norishell.android.bridge

import android.app.Service
import android.content.Intent
import android.os.Binder
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.Parcel
import android.os.Process
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Plugin Host in an `android:isolatedProcess` (`:plugin_host`). One process per
 * plugin instance: [PluginHostConnection] binds it with a unique instance name.
 *
 * The only Binder transaction is [TX_RUN]: it carries one end of a socketpair,
 * which is handed to the Rust frame loop ([NoriShellNative.nativeRunPluginHost])
 * on a dedicated thread. The process kills itself as soon as that loop ends
 * (Shutdown, EOF or protocol error), when it is unbound, or when no channel
 * arrives in time, so it never outlives its owner.
 *
 * Only `norishell_desktop` is loaded here; the library has no JNI_OnLoad and
 * Tauri is initialised only through `Rust.kt`/`WryActivity`, which this
 * process never touches.
 */
class PluginHostService : Service() {
    private val started = AtomicBoolean(false)

    private val binder = object : Binder() {
        override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
            if (code != TX_RUN) return super.onTransact(code, data, reply, flags)
            data.enforceInterface(DESCRIPTOR)
            // Not exported, but still accept the channel only from the owning app.
            if (Binder.getCallingUid() != applicationInfo.uid) {
                throw SecurityException("plugin host caller rejected")
            }
            val channel = data.readFileDescriptor()
            if (channel == null || !libraryLoaded || !started.compareAndSet(false, true)) {
                channel?.close()
                reply?.writeNoException()
                reply?.writeInt(0)
                return true
            }
            val fd = channel.detachFd()
            Thread({
                val exitCode = try {
                    NoriShellNative.nativeRunPluginHost(fd)
                } catch (error: Throwable) {
                    EXIT_JNI_FAILURE
                }
                Log.i(TAG, "plugin host loop ended: $exitCode")
                Process.killProcess(Process.myPid())
            }, "norishell-plugin-host").start()
            reply?.writeNoException()
            reply?.writeInt(Process.myPid())
            return true
        }
    }

    override fun onCreate() {
        super.onCreate()
        Handler(Looper.getMainLooper()).postDelayed({
            if (!started.get()) {
                Log.w(TAG, "plugin host received no channel; exiting")
                Process.killProcess(Process.myPid())
            }
        }, CHANNEL_DEADLINE_MS)
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onUnbind(intent: Intent?): Boolean {
        Process.killProcess(Process.myPid())
        return false
    }

    override fun onDestroy() {
        super.onDestroy()
        Process.killProcess(Process.myPid())
    }

    companion object {
        private const val TAG = "NoriShellPluginHost"
        const val DESCRIPTOR = "app.norishell.android.bridge.PluginHostService"
        const val TX_RUN = IBinder.FIRST_CALL_TRANSACTION
        private const val CHANNEL_DEADLINE_MS = 10_000L
        private const val EXIT_JNI_FAILURE = 78

        private val libraryLoaded: Boolean by lazy {
            try {
                System.loadLibrary("norishell_desktop")
                true
            } catch (error: Throwable) {
                Log.e(TAG, "plugin host library could not be loaded")
                false
            }
        }
    }
}
