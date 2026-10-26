//! The runner's side of a mirrored IME (Android GameActivity): which field the
//! IME holds, and which of its state reports to believe.

use fenestra_core::ImeTextState;

/// How long a state report that does not match a field switch is distrusted
/// (seconds); past it, reports are believed again so input never locks up.
const SWITCH_GRACE: f64 = 0.5;

/// What the IME was last told, and whether a switch to another field is
/// still settling.
#[derive(Debug, Default)]
pub(crate) struct ImeSync {
    synced: Option<ImeTextState>,
    /// A push that moved the IME to another field, with when it was sent:
    /// until the IME reports exactly this state back, its reports still
    /// describe the old field's buffer and are dropped.
    switching: Option<(ImeTextState, f64)>,
}

impl ImeSync {
    /// The state the IME holds, as far as the runner knows.
    pub(crate) fn synced(&self) -> Option<&ImeTextState> {
        self.synced.as_ref()
    }

    /// Records that `state` was pushed to the IME at `now` (seconds).
    pub(crate) fn pushed(&mut self, state: Option<ImeTextState>, now: f64) {
        let switched = state
            .as_ref()
            .is_some_and(|s| self.synced.as_ref().is_some_and(|old| old.field != s.field));
        self.switching = match (&state, switched) {
            (Some(s), true) => Some((s.clone(), now)),
            _ => None,
        };
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
        if let Some((pushed, at)) = &self.switching {
            if *pushed == ime {
                self.switching = None;
            } else if now - at < SWITCH_GRACE {
                return None;
            } else {
                self.switching = None;
            }
        }
        // The IME already holds this state: only differences the field
        // makes from here on go back to it.
        self.synced = Some(ime.clone());
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

    #[test]
    fn a_push_within_one_field_drops_nothing() {
        let mut sync = ImeSync::default();
        sync.pushed(Some(state("ab", 1)), 0.0);
        sync.pushed(Some(state("abc", 1)), 1.0);
        assert_eq!(recv(&mut sync, "abcd", 1.001), Some(state("abcd", 1)));
    }
}
