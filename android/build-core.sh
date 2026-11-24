#!/bin/sh
# Builds the wallet core for phones (the `phone` profile) as the library the app loads (crates/tachys-c) and puts
# it where the app packs it:
#   app/src/main/jniLibs/<abi>/libtachys_c.so
# Then build the app with ./gradlew assembleRelease (or assembleDebug for a build with the adb test hooks).
#
#   ./build-core.sh              arm64-v8a and armeabi-v7a, the two the release APKs carry
#   ./build-core.sh --emulator   x86_64 as well, left in target/ (the APKs carry the two above only)
#
# The build machine's paths are kept out of the binary. Panic messages carry the source path of the
# code that raised them, which for dependencies is under the builder's home directory; without the
# remapping below a release would publish that directory's name about three hundred times. The
# script fails if any is left.
set -eu
cd "$(dirname "$0")/.."
ROOT=$(pwd)

SDK=${ANDROID_HOME:-$HOME/Android/Sdk}
NDK=${ANDROID_NDK_HOME:-$(ls -d "$SDK"/ndk/* 2>/dev/null | sort -V | tail -1)}
BIN=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin
API=24 # the app's minSdk
[ -x "$BIN/aarch64-linux-android$API-clang" ] || { echo "no Android NDK found under $SDK/ndk (set ANDROID_NDK_HOME)" >&2; exit 1; }

# when several prefixes match, the last one given wins: the catch-all goes first
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/builder --remap-path-prefix=$HOME/.cargo/registry/src=/registry --remap-path-prefix=$HOME/.cargo/git/checkouts=/git --remap-path-prefix=$ROOT=/tachys"
export RUSTFLAGS

build() { # rust target, clang prefix, abi directory (empty: do not copy)
    target=$1; clang=$2; abi=$3
    env_name=$(echo "$target" | tr 'a-z-' 'A-Z_')
    cc_name=$(echo "$target" | tr '-' '_')
    env "CARGO_TARGET_${env_name}_LINKER=$BIN/$clang$API-clang" "CC_$cc_name=$BIN/$clang$API-clang" "AR_$cc_name=$BIN/llvm-ar" \
        cargo build --profile phone -p tachys-c --target "$target"
    out=target/$target/phone/libtachys_c.so
    left=$(strings -a "$out" | grep -c -F "$HOME" || true)
    [ "$left" = 0 ] || { echo "$out still carries $left paths under $HOME; not copied" >&2; exit 1; }
    # the door the app knocks on must be there: without it the app builds, installs, and can do nothing
    "$BIN/llvm-nm" -D --defined-only "$out" | grep -q ' T Java_org_tachys_wallet_Native_call$' || { echo "$out has no JNI entry; not copied" >&2; exit 1; }
    if [ -n "$abi" ]; then
        mkdir -p "android/app/src/main/jniLibs/$abi"
        # the core as a program, from a build before this one: the app no longer runs it and must not carry it
        rm -f "android/app/src/main/jniLibs/$abi/libtachys.so"
        cp "$out" "android/app/src/main/jniLibs/$abi/libtachys_c.so"
    fi
    echo "$target: $(wc -c < "$out") bytes, no build-machine paths${abi:+, copied to jniLibs/$abi}"
}

build aarch64-linux-android aarch64-linux-android arm64-v8a
build armv7-linux-androideabi armv7a-linux-androideabi armeabi-v7a
if [ "${1:-}" = "--emulator" ]; then build x86_64-linux-android x86_64-linux-android ""; fi
