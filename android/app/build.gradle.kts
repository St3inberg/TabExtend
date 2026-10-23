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

    buildTypes {
        release {
            isMinifyEnabled = false
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
