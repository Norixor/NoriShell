// Copyright 2019-2024 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

package app.tauri.plugin

import android.content.res.Configuration
import android.content.Context
import android.content.Intent
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.activity.result.ActivityResultLauncher
import androidx.activity.result.IntentSenderRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.InvokeArg
import app.tauri.FsUtils
import app.tauri.JniMethod
import app.tauri.Logger
import com.fasterxml.jackson.annotation.JsonAutoDetect
import com.fasterxml.jackson.annotation.PropertyAccessor
import com.fasterxml.jackson.databind.DeserializationFeature
import com.fasterxml.jackson.databind.JsonNode
import com.fasterxml.jackson.databind.ObjectMapper
import com.fasterxml.jackson.databind.module.SimpleModule
import java.lang.reflect.InvocationTargetException

object PluginManager {
  fun interface RequestPermissionsCallback {
    fun onResult(permissions: Map<String, Boolean>)
  }

  fun interface ActivityResultCallback {
    fun onResult(result: ActivityResult)
  }

  lateinit var activity: AppCompatActivity
  private val plugins: HashMap<String, PluginHandle> = HashMap()
  private lateinit var startActivityForResultLauncher: ActivityResultLauncher<Intent>
  private lateinit var startIntentSenderForResultLauncher: ActivityResultLauncher<IntentSenderRequest>
  private lateinit var requestPermissionsLauncher: ActivityResultLauncher<Array<String>>
  private var requestPermissionsCallback: RequestPermissionsCallback? = null
  private var startActivityForResultCallback: ActivityResultCallback? = null
  private var startIntentSenderForResultCallback: ActivityResultCallback? = null
  private var jsonMapper: ObjectMapper = ObjectMapper()
    .disable(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES)
    .enable(DeserializationFeature.FAIL_ON_NULL_FOR_PRIMITIVES)
    .setVisibility(PropertyAccessor.FIELD, JsonAutoDetect.Visibility.ANY)

  init {
    val channelDeserializer = ChannelDeserializer({ channelId, payload ->
      sendChannelData(channelId, payload)
    }, jsonMapper)
    jsonMapper
      .registerModule(SimpleModule().addDeserializer(Channel::class.java, channelDeserializer))
  }

  fun onActivityCreate(activity: AppCompatActivity) {
    // NoriShell: a relaunched host Activity (configuration change outside
    // `configChanges`, e.g. a navigation-mode overlay) takes over the plugin
    // state. Upstream returned here and kept the launchers registered on the
    // destroyed Activity ("unregistered ActivityResultLauncher": pickers fail
    // silently) and every plugin pointed at the dead Activity.
    if (::activity.isInitialized) {
      if (this.activity === activity) return
      this.activity = activity
      registerLaunchers(activity)
      for (plugin in plugins.values) {
        plugin.instance.triggerActivityRecreated(activity)
      }
      return
    }
    this.activity = activity
    registerLaunchers(activity)
  }

