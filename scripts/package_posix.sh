#!/usr/bin/env bash
# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# package_posix.sh — assemblage des artefacts Linux/macOS.
#
# Produit dans dist/ :
#   <target>/                                  binaires + web/ + manifest
#   OnionBit-<ver>-<os>-<arch>.tar.gz          tarball de distribution
#   onionbit_<ver>_<arch>.deb                  paquet Debian/Ubuntu
#                                            (linux + dpkg-deb requis)
#
# Usage :
#   ./scripts/package_posix.sh
#   ./scripts/package_posix.sh --target aarch64-unknown-linux-gnu
#   ./scripts/package_posix.sh --web-dir /chemin/app/build/web
#   ./scripts/package_posix.sh --skip-build    # binaires deja compiles
#
# Layout du .deb : /opt/onionbit/{daemon,cli,web/} + liens /usr/bin —
# l'auto-detection `<exe>/web` du daemon fonctionne car current_exe
# resout le lien symbolique vers le vrai chemin. Unite systemd
# utilisateur `onionbit-daemon.service` incluse.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET=""
WEB_DIR=""
SKIP_BUILD=0

while [ $# -gt 0 ]; do
    case "$1" in
        --target)    TARGET="$2"; shift 2 ;;
        --web-dir)   WEB_DIR="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        *) echo "argument inconnu : $1" >&2; exit 2 ;;
    esac
done

if [ -z "$TARGET" ]; then
    TARGET="$(rustc -vV | awk '/^host:/ {print $2}')"
fi
[ -z "$WEB_DIR" ] && WEB_DIR="$ROOT/app/build/web"

VER="$(cargo pkgid -p onionbit-daemon --manifest-path "$ROOT/Cargo.toml" | awk -F'[#@]' '{print $NF}')"
case "$TARGET" in
    x86_64-unknown-linux-gnu)    OS=linux;  ARCH=x64 ;;
    aarch64-unknown-linux-gnu)   OS=linux;  ARCH=arm64 ;;
    aarch64-apple-darwin)        OS=macos;  ARCH=arm64 ;;
    x86_64-apple-darwin)         OS=macos;  ARCH=x64 ;;
    x86_64-pc-windows-*)         OS=windows; ARCH=x64 ;;
    aarch64-pc-windows-*)        OS=windows; ARCH=arm64 ;;
    *)                           OS="$TARGET"; ARCH=unknown ;;
esac
DEB_ARCH=""
case "$ARCH" in
    x64)   DEB_ARCH=amd64 ;;
    arm64) DEB_ARCH=arm64 ;;
esac

echo "== package $VER pour $TARGET ($OS-$ARCH) =="

# -- 1) Build release ----------------------------------------------------
if [ "$SKIP_BUILD" -eq 0 ]; then
    echo "== cargo build --release --target $TARGET =="
    cargo build --release -p onionbit-daemon -p onionbit-cli \
        --manifest-path "$ROOT/Cargo.toml" --target "$TARGET"
fi

SUFFIX=""; case "$TARGET" in *windows*) SUFFIX=".exe" ;; esac
SRC_DIR="$ROOT/target/$TARGET/release"
OUT="$ROOT/dist/$TARGET"
mkdir -p "$OUT"
for bin in onionbit-daemon onionbit-cli; do
    cp -f "$SRC_DIR/$bin$SUFFIX" "$OUT/"
done

# -- 2) Interface web (optionnelle mais incluse si presente) -------------
if [ -f "$WEB_DIR/index.html" ]; then
    rm -rf "$OUT/web"
    cp -r "$WEB_DIR" "$OUT/web"
else
    echo "  note : $WEB_DIR/index.html absent — tarball sans interface web"
fi

cp -f "$ROOT/LICENSE" "$OUT/"

# -- 3) Manifest ----------------------------------------------------------
cat > "$OUT/build-manifest.json" <<JSON
{
  "target": "$TARGET",
  "version": "$VER",
  "commit": "$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')",
  "rustc": "$(rustc -V)",
  "built_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
JSON

# -- 4) Tarball -----------------------------------------------------------
TARBALL="$ROOT/dist/OnionBit-$VER-$OS-$ARCH.tar.gz"
BUNDLE_DIR="$ROOT/dist/OnionBit-$VER-$OS-$ARCH"
rm -rf "$BUNDLE_DIR" "$TARBALL"
mkdir -p "$BUNDLE_DIR"
cp -r "$OUT"/. "$BUNDLE_DIR/"
tar -C "$ROOT/dist" -czf "$TARBALL" "$(basename "$BUNDLE_DIR")"
echo "  tar.gz : $TARBALL"

# -- 5) Paquet .deb (Linux + dpkg-deb) ------------------------------------
if [ "$OS" = linux ] && [ -n "$DEB_ARCH" ] && command -v dpkg-deb >/dev/null; then
    PKG="$ROOT/dist/deb/onionbit_${VER}_${DEB_ARCH}"
    rm -rf "$PKG"
    mkdir -p "$PKG/DEBIAN" "$PKG/opt/onionbit" "$PKG/usr/bin" \
             "$PKG/usr/lib/systemd/user" "$PKG/usr/share/doc/onionbit"

    cp "$SRC_DIR/onionbit-daemon" "$SRC_DIR/onionbit-cli" "$PKG/opt/onionbit/"
    [ -d "$OUT/web" ] && cp -r "$OUT/web" "$PKG/opt/onionbit/web"
    cp "$ROOT/LICENSE" "$PKG/usr/share/doc/onionbit/copyright"

    # Liens /usr/bin : le daemon resout <exe>/web sur le vrai chemin
    # (/proc/self/exe suit les liens) -> /opt/onionbit/web trouve.
    ln -s ../../opt/onionbit/onionbit-daemon "$PKG/usr/bin/onionbit-daemon"
    ln -s ../../opt/onionbit/onionbit-cli    "$PKG/usr/bin/onionbit-cli"

    cat > "$PKG/DEBIAN/control" <<EOF
Package: onionbit
Version: $VER
Architecture: $DEB_ARCH
Maintainer: Laurent Geynet <laurent.geynet@gmail.com>
Section: net
Priority: optional
Homepage: https://github.com/laurentgeynet-ux/OnionBit
Depends: ca-certificates
Description: Anonymous BitTorrent daemon (native Rust port of Tribler)
 OnionBit provides anonymous BitTorrent downloads over multi-hop onion
 circuits (IPv8 overlay + TunnelCommunity protocol), exposed through a
 local REST API on 127.0.0.1:8085 with an embedded web UI.
 .
 The daemon starts on demand; a systemd user unit is included:
   systemctl --user enable --now onionbit-daemon
EOF

    cat > "$PKG/usr/lib/systemd/user/onionbit-daemon.service" <<EOF
[Unit]
Description=OnionBit daemon (anonymous BitTorrent)
After=network-online.target

[Service]
ExecStart=/opt/onionbit/onionbit-daemon
Restart=on-failure

[Install]
WantedBy=default.target
EOF

    DEB="$ROOT/dist/onionbit_${VER}_${DEB_ARCH}.deb"
    rm -f "$DEB"
    dpkg-deb --build "$PKG" "$DEB"
    echo "  .deb   : $DEB"
fi

echo "Build OK -> dist/"
ls -lh "$ROOT/dist/"*.tar.gz "$ROOT/dist/"*.deb 2>/dev/null || true
