#!/bin/sh
# Refreshes the release checkpoint (anchors/mainnet.json, anchors/testnet.json): the first step of a release
# (docs/maintaining.md). It is the one thing every install trusts before it has heard from anyone, so this script
# fetches and compares, and leaves the reading of the result, and the commit, to the maintainer.
#
#   ./refresh-anchors.sh            both networks: fetch, compare, write the files, say what to do next
#   ./refresh-anchors.sh --check    the same, writing nothing
#   ./refresh-anchors.sh testnet    one network only (mainnet, testnet)
#   MAINNET_NODES="URL URL ..." TESTNET_NODES="URL ..." ./refresh-anchors.sh     other nodes than the well-known ones
#   MAINNET_EXPLORERS="URL ..." TESTNET_EXPLORERS="URL ..." (or "" for none)     other explorers
#
# For each network: every node is asked for its height; the header taken is the one a day of blocks below the lowest
# height any of them gives (never one of the newest: a day is far beyond any reorganisation seen). Each node is
# asked for the id at that height and for that header, and every one must give the same id and the same header (but
# for `size`, a node's own count that is no part of the header; the first node's is what is written). Then
# each explorer is asked for the id at that height: it gives no header this script reads, but the id is the hash of
# the header (which `cargo test --workspace` checks of both checkpoints), so an explorer with the same id has the
# same header. Two sources that agree are needed, nodes and explorers counted alike: testnet has one well-known
# node, and its explorer is the second. An explorer that does not answer is said and not counted; one that gives
# another id stops the refresh. With one source only the header is that node's word alone, which the script says,
# and writes only if told to (--one-source). Whether a node and an explorer are run by the same people the script
# cannot know: sources of your own, named above, are worth more than the well-known ones.
#
# What it does not check: that the header is on the chain this wallet's last release links to, or its proof of work.
# `cargo test --workspace` afterwards checks the work of both checkpoints (tachys-verify/tests/pow.rs).
set -eu
cd "$(dirname "$0")"

MAINNET_NODES=${MAINNET_NODES:-"http://213.239.193.208:9053 http://128.253.41.49:9053"}
TESTNET_NODES=${TESTNET_NODES:-"http://213.239.193.208:9052"}
MAINNET_EXPLORERS=${MAINNET_EXPLORERS-"https://api.ergoplatform.com"}
TESTNET_EXPLORERS=${TESTNET_EXPLORERS-"https://api-testnet.ergoplatform.com"}
check=0; one_source=0; only=""
for a in "$@"; do case "$a" in
    --check) check=1 ;;
    --one-source) one_source=1 ;;
    mainnet|testnet) only="$only $a" ;;
    *) echo "usage: $0 [--check] [--one-source] [mainnet] [testnet]" >&2; exit 2 ;;
esac; done
for c in curl jq; do command -v "$c" >/dev/null || { echo "missing: $c" >&2; exit 2; }; done

get() { curl -sS -f --max-time 30 --retry 2 --retry-delay 5 --retry-connrefused "$1"; }
failed=0

refresh() { # network, blocks in a day, its nodes, its explorers
    net=$1; day=$2; nodes=$3; explorers=$4
    file=anchors/$net.json
    old=$(jq -r .height "$file")
    lowest=""
    for n in $nodes; do
        h=$(get "$n/info" | jq -r '.fullHeight // empty') || h=""
        [ -n "$h" ] || { echo "$net: $n did not give its height" >&2; return 1; }
        echo "$net: $n is at height $h"
        if [ -z "$lowest" ] || [ "$h" -lt "$lowest" ]; then lowest=$h; fi
    done
    at=$((lowest - day))
    [ "$at" -gt "$old" ] || { echo "$net: a day below the tip ($at) is not above the checkpoint there is ($old)" >&2; return 1; }
    id=""; header=""; same=""; agree=0
    for n in $nodes; do
        i=$(get "$n/blocks/at/$at" | jq -r '.[0] // empty') || i=""
        [ -n "$i" ] || { echo "$net: $n has no block at height $at" >&2; return 1; }
        # the fields in one order, so that two nodes' answers can be compared as text
        j=$(get "$n/blocks/$i/header" | jq -cS .) || j=""
        [ "$(echo "$j" | jq -r '.id // empty')" = "$i" ] && [ "$(echo "$j" | jq -r '.height // empty')" = "$at" ] || { echo "$net: $n gave a header that is not block $i at height $at" >&2; return 1; }
        # `size` is each node's own count of the header's bytes, not part of the header: two honest nodes differ in it
        k=$(echo "$j" | jq -cS 'del(.size)')
        if [ -z "$id" ]; then id=$i; header=$j; same=$k
        elif [ "$i" != "$id" ]; then echo "$net: the nodes DISAGREE at height $at: $id and $i ($n). Nothing written" >&2; return 1
        elif [ "$k" != "$same" ]; then echo "$net: the nodes give different headers for $id ($n). Nothing written" >&2; return 1
        fi
        agree=$((agree + 1))
    done
    for e in $explorers; do
        i=$(get "$e/api/v0/blocks/at/$at" | jq -r '.[0] // empty') || i=""
        if [ -z "$i" ]; then echo "$net: the explorer $e gave no id at height $at: not counted" >&2
        elif [ "$i" != "$id" ]; then echo "$net: the explorer $e DISAGREES at height $at: $i, and the nodes $id. Nothing written" >&2; return 1
        else echo "$net: the explorer $e has the same id at height $at"; agree=$((agree + 1))
        fi
    done
    age=$(( ($(date +%s) - $(echo "$header" | jq -r .timestamp) / 1000) / 3600 ))
    echo "$net: height $at, id $id, $age hours old, the same from $agree source(s); the checkpoint there is at $old"
    if [ "$agree" -lt 2 ] && [ "$one_source" = 0 ]; then
        echo "$net: ONE SOURCE ONLY. Check the id on a second one (another explorer, a node of your own), then run again with --one-source, or name a second node in $(echo "$net" | tr a-z A-Z)_NODES. Nothing written" >&2
        return 1
    fi
    [ "$agree" -ge 2 ] || echo "$net: written on one node's word (--one-source)"
    if [ "$check" = 1 ]; then echo "$net: --check, nothing written"; return 0; fi
    # the first node's answer, its fields in jq's sorted order, on one line
    echo "$header" > "$file.tmp" && mv "$file.tmp" "$file"
    echo "$net: wrote $file"
}

wanted() { [ -z "$only" ] || case " $only " in *" $1 "*) true ;; *) false ;; esac; }
if wanted mainnet; then refresh mainnet 720 "$MAINNET_NODES" "$MAINNET_EXPLORERS" || failed=1; fi
if wanted testnet; then refresh testnet 1920 "$TESTNET_NODES" "$TESTNET_EXPLORERS" || failed=1; fi

[ "$check" = 1 ] || cat <<'EOF'

Next (docs/maintaining.md):
  git diff --stat anchors/                 one line in each file refreshed
  cargo test --workspace                   no test leans on which header the checkpoint is: read any failure
  cargo run -q -p tachys -- version        the age it now says
EOF
exit $failed
