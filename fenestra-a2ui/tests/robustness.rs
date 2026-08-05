//! Regression tests from the 2026-07-24 adversarial review of the A2UI
//! renderer: binding resolution gaps, protocol tolerance, and silent
//! write failures. Each test names the finding it pins.

use fenestra_a2ui::{A2uiMsg, A2uiSignal, Client, NoteKind, parse_stream};
use fenestra_core::{Element, Theme};

fn apply(stream: &str) -> Client {
    let msgs = parse_stream(stream).expect("stream parses");
    let mut client = Client::new();
    client.apply_all(&msgs).expect("stream applies");
    client
}

fn frame_tree(el: &Element<A2uiMsg>, size: (f32, f32)) -> String {
    let mut fonts = fenestra_core::Fonts::embedded();
    let mut state = fenestra_core::FrameState::new();
    let frame = fenestra_core::build_frame(el, &Theme::light(), &mut fonts, &mut state, size, 1.0);
    frame.debug_tree()
}

/// Finding 1: `Icon.name` is a dynamic value in the catalog — a bound name
/// must resolve against the data model, not stringify the binding object.
/// The official task-card example binds `/priorityIcon` = `"priority_high"`
/// (a Material name, honestly noted as outside the vendored Lucide set) —
/// the note must name the *resolved* value, never `{"path": …}`.
#[test]
fn bound_icon_names_resolve() {
    let stream = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/07_task-card.json"),
    )
    .expect("fixture exists");
    let client = apply(&stream);
    let rendered = client
        .single_surface()
        .expect("surface")
        .render(&Theme::light());
    assert!(
        !rendered
            .notes
            .iter()
            .any(|n| n.detail.contains("{\"path\"")),
        "the binding object leaked into rendering, notes: {:?}",
        rendered.notes
    );
    let tree = frame_tree(&rendered.element, (480.0, 640.0));
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::UnknownIcon && n.detail.contains("priority_high"))
            || tree.contains("priority_high"),
        "the bound icon name must resolve to priority_high; notes: {:?}",
        rendered.notes
    );
}

/// Finding 2: a template with an *absolute* path inside a collection scope
/// must resolve item scopes from that absolute path, not from a corrupted
/// `{scope}//abs` join.
#[test]
fn nested_template_absolute_paths_resolve() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","path":"/","value":{
            "rows": [{"title": "first"}, {"title": "second"}],
            "orders": [{"name": "alpha"}, {"name": "beta"}]
        }}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Column","children":{"componentId":"row-tpl","path":"/rows"}},
            {"id":"row-tpl","component":"Column","children":{"componentId":"order-tpl","path":"/orders"}},
            {"id":"order-tpl","component":"Text","text":{"path":"name"}}
        ]}}
    ]"#;
    let client = apply(stream);
    let rendered = client
        .single_surface()
        .expect("surface")
        .render(&Theme::light());
    let tree = frame_tree(&rendered.element, (480.0, 640.0));
    assert!(
        tree.contains("alpha") && tree.contains("beta"),
        "absolute template paths under a scope must resolve; notes: {:?}\ntree:\n{tree}",
        rendered.notes
    );
}

/// Finding 3: an action's message must carry the source component id, so
/// hosts can populate the client→server action message's required
/// `sourceComponentId`.
#[test]
fn event_actions_carry_the_source_component() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Column","children":["go"]},
            {"id":"go","component":"Button","child":"go-label",
             "action":{"event":{"name":"launch"}}},
            {"id":"go-label","component":"Text","text":"Go"}
        ]}}
    ]"#;
    let mut client = apply(stream);
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let msg = find_click(&rendered.element).expect("the button carries a click message");
    let A2uiMsg::Event {
        ref source_id,
        ref name,
        ..
    } = msg
    else {
        panic!("expected an event message, got {msg:?}");
    };
    assert_eq!(name, "launch");
    assert_eq!(source_id, "go", "the firing component's id must ride along");
    let signal = client
        .surface_mut("s")
        .expect("surface")
        .handle(msg.clone())
        .pop()
        .expect("events surface as signals");
    let A2uiSignal::Event { source_id, .. } = signal else {
        panic!("expected an event signal");
    };
    assert_eq!(source_id, "go");
}

fn find_click(el: &Element<A2uiMsg>) -> Option<A2uiMsg> {
    if let Some(msg) = el.click_msg() {
        return Some(msg.clone());
    }
    el.children_ref().iter().find_map(find_click)
}

