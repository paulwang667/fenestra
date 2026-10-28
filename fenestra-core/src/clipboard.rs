//! Clipboard abstraction: the core stays windowless, so the OS clipboard
//! (arboard) is injected by the shell; headless rendering uses the
//! deterministic in-memory default.

/// A picture on the clipboard, as straight-alpha RGBA8 rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipImage {
    pub width: usize,
    pub height: usize,
    /// Row-major RGBA8, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Read/write access to a clipboard.
pub trait Clipboard {
    /// Current clipboard text, if any.
    fn get(&mut self) -> Option<String>;
    /// Replaces the clipboard text.
    fn set(&mut self, text: String);

    /// Whether there is text to paste, without reading it. iOS shows a
    /// "pasted from" banner whenever the pasteboard is read, so deciding
    /// whether to offer Paste must not read; system clipboards answer
    /// from the platform's own query.
    fn has_text(&mut self) -> bool {
        self.get().is_some()
    }

    /// The picture on the clipboard, if there is one.
    ///
    /// The counterpart of [`Clipboard::set_image`], and `None` by default for
    /// the same reason: not every clipboard carries pictures.
    fn get_image(&mut self) -> Option<ClipImage> {
        None
    }

    /// Puts a picture on the clipboard. Returns whether it got there.
    ///
    /// **Answered rather than assumed.** Not every clipboard carries pictures
    /// — Android's fallback here is an in-memory text one — and an app that
    /// told somebody "copied" over a clipboard that took nothing would have
    /// them paste an old thing into a document and not notice.
    ///
    /// The default declines, so a clipboard that only does text says so by
    /// saying nothing.
    fn set_image(&mut self, image: &ClipImage) -> bool {
        let _ = image;
        false
    }
}

/// The default in-memory clipboard (headless tests use this).
#[derive(Default)]
pub struct MemoryClipboard {
    text: Option<String>,
    image: Option<ClipImage>,
}

impl MemoryClipboard {
    /// The picture last put here, for tests that need to see what an app
    /// copied rather than trust that it tried.
    #[must_use]
    pub const fn image(&self) -> Option<&ClipImage> {
        self.image.as_ref()
    }
}

impl Clipboard for MemoryClipboard {
    fn get(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn set(&mut self, text: String) {
        self.text = Some(text);
    }

    fn get_image(&mut self) -> Option<ClipImage> {
        self.image.clone()
    }

    fn set_image(&mut self, image: &ClipImage) -> bool {
        self.image = Some(image.clone());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Clipboard, MemoryClipboard};

    /// `has_text` answers without reading: empty until text is set.
    #[test]
    fn has_text_follows_what_was_set() {
        let mut clipboard = MemoryClipboard::default();
        assert!(!clipboard.has_text());
        clipboard.set("x".to_owned());
        assert!(clipboard.has_text());
    }
}
