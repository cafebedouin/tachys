plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

val releaseStore: String? = providers.gradleProperty("tachysStoreFile").orNull
val idSuffix: String? = providers.gradleProperty("tachysIdSuffix").orNull

// A release build with no key named used to be signed with the debug key and say nothing (finding 63). It is now
// refused, unless it is the copy that installs beside the real app (tachysIdSuffix), which no phone takes for it.
gradle.taskGraph.whenReady {
    if (releaseStore == null && idSuffix == null && allTasks.any { it.project == project && it.name in setOf("packageRelease", "packageReleaseBundle", "signReleaseBundle") })
        throw GradleException("A release build needs a signing key: name one (tachysStoreFile, tachysStorePassword, tachysKeyPassword; see app/build.gradle.kts and docs/maintaining.md), or pass -PtachysIdSuffix=.test for a copy signed with the debug key that installs beside the real app, or build assembleDebug.")
}

android {
    namespace = "org.tachys.wallet"
    compileSdk = 37
    defaultConfig {
        applicationId = "org.tachys.wallet"
        minSdk = 24
        targetSdk = 37
        // -PtachysIdSuffix=.test builds a copy that installs beside the real app, with its own wallets: for trying a build on a phone that is in use
        idSuffix?.let { applicationIdSuffix = it }
        versionCode = 2
        versionName = "0.0.2-spike"
    }
    // The release key lives outside the repository. Name it in ~/.gradle/gradle.properties:
    //   tachysStoreFile=/path/to/tachys-release.jks   tachysStorePassword=…   tachysKeyAlias=tachys   tachysKeyPassword=…
    // or for one build, with nothing written down, in the environment: ORG_GRADLE_PROJECT_tachysStoreFile=… and so on.
    signingConfigs {
        if (releaseStore != null) create("release") {
            storeFile = file(releaseStore)
            storePassword = providers.gradleProperty("tachysStorePassword").orNull
            keyAlias = providers.gradleProperty("tachysKeyAlias").orNull ?: "tachys"
            keyPassword = providers.gradleProperty("tachysKeyPassword").orNull
        }
    }
    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"))
            // signed with the key named; with none, only the copy beside the real app is built (see the top), with the debug key
            signingConfig = if (releaseStore != null) signingConfigs.getByName("release") else signingConfigs.getByName("debug")
        }
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    buildFeatures { compose = true }
    // The Rust core is a library the app loads (libtachys_c.so, crates/tachys-c). It is packed compressed and unpacked
    // when the app is installed, as it was while the core was a program that had to be unpacked to be run: the way
    // every phone this has been tried on has had it.
    packaging { jniLibs { useLegacyPackaging = true } }
    splits { abi { isEnable = true; reset(); include("arm64-v8a", "armeabi-v7a"); isUniversalApk = false } }
}

dependencies {
    // the screens and the wallet logic (with Compose, which it passes on); their tests are there too: ./gradlew :shared:desktopTest
    implementation(project(":shared"))
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("com.google.zxing:core:3.5.4") // QR reading; QR drawing comes from the Rust core
}

kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) } }
