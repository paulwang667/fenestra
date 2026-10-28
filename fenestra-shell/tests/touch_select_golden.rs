//! What touch selection looks like, per platform style: the handles and
//! the edit menu over a selected word in a field, and the magnifier while
//! a handle is dragged.

use fenestra_core::{App, Element, SelectionControls, Theme, by, col, raw_input};
use fenestra_shell::Harness;
use fenestra_shell::testing::assert_png_snapshot;

struct Page;

impl App for Page {
    type Msg = ();
    fn update(&mut self, _msg: ()) {}
    fn view(&self) -> Element<()> {
        col()
            .p(20.0)
            .pt(90.0)
            .children([raw_input("select some text", "")
                .label("field")
                .w(260.0)
                .h(44.0)])
    }
}

fn snapshot_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
}

fn harness(controls: SelectionControls) -> Harness<Page> {
    let mut theme = Theme::light();
    theme.selection_controls = controls;
    let mut h = Harness::new(Page, theme, (320, 240));
    h.clipboard().set("x".to_owned());
    let r = h.get(&by::label("field")).rect;
    // Long-press "some".
    #[expect(clippy::cast_possible_truncation)]
    h.touch_down_at(((r.x0 + 70.0) as f32, r.center().y as f32));
    h.pump(500.0);
    h.touch_up();
    assert!(h.touch_selection().is_some());
    h
}

#[test]
fn touch_select_material() {
    let mut h = harness(SelectionControls::Material);
    assert_png_snapshot(snapshot_dir(), "touch_select_material", &h.render());
}

#[test]
fn touch_select_cupertino() {
    let mut h = harness(SelectionControls::Cupertino);
    assert_png_snapshot(snapshot_dir(), "touch_select_cupertino", &h.render());
}

/// Drags the end handle a little right and renders before lifting.
fn dragging(controls: SelectionControls) -> Harness<Page> {
    let mut h = harness(controls);
    let c = h.touch_selection().unwrap().end_handle.unwrap().center();
    #[expect(clippy::cast_possible_truncation)]
    {
        h.touch_down_at((c.x as f32, c.y as f32));
        h.touch_move_to(((c.x + 30.0) as f32, c.y as f32));
    }
    assert!(h.touch_selection().unwrap().magnifier.is_some());
    h
}

#[test]
fn touch_magnifier_material() {
    let mut h = dragging(SelectionControls::Material);
    assert_png_snapshot(snapshot_dir(), "touch_magnifier_material", &h.render());
}

#[test]
fn touch_magnifier_cupertino() {
    let mut h = dragging(SelectionControls::Cupertino);
    assert_png_snapshot(snapshot_dir(), "touch_magnifier_cupertino", &h.render());
}
