//! Language plugins: WebAssembly components loaded at run time (U5, U7).
//!
//! A plugin implements the `language-plugin` world of `wit-v2/extractor.wit`
//! (`ra:plugin@0.2.0`) or of the frozen `wit/extractor.wit` (`0.1.0`); the
//! host loads either. This module owns everything on the host side of that
//! boundary:
//!
//! - [`host`] — the sandbox: a WASI context that grants nothing (no files, no
//!   network, no environment, a frozen clock, deterministic randomness), fuel
//!   and memory bounds, and the calls across.
//! - [`manifest`] — what a plugin says about itself, checked before it is
//!   trusted with a single file.
//!
//! The graph never crosses the boundary. A plugin returns operations in wire
//! form (kinds as strings); the extraction layer turns them into graph changes
//! and does the name resolution itself (implementation note D1).

pub mod bundled;
pub mod host;
pub mod manifest;
pub mod set;

pub use host::{Plugin, PluginError, Session};
pub use manifest::PluginManifest;
pub use set::{Origin, PluginSet, PluginSetError, PluginSpec};
