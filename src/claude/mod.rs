//! Thin client for the Claude Messages API (`POST /v1/messages`) over raw HTTP.
//!
//! There is no official Rust SDK, so this module speaks the wire format directly:
//! * [`types`] — serde models for requests, content blocks, responses and usage.
//! * [`client`] — [`client::Client`]: streaming SSE transport with retries, plus
//!   model-aware request shaping (adaptive thinking vs. thinking budgets, effort, fallbacks).
//! * [`Usage`] accounting so every pipeline step can report tokens and estimated cost.

pub mod client;
pub mod types;

pub use client::{Client, ClientError};
pub use types::*;
