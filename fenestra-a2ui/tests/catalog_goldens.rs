//! A golden PNG for every component in the A2UI basic catalog.
//!
//! The conformance goldens pin two *composite* surfaces, which is a coarse
//! net: a component that appears in neither (or only deep inside one) can
//! lose its border, its disabled tint, or its label spacing and every test
//! still passes, because nothing renders it in isolation. These pin each
//! catalog entry on its own, so a kit styling regression names the component
//! it broke.
//!
//! Regenerate with `FENESTRA_UPDATE_SNAPSHOTS=1 cargo test -p fenestra-a2ui`,
//! then *look at* the PNGs — that is the point of them.

use std::path::PathBuf;

use fenestra_a2ui::catalog::BASIC_CATALOG;
use fenestra_a2ui::{Client, Note, NoteKind, NoteSeverity, Rendered, any_broken, parse_stream};
use fenestra_core::Theme;
use fenestra_shell::{render_element, testing::assert_png_snapshot};

/// One surface per catalog entry, sized alike so the goldens are comparable.
const SIZE: (u32, u32) = (360, 240);

/// One catalog entry's golden.
struct Case {
    /// The catalog name, which must match [`BASIC_CATALOG`] exactly.
    name: &'static str,
    /// What `root` holds. `None` means just the subject; a case overrides it
    /// only when the component is unreadable alone — a lone `Divider` is a
    /// hairline flush against the top edge, a golden no reviewer can look at
    /// and judge.
    root: Option<&'static str>,
    /// The components of the surface. `subject` is the one under test.
    body: &'static str,
}

/// The subject always hangs off the same `root`, so every golden has the
/// same frame and a diff is about the component, not the page.
///
/// Bindings use the protocol's `{"path": …}` form on purpose: a bare string
/// is a *literal* in `Dyn`, so `"value": "/email"` would pin a field
/// displaying the text `/email` and prove nothing about binding at all.
const CASES: &[Case] = &[
    Case {
        name: "Text",
        root: None,
        body: r#"{"id":"subject","component":"Text","text":"Heading","variant":"h3"}"#,
    },
    Case {
        name: "Image",
        root: None,
        body: r#"{"id":"subject","component":"Image","url":"https://example.com/a.png",
                  "description":"A product photo","variant":"smallFeature"}"#,
    },
    Case {
        name: "Icon",
        root: None,
        body: r#"{"id":"subject","component":"Icon","name":"star"}"#,
    },
    Case {
        name: "Video",
        root: None,
        body: r#"{"id":"subject","component":"Video","url":"https://example.com/clip.mp4"}"#,
    },
    Case {
        name: "AudioPlayer",
        root: None,
        body: r#"{"id":"subject","component":"AudioPlayer","url":"https://example.com/a.mp3",
                  "description":"Episode 3"}"#,
    },
    Case {
        name: "Row",
        root: None,
        body: r#"{"id":"subject","component":"Row","children":["one","two"],
                  "justify":"spaceBetween"},
                 {"id":"one","component":"Text","text":"Left"},
                 {"id":"two","component":"Text","text":"Right"}"#,
    },
    Case {
        name: "Column",
        root: None,
        body: r#"{"id":"subject","component":"Column","children":["one","two"]},
                 {"id":"one","component":"Text","text":"Above"},
                 {"id":"two","component":"Text","text":"Below"}"#,
    },
    Case {
        name: "List",
        root: None,
        body: r#"{"id":"subject","component":"List","children":["one","two"]},
                 {"id":"one","component":"Text","text":"First item"},
                 {"id":"two","component":"Text","text":"Second item"}"#,
    },
    Case {
        name: "Card",
        root: None,
        body: r#"{"id":"subject","component":"Card","child":"inner"},
                 {"id":"inner","component":"Text","text":"Framed"}"#,
    },
    Case {
        name: "Tabs",
        root: None,
        body: r#"{"id":"subject","component":"Tabs","tabs":[
                   {"title":"Overview","child":"one"},{"title":"Details","child":"two"}]},
                 {"id":"one","component":"Text","text":"Overview pane"},
                 {"id":"two","component":"Text","text":"Details pane"}"#,
    },
    Case {
        // No action on the trigger: a Modal is what makes it pressable, and
        // that is the path the trigger fix restored. A greyed-out "Open"
        // here means the inert-button rule has swallowed a live control.
        name: "Modal",
        root: None,
        body: r#"{"id":"subject","component":"Modal","trigger":"open","content":"sheet"},
                 {"id":"open","component":"Button","child":"open_label"},
                 {"id":"open_label","component":"Text","text":"Open"},
                 {"id":"sheet","component":"Text","text":"Dialog body"}"#,
    },
    Case {
        name: "Divider",
        root: Some(r#""above","subject","below""#),
        body: r#"{"id":"subject","component":"Divider"},
                 {"id":"above","component":"Text","text":"Section one"},
                 {"id":"below","component":"Text","text":"Section two"}"#,
    },
    Case {
        name: "Button",
        root: None,
        body: r#"{"id":"subject","component":"Button","child":"label","variant":"primary",
                  "action":{"event":{"name":"submit"}}},
                 {"id":"label","component":"Text","text":"Submit"}"#,
    },
    Case {
        name: "TextField",
        root: None,
        body: r#"{"id":"subject","component":"TextField","label":"Email",
                  "value":{"path":"/email"}}"#,
    },
    Case {
        name: "CheckBox",
        root: None,
        body: r#"{"id":"subject","component":"CheckBox","label":"Remember me",
                  "value":{"path":"/remember"}}"#,
    },
    Case {
        name: "ChoicePicker",
        root: None,
        body: r#"{"id":"subject","component":"ChoicePicker","label":"Size",
                  "variant":"mutuallyExclusive",
                  "options":[{"label":"Small","value":"s"},{"label":"Large","value":"l"}],
                  "value":{"path":"/size"}}"#,
    },
    Case {
        name: "Slider",
        root: None,
        body: r#"{"id":"subject","component":"Slider","label":"Volume","min":0,"max":100,
                  "value":{"path":"/volume"}}"#,
    },
    Case {
        name: "DateTimeInput",
        root: None,
        body: r#"{"id":"subject","component":"DateTimeInput","label":"Starts",
                  "value":{"path":"/starts"},"enableDate":true,"enableTime":true}"#,
    },
];

