//! Conformance against the official A2UI v0.9 gallery examples (vendored
//! under `fixtures/`, see NOTICE): every stream parses, folds, and renders
//! headlessly; representative surfaces are pinned as goldens; bindings,
//! templates, actions, and two-way writes behave per the protocol.

use std::path::PathBuf;

use fenestra_a2ui::{A2uiMsg, A2uiSignal, Client, NoteKind, NoteSeverity, parse_stream};
use fenestra_core::{Theme, by};
use fenestra_shell::{render_element, testing::assert_png_snapshot};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(path).expect("fixture exists")
}

fn client_for(name: &str) -> Client {
    let msgs = parse_stream(&fixture(name)).expect("fixture parses");
    let mut client = Client::new();
    client.apply_all(&msgs).expect("stream applies");
    client
}

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

/// Every vendored official example parses, applies, and renders without a
/// structural failure.
#[test]
fn every_official_example_renders() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).expect("fixtures dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let msgs = parse_stream(&std::fs::read_to_string(&path).expect("read"))
            .unwrap_or_else(|e| panic!("{name}: parse failed: {e}"));
        let mut client = Client::new();
        client
            .apply_all(&msgs)
            .unwrap_or_else(|e| panic!("{name}: apply failed: {e}"));
        let surface = client.single_surface().expect("one surface per example");
        let rendered = surface.render(&Theme::light());
        let img = render_element(rendered.element, &Theme::light(), (480, 640));
        assert!(img.width() > 0, "{name}: rendered");
        count += 1;
    }
    assert!(count >= 10, "the fixture corpus is present ({count})");
}

/// `any_broken` over the whole official corpus, with a named exemption
/// for every remaining known gap. This is the crate's fidelity-or-report
/// contract made testable: the suite is green only when *every* broken
/// note in *every* official example is one that someone has read and
/// decided is acceptable — and the decision, with its reason, lives in
/// the table below instead of in a chat. Both failure modes are
/// intentional:
///
/// - A broken note appears on a fixture or kind the table does not name —
///   a mapping regression, or a new gap that needs a decision.
/// - An entry stops firing — the gap it documented is closed (masking
///   landed, a function implemented) and the entry is stale: remove it.
#[test]
fn official_examples_hold_the_fidelity_contract() {
    /// Why a broken note on this fixture is allowed to stay broken —
    /// `None` means it is not.
    fn exemption_for(fixture: &str, kind: NoteKind) -> Option<&'static str> {
        match (fixture, kind) {
            // The kit has no password masking, and the crate refuses to
            // paint a value the stream asked to obscure: the field renders
            // visible and the note says so. Closes when masking lands.
            ("00_simple-login-form.json", NoteKind::SecretExposed) => {
                Some("no password masking in the kit yet; the note is the honest state")
            }
            // Nested function calls — `formatString` inside
            // `formatString` — are a documented gap in `functions.rs`.
            ("05_product-card.json", NoteKind::UnimplementedFunction) => {
                Some("nested function calls are not implemented yet")
            }
            // `priority_high` is not in the spec's basic-catalog icon
            // enum; the fixture is off-spec, and a labeled placeholder is
            // the correct answer.
            ("07_task-card.json", NoteKind::UnknownIcon) => {
                Some("priority_high is not a spec icon name")
            }
            _ => None,
        }
    }

    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).expect("fixtures dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let rendered = client_for(&name)
            .single_surface()
            .unwrap_or_else(|| panic!("{name}: one surface per example"))
            .render(&Theme::light());
        checked += 1;
        for note in rendered
            .notes
            .iter()
            .filter(|n| n.severity() == NoteSeverity::Broken)
        {
            let reason = exemption_for(&name, note.kind).unwrap_or_else(|| {
                panic!(
                    "{name}: broken note with no exemption — a regression, or a \
                     gap that needs a decision: {note}"
                )
            });
            let _ = reason; // the exemption exists; its text is the decision
        }
    }
    assert!(checked >= 10, "the fixture corpus is present ({checked})");

    // The other direction: every exemption must still be firing, or it is
    // stale. Re-render the three exempted fixtures and confirm the notes
    // they document are still there.
    for (name, kind) in [
        ("00_simple-login-form.json", NoteKind::SecretExposed),
        ("05_product-card.json", NoteKind::UnimplementedFunction),
        ("07_task-card.json", NoteKind::UnknownIcon),
    ] {
        let rendered = client_for(name)
            .single_surface()
            .expect("one surface per example")
            .render(&Theme::light());
        assert!(
            rendered
                .notes
                .iter()
                .any(|n| n.kind == kind && n.severity() == NoteSeverity::Broken),
            "{name}: exemption for {kind:?} no longer fires — the gap is \
             closed, remove the entry"
        );
    }
}

