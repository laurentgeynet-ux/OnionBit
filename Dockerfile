# syntax=docker/dockerfile:1
# This file is part of OnionBit.
# Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Image headless du daemon OnionBit (ADR-0024, Phase 16).
#
#   build daemon-seul : docker build --target final       -t onionbit:dev .
#   build + web UI    : docker build --target final-webui -t onionbit:dev-webui .
#   run (recommande)  : docker run --network host -v ./data:/data onionbit:dev-webui
#
# L'API de controle bind 127.0.0.1 en dur (jamais affaiblie) :
# `--network host` la rend joignable depuis l'hote ; en bridge,
# utiliser le forwarder de docker-compose.yml (network_mode: service:).

# ---------- couche deps (cachee) : manifests + vendor + stubs --------
FROM rust:bookworm AS deps
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
# Les [patch.crates-io] par path exigent vendor/ complet des cargo
# fetch — copie avant les manifests.
COPY vendor ./vendor
# COPY --parents (dockerfile >= 1.7) conserve crates/*/Cargo.toml.
COPY --parents crates/*/Cargo.toml ./
# Stubs lib/bin : compiler les deps tierces (dont bitdaemon-* en git
# epingle — git est present dans rust:bookworm) sans les sources
# metier. Un main.rs ET un lib.rs par crate couvrent les cibles
# explicites ([[bin]] -> src/main.rs) et autodetectees. Les build.rs
# des crates (daemon, launcher — cfg(windows)) ne sont PAS copies :
# non declares par `build =`, leur absence est ignoree. Attention si
# un futur manifest ajoute `build = "x.rs"` ou `[[bin]] path =
# "src/bin/…"` : copier le fichier ou le stuber ici.
RUN set -eu; \
    for d in crates/*/; do \
        mkdir -p "${d}src"; \
        printf 'fn main() {}\n' > "${d}src/main.rs"; \
        printf 'pub fn _stub() {}\n' > "${d}src/lib.rs"; \
    done; \
    cargo build --release --locked -p onionbit-daemon -p onionbit-cli

# ---------- build reel : seuls les crates metier recompilent ---------
FROM deps AS build
RUN rm -rf crates
COPY crates ./crates
# COPY preserve les mtimes du contexte (BuildKit) : les sources
# reelles sont "plus vieilles" que les stubs crees dans `deps` —
# cargo les considererait inchangees et reutiliserait les rlibs
# stubs (erreurs `unresolved imports`). Touch force le re-build.
RUN find crates -type f -exec touch {} +
RUN cargo build --release --locked -p onionbit-daemon -p onionbit-cli

# ---------- web UI Flutter (variante final-webui) --------------------
FROM ghcr.io/cirruslabs/flutter:stable AS webui
WORKDIR /app
COPY app/pubspec.yaml app/pubspec.lock ./
RUN flutter pub get
COPY app ./
RUN flutter config --enable-web && flutter build web --release

# ---------- runtime : debian slim, user non-root, un volume /data ----
FROM debian:bookworm-slim AS final
# ca-certificates : rustls-native-certs -> trackers HTTPS / webseeds.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --shell /usr/sbin/nologin onionbit \
    && mkdir -p /data \
    && chown onionbit:onionbit /data
COPY --from=build /src/target/release/onionbit-daemon /usr/local/bin/
COPY --from=build /src/target/release/onionbit-cli /usr/local/bin/
USER onionbit
# Tout l'etat vit sous /data (bind mount : chown -R 10001:10001 ./data
# cote hote). --state-dir /data/state -> data/ voisine /data/data
# (convention ADR-0018).
VOLUME /data
# onionbit-cli (docker exec / HEALTHCHECK) lit configuration.json via
# cette env — pas besoin de repasser --state-dir.
ENV ONIONBIT_STATE_DIR=/data/state
# Documentation seule : l'API ne bind que 127.0.0.1 (injoignable via
# -p). 45000 = libtorrent/port (tcp pairs + udp uTP/DHT).
EXPOSE 8085/tcp 45000/tcp 45000/udp
STOPSIGNAL SIGTERM
# Healthcheck via l'API reelle (cle lue dans configuration.json) —
# curl / renverrait 404 quand la web UI est desactivee (fallback
# statique non monte).
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["onionbit-cli", "status"]
ENTRYPOINT ["onionbit-daemon", "--state-dir", "/data/state", "--no-tray"]
CMD []

# ---------- variante web UI : build `app/build/web` embarque ---------
FROM final AS final-webui
COPY --from=webui /app/build/web /opt/onionbit/web
# L'auto-detection ne couvre que <exe>/web et <state_dir>/web :
# --web-ui-dir explicite obligatoire.
ENTRYPOINT ["onionbit-daemon", "--state-dir", "/data/state", "--no-tray", "--web-ui-dir", "/opt/onionbit/web"]
