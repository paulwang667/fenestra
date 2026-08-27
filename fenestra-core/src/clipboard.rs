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

    fn set_image(&mut self, image: &ClipImage) -> bool {
        self.image = Some(image.clone());
        true
    }
}
