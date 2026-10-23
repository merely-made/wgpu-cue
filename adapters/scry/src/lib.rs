// SPDX-License-Identifier: MPL-2.0
//! Concrete, non-blocking system-WebView adapters. Only the macOS WKWebView
//! bridge is implemented in this first proof. Engines keep platform policy.
#![forbid(unsafe_code)]
#[cfg(target_os = "macos")]
mod wk;
#[cfg(target_os = "macos")]
pub use wk::*;
