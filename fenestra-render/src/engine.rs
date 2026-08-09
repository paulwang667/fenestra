//! The pixel and stateful engine: render a description to PNG, drive it through
//! scripted interactions on the headless harness, and compare against a baseline.
//! Built on `fenestra-shell`, so these are the operations that need a GPU; the
//! structural ops (access tree, query, aria, a11y) come from `fenestra-describe`.

use fenestra_core::{Key, KeyInput, Query, Theme};
use fenestra_describe::dto::{A11yReport, AccessNodeDto, Bounds};
use fenestra_describe::error::DescribeError;
use fenestra_describe::format::Description;
use fenestra_describe::inspect::{self, Selector};
use fenestra_describe::parse::to_element;
use fenestra_describe::state::{Action, StateMap};
use fenestra_shell::testing::{clamp_strip_scale, filmstrip_image};
use fenestra_shell::{Harness, MAX_FILM_INTERVAL_MS, ShellError, try_render_element};
use image::{Rgba, RgbaImage};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

use crate::described_app::DescribedApp;

/// An engine error: a description that did not parse, or an interaction step
/// whose target could not be resolved. Both carry enough to self-correct.
#[derive(Debug)]
pub enum EngineError {
    /// The description did not parse; the path-pointed problems.
    Parse(Vec<DescribeError>),
    /// An interaction step could not resolve its target (a miss or ambiguity).
    /// Carries the step index, the reason, and the current access tree.
    Step {
        /// Zero-based index of the failing step.
        index: usize,
        /// What went wrong.
        message: String,
        /// The accessibility tree at the point of failure, for self-correction.
        tree: String,
    },
    /// A scenario could not be set up: an unrecognized schema tag, an invalid
    /// theme or size, an unreadable screenshot baseline, or a malformed expected
    /// pattern. Distinct from a *verification failure* (a check that ran and did
    /// not pass), which is a normal verify report the caller reads.
    Scenario(String),
    /// The headless renderer is unavailable or the render failed — most
    /// commonly no GPU adapter on the machine (the carried [`ShellError`]
    /// says how to fix that). The description itself was fine.
    Render(ShellError),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(errs) => {
                writeln!(f, "description did not parse:")?;
                for e in errs {
                    writeln!(f, "  {e}")?;
                }
                Ok(())
            }
            Self::Step {
                index,
                message,
                tree,
            } => write!(f, "step {index}: {message}\naccessibility tree:\n{tree}"),
            Self::Scenario(message) => write!(f, "scenario error: {message}"),
            Self::Render(e) => write!(f, "render failed: {e}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// What [`render`] produced.
pub struct RenderOut {
    /// The typed access tree — the agent reads this first.
    pub tree: AccessNodeDto,
    /// The rendered pixels.
    pub png: RgbaImage,
    /// Automatic accessibility warnings (contrast, labeling, per-node legibility).
    pub warnings: A11yReport,
}

/// Renders a description: the typed access tree (first), the pixels, and the
/// automatic accessibility report.
///
/// # Errors
/// [`EngineError::Parse`] when the description does not parse cleanly;
/// [`EngineError::Render`] when the headless renderer is unavailable (e.g.
/// no GPU adapter) or the render fails.
pub fn render(
    desc: &Description,
    theme: &Theme,
    size: (u32, u32),
) -> Result<RenderOut, EngineError> {
    let tree = inspect::access_tree(desc, theme, size).map_err(EngineError::Parse)?;
    let warnings = inspect::check_a11y(desc, theme, size).map_err(EngineError::Parse)?;
    let el = to_element(desc, theme).map_err(EngineError::Parse)?;
    let png = try_render_element(el, theme, size).map_err(EngineError::Render)?;
    Ok(RenderOut {
        tree,
        png,
        warnings,
    })
}

/// One interaction step. Targets are semantic selectors — never coordinates.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    /// Click the matched node.
    Click(Selector),
    /// Right-click the matched node.
    RightClick(Selector),
    /// Double-click the matched node.
    DoubleClick(Selector),
    /// Triple-click the matched node.
    TripleClick(Selector),
    /// Shift-click the matched node.
    ShiftClick(Selector),
    /// Move the pointer over the matched node.
    Hover(Selector),
    /// Commit text to the focused element.
    Type(String),
    /// Press a key chord, e.g. `"enter"`, `"cmd+z"`, `"ctrl+shift+a"`.
    Key(String),
    /// Tab forward `n` times.
    Tab(u32),
    /// Tab backward `n` times.
    ShiftTab(u32),
    /// Scroll the wheel over the matched node.
    Wheel {
        /// The node to scroll.
        target: Selector,
        /// Horizontal delta (positive moves content right).
        #[serde(default)]
        dx: f32,
        /// Vertical delta (positive moves content down).
        dy: f32,
    },
    /// Drag from one node to another. The selectors are boxed: `Selector` grew
    /// with state/range criteria, and two of them by value made this the lone
    /// outsized enum variant.
    Drag {
        /// Press here.
        from: Box<Selector>,
        /// Release here.
        to: Box<Selector>,
    },
    /// Advance the deterministic clock by `ms` milliseconds.
    PumpMs(f64),
}

/// What [`interact`] produced.
pub struct InteractOut {
    /// Intent strings emitted by handlers during the steps (the Elm-level signal).
    pub emitted: Vec<String>,
    /// The access tree after the steps (framework-owned state changes are visible).
    pub tree: AccessNodeDto,
    /// The rendered pixels after the steps, when requested.
    pub png: Option<RgbaImage>,
    /// The runtime state after the steps (bound widgets' values reflect here).
    pub state: StateMap,
}

/// Drives a description through scripted interactions on the headless harness,
/// then captures the emitted intents and the resulting access tree (and pixels,
/// when `want_png`). Selectors resolve strictly; a miss returns the access tree
/// so the caller self-corrects.
///
/// # Errors
/// [`EngineError::Parse`] on a parse error, or [`EngineError::Step`] when a step's
/// target does not resolve to exactly one node.
pub fn interact(
    desc: &Description,
    theme: &Theme,
    size: (u32, u32),
    steps: &[Step],
    want_png: bool,
) -> Result<InteractOut, EngineError> {
    let mut h = drive(desc, theme, size, steps)?;
    // Map the emitted actions: keep the inert author intents (the Elm-level
    // signal); the framework-owned Set actions are already applied to the state.
    let emitted = emitted_intents(&mut h);
    let state = h.app().state().clone();
    let tree = inspect::frame_access_tree(h.frame());
    let png = if want_png { Some(h.render()) } else { None };
    Ok(InteractOut {
        emitted,
        tree,
        png,
        state,
    })
}

/// What [`film`] produced.
pub struct FilmOut {
    /// The individual captured frames, before composition.
    pub frames: Vec<RgbaImage>,
    /// The frames composed into one captioned, bordered filmstrip.
    pub strip: RgbaImage,
    /// The frame count actually captured (`Harness::film` floors at 1 and
    /// ceilings at `fenestra_shell::MAX_FILM_FRAMES`) — may differ from the
    /// request.
    pub frame_count: usize,
    /// The interval actually used between frames, in ms (ceilinged at
    /// [`MAX_FILM_INTERVAL_MS`]) — may differ from the request.
    pub interval_ms: u64,
    /// The per-cell strip scale actually used (clamped, see
    /// [`clamp_strip_scale`]) — may differ from the request.
    pub scale: f32,
}

/// Drives `steps` (applied first — so a click can trigger the transition
/// about to be watched), then captures `frames` renders spaced `interval_ms`
/// apart and composes them into one filmstrip.
///
/// Unlike every other verb in this module (which stays reduced-motion for
/// deterministic pixels), `film` turns real animation on *before* driving
/// anything: the whole point is watching motion play, and a transition a
/// step triggers under reduced motion would already be snapped to its end
/// state by the time capture starts, making the filmstrip static regardless
/// of `frames`/`interval_ms`.
///
/// # Errors
/// [`EngineError::Parse`] on a parse error, [`EngineError::Step`] when a
/// step's target does not resolve, or [`EngineError::Scenario`] when the
/// captured frames can't compose into a strip (an oversized `scale` on a
/// long `frames` request — `Harness::film` always captures at least one
/// frame, so the empty-input case never reaches this path).
pub fn film(
    desc: &Description,
    theme: &Theme,
    size: (u32, u32),
    steps: &[Step],
    frames: usize,
    interval_ms: u64,
    scale: f32,
) -> Result<FilmOut, EngineError> {
    to_element(desc, theme).map_err(EngineError::Parse)?;
    let app = DescribedApp::new(desc.clone(), theme.clone());
    let mut h = Harness::try_new(app, theme.clone(), size).map_err(EngineError::Render)?;
    h.set_reduced_motion(false);
    for (index, step) in steps.iter().enumerate() {
        apply_step(&mut h, step, index)?;
    }
    let captured = h.film(frames, interval_ms);
    let interval_ms = interval_ms.min(MAX_FILM_INTERVAL_MS);
    let scale = clamp_strip_scale(scale);
    let strip = filmstrip_image(&captured, interval_ms, scale)
        .map_err(|e| EngineError::Scenario(format!("cannot compose filmstrip: {e}")))?;
    Ok(FilmOut {
        frame_count: captured.len(),
        frames: captured,
        strip,
        interval_ms,
        scale,
    })
}

/// Validates the description (so a parse error is reported before anything is
/// driven), then runs `steps` on a fresh headless harness and hands back the
/// live harness for inspection — the post-interaction frame, state, and emitted
/// messages. The shared spine of [`interact`] and scenario `verify`.
pub(crate) fn drive(
    desc: &Description,
    theme: &Theme,
    size: (u32, u32),
    steps: &[Step],
) -> Result<Harness<DescribedApp>, EngineError> {
    to_element(desc, theme).map_err(EngineError::Parse)?;
    let app = DescribedApp::new(desc.clone(), theme.clone());
    let mut h = Harness::try_new(app, theme.clone(), size).map_err(EngineError::Render)?;
    for (index, step) in steps.iter().enumerate() {
        apply_step(&mut h, step, index)?;
    }
    Ok(h)
}

/// Drains the harness's emitted messages down to the inert author intents (the
/// Elm-level signal); the framework-owned `Set*` actions are already applied to
/// the runtime state, so they are dropped here.
pub(crate) fn emitted_intents(h: &mut Harness<DescribedApp>) -> Vec<String> {
    h.take_messages()
        .into_iter()
        .filter_map(|a| match a {
            Action::Intent(s) => Some(s),
            Action::SetBool(..) | Action::SetText(..) | Action::SetNumber(..) => None,
        })
        .collect()
}

/// Resolves a selector against the harness's current frame, returning a
/// self-explaining error (with the tree) on a miss or ambiguity.
fn resolve(h: &Harness<DescribedApp>, sel: &Selector, index: usize) -> Result<Query, EngineError> {
    let fail = |message: String| EngineError::Step {
        index,
        message,
        tree: h.frame().access_yaml(),
    };
    let q = sel.to_query().map_err(&fail)?;
    h.frame()
        .try_get(&q)
        .map_err(|e| fail(format!("target [{q}]: {e}")))?;
    Ok(q)
}

/// The most times a single [`Step::Tab`]/[`Step::ShiftTab`] will move focus.
/// Each repeat dispatches an event and rebuilds the frame (re-deriving the whole
/// element tree), so an unbounded `u32` repeat is a multi-billion-iteration
/// hang. A keyboard focus order is tiny — a few hundred stops in even an
/// enormous UI — so a few thousand covers any real cycle many times over while
/// turning a hostile `u32::MAX` into a handful of milliseconds.
const MAX_TAB_REPEAT: u32 = 4_096;

/// Applies one step to the harness.
fn apply_step(h: &mut Harness<DescribedApp>, step: &Step, index: usize) -> Result<(), EngineError> {
    match step {
        Step::Click(s) => {
            let q = resolve(h, s, index)?;
            h.click(&q);
        }
        Step::RightClick(s) => {
            let q = resolve(h, s, index)?;
            h.right_click(&q);
        }
        Step::DoubleClick(s) => {
            let q = resolve(h, s, index)?;
            h.double_click(&q);
        }
        Step::TripleClick(s) => {
            let q = resolve(h, s, index)?;
            h.triple_click(&q);
        }
        Step::ShiftClick(s) => {
            let q = resolve(h, s, index)?;
            h.shift_click(&q);
        }
        Step::Hover(s) => {
            let q = resolve(h, s, index)?;
            h.hover(&q);
        }
        Step::Type(text) => h.type_text(text.clone()),
        Step::Key(spec) => {
            let key = key_from_str(spec).map_err(|message| EngineError::Step {
                index,
                message,
                tree: h.frame().access_yaml(),
            })?;
            h.key(key);
        }
        Step::Tab(n) => {
            for _ in 0..(*n).min(MAX_TAB_REPEAT) {
                h.tab();
            }
        }
        Step::ShiftTab(n) => {
            for _ in 0..(*n).min(MAX_TAB_REPEAT) {
                h.shift_tab();
            }
        }
        Step::Wheel { target, dx, dy } => {
            let q = resolve(h, target, index)?;
            h.wheel_xy(&q, *dx, *dy);
        }
        Step::Drag { from, to } => {
            // Both endpoints resolve strictly (like every other target step) so
            // a missing/ambiguous `to` returns a self-explaining EngineError::Step
            // with the access tree — never a panic in `Frame::get` (which the bare
            // `to.to_query()` used to reach via `h.drag` → `center`).
            let from_q = resolve(h, from, index)?;
            let to_q = resolve(h, to, index)?;
            h.drag(&from_q, &to_q);
        }
        Step::PumpMs(ms) => h.pump(*ms),
    }
    Ok(())
}

/// Parses a key chord like `"enter"`, `"cmd+z"`, or `"ctrl+shift+a"`.
fn key_from_str(spec: &str) -> Result<KeyInput, String> {
    let mut input = KeyInput::plain(Key::Enter);
    let mut key = None;
    for token in spec.split('+') {
        match token.trim().to_lowercase().as_str() {
            "shift" => input.shift = true,
            "ctrl" | "control" => input.ctrl = true,
            "alt" | "option" => input.alt = true,
            "cmd" | "meta" | "super" | "win" => input.meta = true,
            "enter" | "return" => key = Some(Key::Enter),
            "space" => key = Some(Key::Space),
            "escape" | "esc" => key = Some(Key::Escape),
            "left" | "arrowleft" => key = Some(Key::ArrowLeft),
            "right" | "arrowright" => key = Some(Key::ArrowRight),
            "up" | "arrowup" => key = Some(Key::ArrowUp),
            "down" | "arrowdown" => key = Some(Key::ArrowDown),
            "home" => key = Some(Key::Home),
            "end" => key = Some(Key::End),
            "backspace" => key = Some(Key::Backspace),
            "delete" => key = Some(Key::Delete),
            "pageup" => key = Some(Key::PageUp),
            "pagedown" => key = Some(Key::PageDown),
            other => {
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => key = Some(Key::Char(c)),
                    _ => return Err(format!("unknown key token {token:?} in {spec:?}")),
                }
            }
        }
    }
    match key {
        Some(k) => {
            input.key = k;
            Ok(input)
        }
        None => Err(format!("no key in {spec:?} (only modifiers)")),
    }
}