/// The weather example exercises templated children and formatString —
/// the resolved values must reach the accessibility tree.
#[test]
fn weather_bindings_and_templates_resolve() {
    let client = client_for("04_weather-current.json");
    let surface = client.single_surface().expect("surface");
    let rendered = surface.render(&Theme::light());
    let frame = fenestra_shell::render_element(rendered.element, &Theme::light(), (480, 640));
    drop(frame);
    // Structural check through a fresh render (render is pure).
    let rendered = surface.render(&Theme::light());
    let mut fonts = fenestra_core::Fonts::embedded();
    let mut state = fenestra_core::FrameState::new();
    let frame = fenestra_core::build_frame(
        &rendered.element,
        &Theme::light(),
        &mut fonts,
        &mut state,
        (480.0, 640.0),
        1.0,
    );
    let tree = frame.debug_tree();
    // The data model in the fixture carries the location and temps that
    // only reach the tree through bindings + ${…} interpolation.
    assert!(
        tree.contains("San Francisco") || tree.contains("72"),
        "resolved data must appear, got tree:\n{tree}"
    );
}

/// Goldens for representative surfaces (login form: inputs; product card:
/// functions + layout).
#[test]
fn login_form_golden() {
    let client = client_for("00_simple-login-form.json");
    let rendered = client
        .single_surface()
        .expect("surface")
        .render(&Theme::light());
    let img = render_element(rendered.element, &Theme::light(), (420, 320));
    assert_png_snapshot(snapshot_dir(), "a2ui_login_form", &img);
}

#[test]
fn product_card_golden() {
    let client = client_for("05_product-card.json");
    let rendered = client
        .single_surface()
        .expect("surface")
        .render(&Theme::light());
    let img = render_element(rendered.element, &Theme::light(), (420, 560));
    assert_png_snapshot(snapshot_dir(), "a2ui_product_card", &img);
}

/// Two-way binding: an input's SetString writes into the data model, and
/// the next render reflects it.
#[test]
fn two_way_binding_writes_the_data_model() {
    let mut client = client_for("00_simple-login-form.json");
    let id = client.surfaces().next().expect("surface").id().to_owned();
    let surface = client.surface_mut(&id).expect("surface");
    let signal = surface.handle(A2uiMsg::SetString {
        path: "/username".into(),
        value: "ada".into(),
    });
    assert!(signal.is_empty(), "binding writes are internal");
    assert_eq!(surface.data().pointer("/username").unwrap(), "ada");
}

/// A button action resolves its context against the data model and, with
/// sendDataModel, attaches the model to the signal.
#[test]
fn actions_surface_as_signals_with_the_data_model() {
    let mut client = client_for("00_simple-login-form.json");
    let id = client.surfaces().next().expect("surface").id().to_owned();
    let surface = client.surface_mut(&id).expect("surface");
    assert!(
        surface
            .handle(A2uiMsg::SetString {
                path: "/username".into(),
                value: "ada".into(),
            })
            .is_empty(),
        "a data-model write is not a host-bound signal"
    );
    let signal = surface.handle(A2uiMsg::Event {
        name: "login".into(),
        context: serde_json::Value::Null,
        source_id: "submit_button".into(),
    });
    match signal.into_iter().next() {
        Some(A2uiSignal::Event {
            name,
            data_model,
            source_id,
            ..
        }) => {
            assert_eq!(name, "login");
            assert_eq!(
                source_id, "submit_button",
                "the firing component rides along"
            );
            let model = data_model.expect("sendDataModel is true in the fixture");
            assert_eq!(model.pointer("/username").unwrap(), "ada");
        }
        other => panic!("expected an event signal, got {other:?}"),
    }
    // The client→server action message carries the ids the spec requires.
    let surface = client.surface(&id).expect("surface");
    let msg = surface.action_message(
        "login",
        "submit_button",
        &serde_json::Value::Null,
        "2026-07-24T00:00:00Z",
    );
    assert_eq!(msg["surfaceId"], id.as_str());
    assert_eq!(msg["name"], "login");
}

/// updateDataModel changes what renders (the protocol's live-update loop).
#[test]
fn data_model_updates_rerender() {
    let mut client = client_for("00_simple-login-form.json");
    let id = client.surfaces().next().expect("surface").id().to_owned();
    let update = format!(
        r#"[{{"version":"v0.9","updateDataModel":{{"surfaceId":"{id}","path":"/username","value":"grace"}}}}]"#
    );
    client
        .apply_all(&parse_stream(&update).expect("parses"))
        .expect("applies");
    let rendered = client
        .surface(&id)
        .expect("surface")
        .render(&Theme::light());
    let mut fonts = fenestra_core::Fonts::embedded();
    let mut state = fenestra_core::FrameState::new();
    let frame = fenestra_core::build_frame(
        &rendered.element,
        &Theme::light(),
        &mut fonts,
        &mut state,
        (420.0, 320.0),
        1.0,
    );
    assert!(
        frame.query(&by::value("grace")).is_some(),
        "the updated value must render"
    );
}

