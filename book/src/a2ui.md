# A2UI: rendering agent-authored surfaces

[A2UI](https://a2ui.org) is an open standard for the thing this whole book
is about: an agent sends declarative JSON describing a UI, and a client
renders it with its own component library. Official renderers exist for
Flutter, Lit, Angular, and React. `fenestra-a2ui` is the native Rust one.

The reason it lives here rather than in its own repo is the rest of
fenestra. An A2UI surface renders to an ordinary `Element` tree, which
means everything downstream already works: windowed running, deterministic
headless PNGs, the accessibility tree, golden tests. It is an A2UI client
whose output an agent can check in CI.

## The message stream

A stream is a list of server→client messages. There are four:

| Message | What it does |
| --- | --- |
| `createSurface` | Starts a surface, with a catalog id and optional theme hints |
| `updateComponents` | Adds or replaces component definitions |
| `updateDataModel` | Writes a value at a JSON Pointer path |
| `deleteSurface` | Removes the surface |

Components are a flat adjacency list — each one has an `id`, and layout
components reference their children by id. `root` anchors the tree.

```json
{ "messages": [
  { "createSurface": { "surfaceId": "s1", "catalogId": "basic" } },
  { "updateDataModel": { "surfaceId": "s1", "value": { "user": { "name": "Ada" } } } },
  { "updateComponents": { "surfaceId": "s1", "components": [
    { "id": "root", "component": "Column", "children": ["greeting"] },
    { "id": "greeting", "component": "Text", "text": { "path": "/user/name" } }
  ] } }
] }
```

## Rendering one

From the command line, where `-` or no path reads stdin:

```sh
fenestra a2ui stream.json --size 480x640 --out surface.png
```

That prints the surface id, the access tree, and the fidelity notes as
JSON, and writes the PNG. An agent gets the same thing through the
`render_a2ui` MCP tool.

In Rust:

```rust,ignore
use fenestra_a2ui::{Client, parse_stream};
use fenestra_core::Theme;

let msgs = parse_stream(&json)?;
let mut client = Client::new();
client.apply_all(&msgs)?;

let surface = client.single_surface().expect("one surface");
let rendered = surface.render(&Theme::light());
// rendered.element → any runner, or render_element for a PNG
// rendered.notes   → what didn't map faithfully
```

`parse_stream` takes either a bare array of messages or the
`{"messages": [...]}` wrapper the official gallery examples use.

## Bindings and templates

Any dynamic value can be a literal, a binding, or a function call.

```json
{ "text": "Hello" }                                  // literal
{ "text": { "path": "/user/name" } }                 // binding
{ "text": { "call": "formatCurrency",                // function
            "args": { "value": { "path": "/total" }, "currency": "USD" } } }
```

Layout components take either a static child list or a template that
repeats one component over a data-model list:

```json
{ "id": "list", "component": "Column",
  "children": { "componentId": "row_tpl", "path": "/items" } }
```

Inside a template, relative paths resolve against the current item, so
`{"path": "name"}` in `row_tpl` reads `/items/3/name` for the fourth row.
Absolute paths still reach the whole model. Template expansion caps at
1000 children, with a note when it truncates.

The implemented functions are `formatString`, `formatNumber`,
`formatCurrency`, `formatDate`, and `pluralize`. They are deterministic
and carry no locale data — formatting is approximate on purpose, so a
render is reproducible on any machine. Anything else resolves to a
placeholder and records a note.

## Interaction

A rendered surface emits `A2uiMsg`. Feed each one to `Surface::handle`,
which mutates the surface and hands back an `A2uiSignal` for anything the
host has to carry out:

```rust,ignore
if let Some(signal) = surface.handle(msg) {
    match signal {
        A2uiSignal::Event { name, context, data_model, source_id } => {
            let msg = surface.action_message(&name, &source_id, &context, &now_iso);
            transport.send(msg);   // the client→server `action` message
        }
        A2uiSignal::OpenUrl(url) => opener::open(url)?,
    }
}
```

Inputs bound to a path write straight back into the data model, so the
next render reads what the user typed. `action_message` takes the
timestamp from you rather than reading a clock, which is what keeps the
crate deterministic.

## Notes are the contract

Every render returns `notes`. An empty list means every component and
every binding mapped cleanly. A non-empty one tells you exactly what
didn't, pointed at the component id:

```
avatar_img: binding "/user/avatar" resolves to nothing
chart_1: component "FancyGauge" did not map onto the basic catalog
```

This is the same fidelity-or-report rule the JSON emitter follows. Nothing
degrades quietly.

Known gaps, all of which record a note: remote images, video, and audio
render as labeled placeholders (a deterministic render never touches the
network), `DateTimeInput` is an ISO text field rather than a calendar,
obscured text fields render unmasked, and `checks` validation rules parse
but do not yet gate actions.

## Verifying it

Because a surface is just an `Element` tree, an A2UI stream is testable
the same way anything else in fenestra is:

```rust,ignore
let rendered = surface.render(&Theme::light());
let image = render_element(rendered.element, &Theme::light(), (480, 640));
assert!(rendered.notes.is_empty(), "stream degraded: {:?}", rendered.notes);
assert_png_snapshot("tests/snapshots", "checkout_surface", &image);
```

Assert on the notes as well as the pixels. A regression that starts
silently degrading a component shows up in the notes long before it is
visible in a diff.
