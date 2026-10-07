package app.norishell.android

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import app.norishell.android.bridge.NotificationsBridge
import app.norishell.android.bridge.SessionForegroundService

/**
 * Invisible entry point for the launcher icon, every notification and the
 * foreground-service actions.
 *
 * MainActivity is `singleTask`: starting it directly clears every Activity
 * above it, which would destroy an open Secure Surface (a Vault prompt half
 * typed, a Host editor draft). This Activity shares the app task's affinity,
 * so the system starts it on top of the existing task (or just brings the task
 * forward when the intent equals the task's launch intent). When the app is
 * already running it hands the intent's bridge actions to their existing
 * handlers and finishes, leaving the task exactly as it was. Only a cold start
 * launches MainActivity, which then handles the same extras itself.
 */
class LaunchTrampolineActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val launch = intent
        if (MainActivity.running) {
            // Same handlers MainActivity and the bridge plugin use on onNewIntent.
            SessionForegroundService.handleLaunchIntent(launch)
            NotificationsBridge.handleIntent(launch)
        } else {
            startActivity(
                Intent(launch).setClass(this, MainActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
        finish()
    }
}
