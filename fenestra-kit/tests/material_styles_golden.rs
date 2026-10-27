//! One page in each material style: a scaffold (app bar, list, tab bar)
//! over vivid stripes, with an open menu and a glass panel — Liquid Glass's
//! clear bars and dock, Acrylic's grained thick glass and strokes, Tonal's
//! accent-tinted surfaces, Solid's opaque bars.

use std::path::PathBuf;

use fenestra_core::{
    Element, MaterialStyle, Overlay, OverlayMode, SP3, Theme, col, row, stack, text,
};
use fenestra_kit::{app_bar, glass_panel, menu, scaffold, tab_bar, tab_item};
use fenestra_shell::{render_element, testing::assert_png_snapshot};

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

const SIZE: (u32, u32) = (640, 420);

fn page(t: &Theme) -> Element<()> {
    // Vivid diagonal-ish stripes for the glass to frost.
    let stripes = col().w_full().children((0..30_usize).map(|i| {
        row()
            .w_full()
            .h(28.0)
            .px(SP3)
            .items_center()
            .bg(t.accents.step(3 + i % 8))
            .child(text(format!("Row {i}")).color(t.text))
    }));
    let list = col().w_full().scroll_y().child(stripes);
    let open_menu = menu([("Pin", ()), ("Mute", ()), ("Delete", ())]).overlay(Overlay {
        mode: OverlayMode::Open,
        ..Overlay::menu()
    });
    stack().w_full().h_full().children([
        scaffold(list)
            .top(app_bar("Material styles"), 48.0)
            .bottom(
                tab_bar(
                    ["Chats", "Contacts", "Discover", "Me"]
                        .map(|n| tab_item(n, text("•"), n == "Chats", ())),
                ),
                56.0,
            )
            .into(),
        col()
            .absolute()
            .left(360.0)
            .top(80.0)
            .w(220.0)
            .children([glass_panel([text("Glass panel").color(t.text)]), open_menu]),
    ])
}

#[test]
fn material_style_goldens() {
    for (style, name) in [
        (MaterialStyle::Solid, "material_style_solid"),
        (MaterialStyle::LiquidGlass, "material_style_liquid_glass"),
        (MaterialStyle::Acrylic, "material_style_acrylic"),
        (MaterialStyle::Tonal, "material_style_tonal"),
    ] {
        let theme = Theme::dark().with_material_style(style);
        let image = render_element(page(&theme), &theme, SIZE);
        assert_png_snapshot(snapshot_dir(), name, &image);
    }
}
