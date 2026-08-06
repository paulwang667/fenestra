//! Client-side validation: the basic catalog's `checks` and the eight
//! boolean functions that go in them.
//!
//! Before this, `checks` and `validationRegexp` parsed and recorded "not
//! enforced yet" — so a stream that said "you must accept the terms before
//! submitting" rendered a submit button that submitted.

use fenestra_a2ui::{A2uiMsg, Client, NoteKind, any_broken, parse_stream};
use fenestra_core::{Element, Theme, by};

fn apply(stream: &str) -> Client {
    let msgs = parse_stream(stream).expect("stream parses");
    let mut client = Client::new();
    client.apply_all(&msgs).expect("stream applies");
    client
}

/// A surface with one checked control over a data model.
fn surface_with(data: &str, component: &str) -> fenestra_a2ui::Rendered {
    let stream = format!(
        r#"[
          {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
          {{"version":"v0.9","updateDataModel":{{"surfaceId":"s","value":{data}}}}},
          {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
            {{"id":"root","component":"Column","children":["subject"]}},
            {component}
          ]}}}}
        ]"#
    );
    apply(&stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light())
}

/// Does any element in the tree carry a click handler?
fn has_click(el: &Element<A2uiMsg>) -> bool {
    el.click_msg().is_some() || el.children_ref().iter().any(has_click)
}

fn shows_text(el: &Element<A2uiMsg>, needle: &str) -> bool {
    let mut fonts = fenestra_core::Fonts::embedded();
    let mut state = fenestra_core::FrameState::new();
    let frame = fenestra_core::build_frame(
        el,
        &Theme::light(),
        &mut fonts,
        &mut state,
        (420.0, 320.0),
        1.0,
    );
    frame.debug_tree().contains(needle)
}

const TEXT_FIELD: &str = r#"{"id":"subject","component":"TextField","label":"Email",
    "value":{"path":"/email"},
    "checks":[{"condition":{"call":"email","args":{"value":{"path":"/email"}}},
               "message":"Enter a work email."}]}"#;

