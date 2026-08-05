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
    // A combobox carries its current option as its accessible *name*; it
    // never sets `value`, so asserting on that would pass vacuously.
    let picker = h.get(&by::role(Semantics::ComboBox));
    assert_eq!(
        picker.label.as_deref(),
        Some("—"),
        "an empty selection must show the placeholder, not the first option"
    );
    let rendered = h.app().surface().render(&Theme::light());
    assert!(
        rendered.notes.is_empty(),
        "showing an empty selection honestly is not a fidelity loss, got: {:?}",
        rendered.notes
    );
}

/// Picking from a picker that was showing the empty placeholder must write
/// the option the user actually chose.
///
/// The placeholder occupies index 0 while it is present, so every real
/// option shifts along by one. Nothing exercised that arithmetic through a
/// real selection — the other picker tests only read the static render, so
/// dropping the shift would have left them all green while the picker
/// wrote the wrong value.
#[test]
fn picking_from_the_placeholder_state_writes_the_chosen_option() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive",
             "value":{"path":"/choice"},"options":[
                {"label":"Small","value":"s"},
                {"label":"Large","value":"l"}
             ]}
        ]}}
    ]"#;
    let mut h = harness(stream);
    h.click(&by::role(Semantics::ComboBox));
    h.click(&by::label("Large"));
    assert_eq!(
        h.app().surface().data().pointer("/choice"),
        Some(&serde_json::json!(["l"])),
        "choosing the second option must write that option, not its neighbour"
    );
}

/// A pixel-level pin on the modal trigger.
///
/// The access tree said `focusable: true` while the button was still
/// *painted* as a dead control — the kit bakes disabled styling (a themed
/// label color, opacity on solid variants) into the widget when it is
/// built, so clearing `Element::disabled` afterwards fixed hit-testing and
/// nothing else. Only pixels catch that, which is why this golden exists.
#[test]
fn modal_trigger_golden() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Column","children":["actionless","acting"]},
            {"id":"actionless","component":"Modal","trigger":"t1","content":"dialog"},
            {"id":"t1","component":"Button","child":"l1","variant":"primary"},
            {"id":"l1","component":"Text","text":"Opens a dialog"},
            {"id":"acting","component":"Button","child":"l2","variant":"primary",
             "action":{"event":{"name":"go"}}},
            {"id":"l2","component":"Text","text":"Has an action"},
            {"id":"dialog","component":"Text","text":"body"}
        ]}}
    ]"#;
    let mut h = harness(stream);
    let image = h.render();
    fenestra_shell::testing::assert_png_snapshot(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots"),
        "modal_trigger",
        &image,
    );
}

/// UI state has to know *which* expansion it belongs to.
///
/// A template renders the same component once per data item, so keying a
/// Modal's open flag by component id alone made every copy share it:
/// clicking one row's trigger opened every row's dialog at once, each
/// showing its own item's content.
#[test]
fn a_templated_modal_opens_only_its_own_row() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"rows":[
            {"label":"Row A","detail":"Detail A"},
            {"label":"Row B","detail":"Detail B"}
        ]}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Column",
             "children":{"componentId":"row","path":"/rows"}},
            {"id":"row","component":"Modal","trigger":"trg","content":"body"},
            {"id":"trg","component":"Button","child":"trglbl"},
            {"id":"trglbl","component":"Text","text":{"path":"label"}},
            {"id":"body","component":"Text","text":{"path":"detail"}}
        ]}}
    ]"#;
    let mut h = harness(stream);
    h.click(&by::role(Semantics::Button).name("Row B"));
    assert!(
        h.query(&by::label_contains("Detail B")).is_some(),
        "the row that was clicked opens"
    );
    assert!(
        h.query(&by::label_contains("Detail A")).is_none(),
        "and no other row does"
    );
}

/// The same defect, on the input side: two expansions of one literal-valued
/// CheckBox shared a single local edit, so ticking one ticked both.
#[test]
fn templated_local_edits_do_not_bleed_between_rows() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"tasks":[
            {"name":"First"},{"name":"Second"}
        ]}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Column",
             "children":{"componentId":"task","path":"/tasks"}},
            {"id":"task","component":"CheckBox","label":{"path":"name"},"value":false}
        ]}}
    ]"#;
    let mut h = harness(stream);
    h.click(
        &by::role(Semantics::Checkbox {
            checked: false,
            mixed: false,
        })
        .name("First"),
    );
    // `by::role` matches the variant, not its payload, so read the state
    // off each node rather than querying for it.
    let ticked: Vec<String> = h
        .get_all(&by::role(Semantics::Checkbox {
            checked: false,
            mixed: false,
        }))
        .into_iter()
        .filter(|n| matches!(n.semantics, Some(Semantics::Checkbox { checked: true, .. })))
        .filter_map(|n| n.label)
        .collect();
    assert_eq!(
        ticked,
        vec!["First".to_owned()],
        "ticking one row must not tick its siblings"
    );
}

/// A picker that has been filled in must still be clearable. The
/// placeholder used to appear only while the selection was empty, so once
/// the user chose something there was no option left that wrote the empty
/// state — the data model could be filled but never emptied through the UI.
#[test]
fn a_filled_picker_can_be_cleared_again() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"choice":["l"]}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive",
             "value":{"path":"/choice"},"options":[
                {"label":"Small","value":"s"},{"label":"Large","value":"l"}
             ]}
        ]}}
    ]"#;
    let mut h = harness(stream);
    assert_eq!(
        h.get(&by::role(Semantics::ComboBox)).label.as_deref(),
        Some("Large"),
        "the bound selection shows"
    );
    h.click(&by::role(Semantics::ComboBox));
    h.click(&by::label("—"));
    assert_eq!(
        h.app().surface().data().pointer("/choice"),
        Some(&serde_json::json!([])),
        "choosing the placeholder must clear the selection"
    );
}
