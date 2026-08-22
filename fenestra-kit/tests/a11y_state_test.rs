//! 0.6 accessibility state: live regions reach the tree (and toasts set
//! them automatically), and text inputs expose their caret/selection
//! headlessly.

use fenestra_core::{App, Element, Key, KeyInput, Semantics, Theme, by, col, div, text};
use fenestra_kit::{accordion, accordion_item, checkbox, text_input, TreeNode, tree_view};
use fenestra_shell::Harness;

#[derive(Default)]
struct Status {
    note: String,
    value: String,
}

#[derive(Clone)]
enum Msg {
    Set(String),
}

impl App for Status {
    type Msg = Msg;

    fn update(&mut self, Msg::Set(s): Msg) {
        self.value = s;
    }

    fn view(&self) -> Element<Msg> {
        col().p(16.0).gap(8.0).items_start().children((
            div()
                .live()
                .id("status")
                .children([text(self.note.clone())]),
            text_input(&self.value)
                .width(220.0)
                .on_input(|s| Msg::Set(s.to_owned()))
                .id("field"),
        ))
    }
}

#[test]
fn live_regions_reach_the_tree_and_the_yaml() {
    let mut h = Harness::new(Status::default(), Theme::light(), (400, 200));
    h.app_mut().note = "Saved!".to_owned();
    h.rebuild();
    let node = h.get(&by::id("status"));
    assert!(node.live, "the region is marked live");
    assert!(
        h.frame().access_yaml().contains("[live]"),
        "yaml shows it:\n{}",
        h.frame().access_yaml()
    );
}

#[test]
fn toasts_are_live_automatically() {
    use fenestra_core::{Fonts, FrameState, build_frame};
    let view: Element<()> = col().children([Element::from(fenestra_kit::toast_stack([(
        "Copied to clipboard",
        fenestra_kit::Status::Accent,
    )]))]);
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let frame = build_frame(
        &view,
        &Theme::light(),
        &mut fonts,
        &mut state,
        (400.0, 200.0),
        1.0,
    );
    let toast = frame.get(&by::role(fenestra_core::Semantics::Alert));
    assert_eq!(toast.label.as_deref(), Some("Copied to clipboard"));
    assert!(toast.live, "toasts announce politely");
}

#[test]
fn inputs_expose_caret_and_selection() {
    let mut h = Harness::new(Status::default(), Theme::light(), (400, 200));
    h.tab();
    h.type_text("hello");
    // Collapsed selection = caret after the typed text.
    let node = h.get(&by::id("field"));
    assert_eq!(node.selection, Some((5, 5)), "caret sits at the end");

    // Select-all widens the exposed range to the whole value.
    let mut select_all = KeyInput::plain(Key::Char('a'));
    select_all.meta = true;
    h.key(select_all);
    let node = h.get(&by::id("field"));
    assert_eq!(node.selection, Some((0, 5)));

    // Home collapses it back to the start.
    h.key(KeyInput::plain(Key::Home));
    let node = h.get(&by::id("field"));
    assert_eq!(node.selection, Some((0, 0)));
}
/// `disabled` and `invalid` are modeled on `AccessNode` and must reach the
/// headless accessibility tree (and therefore be projected to AccessKit), so a
/// screen reader announces `aria-disabled` / `aria-invalid` instead of
/// treating a disabled control as absent.

#[derive(Default)]
struct State {
    value: String,
}

impl App for State {
    type Msg = Msg;

    fn update(&mut self, msg: Msg) {
        match msg {
            Msg::Set(s) => self.value = s,
        }
    }

    fn view(&self) -> Element<Msg> {
        col()
            .p(16.0)
            .gap(8.0)
            .items_start()
            .children((
                checkbox(true)
                    .disabled(true)
                    .id("dis"),
                text_input(&self.value)
                    .invalid(true)
                    .width(200.0)
                    .on_input(Msg::Set)
                    .id("inv"),
            ))
    }
}

