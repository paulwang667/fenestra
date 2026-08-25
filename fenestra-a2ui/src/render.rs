//! Surface → `Element` rendering: the A2UI basic catalog mapped onto
//! fenestra-kit widgets, with data bindings resolved against the surface
//! data model. Everything that cannot map faithfully renders a labeled
//! placeholder and records a note — silence means fidelity.

use fenestra_core::{Element, TextSize, Theme, Weight, col, div, divider, row, text};
use fenestra_kit::{
    ButtonVariant, button, card, checkbox, field, icon_button, modal, multi_select, select, slider,
    tabs, text_area, text_input,
};
use serde::Deserialize;
use serde_json::Value;

use crate::catalog::{
    Action, Check, Checks, ChildList, ChoiceOption, Component, Dyn, FunctionCall, Kind,
};
use crate::note::{Note, NoteKind};
use crate::surface::Surface;
use crate::{checks, functions};

/// The deepest component chain the renderer follows. True cycles are
/// caught exactly by the render-path stack (see [`render_by_id`]); this
/// cap bounds legitimate-but-absurd nesting so the produced *element*
/// tree stays well inside `fenestra_core::MAX_TREE_DEPTH` (each catalog
/// component lowers to roughly 1–3 element levels: 12 × 3 = 36 < 48).
///
/// It is also the deepest chain the render recursion keeps on a 2 MiB
/// thread — the `std::thread` / tokio blocking-pool default the MCP
/// server renders on — with margin. The cap was 16; after `Element` grew
/// its per-element gesture-handler fields, a 16-level chain measured just
/// over 2 MiB, so it came down to 12, which needs about 1.6 MiB. `renders_at_the_full_depth_cap`
/// in the amplification tests re-measures this; raise it only with a
/// fresh `FENESTRA_PROBE_STACK` measurement.
const MAX_DEPTH: usize = 12;

/// The catalog this build implements, by its canonical spec URL. Streams
/// name it either this way or with the bare id `basic`.
const BASIC_CATALOG_URL: &str = "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json";

/// The most children one child list materializes, template or static.
///
/// Shared by both arms of [`children_of`]. The template arm had it first;
/// giving the static arm its own cap is what keeps one enormous list from
/// spending the whole render's budget in document order and leaving every
/// later sibling of the surface blank.
const MAX_CHILDREN_PER_EXPANSION: usize = 1000;

/// The most children one whole render materializes, from every source.
///
/// Capping each expansion bounds a factor; this bounds the product. Real
/// surfaces do not come close — an eagerly-built tree of ten thousand rows
/// is already past what anyone would put on screen, and nothing here is
/// virtualized — while a nested container over the same list reaches it in
/// three levels and would otherwise keep going.
///
/// This counts *every* child [`children_of`] materializes, not only the
/// ones a template generated. The first cut of this budget charged the
/// template arm alone, which left the cheaper amplifier of the two running
/// free: a static child list needs no data model to expand against, so
/// eleven `Column`s naming the next one three times — under a kilobyte of
/// JSON — built 265 720 components, and the same shape at `MAX_DEPTH`'s
/// full 16 levels with a fan-out of ten is past 10^16. Cycle detection does
/// not fire, because every level is a distinct component id, and
/// `fenestra_core` guards element-tree *depth* rather than breadth, so
/// nothing downstream caught it either. One budget over both arms is what
/// makes the bound hold: two counters can be played against each other by
/// alternating the kinds of child list.
const MAX_RENDERED_CHILDREN: usize = 10_000;

/// Identity for one piece of client-side UI state: which component, and —
/// when it was rendered inside a template expansion — which item.
///
/// A component id alone is not enough. `children_of` renders the same
/// component once per item of a data-model list, so keying a Modal's open
/// flag or an input's local edit by id would make every expansion share one
/// value: opening row three's dialog opens all of them at once, and typing
/// into one row's field types into every row's.
fn ui_key(id: &str, scope: Option<&str>) -> String {
    match scope {
        // U+0001 cannot appear in a JSON Pointer or a sane component id, so
        // no id can be mistaken for a scope boundary.
        Some(scope) => format!("{scope}\u{1}{id}"),
        None => id.to_owned(),
    }
}

/// Shown by a single-selection picker when the model has chosen nothing.
/// The kit's `select` always renders *some* option, so without this the
/// control would assert a choice the user never made.
const UNSELECTED_LABEL: &str = "—";

/// The basic catalog names its 60 icons in Material style
/// (`accountCircle`, `arrowBack`, `payment`, …); the vendored set speaks
/// Lucide's kebab-case vocabulary (`user`, `arrow-left`, `credit-card`, …).
/// This table translates the Material names that have a *faithful* visual
/// counterpart in the vendored set. Names with no faithful glyph
/// (`locationOn`, `moreVert`, `starHalf`, `favoriteOff`, the `volume*`
/// family, …) deliberately have no entry: they render the `[icon: …]`
/// placeholder with an `unknownIcon` note instead of a subtly wrong icon —
/// a full star where the stream asked for a half one is a lie the pixels
/// would keep. A name already in Lucide's vocabulary (or any other
/// custom name) misses the table and is looked up as-is.
const MATERIAL_TO_LUCIDE: &[(&str, &str)] = &[
    ("accountCircle", "user"),
    ("add", "plus"),
    ("arrowBack", "arrow-left"),
    ("arrowForward", "arrow-right"),
    ("attachFile", "link"),
    ("calendarToday", "calendar"),
    ("close", "x"),
    ("delete", "trash-2"),
    ("edit", "pencil"),
    ("event", "calendar-days"),
    ("favorite", "heart"),
    ("home", "house"),
    ("notifications", "bell"),
    ("payment", "credit-card"),
    ("person", "user"),
    ("refresh", "refresh-cw"),
    ("share", "share-2"),
    ("visibility", "eye"),
    ("warning", "triangle-alert"),
];

/// The vendored Lucide name for a catalog icon name, or the name itself
/// when it is not a Material alias.
fn lucide_name_for(name: &str) -> &str {
    MATERIAL_TO_LUCIDE
        .iter()
        .find(|(m, _)| *m == name)
        .map(|(_, l)| *l)
        .unwrap_or(name)
}

/// Messages the rendered surface emits; feed them to [`Surface::handle`].
#[derive(Clone, Debug)]
pub enum A2uiMsg {
    /// Write a string at an absolute data-model path (two-way binding).
    SetString {
        /// Absolute JSON Pointer.
        path: String,
        /// The new value.
        value: String,
    },
    /// Write a boolean at an absolute data-model path.
    SetBool {
        /// Absolute JSON Pointer.
        path: String,
        /// The new value.
        value: bool,
    },
    /// Write a number at an absolute data-model path.
    SetNumber {
        /// Absolute JSON Pointer.
        path: String,
        /// The new value.
        value: f64,
    },
    /// Write a string list at an absolute data-model path.
    SetList {
        /// Absolute JSON Pointer.
        path: String,
        /// The new values.
        values: Vec<String>,
    },
    /// Store a local edit for a literal-valued input (no binding path).
    LocalEdit {
        /// The input's instance key — its component id, plus which
        /// template item it belongs to when it came from one.
        key: String,
        /// The edited value.
        value: Value,
    },
    /// A server-bound action fired (button click).
    Event {
        /// The action name.
        name: String,
        /// Resolved context payload.
        context: Value,
        /// The id of the component that fired the action — what the
        /// client→server action message's `sourceComponentId` requires
        /// (see [`Surface::action_message`]).
        source_id: String,
    },
    /// A local `openUrl` function action.
    OpenUrl(
        /// The URL to open.
        String,
    ),
    /// Open a Modal component.
    OpenModal(
        /// The Modal's instance key.
        String,
    ),
    /// Close a Modal component.
    CloseModal(
        /// The Modal's instance key.
        String,
    ),
    /// Switch a Tabs component to a tab.
    SelectTab {
        /// The Tabs component's instance key.
        key: String,
        /// The new active index.
        index: usize,
    },
    /// Several messages from one interaction, applied in order.
    ///
    /// One click can legitimately mean two things — a Modal trigger that
    /// is also a Button opens the dialog *and* reports its own action to
    /// the agent. fenestra dispatches a press to exactly one element, so
    /// the composition happens here rather than by stacking handlers.
    Many(
        /// The messages, in the order they apply.
        Vec<A2uiMsg>,
    ),
    /// Nothing happens, on purpose — an interaction the catalog defines
    /// but this build cannot carry out. Keeps a control clickable (and
    /// honestly noted) instead of inventing an event the agent never
    /// asked for.
    Ignored,
}

/// What [`Surface::handle`] hands back to the host: the effects the host
/// (agent transport, OS integration) must carry out.
#[derive(Clone, Debug)]
pub enum A2uiSignal {
    /// Dispatch this action event to the agent (the client→server
    /// `action` message; see [`Surface::action_message`], which takes
    /// `source_id` as its `sourceComponentId`).
    Event {
        /// The action name.
        name: String,
        /// Resolved context payload.
        context: Value,
        /// The full data model, when the surface asked to send it.
        data_model: Option<Value>,
        /// The id of the component that fired the action.
        source_id: String,
    },
    /// Open a URL with the platform opener.
    ///
    /// Safe to hand to `open(1)`, `xdg-open`, or the browser, because those
    /// openers launch whichever application registered the scheme, and the
    /// stream that named it is only as trustworthy as whatever the agent
    /// writing it last read.
    ///
    /// Two rules, not one. The scheme must be in [`OPENABLE_SCHEMES`]; and
    /// a `mailto:` must additionally carry only the header fields a
    /// generated link needs (`to`, `cc`, `bcc`, `subject`, `body`,
    /// `in-reply-to`), with no CR or LF in the address or in the values of
    /// the ones that become headers — an allowed scheme
    /// is not an allowed URL, since a mail client that honours an
    /// attachment field will stage a local file the user never chose.
    /// Field names and values are percent-decoded before either check.
    ///
    /// Both are enforced twice: where the renderer resolves the action (the
    /// control renders visible but inert, with a
    /// [`NoteKind::BlockedUrlScheme`] note), and again in
    /// [`Surface::handle`], which refuses to emit this signal at all for a
    /// URL that fails them. The second is what makes the guarantee a
    /// property of this type rather than of the renderer's call graph:
    /// [`A2uiMsg::OpenUrl`] is public, so a host can build one, replay one
    /// from a log, or round-trip one through its own message type.
    OpenUrl(
        /// The URL, scheme-checked.
        String,
    ),
}

/// The URL schemes [`A2uiSignal::OpenUrl`] may carry.
///
/// An allowlist rather than a blocklist, because the set of schemes a
/// desktop will launch is open — every installed application may add one,
/// and none of them are known here. These three are what a link in a
/// generated surface is for.
///
/// A slice, not a fixed-size array: this list is expected to grow (`tel:`
/// and `sms:` are the obvious candidates), and an array bakes its length
/// into the public type, so adding one would be a breaking change for no
/// reason.
///
/// **This is necessary and not sufficient.** A `mailto:` whose scheme is
/// on this list can still be refused, over its header fields — so a host
/// re-validating a URL should call [`is_openable_url`], which is the whole
/// rule, rather than testing membership here and believing it has
/// reproduced the renderer's decision.
pub const OPENABLE_SCHEMES: &[&str] = &["http", "https", "mailto"];

