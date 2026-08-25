//! M5 acceptance: headless typing, selection with Shift+arrows, copy and
//! paste through the (in-memory) clipboard, asserting the final string and
//! goldens for every visual state.

use std::path::PathBuf;

use fenestra_core::{
    App, Element, Fonts, FrameState, Key, KeyInput, Semantics, SP3, SP4, Theme, build_frame, col,
    by,
};
use fenestra_kit::text_input;
use fenestra_shell::{Harness, SyntheticEvent, render_app, testing::assert_png_snapshot};

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

struct Form {
    value: String,
}

#[derive(Clone)]
enum Msg {
    Edit(String),
}

/// A read-only field: the app owns the value; nothing can edit it.
#[derive(Default)]
struct Locked {
    value: String,
}

impl App for Locked {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Edit(v) => self.value = v,
        }
    }

    fn view(&self) -> Element<Msg> {
        col()
            .p(SP4)
            .items_start()
            .children([text_input(&self.value).read_only(true)])
    }
}

impl App for Form {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Edit(v) => self.value = v,
        }
    }

    fn view(&self) -> Element<Msg> {
        // p(16): the input occupies x 16..236, y 16..52.
        col().p(SP4).items_start().children([text_input(&self.value)
            .placeholder("Type here…")
            .on_input(Msg::Edit)])
    }
}

fn shift(key: Key) -> SyntheticEvent {
    SyntheticEvent::Key(KeyInput {
        key,
        shift: true,
        ctrl: false,
        alt: false,
        meta: false,
    })
}

fn ctrl(c: char) -> SyntheticEvent {
    SyntheticEvent::Key(KeyInput {
        key: Key::Char(c),
        shift: false,
        ctrl: true,
        alt: false,
        meta: false,
    })
}

const SIZE: (u32, u32) = (270, 70);
const CLICK: SyntheticEvent = SyntheticEvent::MouseMove { x: 60.0, y: 34.0 };

/// Type, select with Shift+arrows, copy, move, paste: the final value
/// reflects every editing operation.
#[test]
fn type_select_copy_paste() {
    let theme = Theme::light();
    let mut app = Form {
        value: String::new(),
    };
    let image = render_app(
        &mut app,
        &[
            CLICK,
            SyntheticEvent::MouseDown,
            SyntheticEvent::MouseUp,
            SyntheticEvent::Text("hello world".into()),
            // Select "world" with Shift+ArrowLeft x5.
            shift(Key::ArrowLeft),
            shift(Key::ArrowLeft),
            shift(Key::ArrowLeft),
            shift(Key::ArrowLeft),
            shift(Key::ArrowLeft),
            ctrl('c'),
            SyntheticEvent::Key(KeyInput::plain(Key::End)),
            SyntheticEvent::Text(" ".into()),
            ctrl('v'),
        ],
        SIZE,
        &theme,
    );
    assert_eq!(app.value, "hello world world");
    assert_png_snapshot(snapshot_dir(), "input_after_paste", &image);
}

/// Cut removes the selection and Home/word-jumps move the caret.
#[test]
fn cut_and_home() {
    let theme = Theme::light();
    let mut app = Form {
        value: String::new(),
    };
    render_app(
        &mut app,
        &[
            CLICK,
            SyntheticEvent::MouseDown,
            SyntheticEvent::MouseUp,
            SyntheticEvent::Text("abcdef".into()),
            shift(Key::ArrowLeft),
            shift(Key::ArrowLeft),
            ctrl('x'),
            SyntheticEvent::Key(KeyInput::plain(Key::Home)),
            ctrl('v'),
        ],
        SIZE,
        &theme,
    );
    assert_eq!(app.value, "efabcd");
}

/// Backspace, select-all replace, and word deletion.
#[test]
fn editing_operations() {
    let theme = Theme::light();
    let mut app = Form {
        value: String::new(),
    };
    render_app(
        &mut app,
        &[
            CLICK,
            SyntheticEvent::MouseDown,
            SyntheticEvent::MouseUp,
            SyntheticEvent::Text("draft".into()),
            SyntheticEvent::Key(KeyInput::plain(Key::Backspace)),
            ctrl('a'),
            SyntheticEvent::Text("final words".into()),
            SyntheticEvent::Key(KeyInput {
                key: Key::Backspace,
                shift: false,
                ctrl: false,
                alt: true,
                meta: false,
            }),
        ],
        SIZE,
        &theme,
    );
    assert_eq!(app.value, "final ");
}

