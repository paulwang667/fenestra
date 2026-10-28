//! The system pasteboard on iOS: what is set comes back, and `has_text`
//! sees it. Run inside the simulator (`xcrun simctl spawn`).

#![cfg(target_os = "ios")]

use fenestra_core::Clipboard;
use fenestra_shell::OsClipboard;

#[test]
fn ios_pasteboard_round_trips() {
    let mut clipboard = OsClipboard::default();
    clipboard.set("fenestra-剪贴板".to_owned());
    assert!(clipboard.has_text());
    assert_eq!(clipboard.get().as_deref(), Some("fenestra-剪贴板"));
}
