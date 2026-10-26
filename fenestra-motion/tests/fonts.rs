//! A registered font must reach the workers that render a contact sheet.
//!
//! The font table is per-thread, so registering a font on the calling
//! thread never reaches a rayon worker. Before this, a Chinese project
//! produced a contact sheet full of tofu boxes: the one artifact a human
//! review depends on was unreadable.

use fenestra_core::{Color, FamilyRole, text};
use fenestra_motion::{Clip, Composition, Frames};

/// A CJK font, if this machine has one.
fn cjk_font() -> Option<Vec<u8>> {
    [
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ]
    .iter()
    .find_map(|path| std::fs::read(path).ok())
}

fn sheet() -> Composition {
    Composition::new(320, 180, 30)
        .duration(Frames(12))
        .background(Color::new([0.1, 0.1, 0.12, 1.0]))
        .clip(Clip::new("title", 0..12).element(|| text("前三季度用户增长")))
}

#[test]
fn a_registered_font_changes_the_sheet() {
    let Some(data) = cjk_font() else {
        eprintln!("skipped: no CJK font on this machine");
        return;
    };
    let plain = sheet()
        .contact_sheet(6, 160, &[])
        .expect("sheet without the font");
    let with = sheet()
        .contact_sheet(6, 160, &[(FamilyRole::Sans, data)])
        .expect("sheet with the font");
    assert_eq!(plain.dimensions(), with.dimensions());
    assert_ne!(
        plain.as_raw(),
        with.as_raw(),
        "registering a CJK font must change what the sheet draws"
    );
}