#[test]
fn a_failing_check_shows_its_message() {
    let rendered = surface_with(r#"{"email":"not-an-email"}"#, TEXT_FIELD);
    assert!(
        shows_text(&rendered.element, "Enter a work email."),
        "the rule's own message must reach the user"
    );
}

#[test]
fn a_passing_check_is_silent_and_shows_nothing() {
    let rendered = surface_with(r#"{"email":"ada@example.com"}"#, TEXT_FIELD);
    assert!(!shows_text(&rendered.element, "Enter a work email."));
    assert!(
        rendered.notes.is_empty(),
        "an enforced check is not a fidelity gap, got: {:?}",
        rendered.notes
    );
}

/// The regression that motivated the feature: `checks` used to record
/// "parsed but does not gate this control yet" on every checked control.
#[test]
fn checks_no_longer_report_themselves_unenforced() {
    let rendered = surface_with(r#"{"email":"ada@example.com"}"#, TEXT_FIELD);
    assert!(
        !rendered
            .notes
            .iter()
            .any(|n| n.detail.contains("does not gate this control yet")),
        "got: {:?}",
        rendered.notes
    );
}

/// The point of a check on a Button: it must not carry out its action.
#[test]
fn a_failing_check_blocks_the_button() {
    let component = r#"{"id":"subject","component":"Button","child":"lbl",
        "action":{"event":{"name":"submit"}},
        "checks":[{"condition":{"call":"required","args":{"value":{"path":"/terms"}}},
                   "message":"Accept the terms first."}]},
       {"id":"lbl","component":"Text","text":"Submit"}"#;

    let blocked = surface_with(r#"{"terms":false}"#, component);
    assert!(
        !has_click(&blocked.element),
        "a button whose check fails must not carry an action"
    );
    assert!(shows_text(&blocked.element, "Accept the terms first."));

    let allowed = surface_with(r#"{"terms":true}"#, component);
    assert!(
        has_click(&allowed.element),
        "ticking the box must make the button live again"
    );
}

#[test]
fn every_leaf_predicate_gates() {
    let cases: &[(&str, &str, &str, bool)] = &[
        ("required", r#"{"v":""}"#, r#""call":"required""#, false),
        ("required", r#"{"v":"x"}"#, r#""call":"required""#, true),
        ("email", r#"{"v":"nope"}"#, r#""call":"email""#, false),
        ("email", r#"{"v":"a@b.co"}"#, r#""call":"email""#, true),
        (
            "length",
            r#"{"v":"abc"}"#,
            r#""call":"length","args":{"value":{"path":"/v"},"min":5}"#,
            false,
        ),
        (
            "length",
            r#"{"v":"abcdef"}"#,
            r#""call":"length","args":{"value":{"path":"/v"},"min":5}"#,
            true,
        ),
        (
            "numeric",
            r#"{"v":"abc"}"#,
            r#""call":"numeric","args":{"value":{"path":"/v"}}"#,
            false,
        ),
        (
            "numeric",
            r#"{"v":150}"#,
            r#""call":"numeric","args":{"value":{"path":"/v"},"max":100}"#,
            false,
        ),
        (
            "numeric",
            r#"{"v":50}"#,
            r#""call":"numeric","args":{"value":{"path":"/v"},"max":100}"#,
            true,
        ),
        (
            "regex",
            r#"{"v":"abc"}"#,
            r#""call":"regex","args":{"value":{"path":"/v"},"pattern":"^\\d+$"}"#,
            false,
        ),
        (
            "regex",
            r#"{"v":"123"}"#,
            r#""call":"regex","args":{"value":{"path":"/v"},"pattern":"^\\d+$"}"#,
            true,
        ),
    ];

    for (name, data, condition, expected_valid) in cases {
        // `required`/`email` take their value the short way; the rest carry
        // explicit args already.
        let condition = if condition.contains("args") {
            (*condition).to_owned()
        } else {
            format!(r#"{condition},"args":{{"value":{{"path":"/v"}}}}"#)
        };
        let component = format!(
            r#"{{"id":"subject","component":"TextField","label":"V",
                 "value":{{"path":"/v"}},
                 "checks":[{{"condition":{{{condition}}},"message":"BAD-{name}"}}]}}"#
        );
        let rendered = surface_with(data, &component);
        let shown = shows_text(&rendered.element, &format!("BAD-{name}"));
        assert_eq!(
            !shown, *expected_valid,
            "{name} over {data}: expected valid={expected_valid}, message shown={shown}; \
             notes: {:?}",
            rendered.notes
        );
    }
}

#[test]
fn and_or_not_compose() {
    let cases: &[(&str, &str, bool)] = &[
        (
            "and",
            r#"{"call":"and","args":{"values":[
                 {"call":"required","args":{"value":{"path":"/v"}}},
                 {"call":"email","args":{"value":{"path":"/v"}}}]}}"#,
            false,
        ),
        (
            "or",
            r#"{"call":"or","args":{"values":[
                 {"call":"email","args":{"value":{"path":"/v"}}},
                 true]}}"#,
            true,
        ),
        ("not", r#"{"call":"not","args":{"value":true}}"#, false),
        (
            "not-of-false",
            r#"{"call":"not","args":{"value":false}}"#,
            true,
        ),
    ];
    for (name, condition, expected_valid) in cases {
        let component = format!(
            r#"{{"id":"subject","component":"TextField","label":"V",
                 "value":{{"path":"/v"}},
                 "checks":[{{"condition":{condition},"message":"BAD-{name}"}}]}}"#
        );
        let rendered = surface_with(r#"{"v":"plain"}"#, &component);
        let shown = shows_text(&rendered.element, &format!("BAD-{name}"));
        assert_eq!(
            !shown, *expected_valid,
            "{name}: notes {:?}",
            rendered.notes
        );
    }
}

/// A pattern that is valid in a browser and not in this engine must be
/// reported, and must not silently pass or silently block.
#[test]
fn an_unsupported_pattern_reports_and_does_not_gate() {
    let component = r#"{"id":"subject","component":"TextField","label":"Password",
        "value":{"path":"/v"},
        "checks":[{"condition":{"call":"regex","args":{
                     "value":{"path":"/v"},"pattern":"(?=.*[A-Z]).{8,}"}},
                   "message":"Needs a capital."}]}"#;
    let rendered = surface_with(r#"{"v":"nope"}"#, component);
    assert!(
        !shows_text(&rendered.element, "Needs a capital."),
        "a check that could not be evaluated must not block the user with a message \
         they cannot act on"
    );
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::InvalidValue && n.detail.contains("lookaround")),
        "got: {:?}",
        rendered.notes
    );
    assert!(
        any_broken(&rendered.notes),
        "a rule the stream asked for and this build cannot enforce is broken"
    );
}

