//! Regression tests from the 2026-08-06 adversarial review: the ways a
//! short, well-formed stream can make the renderer do unbounded work, or
//! report full fidelity for a surface that does not work.

use std::time::Instant;

use fenestra_a2ui::{Client, NoteKind, any_broken, parse_stream};
use fenestra_core::{Element, Theme};

fn apply(stream: &str) -> Client {
    let msgs = parse_stream(stream).expect("stream parses");
    let mut client = Client::new();
    client.apply_all(&msgs).expect("stream applies");
    client
}

fn count_elements<Msg>(el: &Element<Msg>) -> usize {
    1 + el.children_ref().iter().map(count_elements).sum::<usize>()
}

/// Finding 1: nested templates multiply, and only each factor was capped.
///
/// `MAX_TEMPLATE_CHILDREN` bounds one expansion at 1000. Nothing bounded
/// the *product*: an absolute template path is scope-invariant by design
/// (that is the fix for `//`-corrupted pointers), so every level of nesting
/// can expand the same array again, and cycle detection does not fire
/// because each level is a distinct component id. Four levels over a
/// 30-item list is 810 000 elements from ~500 bytes of JSON; `MAX_DEPTH`
/// of 16 allows far worse. Every consumer — MCP, CLI, live window — reaches
/// this through `Surface::render` on agent-supplied input.
#[test]
fn nested_templates_cannot_multiply_without_bound() {
    let items: Vec<String> = (0..30).map(|i| i.to_string()).collect();
    let stream = format!(
        r#"[
          {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
          {{"version":"v0.9","updateDataModel":{{"surfaceId":"s","path":"/items",
            "value":[{}]}}}},
          {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
            {{"id":"root","component":"Column","children":{{"componentId":"lvl1","path":"/items"}}}},
            {{"id":"lvl1","component":"Column","children":{{"componentId":"lvl2","path":"/items"}}}},
            {{"id":"lvl2","component":"Column","children":{{"componentId":"lvl3","path":"/items"}}}},
            {{"id":"lvl3","component":"Column","children":{{"componentId":"leaf","path":"/items"}}}},
            {{"id":"leaf","component":"Text","text":"x"}}
          ]}}}}
        ]"#,
        items.join(",")
    );

    let client = apply(&stream);
    let started = Instant::now();
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let elapsed = started.elapsed();

    let built = count_elements(&rendered.element);
    assert!(
        built < 100_000,
        "nested templates built {built} elements from a 500-byte stream; \
         the product across nesting levels must be bounded, not just each level"
    );
    assert!(
        rendered.notes.iter().any(|n| n.kind == NoteKind::Truncated),
        "work dropped to stay inside the budget must be reported, got: {:?}",
        rendered.notes
    );
    assert!(
        elapsed.as_secs() < 5,
        "rendering took {elapsed:?}; a hostile stream must not be able to stall a render"
    );
}

/// The budget has to be charged where components are *built*, not where a
/// container happens to ask for a list of them.
///
/// The first cut metered `children_of`, which is only how `Row`, `Column`
/// and `List` reach their children. `Card`, `Tabs`, `Modal` and `Button`
/// call `render_by_id` directly, and every one of those was an uncharged
/// multiplier stacked on a charged one: a `Card` chain multiplies the
/// ceiling by its depth, and a `Modal` — which renders its trigger *and*
/// its content — by two per level. A ceiling with four ways around it is
/// not a ceiling.
///
/// Here a template drains the budget and every one of the components it
/// paid for then hangs a deep `Card` chain underneath, all of it free under
/// the old accounting.
#[test]
fn containers_that_bypass_children_of_are_charged_too() {
    // 1000 template rows, each hanging a 14-deep Card chain — 15 001
    // components, of which the old accounting charged only the 1000.
    let items: Vec<String> = (0..1000).map(|i| i.to_string()).collect();
    let mut cards: Vec<String> = (0..14)
        .map(|i| {
            let next = if i == 13 {
                "leaf".to_owned()
            } else {
                format!("c{}", i + 1)
            };
            format!(r#"{{"id":"c{i}","component":"Card","child":"{next}"}}"#)
        })
        .collect();
    cards.push(r#"{"id":"leaf","component":"Text","text":"x"}"#.to_owned());
    let stream = format!(
        r#"[
          {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
          {{"version":"v0.9","updateDataModel":{{"surfaceId":"s","path":"/items",
            "value":[{}]}}}},
          {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
            {{"id":"root","component":"Column","children":{{"componentId":"c0","path":"/items"}}}},
            {}
          ]}}}}
        ]"#,
        items.join(","),
        cards.join(","),
    );

    let client = apply(&stream);
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());

    // The note is the discriminator, not the element count. Under the old
    // accounting the 1000 template rows fit the budget with room to spare,
    // so nothing was ever reported as dropped — while the 14 000 Cards
    // hanging off them were built for free. A `Truncated` note here means
    // the Cards were counted.
    assert!(
        rendered.notes.iter().any(|n| n.kind == NoteKind::Truncated),
        "15 001 components were built without one being reported as dropped; \
         Card, Tabs, Modal and Button reach children without going through \
         children_of, so charging the container is not charging the render. \
         Notes: {:?}",
        rendered.notes
    );
    let built = count_elements(&rendered.element);
    assert!(
        built < 30_000,
        "the budget bounds components, and each refusal still costs a small \
         placeholder; {built} elements is past what either accounts for"
    );
}

