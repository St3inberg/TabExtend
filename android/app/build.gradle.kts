// Kotlin support is built into the Android Gradle Plugin from 9.0.
plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "io.github.st3inberg.tabextend"
    compileSdk = 37

    defaultConfig {
        applicationId = "io.github.st3inberg.tabextend"
        // Android 11: low-latency decoding, TLS 1.3, WindowInsetsController.
        minSdk = 30
        targetSdk = 36
        versionCode = 2
        versionName = "0.2.0"
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

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // For scanning the pairing QR code (all Apache-2.0, no Google Play services).
    implementation("androidx.activity:activity:1.13.0")

    // The app's screens: Jetpack Compose with Material 3 (Apache-2.0).
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.core:core-splashscreen:1.2.0")
    implementation("androidx.camera:camera-camera2:1.6.2")
    implementation("androidx.camera:camera-lifecycle:1.6.2")
    implementation("androidx.camera:camera-view:1.6.2")
    implementation("com.google.zxing:core:3.5.4")

    testImplementation("junit:junit:4.13.2")
}