/// What [`match_screenshot`] produced.
pub struct ScreenshotDiff {
    /// True when the differing-pixel fraction is within budget.
    pub ok: bool,
    /// Pixels exceeding the per-channel tolerance (masked pixels excluded).
    pub differing: u64,
    /// Total compared pixels.
    pub total: u64,
    /// Largest per-channel delta seen.
    pub max_delta: u8,
    /// Coordinate of the worst pixel.
    pub worst: (u32, u32),
    /// A diff image (offending pixels in red over the dimmed *render*), when
    /// not ok. Deliberately never the baseline: see [`diff_images`].
    pub diff_png: Option<RgbaImage>,
}

/// Renders the description and compares it to `baseline`, pixel by pixel,
/// allowing `channel_tol` per-channel delta and up to `budget` (a fraction) of
/// pixels to exceed it. Masked rectangles are ignored.
///
/// # Errors
/// [`EngineError::Parse`] when the description does not parse cleanly.
pub fn match_screenshot(
    desc: &Description,
    theme: &Theme,
    size: (u32, u32),
    baseline: &RgbaImage,
    channel_tol: u8,
    budget: f64,
    masks: &[Bounds],
) -> Result<ScreenshotDiff, EngineError> {
    let el = to_element(desc, theme).map_err(EngineError::Parse)?;
    let actual = try_render_element(el, theme, size).map_err(EngineError::Render)?;
    Ok(diff_images(baseline, &actual, channel_tol, budget, masks))
}

