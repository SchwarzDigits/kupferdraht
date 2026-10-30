// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Recovery data (Step 112, ADR-0035): the account's seed sealed with a
//! PIN, so a browser that lost its storage gets the account back. The key
//! that seals it comes from the PIN service's OPRF evaluation of the
//! blinded PIN (every guess needs the service, which limits them) and
//! Argon2id of the PIN (a leaked service key still leaves Argon2id per
//! guess): `HKDF-SHA256(o ‖ a, "kupferdraht/v1/recovery-data")`, then
//! AES-256-GCM over the seed with the header as associated data. The
//! embedder only ever holds the opaque bytes. Compiled for the browser and
//! for tests.

pub(crate) mod account;
pub(crate) mod data;
pub(crate) mod flow;
pub(crate) mod oprf;
pub(crate) mod pin;
pub(crate) mod pin_service;
pub(crate) mod receipt;
pub(crate) mod seal;
pub(crate) mod verifier;
