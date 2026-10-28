//! Single-line text editing on parley's `PlainEditor`: per-widget editor
//! state, keyboard/pointer handling, and painting (selection, caret,
//! placeholder, horizontal follow-scroll).

use kurbo::{Affine, Rect};
use parley::PositionedLayoutItem;
use peniko::{Color, Fill};

use crate::clipboard::Clipboard;
use crate::draw::{DrawTarget, Glyph, GlyphRun};
use crate::events::{Key, KeyInput};
use crate::text::{Fonts, LayoutBrush, ResolvedText};

/// Caret blink period: 530ms visible, 530ms hidden.
const BLINK_HALF_PERIOD: f64 = 0.53;
/// Caret stroke width in logical px.
const CARET_WIDTH: f64 = 1.5;
/// The glyph a masked field draws in place of each character.
const MASK_CHAR: char = '\u{2022}';
/// The mask glyph's UTF-8 length. The mask is a run of them, so a byte
/// offset in the mask is this many times its character index.
const MASK_LEN: usize = MASK_CHAR.len_utf8();

/// Retained editing state for one input widget.
pub(crate) struct EditorState {
    pub editor: parley::PlainEditor<LayoutBrush>,
    /// Horizontal scroll keeping the caret visible in a narrow box.
    pub scroll_x: f64,
    /// Clock time of the last edit or caret move (restarts the blink).
    pub last_activity: f64,
    /// Frame stamp for garbage collection.
    pub seen: u64,
    /// Multiline mode: wraps, accepts newlines, moves by line.
    pub multiline: bool,
    /// Read-only mode: selection and copy stay live, every mutation is a
    /// no-op (HTML `<input readonly>`).
    pub read_only: bool,
    /// Character cap on the value (OTP digit boxes cap at 1). Insertions
    /// past the cap are ignored; paste truncates.
    pub max_chars: Option<usize>,
    /// Masked mode (a password field): the value is drawn as bullets. The
    /// buffer still holds the real text — only the painting differs, so
    /// editing, undo, copy, and the app's value are untouched.
    pub masked: bool,
    /// The on-screen keyboard the field asks for.
    pub keyboard: crate::Keyboard,
    /// The enter key as the element declared it.
    pub declared_enter: crate::EnterKey,
    /// The enter key resolved ([`crate::resolve_enter_key`]): `Newline`
    /// alone makes Enter a line break.
    pub enter_key: crate::EnterKey,
    /// The resolved text style, kept so the pointer handlers can lay out the
    /// bullets they hit-test against.
    pub style: ResolvedText,
    /// The bullets standing in for the value, and the character count they
    /// were built for.
    mask: String,
    mask_count: usize,
    /// Undo/redo history (QUndoStack semantics: coalesced runs,
    /// boundaries on caret moves, redo cleared by new edits).
    pub undo: UndoStack,
}

/// What kind of edit a coalescing run holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditRun {
    Insert,
    Delete,
}

/// One restorable editor moment: text plus the selected byte range.
#[derive(Debug, Clone)]
struct Snapshot {
    text: String,
    selection: (usize, usize),
}

/// Bounded undo/redo stacks with typing-run coalescing.
#[derive(Debug, Default)]
pub(crate) struct UndoStack {
    undos: Vec<Snapshot>,
    redos: Vec<Snapshot>,
    /// The open coalescing run; edits of the same kind merge into it.
    run: Option<EditRun>,
}

/// History depth: plenty for a field, bounded so editors never grow
/// without limit.
const UNDO_LIMIT: usize = 100;

fn snapshot(editor: &parley::PlainEditor<LayoutBrush>) -> Snapshot {
    let range = editor.raw_selection().text_range();
    Snapshot {
        text: editor.raw_text().to_owned(),
        selection: (range.start, range.end),
    }
}

impl UndoStack {
    /// Opens (or continues) a coalescing run of `kind`: the first edit
    /// of a run snapshots the pre-edit state and clears redos.
    fn begin(&mut self, editor: &parley::PlainEditor<LayoutBrush>, kind: EditRun) {
        if self.run != Some(kind) {
            self.undos.push(snapshot(editor));
            if self.undos.len() > UNDO_LIMIT {
                self.undos.remove(0);
            }
            self.redos.clear();
            self.run = Some(kind);
        }
    }

    /// Ends the open run: the next edit starts a fresh undo unit.
    /// Called on caret/selection moves and external value changes.
    pub(crate) fn break_run(&mut self) {
        self.run = None;
    }
}

impl EditorState {
    pub(crate) fn new(style: &ResolvedText, now: f64, multiline: bool) -> Self {
        let mut editor = parley::PlainEditor::new(style.px);
        apply_style(&mut editor, style, None, None);
        editor.set_width(None); // multiline editors get their width at paint
        Self {
            editor,
            scroll_x: 0.0,
            last_activity: now,
            seen: 0,
            multiline,
            read_only: false,
            max_chars: None,
            masked: false,
            keyboard: crate::Keyboard::Text,
            declared_enter: crate::EnterKey::Default,
            enter_key: crate::EnterKey::Default,
            style: *style,
            mask: String::new(),
            mask_count: 0,
            undo: UndoStack::default(),
        }
    }