/// Where a baseline PNG may be read from.
///
/// The two front doors are in genuinely different positions. The CLI's
/// baseline path comes from the command line, so the person choosing the file
/// is the person running the tool, and confining them to a directory would
/// only be in the way. The MCP server's path arrives inside a tool call from
/// an agent — a party that may be acting on a web page, an issue comment, or
/// a source file it read a moment ago — so there the path is untrusted input
/// and gets a root.
///
/// One type for both, because the alternative is a plain `&Path` at every
/// call site and a comment asking people to remember which door they are.
#[derive(Debug, Clone)]
pub enum BaselineRoot {
    /// Any path this process can read.
    Anywhere,
    /// Only paths resolving inside this directory, which is canonical (see
    /// [`BaselineRoot::within`]).
    Within(PathBuf),
    /// No path at all. The posture for a caller that cannot establish a
    /// root, and for an embedder who wants file baselines off entirely.
    ///
    /// This exists because the obvious spelling of "no root" — an empty
    /// [`Within`](Self::Within) — is the opposite of what it looks like:
    /// every path is inside the empty prefix, so it would permit
    /// everything while reading as if it permitted nothing.
    Nowhere,
}

impl BaselineRoot {
    /// A root confined to `dir`.
    ///
    /// The directory is canonicalized once, here, so that later containment
    /// checks compare like with like — on macOS `/tmp` is a symlink to
    /// `/private/tmp`, and a root that skipped this would reject its own
    /// files.
    ///
    /// # Errors
    /// When `dir` does not exist, cannot be canonicalized, or is not a
    /// directory.
    pub fn within(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref();
        let canon = dir
            .canonicalize()
            .map_err(|e| format!("baseline root {}: {e}", dir.display()))?;
        if !canon.is_dir() {
            return Err(format!(
                "baseline root {} is not a directory",
                dir.display()
            ));
        }
        Ok(Self::Within(canon))
    }

