//! The OS clipboard (arboard), injected into `FrameState` by the windowed
//! runner. Headless rendering keeps core's in-memory clipboard.
//!
//! Android has no arboard backend: fall back to the in-memory clipboard so
//! in-app copy/paste still works (system-clipboard bridge is a no-op).

#[cfg(target_os = "android")]
use fenestra_core::MemoryClipboard;
use fenestra_core::{ClipImage, Clipboard};

#[cfg(target_os = "android")]
#[derive(Default)]
pub struct OsClipboard(MemoryClipboard);

#[cfg(target_os = "android")]
impl Clipboard for OsClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.get()
    }

    fn set(&mut self, text: String) {
        self.0.set(text)
    }

    // No pictures: the fallback here is core's in-memory *text* clipboard,
    // and there is no Android arboard backend to hand one to.
}

#[cfg(not(target_os = "android"))]
/// Lazy arboard wrapper; failures (no display server) degrade to a no-op.
#[derive(Default)]
pub struct OsClipboard {
    inner: Option<arboard::Clipboard>,
}

#[cfg(not(target_os = "android"))]
impl OsClipboard {
    fn ensure(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }
        self.inner.as_mut()
    }
}

#[cfg(not(target_os = "android"))]
impl Clipboard for OsClipboard {
    fn get(&mut self) -> Option<String> {
        self.ensure().and_then(|c| c.get_text().ok())
    }

    fn set(&mut self, text: String) {
        if let Some(c) = self.ensure() {
            let _ = c.set_text(text);
        }
    }

    fn set_image(&mut self, image: &ClipImage) -> bool {
        // Borrowed, not copied: arboard takes a `Cow` and the caller already
        // owns these bytes, which for a screen grab is megabytes of them.
        self.ensure().is_some_and(|c| {
            c.set_image(arboard::ImageData {
                width: image.width,
                height: image.height,
                bytes: std::borrow::Cow::Borrowed(&image.rgba),
            })
            .is_ok()
        })
    }
}
