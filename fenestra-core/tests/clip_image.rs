//! A clipboard that carries pictures, and one that says it cannot.
//!
//! The answer matters: an app that reported "copied" over a clipboard which
//! took nothing would have somebody paste an older thing into a document and
//! not notice.

use fenestra_core::{ClipImage, Clipboard, MemoryClipboard};

/// A text-only clipboard, which is what the default is for.
#[derive(Default)]
struct TextOnly(Option<String>);

impl Clipboard for TextOnly {
    fn get(&mut self) -> Option<String> {
        self.0.clone()
    }
    fn set(&mut self, text: String) {
        self.0 = Some(text);
    }
}

fn swatch() -> ClipImage {
    ClipImage {
        width: 2,
        height: 1,
        rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
    }
}

#[test]
fn a_clipboard_that_takes_pictures_says_so_and_keeps_them() {
    let mut board = MemoryClipboard::default();
    assert_eq!(
        board.image(),
        None,
        "something was on it before anything was put"
    );
    assert!(
        board.set_image(&swatch()),
        "it took the picture and denied it"
    );
    assert_eq!(
        board.image(),
        Some(&swatch()),
        "it said yes and kept nothing"
    );

    // Text and pictures do not evict each other here, which is what lets a
    // test check both halves of one action.
    board.set("hello".to_owned());
    assert_eq!(board.get().as_deref(), Some("hello"));
    assert_eq!(board.image(), Some(&swatch()));
}

#[test]
fn a_clipboard_that_does_not_take_pictures_declines() {
    let mut board = TextOnly::default();
    assert!(
        !board.set_image(&swatch()),
        "a text-only clipboard claimed it took a picture"
    );
    assert_eq!(
        board.get_image(),
        None,
        "a text-only clipboard produced a picture from somewhere"
    );
}
