//! A masked field (a password): the bullets on screen, and the value on
//! neither the screen nor the accessibility tree.

use std::path::PathBuf;

use fenestra_core::{Element, SP6, Theme, col};
use fenestra_kit::{field, text_input};
use fenestra_shell::{render_element, testing::assert_png_snapshot};

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

const SIZE: (u32, u32) = (360, 200);

fn view(theme: &Theme) -> Element<()> {
    let password: Element<()> = field("Password")
        .child(text_input("hunter2").masked(true))
        .into();
    let username: Element<()> = field("Username").child(text_input("ada")).into();
    col()
        .p(SP6)
        .gap(SP6)
        .bg(theme.bg)
        .children([username, password])
}

#[test]
fn masked_light() {
    let theme = Theme::light();
    let image = render_element(view(&theme), &theme, SIZE);
    assert_png_snapshot(snapshot_dir(), "masked_light", &image);
}

#[test]
fn masked_dark() {
    let theme = Theme::dark();
    let image = render_element(view(&theme), &theme, SIZE);
    assert_png_snapshot(snapshot_dir(), "masked_dark", &image);
}

/// The value is not on screen. Two secrets of the same length render to the
/// same picture — the bullets are all the renderer has.
#[test]
fn the_value_is_not_on_screen() {
    let theme = Theme::light();
    let field_of = |value: &str| {
        let view: Element<()> = text_input(value).masked(true).into();
        render_element(view, &theme, (240, 60)).into_raw()
    };
    assert_eq!(
        field_of("hunter2"),
        field_of("abcdefg"),
        "two secrets of the same length must render to the same pixels"
    );
}