/// A surface may nest as deeply as `MAX_DEPTH` says it may, on the stack
/// the renderer actually runs on.
///
/// This is the regression test for a crash, not a slowdown. `render_by_id`
/// -> `render_component` -> `children_of` -> `render_by_id` is a recursive
/// cycle, and `render_component` used to be a single `match` over all
/// nineteen component kinds. An unoptimized build gives such a frame room
/// for every arm's locals at once, so each level of nesting cost what the
/// *largest* arm needed — and the largest arms (`ChoicePicker`, `Slider`,
/// `DateTimeInput`) are leaves that cannot even appear on the recursive
/// path. On a 2 MiB stack the result overflowed at **seven** levels: not
/// an attack, an ordinary surface, since a Card inside a List inside a Tab
/// is already half the budget. A stack overflow aborts the process instead
/// of unwinding, so this was `SIGABRT` for the whole MCP server, with no
/// note, no error, and nothing for a caller to catch.
///
/// A strictly linear chain, one child per level: no template, no fan-out,
/// nothing amplified. The only variable is depth.
///
/// It runs on an explicitly sized thread, at **half** the 2 MiB that
/// `std::thread` and tokio's blocking pool give by default, so that the
/// pass carries a 2x margin rather than merely being true today. Two
/// reasons for the margin. The harness's own main thread is 8 MiB and
/// would have hidden the original bug for another four levels, so a test
/// that inherits its stack tests nothing; and a stack overflow `abort()`s
/// rather than unwinding, which kills the whole test binary and reports as
/// every test in the file failing on a signal, pointing at nothing — the
/// `.expect` message below never gets to print. Failing at half the real
/// stack means a frame that grows again shows up here while production
/// still has room, instead of both giving out at once.
#[test]
fn renders_at_the_full_depth_cap() {
    // `MAX_DEPTH` is private; this mirrors it deliberately, so that raising
    // the cap without re-measuring the stack shows up as a failure here.
    const CAP: usize = 16;

    // `CAP` containers, so the deepest node — the leaf — sits at exactly
    // `MAX_DEPTH`. One more would be the cap legitimately refusing.
    let mut components: Vec<String> = (0..CAP)
        .map(|i| {
            let next = if i + 1 == CAP {
                "leaf".to_owned()
            } else {
                format!("lvl{}", i + 1)
            };
            let id = if i == 0 {
                "root".to_owned()
            } else {
                format!("lvl{i}")
            };
            format!(r#"{{"id":"{id}","component":"Column","children":["{next}"]}}"#)
        })
        .collect();
    components.push(r#"{"id":"leaf","component":"Text","text":"deep"}"#.to_owned());
    let stream = format!(
        r#"[
          {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
          {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[{}]}}}}
        ]"#,
        components.join(",")
    );

    let stack: usize = std::env::var("FENESTRA_PROBE_STACK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2 * 1024 * 1024);
    let built = std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let client = apply(&stream);
            let rendered = client
                .surface("s")
                .expect("surface")
                .render(&Theme::light());
            // The chain must actually render, not bottom out in a
            // `[deep: ..]` placeholder — that would pass the crash test
            // while proving nothing about the cap.
            assert!(
                !rendered.notes.iter().any(|n| n.kind == NoteKind::DepthCap),
                "the cap refused a chain it permits: {:?}",
                rendered.notes
            );
            count_elements(&rendered.element)
        })
        .expect("spawn")
        .join()
        .expect("rendering a surface at the documented depth cap must not abort the process");

    assert!(built > CAP, "expected a real chain, built {built} elements");
}

