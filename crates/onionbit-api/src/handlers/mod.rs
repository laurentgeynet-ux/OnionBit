// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Handlers des endpoints.

pub mod asyncio;
pub mod connections;
pub mod createtorrent;
pub mod dht;
pub mod downloads;
pub mod downloads_extra;
pub mod events;
pub mod files;
pub mod identity;
pub mod ipv8;
pub mod libtorrent;
pub mod logging;
pub mod messaging;
pub mod metadata;
pub mod pairing;
pub mod rss;
pub mod search;
pub mod settings;
pub mod shutdown;
pub mod statistics;
pub mod stealth;
pub mod torrentinfo;
pub mod versioning;