/// A data model wide enough for every bound path above, so the goldens show
/// real values rather than the empty state of an unresolved binding.
const DATA: &str = r#"{
    "email":"ada@example.com","remember":true,"size":"s",
    "volume":40,"starts":"2026-08-06T09:30:00Z"
}"#;

fn snapshot_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots")
}

/// `AudioPlayer` -> `catalog_audio_player`: the goldens should read like a
/// listing of the catalog itself.
fn golden_name(component: &str) -> String {
    let mut out = String::from("catalog_");
    for (i, ch) in component.char_indices() {
        if ch.is_ascii_uppercase() && i != 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

fn client_for(case: &Case) -> Client {
    let root = case.root.unwrap_or(r#""subject""#);
    let body = case.body;
    let stream = format!(
        r#"[
          {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
          {{"version":"v0.9","updateDataModel":{{"surfaceId":"s","value":{DATA}}}}},
          {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
             {{"id":"root","component":"Column","children":[{root}]}},
             {body}
          ]}}}}
        ]"#
    );
    let msgs = parse_stream(&stream).expect("catalog fixture parses");
    let mut client = Client::new();
    client.apply_all(&msgs).expect("catalog fixture applies");
    client
}

fn render_case(case: &Case) -> Rendered {
    client_for(case)
        .surface("s")
        .expect("surface")
        .render(&Theme::light())
}

/// A fixture with a typo deserializes to `Kind::Unknown` and renders a
/// plausible-looking placeholder, which would then be pinned as the golden
/// for a component that never rendered at all — a test that passes forever
/// while testing nothing. Same for a binding pointed at a path the data
/// model lacks.
fn fixture_faults(notes: &[Note]) -> Vec<String> {
    notes
        .iter()
        .filter(|n| {
            matches!(
                n.kind,
                NoteKind::UnknownComponent
                    | NoteKind::MalformedComponent
                    | NoteKind::MissingComponent
                    | NoteKind::UnresolvedBinding
                    | NoteKind::BindingType
                    | NoteKind::UnknownIcon
                    | NoteKind::Unreachable
            )
        })
        .map(|n| format!("{:?}: {n}", n.kind))
        .collect()
}

/// Every fixture is checked before any is pinned, so one typo reports itself
/// alongside the others instead of hiding the rest behind an early panic.
#[test]
fn the_catalog_fixtures_render_what_they_claim() {
    let mut faults = Vec::new();
    for case in CASES {
        for fault in fixture_faults(&render_case(case).notes) {
            faults.push(format!("  {}: {fault}", case.name));
        }
    }
    assert!(
        faults.is_empty(),
        "these fixtures are wrong, not the renderer:\n{}",
        faults.join("\n")
    );
}

#[test]
fn every_catalog_component_has_a_golden() {
    for case in CASES {
        let img = render_element(render_case(case).element, &Theme::light(), SIZE);
        assert_png_snapshot(snapshot_dir(), &golden_name(case.name), &img);
    }
}

/// The table must cover the catalog. Adding a component without a golden is
/// exactly the gap these tests exist to close, so it fails here.
#[test]
fn the_golden_table_covers_the_whole_catalog() {
    let covered: Vec<&str> = CASES.iter().map(|c| c.name).collect();
    let missing: Vec<&&str> = BASIC_CATALOG
        .iter()
        .filter(|name| !covered.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "catalog entries with no golden: {missing:?}"
    );

    let stray: Vec<&&str> = covered
        .iter()
        .filter(|name| !BASIC_CATALOG.contains(name))
        .collect();
    assert!(
        stray.is_empty(),
        "goldens for names outside the catalog: {stray:?}"
    );
}

/// The network-asset components are *meant* to announce a fidelity gap.
/// Pinning their pixels without pinning that would let the note quietly
/// disappear while the placeholder keeps looking deliberate.
///
/// It stays `Approximate`, deliberately. Refusing the network is a design
/// stance, not a defect, and nearly every real surface carries a remote
/// image — grading these `Broken` would fire [`any_broken`] on almost
/// everything and cost the check its meaning. The line is "can the surface
/// still do its job": a card with a grey box where the photo goes still
/// shows its title, price and button.
#[test]
fn the_network_asset_placeholders_still_say_so() {
    for component in ["Image", "Video", "AudioPlayer"] {
        let case = CASES
            .iter()
            .find(|c| c.name == component)
            .expect("case exists");
        let rendered = render_case(case);
        let note = rendered
            .notes
            .iter()
            .find(|n| n.kind == NoteKind::NetworkAsset)
            .unwrap_or_else(|| {
                panic!(
                    "{component} renders a placeholder but says nothing: {:?}",
                    rendered.notes
                )
            });
        assert_eq!(
            note.severity(),
            NoteSeverity::Approximate,
            "{component}: a stood-in remote asset is inexact, not broken"
        );
        assert!(
            !any_broken(&rendered.notes),
            "{component}: a remote asset must not trip the CI gate on its own: {:?}",
            rendered.notes
        );
    }
}
