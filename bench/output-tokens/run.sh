#!/usr/bin/env bash
# Repeats the published measurement: downloads each tool's release and checks
# it against its published checksum, builds this checkout's rtk, then runs
# output-tokens over corpus.toml. Linux x86_64; needs git, curl, cargo, node,
# uv and rg. Usage: bench/output-tokens/run.sh <work directory>
set -euo pipefail

WORK=$(mkdir -p "${1:?usage: run.sh <work directory>}" && cd "$1" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
T=$WORK/tools
mkdir -p "$T"

# fetch URL FILE: downloads once.
fetch() { [ -s "$2" ] || curl -fsSL -o "$2" "$1"; }
# verify LIST FILE: the line for FILE in a sha256sum-style list must match.
verify() { awk -v f="$2" '$2 == f || $2 == "*" f { print $1 "  " f }' "$1" | grep . | sha256sum -c -; }

RTK=https://github.com/rtk-ai/rtk/releases/download/v1.2.0
PL=https://github.com/yungSNAT/PascalsLetter/releases/download/fork-v1.2.0-1
TOKENADE=https://github.com/pi-infected/tokenade/releases/download/v2.0.9
SNIP=https://github.com/edouard-claude/snip/releases/download/v0.26.0

mkdir -p "$T/rtk" "$T/rtk-oss" "$T/pl-oss" "$T/tokenade" "$T/snip"
(cd "$T/rtk" && fetch "$RTK/checksums.txt" checksums.txt &&
  fetch "$RTK/rtk-x86_64-unknown-linux-musl.tar.gz" rtk-x86_64-unknown-linux-musl.tar.gz &&
  verify checksums.txt rtk-x86_64-unknown-linux-musl.tar.gz &&
  tar -xzf rtk-x86_64-unknown-linux-musl.tar.gz)
(cd "$T/rtk-oss" && fetch "$RTK/checksums.txt" checksums.txt &&
  fetch "$RTK/rtk-oss-x86_64-unknown-linux-musl.tar.gz" rtk-oss-x86_64-unknown-linux-musl.tar.gz &&
  verify checksums.txt rtk-oss-x86_64-unknown-linux-musl.tar.gz &&
  tar -xzf rtk-oss-x86_64-unknown-linux-musl.tar.gz)
(cd "$T/pl-oss" && fetch "$PL/checksums.txt" checksums.txt &&
  fetch "$PL/rtk-oss-x86_64-unknown-linux-musl.tar.gz" rtk-oss-x86_64-unknown-linux-musl.tar.gz &&
  verify checksums.txt rtk-oss-x86_64-unknown-linux-musl.tar.gz &&
  tar -xzf rtk-oss-x86_64-unknown-linux-musl.tar.gz)
(cd "$T/tokenade" && A=tokenade-2.0.9-x86_64-unknown-linux-musl.tar.gz &&
  fetch "$TOKENADE/$A" "$A" && fetch "$TOKENADE/$A.sha256" "$A.sha256" &&
  echo "$(cut -d' ' -f1 "$A.sha256")  $A" | sha256sum -c - && tar -xzf "$A")
(cd "$T/snip" && A=snip_0.26.0_linux_amd64.tar.gz &&
  fetch "$SNIP/checksums.txt" checksums.txt && fetch "$SNIP/$A" "$A" &&
  verify checksums.txt "$A" && tar -xzf "$A")
[ -d "$T/chisle" ] ||
  git clone --quiet --depth 1 --branch v3.12.1 https://github.com/JayPokale/Chisle "$T/chisle"

# This checkout's full edition, as `cargo install --git` builds it.
CARGO_TARGET_DIR=$WORK/target-rtk cargo build --release --locked --manifest-path "$ROOT/Cargo.toml"
CARGO_TARGET_DIR=$WORK/target-bench cargo build --release --manifest-path "$HERE/Cargo.toml"

OUT=$WORK/run-$(date -u +%Y%m%dT%H%M%SZ)
"$WORK/target-bench/release/output-tokens" \
  --corpus "$HERE/corpus.toml" --work "$WORK/repos" --out "$OUT" \
  --rtk "rtk 1.2.0=$T/rtk/rtk" \
  --rtk "rtk-oss 1.2.0=$T/rtk-oss/rtk" \
  --rtk "PascalsLetter fork-v1.2.0-1 oss=$T/pl-oss/rtk" \
  --rtk "PascalsLetter (this checkout)=$WORK/target-rtk/release/rtk" \
  --wrap "Tokenade 2.0.9=env TOKENADE_NO_AUTO_UPDATE=1 TOKENADE_NO_STAR_INVITE=1 $T/tokenade/tokenade wrap {qcmd}" \
  --wrap "snip 0.26.0=$T/snip/snip {cmd}" \
  --chisle "Chisle 3.12.1=$T/chisle/hooks/chisle-compress-output.js"
{ cargo --version; cargo clippy --version; node --version; uv --version
  "$WORK/repos/click/.venv/bin/python" --version; git --version; rg --version | head -1; } >"$OUT/toolchain.txt"
