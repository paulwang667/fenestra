//! A press the harness reports as delivered has to have been delivered.
//!
//! `click` presses the centre of a node's *layout* rect. That rect is not
//! clamped to the window and not clipped by whatever sits between the node
//! and the pointer, so a control scrolled below the fold, covered by an
//! overlay, or clipped out of its scroll container used to take the press
//! somewhere else entirely: the handler never ran, nothing was reported, and
//! the test read as "the button does nothing". That is a placebo — it goes
//! green the day somebody deletes the button.

use fenestra_core::{App, Element, Semantics, Theme, by, col, div, text};
use fenestra_shell::Harness;

#[derive(Default)]
struct Pressed {
    count: usize,
}

#[derive(Clone)]
enum Msg {
    Press,
}

impl App for Pressed {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Press => self.count += 1,
        }
    }

    /// One short list in a scrolling column, with the button at the bottom.
    /// The window is 200 tall and the content is far taller, so the button
    /// starts well below it.
    fn view(&self) -> Element<Msg> {
        let filler: Vec<Element<Msg>> = (0..20)
            .map(|i| text(format!("row {i}")).h(30.0).shrink0().into())
            .collect();
        col().w_full().h_full().scroll_y().id("body").children(
            filler
                .into_iter()
                .chain([div()
                    .h(30.0)
                    .shrink0()
                    .focusable(true)
                    .semantics(Semantics::Button)
                    .label("Deep")
                    .on_click(Msg::Press)
                    .into()])
                .collect::<Vec<_>>(),
        )
    }
}

fn deep() -> fenestra_core::Query {
    by::role(Semantics::Button).name("Deep")
}

#[test]
#[should_panic(expected = "does not reach")]
fn pressing_a_control_below_the_fold_is_refused() {
    let mut h = Harness::new(Pressed::default(), Theme::light(), (300, 200));
    h.rebuild();
    // It is in the tree and has a rect — that is exactly the trap.
    assert!(h.query(&deep()).is_some(), "the button should exist");
    h.click(&deep());
}

/// And once it is scrolled to, the press lands.
#[test]
fn pressing_it_after_scrolling_to_it_works() {
    let mut h = Harness::new(Pressed::default(), Theme::light(), (300, 200));
    h.rebuild();
    for _ in 0..40 {
        if h.get(&deep()).rect.y1 <= 200.0 {
            break;
        }
        h.wheel(&by::id("body"), -120.0);
    }
    h.click(&deep());
    assert_eq!(h.app().count, 1, "the press did not reach the button");
}