/// A reference cycle degrades to a pointed note, never a stack overflow.
#[test]
fn reference_cycles_degrade_with_a_note() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Column","children":["a"]},
            {"id":"a","component":"Column","children":["root"]}
        ]}}
    ]"#;
    let mut client = Client::new();
    client
        .apply_all(&parse_stream(stream).expect("parses"))
        .expect("applies");
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::ReferenceCycle),
        "cycle must be reported, got: {:?}",
        rendered.notes
    );
}

/// Unknown components degrade to labeled placeholders with a note.
#[test]
fn unknown_components_degrade_with_a_note() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"custom"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"FancyGauge","value":42}
        ]}}
    ]"#;
    let mut client = Client::new();
    client
        .apply_all(&parse_stream(stream).expect("parses"))
        .expect("applies");
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::UnknownComponent && n.detail.contains("FancyGauge")),
        "an out-of-catalog name is unknown, not malformed, got: {:?}",
        rendered.notes
    );
}

fn rendered_tree(el: &fenestra_core::Element<A2uiMsg>, size: (f32, f32)) -> String {
    let mut fonts = fenestra_core::Fonts::embedded();
    let mut state = fenestra_core::FrameState::new();
    let frame = fenestra_core::build_frame(el, &Theme::light(), &mut fonts, &mut state, size, 1.0);
    frame.debug_tree()
}

fn client_from(stream: &str) -> Client {
    let mut client = Client::new();
    client
        .apply_all(&parse_stream(stream).expect("stream parses"))
        .expect("stream applies");
    client
}

/// `pluralize` resolves the `zero` category when the stream provides it.
/// Previously the argument was parsed and silently dropped, so a stream
/// that said `zero: "no items"` rendered "0 items" in the `other` form.
#[test]
fn pluralize_resolves_the_zero_category() {
    // Plain template (no format! brace-escaping) with a count token.
    let template = r#"[
    {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
    {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"count":@COUNT@}}},
    {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Text","text":{
            "call":"pluralize",
            "args":{"value":{"path":"/count"},
                     "zero":"no items",
                     "one":"1 item",
                     "other":"many items"}}
        }
    ]}}
]
"#;
    let stream = |count: u8| template.replace("@COUNT@", &count.to_string());
    for (count, want) in [(0, "no items"), (1, "1 item"), (3, "many items")] {
        let client = client_from(&stream(count));
        let rendered = client
            .surface("s")
            .expect("surface")
            .render(&Theme::light());
        assert!(
            rendered.notes.is_empty(),
            "count {count}: a faithful pluralize records nothing, got: {:?}",
            rendered.notes
        );
        let tree = rendered_tree(&rendered.element, (420.0, 320.0));
        assert!(
            tree.contains(want),
            "count {count}: wanted {want:?}, tree:\n{tree}"
        );
    }
}

/// `\${` is the spec's escape for a literal `${` in a `formatString`
/// template. It renders verbatim and records no note — before the escape
/// existed, a literal `${` was parsed as an expression and produced a
/// spurious unresolved-binding note.
#[test]
fn escaped_dollar_brace_renders_literal() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"price":5}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Text","text":{
                "call":"formatString",
                "args":{"value":"Cost: ${/price} — literal \\${off} only"}
            }}
        ]}}
    ]"#;
    let client = client_from(stream);
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered.notes.is_empty(),
        "the escape must not be reported, got: {:?}",
        rendered.notes
    );
    let tree = rendered_tree(&rendered.element, (420.0, 320.0));
    assert!(
        tree.contains("Cost: 5") && tree.contains("literal ${off} only"),
        "resolved value and escaped literal must both render, tree:\n{tree}"
    );
}

/// Deleting an array element sets the slot to null and preserves the
/// length (the protocol's `undefined` is JSON's null). The old removal
/// shifted every later index, silently repointing every later binding.
#[test]
fn array_delete_nulls_the_slot_and_preserves_length() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"items":["a","b","c"]}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Column","children":["t1","t2"]},
            {"id":"t1","component":"Text","text":{"path":"/items/1"}},
            {"id":"t2","component":"Text","text":{"path":"/items/2"}}
        ]}}
    ]"#;
    let mut client = client_from(stream);
    let del = r#"[{"version":"v0.9","updateDataModel":{"surfaceId":"s","path":"/items/1"}}]"#;
    client
        .apply_all(&parse_stream(del).expect("parses"))
        .expect("applies");
    let surface = client.surface("s").expect("surface");
    let items = surface
        .data()
        .pointer("/items")
        .expect("items")
        .as_array()
        .expect("still an array");
    assert_eq!(items.len(), 3, "the length is preserved");
    assert!(
        items[1].is_null(),
        "the deleted slot is nulled, not removed"
    );
    assert_eq!(items[2].as_str(), Some("c"), "later indices must not shift");
    let rendered = surface.render(&Theme::light());
    let tree = rendered_tree(&rendered.element, (420.0, 320.0));
    assert!(
        tree.contains("c"),
        "the binding at /items/2 must still resolve, tree:\n{tree}"
    );
}
