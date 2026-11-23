#!/bin/sh
# Builds the Linux x86_64 program a release carries, and its line of SHA256SUMS:
#   target/dist/tachys-VERSION-linux-x86_64   target/dist/SHA256SUMS
# The release workflow (.github/workflows/release.yml) runs this on a tag; it runs the same on any Linux machine.
#
# As android/build-core.sh does for the phone, the build machine's paths are kept out of the binary (panic messages
# carry the source path of the code that raised them), and the script fails if any is left.
set -eu
cd "$(dirname "$0")"
ROOT=$(pwd)
TARGET=x86_64-unknown-linux-gnu

# when several prefixes match, the last one given wins: the catch-all goes first
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/builder --remap-path-prefix=$HOME/.cargo/registry/src=/registry --remap-path-prefix=$HOME/.cargo/git/checkouts=/git --remap-path-prefix=$ROOT=/tachys"
export RUSTFLAGS

# a target named, so that these flags do not rebuild what an ordinary `cargo build --release` left in target/release
cargo build --release --locked -p tachys --target "$TARGET"
built=target/$TARGET/release/tachys

version=$("$built" version --json 2>/dev/null | sed -n 's/.*"version":"\([^"]*\)".*/\1/p')
[ -n "$version" ] || { echo "the program built does not say its version" >&2; exit 1; }
name=tachys-$version-linux-x86_64
mkdir -p target/dist
cp "$built" "target/dist/$name"
strip "target/dist/$name"

for dir in "$HOME" "$ROOT"; do
    left=$(strings -a "target/dist/$name" | grep -c -F "$dir" || true)
    [ "$left" = 0 ] || { echo "target/dist/$name still carries $left paths under $dir" >&2; exit 1; }
done

# the newest glibc symbol it asks for: the oldest glibc it runs on
glibc=$(objdump -T "target/dist/$name" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1 | cut -d_ -f2)
( cd target/dist && sha256sum "$name" > SHA256SUMS )
echo "$name: $(wc -c < "target/dist/$name") bytes, needs glibc $glibc, no build-machine paths"
cat target/dist/SHA256SUMS