/// The `mailto:` header fields a generated link may carry.
///
/// An allowlist, for the same reason [`OPENABLE_SCHEMES`] is one, and
/// arrived at the same way. The first cut named the two fields known to
/// stage a local file — `attach` and `attachment` — which is a blocklist,
/// and it was wrong twice over: mail clients ship their own spellings
/// (`x-mozilla-attach` and friends), and RFC 6068 writes `hfname` as
/// `*qchar` with `pct-encoded` among the `qchar`s, so `%61ttach` is
/// `attach` by the time a conforming client reads it and matched neither
/// name. Enumerating what a link legitimately needs is finite; enumerating
/// what a mail client might act on is not.
///
/// These are the fields RFC 6068 describes for composing a message. Anything
/// else — including a field this build simply has not heard of — makes the
/// URL unopenable rather than being passed through and hoped about.
const MAILTO_SAFE_FIELDS: &[&str] = &["to", "cc", "bcc", "subject", "body", "in-reply-to"];

/// The subset of [`MAILTO_SAFE_FIELDS`] whose value becomes a message
/// *header*, and therefore may not contain a line break.
///
/// `body` is deliberately absent, and that is not an oversight: RFC 6068
/// §6.1's own worked example is
/// `mailto:infobot@example.com?body=send%20current-issue%0D%0Asend%20index`,
/// where `%0D%0A` is how the spec says to write a newline in a message
/// body. Banning CR/LF everywhere — which the first cut of this check did —
/// refuses a conformant multi-line mail link and tells its author the
/// scheme is unopenable. A body cannot inject a header; it *is* the payload,
/// and everything after the header block belongs to it.
const MAILTO_HEADER_FIELDS: &[&str] = &["to", "cc", "bcc", "subject", "in-reply-to"];

/// Whether `url` is something a host may hand to a platform opener — the
/// renderer's own decision, in full.
///
/// Public because [`OPENABLE_SCHEMES`] on its own is a trap for the host
/// that tries to re-derive this. A scheme test alone accepts
/// `mailto:a@b?%61ttach=/Users/u/.ssh/id_rsa`, which this refuses; the
/// scheme is necessary and not sufficient, and the rest of the rule lives
/// in private constants a caller cannot see. Any host re-validating a URL
/// it replayed, logged, or built itself should call this rather than
/// reimplement it and drift.
///
/// The rules: the scheme must be one of [`OPENABLE_SCHEMES`]; and a
/// `mailto:` may carry only the header fields a generated link needs, with
/// no CR or LF in the address or in the values of the ones that become
/// headers (`body` may contain them; RFC 6068 §6.1 requires that). Names
/// and values are percent-decoded
/// first. A URL with no scheme at all is refused too — `open(1)` and
/// `xdg-open` both treat a bare path as a local file, so a "relative" URL
/// is a `file:` in disguise, and a surface meaning to link to the web can
/// say so.
#[must_use]
pub fn is_openable_url(url: &str) -> bool {
    is_openable(url)
}

/// Whether `url` is something the host may hand to a platform opener.
///
/// A URL with no scheme at all is refused too. `open(1)` and `xdg-open`
/// both treat a bare path as a local file, so a "relative" URL is a `file:`
/// in disguise — and a surface meaning to link to the web can say so.
fn is_openable(url: &str) -> bool {
    // A scheme is `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"` (RFC
    // 3986). Anything before a `:` that does not fit that shape is not a
    // scheme, so the string has none.
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    let well_formed = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !well_formed
        || !OPENABLE_SCHEMES
            .iter()
            .any(|s| scheme.eq_ignore_ascii_case(s))
    {
        return false;
    }
    // The scheme being allowed is not the end of it. A `mailto:` may carry
    // header fields naming a path on the user's disk, which is the same
    // "a stream chose a local file" problem the scheme check exists to
    // stop, one layer in.
    if scheme.eq_ignore_ascii_case("mailto") {
        return mailto_fields_are_safe(rest);
    }
    true
}

/// Whether a `mailto:` carries only things a generated link may carry.
///
/// Two halves, because a `mailto:` has two. The address part (RFC 6068's
/// `to`, everything before `?`) is checked for line breaks, and the query
/// part is checked field by field against [`MAILTO_SAFE_FIELDS`].
///
/// Checking only the query — which the first cut of this did, by discarding
/// the address with `let Some((_, query))` — missed the simpler attack
/// entirely: `mailto:victim@example.com%0D%0Aattach=/path` has no `?` at
/// all, so the function returned `true` without inspecting anything. The
/// address is `pct-encoded`-capable in the same way the fields are, so it
/// smuggles a header just as well and needs the same rule.
///
/// An unparseable or unknown field fails closed: a name that percent-decodes
/// to something containing its own `&` or `=` matches no allowed field, so a
/// stream cannot smuggle a second field inside the first one's name.
fn mailto_fields_are_safe(rest: &str) -> bool {
    let (addr, query) = match rest.split_once('?') {
        Some((addr, query)) => (addr, Some(query)),
        None => (rest, None),
    };
    // The address is a header value like any other.
    if has_line_break(addr) {
        return false;
    }
    let Some(query) = query else {
        return true;
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .all(|pair| {
            let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = percent_decode(raw_name);
            let name = name.trim();
            if !MAILTO_SAFE_FIELDS
                .iter()
                .any(|f| name.eq_ignore_ascii_case(f))
            {
                return false;
            }
            // Naming a safe field is not enough for the ones that become
            // headers: RFC 6068 §7 warns that a client writing them out
            // without sanitizing can be made to emit fields the URL never
            // listed, and a decoded CR or LF in `subject` is how `attach`
            // gets added behind an allowlist that only inspected names.
            // `body` is exempt — see [`MAILTO_HEADER_FIELDS`].
            !MAILTO_HEADER_FIELDS
                .iter()
                .any(|f| name.eq_ignore_ascii_case(f))
                || !has_line_break(raw_value)
        })
}

/// Whether `raw` contains a CR or LF once percent-decoding is applied.
fn has_line_break(raw: &str) -> bool {
    percent_decode(raw).contains(['\r', '\n'])
}

/// Percent-decodes a `mailto:` header field name.
///
/// RFC 6068 writes `hfname` as `*qchar` and includes `pct-encoded` among
/// the `qchar`s, so the name a mail client acts on is the *decoded* one —
/// `%61ttach` is `attach`. Matching the raw text let every encoded spelling
/// through, which is the hole this closes.
///
/// A `%` that does not introduce two hex digits is passed through as a
/// literal, exactly as a lenient client would read it; the result is
/// compared against an allowlist, so anything this decodes oddly is refused
/// rather than admitted. Invalid UTF-8 goes through
/// [`String::from_utf8_lossy`] — a replacement character matches no ASCII
/// field name, which is the direction to fail in.
fn percent_decode(s: &str) -> String {
    /// One hex digit as its value.
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // `hi * 16 + lo` cannot overflow: both are at most 15.
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A rendered surface: the element tree plus render-time fidelity notes.
pub struct Rendered {
    /// The tree, ready for any fenestra runner or headless render.
    pub element: Element<A2uiMsg>,
    /// Render-time notes (unknown components, unresolved calls,
    /// truncations). Empty means every component mapped cleanly.
    pub notes: Vec<Note>,
}

struct Ctx<'a> {
    surface: &'a Surface,
    theme: &'a Theme,
    /// The chain of component ids currently being rendered *as some
    /// Modal's trigger*, innermost last.
    ///
    /// A Button with no `action` renders disabled, which is honest for an
    /// inert button and wrong for a modal trigger — that one opens a
    /// dialog. The kit bakes disabled *styling* into the widget when it is
    /// built (a themed label color, and opacity on solid variants), so
    /// clearing `Element::disabled` afterwards restores hit-testing while
    /// leaving the button painted dead. The decision has to be made before
    /// the button is built, which means knowing here.
    ///
    /// This used to be a set pre-scanned from every component the surface
    /// had ever defined, which answered a subtly different question: "does
    /// *some* Modal name this id?" rather than "is this Modal actually on
    /// screen?". A Modal nothing references — the ordinary state of a
    /// progressively-delivered stream, or of two Modals sharing a trigger
    /// id like `close` — armed a button that no Modal would ever wrap. The
    /// button then came out neither inert (so no note, no disabled paint)
    /// nor clickable, and the surface reported full fidelity for a control
    /// that does nothing. Only the Modal arm renders a trigger, so only it
    /// can arm one.
    armed_triggers: std::cell::RefCell<Vec<String>>,
    /// Set by a Button that rendered *blocked by a failing check* while it
    /// was armed as a Modal's trigger.
    ///
    /// A blocked button is built disabled and then wrapped, with its
    /// message, in a column — and it was that column the Modal armed, so
    /// clicking a control painted dead and marked invalid opened the dialog
    /// anyway. The check has to win: it is the stream's own instruction not
    /// to act yet.
    blocked_trigger: std::cell::Cell<bool>,
    /// How many more children this render may build, from any source.
    ///
    /// [`MAX_CHILDREN_PER_EXPANSION`] bounds one child list; this bounds
    /// the product of every nesting level, static lists included. An
    /// absolute template path is scope-invariant by design, so nesting
    /// templates over the same list multiplies without ever repeating a
    /// component id — which is what cycle detection watches for. Four levels
    /// over a 30-item list is 810 000 elements from half a kilobyte of JSON,
    /// and a static child list does the same thing without needing a data
    /// model at all. See [`MAX_RENDERED_CHILDREN`].
    child_budget: std::cell::Cell<usize>,
    /// Compiled validation patterns, keyed by their source text.
    ///
    /// A template renders its component once per item, so a `regex` check
    /// inside a thousand-row list compiles the same pattern a thousand
    /// times per frame otherwise. `Err` is cached too — a pattern this
    /// engine cannot take does not get cheaper on the second try.
    patterns: std::cell::RefCell<
        std::collections::HashMap<String, Result<regex::Regex, crate::checks::PatternError>>,
    >,
    notes: std::cell::RefCell<Vec<Note>>,
    /// The id chain currently being rendered: exact cycle detection
    /// (`a → b → a` trips on re-entry, not after burning stack).
    path_stack: std::cell::RefCell<Vec<String>>,
}

impl Ctx<'_> {
    fn note(&self, id: &str, kind: NoteKind, detail: impl std::fmt::Display) {
        crate::note::push_bounded(
            &mut self.notes.borrow_mut(),
            Note::new(id, kind, detail.to_string()),
        );
    }

    /// Whether `id` already carries a note of `kind`.
    ///
    /// Used to keep one cause from producing two diagnoses. A control can be
    /// inert for several reasons and the generic "nothing to do here" note is
    /// right for most of them, but not when something more specific has
    /// already said why — an agent branching on [`NoteKind`] should see the
    /// reason, not the reason plus a vaguer restatement of it.
    fn noted(&self, id: &str, kind: NoteKind) -> bool {
        self.notes
            .borrow()
            .iter()
            .any(|n| n.kind == kind && n.component_id == id)
    }

    /// A compiled validation pattern, from the cache or freshly compiled.
    ///
    /// Returns a clone of the compiled `Regex` — `regex::Regex` is an
    /// `Arc` inside, so this is a refcount bump, not a recompile, and it
    /// keeps the `RefCell` borrow from spanning the note that a failure
    /// wants to record.
    fn pattern(&self, pattern: &str) -> Result<regex::Regex, crate::checks::PatternError> {
        self.patterns
            .borrow_mut()
            .entry(pattern.to_owned())
            .or_insert_with(|| checks::compile_pattern(pattern))
            .clone()
    }

    /// Charges one component to the render-wide budget, returning whether
    /// it may be built.
    ///
    /// Called from [`render_by_id`] and nowhere else, because that is the
    /// one function every materialized component passes through. Charging
    /// at the *container* instead — which the first cut of this budget did,
    /// metering only [`children_of`] — bounds nothing, because `Card`,
    /// `Tabs`, `Modal` and `Button` reach their children by calling
    /// `render_by_id` directly. Each of those was an uncharged multiplier
    /// on top of a charged one: a `Card` chain multiplies the ceiling by
    /// its depth, and a `Modal` (trigger *and* content) by two per level.
    /// A bound with four ways around it is not a bound, and the way to stop
    /// writing a fifth is to charge where the work actually happens.
    fn charge_child(&self) -> bool {
        let budget = self.child_budget.get();
        if budget == 0 {
            return false;
        }
        self.child_budget.set(budget - 1);
        true
    }

    /// How many more components this render may materialize.
    ///
    /// Read-only: [`children_of`] uses it to avoid walking a hundred
    /// thousand ids to build a hundred thousand refusals. The charge itself
    /// happens in [`Ctx::charge_child`].
    fn remaining_children(&self) -> usize {
        self.child_budget.get()
    }

    /// Reports children dropped to stay inside the render-wide budget.
    fn note_children_dropped(&self, id: &str, dropped: usize) {
        self.note(
            id,
            NoteKind::Truncated,
            format!(
                "this render has materialized {} of {MAX_RENDERED_CHILDREN} children; \
                 {dropped} more here were dropped (nested containers multiply)",
                MAX_RENDERED_CHILDREN - self.child_budget.get(),
            ),
        );
    }

    /// Reports an enum string the catalog does not define.
    ///
    /// Every one of these falls back to a sensible default, which is
    /// exactly why they need saying: a typo renders as a perfectly
    /// plausible control, and the stream is told it got what it asked for.
    fn note_unknown_variant(&self, id: &str, field: &str, got: &str, used: &str) {
        self.note(
            id,
            NoteKind::InvalidValue,
            format!("`{field}` value {got:?} is not in the catalog; rendered as {used}"),
        );
    }
}

