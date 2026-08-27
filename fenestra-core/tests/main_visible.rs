//! `App::main_visible`: the app getting out of its own way.
//!
//! Declared, not commanded — the same shape as `App::windows`, so a window
//! hidden while something is happening comes back when it stops without
//! anybody remembering to say so.

use fenestra_core::{App, Element, WindowDesc, col, text};

#[derive(Default)]
struct Picker {
    picking: bool,
}

impl App for Picker {
    type Msg = ();

    fn update(&mut self, (): ()) {
        self.picking = !self.picking;
    }

    fn view(&self) -> Element<()> {
        col().child(text("main"))
    }

    fn windows(&self) -> Vec<WindowDesc<()>> {
        if !self.picking {
            return Vec::new();
        }
        vec![WindowDesc::new("pick", "Pick", (100.0, 100.0), ())]
    }

    fn main_visible(&self) -> bool {
        !self.picking
    }
}

#[test]
fn an_app_can_declare_its_own_window_away_and_back() {
    let mut app = Picker::default();
    assert!(app.main_visible(), "the window started hidden");
    assert!(app.windows().is_empty());

    app.update(());
    assert!(!app.main_visible(), "the window stayed while picking");
    assert_eq!(
        app.windows().len(),
        1,
        "the overlay went away with the window it was declared beside"
    );

    app.update(());
    assert!(app.main_visible(), "the window did not come back");
}

/// The default is on, so nothing that does not care has to say so.
#[test]
fn an_app_that_says_nothing_keeps_its_window() {
    struct Quiet;
    impl App for Quiet {
        type Msg = ();
        fn update(&mut self, (): ()) {}
        fn view(&self) -> Element<()> {
            col()
        }
    }
    assert!(Quiet.main_visible());
}
