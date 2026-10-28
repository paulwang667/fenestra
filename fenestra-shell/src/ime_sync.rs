//! The runner's side of a mirrored IME (Android GameActivity, iOS with the
//! winit fork): which field the IME holds, and which of its state reports
//! to believe.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "Android uses the EditorInfo half, iOS the traits half"
    )
)]

use fenestra_core::{EnterKey, ImeTextState, Keyboard};

/// What an Android IME is told a field is (`EditorInfo`): its input type,
/// its enter key's action, and its IME options — raw values, so the
/// mapping is tested on any platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EditorInfo {
    pub input_type: u32,
    pub action: i32,
    pub options: u32,
}

// android.text.InputType and android.view.inputmethod.EditorInfo.
const TYPE_CLASS_TEXT: u32 = 0x1;
const TYPE_CLASS_NUMBER: u32 = 0x2;
const TYPE_CLASS_PHONE: u32 = 0x3;
const TYPE_NUMBER_FLAG_DECIMAL: u32 = 0x2000;
const TYPE_NUMBER_VARIATION_PASSWORD: u32 = 0x10;
const TYPE_TEXT_VARIATION_URI: u32 = 0x10;
const TYPE_TEXT_VARIATION_EMAIL_ADDRESS: u32 = 0x20;
const TYPE_TEXT_VARIATION_PASSWORD: u32 = 0x80;
const TYPE_TEXT_FLAG_MULTI_LINE: u32 = 0x2_0000;
const TYPE_TEXT_FLAG_NO_SUGGESTIONS: u32 = 0x8_0000;
const IME_ACTION_NONE: i32 = 1;
const IME_ACTION_GO: i32 = 2;
const IME_ACTION_SEARCH: i32 = 3;
const IME_ACTION_SEND: i32 = 4;
const IME_ACTION_NEXT: i32 = 5;
const IME_ACTION_DONE: i32 = 6;
const IME_FLAG_NO_PERSONALIZED_LEARNING: u32 = 0x0100_0000;
const IME_FLAG_NO_FULLSCREEN: u32 = 0x0200_0000;
const IME_FLAG_NO_ENTER_ACTION: u32 = 0x4000_0000;

/// The `EditorInfo` for a focused field: its keyboard (a password gets the
/// password variation, no suggestions and nothing learned), its enter key
/// as the action — a newline key keeps the enter key a line break, and
/// alone flags a multiline field multi-line — and, as GameActivity's
/// default, no fullscreen editor.
pub(crate) fn editor_info(field: &ImeTextState) -> EditorInfo {
    let numeric = matches!(
        field.keyboard,
        Keyboard::Number | Keyboard::Decimal | Keyboard::Phone
    );
    let mut input_type = match (field.keyboard, field.masked) {
        (_, true) if numeric => TYPE_CLASS_NUMBER | TYPE_NUMBER_VARIATION_PASSWORD,
        (_, true) => TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_PASSWORD | TYPE_TEXT_FLAG_NO_SUGGESTIONS,
        (Keyboard::Number, false) => TYPE_CLASS_NUMBER,
        (Keyboard::Decimal, false) => TYPE_CLASS_NUMBER | TYPE_NUMBER_FLAG_DECIMAL,
        (Keyboard::Phone, false) => TYPE_CLASS_PHONE,
        (Keyboard::Email, false) => TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_EMAIL_ADDRESS,
        (Keyboard::Url, false) => TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_URI,
        _ => TYPE_CLASS_TEXT,
    };
    // Multi-line only with a newline key: Gboard shows a newline key on any
    // multi-line field, whatever its action, so a composer with a Send key
    // is not flagged (the editor wraps it all the same).
    if field.multiline
        && field.enter_key == EnterKey::Newline
        && input_type & 0xf == TYPE_CLASS_TEXT
    {
        input_type |= TYPE_TEXT_FLAG_MULTI_LINE;
    }
    let action = match field.enter_key {
        EnterKey::Newline => IME_ACTION_NONE,
        EnterKey::Send => IME_ACTION_SEND,
        EnterKey::Search => IME_ACTION_SEARCH,
        EnterKey::Go => IME_ACTION_GO,
        EnterKey::Next => IME_ACTION_NEXT,
        _ => IME_ACTION_DONE,
    };
    #[expect(clippy::cast_sign_loss, reason = "IME actions are small and positive")]
    let mut options = action as u32 | IME_FLAG_NO_FULLSCREEN;
    if action == IME_ACTION_NONE {
        options |= IME_FLAG_NO_ENTER_ACTION;
    }
    if field.masked {
        options |= IME_FLAG_NO_PERSONALIZED_LEARNING;
    }
    EditorInfo {
        input_type,
        action,
        options,
    }
}