impl Surface {
    /// Reports a `catalogId` this build does not implement.
    ///
    /// The crate implements the v0.9 *basic* catalog, named either bare or
    /// by its spec URL. Anything else still renders — a foreign catalog's
    /// components come out as `unknownComponent` placeholders one by one —
    /// but that per-component safety net says nothing when the other
    /// catalog reuses basic's *names* with different semantics, which is
    /// the case where the surface looks perfect and is not.
    fn note_foreign_catalog(&self, ctx: &Ctx) {
        let Some(catalog) = self.catalog_id() else {
            return;
        };
        // Matched whole, not by suffix. A suffix test accepts
        // `.../v1_5/catalogs/basic/catalog.json` and any host's copy — and a
        // later revision of the *basic* catalog is precisely the case this
        // note exists for, since the component names stay the same while
        // their meanings move.
        let basic = matches!(
            catalog.trim_end_matches('/'),
            "basic" | BASIC_CATALOG_URL | "https://a2ui.org/specification/v0_9/catalogs/basic"
        );
        if !basic {
            ctx.note(
                "",
                NoteKind::UnknownCatalog,
                format!(
                    "surface declares catalog {}; this build implements the v0.9 basic \
                     catalog ({BASIC_CATALOG_URL}), so its components render best-effort",
                    quoted(catalog)
                ),
            );
        }
    }

    /// Renders the surface's component tree. Missing `root` renders an
    /// empty placeholder with a note (progressive streams may simply not
    /// have delivered it yet).
    #[must_use]
    pub fn render(&self, theme: &Theme) -> Rendered {
        let ctx = Ctx {
            surface: self,
            theme,
            armed_triggers: std::cell::RefCell::new(Vec::new()),
            blocked_trigger: std::cell::Cell::new(false),
            child_budget: std::cell::Cell::new(MAX_RENDERED_CHILDREN),
            patterns: std::cell::RefCell::new(std::collections::HashMap::new()),
            notes: std::cell::RefCell::new(Vec::new()),
            path_stack: std::cell::RefCell::new(Vec::new()),
        };
        self.note_foreign_catalog(&ctx);
        let element = if self.components.contains_key("root") {
            render_by_id(&ctx, "root", None, 0)
        } else {
            ctx.note(
                "root",
                NoteKind::MissingComponent,
                "no root component yet (stream incomplete?)",
            );
            col()
        };
        Rendered {
            element,
            notes: ctx.notes.into_inner(),
        }
    }

    /// Applies one rendered-surface message: binding writes and UI state
    /// mutate the surface; agent-facing effects come back as signals.
    ///
    /// Returns every signal the message produced, in order — usually none
    /// or one, but an [`A2uiMsg::Many`] (a Modal trigger that is also a
    /// Button) can produce several.
    ///
    /// # Where this records its notes
    ///
    /// On the *surface* ([`Surface::notes`]), not on the [`Rendered`] value
    /// from the last [`Surface::render`] — the two lists are disjoint and
    /// stay that way on purpose, since render notes are rebuilt every frame
    /// and these accumulate across interactions. A caller checking only
    /// `Rendered::notes` will not see that an `OpenUrl` was refused here.
    /// Read both, as `fenestra_render::render_a2ui` does; `any_broken` over
    /// the concatenation is the honest one-line check.
    #[must_use = "these are the effects the host must carry out; dropping them silently discards \
                  every agent-bound event the interaction produced"]
    pub fn handle(&mut self, msg: A2uiMsg) -> Vec<A2uiSignal> {
        match msg {
            A2uiMsg::Many(msgs) => msgs.into_iter().flat_map(|m| self.handle(m)).collect(),
            A2uiMsg::Ignored => Vec::new(),
            A2uiMsg::SetString { path, value } => {
                self.write(&path, Some(Value::String(value)));
                Vec::new()
            }
            A2uiMsg::SetBool { path, value } => {
                self.write(&path, Some(Value::Bool(value)));
                Vec::new()
            }
            A2uiMsg::SetNumber { path, value } => {
                match serde_json::Number::from_f64(value) {
                    Some(n) => self.write(&path, Some(Value::Number(n))),
                    // `write(_, None)` means "remove this key". An infinite
                    // or NaN value is not a request to delete the binding —
                    // it is a value JSON cannot carry, and deleting on its
                    // behalf destroys the data the control was bound to.
                    None => self.push_note(Note::new(
                        &path,
                        NoteKind::RejectedWrite,
                        format!(
                            "{value} cannot be represented in JSON; the model keeps its previous \
                             value"
                        ),
                    )),
                }
                Vec::new()
            }
            A2uiMsg::SetList { path, values } => {
                self.write(
                    &path,
                    Some(Value::Array(
                        values.into_iter().map(Value::String).collect(),
                    )),
                );
                Vec::new()
            }
            A2uiMsg::LocalEdit { key, value } => {
                self.ui.local_edits.insert(key, value);
                Vec::new()
            }
            A2uiMsg::Event {
                name,
                context,
                source_id,
            } => vec![A2uiSignal::Event {
                name,
                context,
                data_model: self.send_data_model.then(|| self.data.clone()),
                source_id,
            }],
            A2uiMsg::OpenUrl(url) => {
                // Checked again here, not only where the action was
                // resolved. [`A2uiSignal::OpenUrl`] promises its reader that
                // the URL is safe to hand to a platform opener, and that
                // promise is what a host acts on — but `A2uiMsg` is public,
                // so a host holding one it built itself (or replayed from a
                // log, or round-tripped through its own message type) can
                // reach this arm without the renderer ever having looked at
                // the string. One check at the point of construction is a
                // property of today's call graph; a check here is a property
                // of the type.
                if is_openable(&url) {
                    vec![A2uiSignal::OpenUrl(url)]
                } else {
                    self.push_note(Note::new(
                        "",
                        NoteKind::BlockedUrlScheme,
                        format!(
                            "openUrl {} is not a URL this renderer will open (the scheme, or \
                             for mailto: the header fields it carries); the signal \
                             was not emitted",
                            quoted(&url)
                        ),
                    ));
                    Vec::new()
                }
            }
            A2uiMsg::OpenModal(id) => {
                self.ui.open_modals.insert(id);
                Vec::new()
            }
            A2uiMsg::CloseModal(id) => {
                self.ui.open_modals.remove(&id);
                Vec::new()
            }
            A2uiMsg::SelectTab { key, index } => {
                self.ui.active_tabs.insert(key, index);
                Vec::new()
            }
        }
    }

    /// Builds the client→server `action` message for an
    /// [`A2uiSignal::Event`], per the v0.9 `client_to_server` schema.
    /// `timestamp` is caller-supplied (ISO-8601) to keep this crate
    /// clock-free and deterministic.
    #[must_use]
    pub fn action_message(
        &self,
        name: &str,
        source_component_id: &str,
        context: &Value,
        timestamp: &str,
    ) -> Value {
        serde_json::json!({
            "name": name,
            "surfaceId": self.id,
            "sourceComponentId": source_component_id,
            "timestamp": timestamp,
            "context": context,
        })
    }
}

// ── Dynamic-value resolution ──────────────────────────────────────────────

/// The one canonical path joiner: absolute paths stand alone; relative
/// paths resolve under the collection scope, or from the root without one.
/// Reads ([`lookup`]), template item scopes ([`children_of`]), and binding
/// *write* paths all go through here, so a value always reads back from
/// exactly where its two-way binding writes.
fn absolute(path: &str, scope: Option<&str>) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        match scope {
            Some(s) => format!("{s}/{path}"),
            None => format!("/{path}"),
        }
    }
}

fn lookup<'a>(surface: &'a Surface, path: &str, scope: Option<&str>) -> Option<&'a Value> {
    surface.data().pointer(&absolute(path, scope))
}

fn resolve_value(ctx: &Ctx, id: &str, d: &Dyn<String>, scope: Option<&str>) -> String {
    match d {
        Dyn::Lit(s) => s.clone(),
        Dyn::Binding { path } => match lookup(ctx.surface, path, scope) {
            Some(v) => functions::display(v),
            None => {
                ctx.note(
                    id,
                    NoteKind::UnresolvedBinding,
                    format!("binding {path:?} resolves to nothing"),
                );
                String::new()
            }
        },
        Dyn::Call(call) => resolve_call(ctx, id, call, scope),
    }
}

/// A bound boolean input value. Absent stays silently `false` — form
/// values legitimately start unset — but a present non-boolean is always
/// an authoring error and records a note.
fn bound_bool(ctx: &Ctx, id: &str, path: &str, scope: Option<&str>) -> bool {
    match lookup(ctx.surface, path, scope) {
        None => false,
        Some(v) => v.as_bool().unwrap_or_else(|| {
            ctx.note(
                id,
                NoteKind::BindingType,
                format!("binding {path:?} is not a boolean; false"),
            );
            false
        }),
    }
}

/// A bound numeric input value; same note policy as [`bound_bool`].
fn bound_f64(ctx: &Ctx, id: &str, path: &str, scope: Option<&str>, fallback: f64) -> f64 {
    match lookup(ctx.surface, path, scope) {
        None => fallback,
        Some(v) => v.as_f64().unwrap_or_else(|| {
            ctx.note(
                id,
                NoteKind::BindingType,
                format!("binding {path:?} is not a number; {fallback}"),
            );
            fallback
        }),
    }
}

fn resolve_bool(ctx: &Ctx, id: &str, d: &Dyn<bool>, scope: Option<&str>) -> bool {
    // A value slot needs an answer, so an unevaluable condition reads as
    // `false` here — it has already recorded why. Only `checks` can afford
    // the third answer, because a rule that does not apply is a coherent
    // thing for a *rule* to be.
    eval_bool(ctx, id, d, scope, 0).unwrap_or(false)
}

/// The most nested `and`/`or`/`not` levels one condition may use.
///
/// The composition functions take conditions as arguments, so a condition
/// is a tree an agent controls the depth of, evaluated by recursion.
const MAX_CONDITION_DEPTH: usize = 16;

