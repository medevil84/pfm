#!/usr/bin/env bash
# build-musl.sh - build a static musl pfm-app on a glibc host and package it.
# Needs: rustup target x86_64-unknown-linux-musl, python3 (pip downloads "ziglang"
# once into ~/.cache/pfm-zig-tools; zig is only used as the musl C cross-compiler for
# bundled SQLite / ring). Usage: ./build-musl.sh [x86_64|aarch64]
set -euo pipefail

ARCH="${1:-x86_64}"
TRIPLE="${ARCH}-unknown-linux-musl"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
TOOLS="${XDG_CACHE_HOME:-$HOME/.cache}/pfm-zig-tools"   # fuori dal repo: venv richiede symlink (no exFAT)
UPPER="$(echo "$TRIPLE" | tr 'a-z-' 'A-Z_')"
LOWER="$(echo "$TRIPLE" | tr '-' '_')"

rustup target add "$TRIPLE"

if [ ! -x "$TOOLS/venv/bin/python" ]; then
  mkdir -p "$TOOLS"
  python3 -m venv "$TOOLS/venv"
  "$TOOLS/venv/bin/pip" install -q ziglang
fi

# cc-rs passes --target=<rust triple>, which zig rejects: drop it, use zig's own.
cat > "$TOOLS/zcc" <<WRAP
#!/bin/sh
for a; do shift; case "\$a" in --target=*) ;; *) set -- "\$@" "\$a";; esac; done
exec "$TOOLS/venv/bin/python" -m ziglang cc -target ${ARCH}-linux-musl "\$@"
WRAP
cat > "$TOOLS/zar" <<WRAP
#!/bin/sh
exec "$TOOLS/venv/bin/python" -m ziglang ar "\$@"
WRAP
chmod +x "$TOOLS/zcc" "$TOOLS/zar"

export "CC_${LOWER}=$TOOLS/zcc" "AR_${LOWER}=$TOOLS/zar"
export "CARGO_TARGET_${UPPER}_LINKER=$TOOLS/zcc"
# zig brings its own crt: don't let rustc add the self-contained one as well.
export "CARGO_TARGET_${UPPER}_RUSTFLAGS=-C link-self-contained=no"

cd "$ROOT"
cargo build --release --locked -p pfm-app --target "$TRIPLE"

"$HERE/package.sh" "$ROOT/target/$TRIPLE/release/pfm-app"
