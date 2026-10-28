//! The OS clipboard, injected into `FrameState` by the windowed runner:
//! arboard on desktops, `ClipboardManager` on Android and `UIPasteboard`
//! on iOS (see `mobile_clipboard`). Headless rendering keeps core's
//! in-memory clipboard.

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use fenestra_core::ClipImage;
use fenestra_core::Clipboard;
#[cfg(target_os = "android")]
use fenestra_core::MemoryClipboard;

/// Android: the system clipboard once the runner has the `AndroidApp`
/// ([`OsClipboard::android`]); before that (or without one) core's
/// in-memory text clipboard, so in-app copy and paste still work.
#[cfg(target_os = "android")]
pub struct OsClipboard(AndroidInner);

#[cfg(target_os = "android")]
enum AndroidInner {
    Memory(MemoryClipboard),
    System(crate::mobile_clipboard::AndroidClipboard),
}

#[cfg(target_os = "android")]
impl Default for OsClipboard {
    fn default() -> Self {
        Self(AndroidInner::Memory(MemoryClipboard::default()))
    }
}

#[cfg(target_os = "android")]
impl OsClipboard {
    /// The system clipboard, reached through the app's activity.
    #[must_use]
    pub fn android(app: winit::platform::android::activity::AndroidApp) -> Self {
        Self(AndroidInner::System(
            crate::mobile_clipboard::AndroidClipboard::new(app),
        ))
    }

    fn inner(&mut self) -> &mut dyn Clipboard {
        match &mut self.0 {
            AndroidInner::Memory(c) => c,
            AndroidInner::System(c) => c,
        }
    }
}

#[cfg(target_os = "android")]
impl Clipboard for OsClipboard {
    fn get(&mut self) -> Option<String> {
        self.inner().get()
    }

    fn set(&mut self, text: String) {
        self.inner().set(text);
    }

    fn has_text(&mut self) -> bool {
        self.inner().has_text()
    }

    // Text only: neither clipboard here carries pictures.
}

/// iOS: the general pasteboard. Text only.
#[cfg(target_os = "ios")]
#[derive(Default)]
pub struct OsClipboard(crate::mobile_clipboard::IosPasteboard);

#[cfg(target_os = "ios")]
impl Clipboard for OsClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.get()
    }

    fn set(&mut self, text: String) {
        self.0.set(text);
    }

    fn has_text(&mut self) -> bool {
        self.0.has_text()
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
/// Lazy arboard wrapper; failures (no display server) degrade to a no-op.
#[derive(Default)]
pub struct OsClipboard {
    inner: Option<arboard::Clipboard>,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl OsClipboard {
    fn ensure(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }
        self.inner.as_mut()
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Clipboard for OsClipboard {
    fn get(&mut self) -> Option<String> {
        self.ensure().and_then(|c| c.get_text().ok())
    }

    fn set(&mut self, text: String) {
        if let Some(c) = self.ensure() {
            let _ = c.set_text(text);
        }
    }

    fn get_image(&mut self) -> Option<ClipImage> {
        let got = self.ensure()?.get_image().ok()?;
        Some(ClipImage {
            width: got.width,
            height: got.height,
            rgba: got.bytes.into_owned(),
        })
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
