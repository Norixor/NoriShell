import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// Release signing material lives outside the repository. The env var points at a
// private properties file (storeFile, storePassword, keyAlias, keyPassword); release
// tasks fail when it is missing so a debug-signed APK can never pass as a release.
val releaseSigningFile = System.getenv("NORISHELL_ANDROID_SIGNING_PROPERTIES")
    ?.takeIf { it.isNotBlank() }
    ?.let { file(it) }
val releaseSigning = releaseSigningFile?.takeIf { it.isFile }?.let { source ->
    Properties().apply { source.inputStream().use { load(it) } }
}

android {
    compileSdk = 36
    namespace = "app.norishell.android"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        manifestPlaceholders["networkSecurityConfig"] = "@xml/network_security_config"
        applicationId = "app.norishell.android"
        minSdk = 29
        targetSdk = 36
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        if (releaseSigning != null) {
            create("release") {
                storeFile = file(releaseSigning.getProperty("storeFile"))
                storePassword = releaseSigning.getProperty("storePassword")
                keyAlias = releaseSigning.getProperty("keyAlias")
                keyPassword = releaseSigning.getProperty("keyPassword")
                enableV1Signing = false
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            // Debug builds load the Vite dev server over cleartext HTTP.
            manifestPlaceholders["networkSecurityConfig"] = "@xml/network_security_config_debug"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            if (releaseSigning != null) {
                signingConfig = signingConfigs.getByName("release")
            }
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

gradle.taskGraph.whenReady {
    val buildsRelease = allTasks.any { task ->
        task.project == project && task.name.matches(Regex("(assemble|bundle|package)\\w*Release"))
    }
    if (buildsRelease && releaseSigning == null) {
        throw GradleException(
            "Release signing is not configured: set NORISHELL_ANDROID_SIGNING_PROPERTIES " +
                "to the private signing.properties file (see .build)."
        )
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")