    /// Syncs the editor with the app-provided value and text style. The app
    /// is the source of truth: external changes reset the buffer.
    /// **In a multiline editor the caret lands after the text, not before
    /// it.** `set_text` leaves it at position 0, which is wrong for the
    /// reasons an app sets a value: filling a field is for the person to
    /// carry on from, and recalling a message is for editing the end of it.
    /// It also made the value unreachable from below — Down from the start of
    /// a one-line value *moves*, so the editor reported MOVED and kept a key
    /// the app was waiting for, and a composer that recalled what it sent
    /// could go up but never come back.
    ///
    /// Single-line editors keep the old landing, because something already
    /// depends on it: an OTP box overwrites in place by having the committed
    /// character inserted at the front and letting the widget keep the first
    /// one (`tests/otp.rs`). Moving that caret is a separate change that owes
    /// that widget a new answer.
    pub(crate) fn sync(&mut self, value: &str, style: &ResolvedText, fonts: &mut Fonts) {
        // The style is kept for the masked path, which lays out the bullets
        // the pointer handlers hit-test against.
        self.style = *style;
        // **A composing editor is left alone.** Its text carries the preedit
        // and the app's value does not — a preedit raises no `on_input`, on
        // purpose, because half-typed pinyin is not the field's value. So the
        // two can never match while composing, and the comparison below would
        // call `set_text` on every frame and drop the composition. Measured:
        // it did not survive one rebuild, which is one redraw, which is what
        // every runner does after the preedit that started it.
        if self.editor.is_composing() {
            apply_style(
                &mut self.editor,
                style,
                fonts.role_family(style.family),
                fonts.locale(),
            );
            return;
        }
        if self.editor.raw_text() != value {
            let fresh = self.editor.raw_text().is_empty()
                && self.undo.undos.is_empty()
                && self.undo.redos.is_empty();
            if fresh {
                // First fill of a brand-new editor: nothing to undo.
                self.editor.set_text(value);
                apply_style(
                    &mut self.editor,
                    style,
                    fonts.role_family(style.family),
                    fonts.locale(),
                );
                self.caret_to_end(fonts);
                return;
            }
            // External (programmatic) change: its own undo unit, so
            // undo can step back across it.
            self.undo.undos.push(snapshot(&self.editor));
            if self.undo.undos.len() > UNDO_LIMIT {
                self.undo.undos.remove(0);
            }
            self.undo.redos.clear();
            self.undo.break_run();
            self.editor.set_text(value);
            apply_style(
                &mut self.editor,
                style,
                fonts.role_family(style.family),
                fonts.locale(),
            );
            self.caret_to_end(fonts);
            return;
        }
        apply_style(
            &mut self.editor,
            style,
            fonts.role_family(style.family),
            fonts.locale(),
        );
    }

    /// Puts the caret after the last character. Styles have to be applied
    /// first: the driver lays the text out to find the position, and laying
    /// it out with the wrong size would put the caret somewhere else.
    fn caret_to_end(&mut self, fonts: &mut Fonts) {
        if !self.multiline {
            return;
        }
        let (font_cx, layout_cx) = fonts.editor_contexts();
        self.editor.driver(font_cx, layout_cx).move_to_text_end();
    }
}

/// The bullet text standing in for `text`: one bullet per character.
///
/// Cached in the editor state — the mask depends only on the value's length,
/// so the string is rebuilt on a length change and reused otherwise.
fn mask_text<'a>(text: &str, cache: &'a mut String, count: &mut usize) -> &'a str {
    let n = text.chars().count();
    if n != *count {
        cache.clear();
        cache.extend(std::iter::repeat_n(MASK_CHAR, n));
        *count = n;
    }
    cache
}

/// The byte offset in the mask text that stands for the value's byte offset
/// `byte`: the n-th character of the value is the n-th bullet.
fn mask_offset(text: &str, byte: usize) -> usize {
    MASK_LEN * text.char_indices().take_while(|(i, _)| *i < byte).count()
}

/// The byte offset in the value that the mask's byte offset `mask_byte`
/// stands for.
fn value_offset(text: &str, mask_byte: usize) -> usize {
    text.char_indices()
        .nth(mask_byte / MASK_LEN)
        .map_or(text.len(), |(byte, _)| byte)
}

fn apply_style(
    editor: &mut parley::PlainEditor<LayoutBrush>,
    style: &ResolvedText,
    registered_family: Option<&str>,
    locale: Option<parley::Language>,
) {
    use parley::{FontFamily, FontWeight, GenericFamily, LineHeight, StyleProperty};
    let styles = editor.edit_styles();
    styles.insert(StyleProperty::Locale(locale));
    styles.insert(StyleProperty::FontSize(style.px));
    styles.insert(StyleProperty::FontWeight(FontWeight::new(style.weight)));
    styles.insert(StyleProperty::LineHeight(LineHeight::FontSizeRelative(
        style.line_height,
    )));
    styles.insert(StyleProperty::LetterSpacing(style.letter_spacing));
    // Features and optical sizing are insert-or-*remove*: re-applying a style
    // that turned a feature (or the opsz axis) off must clear the prior property,
    // not leave it stuck on a persistent editor (the 0.16 known limitation).
    // `edit_styles` is a discriminant-keyed set, so removal by variant is exact.
    match style.features.feature_string() {
        Some(s) => {
            styles.insert(StyleProperty::FontFeatures(parley::FontFeatures::Source(
                std::borrow::Cow::Owned(s),
            )));
        }
        None => {
            styles.remove(std::mem::discriminant(&StyleProperty::FontFeatures(
                parley::FontFeatures::empty(),
            )));
        }
    }
    // Optical sizing (no-op on the editor's static Inter; correct if an app
    // registers a variable face).
    match style.optical.opsz_at(style.px) {
        Some(opsz) => {
            styles.insert(StyleProperty::FontVariations(
                parley::FontVariations::Source(std::borrow::Cow::Owned(format!("\"opsz\" {opsz}"))),
            ));
        }
        None => {
            styles.remove(std::mem::discriminant(&StyleProperty::FontVariations(
                parley::FontVariations::empty(),
            )));
        }
    }
    let family = if let Some(name) = registered_family {
        FontFamily::Source(std::borrow::Cow::Owned(format!(
            "\"{}\"",
            name.replace('\\', "\\\\").replace('"', "\\\"")
        )))
    } else {
        match style.family {
            crate::tokens::FamilyRole::Mono => FontFamily::Single(GenericFamily::Monospace.into()),
            _ => FontFamily::named("Inter"),
        }
    };
    styles.insert(StyleProperty::FontFamily(family));
}

