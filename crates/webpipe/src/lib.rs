//! Public facade crate for `webpipe`.
//!
//! This crate intentionally contains no IO or provider-specific logic.
//! It re-exports the backend-agnostic types/traits from `webpipe-core`.

pub use webpipe_core::*;