/// Finding 4: toggling a literal-valued CheckBox stores a local edit; the
/// next render must *read it back* (the toggle message flips).
#[test]
fn literal_checkbox_toggles_take_effect() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"CheckBox","label":"Agree","value":false}
        ]}}
    ]"#;
    let mut client = apply(stream);
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let toggle = find_click(&rendered.element).expect("the checkbox toggles on click");
    let A2uiMsg::LocalEdit { ref value, .. } = toggle else {
        panic!("a literal checkbox stores a local edit, got {toggle:?}");
    };
    assert_eq!(
        value,
        &serde_json::Value::Bool(true),
        "unchecked toggles on"
    );
    assert!(
        client
            .surface_mut("s")
            .expect("surface")
            .handle(toggle)
            .is_empty()
    );
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let toggle = find_click(&rendered.element).expect("still toggleable");
    let A2uiMsg::LocalEdit { value, .. } = toggle else {
        panic!("expected a local edit");
    };
    assert_eq!(
        value,
        serde_json::Value::Bool(false),
        "after toggling on, the checkbox must render checked (next toggle turns it off)"
    );
}

/// Finding 5: an unknown message type (a newer protocol revision) is
/// skipped with a note on the surface it names — the stream around it
/// still applies and renders.
#[test]
fn unknown_message_types_are_skipped_not_fatal() {
    let stream = r##"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateTheme":{"surfaceId":"s","primaryColor":"#ff0000"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Text","text":"still here"}
        ]}}
    ]"##;
    let client = apply(stream);
    let surface = client.surface("s").expect("the stream still applies");
    let rendered = surface.render(&Theme::light());
    let tree = frame_tree(&rendered.element, (480.0, 640.0));
    assert!(
        tree.contains("still here"),
        "known messages around the unknown one apply"
    );
    assert!(
        surface
            .notes()
            .iter()
            .any(|n| n.kind == NoteKind::UnknownMessage && n.component_id == "updateTheme"),
        "the skipped message type is noted, got: {:?}",
        surface.notes()
    );
}

/// Finding 6: a *known* component whose body is malformed (Slider without
/// `max`) degrades to a placeholder with a note; its siblings and the rest
/// of the stream are untouched.
#[test]
fn malformed_known_component_degrades_not_fails() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Column","children":["ok","broken"]},
            {"id":"ok","component":"Text","text":"fine"},
            {"id":"broken","component":"Slider","value":3}
        ]}}
    ]"#;
    let client = apply(stream);
    let rendered = client
        .single_surface()
        .expect("the stream applies despite the malformed component")
        .render(&Theme::light());
    let tree = frame_tree(&rendered.element, (480.0, 640.0));
    assert!(tree.contains("fine"), "siblings render");
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::MalformedComponent && n.detail.contains("Slider")),
        "a *known* name that failed to parse is malformed, not unknown, got: {:?}",
        rendered.notes
    );
}

/// Finding 8: data-model array writes support RFC 6901 `-` (append), and a
/// write that cannot apply records a note instead of vanishing.
#[test]
fn array_appends_apply_and_bad_writes_are_noted() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","path":"/items","value":[1]}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","path":"/items/-","value":2}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","path":"/items/9","value":3}}
    ]"#;
    let client = apply(stream);
    let surface = client.surface("s").expect("surface");
    assert_eq!(
        surface.data().pointer("/items").unwrap(),
        &serde_json::json!([1, 2]),
        "`-` appends"
    );
    assert!(
        surface
            .notes()
            .iter()
            .any(|n| n.kind == NoteKind::RejectedWrite && n.component_id == "/items/9"),
        "the dropped out-of-range write is noted, got: {:?}",
        surface.notes()
    );
}

/// Finding 4 (mature form): every literal-valued input control stays
/// interactive through local edits — a ChoicePicker with a literal value
/// renders the locally edited selection after the user changes it.
#[test]
fn literal_choice_picker_reads_local_edits() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive",
             "options":[{"label":"Pro","value":"pro"},{"label":"Basic","value":"basic"}],
             "value":"pro"}
        ]}}
    ]"#;
    let mut client = apply(stream);
    let signal = client
        .surface_mut("s")
        .expect("surface")
        .handle(A2uiMsg::LocalEdit {
            key: "root".into(),
            value: serde_json::json!(["basic"]),
        });
    assert!(signal.is_empty(), "local edits are internal");
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let tree = frame_tree(&rendered.element, (480.0, 640.0));
    assert!(
        tree.contains("Basic"),
        "the locally edited selection must render; tree:\n{tree}"
    );
}

