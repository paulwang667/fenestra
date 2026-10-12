//! `Slider::on_release`: a drag on the slider reports its end once, after
//! its value changes, so an app can close one undo step per gesture.

use fenestra_core::{App, Element, Theme, by, col};
use fenestra_kit::slider;
use fenestra_shell::Harness;

#[derive(Default)]
struct Probe {
    changes: usize,
    releases: usize,
    /// Released after the last change, so a test can check the order.
    last_was_release: bool,
}

#[derive(Clone)]
enum Msg {
    Changed,
    Released,
}

impl App for Probe {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Changed => {
                self.changes += 1;
                self.last_was_release = false;
            }
            Msg::Released => {
                self.releases += 1;
                self.last_was_release = true;
            }
        }
    }

    fn view(&self) -> Element<Msg> {
        col().w(300.0).children([Element::from(
            slider(0.5)
                .id("s")
                .on_change(|_| Msg::Changed)
                .on_release(Msg::Released),
        )])
    }
}

#[test]
fn a_drag_on_the_slider_reports_its_release_once_after_its_changes() {
    let mut h = Harness::new(Probe::default(), Theme::dark(), (300, 100));
    h.drag(&by::id("s"), &by::id("s"));
    assert!(h.app().changes >= 1, "the drag changes the value");
    assert_eq!(h.app().releases, 1, "the drag ends once");
    assert!(
        h.app().last_was_release,
        "the release comes after the changes"
    );
}
