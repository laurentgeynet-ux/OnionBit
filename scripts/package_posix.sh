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
#   ./scripts/package_posix.sh --dillo-source ../dillorust
#   ./scripts/package_posix.sh --dillo-bin /chemin/onionbit-browser
#   ./scripts/package_posix.sh --no-dillo      # forcer l'absence
#
# Navigateur .onion (ADR-0030) : comme build_dist.ps1, un checkout
# DilloRust en ../dillorust est detecte et build (`cargo build
# -p dillo`) ; le binaire est stage en <bundle>/bin/onionbit-browser —
# chemin resolu par l'auto-detection `<exe>/bin/` du daemon. Absence =
# `browser_not_configured` cote UI, jamais un echec du packaging.
#
# Layout du .deb : /opt/onionbit/{daemon,cli,web/} + liens /usr/bin —
# l'auto-detection `<exe>/web` du daemon fonctionne car current_exe
# resout le lien symbolique vers le vrai chemin. Unite systemd
# utilisateur `onionbit-daemon.service` + entrees de menu `.desktop`
# (OnionBit web UI / console daemon) + icone hicolor. L'etat vit sous
# `$XDG_DATA_HOME/onionbit` (~/.local/share/onionbit) : /opt/onionbit
# n'est pas inscriptible, `resolve_state_dir` y replie le state_dir.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET=""
WEB_DIR=""
SKIP_BUILD=0
DILLO_SOURCE=""
DILLO_BIN=""
NO_DILLO=0

while [ $# -gt 0 ]; do
    case "$1" in
        --target)    TARGET="$2"; shift 2 ;;
        --web-dir)   WEB_DIR="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=1; shift ;;
        --dillo-source) DILLO_SOURCE="$2"; shift 2 ;;
        --dillo-bin)   DILLO_BIN="$2"; shift 2 ;;
        --no-dillo)    NO_DILLO=1; shift ;;
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

# -- 1b) Navigateur .onion (DilloRust) -------------------------------------
# Miroir de build_dist.ps1 : checkout explicite (--dillo-source),
# binaire pret (--dillo-bin), defaut ../dillorust, --no-dillo force
# l'absence. Stage en <bundle>/bin/onionbit-browser — auto-detection
# `<exe>/bin/` du daemon (cf. bundled_browser_binary).
DILLO_SRC=""
if [ "$NO_DILLO" -eq 0 ]; then
    if [ -n "$DILLO_BIN" ]; then
        DILLO_SRC="$DILLO_BIN"
    else
        [ -z "$DILLO_SOURCE" ] && DILLO_SOURCE="$(cd "$ROOT/.." && pwd)/dillorust"
        if [ -f "$DILLO_SOURCE/Cargo.toml" ]; then
            echo "== cargo build dillo ($TARGET, $DILLO_SOURCE) =="
            cargo build --release -p dillo \
                --manifest-path "$DILLO_SOURCE/Cargo.toml" --target "$TARGET"
            DILLO_SRC="$DILLO_SOURCE/target/$TARGET/release/onionbit-browser$SUFFIX"
        else
            echo "  note : dillorust absent ($DILLO_SOURCE) — bundle sans navigateur"
        fi
    fi
fi
if [ -n "$DILLO_SRC" ]; then
    [ -f "$DILLO_SRC" ] || { echo "onionbit-browser introuvable : $DILLO_SRC" >&2; exit 1; }
    mkdir -p "$OUT/bin"
    cp -f "$DILLO_SRC" "$OUT/bin/onionbit-browser$SUFFIX"
    echo "  navigateur embarque : bin/onionbit-browser$SUFFIX"
fi

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
             "$PKG/usr/lib/systemd/user" "$PKG/usr/share/doc/onionbit" \
             "$PKG/usr/share/applications" \
             "$PKG/usr/share/icons/hicolor/192x192/apps" \
             "$PKG/usr/share/icons/hicolor/512x512/apps"

    cp "$SRC_DIR/onionbit-daemon" "$SRC_DIR/onionbit-cli" "$PKG/opt/onionbit/"
    [ -d "$OUT/web" ] && cp -r "$OUT/web" "$PKG/opt/onionbit/web"
    # Navigateur .onion : /opt/onionbit/bin/ — le lien /usr/bin est
    # resolu vers /opt/onionbit/ -> <exe>/bin/onionbit-browser trouve.
    if [ -f "$OUT/bin/onionbit-browser$SUFFIX" ]; then
        mkdir -p "$PKG/opt/onionbit/bin"
        cp "$OUT/bin/onionbit-browser$SUFFIX" "$PKG/opt/onionbit/bin/onionbit-browser"
    fi
    cp "$ROOT/LICENSE" "$PKG/usr/share/doc/onionbit/copyright"

    # Icone hicolor : le PNG brande du build web Flutter.
    for size in 192 512; do
        icon="$ROOT/app/web/icons/Icon-$size.png"
        [ -f "$icon" ] && cp "$icon" \
            "$PKG/usr/share/icons/hicolor/${size}x${size}/apps/onionbit.png"
    done

    # Entrees de menu (equivalent des .lnk racine du bundle Windows) :
    #   « OnionBit »        → daemon --open-webui (idempotent : un
    #                         second lancement rouvre juste le
    #                         navigateur sur le port reel)
    #   « OnionBit Daemon » → console de logs du daemon
    cat > "$PKG/usr/share/applications/onionbit.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.5
Name=OnionBit
GenericName=Anonymous BitTorrent client
Comment=Start the OnionBit daemon and open the web UI
Exec=/opt/onionbit/onionbit-daemon --open-webui
Icon=onionbit
Terminal=false
Categories=Network;FileTransfer;P2P;
Keywords=bittorrent;anonymous;onion;tribler;
StartupNotify=true
EOF

    cat > "$PKG/usr/share/applications/onionbit-daemon.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.5
Name=OnionBit Daemon
GenericName=Anonymous BitTorrent daemon
Comment=Run the OnionBit daemon in a console (logs + tray icon)
Exec=/opt/onionbit/onionbit-daemon
Icon=onionbit
Terminal=true
Categories=Network;FileTransfer;P2P;
Keywords=bittorrent;anonymous;daemon;
EOF

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
Depends: ca-certificates, xdg-utils
Description: Anonymous BitTorrent daemon (native Rust port of Tribler)
 OnionBit provides anonymous BitTorrent downloads over multi-hop onion
 circuits (IPv8 overlay + TunnelCommunity protocol), exposed through a
 local REST API on 127.0.0.1:8085 with an embedded web UI.
 .
 Desktop entries « OnionBit » (web UI) and « OnionBit Daemon »
 (console) are installed; a systemd user unit is included:
   systemctl --user enable --now onionbit-daemon
EOF

    # postinst : rafraichir les caches menu/icones si les outils sont
    # la (pas de dependance dure — les DE re-scannent aussi seuls).
    cat > "$PKG/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
command -v update-desktop-database >/dev/null 2>&1 && \
    update-desktop-database -q /usr/share/applications || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && \
    gtk-update-icon-cache -q /usr/share/icons/hicolor || true
exit 0
EOF
    chmod 755 "$PKG/DEBIAN/postinst"

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
