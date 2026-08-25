//! Chip toggle/remove behavior and time-picker stepping, plus light/dark
//! state goldens for both.

use std::path::PathBuf;

use fenestra_core::{App, Element, Key, KeyInput, Semantics, Theme, by, col, row};
use fenestra_kit::{chip, time_picker};
use fenestra_shell::{Harness, testing::assert_png_snapshot};

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

// ------------------------------------------------------------------ chips

struct Chips {
    selected: bool,
    removed: bool,
}

#[derive(Clone)]
enum ChipsMsg {
    Toggle(bool),
    Remove,
}

impl App for Chips {
    type Msg = ChipsMsg;

    fn update(&mut self, msg: ChipsMsg) {
        match msg {
            ChipsMsg::Toggle(s) => self.selected = s,
            ChipsMsg::Remove => self.removed = true,
        }
    }

    fn view(&self) -> Element<ChipsMsg> {
        col().p(16.0).gap(8.0).children([row().gap(8.0).children([
            chip("Rust").selected(self.selected).on_toggle(ChipsMsg::Toggle),
            chip("Static"),
            chip("Tag").on_remove(ChipsMsg::Remove),
            chip("Locked").disabled(true),
        ])])
    }
}

#[test]
fn chip_toggles_and_removes() {
    let mut h = Harness::new(
        Chips {
            selected: false,
            removed: false,
        },
        Theme::light(),
        (420, 120),
    );
    // The toggle chip is a checkbox to assistive tech; clicking flips it.
    h.click(&by::role(Semantics::Checkbox {
        checked: false,
        mixed: false,
    })
    .name("Rust"));
    assert!(h.app().selected);
    assert!(h
        .query(&by::role(Semantics::Checkbox {
            checked: true,
            mixed: false,
        }))
        .is_some());
    // The dismissible chip's × is its own accessible button.
    h.click(&by::role(Semantics::Button).name("Remove Tag"));
    assert!(h.app().removed);
}

// ------------------------------------------------------------ time picker

struct Clock {
    h: u32,
    m: u32,
    s: u32,
}

#[derive(Clone)]
enum ClockMsg {
    At(u32, u32, u32),
}

impl App for Clock {
    type Msg = ClockMsg;

    fn update(&mut self, msg: ClockMsg) {
        match msg {
            ClockMsg::At(h, m, s) => (self.h, self.m, self.s) = (h, m, s),
        }
    }

    fn view(&self) -> Element<ClockMsg> {
        col().p(16.0).child(time_picker(self.h, self.m, self.s).with_seconds(true).on_change(ClockMsg::At))
    }
}

#[test]
fn time_picker_steps_with_wraparound() {
    let mut h = Harness::new(
        Clock { h: 23, m: 0, s: 59 },
        Theme::light(),
        (240, 120),
    );
    h.click(&by::label("Hour"));
    h.key(KeyInput::plain(Key::ArrowUp)); // 23 wraps to 00
    assert_eq!((h.app().h, h.app().m, h.app().s), (0, 0, 59));
    h.click(&by::label("Minute"));
    h.key(KeyInput::plain(Key::ArrowDown)); // 00 wraps to 59
    assert_eq!(h.app().m, 59);
    h.click(&by::label("Second"));
    h.key(KeyInput::plain(Key::ArrowDown)); // 59 → 58
    assert_eq!(h.app().s, 58);
    // Segments are spinbuttons with zero-padded value text.
    let yaml = h.frame().access_yaml();
    assert!(yaml.contains("spinbutton"), "{yaml}");
    assert!(yaml.contains("[value=\"00\"]"), "{yaml}");
}

// ---------------------------------------------------------------- goldens

fn chip_scene(theme: &Theme) -> Element<()> {
    col()
        .p(16.0)
        .gap(8.0)
        .bg(theme.bg)
        .children([
            row().gap(8.0).children([
                chip("Rust"),
                chip("Rust").selected(true),
                chip("Static"),
            ]),
            row().gap(8.0).children([
                chip("Draft").on_remove(()),
                chip("Draft").selected(true).on_remove(()),
                chip("Locked").disabled(true),
            ]),
        ])
}

#[test]
fn chip_states_golden() {
    let theme = Theme::light();
    let image = fenestra_shell::render_element(chip_scene(&theme), &theme, (360, 120));
    assert_png_snapshot(snapshot_dir(), "chip_states_light", &image);
}

#[test]
fn chip_states_dark_golden() {
    let theme = Theme::dark();
    let image = fenestra_shell::render_element(chip_scene(&theme), &theme, (360, 120));
    assert_png_snapshot(snapshot_dir(), "chip_states_dark", &image);
}

fn time_scene(theme: &Theme) -> Element<()> {
    col()
        .p(16.0)
        .gap(8.0)
        .bg(theme.bg)
        .children([
            time_picker(9, 30, 0).on_change(|_, _, _| ()),
            time_picker(14, 5, 59).with_seconds(true).on_change(|_, _, _| ()),
            time_picker(0, 0, 0).disabled(true).on_change(|_, _, _| ()),
        ])
}

#[test]
fn time_picker_golden() {
    let theme = Theme::light();
    let image = fenestra_shell::render_element(time_scene(&theme), &theme, (240, 180));
    assert_png_snapshot(snapshot_dir(), "time_picker_light", &image);
}

#[test]
fn time_picker_dark_golden() {
    let theme = Theme::dark();
    let image = fenestra_shell::render_element(time_scene(&theme), &theme, (240, 180));
    assert_png_snapshot(snapshot_dir(), "time_picker_dark", &image);
}
