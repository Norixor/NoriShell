plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "app.norishell.android.bridge"
    compileSdk = 36

    defaultConfig {
        minSdk = 29
        consumerProguardFiles("consumer-rules.pro")
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
}

// The Kotlin half of rustls-platform-verifier ships inside the Cargo package
// `rustls-platform-verifier-android`; the root Gradle project resolves its local
// Maven directory and version through `cargo metadata`.
val rustlsPlatformVerifierVersion: String by rootProject.extra

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("rustls:rustls-platform-verifier:$rustlsPlatformVerifierVersion")
    implementation(project(":tauri-android"))
}