/// A `DynamicBoolean`: a literal, a binding, or one of the catalog's
/// boolean functions. `None` means *this build could not evaluate it*.
///
/// That third answer is the whole design. `true` cannot stand in for "does
/// not apply", because `not` inverts it into `false` — which gates, showing
/// the user a message they cannot act on, over a rule nobody could evaluate
/// in the first place. Whether that happened came down to the parity of the
/// surrounding `not`s. `None` propagates through every composition instead,
/// and a note is recorded wherever it is produced.
///
/// A function call here used to be reported as a type error and read as
/// `false`, which had it backwards — the catalog defines eight functions
/// that return booleans and exist precisely to go in this slot.
fn eval_bool(
    ctx: &Ctx,
    id: &str,
    d: &Dyn<bool>,
    scope: Option<&str>,
    depth: usize,
) -> Option<bool> {
    match d {
        Dyn::Lit(b) => Some(*b),
        Dyn::Binding { path } => Some(bound_bool(ctx, id, path, scope)),
        Dyn::Call(call) => eval_bool_call(ctx, id, call, scope, depth),
    }
}

/// Deserializes a `DynamicBoolean` operand without cloning it: the JSON
/// subtree is borrowed, not copied, which matters when a checked control
/// sits inside a template and every row re-reads the same condition.
fn operand(v: &Value) -> Option<Dyn<bool>> {
    Dyn::<bool>::deserialize(v).ok()
}

/// Reads one argument as a `DynamicBoolean` and evaluates it.
fn arg_bool(
    ctx: &Ctx,
    id: &str,
    call: &FunctionCall,
    key: &str,
    scope: Option<&str>,
    depth: usize,
) -> Option<bool> {
    match call.args.get(key) {
        Some(v) => match operand(v) {
            Some(d) => eval_bool(ctx, id, &d, scope, depth + 1),
            None => {
                ctx.note(
                    id,
                    NoteKind::BindingType,
                    format!(
                        "`{}` argument {key:?} is not a boolean condition; \
                         the check does not gate",
                        call.call
                    ),
                );
                None
            }
        },
        None => {
            ctx.note(
                id,
                NoteKind::InvalidValue,
                format!(
                    "`{}` needs a {key:?} argument; the check does not gate",
                    call.call
                ),
            );
            None
        }
    }
}

/// A bound the stream may or may not have given, and may have given badly.
enum Bound {
    /// The stream omitted it; there is no bound to enforce.
    Absent,
    /// A usable number.
    Given(f64),
    /// Present, and not a number this build can use — an unresolvable
    /// binding, a bool, an object, text that does not parse.
    Unusable,
}

/// A numeric argument as one of those three.
///
/// The distinction is the point: folding "present but unusable" into
/// "absent" silently drops the bound, so `{"call": "length", "min":
/// "eight"}` renders a password field with no minimum length, no note, and
/// `any_broken() == false`. That is the "this form looks validated" failure
/// the note system exists to prevent.
fn arg_bound(ctx: &Ctx, id: &str, call: &FunctionCall, key: &str, scope: Option<&str>) -> Bound {
    if !call.args.contains_key(key) {
        return Bound::Absent;
    }
    match arg_value(ctx, id, &call.args, key, scope) {
        Value::Number(n) => n.as_f64().map_or(Bound::Unusable, Bound::Given),
        Value::String(s) => s.trim().parse().map_or(Bound::Unusable, Bound::Given),
        _ => Bound::Unusable,
    }
}

/// Reads `min`/`max` for one predicate. `None` means at least one of them
/// was given and could not be used, so the caller must not gate.
fn arg_bounds(
    ctx: &Ctx,
    id: &str,
    call: &FunctionCall,
    scope: Option<&str>,
    whole: bool,
) -> Option<(Option<f64>, Option<f64>)> {
    let read = |key: &str| match arg_bound(ctx, id, call, key, scope) {
        Bound::Absent => Some(None),
        // `length` bounds are counts: a negative or fractional one is not a
        // length, and rounding it would enforce a limit nobody wrote.
        Bound::Given(n) if n.is_finite() && (!whole || (n >= 0.0 && n.fract() == 0.0)) => {
            Some(Some(n))
        }
        Bound::Given(_) | Bound::Unusable => {
            ctx.note(
                id,
                NoteKind::InvalidValue,
                format!(
                    "`{}` bound {key:?} is not a usable {}; the check does not gate",
                    call.call,
                    if whole { "count" } else { "number" }
                ),
            );
            None
        }
    };
    // Both are read before either is judged, so a stream with two bad
    // bounds hears about both rather than only the first.
    let (min, max) = (read("min"), read("max"));
    Some((min?, max?))
}

/// A predicate's subject value. `None` means the rule never named one,
/// which is a malformed rule rather than "the value is null" — `required`
/// read the latter as "nothing provided" and blocked the control with no
/// note at all.
fn arg_subject(ctx: &Ctx, id: &str, call: &FunctionCall, scope: Option<&str>) -> Option<Value> {
    if !call.args.contains_key("value") {
        ctx.note(
            id,
            NoteKind::InvalidValue,
            format!(
                "`{}` names no `value` to check; the check does not gate",
                call.call
            ),
        );
        return None;
    }
    Some(arg_value(ctx, id, &call.args, "value", scope))
}

/// The catalog's boolean functions.
///
/// `None` is "this build could not evaluate the rule" and always comes with
/// a note. The rule then does not gate: failing *closed* would show a user a
/// message they cannot satisfy on a control they cannot use, over a rule
/// nobody could evaluate. Failing open with a **broken** note keeps the
/// surface usable and still tells the caller a rule the stream asked for is
/// not being enforced, so `any_broken` fires and nothing passes silently.
fn eval_bool_call(
    ctx: &Ctx,
    id: &str,
    call: &FunctionCall,
    scope: Option<&str>,
    depth: usize,
) -> Option<bool> {
    if depth >= MAX_CONDITION_DEPTH {
        ctx.note(
            id,
            NoteKind::DepthCap,
            format!("condition nests deeper than {MAX_CONDITION_DEPTH}; it does not gate"),
        );
        return None;
    }
    match call.call.as_str() {
        "required" => Some(checks::required(&arg_subject(ctx, id, call, scope)?)),
        "email" => Some(checks::email(&functions::display(&arg_subject(
            ctx, id, call, scope,
        )?))),
        "length" => {
            let value = functions::display(&arg_subject(ctx, id, call, scope)?);
            let (min, max) = arg_bounds(ctx, id, call, scope, true)?;
            #[expect(
                clippy::cast_sign_loss,
                clippy::cast_possible_truncation,
                reason = "arg_bounds accepts only finite, non-negative whole numbers here"
            )]
            Some(checks::length(
                &value,
                min.map(|n| n as u64),
                max.map(|n| n as u64),
            ))
        }
        "numeric" => {
            let value = arg_subject(ctx, id, call, scope)?;
            let (min, max) = arg_bounds(ctx, id, call, scope, false)?;
            Some(checks::numeric(&value, min, max))
        }
        "regex" => {
            let value = functions::display(&arg_subject(ctx, id, call, scope)?);
            let Some(pattern) = call.args.get("pattern").and_then(Value::as_str) else {
                ctx.note(
                    id,
                    NoteKind::InvalidValue,
                    "`regex` needs a string `pattern`; the check does not gate",
                );
                return None;
            };
            match ctx.pattern(pattern) {
                Ok(re) => Some(checks::matches(&re, &value)),
                Err(why) => {
                    note_bad_pattern(ctx, id, "pattern", pattern, &why, "the check does not gate");
                    None
                }
            }
        }
        "not" => Some(!arg_bool(ctx, id, call, "value", scope, depth)?),
        "and" | "or" => {
            let all = call.call == "and";
            let Some(Value::Array(values)) = call.args.get("values") else {
                ctx.note(
                    id,
                    NoteKind::InvalidValue,
                    format!(
                        "`{}` needs a `values` list; the check does not gate",
                        call.call
                    ),
                );
                return None;
            };
            // The catalog requires at least two operands. Below that the
            // result is vacuous rather than meaningful — and `or` over an
            // empty list is vacuously *false*, which gates. A stream that
            // sends a composition before its operands (the same progressive
            // delivery the modal fix was written for) would lock the form
            // with no diagnostic at all.
            if values.len() < 2 {
                ctx.note(
                    id,
                    NoteKind::InvalidValue,
                    format!(
                        "`{}` needs at least two operands, got {}; the check does not gate",
                        call.call,
                        values.len()
                    ),
                );
                return None;
            }
            // Evaluated in full rather than short-circuited: a later operand
            // that cannot be evaluated has a note to record, and stopping
            // early would hide it on exactly the streams where it matters.
            let mut known = Vec::with_capacity(values.len());
            for v in values {
                match operand(v) {
                    Some(d) => known.push(eval_bool(ctx, id, &d, scope, depth + 1)),
                    None => {
                        ctx.note(
                            id,
                            NoteKind::BindingType,
                            format!(
                                "`{}` was given something that is not a condition; \
                                 that operand does not gate",
                                call.call
                            ),
                        );
                        known.push(None);
                    }
                }
            }
            // Operands this build could not evaluate drop out rather than
            // poisoning the whole composition: `and(required(x), unknown())`
            // still enforces `required`, which is more of the stream's
            // intent than enforcing nothing. If none survive there is no
            // answer to give.
            let answered: Vec<bool> = known.into_iter().flatten().collect();
            if answered.is_empty() {
                return None;
            }
            Some(if all {
                answered.iter().all(|b| *b)
            } else {
                answered.iter().any(|b| *b)
            })
        }
        other => {
            ctx.note(
                id,
                NoteKind::UnimplementedFunction,
                format!("boolean function {other:?} is not implemented; the check does not gate"),
            );
            None
        }
    }
}

/// Explains a pattern that would not compile, without guessing at why.
///
/// Blaming every failure on the missing lookaround and backreferences sends
/// an agent to rewrite its *client* when what it actually typed was `[a-`.
/// The engine already knows the difference; [`checks::PatternError`] carries
/// it, and this says only what is true.
fn note_bad_pattern(
    ctx: &Ctx,
    id: &str,
    field: &str,
    pattern: &str,
    why: &crate::checks::PatternError,
    consequence: &str,
) {
    let cause = if why.unsupported_here {
        "; this engine runs in linear time and has no backreferences or lookaround, \
         so a pattern written for a browser client may need rewriting"
    } else {
        "; the pattern itself is malformed"
    };
    ctx.note(
        id,
        NoteKind::InvalidValue,
        format!(
            "{field} {} does not compile ({}){cause}, so {consequence}",
            quoted(pattern),
            why.message
        ),
    );
}

/// The first failing check's message, or `None` when the value is valid.
///
/// Order is the stream's: a control shows one message at a time, and the
/// first rule written is the one an author expects to see first.
fn evaluate_checks(ctx: &Ctx, id: &str, checks: &Checks, scope: Option<&str>) -> Option<String> {
    let mut failure = None;
    for check in &checks.0 {
        match check {
            Check::Rule(rule) => {
                // `None` — could not be evaluated — deliberately does not
                // gate; whatever produced it has already recorded a note.
                if eval_bool(ctx, id, &rule.condition, scope, 0) == Some(false) && failure.is_none()
                {
                    failure = Some(rule.message.clone());
                }
            }
            Check::Malformed(raw) => ctx.note(
                id,
                NoteKind::MalformedComponent,
                format!(
                    "check {} is not a {{condition, message}} rule; it does not gate",
                    quoted(&raw.to_string())
                ),
            ),
        }
    }
    failure
}

/// Wraps a control in its label and, when a check fails, its message.
///
/// One helper for every input in the catalog: `field` already renders a
/// danger-toned error line and the kit's controls already have an invalid
/// ring, so the alternative was five slightly different hand-rolled
/// versions of the same thing.
fn labeled_control(
    label: Option<String>,
    control: Element<A2uiMsg>,
    failure: Option<String>,
    theme: &Theme,
) -> Element<A2uiMsg> {
    match (label, failure) {
        (Some(label), failure) => {
            let mut f = field(label).child(control);
            if let Some(message) = failure {
                f = f.error(message);
            }
            f.into()
        }
        // No label means no `field` to hang an error line off, so the
        // message goes directly beneath rather than being dropped.
        (None, Some(message)) => col().gap(4.0).children((
            control,
            text(message).size(TextSize::Sm).color(theme.danger.text),
        )),
        (None, None) => control,
    }
}