#[test]
fn an_unknown_boolean_function_reports_and_does_not_gate() {
    let component = r#"{"id":"subject","component":"TextField","label":"V",
        "value":{"path":"/v"},
        "checks":[{"condition":{"call":"isPrime","args":{"value":{"path":"/v"}}},
                   "message":"Not prime."}]}"#;
    let rendered = surface_with(r#"{"v":"4"}"#, component);
    assert!(!shows_text(&rendered.element, "Not prime."));
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::UnimplementedFunction && n.detail.contains("isPrime")),
        "got: {:?}",
        rendered.notes
    );
}

/// One bad rule must not erase the control, nor its well-formed siblings.
#[test]
fn a_malformed_rule_is_reported_and_its_siblings_still_gate() {
    let component = r#"{"id":"subject","component":"TextField","label":"V",
        "value":{"path":"/v"},
        "checks":[{"nonsense":true},
                  {"condition":{"call":"required","args":{"value":{"path":"/v"}}},
                   "message":"Required."}]}"#;
    let rendered = surface_with(r#"{"v":""}"#, component);
    assert!(
        rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::MalformedComponent),
        "the bad rule must be reported, got: {:?}",
        rendered.notes
    );
    assert!(
        shows_text(&rendered.element, "Required."),
        "the good rule must still gate"
    );
    assert!(
        rendered.element.children_ref().iter().any(|_| true),
        "the control itself must still render"
    );
}

/// `checks` in any shape at all must never sink the component into an
/// unknown-component placeholder.
#[test]
fn checks_of_the_wrong_shape_never_erase_the_control() {
    for shape in [
        r#""checks":"yes""#,
        r#""checks":{"a":1}"#,
        r#""checks":[1,2]"#,
    ] {
        let component = format!(
            r#"{{"id":"subject","component":"TextField","label":"Email",
                 "value":{{"path":"/v"}},{shape}}}"#
        );
        let rendered = surface_with(r#"{"v":"x"}"#, &component);
        assert!(
            !rendered
                .notes
                .iter()
                .any(|n| n.kind == NoteKind::UnknownComponent),
            "{shape} turned the whole TextField into a placeholder: {:?}",
            rendered.notes
        );
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
            frame.query(&by::label("Email")).is_some(),
            "{shape}: the labeled field must still be there"
        );
    }
}

#[test]
fn validation_regexp_is_enforced() {
    let component = r#"{"id":"subject","component":"TextField","label":"Code",
        "value":{"path":"/v"},"validationRegexp":"^[A-Z]{3}$"}"#;
    assert!(
        shows_text(
            &surface_with(r#"{"v":"abcd"}"#, component).element,
            "Invalid format."
        ),
        "a value outside the pattern must be marked"
    );
    let ok = surface_with(r#"{"v":"ABC"}"#, component);
    assert!(!shows_text(&ok.element, "Invalid format."));
    assert!(
        !ok.notes.iter().any(|n| n.detail.contains("not enforced")),
        "got: {:?}",
        ok.notes
    );
}

/// A condition nested past the cap must stop rather than recurse until the
/// stack runs out.
///
/// Two limits stack here, and the renderer's is the tighter one on purpose.
/// serde_json refuses to parse past its own recursion limit (~128 levels of
/// JSON nesting, which is about 40 `not`s once each one's `{call, args,
/// value}` wrapping is counted), so a stream deep enough to threaten the
/// stack never reaches the renderer at all. That makes the parser a real
/// backstop and not one to rely on: it bounds *nesting*, not the work done
/// per level, and it is serde_json's choice to change, not this crate's.
#[test]
fn a_deeply_nested_condition_stops_at_the_cap() {
    let mut condition = "true".to_owned();
    for _ in 0..24 {
        condition = format!(r#"{{"call":"not","args":{{"value":{condition}}}}}"#);
    }
    let component = format!(
        r#"{{"id":"subject","component":"TextField","label":"V",
             "value":{{"path":"/v"}},
             "checks":[{{"condition":{condition},"message":"Nope."}}]}}"#
    );
    let rendered = surface_with(r#"{"v":"x"}"#, &component);
    assert!(
        rendered.notes.iter().any(|n| n.kind == NoteKind::DepthCap),
        "got: {:?}",
        rendered.notes
    );
}

/// The failing state is a *look*, not just a message string: a danger ring
/// on the control and a danger-toned line beneath it. The access tree
/// cannot see either, so this is a golden or it is untested.
#[test]
fn the_failing_state_is_pinned_in_pixels() {
    let component = r#"{"id":"subject","component":"TextField","label":"Email",
        "value":{"path":"/email"},
        "checks":[{"condition":{"call":"email","args":{"value":{"path":"/email"}}},
                   "message":"Enter a valid email address."}]}"#;
    let rendered = surface_with(r#"{"email":"not-an-email"}"#, component);
    let img = fenestra_shell::render_element(rendered.element, &Theme::light(), (360, 160));
    fenestra_shell::testing::assert_png_snapshot(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots"),
        "validation_failing_field",
        &img,
    );
}

// ── Round five: what the review of the validation feature found ──────────

/// `true` was doing double duty — a real answer *and* the "could not
/// evaluate" sentinel — so `not` inverted the sentinel into a hard block.
/// A rule this build cannot evaluate must never gate, however it is
/// wrapped.
#[test]
fn not_of_an_unevaluable_rule_still_does_not_gate() {
    for condition in [
        r#"{"call":"not","args":{"value":{"call":"isPrime","args":{}}}}"#,
        r#"{"call":"not","args":{"value":{"call":"regex","args":{
             "value":{"path":"/v"},"pattern":"(?=.*[A-Z]).{8,}"}}}}"#,
        r#"{"call":"not","args":{"value":{"call":"not","args":{"value":
             {"call":"isPrime","args":{}}}}}}"#,
    ] {
        let component = format!(
            r#"{{"id":"subject","component":"TextField","label":"V",
                 "value":{{"path":"/v"}},
                 "checks":[{{"condition":{condition},"message":"BLOCKED"}}]}}"#
        );
        let rendered = surface_with(r#"{"v":"x"}"#, &component);
        assert!(
            !shows_text(&rendered.element, "BLOCKED"),
            "a rule that could not be evaluated blocked the user: {condition}\nnotes: {:?}",
            rendered.notes
        );
        assert!(
            any_broken(&rendered.notes),
            "…and it must still be reported: {condition}"
        );
    }
}

