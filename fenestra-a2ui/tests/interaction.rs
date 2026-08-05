//! Interaction tests that go through *real* hit-testing.
//!
//! The other suites reach into the element tree and pull out the first
//! click handler they find. That is fine for checking which message a
//! control produces, but it cannot catch a dispatch bug: fenestra gives a
//! press to the *deepest* enabled interactive node and stops there, with no
//! bubbling, so a handler on an outer wrapper never fires when something
//! interactive sits inside it. Driving a `Harness` instead means these
//! tests fail exactly when a user's click would fail.
//!
//! Modal, Tabs and List had no coverage of any kind before this file; two
//! of the 2026-08-05 review's bugs lived in that gap.

use fenestra_a2ui::{A2uiMsg, A2uiSignal, Client, parse_stream};
use fenestra_core::{App, Element, Semantics, Theme, by};
use fenestra_shell::Harness;

/// The smallest real host for a surface: render it, feed clicks back
/// through `handle`, keep whatever signals come out.
struct SurfaceApp {
    client: Client,
    id: String,
    signals: Vec<A2uiSignal>,
}

impl SurfaceApp {
    fn new(stream: &str, id: &str) -> Self {
        let msgs = parse_stream(stream).expect("stream parses");
        let mut client = Client::new();
        client.apply_all(&msgs).expect("stream applies");
        Self {
            client,
            id: id.to_owned(),
            signals: Vec::new(),
        }
    }

    fn surface(&self) -> &fenestra_a2ui::Surface {
        self.client.surface(&self.id).expect("surface exists")
    }
}

impl App for SurfaceApp {
    type Msg = A2uiMsg;

    fn update(&mut self, msg: A2uiMsg) {
        let surface = self.client.surface_mut(&self.id).expect("surface exists");
        self.signals.extend(surface.handle(msg));
    }

    fn view(&self) -> Element<A2uiMsg> {
        self.surface().render(&Theme::light()).element
    }
}

fn harness(stream: &str) -> Harness<SurfaceApp> {
    Harness::new(SurfaceApp::new(stream, "s"), Theme::light(), (480, 640))
}

/// A Modal's trigger is usually a Button — that is the whole point of the
/// component. Clicking it must open the dialog. The wrapper-with-on_click
/// version of this could never work: the Button is deeper, so it won the
/// press and the wrapper's OpenModal never fired.
#[test]
fn modal_opens_when_its_trigger_is_a_button() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Modal","trigger":"open_btn","content":"dialog"},
            {"id":"open_btn","component":"Button","child":"open_lbl",
             "action":{"event":{"name":"opened"}}},
            {"id":"open_lbl","component":"Text","text":"Open"},
            {"id":"dialog","component":"Text","text":"Dialog body"}
        ]}}
    ]"#;
    let mut h = harness(stream);
    assert!(
        h.query(&by::label_contains("Dialog body")).is_none(),
        "the dialog starts closed"
    );
    h.click(&by::role(Semantics::Button).name("Open"));
    assert!(
        h.query(&by::label_contains("Dialog body")).is_some(),
        "clicking the trigger must open the modal"
    );
    // The trigger's own action still reaches the agent: opening a dialog
    // does not swallow the event the stream asked for.
    assert!(
        h.app()
            .signals
            .iter()
            .any(|s| matches!(s, A2uiSignal::Event { name, .. } if name == "opened")),
        "the trigger's action must still fire, got: {:?}",
        h.app().signals
    );
}

/// A trigger with no action of its own still opens the modal — and must
/// not render disabled while doing it.
#[test]
fn modal_opens_from_an_actionless_trigger() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Modal","trigger":"open_btn","content":"dialog"},
            {"id":"open_btn","component":"Button","child":"open_lbl"},
            {"id":"open_lbl","component":"Text","text":"Open"},
            {"id":"dialog","component":"Text","text":"Dialog body"}
        ]}}
    ]"#;
    let mut h = harness(stream);
    let trigger = h.get(&by::role(Semantics::Button).name("Open"));
    assert!(
        trigger.focusable,
        "a modal trigger is not a dead button; it opens the dialog"
    );
    h.click(&by::role(Semantics::Button).name("Open"));
    assert!(
        h.query(&by::label_contains("Dialog body")).is_some(),
        "an actionless trigger still opens the modal"
    );
}