/// Finding 10: a mutually-exclusive ChoicePicker bound to a *string* value
/// selects the matching option, exactly like the literal-string form.
#[test]
fn bound_string_choice_picker_selects() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","path":"/plan","value":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive",
             "options":[{"label":"Pro","value":"pro"},{"label":"Basic","value":"basic"}],
             "value":{"path":"/plan"}}
        ]}}
    ]"#;
    let client = apply(stream);
    let rendered = client
        .single_surface()
        .expect("surface")
        .render(&Theme::light());
    let tree = frame_tree(&rendered.element, (480.0, 640.0));
    assert!(
        tree.contains("Basic"),
        "the bound string selection must show; tree:\n{tree}"
    );
}

// ── From the 2026-08-05 review: silent fidelity losses ────────────────────

/// `Slider::range` ignores anything that is not `max > min`, so an empty or
/// non-finite range used to leave the control quietly on its default
/// 0..=1 domain — a slider showing the wrong scale with nothing to say
/// about it.
#[test]
fn empty_slider_ranges_are_reported() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Slider","min":5,"max":5,"value":5}
        ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::InvalidValue && n.detail.contains("slider range")),
        "an unusable range must be reported, got: {:?}",
        rendered.notes
    );
}

/// A selection that names no existing option renders as *nothing selected*,
/// which is indistinguishable from an empty picker unless it is reported.
#[test]
fn selections_matching_no_option_are_reported() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive",
             "value":["xl"],"options":[
                {"label":"Small","value":"s"},
                {"label":"Large","value":"l"}
             ]}
        ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::InvalidValue && n.detail.contains("matches none")),
        "a selection outside the option set must be reported, got: {:?}",
        rendered.notes
    );
}

/// Remote assets render as placeholders because a deterministic render
/// never touches the network. The crate documented that as noted; it was
/// not. A surface of grey boxes must not report full fidelity.
///
/// All three asset components are checked: Video and AudioPlayer had no
/// test of any kind before this.
#[test]
fn remote_assets_report_their_placeholders() {
    for (component, extra) in [
        ("Image", ""),
        ("Video", ""),
        ("AudioPlayer", r#","description":"a podcast""#),
    ] {
        let stream = format!(
            r#"[
            {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
            {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
                {{"id":"root","component":"{component}","url":"https://example.com/a"{extra}}}
            ]}}}}
        ]"#
        );
        let rendered = apply(&stream)
            .surface("s")
            .expect("surface")
            .render(&Theme::light());
        assert!(
            rendered
                .notes
                .iter()
                .any(|n| n.kind == NoteKind::NetworkAsset),
            "a placeholder {component} must be reported, got: {:?}",
            rendered.notes
        );
        assert!(
            !fenestra_a2ui::any_broken(&rendered.notes),
            "…but a placeholder is approximate, not broken: {:?}",
            rendered.notes
        );
    }
}

/// `checks` and `validationRegexp` parse and then gate nothing. Silence
/// there is the difference between a validated form and one that merely
/// looks validated.
#[test]
fn unenforced_validation_is_reported() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"TextField","label":"Email","value":"",
             "validationRegexp":"^.+@.+$"}
        ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::Unsupported && n.detail.contains("validationRegexp")),
        "an unenforced validation rule must be reported, got: {:?}",
        rendered.notes
    );
}

// ── From the 2026-08-05 review of PR #19 ──────────────────────────────────

/// An obscured field renders in cleartext, so the pixels handed back by a
/// headless render contain the secret. That is not an inexactness, and
/// `any_broken` — the check the book tells people to put in CI — has to say
/// so.
#[test]
fn an_unmasked_secret_is_broken_not_approximate() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"pw":"hunter2"}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"TextField","label":"Password","variant":"obscured",
             "value":{"path":"/pw"}}
        ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::SecretExposed),
        "got: {:?}",
        rendered.notes
    );
    assert!(
        fenestra_a2ui::any_broken(&rendered.notes),
        "a surface whose pixels leak a secret must not pass any_broken: {:?}",
        rendered.notes
    );
}

/// A dialog nothing can open is broken, not approximate — the trigger's
/// interactive child takes every press.
#[test]
fn an_unopenable_modal_is_broken() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Modal","trigger":"card","content":"dlg"},
            {"id":"card","component":"Card","child":"btn"},
            {"id":"btn","component":"Button","child":"lbl","action":{"event":{"name":"go"}}},
            {"id":"lbl","component":"Text","text":"Press me"},
            {"id":"dlg","component":"Text","text":"body"}
        ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::Unreachable),
        "got: {:?}",
        rendered.notes
    );
    assert!(
        fenestra_a2ui::any_broken(&rendered.notes),
        "a dialog that cannot be opened must not pass any_broken: {:?}",
        rendered.notes
    );
}

