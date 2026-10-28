//! The system clipboard on Android (`ClipboardManager`, through JNI) and
//! iOS (`UIPasteboard`): arboard has neither. Text only.
//!
//! The workspace's second `unsafe` module (after fenestra-raster's
//! `coretext`): JNI from the raw `JavaVM`/activity pointers
//! android-activity hands out, and objc2 calls into UIKit.
#![allow(unsafe_code)]

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "ios")]
mod ios;

#[cfg(target_os = "android")]
pub(crate) use android::AndroidClipboard;
#[cfg(target_os = "ios")]
pub(crate) use ios::IosPasteboard;
