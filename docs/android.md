# The phone app

`android/` is a Kotlin (Compose) app around the same core the command line uses. The core is built as a program,
packed where Android will let it run, and started once for each thing the app does; the app decides on its JSON answer
(`json.md`) and exit code and on nothing else (it shows the sentences, and draws a QR code from the rows `tachys qr` prints). Starting it costs about 16 ms on a Galaxy Note 2 (2012, Android 7.1.2),
the oldest phone it has run on.

## Build

```
android/build-core.sh              # the core for arm64 and 32-bit ARM, into app/src/main/jniLibs/<abi>/libtachys.so
cd android && ./gradlew assembleRelease     # or assembleDebug, the only build that keeps the adb test hooks
```

To try a build on a phone that is in use, `./gradlew assembleRelease -PtachysIdSuffix=.test` makes a copy that
installs beside the real app (`org.tachys.wallet.test`) with its own wallets.

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
