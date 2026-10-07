# JNI and reflection entry points: R8 cannot see native or Tauri annotation usage.
-keep class app.norishell.android.bridge.** { *; }
-keep, includedescriptorclasses class org.rustls.platformverifier.** { *; }
