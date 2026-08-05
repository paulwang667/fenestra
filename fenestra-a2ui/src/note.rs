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
    /// Mapped onto the nearest thing fenestra has, not the exact one.
    Approximated,
    /// A catalog feature parsed but not honored yet.
    Unsupported,
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
    #[must_use]
    pub fn severity(self) -> NoteSeverity {
        match self {
            Self::NetworkAsset | Self::Approximated | Self::Unsupported => {
                NoteSeverity::Approximate
            }
            _ => NoteSeverity::Broken,
        }
    }
}

/// One thing that did not map faithfully, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
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
    }
}
