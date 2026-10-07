import groovy.json.JsonSlurper

buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:8.11.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:1.9.25")
    }
}

// rustls-platform-verifier needs its Kotlin component in the APK. It ships as a
// local Maven repository inside the Cargo package `rustls-platform-verifier-android`,
// so resolve that package's directory and version from `cargo metadata` once.
val rustlsPlatformVerifierPackage: Map<*, *> = run {
    val metadata = providers.exec {
        workingDir = file("../..")
        commandLine(
            "cargo", "metadata", "--format-version", "1", "--locked",
            "--filter-platform", "aarch64-linux-android",
            "--manifest-path", file("../../Cargo.toml").absolutePath,
        )
    }.standardOutput.asText.get()
    val packages = (JsonSlurper().parseText(metadata) as Map<*, *>)["packages"] as List<*>
    packages.map { it as Map<*, *> }
        .first { it["name"] == "rustls-platform-verifier-android" }
}
val rustlsPlatformVerifierMaven: File =
    File(File(rustlsPlatformVerifierPackage["manifest_path"] as String).parentFile, "maven")
val rustlsPlatformVerifierVersion by extra(rustlsPlatformVerifierPackage["version"] as String)

allprojects {
    repositories {
        google()
        mavenCentral()
        maven {
            url = uri(rustlsPlatformVerifierMaven)
            metadataSources.artifact()
            content { includeGroup("rustls") }
        }
    }
}

tasks.register("clean").configure {
    delete("build")
}