/// The outcome of one editor event.
pub(crate) struct EditOutcome {
    /// The buffer changed (emit `on_input`).
    pub changed: bool,
    /// The event was consumed by the editor (do not forward to `on_key`).
    pub consumed: bool,
}

const HANDLED: EditOutcome = EditOutcome {
    changed: true,
    consumed: true,
};
const MOVED: EditOutcome = EditOutcome {
    changed: false,
    consumed: true,
};
const IGNORED: EditOutcome = EditOutcome {
    changed: false,
    consumed: false,
};

/// Applies a key press to the editor. Word jumps follow the common
/// conventions: Alt+arrow (mac) or Ctrl+arrow jump words; Cmd/Ctrl+arrow or
/// Home/End jump to the line edges; Cmd/Ctrl+A/C/X/V are the selection and
/// clipboard shortcuts.
pub(crate) fn handle_key(
    state: &mut EditorState,
    fonts: &mut Fonts,
    clipboard: &mut dyn Clipboard,
    key: &KeyInput,
) -> EditOutcome {
    // Candidate-selection keys belong to the input method while it is composing.
    // Editing here would add a newline/delete text before the IME commit arrives.
    if state.editor.is_composing() {
        return MOVED;
    }
    let outcome = handle_key_inner(state, fonts, clipboard, key);
    if !outcome.changed {
        // Caret/selection moves are coalescing boundaries: the next
        // edit starts a fresh undo unit.
        state.undo.break_run();
    }
    outcome
}

fn handle_key_inner(
    state: &mut EditorState,
    fonts: &mut Fonts,
    clipboard: &mut dyn Clipboard,
    key: &KeyInput,
) -> EditOutcome {
    let multiline = state.multiline;
    // Only a newline key makes Enter a line break; any other leaves it to
    // the app (`submit_on_enter` is folded into the resolved key).
    let enter_to_app = state.enter_key != crate::EnterKey::Newline;
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    let shortcut = key.meta || key.ctrl;
    let word = key.alt || (key.ctrl && !key.meta);

    match key.key {
        Key::Char(c) if shortcut => match c.to_ascii_lowercase() {
            'a' => {
                drv.select_all();
                MOVED
            }
            'c' => {
                if let Some(text) = drv.editor.selected_text() {
                    clipboard.set(text.to_owned());
                }
                MOVED
            }
            'x' => {
                // Read-only: cut/paste/undo/typing are all no-ops; only
                // selection and copy stay live (HTML `readonly`).
                if state.read_only {
                    return IGNORED;
                }
                if let Some(text) = drv.editor.selected_text() {
                    clipboard.set(text.to_owned());
                    // Cut is its own undo unit, never coalesced.
                    state.undo.break_run();
                    state.undo.begin(&state.editor, EditRun::Delete);
                    let (font_cx, layout_cx) = fonts.editor_contexts();
                    let mut drv = state.editor.driver(font_cx, layout_cx);
                    drv.delete_selection();
                    state.undo.break_run();
                    return HANDLED;
                }
                MOVED
            }
            'v' => {
                if state.read_only {
                    return IGNORED;
                }
                if let Some(text) = clipboard.get() {
                    // Paste is its own undo unit, never coalesced.
                    state.undo.break_run();
                    state.undo.begin(&state.editor, EditRun::Insert);
                    let (font_cx, layout_cx) = fonts.editor_contexts();
                    let mut drv = state.editor.driver(font_cx, layout_cx);
                    drv.insert_or_replace_selection(&sanitize(&text, multiline));
                    state.undo.break_run();
                    return HANDLED;
                }
                MOVED
            }
            'z' => {
                if state.read_only {
                    return IGNORED;
                }
                let applied = if key.shift {
                    redo(state, fonts)
                } else {
                    undo(state, fonts)
                };
                if applied { HANDLED } else { MOVED }
            }
            'y' if key.ctrl => {
                if state.read_only {
                    return IGNORED;
                }
                if redo(state, fonts) { HANDLED } else { MOVED }
            }
            _ => IGNORED,
        },
        // Control characters (Enter arriving as '\r', Tab, DEL, ...) never
        // become text, matching the text-commit and paste path filters.
        Key::Char(c) if c.is_control() => IGNORED,
        Key::Char(c) if !key.ctrl && !key.meta => {
            if state.read_only {
                return IGNORED;
            }
            // A full capped box takes no more characters (OTP auto-advance
            // keys off exactly this saturation).
            if state
                .max_chars
                .is_some_and(|m| state.editor.raw_text().chars().count() >= m)
            {
                return IGNORED;
            }
            state.undo.begin(&state.editor, EditRun::Insert);
            let (font_cx, layout_cx) = fonts.editor_contexts();
            let mut drv = state.editor.driver(font_cx, layout_cx);
            drv.insert_or_replace_selection(&c.to_string());
            HANDLED
        }
        Key::ArrowLeft => {
            match (key.shift, word, key.meta) {
                (true, _, true) => drv.select_to_line_start(),
                (false, _, true) => drv.move_to_line_start(),
                (true, true, _) => drv.select_word_left(),
                (false, true, _) => drv.move_word_left(),
                (true, false, _) => drv.select_left(),
                (false, false, _) => drv.move_left(),
            }
            MOVED
        }
        Key::ArrowRight => {
            match (key.shift, word, key.meta) {
                (true, _, true) => drv.select_to_line_end(),
                (false, _, true) => drv.move_to_line_end(),
                (true, true, _) => drv.select_word_right(),
                (false, true, _) => drv.move_word_right(),
                (true, false, _) => drv.select_right(),
                (false, false, _) => drv.move_right(),
            }
            MOVED
        }
        Key::Home => {
            if key.shift {
                drv.select_to_line_start();
            } else {
                drv.move_to_line_start();
            }
            MOVED
        }
        Key::End => {
            if key.shift {
                drv.select_to_line_end();
            } else {
                drv.move_to_line_end();
            }
            MOVED
        }
        // **Shift decides, when the caller asked it to.** A composer sends on
        // Enter and breaks a line on Shift+Enter; a form textarea makes a
        // paragraph either way. Without the first branch the editor handles
        // every Enter and an `on_key` on the element never fires, so the
        // composer's rule could be written and could not work.
        // Modified Enter is a form submission chord, never a paragraph.
        Key::Enter if multiline && (key.meta || key.ctrl) => IGNORED,
        Key::Enter if multiline && enter_to_app && !key.shift => IGNORED,
        Key::Enter if multiline => {
            if state.read_only {
                return IGNORED;
            }
            state.undo.begin(&state.editor, EditRun::Insert);
            let (font_cx, layout_cx) = fonts.editor_contexts();
            let mut drv = state.editor.driver(font_cx, layout_cx);
            drv.insert_or_replace_selection("\n");
            HANDLED
        }
        // **A caret that could not move was not handled.** `move_up` on the
        // first line and `move_down` on the last do nothing, and reporting
        // MOVED for them tells the app the key was consumed — so an app that
        // wants Up for something else at the edges (a composer recalling what
        // it sent last, the way a shell does) can never see it. Reporting
        // what actually happened costs nothing and hands that case over.
        //
        // Shift keeps the old answer: extending a selection to the edge is a
        // selection change even when the caret stays, and the app has no
        // business in a drag.
        Key::ArrowUp if multiline => {
            if key.shift {
                drv.select_up();
                return MOVED;
            }
            let before = state.editor.raw_selection().text_range();
            let (font_cx, layout_cx) = fonts.editor_contexts();
            state.editor.driver(font_cx, layout_cx).move_up();
            if state.editor.raw_selection().text_range() == before {
                IGNORED
            } else {
                MOVED
            }
        }
        Key::ArrowDown if multiline => {
            if key.shift {
                drv.select_down();
                return MOVED;
            }
            let before = state.editor.raw_selection().text_range();
            let (font_cx, layout_cx) = fonts.editor_contexts();
            state.editor.driver(font_cx, layout_cx).move_down();
            if state.editor.raw_selection().text_range() == before {
                IGNORED
            } else {
                MOVED
            }
        }
        Key::Backspace => {
            if state.read_only {
                return IGNORED;
            }
            state.undo.begin(&state.editor, EditRun::Delete);
            let (font_cx, layout_cx) = fonts.editor_contexts();
            let mut drv = state.editor.driver(font_cx, layout_cx);
            if word {
                drv.backdelete_word();
            } else {
                drv.backdelete();
            }
            HANDLED
        }
        Key::Delete => {
            if state.read_only {
                return IGNORED;
            }
            state.undo.begin(&state.editor, EditRun::Delete);
            let (font_cx, layout_cx) = fonts.editor_contexts();
            let mut drv = state.editor.driver(font_cx, layout_cx);
            if word {
                drv.delete_word();
            } else {
                drv.delete();
            }
            HANDLED
        }
        _ => IGNORED,
    }
}