    /// Opens `path` as an RGBA image, refusing anything outside the root.
    ///
    /// # Errors
    /// A ready-to-show message when the path escapes the root, or when the
    /// file cannot be read or decoded.
    pub fn open(&self, path: impl AsRef<Path>) -> Result<RgbaImage, String> {
        let path = path.as_ref();
        let resolved = match self {
            Self::Anywhere => path.to_path_buf(),
            Self::Within(root) => Self::resolve_within(root, path)?,
            Self::Nowhere => return Err(Self::nowhere(path)),
        };
        Ok(image::open(&resolved)
            .map_err(|e| format!("cannot read baseline {}: {e}", path.display()))?
            .into_rgba8())
    }

    /// The refusal from a [`Nowhere`](Self::Nowhere) root.
    fn nowhere(path: &Path) -> String {
        format!(
            "baseline {} cannot be used: this caller has no permitted \
             directory to read baselines from",
            path.display()
        )
    }

    /// Resolves `path` as a *write* target, refusing anything outside the
    /// root.
    ///
    /// Writing needs its own resolver because the file is allowed not to
    /// exist yet, and `canonicalize` answers only for paths that do. So the
    /// parent directory is what gets canonicalized and contained, and the
    /// file name is joined back on afterwards. Blessing a baseline is a CLI
    /// operation today and no MCP tool reaches it — this exists so that
    /// stays true by construction if one ever does, rather than by nobody
    /// having noticed that the write side never grew a root.
    ///
    /// # Errors
    /// A ready-to-show message when the path escapes the root, names no
    /// file, or has no reachable parent directory.
    pub fn resolve_write(&self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let path = path.as_ref();
        let root = match self {
            Self::Anywhere => return Ok(path.to_path_buf()),
            Self::Nowhere => return Err(Self::nowhere(path)),
            Self::Within(root) => root,
        };
        let name = path
            .file_name()
            .ok_or_else(|| format!("baseline {} names no file", path.display()))?;
        let parent = Self::resolve_within(root, path.parent().unwrap_or(Path::new("")))?;
        Ok(parent.join(name))
    }