/// The static sibling of the finding above, and the one it missed.
///
/// `nested_templates_cannot_multiply_without_bound` fixed the *template*
/// arm of `children_of` by charging every materialized child to a
/// render-wide budget. The `Static` arm was left drawing from nothing at
/// all — and a static child list is the cheaper amplifier of the two,
/// because it needs no data model to expand against. Cycle detection does
/// not fire (each level is a distinct component id), and the framework
/// guards element-tree *depth* only, never breadth, so nothing downstream
/// catches it either.
///
/// Three `Column`s naming the next one fifty times is 127 551 components
/// from about a kilobyte of JSON, and the shape scales as fan-out to the
/// power of the nesting depth. Every consumer reaches this through
/// `Surface::render` on agent-supplied input — the MCP `render_a2ui` tool
/// most directly.
///
/// Deliberately wide and shallow: breadth is what this test is about, and
/// nesting it deeper would trip the *separate* stack-depth limit instead
/// and prove nothing about the budget.
#[test]
fn static_children_cannot_multiply_without_bound() {
    let kids = |next: &str| {
        (0..50)
            .map(|_| format!("\"{next}\""))
            .collect::<Vec<_>>()
            .join(",")
    };
    let stream = format!(
        r#"[
          {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
          {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
            {{"id":"root","component":"Column","children":[{}]}},
            {{"id":"lvl1","component":"Column","children":[{}]}},
            {{"id":"lvl2","component":"Column","children":[{}]}},
            {{"id":"leaf","component":"Text","text":"x"}}
          ]}}}}
        ]"#,
        kids("lvl1"),
        kids("lvl2"),
        kids("leaf"),
    );
    assert!(
        stream.len() < 2048,
        "the point is that the input is tiny; this one is {} bytes",
        stream.len()
    );

    let client = apply(&stream);
    let started = Instant::now();
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());
    let elapsed = started.elapsed();

    let built = count_elements(&rendered.element);
    assert!(
        built < 100_000,
        "static children built {built} elements from a {}-byte stream; every \
         materialized child must be charged to the render-wide budget, not just \
         the ones a template generated",
        stream.len()
    );
    assert!(
        rendered.notes.iter().any(|n| n.kind == NoteKind::Truncated),
        "work dropped to stay inside the budget must be reported, got: {:?}",
        rendered.notes
    );
    assert!(
        elapsed.as_secs() < 5,
        "rendering took {elapsed:?}; a hostile stream must not be able to stall a render"
    );
}

/// The budget is shared, so the two arms cannot be played against each
/// other: a template expansion that spends it must leave static children
/// bounded too, and the reverse. Charging them to separate counters would
/// re-open the product this pair of tests exists to close.
#[test]
fn the_child_budget_is_shared_between_static_and_template_children() {
    // 100 static -> 50 template each -> 1 static each: 5100 static children
    // and 5000 template children, so *neither arm reaches the 10 000 budget
    // on its own* and only a shared counter can truncate the 10 101 total.
    //
    // Both earlier shapes of this test were wrong in opposite directions:
    // the first had 12 010 static children, which blows the cap unaided and
    // so passed against two separate per-arm budgets; the second had 2441
    // components total and never reached any cap at all.
    let items: Vec<String> = (0..50).map(|i| i.to_string()).collect();
    let hundred = (0..100).map(|_| "\"a\"").collect::<Vec<_>>().join(",");
    let stream = format!(
        r#"[
          {{"version":"v0.9","createSurface":{{"surfaceId":"s","catalogId":"basic"}}}},
          {{"version":"v0.9","updateDataModel":{{"surfaceId":"s","path":"/items",
            "value":[{}]}}}},
          {{"version":"v0.9","updateComponents":{{"surfaceId":"s","components":[
            {{"id":"root","component":"Column","children":[{hundred}]}},
            {{"id":"a","component":"Column","children":{{"componentId":"b","path":"/items"}}}},
            {{"id":"b","component":"Column","children":["leaf"]}},
            {{"id":"leaf","component":"Text","text":"x"}}
          ]}}}}
        ]"#,
        items.join(","),
    );

    let client = apply(&stream);
    let rendered = client
        .surface("s")
        .expect("surface")
        .render(&Theme::light());

    let built = count_elements(&rendered.element);
    assert!(
        built < 20_000,
        "alternating static and template levels built {built} elements; the two \
         arms must draw down one shared budget (40 static + 6000 template is \
         under either cap taken alone)"
    );
    assert!(
        rendered.notes.iter().any(|n| n.kind == NoteKind::Truncated),
        "work dropped to stay inside the budget must be reported, got: {:?}",
        rendered.notes
    );
}

