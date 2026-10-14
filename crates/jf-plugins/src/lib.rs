//! Python plugin host.
//!
//! Plugins are ordinary Python modules run in-process via pyo3. They receive a
//! narrow capability handle (see [`api`]) and never touch the server directly.
//!
//! See `host.rs` for the threading contract, which is the part that matters:
//! plugin calls hold the GIL and must never run on a tokio worker thread.

pub mod api;
pub mod error;
pub mod host;

pub use api::{
    API_VERSION, Capability, HookResult, Manifest, MetadataImage, MetadataPerson, MetadataQuery,
    MetadataResult, PluginEvent,
};
pub use error::PluginError;
pub use host::{ItemLookup, LoadedPlugin, LoadReport, PluginConfigHandle, PluginHost};