    /// Resolves `path` against `root`, refusing to leave it. A relative path
    /// is taken from the root; an absolute one is accepted only if it is
    /// already inside it.
    ///
    /// Two passes, and both are load-bearing. The lexical pass runs first and
    /// touches no filesystem, so anything outside the root is refused with a
    /// message that cannot depend on whether the target exists — a refusal
    /// that said "no such file" for one path outside the root and "permission
    /// denied" for another would still answer questions about a disk the
    /// caller is not allowed to read. Only paths that pass it are
    /// canonicalized, and that second pass catches what lexical analysis
    /// cannot: a symlink *inside* the root whose target is outside it.
    ///
    /// The root is canonical, so an absolute path must be spelled that way
    /// too — on macOS `/tmp/x` does not match a root of `/private/tmp`. That
    /// is why the refusal names the root: it is the retry instruction.
    fn resolve_within(root: &Path, path: &Path) -> Result<PathBuf, String> {
        let outside = || {
            format!(
                "baseline {} is outside the permitted root {}",
                path.display(),
                root.display()
            )
        };
        let mut out = if path.is_absolute() {
            PathBuf::new()
        } else {
            root.to_path_buf()
        };
        for comp in path.components() {
            match comp {
                Component::Prefix(p) => out.push(p.as_os_str()),
                Component::RootDir => out.push(Component::RootDir.as_os_str()),
                Component::Normal(c) => out.push(c),
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() {
                        return Err(outside());
                    }
                }
            }
        }
        if !out.starts_with(root) {
            return Err(outside());
        }
        // Neutral wording: this resolver serves both the read and the write
        // side, and "cannot read" would be a lie on one of them.
        let canon = out
            .canonicalize()
            .map_err(|e| format!("baseline {}: {e}", path.display()))?;
        if !canon.starts_with(root) {
            return Err(outside());
        }
        Ok(canon)
    }
}

