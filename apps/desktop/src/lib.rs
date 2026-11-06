//! Library root for `gtt-desktop` (`T-070`, `T-073`, `T-074`, `T-080`, `T-081`, `T-100`, `T-101`, `T-102`).
//!
//! Exposes the per-user installer, WebView2 runtime detection, signed update staging and
//! rollback engine, uninstaller (`packaging`), encrypted OS credential vault (`credentials`),
//! and the in-process Engine <-> UI bridge coordinator (`engine_bridge`) so integration
//! tests and the native desktop executable share identical, strictly linted logic.

pub mod credentials;
pub mod engine_bridge;
pub mod http_server;
pub mod packaging;
