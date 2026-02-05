//! `webpipe` crate (library surface).
//!
//! The primary entrypoint for end users is the `webpipe` binary (CLI + MCP stdio).
//! This library module exists to support embedding and to provide a stable way to
//! reuse core types without depending on internal crate layout.

pub use webpipe_core as core;
