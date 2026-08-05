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
