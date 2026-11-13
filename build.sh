#!/usr/bin/env bash
# Build the PFM workspace (release). Cross-platform POSIX script.
set -euo pipefail

cd "$(dirname "$0")"

# ── Static asset gates ────────────────────────────────────────────────────────
# web/app.js once shipped with 28 U+FFFD characters and a syntax error, which
# meant no frontend JavaScript ran at all. Nothing in the build caught it: Rust
# compiles fine regardless. These two checks make that class of failure loud.

# Only our own text assets: fonts and PNGs can contain any byte sequence, and
# the vendored bundles are not ours to fix.
TEXT_ASSETS=(--include='*.js' --include='*.html' --include='*.json' --include='*.css'
             --include='*.webmanifest' --exclude-dir=vendor)

echo "==> web/: no U+FFFD replacement characters"
if grep -rlP "${TEXT_ASSETS[@]}" '\xef\xbf\xbd' web/ 2>/dev/null | grep -q .; then
  echo "ERROR: replacement characters (U+FFFD) found in:" >&2
  grep -rlP "${TEXT_ASSETS[@]}" '\xef\xbf\xbd' web/ >&2
  echo "A tool has rewritten these files in the wrong encoding. Restore UTF-8." >&2
  exit 1
fi

if command -v node >/dev/null 2>&1; then
  echo "==> node --check on web/*.js"
  for js in web/*.js; do
    node --check "$js"
  done

  echo "==> frontend regression tests"
  node scripts/frontend_test.js
else
  echo "WARNING: node not found; skipping JavaScript syntax check" >&2
fi

# The U+FFFD check above only catches a corruption that went through a decoder
# which emitted replacement characters. The same PowerShell round-trip can also
# leave readable mojibake: 'dY"Z' survived in web/app.js for exactly this
# reason. These are the byte sequences a UTF-8 file cannot legitimately contain.
echo "==> web/: no mojibake from an encoding round-trip"
if grep -rlP "${TEXT_ASSETS[@]}" '(\xc3\x83|\xc3\xa2\xc2\x80|\xc3\xaf\xc2\xbf|dY[\x22\x80-\xbf])' web/ 2>/dev/null | grep -q .; then
  echo "ERROR: mojibake found in:" >&2
  grep -rlP "${TEXT_ASSETS[@]}" '(\xc3\x83|\xc3\xa2\xc2\x80|\xc3\xaf\xc2\xbf|dY[\x22\x80-\xbf])' web/ >&2
  echo "A tool has rewritten these files in the wrong encoding. Restore UTF-8." >&2
  exit 1
fi

# Every element the frontend reaches for must exist in the markup. A renamed or
# removed id used to fail silently at runtime (recEndType against rEndType, and
# a toast written to a #toast that was never in the page).
if command -v python >/dev/null 2>&1; then
  echo "==> web/: every getElementById target exists in index.html"
  python - <<'PY'
import re, sys
idx = open('web/index.html', encoding='utf-8').read()
app = open('web/app.js', encoding='utf-8').read()
present = set(re.findall(r'\bid="([^"]+)"', idx)) | set(re.findall(r'\bid="([^"]+)"', app))
wanted = set(re.findall(r"getElementById\(['\"]([A-Za-z0-9_-]+)['\"]\)", app))
for m in re.finditer(r"querySelector(?:All)?\(\s*[`'\"]([^`'\"]+)[`'\"]", app):
    wanted |= set(re.findall(r"#([A-Za-z][A-Za-z0-9_-]*)", m.group(1)))
missing = sorted(wanted - present)
if missing:
    sys.exit('ERROR: app.js targets ids absent from the markup: ' + ', '.join(missing))
PY

  echo "==> web/i18n: it/en key parity and no missing key"
  python - <<'PY'
import json, re, sys
it = json.load(open('web/i18n/it.json', encoding='utf-8'))
en = json.load(open('web/i18n/en.json', encoding='utf-8'))
diff = sorted(set(it) ^ set(en))
if diff:
    sys.exit('ERROR: it/en catalogues differ on: ' + ', '.join(diff))
app = open('web/app.js', encoding='utf-8').read()
used = set(re.findall(r"\bt\(\s*'([a-zA-Z0-9_.]+)'", app))
missing = sorted(k for k in used if k not in it)
if missing:
    sys.exit('ERROR: keys used by app.js and absent from the catalogues: ' + ', '.join(missing))
PY
else
  echo "WARNING: python not found; skipping DOM and i18n gates" >&2
fi

echo "==> cargo fmt --check"
cargo fmt --check

echo "==> cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

echo "==> cargo build --release"
cargo build --release

echo "Build complete: target/release/pfm-app"