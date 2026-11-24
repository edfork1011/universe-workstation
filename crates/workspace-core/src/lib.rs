// SPDX-License-Identifier: MIT
//! Core protocol, cryptography, discovery, persistence, clipboard codec, and streaming mTLS
//! shared between Linux (`me-edfork-workspace`) and Android (`workspaced`).

pub mod checksum;
pub mod clipboard_model;
pub mod crypto;
pub mod discovery;
pub mod error;
pub mod limits;
pub mod mtls_http;
pub mod protocol;
pub mod store;
pub mod transfer;
pub mod version;