/// Validates a comparison's parameters before they reach [`diff_images`].
///
/// Diffing itself never panics on a stray input — a `NaN` comparison is
/// simply false, a negative extent matches nothing — but a boundary that
/// accepts them from an untrusted caller (the CLI, the MCP tools, a scenario
/// file) should reject the mistake rather than silently ignore it, so every
/// such boundary calls this first. One validator rather than one per door:
/// the reason `budget` and `channel_tol` are checked here at all is that
/// they were checked *nowhere*, and the pair of them was the whole exploit
/// against the old baseline underlay (see [`diff_images`]).
///
/// # Errors
/// A ready-to-show message for a budget that is not a finite fraction, a
/// tolerance that compares nothing, or (path-pointed, `mask[i].field`) the
/// first non-finite coordinate or negative extent in `masks`.
pub fn validate_diff_params(channel_tol: u8, budget: f64, masks: &[Bounds]) -> Result<(), String> {
    if !budget.is_finite() || !(0.0..=1.0).contains(&budget) {
        return Err(format!(
            "budget must be a finite fraction between 0 and 1, got {budget}"
        ));
    }
    // A per-channel delta cannot exceed 255, so this tolerance passes every
    // pixel of every image. A caller who meant "ignore small differences"
    // wants a number; a caller who meant "compare nothing" wants a mask.
    if channel_tol == u8::MAX {
        return Err(
            "tolerance 255 accepts every pixel, so the comparison checks nothing; \
             use a mask to exclude a region"
                .to_owned(),
        );
    }
    validate_masks(masks)
}

/// The mask half of [`validate_diff_params`].
///
/// # Errors
/// A path-pointed message (`mask[i].field`) for the first non-finite
/// coordinate or negative width/height found.
fn validate_masks(masks: &[Bounds]) -> Result<(), String> {
    for (i, m) in masks.iter().enumerate() {
        for (field, v) in [("x", m.x), ("y", m.y), ("w", m.w), ("h", m.h)] {
            if !v.is_finite() {
                return Err(format!("mask[{i}].{field} must be finite, got {v}"));
            }
        }
        if m.w < 0.0 || m.h < 0.0 {
            return Err(format!(
                "mask[{i}] has a negative size: w={}, h={}",
                m.w, m.h
            ));
        }
    }
    Ok(())
}

/// Whether `(x, y)` lies inside any mask rectangle.
fn masked(x: u32, y: u32, masks: &[Bounds]) -> bool {
    masks.iter().any(|m| {
        let (px, py) = (f64::from(x), f64::from(y));
        px >= m.x && px < m.x + m.w && py >= m.y && py < m.y + m.h
    })
}

