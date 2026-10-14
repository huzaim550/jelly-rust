//! JavaScript plugin host.
//!
//! Plugins are ordinary npm packages run in a Node subprocess, supervised by
//! [`supervisor::PluginHost`] over newline-delimited JSON. They never touch the
//! server directly: they receive a narrow capability handle from the SDK in
//! `pluginhost/src/sdk.ts` and are confined to what it exposes.
//!
//! See `supervisor` for why the runtime is a subprocess rather than an
//! embedded engine, and `api` for the capability surface.

pub mod api;
pub mod supervisor;

pub use api::{
    API_VERSION, Capability, ImageKind, ItemSummary, LogLevel, LogRecord, Manifest,
    MetadataImage, MetadataPerson, MetadataQuery, MetadataResult, PersonKind, PluginError,
    PluginEvent,
};
pub use supervisor::{
    level_name, read_manifest, FailedPlugin, LoadReport, LoadedPluginInfo, PluginHost,
};
