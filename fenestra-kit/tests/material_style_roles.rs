//! Kit surfaces follow the theme's material style: each widget's surface
//! frosts exactly when its role does in that style, tinted with the role's
//! fill. (Under Solid the modal stays the kit's glass sheet and the toolbar
//! its raised strip, as before material styles.)

use fenestra_core::{
    Element, Fonts, FrameState, MaterialStyle, Overlay, OverlayMode, Semantics, Style, Surface,
    Theme, build_frame, by, col, stack, text,
};
use fenestra_kit::{glass_panel, menu, modal, popover, toolbar};

fn view() -> Element<()> {
    stack().w_full().h_full().children([
        col().w_full().h_full(),
        menu([("Open", ()), ("Close", ())]).id("menu"),
        // Queryable by id: give the plain panels a role.
        popover(text("Hello"))
            .overlay(Overlay {
                mode: OverlayMode::Open,
                ..Overlay::menu()
            })
            .id("popover")
            .semantics(Semantics::Label),
        glass_panel([text("Glass")])
            .id("glass")
            .semantics(Semantics::Label),
        Element::from(toolbar().item(text("Bold")).item(text("Italic"))).id("toolbar"),
        Element::from(modal("Settings").child(text("Body"))).id("modal"),
    ])
}

#[test]
fn kit_surfaces_follow_the_material_style() {
    for style in MaterialStyle::ALL {
        let theme = Theme::dark().with_material_style(style);
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        let frame = build_frame(&view(), &theme, &mut fonts, &mut state, (800.0, 600.0), 1.0);
        for (id, role) in [
            ("menu", Surface::Menu),
            ("popover", Surface::Popover),
            ("glass", Surface::Glass),
            ("toolbar", Surface::Dock),
            ("modal", Surface::Modal),
        ] {
            let resolved = theme.apply_surface(role, Style::default());
            let solid_sheet = style == MaterialStyle::Solid && id == "modal";
            let frosted = solid_sheet || resolved.backdrop_blur.is_some();
            let node = frame.get(&by::id(id)).id;
            let body = frame.glass_body(node);
            assert_eq!(body.is_some(), frosted, "{style:?} {id}");
            if let (Some(body), false) = (body, solid_sheet) {
                let fill = match resolved.fill {
                    Some(fenestra_core::Paint::Solid(c)) => c,
                    other => panic!("{style:?} {id}: {other:?}"),
                };
                assert_eq!(body.tint, Some(fill), "{style:?} {id} tint");
            }
        }
    }
}

/// Reduced transparency is opaque everywhere: no kit surface frosts, the
/// solid style's glass sheet included.
#[test]
fn reduced_transparency_frosts_no_kit_surface() {
    for style in MaterialStyle::ALL {
        let theme = Theme::dark()
            .with_material_style(style)
            .with_reduce_transparency(true);
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        let frame = build_frame(&view(), &theme, &mut fonts, &mut state, (800.0, 600.0), 1.0);
        for id in ["menu", "popover", "glass", "toolbar", "modal"] {
            let node = frame.get(&by::id(id)).id;
            assert!(frame.glass_body(node).is_none(), "{style:?} {id} frosts");
        }
    }
}

/// A select, or (modal, so alone) a command palette.
struct Pickers {
    palette: bool,
}

impl fenestra_core::App for Pickers {
    type Msg = ();
    fn update(&mut self, _msg: ()) {}
    fn view(&self) -> Element<()> {
        if self.palette {
            fenestra_kit::command_palette("", true, [("Open", ()), ("Close", ())])
                .on_close(())
                .into()
        } else {
            fenestra_kit::select(0, ["Rust", "Go", "Zig"])
                .on_change(|_| ())
                .into()
        }
    }
}

/// The select's listbox and the command palette are menus: they frost
/// exactly when the style's menu does.
#[test]
fn select_and_palette_follow_the_material_style() {
    for style in MaterialStyle::ALL {
        let theme = Theme::dark().with_material_style(style);
        let frosted = theme
            .apply_surface(Surface::Menu, Style::default())
            .backdrop_blur
            .is_some();
        let mut h =
            fenestra_shell::Harness::new(Pickers { palette: false }, theme.clone(), (800, 600));
        h.click(&by::role(Semantics::ComboBox));
        let listbox = h.get(&by::id("listbox")).id;
        assert_eq!(
            h.frame().glass_body(listbox).is_some(),
            frosted,
            "{style:?} select"
        );
        let h = fenestra_shell::Harness::new(Pickers { palette: true }, theme, (800, 600));
        let panel = h.get(&by::role(Semantics::Dialog)).id;
        assert_eq!(
            h.frame().glass_body(panel).is_some(),
            frosted,
            "{style:?} palette"
        );
    }
}
