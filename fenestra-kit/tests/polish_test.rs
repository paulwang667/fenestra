//! Button loading state, hyperlink role, FAB, and the page control.

use fenestra_core::{App, Element, Semantics, Theme, by, col};
use fenestra_kit::{button, fab, hyperlink, icons, page_control};
use fenestra_shell::Harness;

#[derive(Clone)]
enum Msg {
    Tap,
}

fn loading_view() -> Element<Msg> {
    col().p(16.0).child(button("Save").loading(true).on_click(Msg::Tap))
}

fn link_view() -> Element<Msg> {
    col().p(16.0).child(hyperlink("Read the docs").on_click(Msg::Tap))
}

fn fab_view() -> Element<Msg> {
    col()
        .p(16.0)
        .child(fab(icons::plus()).label("Compose").on_click(Msg::Tap))
}

fn pager_view() -> Element<Msg> {
    col().p(16.0).child(page_control(5, 1))
}

/// A loading button is inert (no click messages) and projects `aria-busy`.
#[test]
fn loading_button_is_inert_and_busy() {
    struct App1;
    impl App for App1 {
        type Msg = Msg;
        fn update(&mut self, _: Msg) {}
        fn view(&self) -> Element<Msg> {
            loading_view()
        }
    }
    let mut h = Harness::new(App1, Theme::light(), (300, 80));
    h.click(&by::role(Semantics::Button).name("Save"));
    let yaml = h.frame().access_yaml();
    assert!(yaml.contains("[busy]"), "{yaml}");
    assert!(yaml.contains("button \"Save\""), "{yaml}");
}

/// The hyperlink carries the ARIA `link` role and activates like a button.
#[test]
fn hyperlink_projects_link_role_and_clicks() {
    struct App2 {
        clicks: usize,
    }
    impl App for App2 {
        type Msg = Msg;
        fn update(&mut self, _: Msg) {
            self.clicks += 1;
        }
        fn view(&self) -> Element<Msg> {
            link_view()
        }
    }
    let mut h = Harness::new(App2 { clicks: 0 }, Theme::light(), (300, 80));
    let yaml = h.frame().access_yaml();
    assert!(yaml.contains("link \"Read the docs\""), "{yaml}");
    h.click(&by::role(Semantics::Link));
    assert_eq!(h.app().clicks, 1);
}

/// The FAB is a named button (the label is its accessible name).
#[test]
fn fab_is_a_named_button() {
    struct App3 {
        clicks: usize,
    }
    impl App for App3 {
        type Msg = Msg;
        fn update(&mut self, _: Msg) {
            self.clicks += 1;
        }
        fn view(&self) -> Element<Msg> {
            fab_view()
        }
    }
    let mut h = Harness::new(App3 { clicks: 0 }, Theme::light(), (160, 120));
    assert!(h.query(&by::role(Semantics::Button).name("Compose")).is_some());
    h.click(&by::role(Semantics::Button).name("Compose"));
    assert_eq!(h.app().clicks, 1);
}

/// The page control is a pure indicator: it renders and stays inert.
#[test]
fn page_control_is_an_inert_indicator() {
    struct App4;
    impl App for App4 {
        type Msg = Msg;
        fn update(&mut self, _: Msg) {}
        fn view(&self) -> Element<Msg> {
            pager_view()
        }
    }
    let mut h = Harness::new(App4, Theme::light(), (160, 80));
    let yaml = h.frame().access_yaml();
    assert!(!yaml.contains("button"), "indicator must have no controls: {yaml}");
}