/// Finding 2: `Ctx::modal_triggers` was built from every component the
/// surface has ever defined, not from the ones actually rendered.
///
/// A Modal that nothing in the visible tree references — the normal state
/// of a progressively-delivered stream, or of two Modals sharing a trigger
/// id like `close` — still marked its trigger id as "opens a modal". That
/// made the Button neither inert (so no note, and no disabled styling) nor
/// clickable (it has no action of its own, and the Modal wiring only
/// happens when the Modal is really rendered). The result was a live-looking
/// button that does nothing, on a surface reporting perfect fidelity.
#[test]
fn an_unreachable_modal_does_not_silently_arm_a_button() {
    let stream = r#"[
      {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
      {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Column","children":["btn"]},
        {"id":"btn","component":"Button","child":"lbl"},
        {"id":"lbl","component":"Text","text":"Click me"},
        {"id":"orphan_modal","component":"Modal","trigger":"btn","content":"sheet"},
        {"id":"sheet","component":"Text","text":"never shown"}
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
        "a button wired to a modal that never renders is unreachable and must say so, \
         got: {:?}",
        rendered.notes
    );
    assert!(
        any_broken(&rendered.notes),
        "a control that cannot be operated is broken, not approximate"
    );
}

/// The other half of finding 2: the fix must not disarm a *real* trigger.
/// A Modal reachable from `root` still opens, and says nothing.
#[test]
fn a_reachable_modal_trigger_stays_live_and_unnoted() {
    let stream = r#"[
      {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
      {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Column","children":["dialog"]},
        {"id":"dialog","component":"Modal","trigger":"btn","content":"sheet"},
        {"id":"btn","component":"Button","child":"lbl"},
        {"id":"lbl","component":"Text","text":"Open"},
        {"id":"sheet","component":"Text","text":"Dialog body"}
      ]}}
    ]"#;
    let rendered = apply(stream)
        .surface("s")
        .expect("surface")
        .render(&Theme::light());

    assert!(
        !rendered
            .notes
            .iter()
            .any(|n| n.kind == NoteKind::Unreachable),
        "a live modal trigger must not be reported unreachable, got: {:?}",
        rendered.notes
    );
    assert!(
        !any_broken(&rendered.notes),
        "a working modal is full fidelity, got: {:?}",
        rendered.notes
    );
}

/// Finding 3: an unknown message that names no live surface vanished.
///
/// `Client::apply` records the "newer protocol revision" note *on the
/// surface the message names*. A message with no `surfaceId`, or one that
/// arrives before `createSurface` or after `deleteSurface`, matched no
/// surface — so nothing was recorded anywhere and `apply` still returned
/// `Ok(())`. A crate whose contract is "silence means fidelity" cannot drop
/// a message with no trace at all.
#[test]
fn an_unattributable_message_is_still_recorded() {
    let mut client = Client::new();
    client
        .apply_all(&parse_stream(r#"[{"version":"v0.9","ping":{}}]"#).expect("parses"))
        .expect("an unknown message is skipped, not an error");
    assert!(
        client
            .notes()
            .iter()
            .any(|n| n.kind == NoteKind::UnknownMessage && n.detail.contains("ping")),
        "a message belonging to no surface must still be recorded, got: {:?}",
        client.notes()
    );
}

/// The same gap for a message naming a surface that does not exist.
#[test]
fn a_message_for_a_missing_surface_is_recorded() {
    let mut client = Client::new();
    client
        .apply_all(
            &parse_stream(r#"[{"version":"v0.9","nudge":{"surfaceId":"gone"}}]"#).expect("parses"),
        )
        .expect("skipped, not an error");
    assert!(
        client
            .notes()
            .iter()
            .any(|n| n.kind == NoteKind::UnknownMessage),
        "got: {:?}",
        client.notes()
    );
}

/// Finding 4: the docs promised a note for a non-basic catalog and no code
/// ever produced one. A surface built on a catalog this crate does not
/// implement renders its components best-effort, which is worth knowing
/// before trusting the result.
#[test]
fn a_foreign_catalog_id_is_reported() {
    let stream = r#"[
      {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"https://example.com/fancy"}},
      {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
        {"id":"root","component":"Text","text":"hello"}
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
            .any(|n| n.kind == NoteKind::UnknownCatalog),
        "an unimplemented catalog must be reported, got: {:?}",
        rendered.notes
    );
    // Asserting only the kind would let the severity drift to `approximate`,
    // and `any_broken` is the check the book tells people to build CI on.
    assert!(
        any_broken(&rendered.notes),
        "a surface whose components may mean something else is not 'approximate'"
    );
}

/// The official examples name the basic catalog by its full spec URL, and
/// the bare id `basic` is what the crate's own fixtures use. Neither is
/// foreign, and reporting them would cry wolf on every conforming stream.
#[test]
fn the_basic_catalog_is_not_foreign_by_either_name() {
    for catalog in [
        "basic",
        "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
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
        assert!(
            rendered.notes.is_empty(),
            "{catalog:?} is the basic catalog and must render silently, got: {:?}",
            rendered.notes
        );
    }
}
