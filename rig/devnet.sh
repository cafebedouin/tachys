#!/usr/bin/env bash
# One patched node on a private devnet, for prototyping wallet-facing node features.
# usage: rig/devnet.sh <jar> <dir> <mine-poll, e.g. 200ms or 5s>     (run from a node runtime directory)
# Ports: peer protocol 19021, REST 19052 (localhost only). Test keys only; never a real network.
set -euo pipefail
JAR="$1"; DIR="$2"; POLL="${3:-2s}"
# optional: P2P_PORT, REST_PORT, KNOWN_PEERS (HOCON list), MINING (true/false)
P2P_PORT="${P2P_PORT:-19021}"; REST_PORT="${REST_PORT:-19052}"; KNOWN_PEERS="${KNOWN_PEERS:-[]}"; MINING="${MINING:-true}"; BIND_IP="${BIND_IP:-127.0.0.1}"
mkdir -p "$DIR/tmp" "$DIR/data"
cat > "$DIR/node.conf" <<CONF
ergo.directory="$DIR/data"
ergo.node.mining=$MINING
ergo.node.offlineGeneration=$MINING
ergo.node.useExternalMiner=false
ergo.node.internalMinerPollingInterval=$POLL
ergo.wallet.testMnemonic="ozone drill grab fiber curtain grace pudding thank cruise elder eight picnic"
ergo.wallet.testKeysQty=5
scorex.network.bindAddress="$BIND_IP:$P2P_PORT"
scorex.network.declaredAddress="$BIND_IP:$P2P_PORT"
scorex.network.knownPeers=$KNOWN_PEERS
scorex.network.allowLocal=true
scorex.network.nodeName="tachys-devnet-$P2P_PORT"
scorex.restApi.bindAddress="$BIND_IP:$REST_PORT"
scorex.restApi.apiKeyHash="324dcf027dd4a30a932c441f365a25e86b173defa4b8e58948253471b81b72cf"
scorex.network.magicBytes=[116,97,99,104]
ergo.node.stateType=utxo
ergo.node.verifyTransactions=true
ergo.node.blocksToKeep=-1
ergo.node.PoPoWBootstrap=false
ergo.node.minimalSuffix=10
ergo.chain.powScheme.powType=autolykos
ergo.chain.powScheme.k=32
ergo.chain.powScheme.n=26
${EXTRA_CONF:-}
CONF
exec java -Xmx768m -Djava.io.tmpdir="$DIR/tmp" -jar "$JAR" --devnet -c "$DIR/node.conf"