#[test]
fn disabled_and_invalid_states_reach_the_access_tree() {
    let h = Harness::new(State::default(), Theme::light(), (400, 200));

    // A disabled checkbox keeps its role but is marked disabled and non-focusable.
    let dis = h.get(&by::id("dis"));
    assert!(dis.disabled, "disabled checkbox is marked disabled");
    assert!(!dis.focusable, "disabled controls cannot be tabbed to");
    assert!(
        matches!(dis.semantics, Some(Semantics::Checkbox { checked: true, mixed: false })),
        "role is preserved while disabled (not dropped from the tree)\n{}",
        h.frame().access_yaml()
    );

    // A marked-invalid input exposes its invalid state.
    let inv = h.get(&by::id("inv"));
    assert!(inv.invalid, "input is marked invalid");

    // Both states surface on the YAML too — the same vocabulary AccessKit emits.
    let yaml = h.frame().access_yaml();
    assert!(yaml.contains("[disabled]"), "yaml shows [disabled]:\n{}", yaml);
    assert!(yaml.contains("[invalid]"), "yaml shows [invalid]:\n{}", yaml);
}

/// `expanded` (ARIA `aria-expanded`) is modeled on `AccessNode` and projected
/// to AccessKit, so a disclosure header announces open/closed state rather than
/// being exposed as a plain button.

#[derive(Default)]
struct Disclosure {}

impl App for Disclosure {
    type Msg = Msg;

    fn update(&mut self, _msg: Msg) {}

    fn view(&self) -> Element<Msg> {
        accordion([
            accordion_item("Section A", text("Open body"))
                .open(true)
                .on_toggle(Msg::Set(String::new()))
                .id("disc"),
            accordion_item("Section B", text("Closed body")).open(false).id("closed"),
        ]).into()
    }
}

#[test]
fn expanded_state_reach_the_access_tree() {
    let h = Harness::new(Disclosure::default(), Theme::light(), (400, 200));

    // An open header is an expanded button; a closed one is not.
    let open = h.get(&by::id("disc"));
    assert!(
        open.expanded,
        "open disclosure header is marked expanded\n{}",
        h.frame().access_yaml()
    );
    assert!(
        matches!(open.semantics, Some(Semantics::Button)),
        "role is preserved\n{}",
        h.frame().access_yaml()
    );

    let closed = h.get(&by::id("closed"));
    assert!(!closed.expanded, "closed disclosure header is not expanded");

    // The state surfaces on the YAML too — the same vocabulary AccessKit emits.
    let yaml = h.frame().access_yaml();
    assert!(yaml.contains("[expanded]"), "yaml shows [expanded]:\n{}", yaml);
}

/// A tree branch that owns its expanded set (Elm-pure) must project
/// `aria-expanded` to the accessibility tree, so a screen reader can tell which
/// nodes have children expanded rather than exposing every node as an anonymous
/// button.
/// anonymous button.

#[derive(Default)]
struct TreeApp {
    expanded: Vec<String>,
}

impl App for TreeApp {
    type Msg = Msg;

    fn update(&mut self, _msg: Msg) {}

    fn view(&self) -> Element<Msg> {
        tree_view([TreeNode::new("root", "root").children([
            TreeNode::new("src", "src"),
            TreeNode::new("docs", "docs"),
        ])])
        .expanded(self.expanded.iter().cloned())
        .into()
    }
}

#[test]
fn tree_branch_states_reach_the_access_tree() {
    let h = Harness::new(
        TreeApp {
            expanded: vec!["root".to_owned()],
        },
        Theme::light(),
        (300, 200),
    );

    // An expanded branch header exposes aria-expanded; the collapsed one does not.
    let root = h.get(&by::id("tree-root"));
    assert!(
        root.expanded,
        "expanded tree branch is marked expanded\n{}",
        h.frame().access_yaml()
    );
    assert!(
        matches!(root.semantics, Some(Semantics::Button)),
        "role is preserved (not dropped from the tree)\n{}",
        h.frame().access_yaml()
    );

    let yaml = h.frame().access_yaml();
    assert!(yaml.contains("[expanded]"), "yaml shows [expanded]:\n{}", yaml);
}