plugins {
    id("org.jetbrains.kotlin.multiplatform")
    id("com.android.kotlin.multiplatform.library")
    id("org.jetbrains.compose")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The screens and the wallet logic, once, for every platform, in src/commonMain: nothing there is of the JVM or of any
// one system. Android and a computer (the JVM: Linux, Windows, macOS) add the few lines in src/jvmShared, which both
// compile, and each its own Platform. The iPhone targets are declared so that the common code is held to what an
// iPhone has; they are built only on a Mac, and src/iosMain has never been compiled (docs/platforms.md).
val composeVersion = "1.12.2"

kotlin {
    android {
        namespace = "org.tachys.wallet.shared"
        compileSdk = 37
        minSdk = 24
    }
    jvm("desktop")
    // (never built here: see the top of this file)
    listOf(iosArm64(), iosSimulatorArm64()).forEach { target ->
        target.compilations.getByName("main").cinterops.create("tachys") {
            definitionFile.set(file("src/nativeInterop/cinterop/tachys.def"))
            includeDirs(rootDir.resolve("../crates/tachys-c"))
        }
    }
    sourceSets {
        commonMain.dependencies {
            api("org.jetbrains.compose.runtime:runtime:$composeVersion")
            api("org.jetbrains.compose.foundation:foundation:$composeVersion")
            api("org.jetbrains.compose.material:material:$composeVersion")
            api("org.jetbrains.compose.ui:ui:$composeVersion")
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0")
            implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.13.0") // the JSON the core answers in (Json.kt)
            implementation("com.squareup.okio:okio:3.18.2") // files (Files.kt)
        }
        androidMain { kotlin.srcDir("src/jvmShared/kotlin") }
        getByName("desktopMain") {
            kotlin.srcDir("src/jvmShared/kotlin")
            // the seed lists and token names the app ships: the Android app's assets, the same files
            resources.srcDir("../app/src/main/assets")
            dependencies {
                implementation(compose.desktop.currentOs)
                implementation("org.jetbrains.kotlinx:kotlinx-coroutines-swing:1.11.0")
            }
        }
        getByName("desktopTest") {
            dependencies {
                implementation("junit:junit:4.13.2")
                implementation("org.json:json:20240303") // what Json.kt is held to (JsonTest.kt)
                // the screens driven with no window (ScreensTest.kt)
                implementation("org.jetbrains.compose.ui:ui-test-junit4:$composeVersion")
                implementation(compose.desktop.currentOs)
            }
        }
    }
}

// The core is found on java.library.path: TACHYS_HOST_CORE is the directory holding the library tachys_c, built
// with `cargo build -p tachys-c --features jni` (--release for use). The tests call it when it is there (NativeTest.kt).
val hostCore: String? = System.getenv("TACHYS_HOST_CORE")
tasks.withType<Test>().configureEach { hostCore?.let { systemProperty("java.library.path", it) } }

// A packaged app for this machine's system, with its own Java and the core inside: ./gradlew :shared:createDistributable
// (a folder under build/compose/binaries), with TACHYS_HOST_CORE naming a release build of the library.
val packedCore = layout.buildDirectory.dir("core")
val packCore = tasks.register<Copy>("packCore") {
    from(hostCore ?: "no-core-named") { include(System.mapLibraryName("tachys_c")) }
    into(packedCore.map { it.dir("common") })
    doLast { if (!packedCore.get().file("common/" + System.mapLibraryName("tachys_c")).asFile.exists()) throw GradleException("TACHYS_HOST_CORE must name the directory holding " + System.mapLibraryName("tachys_c")) }
}
tasks.matching { it.name == "prepareAppResources" }.configureEach { dependsOn(packCore) }

compose.desktop {
    application {
        mainClass = "org.tachys.wallet.MainKt"
        hostCore?.let { jvmArgs("-Djava.library.path=$it") }
        nativeDistributions {
            packageName = "tachys"
            packageVersion = "1.0.0" // the packagers of some systems refuse a version below 1; the app's own version is the Android app's
            appResourcesRootDir.set(packedCore)
        }
    }
}
