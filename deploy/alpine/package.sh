#!/bin/sh
# package.sh - Build Alpine Linux tarball for pfm service
# Usage: ./package.sh [path-to-binary]
# Requires: POSIX sh, tar, file, grep, sed, sha256sum

set -eu

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
BINARY_PATH="${1:-$SCRIPT_DIR/../../target/x86_64-unknown-linux-musl/release/pfm-app}"

# Resolve absolute path
if ! expr "$BINARY_PATH" : '/' > /dev/null; then
  BINARY_PATH="$(cd "$(dirname "$BINARY_PATH")" && pwd)/$(basename "$BINARY_PATH")"
fi

echo "=== PFM Alpine Package Builder ==="
echo "Binary: $BINARY_PATH"

# Verify binary exists
if [ ! -f "$BINARY_PATH" ]; then
  echo "ERROR: Binary not found: $BINARY_PATH" >&2
  exit 1
fi

# Check binary is musl-compatible ELF
if command -v file > /dev/null 2>&1; then
  FILE_OUTPUT=$(file "$BINARY_PATH" 2>/dev/null || echo "")
  if echo "$FILE_OUTPUT" | grep -q "dynamically linked"; then
    if ! echo "$FILE_OUTPUT" | grep -q "musl"; then
      echo "ERROR: Binary is dynamically linked with non-musl libc" >&2
      echo "Output: $FILE_OUTPUT" >&2
      exit 1
    fi
  fi
  # Extract arch
  if echo "$FILE_OUTPUT" | grep -q "x86-64"; then
    ARCH="x86_64"
  elif echo "$FILE_OUTPUT" | grep -q "aarch64"; then
    ARCH="aarch64"
  else
    ARCH=$(uname -m)
  fi
else
  ARCH=$(uname -m)
fi

echo "Architecture: $ARCH"

# Extract version from Cargo.toml
CARGO_PATH="$SCRIPT_DIR/../../Cargo.toml"
if [ ! -f "$CARGO_PATH" ]; then
  echo "ERROR: Cargo.toml not found: $CARGO_PATH" >&2
  exit 1
fi

VERSION=$(grep -A 10 '^\[workspace\.package\]' "$CARGO_PATH" | \
  grep '^version = ' | sed 's/^version = "\([^"]*\)".*/\1/')

if [ -z "$VERSION" ]; then
  echo "ERROR: Could not extract version from Cargo.toml" >&2
  exit 1
fi

echo "Version: $VERSION"

# Create staging directory
STAGING="pfm-${VERSION}-${ARCH}-musl"
STAGING_DIR=$(mktemp -d)
trap "rm -rf $STAGING_DIR" EXIT

mkdir -p "$STAGING_DIR/$STAGING"
cd "$STAGING_DIR/$STAGING"

# Copy binary
cp "$BINARY_PATH" pfm-app

# Copy files directory
cp -r "$SCRIPT_DIR/files" .

# Copy install/uninstall scripts and README if it exists
cp "$SCRIPT_DIR/install.sh" .
cp "$SCRIPT_DIR/uninstall.sh" .
if [ -f "$SCRIPT_DIR/README.txt" ]; then
  cp "$SCRIPT_DIR/README.txt" .
fi

# Strip CRLF from shell scripts and config files
for file in install.sh uninstall.sh files/etc/init.d/pfm files/etc/conf.d/pfm \
            files/etc/pfm/config.toml.template files/etc/pfm/backup.conf \
            files/usr/local/sbin/pfm-setup files/usr/local/sbin/pfm-backup \
            files/usr/local/sbin/pfm-rclone-setup; do
  if [ -f "$file" ]; then
    sed -i 's/\r$//' "$file"
  fi
done

# Normalize permissions
chmod 755 pfm-app install.sh uninstall.sh
if [ -f README.txt ]; then
  chmod 644 README.txt
fi
chmod 755 files/etc/init.d/pfm files/usr/local/sbin/pfm-setup \
         files/usr/local/sbin/pfm-backup files/usr/local/sbin/pfm-rclone-setup
chmod 644 files/etc/conf.d/pfm files/etc/pfm/config.toml.template files/etc/pfm/backup.conf

# Create dist directory
mkdir -p "$SCRIPT_DIR/dist"

# Build tarball
TARBALL="$SCRIPT_DIR/dist/pfm-${VERSION}-${ARCH}-musl.tar.gz"
cd "$STAGING_DIR"
tar --owner=0 --group=0 --numeric-owner --sort=name -czf "$TARBALL" "$STAGING"

# Create sha256 file
SHA256FILE="${TARBALL%.tar.gz}.sha256"
sha256sum "$TARBALL" > "$SHA256FILE"

# Report results
TARBALL_SIZE=$(ls -lh "$TARBALL" | awk '{print $5}')
SHA256_SIZE=$(ls -lh "$SHA256FILE" | awk '{print $5}')

echo ""
echo "=== Build Complete ==="
echo "Tarball: $TARBALL ($TARBALL_SIZE)"
echo "SHA256:  $SHA256FILE ($SHA256_SIZE)"
