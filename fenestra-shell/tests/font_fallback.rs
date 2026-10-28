//! Script fallback through system fonts: CJK text shapes through a real
//! family under `Fonts::with_system`, instead of Inter's missing-glyph
//! boxes. Pixel-proven, macOS-gated (the CI reference platform ships
//! PingFang; other runners' font inventories vary).

#![cfg(target_os = "macos")]

use fenestra_core::{App, Element, FamilyRole, Fonts, Theme, col, text};
use fenestra_shell::{Harness, render_element_with, with_fonts};

fn cjk_view() -> Element<()> {
    col()
        .p(12.0)
        .children([text("evolution 进化论 シンカロン 진화").size_px(22.0)])
}

#[test]
fn system_fonts_provide_cjk_fallback() {
    let theme = Theme::light();
    let mut embedded = Fonts::embedded();
    let a = render_element_with(cjk_view(), &theme, (460, 60), &mut embedded);
    let mut system = Fonts::with_system();
    let b = render_element_with(cjk_view(), &theme, (460, 60), &mut system);

    let differing = a
        .pixels()
        .zip(b.pixels())
        .filter(|(pa, pb)| pa != pb)
        .count();
    assert!(
        differing > 500,
        "system fallback should change the CJK glyphs ({differing} pixels differ)"
    );

    // And the system render must actually contain ink (not a blank line).
    let ink = b
        .pixels()
        .filter(|p| p.0[0] < 200 || p.0[1] < 200 || p.0[2] < 200)
        .count();
    assert!(
        ink > 300,
        "CJK text should render visible glyphs, got {ink}"
    );
}

struct Cjk;

impl App for Cjk {
    type Msg = ();
    fn update(&mut self, _msg: ()) {}
    fn view(&self) -> Element<()> {
        cjk_view()
    }
}

#[test]
fn a_harness_with_system_fonts_renders_cjk() {
    let theme = Theme::light();
    let mut h = Harness::new(Cjk, theme.clone(), (460, 60));
    let embedded = h.render();
    h.set_system_fonts(true);
    let system = h.render();
    let expected = render_element_with(cjk_view(), &theme, (460, 60), &mut Fonts::with_system());

    let differing = |a: &image::RgbaImage, b: &image::RgbaImage| {
        a.pixels()
            .zip(b.pixels())
            .filter(|(pa, pb)| pa != pb)
            .count()
    };
    assert!(
        differing(&embedded, &system) > 500,
        "system fonts should replace the missing-glyph boxes"
    );
    assert_eq!(
        differing(&system, &expected),
        0,
        "the harness draws what system-font headless rendering draws"
    );

    h.set_system_fonts(false);
    assert_eq!(
        differing(&h.render(), &embedded),
        0,
        "turning it off goes back to the embedded fonts"
    );
}

struct DisplayLine;

fn display_view() -> Element<()> {
    col().p(12.0).children([text("Display 微信")
        .size_px(22.0)
        .family(FamilyRole::Display)])
}

impl App for DisplayLine {
    type Msg = ();
    fn update(&mut self, _msg: ()) {}
    fn view(&self) -> Element<()> {
        display_view()
    }
}

/// A font registered through `with_fonts` applies with system fonts on as
/// well, even when the system set already exists.
#[test]
fn a_registered_font_applies_with_system_fonts() {
    let Ok(songti) = std::fs::read("/System/Library/Fonts/Supplemental/Songti.ttc") else {
        eprintln!("skipped: no Songti");
        return;
    };
    let theme = Theme::light();
    let mut h = Harness::new(DisplayLine, theme.clone(), (300, 60));
    h.set_system_fonts(true);
    let before = h.render();
    assert!(with_fonts(
        |f| f.register(FamilyRole::Display, songti.clone())
    ));
    h.rebuild();
    let after = h.render();

    let mut fonts = Fonts::with_system();
    fonts.register(FamilyRole::Display, songti);
    let expected = render_element_with(display_view(), &theme, (300, 60), &mut fonts);
    // Two vello renders of the same frame can differ by a few anti-aliased
    // pixels from run to run; a channel tolerance keeps this about faces.
    let differing = |a: &image::RgbaImage, b: &image::RgbaImage| {
        a.pixels()
            .zip(b.pixels())
            .filter(|(pa, pb)| pa.0.iter().zip(pb.0).any(|(x, y)| x.abs_diff(y) > 24))
            .count()
    };
    assert!(
        differing(&before, &after) > 300,
        "the registered face draws"
    );
    assert!(
        differing(&after, &expected) < 20,
        "as system-font rendering with it registered"
    );
}
