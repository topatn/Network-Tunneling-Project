//! Library root for `gtt-desktop` (`T-080`, `T-081`, `T-100`, `T-101`, `T-102`).
//!
//! Exposes the per-user installer, WebView2 runtime detection, signed update staging and
//! rollback engine, and uninstaller (`packaging`) so integration tests and the CLI entrypoint
//! share identical, strictly linted logic.

pub mod packaging;
