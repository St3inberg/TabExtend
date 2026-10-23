// Kotlin support is built into the Android Gradle Plugin from 9.0.
plugins {
    id("com.android.application")
}

android {
    namespace = "io.github.st3inberg.tabextend"
    compileSdk = 36

    defaultConfig {
        applicationId = "io.github.st3inberg.tabextend"
        // Android 11: low-latency decoding, TLS 1.3, WindowInsetsController.
        minSdk = 30
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    // Release signing key from the environment, never from the repo:
    //   TABEXTEND_KEYSTORE=/path/release.p12
    //   TABEXTEND_KEYSTORE_PASSWORD_FILE=/path/password
    // Without them, release builds are unsigned.
    val keystore = providers.environmentVariable("TABEXTEND_KEYSTORE").orNull
    val passwordFile = providers.environmentVariable("TABEXTEND_KEYSTORE_PASSWORD_FILE").orNull
    if (keystore != null && passwordFile != null) {
        signingConfigs {
            create("release") {
                storeFile = file(keystore)
                storeType = "pkcs12"
                storePassword = file(passwordFile).readText().trim()
                keyAlias = "tabextend"
                keyPassword = storePassword
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    testImplementation("junit:junit:4.13.2")
}