// ------------------------------------------------------------ state goldens

#[test]
fn input_states_golden() {
    let theme = Theme::light();
    let states: Element<()> = col().p(SP4).gap(SP3).items_start().bg(theme.bg).children([
        text_input("").placeholder("Placeholder…").id("empty"),
        text_input("Filled value").id("filled"),
        text_input("Invalid value").invalid(true).id("invalid"),
        text_input("Read-only").read_only(true).id("readonly"),
        text_input("Disabled").disabled(true).id("disabled"),
    ]);
    let image = fenestra_shell::render_element(states, &theme, (270, 244));
    assert_png_snapshot(snapshot_dir(), "input_states_light", &image);
}

#[test]
fn input_states_dark_golden() {
    let theme = Theme::dark();
    let states: Element<()> = col().p(SP4).gap(SP3).items_start().bg(theme.bg).children([
        text_input("").placeholder("Placeholder…").id("empty"),
        text_input("Filled value").id("filled"),
        text_input("Invalid value").invalid(true).id("invalid"),
        text_input("Read-only").read_only(true).id("readonly"),
        text_input("Disabled").disabled(true).id("disabled"),
    ]);
    let image = fenestra_shell::render_element(states, &theme, (270, 244));
    assert_png_snapshot(snapshot_dir(), "input_states_dark", &image);
}

#[test]
fn input_hover_golden() {
    let theme = Theme::light();
    let mut app = Form {
        value: "Hover me".into(),
    };
    let image = render_app(&mut app, &[CLICK], SIZE, &theme);
    assert_png_snapshot(snapshot_dir(), "input_hover", &image);
}

/// Focused input with a selection: accent border, caret, selection tint.
#[test]
fn input_focus_selection_golden() {
    let theme = Theme::light();
    let mut app = Form {
        value: String::new(),
    };
    let image = render_app(
        &mut app,
        &[
            CLICK,
            SyntheticEvent::MouseDown,
            SyntheticEvent::MouseUp,
            SyntheticEvent::Text("selected text".into()),
            shift(Key::ArrowLeft),
            shift(Key::ArrowLeft),
            shift(Key::ArrowLeft),
            shift(Key::ArrowLeft),
        ],
        SIZE,
        &theme,
    );
    assert_eq!(app.value, "selected text");
    assert_png_snapshot(snapshot_dir(), "input_focus_selection", &image);
}

// ---------------------------------------------------------------- read-only

/// A read-only field stays focusable and selectable, but no edit path —
/// typed text, Backspace, and paste all leave the app-owned value alone.
#[test]
fn read_only_keeps_selection_blocks_edits() {
    let mut h = Harness::new(
        Locked {
            value: "locked".into(),
        },
        Theme::light(),
        (300, 80),
    );
    h.click(&by::role(Semantics::TextInput {
        multiline: false,
    }));
    // Select all: the selection must survive on a read-only field.
    h.key(KeyInput {
        key: Key::Char('a'),
        shift: false,
        ctrl: false,
        alt: false,
        meta: true,
    });
    fn sel(h: &Harness<Locked>) -> Option<(usize, usize)> {
        h.frame()
            .get(&by::role(Semantics::TextInput {
                multiline: false,
            }))
            .selection
    }
    assert_eq!(sel(&h), Some((0, 6)), "select-all should span the value");
    h.type_text("X");
    h.key(KeyInput::plain(Key::Backspace));
    assert_eq!(h.app().value, "locked");
    assert_eq!(sel(&h), Some((0, 6)), "edits must not move the selection");
}

/// The read-only and invalid states reach the accessibility tree as
/// `[readonly]` / `[invalid]` attributes (ARIA `aria-readonly` / the
/// `aria-invalid` ring state).
#[test]
fn read_only_and_invalid_project_to_access_tree() {
    let view: Element<()> = col().children([
        text_input("v").read_only(true).id("ro"),
        text_input("w").invalid(true).id("bad"),
    ]);
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let frame = build_frame(
        &view,
        &Theme::light(),
        &mut fonts,
        &mut state,
        (240.0, 100.0),
        1.0,
    );
    let yaml = frame.access_yaml();
    assert!(yaml.contains("[readonly]"), "{yaml}");
    assert!(yaml.contains("[invalid]"), "{yaml}");
}
