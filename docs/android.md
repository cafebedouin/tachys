# The phone app

`android/` is a Kotlin (Compose) app around the same core the command line uses. The core is built as a library
(`crates/tachys-c`, `libtachys_c.so`) that the app loads and calls in its own process, once for each thing the app
does, with the command line the `tachys` program would be given (`Native.kt`; one JNI method, and only arrays of
bytes cross it). The app decides on the JSON answer (`json.md`) and the code the command ended with and on nothing
else (it shows the sentences, and draws a QR code from the rows `tachys qr` prints). Calls run side by side: the
address shown as a QR code does not wait for a sync. A panic inside the core is an answer the app shows; a stack
overflow or an abort inside it would close the app, where it used to end only the core's own process (finding 64).

## Build

```
android/build-core.sh              # the core for arm64 and 32-bit ARM, into app/src/main/jniLibs/<abi>/libtachys_c.so
cd android && ./gradlew assembleRelease     # or assembleDebug, the only build that keeps the adb test hooks
```

An app built without the first step installs and starts and can do nothing: every answer is "the core is not in
this build". The app's tests call the core too, through the same JNI method, when the build machine has the library
for itself and says where; without `TACHYS_HOST_CORE` those tests are skipped:

```
cargo build -p tachys-c --features jni
cd android && TACHYS_HOST_CORE=$PWD/../target/debug ./gradlew testDebugUnitTest
```

`assembleRelease` needs a signing key and is refused without one: name your own (the comment in
`android/app/build.gradle.kts` says how; an app signed with another key than the project's will not install over
the project's), or build the copy below, or `assembleDebug`.

To try a build on a phone that is in use, `./gradlew assembleRelease -PtachysIdSuffix=.test` makes a copy that
installs beside the real app (`org.tachys.wallet.test`) with its own wallets. It needs no key: it is signed with
the debug key.

`build-core.sh` needs the Android NDK and the Rust targets `aarch64-linux-android` and `armv7-linux-androideabi`.
It keeps the build machine's paths out of the binary and fails if any is left. The launcher icons are made from
`img/tachys-logo.jpg` by `android/make-icons.py` and are checked in; run it again only if the logo changes.

## What it does

Several wallets, mainnet and testnet. Create, restore from a seed phrase, or watch an address. Sync (a tip the
peers agree on, then every box proven). Receive: an address to show or copy, and a receipt to check. Send: ERG and
tokens, the fee in ERG or in a token, a receipt to hand the payee; with no network, the payment is signed and
waits, and the receipt carries the signed transaction so the payee can submit it too. Sign: a request scanned
from another site (ErgoPay), read back in the wallet's own words before anything is signed. Details: pending
payments, the boxes (and a button to make them one), history.

Seed phrases are sealed with a password the app makes and keeps in the Android Keystore, released only by the
phone's screen lock. A phone with no screen lock gets the encryption but no guard.

A wallet can instead be given a password of its own when it is made or restored ("Guarded by": Screen lock or
Password). Its key is then opened by that password and never by the screen lock, so the wallet can be handed to
someone, or to an agent driving the phone, without the phone's lock. The password is the whole wallet (it shows
the seed phrase), and a weak one can be guessed from a copy of the wallet's file; a forgotten one is put right by
restoring from the written words (finding 65).

Nodes are found automatically (well-known ones first). To use one node only, your own say, write its address in
Settings as `http://HOST:PORT`.

## Signing

A release build is signed with the key named in `~/.gradle/gradle.properties` (`tachysStoreFile` and the rest; see
`app/build.gradle.kts`); with none it is signed with the debug key. A release of this project carries APKs signed with
the project's key (`arm64-v8a` for phones from about 2016 on, `armeabi-v7a` for older ones). One you build
yourself is signed by you, and Android will only update an app with another signed by the same key: removing
the app to change keys removes its wallets, so write the seed phrases down first.

## What has and has not been run

Run by hand on a Galaxy Note 2 and a Pixel 6 over the project's life: sync, receive by receipt, send, a signing
request, restore. After the last change to the build (finding 49) a copy holding a read-only wallet was driven
over adb on a Galaxy Note 2 (Android 7.1.2) and a Pixel 6 (Android 17): sync, Receive and its Copy button, the
Boxes list, the back button. The build released as v0.0.1 was then tried by hand on the Note 2 by
the maintainer, who reports that most of the app was tried and that everything tried worked; which screens was
not written down, and no log was kept. It has not been run on the Pixel 6 or any other phone.

After findings 52 to 57 a debug build was driven over adb on the Note 2, on testnet (finding 58,
`research/runs/2026-11-20-note2-screens.txt`): restore from a phrase (its first sync linking to the checkpoint),
Receive with Copy and a receipt checked, Send, consolidation, and a signing request opened as an `ergopay:` link and
signed; each payment was proven from the payee's side. Two faults found there are fixed: a request opened while the
app synced stayed at "Reading…", and coins seen only in a block one node had shown made the card say "no boxes yet".
The New, Restore and Read-only forms now go back to the three choices. Not run since: the release build, mainnet, the
camera, the Pixel 6.

Since the core became a library the app calls in its own process (finding 64), a release build of the copy that
installs beside the app has run on the Note 2, on mainnet, with a read-only wallet: a sync, the balance (the same
as the command line gave), Receive opened while a sync ran, a rename while a sync ran. With a wallet sealed by a password of its own (finding 65) it has also
made, opened, removed and restored a wallet, and paid 0.01 ERG to it and back on mainnet. Not run since: anything
behind the screen lock, a signing request, testnet, the Pixel 6, the camera.