/// Filters control characters out of committed or pasted text. Multiline
/// editors keep tabs and newlines (normalized to `\n`); single-line editors strip
/// them with everything else.
fn sanitize(text: &str, multiline: bool) -> String {
    if multiline {
        text.replace("\r\n", "\n")
            .chars()
            .map(|c| if c == '\r' { '\n' } else { c })
            .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
            .collect()
    } else {
        text.chars().filter(|c| !c.is_control()).collect()
    }
}

/// Inserts committed text (typing or IME commit).
/// Applies one undo step. Returns whether anything changed.
pub(crate) fn undo(state: &mut EditorState, fonts: &mut Fonts) -> bool {
    let Some(snap) = state.undo.undos.pop() else {
        return false;
    };
    state.undo.redos.push(snapshot(&state.editor));
    apply_snapshot(state, fonts, &snap);
    true
}

/// Applies one redo step. Returns whether anything changed.
pub(crate) fn redo(state: &mut EditorState, fonts: &mut Fonts) -> bool {
    let Some(snap) = state.undo.redos.pop() else {
        return false;
    };
    state.undo.undos.push(snapshot(&state.editor));
    apply_snapshot(state, fonts, &snap);
    true
}

fn apply_snapshot(state: &mut EditorState, fonts: &mut Fonts, snap: &Snapshot) {
    state.editor.set_text(&snap.text);
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    drv.select_byte_range(snap.selection.0, snap.selection.1);
    state.undo.break_run();
}

