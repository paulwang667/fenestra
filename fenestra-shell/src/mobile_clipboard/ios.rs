//! `UIPasteboard.generalPasteboard`.

use objc2_foundation_uikit::NSString;
use objc2_ui_kit::UIPasteboard;

/// The system pasteboard. Reading it shows iOS's "pasted from" banner, so
/// `has_text` asks `hasStrings`, which does not.
#[derive(Default)]
pub(crate) struct IosPasteboard;

impl fenestra_core::Clipboard for IosPasteboard {
    fn get(&mut self) -> Option<String> {
        // SAFETY: plain UIKit property reads.
        unsafe { UIPasteboard::generalPasteboard().string() }.map(|s| s.to_string())
    }

    fn set(&mut self, text: String) {
        let text = NSString::from_str(&text);
        // SAFETY: a plain UIKit property write.
        unsafe { UIPasteboard::generalPasteboard().setString(Some(&text)) };
    }

    fn has_text(&mut self) -> bool {
        // SAFETY: a plain UIKit property read.
        unsafe { UIPasteboard::generalPasteboard().hasStrings() }
    }
}