fn resolve_f64(ctx: &Ctx, id: &str, d: &Dyn<f64>, scope: Option<&str>) -> f64 {
    match d {
        Dyn::Lit(n) => *n,
        Dyn::Binding { path } => bound_f64(ctx, id, path, scope, 0.0),
        Dyn::Call(call) => {
            ctx.note(
                id,
                NoteKind::BindingType,
                format!("function {:?} in a numeric slot; 0", call.call),
            );
            0.0
        }
    }
}

fn arg_value(
    ctx: &Ctx,
    id: &str,
    args: &serde_json::Map<String, Value>,
    key: &str,
    scope: Option<&str>,
) -> Value {
    match args.get(key) {
        Some(Value::Object(o)) if o.contains_key("path") => {
            let path = o["path"].as_str().unwrap_or_default();
            lookup(ctx.surface, path, scope)
                .cloned()
                .unwrap_or(Value::Null)
        }
        Some(v) => v.clone(),
        None => {
            let _ = id;
            Value::Null
        }
    }
}

fn resolve_call(ctx: &Ctx, id: &str, call: &FunctionCall, scope: Option<&str>) -> String {
    match call.call.as_str() {
        "formatString" => {
            let template = match call.args.get("value") {
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            interpolate(ctx, id, &template, scope)
        }
        "formatNumber" => {
            let v = arg_value(ctx, id, &call.args, "value", scope);
            functions::format_number(v.as_f64().unwrap_or(0.0))
        }
        "formatCurrency" => {
            let v = arg_value(ctx, id, &call.args, "value", scope);
            let currency = call
                .args
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD");
            functions::format_currency(v.as_f64().unwrap_or(0.0), currency)
        }
        "formatDate" => {
            let v = arg_value(ctx, id, &call.args, "value", scope);
            let pattern = call
                .args
                .get("format")
                .and_then(Value::as_str)
                .unwrap_or("yyyy-MM-dd");
            let raw = functions::display(&v);
            functions::format_date(&raw, pattern).unwrap_or_else(|| {
                ctx.note(
                    id,
                    NoteKind::InvalidValue,
                    format!("formatDate could not parse {raw:?}"),
                );
                raw
            })
        }
        "pluralize" => {
            let v = arg_value(ctx, id, &call.args, "value", scope);
            let zero = call.args.get("zero").and_then(Value::as_str);
            let one = call.args.get("one").and_then(Value::as_str);
            let other = call.args.get("other").and_then(Value::as_str).unwrap_or("");
            functions::pluralize(v.as_f64().unwrap_or(0.0), zero, one, other)
        }
        other => {
            ctx.note(
                id,
                NoteKind::UnimplementedFunction,
                format!("function {other:?} is not implemented"),
            );
            format!("[{other}]")
        }
    }
}

/// `${…}` interpolation for `formatString`: absolute/relative data paths
/// resolve; `\${` renders a literal `${` (the spec's escape); nested
/// function-call syntax (`${fn(…)}`) is beyond this pass and resolves to
/// nothing, with a note. Braces balance, so a nested expression is
/// skipped whole rather than split at its first `}`.
fn interpolate(ctx: &Ctx, id: &str, template: &str, scope: Option<&str>) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        // The spec's escape: a backslash directly before `${` makes it
        // literal — the backslash is consumed, the `${` is kept.
        if start > 0 && rest.as_bytes()[start - 1] == b'\\' {
            out.push_str(&rest[..start - 1]);
            out.push_str("${");
            rest = &rest[start + 2..];
            continue;
        }
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        // Find the matching close brace, counting nested `${`/`}` pairs
        // (an escaped `\${` inside does not open a nested expression).
        let mut depth = 1_usize;
        let mut end = None;
        let bytes = after.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                // An escaped `\${` is literal text, not a nested
                // expression: do not open a brace level for it.
                b'{' if !(i >= 2 && bytes[i - 1] == b'$' && bytes[i - 2] == b'\\') => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let Some(end) = end else {
            out.push_str(&rest[start..]);
            return out;
        };
        let expr = &after[..end];
        if expr.contains('(') {
            ctx.note(
                id,
                NoteKind::UnimplementedFunction,
                format!("nested call in formatString template ({expr:?}) is not implemented"),
            );
        } else {
            match lookup(ctx.surface, expr, scope) {
                Some(v) => out.push_str(&functions::display(v)),
                None => ctx.note(
                    id,
                    NoteKind::UnresolvedBinding,
                    format!("template path {expr:?} resolves to nothing"),
                ),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

// ── Component rendering ───────────────────────────────────────────────────

/// Whether any descendant (not the element itself) would take a press.
///
/// Asks [`Element::takes_press`] — the dispatcher's own question — rather
/// than checking for a click handler. A focusable descendant with no
/// `on_click` (a TextField, a select) still wins the press and still
/// swallows the modal-open, so a narrower check would miss exactly the
/// cases worth warning about.
fn has_pressable_descendant(el: &Element<A2uiMsg>) -> bool {
    el.children_ref()
        .iter()
        .any(|c| c.takes_press() || has_pressable_descendant(c))
}

/// Turns a Modal's rendered trigger into something that actually opens the
/// modal.
///
/// The obvious version — wrap the trigger in a clickable `div` — cannot
/// work, because fenestra hands a press to the *deepest* enabled
/// interactive node and stops: a Button trigger wins the press and the
/// wrapper never hears it. So the open message is composed onto the
/// trigger element itself, keeping whatever the trigger already did
/// ([`A2uiMsg::Many`]), and clearing the disabled flag an actionless
/// Button would otherwise carry — a modal trigger is never a dead control.
///
/// A trigger that *contains* its own interactive child is the one shape
/// this cannot rescue; that child still wins the press. Say so rather than
/// leave a dialog that opens only when you miss the button inside it.
fn open_modal_trigger(
    ctx: &Ctx,
    id: &str,
    key: &str,
    trigger: Element<A2uiMsg>,
) -> Element<A2uiMsg> {
    if has_pressable_descendant(&trigger) {
        ctx.note(
            id,
            NoteKind::Unreachable,
            "the modal trigger contains its own interactive child, which takes the press; \
             clicking that child will not open the dialog",
        );
    }
    let open = A2uiMsg::OpenModal(key.to_owned());
    let composed = match trigger.click_msg().cloned() {
        Some(existing) => A2uiMsg::Many(vec![existing, open]),
        None => open,
    };
    trigger.disabled(false).on_click(composed)
}

/// Reports a catalog field that parsed and then went nowhere.
///
/// Each of these still renders something sensible, so they are approximate
/// rather than broken — but staying quiet would tell the stream it got what
/// it asked for, which is the one thing this crate promises not to do.
fn note_unhonored(ctx: &Ctx, id: &str, field: &str, instead: &str) {
    ctx.note(
        id,
        NoteKind::Unsupported,
        format!("`{field}` is not honored yet; {instead}"),
    );
}

/// TextField's `validationRegexp`, as a failure message or nothing.
///
/// The catalog gives no message for this one — unlike a `checks` rule,
/// which carries its own — so the field says what is wrong in the plainest
/// terms available.
fn pattern_failure(ctx: &Ctx, id: &str, pattern: Option<&str>, current: &str) -> Option<String> {
    let pattern = pattern?;
    match ctx.pattern(pattern) {
        Ok(re) => (!checks::matches(&re, current)).then(|| "Invalid format.".to_owned()),
        Err(why) => {
            note_bad_pattern(
                ctx,
                id,
                "validationRegexp",
                pattern,
                &why,
                "the field is not validated",
            );
            None
        }
    }
}

/// A short quoted form for note prose, so a page-long data URI cannot bury
/// the rest of the note.
fn quoted(s: &str) -> String {
    format!("{:?}", truncate_label(s, 60))
}

/// Trims a placeholder label to a displayable length (char-safe).
fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

fn placeholder<Msg: 'static>(label: String, theme: &Theme) -> Element<Msg> {
    let border = theme.border_subtle;
    let muted = theme.text_muted;
    div()
        .p(8.0)
        .rounded(4.0)
        .bg(theme.surface)
        .child(text(label).size(TextSize::Sm).color(muted))
        .border(1.0, border)
}

fn render_by_id(ctx: &Ctx, id: &str, scope: Option<&str>, depth: usize) -> Element<A2uiMsg> {
    // Charged first, before every early return below, because each of those
    // returns builds a placeholder — and a placeholder is an `Element` and
    // two `String`s, which is work an untrusted stream can ask for. Charging
    // after them (the first cut of this) left the bound bypassable roughly
    // a thousandfold: a `Column` naming one undefined id a thousand times
    // costs one charge and builds a thousand `[missing: ..]` placeholders,
    // and every charged component can host another such list. The same free
    // multiplier sat behind the cycle and depth-cap returns. What the budget
    // has to bound is *calls that build something*, which is all of them.
    if !ctx.charge_child() {
        ctx.note(
            id,
            NoteKind::Truncated,
            format!(
                "this render reached its {MAX_RENDERED_CHILDREN}-component budget; \
                 {id:?} and anything below it were not built (nested containers multiply)"
            ),
        );
        return placeholder(format!("[budget: {id}]"), ctx.theme);
    }
    if ctx.path_stack.borrow().iter().any(|p| p == id) {
        ctx.note(
            id,
            NoteKind::ReferenceCycle,
            "reference cycle detected; rendering a placeholder",
        );
        return placeholder(format!("[cycle: {id}]"), ctx.theme);
    }
    if depth > MAX_DEPTH {
        ctx.note(
            id,
            NoteKind::DepthCap,
            "component chain exceeds the depth cap; rendering a placeholder",
        );
        return placeholder(format!("[deep: {id}]"), ctx.theme);
    }
    let Some(component) = ctx.surface.components.get(id) else {
        ctx.note(
            id,
            NoteKind::MissingComponent,
            "referenced component is not defined",
        );
        return placeholder(format!("[missing: {id}]"), ctx.theme);
    };
    ctx.path_stack.borrow_mut().push(id.to_owned());
    let el = render_component(ctx, component, scope, depth);
    ctx.path_stack.borrow_mut().pop();
    match component.weight {
        #[expect(clippy::cast_possible_truncation, reason = "flex weights are small")]
        Some(w) if w > 0.0 => el.grow_by(w as f32),
        _ => el,
    }
}

fn children_of(
    ctx: &Ctx,
    id: &str,
    list: &ChildList,
    scope: Option<&str>,
    depth: usize,
) -> Vec<Element<A2uiMsg>> {
    match list {
        ChildList::Static(ids) => {
            // The same two caps a template expansion gets, and for the same
            // reasons. The per-expansion one keeps a single enormous list
            // from spending the whole render's budget in document order and
            // leaving every later sibling blank; the render-wide one bounds
            // the product across nesting levels. Only the first is applied
            // here — the budget itself is charged per component in
            // `render_by_id`; this merely avoids walking a list to build
            // refusals nobody can see.
            if ids.len() > MAX_CHILDREN_PER_EXPANSION {
                ctx.note(
                    id,
                    NoteKind::Truncated,
                    format!(
                        "{} static children exceed the cap ({MAX_CHILDREN_PER_EXPANSION}); \
                         extra children dropped",
                        ids.len()
                    ),
                );
            }
            let wanted = ids.len().min(MAX_CHILDREN_PER_EXPANSION);
            let allowed = wanted.min(ctx.remaining_children());
            if allowed < wanted {
                ctx.note_children_dropped(id, wanted - allowed);
            }
            ids[..allowed]
                .iter()
                .map(|cid| render_by_id(ctx, cid, scope, depth + 1))
                .collect()
        }
        ChildList::Template { component_id, path } => {
            let Some(Value::Array(items)) = lookup(ctx.surface, path, scope) else {
                ctx.note(
                    id,
                    NoteKind::BindingType,
                    format!("template path {path:?} is not a list"),
                );
                return Vec::new();
            };
            if items.len() > MAX_CHILDREN_PER_EXPANSION {
                ctx.note(
                    id,
                    NoteKind::Truncated,
                    format!(
                        "{} template items exceed the cap ({MAX_CHILDREN_PER_EXPANSION}); \
                         extra items dropped",
                        items.len()
                    ),
                );
            }
            // Per-expansion cap, then the render-wide one: nesting templates
            // over the same list multiplies, and each level alone is
            // perfectly reasonable.
            let wanted = items.len().min(MAX_CHILDREN_PER_EXPANSION);
            let allowed = wanted.min(ctx.remaining_children());
            if allowed < wanted {
                ctx.note_children_dropped(id, wanted - allowed);
            }
            // The canonical join: an absolute template path stays absolute
            // even inside a collection scope (a naive `{scope}/{path}` join
            // used to corrupt it into a `//` pointer).
            let base = absolute(path, scope);
            (0..allowed)
                .map(|i| {
                    let item_scope = format!("{base}/{i}");
                    render_by_id(ctx, component_id, Some(&item_scope), depth + 1)
                })
                .collect()
        }
    }
}

fn apply_flex(
    mut el: Element<A2uiMsg>,
    justify: Option<&str>,
    align: Option<&str>,
    id: &str,
    ctx: &Ctx,
) -> Element<A2uiMsg> {
    el = match justify {
        Some("center") => el.justify_center(),
        Some("end") => el.justify_end(),
        Some("spaceBetween") => el.justify_between(),
        Some("spaceAround" | "spaceEvenly") => {
            ctx.note(
                id,
                NoteKind::Approximated,
                "spaceAround/spaceEvenly approximate as spaceBetween",
            );
            el.justify_between()
        }
        Some("stretch") | Some("start") | None => el,
        Some(other) => {
            ctx.note(
                id,
                NoteKind::InvalidValue,
                format!("unknown justify {other:?}"),
            );
            el
        }
    };
    match align {
        Some("center") => el.items_center(),
        Some("end") => el.items_end(),
        Some("start") => el.items_start(),
        Some("stretch") | None => el,
        Some(other) => {
            ctx.note(
                id,
                NoteKind::InvalidValue,
                format!("unknown align {other:?}"),
            );
            el
        }
    }
}

/// Renders one component.
///
/// Split in two, and the split is load-bearing rather than cosmetic. This
/// function holds only the kinds that recurse into children; every leaf
/// kind lives in [`render_leaf`], which is `#[inline(never)]` so that its
/// locals get a frame of their own.
///
/// The reason is that `render_by_id` -> `render_component` ->
/// `children_of` -> `render_by_id` is a recursive cycle, so whatever this
/// frame costs is paid once per level of component nesting. With all
/// nineteen kinds in one `match`, an unoptimized build gives the frame
/// room for every arm's locals at once — the arms do not share slots
/// without optimization — and the biggest arms (`ChoicePicker`, `Slider`,
/// `DateTimeInput`) are leaves that can never be on the recursive path at
/// all. Measured on a 2 MiB stack (the `std::thread` and tokio
/// blocking-pool default, which is what the MCP server renders on), that
/// arrangement overflowed at **seven** levels of nesting — well under this
/// module's own `MAX_DEPTH`, and well under what an ordinary surface
/// nests: a Card in a List in a Tab is already half of it. The overflow
/// aborts the process rather than unwinding, so it took the whole server
/// with it and no note or error could describe what happened.
///
/// Keeping the leaves off the recursive path is what buys the depth back.
/// If a new *container* kind is added, it belongs here; if a new leaf is
/// added, it belongs in `render_leaf`, and putting it in the wrong one is
/// a stack regression rather than a compile error — which is what
/// `renders_at_the_full_depth_cap` is watching for.
fn render_component(
    ctx: &Ctx,
    component: &Component,
    scope: Option<&str>,
    depth: usize,
) -> Element<A2uiMsg> {
    let id = component.id.as_str();
    match &component.kind {
        Kind::Row {
            children,
            justify,
            align,
        } => {
            let kids = children_of(ctx, id, children, scope, depth);
            apply_flex(
                row().gap(8.0).children(kids),
                justify.as_deref(),
                align.as_deref(),
                id,
                ctx,
            )
        }
        Kind::Column {
            children,
            justify,
            align,
        } => {
            let kids = children_of(ctx, id, children, scope, depth);
            apply_flex(
                col().gap(8.0).children(kids),
                justify.as_deref(),
                align.as_deref(),
                id,
                ctx,
            )
        }
        Kind::List {
            children,
            direction,
            align,
        } => {
            let kids = children_of(ctx, id, children, scope, depth);
            if let Some(d) = direction.as_deref()
                && !matches!(d, "horizontal" | "vertical")
            {
                ctx.note_unknown_variant(id, "direction", d, "a vertical list");
            }
            let horizontal = direction.as_deref() == Some("horizontal");
            let el = if horizontal {
                row().gap(8.0).children(kids).scroll_x()
            } else {
                col().gap(8.0).children(kids).scroll_y()
            };
            apply_flex(el.id(id), None, align.as_deref(), id, ctx)
        }
        Kind::Card { child } => card()
            .child(render_by_id(ctx, child, scope, depth + 1))
            .p(16.0),
        Kind::Tabs { tabs: items } => {
            let labels: Vec<String> = items
                .iter()
                .map(|t| resolve_value(ctx, id, &t.title, scope))
                .collect();
            let tabs_key = ui_key(id, scope);
            let active = ctx
                .surface
                .ui
                .active_tabs
                .get(&tabs_key)
                .copied()
                .unwrap_or(0)
                .min(items.len().saturating_sub(1));
            let strip = tabs(active, labels, move |index| A2uiMsg::SelectTab {
                key: tabs_key.clone(),
                index,
            });
            let mut container = col().gap(8.0).child(strip);
            if let Some(tab) = items.get(active) {
                container = container.child(render_by_id(ctx, &tab.child, scope, depth + 1));
            }
            container
        }
        Kind::Modal { trigger, content } => {
            let modal_key = ui_key(id, scope);
            let open = ctx.surface.ui.open_modals.contains(&modal_key);
            // Arm the trigger only while it is genuinely being rendered as
            // one, so a Modal nothing reaches cannot vouch for a button it
            // will never wrap. Popped straight after: a trigger that is
            // itself inside another Modal's trigger must not stay armed for
            // its siblings.
            ctx.armed_triggers.borrow_mut().push(trigger.clone());
            ctx.blocked_trigger.set(false);
            let trigger_el = render_by_id(ctx, trigger, scope, depth + 1);
            ctx.armed_triggers.borrow_mut().pop();
            // A trigger whose own checks fail stays as rendered: disabled,
            // showing why. Arming it would hand the press to the wrapper
            // around the dead button and open the dialog regardless.
            let opener = if ctx.blocked_trigger.replace(false) {
                trigger_el
            } else {
                open_modal_trigger(ctx, id, &modal_key, trigger_el)
            };
            if open {
                col().children((
                    opener,
                    modal("")
                        .child(render_by_id(ctx, content, scope, depth + 1))
                        .on_close(A2uiMsg::CloseModal(modal_key.clone())),
                ))
            } else {
                opener
            }
        }
        Kind::Button {
            child,
            variant,
            action,
            checks,
        } => {
            let failure = evaluate_checks(ctx, id, checks, scope);
            // Extract a text label when the child is a Text component; any
            // other child renders inside an icon button.
            let child_component = ctx.surface.components.get(child);
            let label = match child_component.map(|c| &c.kind) {
                Some(Kind::Text { text: content, .. }) => {
                    Some(resolve_value(ctx, id, content, scope))
                }
                _ => None,
            };
            let kit_variant = match variant.as_deref() {
                Some("primary") => ButtonVariant::Primary,
                Some("borderless") => ButtonVariant::Ghost,
                other => {
                    if let Some(v) = other {
                        ctx.note_unknown_variant(id, "variant", v, "a secondary button");
                    }
                    ButtonVariant::Secondary
                }
            };
            let msg = action.as_ref().map(|a| action_msg(ctx, id, a, scope));
            // Only the Modal currently rendering *this* component as its
            // trigger counts. `last()` is the innermost one, which is the
            // Modal that will wrap what we are building right now.
            let opens_a_modal = ctx
                .armed_triggers
                .borrow()
                .last()
                .is_some_and(|armed| armed == id);
            // An action that resolved to nothing leaves the button just as
            // dead as no action at all — `Ignored` is what an unimplemented
            // function or an unresolvable openUrl becomes. Both answers to
            // "can this button do anything?" have to be the same, or the
            // note below is true of one path and a lie about the other.
            let inert = !opens_a_modal && matches!(msg, None | Some(A2uiMsg::Ignored));
            // A failing check is the whole point of putting one on a
            // button: it must not carry out its action. Decided here, before
            // the widget is built, because the kit bakes disabled styling in
            // at build time.
            let blocked = failure.is_some();
            if blocked && opens_a_modal {
                ctx.blocked_trigger.set(true);
            }
            // A blocked URL scheme already recorded *why* this button does
            // nothing, and it is not "no action it can carry out" — the
            // action was understood and deliberately refused. Two Broken
            // notes for one cause, the second of them untrue, is worse than
            // one.
            if inert && !ctx.noted(id, NoteKind::BlockedUrlScheme) {
                ctx.note(
                    id,
                    NoteKind::Unreachable,
                    "button has no action it can carry out and opens nothing; rendered as a \
                         disabled control",
                );
            }
            match label {
                Some(label) => {
                    let mut b = button(label).variant(kit_variant);
                    // A modal trigger has something to do even without an
                    // action of its own, so it must not be *built* disabled
                    // — see `Ctx::armed_triggers`.
                    if inert || blocked {
                        b = b.disabled(true);
                    } else if let Some(m) = msg {
                        b = b.on_click(m);
                    }
                    labeled_control(None, b.into(), failure, ctx.theme)
                }
                None => {
                    let inner = render_by_id(ctx, child, scope, depth + 1);
                    let mut b = icon_button(inner);
                    // Same rule as the labeled branch — an inert button
                    // must *look* inert, or the note above is a lie and the
                    // user presses a live-looking control that does nothing.
                    if inert || blocked {
                        b = b.disabled(true);
                    } else if let Some(m) = msg {
                        b = b.on_click(m);
                    }
                    labeled_control(None, b.into(), failure, ctx.theme)
                }
            }
        }
        // Every other kind is a leaf: it renders no children, so it can
        // never be on the recursive path, and its locals do not belong in
        // a frame that is paid for once per level.
        _ => render_leaf(ctx, component, scope),
    }
}

/// The component kinds that render no children.
///
/// `#[inline(never)]` on purpose: see [`render_component`]. Inlined back
/// into the dispatcher, this function's locals would rejoin the recursive
/// frame and undo the fix.
#[inline(never)]
fn render_leaf(ctx: &Ctx, component: &Component, scope: Option<&str>) -> Element<A2uiMsg> {
    let id = component.id.as_str();
    let theme = ctx.theme;
    match &component.kind {
        Kind::Text {
            text: content,
            variant,
        } => {
            let resolved = resolve_value(ctx, id, content, scope);
            match variant.as_deref() {
                Some("h1") => text(resolved).size_px(28.0).weight(Weight::Semibold),
                Some("h2") => text(resolved).size_px(22.0).weight(Weight::Semibold),
                Some("h3") => text(resolved).size_px(18.0).weight(Weight::Semibold),
                Some("h4") => text(resolved).size_px(16.0).weight(Weight::Medium),
                Some("h5") => text(resolved).size_px(14.0).weight(Weight::Medium),
                Some("caption") => text(resolved).size(TextSize::Xs).color(theme.text_muted),
                // Body text supports simple Markdown per the catalog docs.
                // `body` is the catalog's default variant and a conforming
                // stream may write it explicitly — same arm as the field
                // being absent, and no note.
                None | Some("body") => fenestra_markdown::markdown(resolved).into(),
                Some(v) => {
                    ctx.note_unknown_variant(id, "variant", v, "body text");
                    fenestra_markdown::markdown(resolved).into()
                }
            }
        }
        Kind::Image {
            url,
            description,
            variant,
            fit,
        } => {
            if fit.is_some() {
                note_unhonored(
                    ctx,
                    id,
                    "fit",
                    "the placeholder uses the variant's own size",
                );
            }
            // Deterministic headless renders never fetch the network: a
            // labeled placeholder stands in, sized by the variant hint.
            let src = resolve_value(ctx, id, url, scope);
            let desc = description
                .as_ref()
                .map(|d| resolve_value(ctx, id, d, scope))
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| src.clone());
            ctx.note(
                id,
                NoteKind::NetworkAsset,
                format!("image {} renders as a labeled placeholder", quoted(&src)),
            );
            let (w, h) = match variant.as_deref() {
                Some("icon") => (24.0, 24.0),
                Some("avatar") => (40.0, 40.0),
                Some("smallFeature") => (80.0, 80.0),
                Some("largeFeature") => (240.0, 180.0),
                Some("header") => (320.0, 120.0),
                other => {
                    if let Some(v) = other {
                        ctx.note_unknown_variant(id, "variant", v, "the default 160x120 box");
                    }
                    (160.0, 120.0)
                }
            };
            let short = truncate_label(&desc, 36);
            let el = div()
                .w(w)
                .h(h)
                .rounded(if variant.as_deref() == Some("avatar") {
                    w / 2.0
                } else {
                    4.0
                })
                .bg(theme.surface)
                .border(1.0, theme.border_subtle)
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(
                    text(format!("[img: {short}]"))
                        .size(TextSize::Xs)
                        .color(theme.text_muted),
                );
            el.label(desc)
        }
        Kind::Icon { name } => {
            let name = resolve_value(ctx, id, name, scope);
            match fenestra_kit::icons::lucide::by_name(lucide_name_for(&name)) {
                Some(icon) => icon.label(name),
                None => {
                    ctx.note(
                        id,
                        NoteKind::UnknownIcon,
                        format!("icon {name:?} is not in the vendored Lucide set"),
                    );
                    placeholder(format!("[icon: {name}]"), theme)
                }
            }
        }
        Kind::Video { url } => {
            let url = resolve_value(ctx, id, url, scope);
            ctx.note(
                id,
                NoteKind::NetworkAsset,
                format!("video {} renders as a labeled placeholder", quoted(&url)),
            );
            placeholder(format!("[video: {}]", truncate_label(&url, 48)), theme)
        }
        Kind::AudioPlayer { url, description } => {
            let src = resolve_value(ctx, id, url, scope);
            let label = description
                .as_ref()
                .map(|d| resolve_value(ctx, id, d, scope))
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| src.clone());
            ctx.note(
                id,
                NoteKind::NetworkAsset,
                format!("audio {} renders as a labeled placeholder", quoted(&src)),
            );
            placeholder(format!("[audio: {}]", truncate_label(&label, 48)), theme)
        }
        Kind::Divider { axis } => match axis.as_deref() {
            Some("vertical") => div().w(1.0).h_full().bg(theme.border_subtle),
            Some("horizontal") | None => divider(),
            Some(other) => {
                ctx.note_unknown_variant(id, "axis", other, "a horizontal rule");
                divider()
            }
        },
        Kind::TextField {
            label,
            value,
            variant,
            validation_regexp,
            checks,
        } => {
            let label = resolve_value(ctx, id, label, scope);
            let (current, write) = input_state(ctx, id, value.as_ref(), scope);
            match variant.as_deref() {
                Some("shortText" | "longText" | "obscured") | None => {}
                // Documented by the catalog, and it does constrain input —
                // rendering an unconstrained text box accepts "abc" where
                // the stream asked for a number.
                Some("number") => note_unhonored(
                    ctx,
                    id,
                    "variant: number",
                    "the field accepts any text, with no numeric constraint",
                ),
                Some(other) => ctx.note_unknown_variant(id, "variant", other, "a short text field"),
            }
            if variant.as_deref() == Some("obscured") {
                ctx.note(
                    id,
                    NoteKind::SecretExposed,
                    "obscured input renders unmasked (masking is a kit gap); the rendered pixels \
                     contain the value the stream asked to hide",
                );
            }
            let failure = evaluate_checks(ctx, id, checks, scope)
                .or_else(|| pattern_failure(ctx, id, validation_regexp.as_deref(), &current));
            let invalid = failure.is_some();
            let control: Element<A2uiMsg> = if variant.as_deref() == Some("longText") {
                text_area(current).invalid(invalid).on_input(write).into()
            } else {
                text_input(current).invalid(invalid).on_input(write).into()
            };
            labeled_control(Some(label), control, failure, ctx.theme)
        }
        Kind::CheckBox {
            label,
            value,
            checks,
        } => {
            let failure = evaluate_checks(ctx, id, checks, scope);
            let label = resolve_value(ctx, id, label, scope);
            let (checked, path) = match value {
                Dyn::Binding { path } => (
                    bound_bool(ctx, id, path, scope),
                    Some(absolute(path, scope)),
                ),
                other => {
                    // Literal-valued inputs stay interactive: the toggle
                    // stores a local edit, and the render reads it back.
                    let base = resolve_bool(ctx, id, other, scope);
                    let checked = ctx
                        .surface
                        .ui
                        .local_edits
                        .get(&ui_key(id, scope))
                        .and_then(Value::as_bool)
                        .unwrap_or(base);
                    (checked, None)
                }
            };
            let mut cb = checkbox(checked).label(label);
            cb = match path {
                Some(path) => cb.on_toggle(A2uiMsg::SetBool {
                    path,
                    value: !checked,
                }),
                None => cb.on_toggle(A2uiMsg::LocalEdit {
                    key: ui_key(id, scope),
                    value: Value::Bool(!checked),
                }),
            };
            labeled_control(None, cb.into(), failure, ctx.theme)
        }
        Kind::ChoicePicker {
            label,
            variant,
            options,
            value,
            display_style,
            filterable,
            checks,
        } => {
            let failure = evaluate_checks(ctx, id, checks, scope);
            if display_style.is_some() {
                note_unhonored(
                    ctx,
                    id,
                    "displayStyle",
                    "the picker renders as a dropdown (single) or a checkbox list (multiple)",
                );
            }
            if filterable == &Some(true) {
                note_unhonored(ctx, id, "filterable", "the option list has no filter box");
            }
            render_choice_picker(
                ctx,
                id,
                scope,
                PickerSpec {
                    label: label.as_ref(),
                    variant: variant.as_deref(),
                    options,
                    value,
                    failure,
                },
            )
        }
        Kind::Slider {
            label,
            min,
            max,
            value,
            checks,
        } => {
            let failure = evaluate_checks(ctx, id, checks, scope);
            let requested_min = min.unwrap_or(0.0);
            let requested_max = *max;
            // `Slider::range` ignores anything that is not max > min, and it
            // decides that in f32 — so validating in f64 proves nothing.
            // 1e39 narrows to infinity, 1.0 and 1.0000001 collapse onto the
            // same f32, and either way the kit silently keeps its default
            // 0..=1 domain. An accepted infinite bound is worse still: the
            // widget's own normalization divides by it and feeds NaN into
            // layout. Validate exactly the values the widget will see.
            #[expect(clippy::cast_possible_truncation, reason = "checked below in f32")]
            let (min, max) = {
                let usable = |lo: f32, hi: f32| lo.is_finite() && hi.is_finite() && hi > lo;
                let (lo, hi) = (requested_min as f32, requested_max as f32);
                if usable(lo, hi) {
                    (lo, hi)
                } else {
                    // The repair has to survive the same test that rejected
                    // the original: near the top of the f32 range `lo + 1.0`
                    // rounds straight back to `lo`, so 0..=1 is the only
                    // honest answer left.
                    let (lo, hi) = if usable(lo, lo + 1.0) {
                        (lo, lo + 1.0)
                    } else {
                        (0.0, 1.0)
                    };
                    ctx.note(
                        id,
                        NoteKind::InvalidValue,
                        format!(
                            "slider range {requested_min}..={requested_max} is empty, not finite, \
                             or collapses to a single value in f32; using {lo}..={hi}"
                        ),
                    );
                    (lo, hi)
                }
            };
            let (current, path) = match value {
                Dyn::Binding { path } => (
                    bound_f64(ctx, id, path, scope, f64::from(min)),
                    Some(absolute(path, scope)),
                ),
                other => {
                    // Literal-valued sliders stay interactive through
                    // local edits, like every other input control.
                    let base = resolve_f64(ctx, id, other, scope);
                    let current = ctx
                        .surface
                        .ui
                        .local_edits
                        .get(&ui_key(id, scope))
                        .and_then(Value::as_f64)
                        .unwrap_or(base);
                    (current, None)
                }
            };
            #[expect(
                clippy::cast_possible_truncation,
                reason = "slider positions fit in f32"
            )]
            let mut s = slider(current as f32).range(min, max);
            s = match path {
                Some(path) => s.on_change(move |v| A2uiMsg::SetNumber {
                    path: path.clone(),
                    value: f64::from(v),
                }),
                None => {
                    let key = ui_key(id, scope);
                    s.on_change(move |v| A2uiMsg::LocalEdit {
                        key: key.clone(),
                        value: serde_json::json!(f64::from(v)),
                    })
                }
            };
            labeled_control(
                label.as_ref().map(|l| resolve_value(ctx, id, l, scope)),
                s.into(),
                failure,
                ctx.theme,
            )
        }
        Kind::DateTimeInput {
            value,
            label,
            enable_date,
            enable_time,
            min,
            max,
            checks,
        } => {
            let failure = evaluate_checks(ctx, id, checks, scope);
            if enable_time == &Some(true) || enable_date == &Some(false) {
                note_unhonored(
                    ctx,
                    id,
                    "enableDate/enableTime",
                    "the field accepts a whole ISO-8601 value either way",
                );
            }
            if min.is_some() || max.is_some() {
                note_unhonored(ctx, id, "min/max", "the field accepts any text");
            }
            ctx.note(
                id,
                NoteKind::Unsupported,
                "DateTimeInput renders as an ISO text field (calendar UI TBD)",
            );
            let (current, write) = input_state(ctx, id, Some(value), scope);
            let input = text_input(current)
                .placeholder("YYYY-MM-DD")
                .invalid(failure.is_some())
                .on_input(write);
            labeled_control(
                label.as_ref().map(|l| resolve_value(ctx, id, l, scope)),
                input.into(),
                failure,
                ctx.theme,
            )
        }
        Kind::Unknown(raw) => {
            let name = raw
                .get("component")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            // A known name that failed to parse is an authoring bug in the
            // stream; an unknown name is the protocol working as intended.
            // Different kinds, because an agent fixes them differently.
            let (kind, why) = if crate::catalog::BASIC_CATALOG.contains(&name) {
                (
                    NoteKind::MalformedComponent,
                    "is a basic-catalog component whose fields did not parse",
                )
            } else {
                (
                    NoteKind::UnknownComponent,
                    "is not part of the v0.9 basic catalog",
                )
            };
            ctx.note(
                id,
                kind,
                format!("component {name:?} {why}; rendering a placeholder"),
            );
            placeholder(format!("[{name}]"), theme)
        }
        // The container kinds, which `render_component` handles before it
        // delegates here — so this arm is unreachable today.
        //
        // It degrades instead of asserting that, and the difference is the
        // whole point. `render_component` dispatches containers explicitly
        // and sends everything else here with a `_` arm, which means adding
        // a `Kind` to the catalog compiles there and fails only *here*. The
        // compiler therefore walks the next author straight to this arm,
        // where the tidy-looking fix is to add the new name to the list —
        // and an `unreachable!` would turn that into a process panic on any
        // stream using the feature, in a crate whose premise is that
        // agent-supplied input degrades with a note and never aborts.
        // Rendering a placeholder makes the mistake cost a visible
        // `[unmapped: id]` and a `broken` note instead.
        Kind::Row { .. }
        | Kind::Column { .. }
        | Kind::List { .. }
        | Kind::Card { .. }
        | Kind::Tabs { .. }
        | Kind::Modal { .. }
        | Kind::Button { .. } => {
            ctx.note(
                id,
                NoteKind::MalformedComponent,
                "component kind reached the leaf renderer, which cannot map it; \
                 it renders as a placeholder (a container kind is missing from \
                 render_component's dispatch)",
            );
            placeholder(format!("[unmapped: {id}]"), theme)
        }
    }
}

