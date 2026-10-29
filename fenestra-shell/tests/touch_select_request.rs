//! An app selects a text's whole content by handing it a new token
//! (`select_all_on`), as `focus_on` moves focus: that is how a message's
//! "Select text" menu item starts a touch selection.

use fenestra_core::{App, EditAction, Element, Theme, by, col, div, text};
use fenestra_shell::Harness;

#[derive(Default)]
struct Page {
    token: u64,
    selectable: bool,
}

impl App for Page {
    type Msg = ();
    fn update(&mut self, _msg: ()) {}
    fn view(&self) -> Element<()> {
        let mut line = text("hello wide world").size_px(20.0);
        if self.selectable {
            line = line.selectable();
        }
        if self.token > 0 {
            line = line.select_all_on(self.token);
        }
        col()
            .p(20.0)
            .gap(20.0)
            .children([line, div().label("blank").w(200.0).h(60.0)])
    }
}

/// The finger is the pointer source (a tap on blank space), as on a phone.
fn touched(page: Page) -> Harness<Page> {
    let mut h = Harness::new(page, Theme::light(), (400, 200));
    h.touch_tap(&by::label("blank"));
    h
}

#[test]
fn select_all_on_selects_and_shows_the_menu() {
    let mut h = touched(Page {
        token: 0,
        selectable: true,
    });
    assert!(h.touch_selection().is_none());
    h.app_mut().token = 1;
    h.rebuild();
    let sel = h.touch_selection().expect("selected");
    assert_eq!(sel.range, (0, 16));
    let actions: Vec<_> = sel.menu.iter().map(|(a, _)| *a).collect();
    assert_eq!(actions, vec![EditAction::Copy, EditAction::SelectAll]);
}

#[test]
fn the_same_token_does_not_reselect() {
    let mut h = touched(Page {
        token: 0,
        selectable: true,
    });
    h.app_mut().token = 1;
    h.rebuild();
    assert!(h.touch_selection().is_some());
    h.touch_tap(&by::label("blank"));
    assert!(h.touch_selection().is_none());
    h.rebuild();
    assert!(
        h.touch_selection().is_none(),
        "the token was already honored"
    );
    h.app_mut().token = 2;
    h.rebuild();
    assert!(h.touch_selection().is_some(), "a new token selects again");
}

#[test]
fn select_all_on_a_non_selectable_text_does_nothing() {
    let h = Harness::new(
        Page {
            token: 1,
            selectable: false,
        },
        Theme::light(),
        (400, 200),
    );
    assert!(h.touch_selection().is_none());
}

/// `copy_text_on`: a new token copies the text's content to the clipboard
/// (an app's own "Copy" command), once.
#[derive(Default)]
struct Copier {
    token: u64,
}

impl App for Copier {
    type Msg = ();
    fn update(&mut self, _msg: ()) {}
    fn view(&self) -> Element<()> {
        let mut line = text("copy me");
        if self.token > 0 {
            line = line.copy_text_on(self.token);
        }
        col().children([line])
    }
}

#[test]
fn copy_text_on_copies_once_per_token() {
    let mut h = Harness::new(Copier::default(), Theme::light(), (200, 100));
    assert_eq!(h.clipboard().get(), None);
    h.app_mut().token = 1;
    h.rebuild();
    assert_eq!(h.clipboard().get().as_deref(), Some("copy me"));
    h.clipboard().set("other".to_owned());
    h.rebuild();
    assert_eq!(
        h.clipboard().get().as_deref(),
        Some("other"),
        "the same token copies once"
    );
}
