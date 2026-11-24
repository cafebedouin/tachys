# The app on other platforms

The app's screens and its wallet logic are written once, in `android/shared/src/commonMain` (Kotlin, Compose
Multiplatform), with nothing of the JVM or of any one system in them. A
platform adds only what the screens need of the machine: where files live, a few settings, the clipboard, and, where
there is one, a screen lock, a camera and a back button (`Platform.kt`). The Rust core is the same everywhere and is
called through `crates/tachys-c`.

| platform | state | calls the core through |
|---|---|---|
| Android | the released app (`android/app`) | the JNI entry of `libtachys_c.so` |
| Linux | runs; a wallet made and opened in its tests, a mainnet tip agreed in a window under WSL | the JNI entry of `libtachys_c.so` |
| Windows, macOS | the same code as Linux; never built or run there | the JNI entry of `tachys_c.dll` / `libtachys_c.dylib` |
| iPhone, iPad | the shared code is ready for it and held to that by the build; no app, and nothing ever built | `tachys.h` and the static library |

## On a computer (Linux, Windows, macOS)

You need a JDK (17 or later) and Rust.

```
cargo build --release -p tachys-c --features jni       # the core, with the entry a JVM calls
cd android
TACHYS_HOST_CORE=$PWD/../target/release ./gradlew :shared:run            # the app, in a window
TACHYS_HOST_CORE=$PWD/../target/debug ./gradlew :shared:desktopTest      # its tests (cargo build without --release)
```

`TACHYS_HOST_CORE` is the directory that holds the library. Wallets and settings are kept in the place the system
gives a program's data, in a folder `tachys-app` (`~/.local/share`, `%APPDATA%`, `~/Library/Application Support`),
or in `TACHYS_APP_DIR` if that is set.

What differs from a phone:
- **Every wallet has a password of its own** (finding 65). The app has no screen lock to put a key behind here, so
  New and Restore ask for a password and nothing else, and there is no app lock.
- **No camera.** Codes are pasted.
- The sentences say "this computer".

To make an app that runs without the source tree, with its own Java and the core inside it:

```
cargo build --release -p tachys-c --features jni
cd android && TACHYS_HOST_CORE=$PWD/../target/release ./gradlew :shared:createDistributable
```

It is a folder under `android/shared/build/compose/binaries/main/app/` (148 MB on Linux), started by `tachys/bin/tachys`.
An installer for a system (`packageDeb`, `packageMsi`, `packageDmg`) is the same plugin's work and has not been tried.

On macOS and Windows this is the same code and should need only the two commands above, since the screens and the
JNI entry are the ones that run on Linux. Nobody has tried it: if you do, an issue saying what happened is welcome.

## iPhone and iPad

There is no iPhone app. What is there:

- **The shared code needs nothing an iPhone lacks.** The screens and the wallet logic use no JVM class: files go
  through okio, JSON through `Json.kt` (on `kotlinx.serialization`), amounts through `Decimal.kt` (exact, on the
  digits). The build declares the iPhone targets, so `./gradlew :shared:compileCommonMainKotlinMetadata` fails, on
  any machine, if something of one platform creeps back in. CI runs it.
- **A first draft of the three things the wallet logic needs of iOS** (`android/shared/src/iosMain/Ios.kt`): the
  call into the core through `tachys.h`, a background thread, a lock, the file system. It has **never been
  compiled**: nothing for an iPhone can be built on Linux, where the project is developed. Expect to correct it.

What someone with a Mac and Xcode would still do:

1. Build the core for the targets: `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`, then
   `cargo build --release -p tachys-c --target …` for each, which gives `libtachys_c.a`. Not tried; some
   dependency may not build there.
2. Get `iosMain` through the compiler (`./gradlew :shared:compileKotlinIosSimulatorArm64`).
3. Write a `Platform` for iOS, as `Phone` (in `android/app`) and `Computer` (in `desktopMain`) are: the app's
   documents directory, `NSUserDefaults`, the pasteboard, a date formatter; `lock = false` to begin with, which makes
   every wallet one with a password of its own. To offer the screen lock as a guard, the Keychain with Face ID or
   the passcode would play the part the Android keystore plays in `MainActivity.kt`.
4. An Xcode project that shows the shared `App` (Compose Multiplatform's `ComposeUIViewController`) and links
   `libtachys_c.a`.
5. A scanner, if codes are to be read by the camera.

Step 3 is perhaps a hundred lines; the rest is building and correcting. If you do it, a pull request is welcome on
the usual terms (`CONTRIBUTING.md`).