/// How an edited string gets back into the surface: through its binding
/// when it has one, as a local edit when the value is a literal.
///
/// Built here rather than at each call site, because a control that
/// forgets the second branch silently becomes read-only — which is exactly
/// what `DateTimeInput` was, while still *reading* local edits that could
/// never be written.
fn string_writer(
    key: String,
    path: Option<String>,
) -> impl Fn(String) -> A2uiMsg + Clone + 'static {
    move |v| match &path {
        Some(p) => A2uiMsg::SetString {
            path: p.clone(),
            value: v,
        },
        None => A2uiMsg::LocalEdit {
            key: key.clone(),
            value: Value::String(v),
        },
    }
}

/// The current value of a string-valued input, plus the writer that puts
/// edits back. Literal-valued inputs read their local edit back, so every
/// one of them stays interactive.
fn input_state(
    ctx: &Ctx,
    id: &str,
    value: Option<&Dyn<String>>,
    scope: Option<&str>,
) -> (String, impl Fn(String) -> A2uiMsg + Clone + 'static) {
    let (current, path) = match value {
        Some(Dyn::Binding { path }) => (
            lookup(ctx.surface, path, scope)
                .map(functions::display)
                .unwrap_or_default(),
            Some(absolute(path, scope)),
        ),
        Some(other) => {
            let base = resolve_value(ctx, id, other, scope);
            let current = ctx
                .surface
                .ui
                .local_edits
                .get(&ui_key(id, scope))
                .map(functions::display)
                .unwrap_or(base);
            (current, None)
        }
        None => (
            ctx.surface
                .ui
                .local_edits
                .get(&ui_key(id, scope))
                .map(functions::display)
                .unwrap_or_default(),
            None,
        ),
    };
    (current, string_writer(ui_key(id, scope), path))
}

