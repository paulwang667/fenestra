# fenestra-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) server that lets an
AI assistant render and verify native UIs described as
[fenestra](https://github.com/richer-richard/fenestra) `fenestra/1` JSON.

## Install

```sh
cargo install fenestra-mcp
```

The server speaks MCP over stdio — point your MCP client at the `fenestra-mcp`
binary.

## Tools

- `describe_vocabulary` — the description grammar: every node type with a
  minimal example, and the theme color roles a color may name. Call this
  first.
- `describe_schema` — the formal JSON Schema for a `fenestra/1` description,
  the machine-checkable complement to `describe_vocabulary`.
- `render_ui` — render to a typed access tree, a downscaled preview image, and
  automatic accessibility warnings.
- `query_ui` — find nodes by a semantic selector (role, name, value, or id);
  a miss returns the nearest candidates to guide a retry.
- `interact` — drive scripted interactions (click, type, key, tab, hover,
  wheel, drag) by semantic selector, never coordinates.
- `check_a11y` — theme contrast, labeling of every interactive control, and
  per-text-node APCA + WCAG 2 legibility.
- `focus_order` — the keyboard focus order: the refs a Tab cycle visits, in
  order, honoring a modal focus trap.
- `check_layout` — layout geometry from the real frame: interactive targets
  below the minimum hit size, and signal-bearing nodes clipped off-screen.
- `match_aria_snapshot` — assert an expected accessibility snapshot (partial /
  strict / regex).
- `match_screenshot` — compare against a baseline PNG, pixel by pixel, with an
  optional tolerance, differing-pixel budget, and mask rectangles to ignore.
- `validate` — validate a description without rendering; problems come back
  path-pointed.
- `run_scenario` — drive a description + optional steps through a whole
  bundle of expectations (emitted intents, a11y, aria, screenshot, queries) in
  one pass, asserted against the post-interaction frame.
- `film_ui` — drive optional steps (applied first, so a click can trigger the
  transition to watch), then capture frames with real motion on and compose
  them into one captioned filmstrip. The one tool that turns reduced motion
  off — every other tool stays reduced-motion for deterministic pixels.

Each tool leads with a typed structured result — `query_ui`, `check_a11y`,
`focus_order`, `check_layout`, `match_aria_snapshot`, and `describe_vocabulary`
carry a formal `outputSchema` so a client knows the result shape up front. The
visual tools also attach a downscaled preview image and a `resource_link` to
the full-resolution PNG (a `file://` temp path), so a large image never
bloats the response yet stays one fetch away.

## Where baselines are read from

Two tools take a path to a PNG on disk: `match_screenshot`, and
`run_scenario` through its `expect.screenshot.baseline`. That path arrives
inside a tool call, which means it comes from an agent — and an agent is only
as trustworthy as whatever it last read. So the server reads baselines from
one directory and no further.

By default that directory is the working directory the MCP client launched
the server in, which is the project being worked on. Set
`FENESTRA_MCP_BASELINE_ROOT` to point somewhere else. Paths in a tool call
may be relative to the root or absolute inside it; anything that resolves
outside — including a symlink inside the root pointing out of it — is
refused, and the refusal names the root so a legitimate call can be retried.
A root that is not a readable directory stops the server from starting rather
than silently widening to the default.

The diff image a failed comparison returns draws the *rendered* pixels, never
the baseline's, for the same reason.

## Registry

- MCP Registry name: `mcp-name: io.github.richer-richard/fenestra-mcp`

## License

Licensed under either of MIT or Apache-2.0 at your option.