pub(crate) fn handle_text(state: &mut EditorState, fonts: &mut Fonts, text: &str) -> EditOutcome {
    if state.read_only {
        return IGNORED;
    }
    state.undo.begin(&state.editor, EditRun::Insert);
    let mut sanitized = sanitize(text, state.multiline);
    if let Some(max) = state.max_chars {
        sanitized = sanitized.chars().take(max).collect();
    }
    if sanitized.is_empty() {
        return IGNORED;
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    drv.insert_or_replace_selection(&sanitized);
    HANDLED
}

/// Drops an unfinished composition, preedit and all; returns whether there
/// was one. The preedit was never the field's value, so nothing is undone.
pub(crate) fn cancel_compose(state: &mut EditorState, fonts: &mut Fonts) -> bool {
    if !state.editor.is_composing() {
        return false;
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    state.editor.driver(font_cx, layout_cx).clear_compose();
    true
}

/// Updates the IME preedit text.
pub(crate) fn handle_preedit(
    state: &mut EditorState,
    fonts: &mut Fonts,
    text: &str,
    cursor: Option<(usize, usize)>,
) {
    if state.read_only {
        return;
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    if text.is_empty() {
        drv.clear_compose();
    } else {
        // Platform IMEs are not trusted to keep offsets in range, and
        // parley debug-asserts on out-of-range compose cursors.
        let cursor = cursor.map(|(a, b)| (a.min(text.len()), b.min(text.len())));
        drv.set_compose(text, cursor);
    }
}

/// Makes the editor match an IME's view of it ([`ImeTextState`]): the
/// committed value is brought in line with one minimal replacement (so undo
/// and `on_input` see a real edit), then the composition and selection are
/// laid over it. A value the field refuses (read-only, over `max_chars`)
/// leaves it as is; the runner then reports the field back to the IME.
pub(crate) fn apply_ime_state(
    state: &mut EditorState,
    fonts: &mut Fonts,
    ime: &crate::events::ImeTextState,
) -> EditOutcome {
    if state.read_only {
        return IGNORED;
    }
    let text = ime.text.as_str();
    // Platform IMEs are not trusted to keep offsets in range or on char
    // boundaries.
    let clamp = |i: usize| {
        let mut i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let compose = ime
        .compose
        .map(|(a, b)| (clamp(a.min(b)), clamp(a.max(b))))
        .filter(|(a, b)| a < b);
    let value = match compose {
        Some((a, b)) => [&text[..a], &text[b..]].concat(),
        None => text.to_owned(),
    };
    let current = state.editor.text().to_string();
    // The edit as one replaced range: the longest common prefix and suffix
    // (per char, so they never overlap) stay.
    let prefix: usize = current
        .chars()
        .zip(value.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let suffix: usize = current[prefix..]
        .chars()
        .rev()
        .zip(value[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let removed = prefix..current.len() - suffix;
    let raw = &value[prefix..value.len() - suffix];
    // Held to the same rules as typed text: no control characters (no line
    // break in a single line), and growth capped at `max_chars` — a value the
    // app set past the cap can still be deleted from.
    let mut inserted = sanitize(raw, state.multiline);
    if let Some(max) = state.max_chars {
        let kept = current.chars().count() - current[removed.clone()].chars().count();
        inserted = inserted.chars().take(max.saturating_sub(kept)).collect();
    }
    let changed = !removed.is_empty() || !inserted.is_empty();
    if changed {
        state.undo.begin(&state.editor, EditRun::Insert);
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    drv.clear_compose();
    if changed {
        drv.select_byte_range(removed.start, removed.end);
        if inserted.is_empty() {
            drv.delete_selection();
        } else {
            drv.insert_or_replace_selection(&inserted);
        }
    }
    if inserted != raw {
        // The IME's offsets describe text the field refused: put the caret
        // after what was kept; the runner resyncs the IME to the field.
        let caret = removed.start + inserted.len();
        drv.select_byte_range(caret, caret);
        return EditOutcome {
            changed,
            consumed: true,
        };
    }
    let (sa, sb) = (clamp(ime.selection.0), clamp(ime.selection.1));
    match compose {
        Some((a, b)) => {
            drv.select_byte_range(a, a);
            let cursor = if a <= sa.min(sb) && sa.max(sb) <= b {
                (sa - a, sb - a)
            } else {
                (b - a, b - a)
            };
            drv.set_compose(&text[a..b], Some(cursor));
        }
        None => drv.select_byte_range(sa, sb),
    }
    EditOutcome {
        changed,
        consumed: true,
    }
}

/// Moves the caret (or extends the selection) to a pointer position given in
/// coordinates local to the text origin.
pub(crate) fn pointer_down(
    state: &mut EditorState,
    fonts: &mut Fonts,
    x: f64,
    y: f64,
    shift: bool,
) {
    state.undo.break_run();
    if state.masked {
        let Some(offset) = masked_point(state, fonts, x, y) else {
            return;
        };
        let (font_cx, layout_cx) = fonts.editor_contexts();
        let mut drv = state.editor.driver(font_cx, layout_cx);
        if shift {
            drv.extend_selection_to_byte(offset);
        } else {
            drv.move_to_byte(offset);
        }
        return;
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
    if shift {
        drv.shift_click_extension(x as f32, y as f32);
    } else {
        drv.move_to_point(x as f32, y as f32);
    }
}

/// The value's byte offset nearest a pointer position (a cluster
/// boundary), in coordinates local to the text origin.
pub(crate) fn byte_at(state: &mut EditorState, fonts: &mut Fonts, x: f64, y: f64) -> usize {
    if state.masked {
        return masked_point(state, fonts, x, y).unwrap_or(0);
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let layout = state.editor.layout(font_cx, layout_cx);
    #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
    parley::Cursor::from_point(layout, x as f32, y as f32).index()
}

/// Selects a byte range (anchor, focus), as touch handles set it.
pub(crate) fn select_range(
    state: &mut EditorState,
    fonts: &mut Fonts,
    anchor: usize,
    focus: usize,
) {
    state.undo.break_run();
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    drv.select_byte_range(anchor, focus);
}

/// Selects the word at a pointer position (double-click).
pub(crate) fn select_word_at(state: &mut EditorState, fonts: &mut Fonts, x: f64, y: f64) {
    state.undo.break_run();
    if state.masked {
        let Some((start, end)) = masked_range(state, fonts, x, y, false) else {
            return;
        };
        let (font_cx, layout_cx) = fonts.editor_contexts();
        let mut drv = state.editor.driver(font_cx, layout_cx);
        drv.select_byte_range(start, end);
        return;
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
    drv.select_word_at_point(x as f32, y as f32);
}

/// Selects the whole line at a pointer position (triple-click).
pub(crate) fn select_line_at(state: &mut EditorState, fonts: &mut Fonts, x: f64, y: f64) {
    state.undo.break_run();
    if state.masked {
        let Some((start, end)) = masked_range(state, fonts, x, y, true) else {
            return;
        };
        let (font_cx, layout_cx) = fonts.editor_contexts();
        let mut drv = state.editor.driver(font_cx, layout_cx);
        drv.select_byte_range(start, end);
        return;
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
    drv.select_line_at_point(x as f32, y as f32);
}

/// Extends the selection during a drag.
pub(crate) fn pointer_drag(state: &mut EditorState, fonts: &mut Fonts, x: f64, y: f64) {
    state.undo.break_run();
    if state.masked {
        let Some(offset) = masked_point(state, fonts, x, y) else {
            return;
        };
        let (font_cx, layout_cx) = fonts.editor_contexts();
        let mut drv = state.editor.driver(font_cx, layout_cx);
        drv.extend_selection_to_byte(offset);
        return;
    }
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let mut drv = state.editor.driver(font_cx, layout_cx);
    #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
    drv.extend_selection_to_point(x as f32, y as f32);
}

/// The value's byte offset for a pointer position in a masked field.
///
/// The pointer hits the bullets, and the n-th bullet stands for the n-th
/// character of the value, so the bullet's byte offset maps straight back.
fn masked_point(state: &mut EditorState, fonts: &mut Fonts, x: f64, y: f64) -> Option<usize> {
    let EditorState {
        editor,
        mask,
        mask_count,
        style,
        ..
    } = state;
    let value = editor.raw_text();
    let mask_text = mask_text(value, mask, mask_count);
    let layout = fonts.layout(mask_text, style, None);
    #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
    let hit = parley::Selection::from_point(layout, x as f32, y as f32);
    Some(value_offset(value, hit.focus().index()))
}

/// The value's byte range for the word (or the line) at a pointer position in
/// a masked field: the word is chosen in the bullet text, and the bullets it
/// covers stand for the same characters of the value.
fn masked_range(
    state: &mut EditorState,
    fonts: &mut Fonts,
    x: f64,
    y: f64,
    line: bool,
) -> Option<(usize, usize)> {
    let EditorState {
        editor,
        mask,
        mask_count,
        style,
        ..
    } = state;
    let value = editor.raw_text();
    let mask_text = mask_text(value, mask, mask_count);
    let layout = fonts.layout(mask_text, style, None);
    #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
    let hit = if line {
        parley::Selection::line_from_point(layout, x as f32, y as f32)
    } else {
        parley::Selection::word_from_point(layout, x as f32, y as f32)
    };
    let range = hit.text_range();
    Some((
        value_offset(value, range.start),
        value_offset(value, range.end),
    ))
}

/// Where [`paint`] puts the editor's layout origin inside `rect`, at the
/// current follow-scroll: the inverse of `input_local` in events.
pub(crate) fn text_origin(
    state: &mut EditorState,
    fonts: &mut Fonts,
    data: &InputPaint,
    rect: Rect,
) -> kurbo::Point {
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let height = f64::from(state.editor.layout(font_cx, layout_cx).height());
    let y = if data.multiline {
        rect.y0 + data.pad_y
    } else {
        rect.y0 + (rect.height() - height) * 0.5
    };
    kurbo::Point::new(rect.x0 + data.pad_x - state.scroll_x, y)
}

/// Everything the painter needs for one input, resolved at build time.
pub(crate) struct InputPaint {
    pub placeholder: String,
    pub style: ResolvedText,
    pub placeholder_color: Color,
    pub caret_color: Color,
    pub selection_color: Color,
    pub focused: bool,
    /// Left/right padding inside the box, logical px.
    pub pad_x: f64,
    /// Top padding inside the box (multiline is top-aligned), logical px.
    pub pad_y: f64,
    /// Multiline mode: wrapped, top-aligned, no horizontal follow-scroll.
    pub multiline: bool,
    /// Masked mode: the value is drawn as one bullet per character.
    pub masked: bool,
}

/// Paints an input's content (selection, text or placeholder, caret) inside
/// `rect`, clipped, vertically centered, with horizontal follow-scroll.
/// Returns the focused caret rect (for IME positioning), if any.
pub(crate) fn paint<T: DrawTarget>(
    target: &mut T,
    fonts: &mut Fonts,
    state: &mut EditorState,
    data: &InputPaint,
    rect: Rect,
    now: f64,
    reduced_motion: bool,
) -> Option<Rect> {
    let content = Rect::new(rect.x0 + data.pad_x, rect.y0, rect.x1 - data.pad_x, rect.y1);

    // A masked field paints bullets, not the value.
    if data.masked {
        return paint_masked(target, fonts, state, data, rect, now, reduced_motion);
    }

    // Multiline editors wrap to the content width (set before layout).
    if data.multiline {
        #[expect(clippy::cast_possible_truncation, reason = "widths fit in f32")]
        state
            .editor
            .set_width(Some(content.width().max(0.0) as f32));
    }

    // Refresh the layout; single lines center vertically, multiline is
    // top-aligned under the padding.
    let (font_cx, layout_cx) = fonts.editor_contexts();
    let layout_height = f64::from(state.editor.layout(font_cx, layout_cx).height());
    let text_y = if data.multiline {
        rect.y0 + data.pad_y
    } else {
        rect.y0 + (rect.height() - layout_height) * 0.5
    };

    // Follow-scroll: keep the caret inside the content box (single line
    // only; multiline grows vertically instead).
    if data.multiline {
        state.scroll_x = 0.0;
    } else if let Some(caret) = state.editor.cursor_geometry(1.0) {
        let caret_x = caret.x0; // layout-local
        let visible_w = content.width().max(0.0);
        if caret_x - state.scroll_x > visible_w {
            state.scroll_x = caret_x - visible_w;
        }
        if caret_x - state.scroll_x < 0.0 {
            state.scroll_x = caret_x;
        }
        let max_scroll = (f64::from(
            state
                .editor
                .try_layout()
                .map_or(0.0, parley::Layout::full_width),
        ) - visible_w)
            .max(0.0);
        state.scroll_x = state.scroll_x.clamp(0.0, max_scroll + CARET_WIDTH);
    } else {
        state.scroll_x = 0.0;
    }
    let text_origin = (content.x0 - state.scroll_x, text_y);

    target.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect);

    // Selection behind the glyphs.
    if data.focused {
        for (bb, _) in state.editor.selection_geometry() {
            let sel = Rect::new(
                text_origin.0 + bb.x0,
                text_origin.1 + bb.y0,
                text_origin.0 + bb.x1,
                text_origin.1 + bb.y1,
            );
            target.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                data.selection_color,
                None,
                &sel,
            );
        }
    }

    // Text, or the placeholder when empty.
    if state.editor.raw_text().is_empty() {
        if !data.placeholder.is_empty() {
            let mut placeholder_style = data.style;
            placeholder_style.color = data.placeholder_color;
            let ph_bottom = if data.multiline {
                rect.y1 - data.pad_y
            } else {
                text_y + layout_height
            };
            let ph_rect = Rect::new(content.x0, text_y, content.x1, ph_bottom);
            fonts.paint(target, &data.placeholder, &placeholder_style, ph_rect, None);
        }
    } else {
        let color = data.style.color;
        let (font_cx, layout_cx) = fonts.editor_contexts();
        let layout = state.editor.layout(font_cx, layout_cx);
        draw_text(target, layout, text_origin, color);
    }

    // The caret, blinking on a 530ms half-period. The rect is computed
    // whenever focused (even while blink-hidden) so the IME popup can
    // anchor to it.
    let mut caret_rect = None;
    if data.focused {
        let phase = ((now - state.last_activity).max(0.0) / BLINK_HALF_PERIOD) as u64;
        let visible = reduced_motion || phase.is_multiple_of(2);
        if let Some(bb) = state.editor.cursor_geometry(1.0) {
            let caret = Rect::new(
                (text_origin.0 + bb.x0 - CARET_WIDTH * 0.5).max(rect.x0),
                text_origin.1 + bb.y0 + 1.0,
                (text_origin.0 + bb.x0 + CARET_WIDTH * 0.5).max(rect.x0 + CARET_WIDTH),
                text_origin.1 + bb.y1 - 1.0,
            );
            caret_rect = Some(caret);
            if visible {
                target.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    data.caret_color,
                    None,
                    &caret,
                );
            }
        }
    }

    target.pop_layer();
    caret_rect
}

/// Paints a masked field: the bullets that stand in for the value, with the
/// selection and the caret over them.
///
/// The editor keeps the real text, so every offset here is a value offset
/// mapped into the bullet text: the n-th character of the value is the n-th
/// bullet, and a byte offset in the mask is three times its character index.
/// The bullets are laid out on their own — laying out the value and drawing
/// a bullet over each glyph would leak the width of every character, and the
/// spacing of the bullets would say as much about the secret as the glyphs
/// did.
fn paint_masked<T: DrawTarget>(
    target: &mut T,
    fonts: &mut Fonts,
    state: &mut EditorState,
    data: &InputPaint,
    rect: Rect,
    now: f64,
    reduced_motion: bool,
) -> Option<Rect> {
    let content = Rect::new(rect.x0 + data.pad_x, rect.y0, rect.x1 - data.pad_x, rect.y1);
    let EditorState {
        editor,
        scroll_x,
        last_activity,
        mask,
        mask_count,
        style,
        ..
    } = state;
    let value = editor.raw_text();
    let mask_text = mask_text(value, mask, mask_count);
    let layout = fonts.layout(mask_text, style, None);

    // The value's offsets, mapped into the bullet text.
    let selection = editor.raw_selection().text_range();
    let caret_byte = editor.raw_selection().focus().index();
    let caret = parley::Cursor::from_byte_index(
        layout,
        mask_offset(value, caret_byte),
        parley::Affinity::Downstream,
    )
    .geometry(layout, 1.0);

    let layout_height = f64::from(layout.height());
    let text_y = rect.y0 + (rect.height() - layout_height) * 0.5;

    // Follow-scroll: keep the caret inside the content box.
    let caret_x = caret.x0; // layout-local
    let visible_w = content.width().max(0.0);
    if caret_x - *scroll_x > visible_w {
        *scroll_x = caret_x - visible_w;
    }
    if caret_x - *scroll_x < 0.0 {
        *scroll_x = caret_x;
    }
    let max_scroll = (f64::from(layout.full_width()) - visible_w).max(0.0);
    *scroll_x = scroll_x.clamp(0.0, max_scroll + CARET_WIDTH);

    let text_origin = (content.x0 - *scroll_x, text_y);

    target.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect);

    // Selection behind the bullets.
    if data.focused {
        let start = mask_offset(value, selection.start);
        let end = mask_offset(value, selection.end);
        let sel = parley::Selection::new(
            parley::Cursor::from_byte_index(layout, start, parley::Affinity::Downstream),
            parley::Cursor::from_byte_index(layout, end, parley::Affinity::Downstream),
        );
        for (bb, _) in sel.geometry(layout) {
            let rect = Rect::new(
                text_origin.0 + bb.x0,
                text_origin.1 + bb.y0,
                text_origin.0 + bb.x1,
                text_origin.1 + bb.y1,
            );
            target.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                data.selection_color,
                None,
                &rect,
            );
        }
    }

    // The bullets, or the placeholder when the value is empty.
    if value.is_empty() {
        if !data.placeholder.is_empty() {
            let mut placeholder_style = data.style;
            placeholder_style.color = data.placeholder_color;
            let ph_rect = Rect::new(content.x0, text_y, content.x1, text_y + layout_height);
            fonts.paint(target, &data.placeholder, &placeholder_style, ph_rect, None);
        }
    } else {
        draw_text(target, layout, text_origin, data.style.color);
    }

    // The caret, blinking on a 530ms half-period. The rect is computed
    // whenever focused (even while blink-hidden) so the IME popup can
    // anchor to it. A composing editor hides its caret, as an ordinary one
    // does.
    let mut caret_rect = None;
    if data.focused && !editor.is_composing() {
        let phase = ((now - *last_activity).max(0.0) / BLINK_HALF_PERIOD) as u64;
        let visible = reduced_motion || phase.is_multiple_of(2);
        let caret = Rect::new(
            (text_origin.0 + caret.x0 - CARET_WIDTH * 0.5).max(rect.x0),
            text_origin.1 + caret.y0 + 1.0,
            (text_origin.0 + caret.x0 + CARET_WIDTH * 0.5).max(rect.x0 + CARET_WIDTH),
            text_origin.1 + caret.y1 - 1.0,
        );
        caret_rect = Some(caret);
        if visible {
            target.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                data.caret_color,
                None,
                &caret,
            );
        }
    }

    target.pop_layer();
    caret_rect
}

/// Draws a laid-out text into the scene, its origin at `origin`.
fn draw_text<T: DrawTarget>(
    target: &mut T,
    layout: &parley::Layout<LayoutBrush>,
    origin: (f64, f64),
    color: Color,
) {
    let transform = Affine::translate(origin);
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let mut x = glyph_run.offset();
            let y = glyph_run.baseline();
            let run = glyph_run.run();
            target.draw_glyphs(
                &GlyphRun {
                    font: run.font(),
                    font_size: run.font_size(),
                    hint: true,
                    normalized_coords: run.normalized_coords(),
                    transform,
                    glyph_transform: None,
                    color,
                },
                glyph_run.glyphs().map(|glyph| {
                    let gx = x + glyph.x;
                    let gy = y + glyph.y;
                    x += glyph.advance;
                    Glyph {
                        id: glyph.id,
                        x: gx,
                        y: gy,
                    }
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn multiline_paste_keeps_tsv_delimiters() {
        assert_eq!(super::sanitize("A\t1\r\nB\t2\0", true), "A\t1\nB\t2");
        assert_eq!(super::sanitize("A\t1\n", false), "A1");
    }

    use super::{LayoutBrush, apply_style};
    use crate::style::{NumericSpacing, OpticalSizing, TextStyle};
    use crate::text::resolve_text;
    use crate::theme::Theme;
    use std::mem::discriminant;

    #[test]
    fn persistent_editor_resolves_registered_font_roles() {
        let mut fonts = crate::text::Fonts::embedded();
        fonts.register(
            crate::tokens::FamilyRole::Mono,
            include_bytes!("../assets/inter/Inter-Regular.otf").to_vec(),
        );
        let style = crate::text::resolve_text(
            &TextStyle {
                family: crate::tokens::FamilyRole::Mono,
                ..Default::default()
            },
            &Theme::light(),
        );
        let mut state = super::EditorState::new(&style, 0.0, true);
        state.sync("Registered input", &style, &mut fonts);
        let key = discriminant(&parley::StyleProperty::<LayoutBrush>::FontFamily(
            parley::FontFamily::named("unused"),
        ));
        assert!(matches!(state.editor.edit_styles().inner().get(&key),
            Some(parley::StyleProperty::FontFamily(parley::FontFamily::Source(name))) if name == "\"Inter\""));
    }

    /// Toggling an OpenType feature (or the opsz axis) off on a *persistent*
    /// editor must clear the prior property, not leave it stuck (the 0.16
    /// known limitation). White-box: inspect the editor's resolved style set.
    #[test]
    fn editor_clears_toggled_off_feature_and_opsz() {
        let theme = Theme::light();
        let mut editor = parley::PlainEditor::<LayoutBrush>::new(16.0);

        let feat_key = discriminant(&parley::StyleProperty::<LayoutBrush>::FontFeatures(
            parley::FontFeatures::empty(),
        ));
        let var_key = discriminant(&parley::StyleProperty::<LayoutBrush>::FontVariations(
            parley::FontVariations::empty(),
        ));

        // Apply a style WITH tabular figures and auto optical sizing.
        let on = TextStyle {
            features: crate::style::FontFeatures {
                spacing: NumericSpacing::Tabular,
                ..Default::default()
            },
            optical: OpticalSizing::Auto,
            ..Default::default()
        };
        apply_style(&mut editor, &resolve_text(&on, &theme), None, None);
        assert!(
            editor.edit_styles().inner().contains_key(&feat_key),
            "feature property is set when enabled"
        );
        assert!(
            editor.edit_styles().inner().contains_key(&var_key),
            "opsz variation is set under Auto"
        );

        // Re-apply a cleared style (features off, opsz explicitly off): both must
        // clear. opsz is `Default` (not the new `Inherit`, which would track the
        // theme's `Auto` and keep the variation set).
        let off = TextStyle {
            optical: OpticalSizing::Default,
            ..Default::default()
        };
        apply_style(&mut editor, &resolve_text(&off, &theme), None, None);
        assert!(
            !editor.edit_styles().inner().contains_key(&feat_key),
            "feature property is removed when toggled off"
        );
        assert!(
            !editor.edit_styles().inner().contains_key(&var_key),
            "opsz variation is removed when toggled off"
        );
    }
}