/// A selection binding with nothing written yet is the ordinary empty
/// state, exactly as it is for a text field. Reporting it as broken made
/// every fresh form fail its own CI check.
#[test]
fn an_empty_selection_binding_is_not_a_fidelity_loss() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive",
             "value":{"path":"/choice"},"options":[
                {"label":"Small","value":"s"},{"label":"Large","value":"l"}
             ]}
        ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        rendered.notes.is_empty(),
        "a form that has not been filled in yet is not degraded: {:?}",
        rendered.notes
    );
}

/// Fields that parse and then go nowhere have to say so — a chips picker
/// rendering as a dropdown, an image ignoring its fit, a time-only input
/// asking for a date. Each renders something sensible, so each is
/// approximate rather than broken, but silence would claim the stream got
/// what it asked for.
#[test]
fn parsed_but_unhonored_fields_are_reported() {
    let cases: [(&str, &str); 3] = [
        (
            r#"{"id":"root","component":"Image","url":"https://e.com/a.png","fit":"cover"}"#,
            "fit",
        ),
        (
            r#"{"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive",
                "value":[],"displayStyle":"chips","filterable":true,
                "options":[{"label":"A","value":"a"}]}"#,
            "displayStyle",
        ),
        (
            r#"{"id":"root","component":"DateTimeInput","value":"2026-01-01",
                "enableDate":false,"enableTime":true}"#,
            "enableDate/enableTime",
        ),
    ];
    for (component, field) in cases {
        let stream = format!(
            r#"[
            {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
            {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[{component}]}}}}
        ]"#
        );
        let rendered = apply(&stream)
            .surface("s")
            .expect("surface")
            .render(&Theme::light());
        assert!(
            rendered
                .notes
                .iter()
                .any(|n| n.kind == NoteKind::Unsupported && n.detail.contains(field)),
            "{field} must be reported, got: {:?}",
            rendered.notes
        );
    }
}

/// An `openUrl` with nothing to open must do nothing, rather than handing
/// the host an empty URL it never asked for.
#[test]
fn open_url_with_an_unresolved_argument_does_nothing() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Button","child":"lbl",
             "action":{"functionCall":{"call":"openUrl","args":{"url":{"path":"/link"}}}}},
            {"id":"lbl","component":"Text","text":"Visit"}
        ]}}
    ]"#;
    let mut client = apply(stream);
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let click = find_click(&rendered.element).expect("the button is clickable");
    assert!(
        matches!(click, A2uiMsg::Ignored),
        "an openUrl with no URL must not reach the host, got {click:?}"
    );
    assert!(
        rendered.notes.iter().any(|n| n.detail.contains("openUrl")),
        "and it must say why, got: {:?}",
        rendered.notes
    );
    assert!(
        client
            .surface_mut("s")
            .expect("surface")
            .handle(click)
            .is_empty(),
        "no signal reaches the host"
    );
}

/// The repaired slider range has to survive the same test that rejected the
/// original: at the top of the f64 range `min + 1.0` rounds straight back
/// to `min`, and the note would then describe a range the widget does not
/// have.
#[test]
fn an_unrepairable_slider_range_falls_back_honestly() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Slider","min":1e308,"max":1e308,"value":1e308}
        ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let note = rendered
        .notes
        .iter()
        .find(|n| n.kind == NoteKind::InvalidValue && n.detail.contains("slider range"))
        .unwrap_or_else(|| panic!("expected a range note, got: {:?}", rendered.notes));
    assert!(
        note.detail.contains("using 0..=1"),
        "the note must describe the range the widget actually got, got: {}",
        note.detail
    );
}

/// The write path is driven by the user, so a control bound to an
/// unwritable pointer produced one note per keystroke and grew without
/// bound — every `render_a2ui` response carrying thousands of copies of
/// one problem.
#[test]
fn repeated_rejected_writes_record_one_note() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"items":[1]}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Text","text":"x"}
        ]}}
    ]"#;
    let mut client = apply(stream);
    let surface = client.surface_mut("s").expect("surface");
    for _ in 0..500 {
        surface.handle(A2uiMsg::SetString {
            path: "/items/9".into(),
            value: "nope".into(),
        });
    }
    assert_eq!(
        surface.notes().len(),
        1,
        "the same rejected write said 500 times is one problem, got: {:?}",
        surface.notes()
    );
}