fn action_msg(ctx: &Ctx, id: &str, action: &Action, scope: Option<&str>) -> A2uiMsg {
    match action {
        Action::Event { event } => {
            let context = event
                .context
                .as_ref()
                .map(|c| resolve_context(ctx, c, scope))
                .unwrap_or(Value::Null);
            A2uiMsg::Event {
                name: event.name.clone(),
                context,
                source_id: id.to_owned(),
            }
        }
        Action::FunctionCall { function_call } if function_call.call == "openUrl" => {
            let url = match function_call.args.get("url") {
                Some(Value::Object(o)) if o.contains_key("path") => {
                    let path = o["path"].as_str().unwrap_or_default();
                    lookup(ctx.surface, path, scope)
                        .map(functions::display)
                        .unwrap_or_default()
                }
                Some(v) => functions::display(v),
                None => String::new(),
            };
            if url.is_empty() {
                // Handing the host an empty URL to open is the same
                // invented-message failure as the synthetic events below:
                // it never came from the stream.
                ctx.note(
                    id,
                    NoteKind::UnresolvedBinding,
                    "openUrl has no URL to open; the action does nothing",
                );
                return A2uiMsg::Ignored;
            }
            if !is_openable(&url) {
                ctx.note(
                    id,
                    NoteKind::BlockedUrlScheme,
                    format!(
                        "openUrl {} is not a URL this renderer will open (the scheme, \
                         or for mailto: the header fields it carries); the control \
                         does nothing",
                        quoted(&url)
                    ),
                );
                return A2uiMsg::Ignored;
            }
            A2uiMsg::OpenUrl(url)
        }
        Action::FunctionCall { function_call } => {
            ctx.note(
                id,
                NoteKind::UnimplementedFunction,
                format!(
                    "action function {:?} is not implemented",
                    function_call.call
                ),
            );
            // Emphatically *not* a synthetic Event: the agent never asked
            // for an action called "unimplemented:openWidget", and sending
            // one makes it defend against messages fenestra invented.
            A2uiMsg::Ignored
        }
    }
}