/// The iOS keyboard layout a field asks for (winit fork's `KeyboardType`;
/// fenestra's own type so the mapping is tested on any platform).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IosKeyboard {
    Default,
    NumberPad,
    DecimalPad,
    PhonePad,
    EmailAddress,
    Url,
}

/// The iOS return key a field asks for (winit fork's `ReturnKeyType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IosReturn {
    Default,
    Send,
    Search,
    Go,
    Done,
    Next,
}

/// What the iOS keyboard is told a field is (`UITextInputTraits`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Traits {
    pub keyboard: IosKeyboard,
    pub return_key: IosReturn,
    pub secure: bool,
    pub autocorrect: bool,
    pub autocapitalize: bool,
}

/// The iOS traits for a focused field, matching [`editor_info`]: its
/// keyboard, its enter key (a newline key is iOS's plain return key), and
/// for a password, an address or a number no suggestions or capitals.
pub(crate) fn text_input_traits(field: &ImeTextState) -> Traits {
    let keyboard = match field.keyboard {
        Keyboard::Number => IosKeyboard::NumberPad,
        Keyboard::Decimal => IosKeyboard::DecimalPad,
        Keyboard::Phone => IosKeyboard::PhonePad,
        Keyboard::Email => IosKeyboard::EmailAddress,
        Keyboard::Url => IosKeyboard::Url,
        _ => IosKeyboard::Default,
    };
    let return_key = match field.enter_key {
        EnterKey::Newline => IosReturn::Default,
        EnterKey::Send => IosReturn::Send,
        EnterKey::Search => IosReturn::Search,
        EnterKey::Go => IosReturn::Go,
        EnterKey::Next => IosReturn::Next,
        _ => IosReturn::Done,
    };
    let suggest = !field.masked && keyboard == IosKeyboard::Default;
    Traits {
        keyboard,
        return_key,
        secure: field.masked,
        autocorrect: suggest,
        autocapitalize: suggest,
    }
}

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

    /// The editor info the IME holds (of the last field it was given).
    pub(crate) fn held_editor_info(&self) -> Option<EditorInfo> {
        self.held.as_ref().map(editor_info)
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
            // The field's kind is the field's, whatever the IME reports.
            keyboard: synced.map_or(Keyboard::Text, |s| s.keyboard),
            enter_key: synced.map_or(EnterKey::Done, |s| s.enter_key),
            multiline: synced.is_some_and(|s| s.multiline),
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
            ..ImeTextState::default()
        }
    }

    /// A focused field of this kind, as `ime_text_state` reports one.
    fn kind(
        keyboard: Keyboard,
        enter_key: EnterKey,
        multiline: bool,
        masked: bool,
    ) -> ImeTextState {
        ImeTextState {
            keyboard,
            enter_key: fenestra_core::resolve_enter_key(enter_key, multiline, false),
            multiline,
            masked,
            ..state("", 1)
        }
    }

    const NO_ENTER_ACTION: u32 = 0x4000_0000;
    const MULTI_LINE: u32 = 0x2_0000;

    #[test]
    fn the_default_field_maps_as_before() {
        // What sync_ime_text pushed for every field: text, Done, no
        // fullscreen.
        let plain = editor_info(&kind(Keyboard::Text, EnterKey::Default, false, false));
        assert_eq!(
            plain,
            EditorInfo {
                input_type: 1,
                action: 6,
                options: 6 | 0x0200_0000
            }
        );
        // And for a password: no suggestions, nothing learned.
        let secret = editor_info(&kind(Keyboard::Text, EnterKey::Default, false, true));
        assert_eq!(
            secret,
            EditorInfo {
                input_type: 1 | 0x80 | 0x8_0000,
                action: 6,
                options: 6 | 0x0200_0000 | 0x0100_0000
            }
        );
    }

    #[test]
    fn each_keyboard_maps_to_its_class() {
        let class = |k| editor_info(&kind(k, EnterKey::Default, false, false)).input_type;
        assert_eq!(class(Keyboard::Number), 2);
        assert_eq!(class(Keyboard::Decimal), 2 | 0x2000);
        assert_eq!(class(Keyboard::Phone), 3);
        assert_eq!(class(Keyboard::Email), 1 | 0x20);
        assert_eq!(class(Keyboard::Url), 1 | 0x10);
    }

    #[test]
    fn actions_map_and_newline_keeps_enter() {
        for (key, action) in [
            (EnterKey::Send, 4),
            (EnterKey::Search, 3),
            (EnterKey::Go, 2),
            (EnterKey::Next, 5),
            (EnterKey::Done, 6),
        ] {
            let info = editor_info(&kind(Keyboard::Text, key, false, false));
            assert_eq!(info.action, action, "{key:?}");
            assert_eq!(info.options & 0xff, action as u32, "{key:?}");
            assert_eq!(info.options & NO_ENTER_ACTION, 0, "{key:?}");
        }
        let newline = editor_info(&kind(Keyboard::Text, EnterKey::Default, true, false));
        assert_eq!(newline.action, 1);
        assert_ne!(newline.options & NO_ENTER_ACTION, 0);
        assert_ne!(newline.input_type & MULTI_LINE, 0);
        let composer = editor_info(&kind(Keyboard::Text, EnterKey::Send, true, false));
        assert_eq!(composer.action, 4);
        assert_eq!(composer.options & NO_ENTER_ACTION, 0, "the Send key shows");
        // Gboard shows a newline key on any multi-line field, whatever its
        // action: a field with an action key is not flagged multi-line (the
        // editor still wraps it).
        assert_eq!(composer.input_type & MULTI_LINE, 0);
    }

    #[test]
    fn a_masked_number_is_a_number_password() {
        let pin = editor_info(&kind(Keyboard::Number, EnterKey::Default, false, true));
        assert_eq!(pin.input_type, 2 | 0x10);
        assert_ne!(pin.options & 0x0100_0000, 0, "nothing learned");
    }

    #[test]
    fn ios_traits_map_each_keyboard() {
        let kb = |k| text_input_traits(&kind(k, EnterKey::Default, false, false)).keyboard;
        assert_eq!(kb(Keyboard::Text), IosKeyboard::Default);
        assert_eq!(kb(Keyboard::Number), IosKeyboard::NumberPad);
        assert_eq!(kb(Keyboard::Decimal), IosKeyboard::DecimalPad);
        assert_eq!(kb(Keyboard::Phone), IosKeyboard::PhonePad);
        assert_eq!(kb(Keyboard::Email), IosKeyboard::EmailAddress);
        assert_eq!(kb(Keyboard::Url), IosKeyboard::Url);
    }

    #[test]
    fn ios_traits_map_each_enter_key() {
        let ret = |key, multiline| {
            text_input_traits(&kind(Keyboard::Text, key, multiline, false)).return_key
        };
        assert_eq!(ret(EnterKey::Send, false), IosReturn::Send);
        assert_eq!(ret(EnterKey::Search, false), IosReturn::Search);
        assert_eq!(ret(EnterKey::Go, false), IosReturn::Go);
        assert_eq!(ret(EnterKey::Next, false), IosReturn::Next);
        assert_eq!(ret(EnterKey::Done, false), IosReturn::Done);
        // A single-line field's default enter key is Done; a multiline
        // field's is a newline, which iOS shows as its plain return key.
        assert_eq!(ret(EnterKey::Default, false), IosReturn::Done);
        assert_eq!(ret(EnterKey::Default, true), IosReturn::Default);
        assert_eq!(
            ret(EnterKey::Send, true),
            IosReturn::Send,
            "a composer's Send key"
        );
    }

    #[test]
    fn a_password_turns_off_suggestions() {
        let plain = text_input_traits(&kind(Keyboard::Text, EnterKey::Default, false, false));
        assert!(!plain.secure && plain.autocorrect && plain.autocapitalize);
        let secret = text_input_traits(&kind(Keyboard::Text, EnterKey::Default, false, true));
        assert!(secret.secure && !secret.autocorrect && !secret.autocapitalize);
        for k in [
            Keyboard::Email,
            Keyboard::Url,
            Keyboard::Number,
            Keyboard::Phone,
        ] {
            let t = text_input_traits(&kind(k, EnterKey::Default, false, false));
            assert!(!t.autocorrect && !t.autocapitalize, "{k:?}");
        }
    }

    #[test]
    fn a_field_switch_repushes_editor_info() {
        let mut sync = ImeSync::default();
        let email = kind(Keyboard::Email, EnterKey::Next, false, false);
        sync.pushed(Some(email.clone()), 0.0);
        assert_eq!(sync.held_editor_info(), Some(editor_info(&email)));
        let number = ImeTextState {
            field: Some(WidgetId(2)),
            ..kind(Keyboard::Number, EnterKey::Done, false, false)
        };
        assert_ne!(Some(editor_info(&number)), sync.held_editor_info());
        // A report from the IME keeps the field's kind.
        let echoed = recv(&mut sync, "a", 0.1).expect("a report");
        assert_eq!(editor_info(&echoed), editor_info(&email));
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
        sync.pushed(Some(secret.clone()), 0.0);
        sync.pushed(None, 1.0);
        assert_eq!(
            sync.held_editor_info(),
            Some(editor_info(&secret)),
            "the password editor info is still set"
        );
        sync.pushed(Some(state("", 2)), 2.0);
        assert_eq!(sync.held_editor_info(), Some(editor_info(&state("", 2))));
        // GameActivity's echo of the old buffer after the switch is stale.
        assert_eq!(recv(&mut sync, "pw", 2.001), None);
        assert_eq!(recv(&mut sync, "", 2.03), Some(state("", 2)));
    }
}