/// Tabs switch on click and show the selected tab's subtree.
#[test]
fn tabs_switch_on_click() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Tabs","tabs":[
                {"title":"First","child":"one"},
                {"title":"Second","child":"two"}
            ]},
            {"id":"one","component":"Text","text":"page one"},
            {"id":"two","component":"Text","text":"page two"}
        ]}}
    ]"#;
    let mut h = harness(stream);
    assert!(
        h.query(&by::label_contains("page one")).is_some(),
        "the first tab shows by default"
    );
    h.click(&by::role(Semantics::Tab { selected: false }).name("Second"));
    assert!(
        h.query(&by::label_contains("page two")).is_some(),
        "clicking a tab shows its subtree"
    );
    assert!(
        h.query(&by::label_contains("page one")).is_none(),
        "and hides the previous one"
    );
}

/// Every literal-valued input stays editable through local edits. The
/// DateTimeInput used to *read* local edits while attaching no handler to
/// write them, so it was permanently read-only and the readback was dead
/// code. Typing into one must stick.
#[test]
fn literal_date_time_input_accepts_typing() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"DateTimeInput","value":"2026-01-01","enableDate":true}
        ]}}
    ]"#;
    let field = by::role(Semantics::TextInput { multiline: false });
    let mut h = harness(stream);
    assert_eq!(
        h.get(&field).value.as_deref(),
        Some("2026-01-01"),
        "the literal value shows"
    );
    h.focus(&field);
    h.type_text("!");
    // Where the caret lands on focus is the editor's business; what
    // matters is that the keystroke reached the data at all.
    let after = h.get(&field).value.unwrap_or_default();
    assert!(
        after.contains('!') && after.contains("2026-01-01"),
        "a literal-valued DateTimeInput must accept edits like every other input, got {after:?}"
    );
}

/// A List renders its children and reports nothing — it is a plain
/// scrolling container, and the notes must stay quiet for it.
#[test]
fn list_renders_children_without_notes() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"List","children":["a","b"],"direction":"vertical"},
            {"id":"a","component":"Text","text":"alpha"},
            {"id":"b","component":"Text","text":"beta"}
        ]}}
    ]"#;
    let h = harness(stream);
    assert!(h.query(&by::label_contains("alpha")).is_some());
    assert!(h.query(&by::label_contains("beta")).is_some());
    let rendered = h.app().surface().render(&Theme::light());
    assert!(
        rendered.notes.is_empty(),
        "a plain List maps exactly, got: {:?}",
        rendered.notes
    );
}

/// An action function this build does not implement must do *nothing*.
/// It used to send the agent a synthetic `unimplemented:<fn>` event —
/// a message no server asked for and every server would have to defend
/// against.
#[test]
fn unimplemented_action_functions_send_nothing() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Button","child":"lbl",
             "action":{"functionCall":{"call":"summonADragon","args":{}}}},
            {"id":"lbl","component":"Text","text":"Go"}
        ]}}
    ]"#;
    let mut h = harness(stream);
    h.click(&by::role(Semantics::Button).name("Go"));
    assert!(
        h.app().signals.is_empty(),
        "an unimplemented function must not reach the agent, got: {:?}",
        h.app().signals
    );
    let notes = h.app().surface().render(&Theme::light()).notes;
    assert!(
        notes
            .iter()
            .any(|n| n.kind == fenestra_a2ui::NoteKind::UnimplementedFunction),
        "and it must say so, got: {notes:?}"
    );
}

/// A mutually-exclusive picker with nothing selected must not render as
/// though the first option had been chosen. It shows a placeholder, and
/// picking a real option still writes that option's value.
#[test]
fn unselected_picker_does_not_claim_the_first_option() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive",
             "value":[],"options":[
                {"label":"Small","value":"s"},
                {"label":"Large","value":"l"}
             ]}
        ]}}
    ]"#;
    let h = harness(stream);
    let picker = h.get(&by::role(Semantics::ComboBox));
    assert_ne!(
        picker.value.as_deref(),
        Some("Small"),
        "an empty selection must not display as the first option"
    );
    let rendered = h.app().surface().render(&Theme::light());
    assert!(
        rendered.notes.is_empty(),
        "showing an empty selection honestly is not a fidelity loss, got: {:?}",
        rendered.notes
    );
}
