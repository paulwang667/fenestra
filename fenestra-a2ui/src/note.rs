//! Fidelity notes: the machine-readable half of "silence means fidelity".
//!
//! Folding a message stream and rendering a surface both record [`Note`]s.
//! An empty list means the stream mapped completely. A non-empty one says
//! exactly what did not, pointed at the component id (or message type)
//! that carried it.
//!
//! The [`NoteKind`] exists so a caller can *branch*. An agent that gets
//! [`NoteKind::UnknownIcon`] back can retry with a different icon name; one
//! that gets [`NoteKind::UnknownComponent`] knows the catalog is the
//! problem, not its data; a CI check can fail on [`NoteSeverity::Broken`]
//! while tolerating [`NoteSeverity::Approximate`]. Matching on the prose
//! instead would break the moment a message is reworded.

use serde::Serialize;

/// Why a note was recorded.
///
/// New kinds may be added, so match with a `_` arm — or match on
/// [`NoteKind::severity`] when the exact cause does not matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub enum NoteKind {
    /// A component name this catalog build does not implement.
    UnknownComponent,
    /// A known component name whose body did not parse (missing required
    /// field, mistyped value).
    MalformedComponent,
    /// A referenced component id has no definition on the surface — often
    /// a stream that has not finished arriving.
    MissingComponent,
    /// The component graph refers back to itself (`a → b → a`).
    ReferenceCycle,
    /// Nesting ran past the renderer's depth cap.
    DepthCap,
    /// A data binding pointed at nothing in the data model.
    UnresolvedBinding,
    /// A binding resolved, but to the wrong JSON type for its slot.
    BindingType,
    /// A literal value this slot cannot use (an empty slider range, an
    /// unrecognized enum string).
    InvalidValue,
    /// An icon name outside the vendored Lucide set.
    UnknownIcon,
    /// A client-side function this build does not implement.
    UnimplementedFunction,
    /// Output was cut short by a cap (template children).
    Truncated,
    /// A data-model write could not be applied; the model kept its
    /// previous value.
    RejectedWrite,
    /// A message type from a newer protocol revision, skipped.
    UnknownMessage,
    /// A remote asset (image, video, audio) rendered as a labeled
    /// placeholder. Deterministic renders never touch the network.
    NetworkAsset,
    /// A value the stream asked to be hidden was rendered in the clear.
    /// The pixels contain a secret, and a headless render hands those
    /// pixels to whoever asked for them.
    SecretExposed,
    /// A control is on screen but cannot be operated — something else
    /// takes its press, so the interaction it exists for is dead.
    Unreachable,
    /// Mapped onto the nearest thing fenestra has, not the exact one.
    Approximated,
    /// A catalog feature parsed but not honored yet.
    Unsupported,
    /// The surface is built on a component catalog this build does not
    /// implement, so its components render best-effort.
    UnknownCatalog,
}

/// How much a note should worry the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NoteSeverity {
    /// The surface does not show what the stream asked for. Something is
    /// missing, replaced by a placeholder, or silently unwritten.
    Broken,
    /// The surface shows the right thing, inexactly — a placeholder for a
    /// remote asset, a nearby layout mode, a feature that parses but does
    /// not yet act.
    Approximate,
}

impl NoteKind {
    /// How much this kind should worry the caller.
    ///
    /// The line is drawn at *can the surface still do its job*. A remote
    /// image standing in as a grey box, a layout mode mapped to its
    /// nearest neighbour, a validation rule that parses without gating —
    /// the surface works, it is merely not exact. Anything that leaves the
    /// user unable to do what the stream described, or that puts something
    /// on screen the stream asked to hide, is broken.
    #[must_use]
    pub fn severity(self) -> NoteSeverity {
        // Exhaustive on purpose. A catch-all would classify every future
        // kind as broken without a word from the compiler, so a genuinely
        // approximate one would start failing every `any_broken` check on
        // surfaces that render correctly. Adding a variant should not
        // compile until someone has decided which side it falls on.
        match self {
            Self::NetworkAsset | Self::Approximated | Self::Unsupported => {
                NoteSeverity::Approximate
            }
            Self::UnknownComponent
            | Self::MalformedComponent
            | Self::MissingComponent
            | Self::ReferenceCycle
            | Self::DepthCap
            | Self::UnresolvedBinding
            | Self::BindingType
            | Self::InvalidValue
            | Self::UnknownIcon
            | Self::UnimplementedFunction
            | Self::Truncated
            | Self::RejectedWrite
            | Self::UnknownMessage
            | Self::SecretExposed
            | Self::Unreachable
            // The dangerous case is the quiet one: another catalog that
            // reuses basic's component *names* with different meanings.
            // Every component then parses, no per-component note fires, and
            // this is the only signal there is — so it has to be the kind
            // that fails `any_broken`, or a CI check passes green over a
            // surface whose every control may mean something else.
            | Self::UnknownCatalog => NoteSeverity::Broken,
        }
    }
}

