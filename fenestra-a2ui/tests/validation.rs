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
