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
for signal in surface.handle(msg) {
    match signal {
        A2uiSignal::Event { name, context, data_model, source_id } => {
            let msg = surface.action_message(&name, &source_id, &context, &now_iso);
            transport.send(msg);   // the client→server `action` message
        }
        A2uiSignal::OpenUrl(url) => opener::open(url)?,
    }
}
```

`handle` returns a `Vec` because one interaction can legitimately mean two
things — a Modal trigger that is also a Button opens the dialog *and*
reports its action.

Handing that `url` straight to the platform opener is safe, and it is worth
knowing why, because it would not be if the string came through unchecked.
`open(1)` and `xdg-open` launch whichever application has registered the
scheme, so a stream that said `file:` or some installed app's custom scheme
would be choosing a program to start on your user's machine — and a stream
is only ever as trustworthy as whatever the agent writing it last read. So
the renderer classifies the URL: `http`, `https` and `mailto` can become
`OpenUrl`, and everything else renders as a visible, inert control carrying
a `blockedUrlScheme` note.

An allowed scheme is not the same as an allowed URL, and `mailto:` is where
that bites. Mail clients that honour an attachment field will stage a local
file the user never chose into a pre-addressed message, so a `mailto:` gets
through only if every header field it carries is one a generated link
actually needs — `to`, `cc`, `bcc`, `subject`, `body`, `in-reply-to` — with
no carriage return or newline in the values, which is how an extra header
gets smuggled into a permitted one. Names and values are percent-decoded
first, because `%61ttach` is `attach` by the time your mail client reads it.
A well-formed `mailto:` can therefore come back inert; the note says so.

The check runs twice: once when the renderer resolves the action, and again
in `Surface::handle`, which will not emit the signal at all for a URL that
fails. The second one matters because `A2uiMsg` is public — if you build an
`OpenUrl` message yourself, or replay one from a log, it is still checked
before it becomes a signal. By the time a URL reaches you it has been
checked.

Inputs bound to a path write straight back into the data model, so the
next render reads what the user typed. `action_message` takes the
timestamp from you rather than reading a clock, which is what keeps the
crate deterministic.

## Notes are the contract

Every render returns `notes`. An empty list means every component and
every binding mapped cleanly. A non-empty one says exactly what didn't,
pointed at the component id that carried it:

```json
{ "componentId": "avatar_img", "kind": "unresolvedBinding",
  "detail": "binding \"/user/avatar\" resolves to nothing" }
```

The `kind` is there so you can branch. An agent that sees `unknownIcon`
retries with a different name; one that sees `unknownComponent` knows the
catalog is the problem, not its data. Matching on the prose would break
the first time a message is reworded.

Each kind also carries a severity. `broken` means the surface is not what
the stream asked for — a missing component, an unresolved binding, a write
that didn't apply. `approximate` means it rendered the right thing
inexactly. `any_broken(&notes)` is the one-line check for a test:

```rust,ignore
assert!(!fenestra_a2ui::any_broken(&rendered.notes), "{:?}", rendered.notes);
```

The approximate cases, all of which record a note: remote images, video,
and audio render as labeled placeholders (a deterministic render never
touches the network), and `DateTimeInput` is an ISO text field rather than
a calendar.

Validation is not on that list any more. A control's `checks` run on every
render, the first failing rule shows its message beneath the control, and a
Button whose checks fail carries no action — so a form that says "accept the
terms first" now means it. (A Modal trigger whose checks fail keeps its
dialog shut, too.) TextField and DateTimeInput also get the kit's invalid
ring; CheckBox, ChoicePicker and Slider show the message alone, because the
kit has no invalid state for those controls yet. The catalog's eight boolean
functions (`required`, `regex`, `length`, `numeric`, `email`, `and`, `or`,
`not`) and TextField's `validationRegexp` all work. Where a rule can't be
evaluated here — Rust's regex engine has no lookaround or backreferences,
so a pattern written for a browser may not compile — the check does not
gate and records a `broken` note, which is the honest split: the user is
not blocked by a rule nobody can satisfy, and the caller is told the rule
isn't being enforced.

Two gaps count as `broken`, not approximate, because the surface cannot do
what the stream described: an `obscured` field renders unmasked — the
pixels contain the value meant to be hidden — and a modal trigger that
wraps its own interactive child never opens its dialog, because that child
takes the press. A form with a password field will fail the `any_broken`
check above until masking lands. That is deliberate: the alternative is a
green check over a screenshot with the password in it.

This is the same fidelity-or-report rule the JSON emitter follows. Nothing
degrades quietly.

## Verifying it

Because a surface is just an `Element` tree, an A2UI stream is testable
the same way anything else in fenestra is:

```rust,ignore
let rendered = surface.render(&Theme::light());
let image = render_element(rendered.element, &Theme::light(), (480, 640));
assert!(!any_broken(&rendered.notes), "stream degraded: {:?}", rendered.notes);
assert_png_snapshot("tests/snapshots", "checkout_surface", &image);
```

Assert on the notes as well as the pixels. A regression that starts
silently degrading a component shows up in the notes long before it is
visible in a diff.
