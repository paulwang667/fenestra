//! A native Rust renderer for [A2UI](https://a2ui.org) v0.9 — the open
//! Agent-to-UI standard where agents send declarative JSON surfaces and
//! the client renders them with its own component library.
//!
//! fenestra is that component library here: an A2UI message stream folds
//! into a [`Client`] of [`Surface`]s, each surface renders to a fenestra
//! [`Element`](fenestra_core::Element) tree through the v0.9 *basic
//! catalog* mapping, and everything downstream of that — windowed
//! running, deterministic headless PNGs, the accessibility tree, golden
//! testing — is the ordinary fenestra pipeline. That last part is the
//! point: this is an A2UI client whose output an agent can verify
//! headlessly, byte-for-byte, in CI.
//!
//! ```no_run
//! use fenestra_a2ui::{Client, messages::parse_stream};
//!
//! let msgs = parse_stream(r#"{ "messages": [] }"#).unwrap();
//! let mut client = Client::new();
//! client.apply_all(&msgs).unwrap();
//! if let Some(surface) = client.single_surface() {
//!     let rendered = surface.render(&fenestra_core::Theme::light());
//!     // rendered.element → any fenestra runner or headless render;
//!     // rendered.notes  → what (if anything) didn't map faithfully.
//! }
//! ```
//!
//! Coverage is the whole 18-component basic catalog. Data bindings
//! (absolute and template-relative JSON Pointers), templated children,
//! two-way input binding, `formatString`/`formatNumber`/`formatCurrency`/
//! `formatDate`/`pluralize`, and server-bound actions with resolved
//! context all work.
//!
//! Everything that does *not* map exactly reports itself. `Surface::notes`
//! and [`Rendered::notes`] hand back typed [`Note`]s, each carrying a
//! [`NoteKind`] so a caller can branch on the cause rather than match on
//! prose, and a [`NoteSeverity`] separating "the surface is not what the
//! stream asked for" from "close, but inexact". [`any_broken`] is the
//! one-line CI check.
//!
//! The current inexact cases: remote images, video and audio render as
//! labeled placeholders (a deterministic render never touches the
//! network), and `DateTimeInput` is an ISO text field rather than a
//! calendar. Those are `approximate` — the surface still does its job.
//!
//! Client-side validation *is* enforced. A control's `checks` are
//! evaluated every render and the first failing rule shows its own
//! message beneath the control; a Button whose checks fail carries no
//! action at all, which is the entire point of putting one there, and a
//! Modal trigger whose checks fail does not open its dialog. TextField and
//! DateTimeInput also take the kit's invalid ring — CheckBox, ChoicePicker
//! and Slider show the message without one, because the kit's controls for
//! those have no invalid state yet. All eight of the catalog's boolean functions work — `required`,
//! `regex`, `length`, `numeric`, `email`, and `and`/`or`/`not` to compose
//! them — as does TextField's `validationRegexp`. A rule this build cannot
//! evaluate (a pattern needing ECMAScript lookaround, a function from a
//! newer catalog) does not gate and records a `broken` note, so the
//! surface stays usable and the caller still learns the rule is not being
//! enforced.
//!
//! Two gaps are `broken` rather than inexact, because they leave the
//! surface unable to do what the stream described: an `obscured` field
//! renders unmasked, so the pixels a headless render hands back contain
//! the value that was meant to be hidden; and a modal trigger wrapping its
//! own interactive child cannot open its dialog at all. A login form will
//! therefore fail [`any_broken`] until masking lands — which is the honest
//! answer, not a false pass.
//!
//! Each records its own note — an empty list really does mean full
//! fidelity.

pub mod catalog;
pub mod checks;
pub mod functions;
pub mod messages;
pub mod note;
pub mod render;
pub mod surface;

pub use messages::{Envelope, MessageStream, parse_stream};
pub use note::{Note, NoteKind, NoteSeverity, any_broken};
pub use render::{A2uiMsg, A2uiSignal, Rendered};
pub use surface::{A2uiError, Client, Surface};