/// Compares two images, producing the diff stats and (on failure) a diff image:
/// offending pixels (those whose per-channel delta exceeds `channel_tol`) in red
/// over the dimmed *rendered* image, masked rectangles excluded. `ok` when the
/// differing fraction is within `budget`. Exposed so a caller that already holds
/// the actual pixels (e.g. a driven scenario's post-interaction render) can diff
/// against a baseline without re-rendering.
///
/// The underlay is `actual`, never `golden`, and that is a security property
/// rather than a style choice. A caller may be allowed to *name* a baseline
/// without being allowed to *read* it — that is exactly the MCP server's
/// position, where the path arrives from an agent. Drawing the baseline
/// underneath the markers turned "compare my render against this file" into
/// "hand me the contents of any PNG on this disk", one call, no iteration:
/// a `channel_tol` of 255 marks nothing as differing, so every pixel fell
/// through to the underlay, and a negative `budget` still reported failure,
/// which is what releases the image. Both of those are now rejected at the
/// boundary by [`validate_diff_params`], but the underlay is what makes the
/// leak impossible rather than merely inconvenient.
#[must_use]
pub fn diff_images(
    golden: &RgbaImage,
    actual: &RgbaImage,
    channel_tol: u8,
    budget: f64,
    masks: &[Bounds],
) -> ScreenshotDiff {
    if golden.dimensions() != actual.dimensions() {
        let total = u64::from(actual.width()) * u64::from(actual.height());
        return ScreenshotDiff {
            ok: false,
            differing: total,
            total,
            max_delta: 255,
            worst: (0, 0),
            diff_png: None,
        };
    }
    let total = u64::from(golden.width()) * u64::from(golden.height());
    let mut differing = 0u64;
    let mut max_delta = 0u8;
    let mut worst = (0u32, 0u32);
    let mut diff = RgbaImage::from_pixel(golden.width(), golden.height(), Rgba([0, 0, 0, 255]));
    for (x, y, a) in actual.enumerate_pixels() {
        if masked(x, y, masks) {
            diff.put_pixel(x, y, Rgba([40, 40, 40, 255]));
            continue;
        }
        let g = golden.get_pixel(x, y);
        let mut exceeds = false;
        for c in 0..4 {
            let delta = g.0[c].abs_diff(a.0[c]);
            if delta > max_delta {
                max_delta = delta;
                worst = (x, y);
            }
            if delta > channel_tol {
                exceeds = true;
            }
        }
        if exceeds {
            differing += 1;
            diff.put_pixel(x, y, Rgba([255, 0, 0, 255]));
        } else {
            // The *rendered* pixel, not the baseline's. A caller who can
            // choose the baseline path but not read the file — the MCP
            // server's agent — would otherwise get the file's contents back
            // as an image, which is a way to read any PNG on the disk. The
            // render is the caller's own output, so drawing it leaks
            // nothing, and it is the more useful underlay anyway: the marks
            // land on the thing being diagnosed.
            let p = a.0;
            diff.put_pixel(x, y, Rgba([p[0] / 3, p[1] / 3, p[2] / 3, 255]));
        }
    }
    #[expect(clippy::cast_precision_loss, reason = "image pixel counts are small")]
    let fraction = differing as f64 / total as f64;
    let ok = fraction <= budget;
    ScreenshotDiff {
        ok,
        differing,
        total,
        max_delta,
        worst,
        diff_png: if ok { None } else { Some(diff) },
    }
}

/// Parses a `WxH` size string like `800x600`; `None` means the default
/// 800×600.
///
/// One parser for the CLI, the scenario runner and the MCP server — each
/// carried its own copy, down to the same error message. Note that the
/// renderer additionally clamps to the GPU's maximum texture dimension, so
/// an enormous-but-parseable size comes back smaller than requested.
///
/// # Errors
/// A ready-to-show message when the string is not `WxH`, or when either
/// dimension is zero.
pub fn parse_size(s: Option<&str>) -> Result<(u32, u32), String> {
    let Some(s) = s else {
        return Ok((800, 600));
    };
    let parsed = s
        .split_once(['x', 'X'])
        .and_then(|(w, h)| Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?)));
    match parsed {
        Some((0, _) | (_, 0)) => Err(format!(
            "invalid size {s:?}; both dimensions must be at least 1"
        )),
        Some(size) => Ok(size),
        None => Err(format!("invalid size {s:?}; expected WxH like 800x600")),
    }
}

/// What [`render_a2ui`] produced.
pub struct A2uiRenderOut {
    /// The rendered surface's id.
    pub surface_id: String,
    /// The typed access tree — same shape as [`render`]'s.
    pub tree: AccessNodeDto,
    /// The rendered pixels.
    pub png: RgbaImage,
    /// Fidelity notes from the catalog mapping (empty means every
    /// component and binding mapped cleanly). Each carries a machine-
    /// readable [`fenestra_a2ui::NoteKind`] alongside the prose.
    pub notes: Vec<fenestra_a2ui::Note>,
}