/// An odd number of `not`s around the depth cap used to flip the sentinel;
/// the existing cap test happens to use an even count, so it passed.
#[test]
fn an_odd_nesting_past_the_cap_does_not_gate_either() {
    let mut condition = "true".to_owned();
    for _ in 0..25 {
        condition = format!(r#"{{"call":"not","args":{{"value":{condition}}}}}"#);
    }
    let component = format!(
        r#"{{"id":"subject","component":"TextField","label":"V",
             "value":{{"path":"/v"}},
             "checks":[{{"condition":{condition},"message":"BLOCKED"}}]}}"#
    );
    let rendered = surface_with(r#"{"v":"x"}"#, &component);
    assert!(
        !shows_text(&rendered.element, "BLOCKED"),
        "notes: {:?}",
        rendered.notes
    );
    assert!(rendered.notes.iter().any(|n| n.kind == NoteKind::DepthCap));
}

/// A bound that is present but unusable is not the same as an absent one.
/// Dropping it silently leaves a password field with no minimum length and
/// a surface claiming full fidelity.
#[test]
fn an_unusable_bound_is_reported_and_the_rule_stops_gating() {
    for bound in [
        r#""min":8.5"#,
        r#""min":"eight""#,
        r#""min":-4"#,
        r#""min":{"path":"/nowhere"}"#,
        r#""min":true"#,
    ] {
        let component = format!(
            r#"{{"id":"subject","component":"TextField","label":"Password",
                 "value":{{"path":"/v"}},
                 "checks":[{{"condition":{{"call":"length","args":{{
                    "value":{{"path":"/v"}},{bound}}}}},"message":"Too short."}}]}}"#
        );
        let rendered = surface_with(r#"{"v":"abc"}"#, &component);
        assert!(
            !rendered.notes.is_empty(),
            "{bound}: a bound that is not being enforced must be reported"
        );
        assert!(
            any_broken(&rendered.notes),
            "{bound}: an unenforced validation bound is broken, got: {:?}",
            rendered.notes
        );
    }
}

/// A rule whose `value` argument is missing is malformed, not "the value is
/// null" — `required` used to fail closed on it, blocking the control with
/// no note at all.
#[test]
fn a_rule_with_no_value_argument_reports_and_does_not_gate() {
    let component = r#"{"id":"subject","component":"TextField","label":"V",
        "value":{"path":"/v"},
        "checks":[{"condition":{"call":"required","args":{}},"message":"BLOCKED"}]}"#;
    let rendered = surface_with(r#"{"v":"x"}"#, component);
    assert!(
        !shows_text(&rendered.element, "BLOCKED"),
        "notes: {:?}",
        rendered.notes
    );
    assert!(any_broken(&rendered.notes), "got: {:?}", rendered.notes);
}