  /**
   * NoriShell: called from onCreate (before STARTED) in the same order on every
   * Activity instance, so the registry's restored keys match and a result that
   * arrives across a relaunch (picker open while the Activity is recreated) is
   * delivered to the new launcher; callbacks are read when the result arrives.
   */
  private fun registerLaunchers(activity: AppCompatActivity) {
    startActivityForResultLauncher =
      activity.registerForActivityResult(ActivityResultContracts.StartActivityForResult()
      ) { result ->
        if (startActivityForResultCallback != null) {
          startActivityForResultCallback!!.onResult(result)
        }
      }

    startIntentSenderForResultLauncher =
      activity.registerForActivityResult(ActivityResultContracts.StartIntentSenderForResult()
      ) { result ->
        if (startIntentSenderForResultCallback != null) {
          startIntentSenderForResultCallback!!.onResult(result)
        }
      }

    requestPermissionsLauncher =
      activity.registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()
      ) { result ->
        if (requestPermissionsCallback != null) {
          requestPermissionsCallback!!.onResult(result)
        }
      }
  }

  fun onNewIntent(intent: Intent) {
    for (plugin in plugins.values) {
      plugin.instance.onNewIntent(intent)
    }
  }

  fun onPause() {
    for (plugin in plugins.values) {
      plugin.instance.onPause()
    }
  }

  fun onResume() {
    for (plugin in plugins.values) {
      plugin.instance.onResume()
    }
  }

  fun onRestart(activity: AppCompatActivity) {
    for (plugin in plugins.values) {
      plugin.instance.triggerOnRestart(activity)
    }
  }

  fun onStop() {
    for (plugin in plugins.values) {
      plugin.instance.onStop()
    }
  }

  fun onDestroy(activity: AppCompatActivity) {
    for (plugin in plugins.values) {
      plugin.instance.triggerOnDestroy(activity)
    }
  }

  fun onConfigurationChanged(newConfig: Configuration) {
    for (plugin in plugins.values) {
      plugin.instance.onConfigurationChanged(newConfig)
    }
  }

  fun startActivityForResult(intent: Intent, callback: ActivityResultCallback) {
    startActivityForResultCallback = callback
    startActivityForResultLauncher.launch(intent)
  }

  fun startIntentSenderForResult(intent: IntentSenderRequest, callback: ActivityResultCallback) {
    startIntentSenderForResultCallback = callback
    startIntentSenderForResultLauncher.launch(intent)
  }

  fun requestPermissions(
    permissionStrings: Array<String>,
    callback: RequestPermissionsCallback
  ) {
    requestPermissionsCallback = callback
    requestPermissionsLauncher.launch(permissionStrings)
  }

  @JniMethod
  fun onWebViewCreated(webView: WebView) {
    for ((_, plugin) in plugins) {
      if (!plugin.loaded) {
        plugin.load(webView)
      }
    }
  }

  @JniMethod
  fun load(webView: WebView?, name: String, plugin: Plugin, config: String) {
    val handle = PluginHandle(this, name, plugin, config, jsonMapper)
    plugins[name] = handle
    if (webView != null) {
      plugin.load(webView)
    }
  }

  @JniMethod
  fun runCommand(id: Int, pluginId: String, command: String, data: String) {
    val successId = 0L
    val errorId = 1L
    val invoke = Invoke(id.toLong(), command, successId, errorId, { fn, result ->
      var success: String? = null
      var error: String? = null
      if (fn == successId) {
        success = result
      } else {
        error = result
      }
      handlePluginResponse(id, success, error)
    }, data, jsonMapper)

    dispatchPluginMessage(invoke, pluginId)
  }

  private fun dispatchPluginMessage(invoke: Invoke, pluginId: String) {
    Logger.verbose(
      Logger.tags("Plugin"),
      "Tauri plugin: pluginId: $pluginId, command: ${invoke.command}"
    )

    try {
      val plugin = plugins[pluginId]
      if (plugin == null) {
        invoke.reject("Plugin $pluginId not initialized")
      } else {
        plugins[pluginId]?.invoke(invoke)
      }
    } catch (e: Exception) {
      var exception: Throwable = e
      if (exception.message?.isEmpty() != false) {
        if (e is InvocationTargetException) {
          exception = e.targetException
        }
      }
      invoke.reject(if (exception.message?.isEmpty() != false) { exception.toString() } else { exception.message })
    }
  }

  fun<T> loadConfig(context: Context, plugin: String, cls: Class<T>): T {
    val tauriConfigJson = FsUtils.readAsset(context.assets, "tauri.conf.json")
    val mapper = ObjectMapper()
      .configure(DeserializationFeature.FAIL_ON_UNKNOWN_PROPERTIES, false)
    val config = mapper.readValue(tauriConfigJson, Config::class.java)
    return mapper.readValue(config.plugins[plugin].toString(), cls)
  }

  private external fun handlePluginResponse(id: Int, success: String?, error: String?)
  private external fun sendChannelData(id: Long, data: String)
}

@InvokeArg
internal class Config {
  lateinit var plugins: Map<String, JsonNode>
}
