//! An app-driven (`OverlayMode::Open`) overlay without a backdrop: a press outside it emits its
//! `on_close`, as `OverlayMode::Open`'s doc says, and is swallowed, as an outside press on a
//! `Toggle` menu is. Before, only a backdrop's press did, so a popover with no scrim closed on
//! Esc and nothing else.

use fenestra_core::{
    App, Element, Overlay, OverlayMode, OverlayPlacement, Semantics, by, col, div, row, text,
};
use fenestra_shell::Harness;

#[derive(Default)]
struct Popover {
    open: bool,
    closed: usize,
    elsewhere: usize,
    inside: usize,
}

#[derive(Clone)]
enum Msg {
    Toggle,
    Close,
    Elsewhere,
    Inside,
}

impl App for Popover {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Toggle => self.open = !self.open,
            Msg::Close => {
                self.open = false;
                self.closed += 1;
            }
            Msg::Elsewhere => self.elsewhere += 1,
            Msg::Inside => self.inside += 1,
        }
    }

    fn view(&self) -> Element<Msg> {
        let mut anchor = col().children([div()
            .id("anchor")
            .w(80.0)
            .h(30.0)
            .semantics(Semantics::Button)
            .label("open")
            .on_click(Msg::Toggle)]);
        if self.open {
            anchor = anchor.child(
                col()
                    .id("panel")
                    .w(160.0)
                    .h(80.0)
                    .overlay(Overlay {
                        mode: OverlayMode::Open,
                        placement: OverlayPlacement::Below { gap: 4.0 },
                        backdrop: false,
                        trap_focus: false,
                        enter: false,
                        match_anchor_width: false,
                    })
                    .on_close(Msg::Close)
                    .child(
                        div()
                            .id("item")
                            .w(100.0)
                            .h(30.0)
                            .semantics(Semantics::Button)
                            .label("item")
                            .on_click(Msg::Inside),
                    ),
            );
        }
        row().p(10.0).gap(200.0).children([
            anchor,
            div()
                .id("elsewhere")
                .w(80.0)
                .h(30.0)
                .semantics(Semantics::Button)
                .label("elsewhere")
                .on_click(Msg::Elsewhere)
                .child(text("x")),
        ])
    }
}

fn opened() -> Harness<Popover> {
    let mut h = Harness::new(Popover::default(), fenestra_core::Theme::light(), (480, 240));
    h.click(&by::id("anchor"));
    assert!(h.app().open, "the anchor did not open it");
    assert!(h.query(&by::id("panel")).is_some(), "the panel is not drawn");
    h
}

#[test]
fn a_press_outside_closes_it_and_goes_no_further() {
    let mut h = opened();
    h.click(&by::id("elsewhere"));
    assert_eq!(h.app().closed, 1, "a press outside did not emit on_close");
    assert!(!h.app().open, "it is still open");
    assert_eq!(h.app().elsewhere, 0, "the press that closed it also pressed what was under it");
}

#[test]
fn a_press_inside_does_not_close_it() {
    let mut h = opened();
    h.click(&by::id("item"));
    assert_eq!(h.app().inside, 1, "the item inside was not pressed");
    assert_eq!(h.app().closed, 0, "a press inside closed it");
}

#[test]
fn a_press_on_its_anchor_is_the_anchors_own() {
    let mut h = opened();
    h.click(&by::id("anchor"));
    assert_eq!(h.app().closed, 0, "the anchor's press also emitted on_close");
    assert!(!h.app().open, "the anchor's own toggle did not close it");
}