/// The catalog requires at least two operands. An empty `or` used to be
/// vacuously false, so it gated — silently.
#[test]
fn a_composition_with_too_few_operands_reports_and_does_not_gate() {
    for condition in [
        r#"{"call":"or","args":{"values":[]}}"#,
        r#"{"call":"and","args":{"values":[]}}"#,
        r#"{"call":"or","args":{"values":[false]}}"#,
    ] {
        let component = format!(
            r#"{{"id":"subject","component":"TextField","label":"V",
                 "value":{{"path":"/v"}},
                 "checks":[{{"condition":{condition},"message":"BLOCKED"}}]}}"#
        );
        let rendered = surface_with(r#"{"v":"x"}"#, &component);
        assert!(
            !shows_text(&rendered.element, "BLOCKED"),
            "{condition} gated: {:?}",
            rendered.notes
        );
        assert!(
            any_broken(&rendered.notes),
            "{condition} said nothing: {:?}",
            rendered.notes
        );
    }
}

/// A Modal trigger whose check fails must not open the dialog. The button
/// is painted dead and marked invalid; arming the wrapper `labeled_control`
/// builds around it made the whole thing clickable anyway.
#[test]
fn a_blocked_modal_trigger_does_not_open_its_dialog() {
    let stream = r#"[
      {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
      {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"terms":false}}},
      {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Column","children":["dialog"]},
        {"id":"dialog","component":"Modal","trigger":"btn","content":"sheet"},
        {"id":"btn","component":"Button","child":"lbl",
         "checks":[{"condition":{"call":"required","args":{"value":{"path":"/terms"}}},
                    "message":"Accept the terms first."}]},
        {"id":"lbl","component":"Text","text":"Open"},
        {"id":"sheet","component":"Text","text":"Dialog body"}
      ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(
        shows_text(&rendered.element, "Accept the terms first."),
        "the failing check must still be explained"
    );
    assert!(
        !has_click(&rendered.element),
        "a blocked trigger must not carry the open-modal click, got a clickable tree"
    );
}

/// Ticking the box makes the same trigger live again.
#[test]
fn an_unblocked_modal_trigger_still_opens() {
    let stream = r#"[
      {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
      {"version":"v0.9","updateDataModel":{"surfaceId":"s","value":{"terms":true}}},
      {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Column","children":["dialog"]},
        {"id":"dialog","component":"Modal","trigger":"btn","content":"sheet"},
        {"id":"btn","component":"Button","child":"lbl",
         "checks":[{"condition":{"call":"required","args":{"value":{"path":"/terms"}}},
                    "message":"Accept the terms first."}]},
        {"id":"lbl","component":"Text","text":"Open"},
        {"id":"sheet","component":"Text","text":"Dialog body"}
      ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    assert!(has_click(&rendered.element), "notes: {:?}", rendered.notes);
    assert!(!any_broken(&rendered.notes), "got: {:?}", rendered.notes);
}

/// A plain syntax error is not "this engine lacks lookaround". An agent
/// reads the cause to decide whether to rewrite the pattern or the client.
#[test]
fn a_malformed_pattern_is_not_blamed_on_the_engine() {
    let component = r#"{"id":"subject","component":"TextField","label":"V",
        "value":{"path":"/v"},"validationRegexp":"[a-"}"#;
    let rendered = surface_with(r#"{"v":"x"}"#, component);
    let note = rendered
        .notes
        .iter()
        .find(|n| n.kind == NoteKind::InvalidValue)
        .unwrap_or_else(|| panic!("expected a note, got {:?}", rendered.notes));
    assert!(
        !note.detail.contains("lookaround"),
        "a syntax error was blamed on the engine: {}",
        note.detail
    );
}

/// A future revision of the basic catalog reuses its component names with
/// different meanings — the one case the note exists for.
#[test]
fn a_different_catalog_revision_is_not_mistaken_for_v0_9() {
    for catalog in [
        "https://a2ui.org/specification/v1_5/catalogs/basic/catalog.json",
        "https://elsewhere.example/catalogs/basic/catalog.json",
    ] {
        let stream = format!(
            r#"[
              {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"{catalog}"}}}},
              {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
                {{"id":"root","component":"Text","text":"hello"}}
              ]}}}}
            ]"#
        );
        let rendered = apply(&stream)
            .surface("s")
            .expect("surface")
            .render(&Theme::light());
        assert!(!rendered.notes.is_empty(), "{catalog} passed as v0.9 basic");
        assert!(
            any_broken(&rendered.notes),
            "{catalog}: a catalog whose components may mean something else cannot be \
             'approximate', got: {:?}",
            rendered.notes
        );
    }
}
