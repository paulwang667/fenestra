//! Sidebar nav-list and rating behavior, plus light/dark state goldens.

use std::path::PathBuf;

use fenestra_core::{App, Element, Key, KeyInput, Semantics, Theme, by, col};
use fenestra_kit::{nav_item, nav_list, rating};
use fenestra_shell::{Harness, testing::assert_png_snapshot};

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

// --------------------------------------------------------------- nav list

struct Nav {
    selected: usize,
}

#[derive(Clone)]
enum NavMsg {
    Go(usize),
}

impl App for Nav {
    type Msg = NavMsg;

    fn update(&mut self, msg: NavMsg) {
        match msg {
            NavMsg::Go(i) => self.selected = i,
        }
    }

    fn view(&self) -> Element<NavMsg> {
        col().p(16.0).child(
            nav_list(
                [
                    nav_item("Alerts").icon(fenestra_kit::icons::lucide::by_name("bell").unwrap()),
                    nav_item("Settings")
                        .icon(fenestra_kit::icons::lucide::by_name("settings").unwrap())
                        .badge("12"),
                    nav_item("Profile")
                        .icon(fenestra_kit::icons::lucide::by_name("user").unwrap()),
                ],
                self.selected,
            )
            .on_select(NavMsg::Go)
            .id("nav"),
        )
    }
}

#[test]
fn nav_list_clicks_and_arrows() {
    let mut h = Harness::new(Nav { selected: 0 }, Theme::light(), (280, 200));
    h.click(&by::role(Semantics::ListItem { selected: false }).name("Settings"));
    assert_eq!(h.app().selected, 1);
    // One tab stop; arrows step with wraparound.
    h.focus(&by::id("nav"));
    h.key(KeyInput::plain(Key::ArrowDown));
    assert_eq!(h.app().selected, 2);
    h.key(KeyInput::plain(Key::ArrowDown)); // wraps to the first row
    assert_eq!(h.app().selected, 0);
    h.key(KeyInput::plain(Key::End));
    assert_eq!(h.app().selected, 2);
    let yaml = h.frame().access_yaml();
    assert!(yaml.contains("listitem"), "{yaml}");
    assert!(yaml.contains("[selected]"), "{yaml}");
}

// ----------------------------------------------------------------- rating

struct Rated {
    value: f32,
}

#[derive(Clone)]
enum RatedMsg {
    Rate(f32),
}

impl App for Rated {
    type Msg = RatedMsg;

    fn update(&mut self, msg: RatedMsg) {
        match msg {
            RatedMsg::Rate(v) => self.value = v,
        }
    }

    fn view(&self) -> Element<RatedMsg> {
        col()
            .p(16.0)
            .child(rating(self.value, 5).precision(0.5).on_change(RatedMsg::Rate).id("stars"))
    }
}

#[test]
fn rating_clicks_and_steps() {
    let mut h = Harness::new(
        Rated { value: 3.0 },
        Theme::light(),
        (220, 100),
    );
    // Clicking the middle of the row lands on the 3rd star.
    h.click(&by::role(Semantics::Slider {
        value: 3.0,
        min: 0.0,
        max: 5.0,
    }));
    assert_eq!(h.app().value, 3.0);
    h.focus(&by::id("stars"));
    h.key(KeyInput::plain(Key::ArrowRight)); // half-star step
    assert_eq!(h.app().value, 3.5);
    h.key(KeyInput::plain(Key::Home));
    assert_eq!(h.app().value, 0.0);
    h.key(KeyInput::plain(Key::End));
    assert_eq!(h.app().value, 5.0);
    // The rating projects as a slider with a friendly value text.
    let yaml = h.frame().access_yaml();
    assert!(yaml.contains("slider"), "{yaml}");
    assert!(yaml.contains("of 5 stars"), "{yaml}");
}

// ---------------------------------------------------------------- goldens

fn nav_scene(theme: &Theme) -> Element<()> {
    col().p(16.0).bg(theme.bg).child(
        nav_list(
            [
                nav_item("Alerts").icon(fenestra_kit::icons::lucide::by_name("bell").unwrap()),
                nav_item("Inbox").badge("3"),
                nav_item("Settings")
                    .icon(fenestra_kit::icons::lucide::by_name("settings").unwrap())
                    .badge("12"),
                nav_item("Profile"),
            ],
            1,
        )
        .on_select(|_| ()),
    )
}

#[test]
fn nav_list_golden() {
    let theme = Theme::light();
    let image = fenestra_shell::render_element(nav_scene(&theme), &theme, (260, 220));
    assert_png_snapshot(snapshot_dir(), "nav_list_light", &image);
}

#[test]
fn nav_list_dark_golden() {
    let theme = Theme::dark();
    let image = fenestra_shell::render_element(nav_scene(&theme), &theme, (260, 220));
    assert_png_snapshot(snapshot_dir(), "nav_list_dark", &image);
}

fn rating_scene(theme: &Theme) -> Element<()> {
    col().p(16.0).gap(8.0).bg(theme.bg).children([
        rating(3.0, 5).on_change(|_| ()),
        rating(3.5, 5).on_change(|_| ()),
        rating(0.0, 5).on_change(|_| ()),
        rating(4.0, 5).read_only(true),
        rating(5.0, 5).disabled(true).on_change(|_| ()),
    ])
}

#[test]
fn rating_golden() {
    let theme = Theme::light();
    let image = fenestra_shell::render_element(rating_scene(&theme), &theme, (180, 200));
    assert_png_snapshot(snapshot_dir(), "rating_light", &image);
}

#[test]
fn rating_dark_golden() {
    let theme = Theme::dark();
    let image = fenestra_shell::render_element(rating_scene(&theme), &theme, (180, 200));
    assert_png_snapshot(snapshot_dir(), "rating_dark", &image);
}
