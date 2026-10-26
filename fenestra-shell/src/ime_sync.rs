//! The runner's side of a mirrored IME (Android GameActivity): which field the
//! IME holds, and which of its state reports to believe.

use fenestra_core::ImeTextState;

/// How long after a field switch the old field's buffer is distrusted
/// (seconds); past it, reports are believed again so input never locks up.
const SWITCH_GRACE: f64 = 0.5;

/// What the IME was last told, and whether a switch to another field is
/// still settling.
#[derive(Debug, Default)]
pub(crate) struct ImeSync {
    /// The focused field's state as the IME knows it; `None` with no field
    /// focused.
    synced: Option<ImeTextState>,
    /// The last field the IME was given, kept when focus leaves every field:
    /// GameActivity still holds its buffer and its editor info.
    held: Option<ImeTextState>,
    /// The old field's text after a push moved the IME to another field,
    /// with when it was sent: GameActivity may echo that buffer before it
    /// takes the push, and such a report is dropped. Any other report ends
    /// the switch.
    switching: Option<(String, f64)>,
}

impl ImeSync {
    /// The state the IME holds, as far as the runner knows.
    pub(crate) fn synced(&self) -> Option<&ImeTextState> {
        self.synced.as_ref()
    }

    /// Whether the IME's editor info is the password one.
    pub(crate) fn held_masked(&self) -> bool {
        self.held.as_ref().is_some_and(|s| s.masked)
    }

    /// Records that `state` was pushed to the IME at `now` (seconds).
    pub(crate) fn pushed(&mut self, state: Option<ImeTextState>, now: f64) {
        self.switching = match (&state, &self.held) {
            (Some(s), Some(old)) if old.field != s.field => Some((old.text.clone(), now)),
            _ => None,
        };
        if state.is_some() {
            self.held.clone_from(&state);
        }
        self.synced = state;
    }

    /// A state reported by the IME at `now`: returns it attributed to the
    /// field it edits, or `None` when it is stale.
    pub(crate) fn received(
        &mut self,
        text: String,
        selection: (usize, usize),
        compose: Option<(usize, usize)>,
        now: f64,
    ) -> Option<ImeTextState> {
        let synced = self.synced.as_ref();
        let ime = ImeTextState {
            text,
            selection,
            compose,
            field: synced.and_then(|s| s.field),
            masked: synced.is_some_and(|s| s.masked),
        };
        if let Some((old, at)) = self.switching.take() {
            // Text the new field also has cannot be told apart: believe it.
            let echo = ime.text == old && synced.is_none_or(|s| s.text != old);
            if echo && now - at < SWITCH_GRACE {
                self.switching = Some((old, at));
                return None;
            }
        }
        // The IME already holds this state: only differences the field
        // makes from here on go back to it.
        self.synced = Some(ime.clone());
        self.held = Some(ime.clone());
        Some(ime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenestra_core::WidgetId;

    fn state(text: &str, field: u64) -> ImeTextState {
        ImeTextState {
            text: text.into(),
            selection: (text.len(), text.len()),
            compose: None,
            field: Some(WidgetId(field)),
            masked: false,
        }
    }

    fn recv(sync: &mut ImeSync, text: &str, now: f64) -> Option<ImeTextState> {
        sync.received(text.into(), (text.len(), text.len()), None, now)
    }

    #[test]
    fn reports_are_attributed_to_the_synced_field() {
        let mut sync = ImeSync::default();
        sync.pushed(Some(state("", 1)), 0.0);
        assert_eq!(recv(&mut sync, "a", 0.1), Some(state("a", 1)));
        assert_eq!(sync.synced(), Some(&state("a", 1)));
    }

    /// GameActivity echoes the old field's buffer once after a push that
    /// moves it to another field, before it acknowledges the push.
    #[test]
    fn the_old_fields_echo_after_a_switch_is_dropped() {
        let mut sync = ImeSync::default();
        sync.pushed(Some(state("你好", 1)), 0.0);
        sync.pushed(Some(state("", 2)), 1.0);
        assert_eq!(recv(&mut sync, "你好", 1.001), None);
        assert_eq!(recv(&mut sync, "", 1.03), Some(state("", 2)));
        // Settled: typing in the new field goes through.
        assert_eq!(recv(&mut sync, "x", 1.5), Some(state("x", 2)));
    }

    #[test]
    fn a_switch_that_is_never_acknowledged_stops_blocking() {
        let mut sync = ImeSync::default();
        sync.pushed(Some(state("old", 1)), 0.0);
        sync.pushed(Some(state("", 2)), 1.0);
        assert_eq!(recv(&mut sync, "y", 1.6), Some(state("y", 2)));
    }

    /// An IME may acknowledge a switch with a state of its own (here, the
    /// selection moved), or the user may type before it acknowledges: only
    /// the old field's buffer is stale.
    #[test]
    fn after_a_switch_only_the_old_buffer_is_dropped() {
        let mut sync = ImeSync::default();
        sync.pushed(Some(state("你好", 1)), 0.0);
        sync.pushed(Some(state("hi", 2)), 1.0);
        let mut moved = state("hi", 2);
        moved.selection = (0, 0);
        assert_eq!(sync.received("hi".into(), (0, 0), None, 1.02), Some(moved));
        sync.pushed(Some(state("", 3)), 2.0);
        assert_eq!(recv(&mut sync, "x", 2.05), Some(state("x", 3)));
    }

    #[test]
    fn a_push_within_one_field_drops_nothing() {
        let mut sync = ImeSync::default();
        sync.pushed(Some(state("ab", 1)), 0.0);
        sync.pushed(Some(state("abc", 1)), 1.0);
        assert_eq!(recv(&mut sync, "abcd", 1.001), Some(state("abcd", 1)));
    }

    /// Leaving every field pushes nothing, but GameActivity still holds the
    /// last field — its buffer and its editor info.
    #[test]
    fn leaving_all_fields_keeps_what_the_ime_holds() {
        let mut sync = ImeSync::default();
        let mut secret = state("pw", 1);
        secret.masked = true;
        sync.pushed(Some(secret), 0.0);
        sync.pushed(None, 1.0);
        assert!(sync.held_masked(), "the password editor info is still set");
        sync.pushed(Some(state("", 2)), 2.0);
        assert!(!sync.held_masked());
        // GameActivity's echo of the old buffer after the switch is stale.
        assert_eq!(recv(&mut sync, "pw", 2.001), None);
        assert_eq!(recv(&mut sync, "", 2.03), Some(state("", 2)));
    }
}
