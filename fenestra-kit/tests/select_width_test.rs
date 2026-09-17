//! The `fill` select's listbox matches its trigger's width - it does not
//! size to the whole canvas.

use fenestra_core::{App, Element, Key, KeyInput, Semantics, Theme, by, col, row, text};
use fenestra_kit::{button, select};
use fenestra_shell::Harness;

#[derive(Clone)]
enum Msg {
    Pick(usize),
    Go,
}

struct A;
impl App for A {
    type Msg = Msg;
    fn update(&mut self, _: Msg) {}
    fn view(&self) -> Element<Msg> {
        col().child(
            row()
                .items_center()
                .gap(8.0)
                .p(16.0)
                .children([
                    text("Model"),
                    select(
                        0,
                        ["deepseek-chat", "deepseek-reasoner", "deepseek-coder"],
                    )
                    .fill()
                    .on_change(Msg::Pick)
                    .into(),
                    button("Go").on_click(Msg::Go).into(),
                ]),
        )
    }
}

#[test]
fn fill_select_listbox_matches_trigger_width() {
    let mut h = Harness::new(A, Theme::light(), (640, 240));
    h.rebuild();
    // Focus the trigger and press Space: that toggles its anchored menu
    // (the trigger has no click handler, so a click can land on a child).
    h.focus(&by::role(Semantics::ComboBox));
    h.key(KeyInput::plain(Key::Space));
    h.rebuild();

    // The listbox panel is present and the combobox reports it expanded.
    assert!(
        h.query(&by::id("listbox")).is_some(),
        "listbox panel did not open"
    );
    let trigger = h.get(&by::role(Semantics::ComboBox));
    assert!(trigger.expanded, "combobox did not expand");

    let trigger_w = trigger.rect.width();
    let listbox_w = h.get(&by::id("listbox")).rect.width();

    // The trigger is NOT full-width (it shares the row with the label and the
    // button), so a buggy listbox would be visibly wider than the trigger.
    assert!(
        trigger_w < 640.0 * 0.9,
        "trigger unexpectedly full-width: {trigger_w}px"
    );
    // The listbox matches the trigger's width (within a couple of px for the
    // panel border), not the canvas.
    assert!(
        (listbox_w - trigger_w).abs() <= 6.0,
        "listbox {listbox_w}px does not match trigger {trigger_w}px"
    );

    // Keep a visual artifact for review.
    let img = h.render();
    let out = std::env::temp_dir().join("select_width_check.png");
    img.save(&out).expect("save png");
    eprintln!("saved {out:?} trigger={trigger_w}px listbox={listbox_w}px");
}