/// The most distinct notes either pipeline keeps.
pub(crate) const MAX_NOTES: usize = 4096;

/// Records a note in `notes`, once, up to [`MAX_NOTES`] of them.
///
/// Both note pipelines are driven by input the crate does not control: the
/// surface's accumulates one rejected write per keystroke for the life of
/// the session, and the renderer's rebuilds every frame, where a single
/// placeholder inside a thousand-row template becomes a thousand notes —
/// and nested templates multiply that. Unbounded, they grow until the
/// process does; undeduplicated, the one real problem is buried in
/// thousands of copies of itself.
///
/// Reaching the cap records one [`NoteKind::Truncated`] note, because a
/// crate whose contract is "silence means fidelity" must not go silent
/// about having gone silent.
pub(crate) fn push_bounded(notes: &mut Vec<Note>, note: Note) {
    if notes.contains(&note) {
        return;
    }
    if notes.len() >= MAX_NOTES {
        let marker = Note::new(
            "",
            NoteKind::Truncated,
            format!("more than {MAX_NOTES} distinct notes; the rest are not recorded"),
        );
        if !notes.contains(&marker) {
            notes.push(marker);
        }
        return;
    }
    notes.push(note);
}

/// One thing that did not map faithfully, and where.
///
/// Serializes as `{componentId, kind, severity, detail}`. `severity` is
/// derived from `kind`, but it ships on the wire on purpose: without it
/// every consumer — the MCP server, the CLI, whatever reads their JSON —
/// has to reimplement [`NoteKind::severity`]'s table and silently drift
/// from it the next time a kind is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// The component id this happened to — or, for stream-level notes, the
    /// message type that carried it.
    pub component_id: String,
    /// Why it happened.
    pub kind: NoteKind,
    /// The human-readable half: what exactly, in prose.
    pub detail: String,
}

impl Note {
    /// Records a note against a component id.
    pub fn new(component_id: impl Into<String>, kind: NoteKind, detail: impl Into<String>) -> Self {
        Self {
            component_id: component_id.into(),
            kind,
            detail: detail.into(),
        }
    }

    /// How much this note should worry the caller.
    #[must_use]
    pub fn severity(&self) -> NoteSeverity {
        self.kind.severity()
    }
}

impl Serialize for Note {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut o = s.serialize_struct("Note", 4)?;
        o.serialize_field("componentId", &self.component_id)?;
        o.serialize_field("kind", &self.kind)?;
        o.serialize_field("severity", &self.severity())?;
        o.serialize_field("detail", &self.detail)?;
        o.end()
    }
}

impl std::fmt::Display for Note {
    /// `"component_id: detail"` — the form notes have always printed in,
    /// so logs and error messages read the same as before the kind existed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.component_id, self.detail)
    }
}

/// Whether any note reports something actually broken (as opposed to
/// merely approximate). The one-line CI check.
#[must_use]
pub fn any_broken(notes: &[Note]) -> bool {
    notes.iter().any(|n| n.severity() == NoteSeverity::Broken)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_keeps_the_id_prefixed_form() {
        let n = Note::new(
            "btn",
            NoteKind::UnknownIcon,
            "icon \"nope\" is not vendored",
        );
        assert_eq!(n.to_string(), "btn: icon \"nope\" is not vendored");
    }

    /// The two cases that motivated a `Broken` classification of their
    /// own: a dialog nothing can open, and a secret on screen.
    #[test]
    fn unusable_and_leaking_surfaces_are_broken() {
        assert_eq!(NoteKind::Unreachable.severity(), NoteSeverity::Broken);
        assert_eq!(NoteKind::SecretExposed.severity(), NoteSeverity::Broken);
        assert_eq!(
            NoteKind::Unsupported.severity(),
            NoteSeverity::Approximate,
            "a feature that parses without acting still leaves a working surface"
        );
    }

    #[test]
    fn severity_splits_broken_from_approximate() {
        assert_eq!(
            NoteKind::UnknownComponent.severity(),
            NoteSeverity::Broken,
            "an unmapped component is not a cosmetic difference"
        );
        assert_eq!(NoteKind::NetworkAsset.severity(), NoteSeverity::Approximate);
        assert!(any_broken(&[Note::new(
            "x",
            NoteKind::ReferenceCycle,
            "cycle"
        )]));
        assert!(!any_broken(&[Note::new(
            "img",
            NoteKind::NetworkAsset,
            "placeholder"
        )]));
    }

    #[test]
    fn notes_serialize_for_agents() {
        let n = Note::new("row_1", NoteKind::UnresolvedBinding, "no such path");
        let v = serde_json::to_value(&n).expect("a note serializes");
        assert_eq!(v["componentId"], "row_1");
        assert_eq!(v["kind"], "unresolvedBinding");
        assert_eq!(v["detail"], "no such path");
        assert_eq!(
            v["severity"], "broken",
            "severity ships on the wire so consumers need not reimplement the table"
        );
    }
}
