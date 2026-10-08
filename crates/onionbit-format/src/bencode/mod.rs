// This file is part of OnionBit.
// Copyright (C) 2026 Laurent Geynet <laurent.geynet@gmail.com>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Encodage bencode (BEP 3) : [`value::BValue`], [`parser`],
//! [`encoder`].

pub mod encoder;
pub mod parser;
pub mod value;

pub use encoder::encode;
pub use parser::decode;
pub use value::BValue;