/// Renders an A2UI v0.9 message stream (the open Agent-to-UI standard,
/// <https://a2ui.org>) through the `fenestra-a2ui` catalog mapping: fold
/// the stream, render the surface, and return the typed access tree, the
/// pixels, and the mapping's fidelity notes.
///
/// Multi-surface streams render their first surface (sorted by id);
/// agents drive one surface per stream in practice.
///
/// # Errors
/// [`EngineError::Scenario`] when the stream does not parse or apply;
/// [`EngineError::Render`] when the headless renderer is unavailable.
pub fn render_a2ui(
    stream_json: &str,
    theme: &Theme,
    size: (u32, u32),
) -> Result<A2uiRenderOut, EngineError> {
    let msgs = fenestra_a2ui::parse_stream(stream_json)
        .map_err(|e| EngineError::Scenario(format!("A2UI stream did not parse: {e}")))?;
    let mut client = fenestra_a2ui::Client::new();
    client
        .apply_all(&msgs)
        .map_err(|e| EngineError::Scenario(format!("A2UI stream did not apply: {e}")))?;
    let surface = client
        .surfaces()
        .next()
        .ok_or_else(|| EngineError::Scenario("the stream created no surface".into()))?;
    let rendered = surface.render(theme);
    // One catalog mapping serves both outputs: the tree reads the element
    // by reference, then the same element renders to pixels.
    let mut fonts = fenestra_core::Fonts::embedded();
    let mut state = fenestra_core::FrameState::new();
    state.reduced_motion = true;
    #[expect(clippy::cast_precision_loss, reason = "window sizes fit in f32")]
    let frame = fenestra_core::build_frame(
        &rendered.element,
        theme,
        &mut fonts,
        &mut state,
        (size.0 as f32, size.1 as f32),
        1.0,
    );
    let tree = inspect::frame_access_tree(&frame);
    drop(frame);
    let png = try_render_element(rendered.element, theme, size).map_err(EngineError::Render)?;
    // Three sources, not two. A message belonging to no surface — one with
    // no `surfaceId`, or naming a surface not yet created or already
    // deleted — is recorded on the client, and reading only the surface's
    // notes would drop it again right at the boundary where every real
    // consumer (the MCP tool, the CLI) reads them.
    let mut notes = client.notes().to_vec();
    notes.extend_from_slice(surface.notes());
    notes.extend(rendered.notes);
    Ok(A2uiRenderOut {
        surface_id: surface.id().to_owned(),
        tree,
        png,
        notes,
    })
}

#[cfg(test)]
mod size_tests {
    use super::parse_size;

    #[test]
    fn parses_the_shapes_the_three_front_doors_accept() {
        assert_eq!(parse_size(None), Ok((800, 600)), "the shared default");
        assert_eq!(parse_size(Some("1024x768")), Ok((1024, 768)));
        assert_eq!(parse_size(Some(" 320 X 240 ")), Ok((320, 240)));
    }

    #[test]
    fn rejects_what_cannot_be_rendered() {
        for bad in ["", "800", "800x", "axb", "-1x10", "800x600x400"] {
            assert!(parse_size(Some(bad)).is_err(), "{bad:?} must not parse");
        }
        assert!(
            parse_size(Some("0x600")).is_err(),
            "a zero dimension is never intended; it used to clamp silently to 1px"
        );
    }
}

#[cfg(test)]
mod a2ui_note_tests {
    use fenestra_a2ui::NoteKind;

    /// A stream-level note has to survive the boundary. `Client::notes()`
    /// exists so a message belonging to no surface is not dropped with
    /// `Ok(())` — and this is the one place every real consumer (the MCP
    /// `render_a2ui` tool, the CLI) reads notes from, so a note that stops
    /// here has not been reported at all.
    #[test]
    fn a_stream_level_note_reaches_the_engine_output() {
        let stream = r#"[
          {"version":"v0.9","createSurface":{"surfaceId":"s","catalogId":"basic"}},
          {"version":"v0.9","ping":{}},
          {"version":"v0.9","updateComponents":{"surfaceId":"s","components":[
            {"id":"root","component":"Text","text":"hi"}
          ]}}
        ]"#;
        let msgs = fenestra_a2ui::parse_stream(stream).expect("parses");
        let mut client = fenestra_a2ui::Client::new();
        client.apply_all(&msgs).expect("applies");
        assert!(
            client
                .notes()
                .iter()
                .any(|n| n.kind == NoteKind::UnknownMessage),
            "precondition: the client records it"
        );

        // `render_a2ui` itself needs a GPU, which CI's software adapters
        // provide but a bare unit test should not require. What is being
        // pinned is the note *plumbing*, so assemble the same list the
        // renderer does.
        let surface = client.surfaces().next().expect("surface");
        let rendered = surface.render(&fenestra_core::Theme::light());
        let mut notes = client.notes().to_vec();
        notes.extend_from_slice(surface.notes());
        notes.extend(rendered.notes);
        assert!(
            notes.iter().any(|n| n.kind == NoteKind::UnknownMessage),
            "the stream-level note was dropped at the boundary: {notes:?}"
        );
    }
}
