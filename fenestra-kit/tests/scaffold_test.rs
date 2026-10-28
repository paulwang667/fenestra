//! The scaffold: an app bar and a bottom bar around content, laid out by the
//! theme's material style — floating over the content in the glass styles
//! (Liquid Glass's bottom bar a dock clear of the edges), beside it in solid
//! and tonal — with one tree in every style.

use fenestra_core::{
    App, Element, Fonts, FrameState, MaterialStyle, Semantics, Theme, build_frame, by, col, text,
};
use fenestra_kit::{app_bar, scaffold, tab_bar, tab_item};
use fenestra_shell::Harness;

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Tab(usize),
}

fn page(tab: usize) -> Element<Msg> {
    let rows = (0..30).map(|i| text(format!("Row {i}")).h(48.0));
    scaffold(
        col()
            .id("list")
            .semantics(Semantics::Label)
            .w_full()
            .scroll_y()
            .children(rows),
    )
    .top(
        Element::from(app_bar("Title"))
            .id("top")
            .semantics(Semantics::Label),
        48.0,
    )
    .bottom(
        tab_bar(
            ["Chats", "Contacts", "Discover", "Me"]
                .into_iter()
                .enumerate()
                .map(|(i, name)| tab_item(name, text("•"), i == tab, Msg::Tab(i))),
        )
        .id("tabs")
        .semantics(Semantics::Label),
        56.0,
    )
    .into()
}

fn frame_for(style: MaterialStyle) -> fenestra_core::Frame {
    frame_in(style, &mut FrameState::new(), &page(0))
}

fn frame_in(
    style: MaterialStyle,
    state: &mut FrameState,
    view: &Element<Msg>,
) -> fenestra_core::Frame {
    let theme = Theme::dark().with_material_style(style);
    let mut fonts = Fonts::embedded();
    build_frame(view, &theme, &mut fonts, state, (411.0, 800.0), 1.0)
}

/// The content's own padding stays: the scaffold adds the bars' room to it
/// in every style, so the first row sits the same distance below the bar.
#[test]
fn the_content_keeps_its_own_padding() {
    let view = Element::from(
        scaffold(
            col()
                .id("list")
                .semantics(Semantics::Label)
                .w_full()
                .p(16.0)
                .scroll_y()
                .child(text("First").id("first").semantics(Semantics::Label)),
        )
        .top(
            Element::from(app_bar("Title"))
                .id("top")
                .semantics(Semantics::Label),
            48.0,
        ),
    );
    for style in MaterialStyle::ALL {
        let f = frame_in(style, &mut FrameState::new(), &view);
        let gap = f.get(&by::id("first")).rect.y0 - f.get(&by::id("top")).rect.y1;
        assert!(
            (gap - 16.0).abs() < 0.5,
            "{style:?}: first row {gap} below the bar"
        );
    }
}

/// A field focused and a list scrolled in one style are still focused and
/// scrolled after a switch to another, the same frame state carried over.
#[test]
fn switching_style_keeps_focus_and_scroll() {
    let view = Element::from(
        scaffold(
            col()
                .id("list")
                .semantics(Semantics::Label)
                .w_full()
                .h_full()
                .scroll_y()
                .child(Element::from(fenestra_kit::text_input("")).id("field"))
                .children((0..30).map(|i| text(format!("Row {i}")).h(48.0).shrink0())),
        )
        .top(
            Element::from(app_bar("Title"))
                .id("top")
                .semantics(Semantics::Label),
            48.0,
        ),
    );
    for (from, to) in [
        (MaterialStyle::Solid, MaterialStyle::LiquidGlass),
        (MaterialStyle::LiquidGlass, MaterialStyle::Solid),
        (MaterialStyle::Acrylic, MaterialStyle::Tonal),
    ] {
        let mut state = FrameState::new();
        let f = frame_in(from, &mut state, &view);
        let (field, list) = (f.get(&by::id("field")).id, f.get(&by::id("list")).id);
        state.set_focus(Some(field));
        // A frame with the field focused (a tap draws one), then the user
        // scrolls: focus moving to the field scrolls it into view once.
        frame_in(from, &mut state, &view);
        state.scroll_to(list, 200.0);
        let f = frame_in(from, &mut state, &view);
        let before = f.scroll_offset(list);
        let f = frame_in(to, &mut state, &view);
        assert_eq!(state.focused(), Some(field), "{from:?} → {to:?} focus");
        assert_eq!(f.get(&by::id("field")).id, field, "{from:?} → {to:?} id");
        assert!(before > 0.0, "{from:?} scrolled");
        assert!(
            (f.scroll_offset(list) - before).abs() < 0.5,
            "{from:?} → {to:?} scroll {} vs {before}",
            f.scroll_offset(list)
        );
    }
}

#[test]
fn scaffold_lays_out_each_material_style() {
    for style in MaterialStyle::ALL {
        let f = frame_for(style);
        let rect = |id| f.get(&by::id(id)).rect;
        let (top, list, bottom) = (rect("top"), rect("list"), rect("tabs"));
        match style {
            MaterialStyle::LiquidGlass => {
                assert!(
                    list.y0 <= top.y0 + 0.5 && list.y1 >= bottom.y1 - 0.5,
                    "{style:?} under both"
                );
                assert!(
                    bottom.x0 > 4.0 && bottom.x1 < 407.0 && bottom.y1 < 796.0,
                    "{style:?} dock {bottom:?}"
                );
            }
            MaterialStyle::Acrylic => {
                assert!(
                    list.y0 <= top.y0 + 0.5 && list.y1 >= bottom.y1 - 0.5,
                    "{style:?} under both"
                );
                assert!(
                    bottom.x0 <= 0.5 && bottom.x1 >= 410.5,
                    "{style:?} edge to edge {bottom:?}"
                );
            }
            MaterialStyle::Solid | MaterialStyle::Tonal => {
                assert!(
                    list.y0 >= top.y1 - 0.5 && list.y1 <= bottom.y0 + 0.5,
                    "{style:?} between {top:?} {list:?} {bottom:?}"
                );
            }
        }
    }
}

/// Focus and scroll live in the frame state by widget id; one tree in
/// every style means a style switch keeps both.
#[test]
fn switching_style_keeps_widget_identity() {
    let ids: Vec<_> = MaterialStyle::ALL
        .into_iter()
        .map(|style| {
            let f = frame_for(style);
            ["top", "list", "tabs"].map(|id| f.get(&by::id(id)).id)
        })
        .collect();
    for pair in ids.windows(2) {
        assert_eq!(pair[0], pair[1]);
    }
}

#[derive(Default)]
struct Tabs {
    tab: usize,
}

impl App for Tabs {
    type Msg = Msg;
    fn update(&mut self, Msg::Tab(i): Msg) {
        self.tab = i;
    }
    fn view(&self) -> Element<Msg> {
        page(self.tab)
    }
}

#[test]
fn tab_bar_items_are_named_buttons() {
    let mut h = Harness::new(Tabs::default(), Theme::dark(), (411, 800));
    h.click(&by::role(Semantics::Button).name("Contacts"));
    assert_eq!(h.app().tab, 1);
}