/// Resolves `{path}` bindings anywhere inside an action context object.
fn resolve_context(ctx: &Ctx, value: &Value, scope: Option<&str>) -> Value {
    match value {
        Value::Object(map) => {
            if map.len() == 1
                && let Some(Value::String(path)) = map.get("path")
            {
                return lookup(ctx.surface, path, scope)
                    .cloned()
                    .unwrap_or(Value::Null);
            }
            Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), resolve_context(ctx, v, scope)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| resolve_context(ctx, v, scope))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Everything a ChoicePicker renders from, gathered so the renderer takes
/// a subject rather than a parameter list.
struct PickerSpec<'a> {
    label: Option<&'a Dyn<String>>,
    variant: Option<&'a str>,
    options: &'a [ChoiceOption],
    /// The selection: a list, a single string, or a `{"path"}` binding.
    value: &'a Value,
    /// The failing check's message, when one failed.
    failure: Option<String>,
}

fn render_choice_picker(
    ctx: &Ctx,
    id: &str,
    scope: Option<&str>,
    spec: PickerSpec<'_>,
) -> Element<A2uiMsg> {
    let PickerSpec {
        label,
        variant,
        options,
        value,
        failure,
    } = spec;
    let labels: Vec<String> = options
        .iter()
        .map(|o| resolve_value(ctx, id, &o.label, scope))
        .collect();
    let values: Vec<String> = options.iter().map(|o| o.value.clone()).collect();
    /// A selection as a string list: an array of values, or one string (a
    /// valid mutually-exclusive selection) — accepted identically whether
    /// it arrives literal or through a binding.
    fn selection_of(v: &Value) -> Option<Vec<String>> {
        match v {
            Value::Array(items) => Some(items.iter().map(functions::display).collect()),
            Value::String(s) => Some(vec![s.clone()]),
            _ => None,
        }
    }
    let (mut selected_values, path): (Vec<String>, Option<String>) = match value {
        Value::Object(o) if o.contains_key("path") => {
            let p = o["path"].as_str().unwrap_or_default();
            let selected = match lookup(ctx.surface, p, scope) {
                Some(v) => selection_of(v).unwrap_or_else(|| {
                    ctx.note(
                        id,
                        NoteKind::BindingType,
                        format!("selection binding {p:?} is neither a list nor a string"),
                    );
                    Vec::new()
                }),
                // An unset binding is the ordinary "nothing chosen yet"
                // state, not a fidelity loss — the same call that
                // `input_state` makes for a text field, and for the same
                // reason: form values legitimately start empty.
                None => Vec::new(),
            };
            (selected, Some(absolute(p, scope)))
        }
        other => (selection_of(other).unwrap_or_default(), None),
    };
    if path.is_none() {
        // Literal-valued pickers stay interactive through local edits.
        if let Some(edited) = ctx
            .surface
            .ui
            .local_edits
            .get(&ui_key(id, scope))
            .and_then(selection_of)
        {
            selected_values = edited;
        }
    }
    let selected_idx: Vec<usize> = values
        .iter()
        .enumerate()
        .filter(|(_, v)| selected_values.contains(v))
        .map(|(i, _)| i)
        .collect();
    // Values the model holds that this picker has no option for. Reporting
    // only the all-or-nothing case hid the more damaging one: a partial
    // match renders as if the unmatched values were not there, and the next
    // toggle writes the visible selection back over them.
    let unmatched: Vec<String> = selected_values
        .iter()
        .filter(|v| !values.contains(v))
        .cloned()
        .collect();
    if !unmatched.is_empty() {
        ctx.note(
            id,
            NoteKind::InvalidValue,
            format!("selection {unmatched:?} matches none of this picker's options {values:?}"),
        );
    }
    // Selection changes write through the binding, or store a local edit
    // for literal-valued pickers — either way the picker stays live.
    let make_msg = {
        let path = path.clone();
        let key = ui_key(id, scope);
        move |values: Vec<String>| match &path {
            Some(p) => A2uiMsg::SetList {
                path: p.clone(),
                values,
            },
            None => A2uiMsg::LocalEdit {
                key: key.clone(),
                value: Value::Array(values.into_iter().map(Value::String).collect()),
            },
        }
    };
    match variant {
        Some("mutuallyExclusive" | "multipleSelection") | None => {}
        Some(other) => ctx.note_unknown_variant(id, "variant", other, "a single-selection picker"),
    }
    let multiple = variant == Some("multipleSelection");
    let control: Element<A2uiMsg> = if multiple {
        let mut ms = multi_select(selected_idx.clone(), labels);
        {
            let values = values.clone();
            let current = selected_idx;
            // The picker can only offer its own options, so rebuilding the
            // list from them alone would drop anything the model holds that
            // this picker cannot show. Carry those through untouched: a
            // control the user cannot see must not be able to delete data.
            let unmatched = unmatched.clone();
            ms = ms.on_toggle(move |i| {
                let mut next: Vec<usize> = current.clone();
                if let Some(pos) = next.iter().position(|&x| x == i) {
                    next.remove(pos);
                } else {
                    next.push(i);
                    next.sort_unstable();
                }
                let mut written: Vec<String> = next
                    .iter()
                    .filter_map(|&x| values.get(x).cloned())
                    .collect();
                written.extend(unmatched.iter().cloned());
                make_msg(written)
            });
        }
        ms.into()
    } else {
        // `select` has no empty state, so handing it index 0 would claim
        // the user picked the first option when the model says nothing is
        // chosen. A leading placeholder entry shows "nothing yet" honestly.
        //
        // It stays in the list once something *is* chosen, because that is
        // the only way back: offering it only while empty would make the
        // empty state unreachable through the UI, so a picker could be
        // filled in but never cleared.
        let mut with_placeholder = Vec::with_capacity(labels.len() + 1);
        with_placeholder.push(UNSELECTED_LABEL.to_owned());
        with_placeholder.extend(labels);
        // Index 0 is the placeholder, so every real option sits one along.
        let index = selected_idx.first().map_or(0, |i| i + 1);
        let mut sel = select(index, with_placeholder);
        {
            let values = values.clone();
            sel = sel.on_change(move |i| {
                make_msg(
                    i.checked_sub(1)
                        .and_then(|x| values.get(x).cloned())
                        .into_iter()
                        .collect(),
                )
            });
        }
        sel.into()
    };
    labeled_control(
        label.map(|l| resolve_value(ctx, id, l, scope)),
        control,
        failure,
        ctx.theme,
    )
}
