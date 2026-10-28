//! Text selection by touch: what a phone does where the desktop drags the
//! mouse. A long press selects a word (in selectable text or a field)
//! unless an app's right-click menu takes it; the selection gets two
//! handles and an edit menu (Cut / Copy / Paste / Select All, as the
//! target allows). Only touch input starts one; the mouse keeps the
//! desktop behavior.
//!
//! The range itself lives where the desktop keeps it (the static
//! selection, or the field's editor); this module keeps which element the
//! touch selection is on and what is showing, and resolves the geometry
//! once per frame ([`refresh`]) for hit tests, painting and
//! [`FrameState::touch_selection`].

use kurbo::{Point, Rect};

use crate::WidgetId;
use crate::frame::Frame;
use crate::frame_state::FrameState;
use crate::style::TextStyle;
use crate::text::{Fonts, resolve_text};
use crate::theme::Theme;

/// An edit menu action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditAction {
    /// Copy the selection to the clipboard and delete it.
    Cut,
    /// Copy the selection to the clipboard.
    Copy,
    /// Replace the selection with the clipboard's text.
    Paste,
    /// Select all of the target's text.
    SelectAll,
}

/// The touch selection as it stands in the last built frame.
#[derive(Clone, Debug, PartialEq)]
pub struct TouchSelection {
    /// The selectable text or field it is in.
    pub target: WidgetId,
    /// The selected byte range, `start <= end` (equal for a bare caret).
    pub range: (usize, usize),
    /// The start handle's touch area (logical px), `None` when there is
    /// none (a bare caret) or it is scrolled out of view.
    pub start_handle: Option<Rect>,
    /// The end handle's touch area (a bare caret's one handle).
    pub end_handle: Option<Rect>,
    /// The edit menu's items and where each is; empty when it is hidden.
    pub menu: Vec<(EditAction, Rect)>,
    /// The magnifier's frame while a handle or the caret is dragged.
    pub magnifier: Option<Rect>,
    /// Each menu item's label, in `menu`'s order.
    pub labels: Vec<String>,
    /// The selection's line boxes (logical px), for anchoring.
    pub(crate) lines: Vec<Rect>,
}

/// What is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(dead_code, reason = "constructed by handle and caret drags, the next step")]
pub(crate) enum Drag {
    Start,
    End,
    Caret,
}

/// The live touch selection (geometry aside).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TouchSel {
    pub target: WidgetId,
    /// In a field (cut and paste apply), rather than read-only text.
    pub input: bool,
    /// The edit menu is wanted (it hides while dragging regardless).
    pub menu: bool,
    /// The clipboard had text when the menu opened (asked once: reading
    /// it on iOS shows a banner, and asking is a platform call).
    pub paste: bool,
    pub drag: Option<Drag>,
    /// The finger while dragging, for the magnifier.
    pub finger: Option<Point>,
}

/// A handle's touch area: at least 44 logical px square, hanging below
/// the anchor (the end of the selected line) where the handle is drawn.
pub(crate) const HANDLE_HIT: f64 = 44.0;
/// Edit menu item height.
pub(crate) const MENU_H: f64 = 44.0;
/// Gap between the selection and the menu, and the menu's margin from the
/// canvas edges.
const MENU_GAP: f64 = 8.0;
/// Horizontal padding inside a menu item.
const ITEM_PAD: f64 = 12.0;
/// Menu label size.
pub(crate) const LABEL_PX: f32 = 15.0;

/// Starts (or restarts) a touch selection on `target`; the menu opens.
pub(crate) fn start(state: &mut FrameState, target: WidgetId, input: bool) {
    let paste = input && state.clipboard.has_text();
    state.touch_sel = Some(TouchSel {
        target,
        input,
        menu: true,
        paste,
        drag: None,
        finger: None,
    });
}

/// Ends the touch selection (the range itself is left to its owner).
pub(crate) fn clear(state: &mut FrameState) -> bool {
    state.touch_geom = None;
    state.touch_sel.take().is_some()
}

/// A handle anchor's touch area.
pub(crate) fn handle_rect(anchor: Point) -> Rect {
    Rect::new(
        anchor.x - HANDLE_HIT / 2.0,
        anchor.y - 4.0,
        anchor.x + HANDLE_HIT / 2.0,
        anchor.y - 4.0 + HANDLE_HIT,
    )
}

/// Resolves the touch selection's geometry against a freshly built frame,
/// dropping it when its target or range no longer exists.
pub(crate) fn refresh(frame: &Frame, state: &mut FrameState, fonts: &mut Fonts, theme: &Theme) {
    let Some(sel) = state.touch_sel else {
        state.touch_geom = None;
        return;
    };
    let Some((range, lines)) = selection_lines(frame, state, fonts, sel) else {
        if sel.input {
            clear(state);
        } else {
            clear(state);
            state.static_sel = None;
        }
        return;
    };
    let Some(first) = lines.first().copied() else {
        clear(state);
        return;
    };
    let last = *lines.last().expect("not empty");
    let clip = frame.clip_of(sel.target).flatten();
    let canvas = frame.canvas() - state.safe_area;
    let visible =
        |p: Point| clip.is_none_or(|c| c.inflate(0.5, 0.5).contains(p)) && canvas.contains(p);
    let collapsed = range.0 == range.1;
    let start_anchor = Point::new(first.x0, first.y1);
    let end_anchor = Point::new(if collapsed { first.x0 } else { last.x1 }, last.y1);
    let start_handle = (!collapsed && visible(start_anchor)).then(|| handle_rect(start_anchor));
    let end_handle = visible(end_anchor).then(|| handle_rect(end_anchor));

    let mut menu = Vec::new();
    let mut labels = Vec::new();
    let shown = start_handle.is_some() || end_handle.is_some();
    if sel.menu && sel.drag.is_none() && shown {
        let mut items = Vec::new();
        let masked = sel.input && state.editors.get(&sel.target).is_some_and(|e| e.masked);
        if sel.input && !collapsed && !masked {
            items.push(EditAction::Cut);
        }
        if !collapsed && !masked {
            items.push(EditAction::Copy);
        }
        if sel.input && sel.paste {
            items.push(EditAction::Paste);
        }
        items.push(EditAction::SelectAll);
        let zh = fonts.locale().is_some_and(|l| l.language() == "zh");
        let style = resolve_text(
            &TextStyle {
                size_px: Some(LABEL_PX),
                ..TextStyle::default()
            },
            theme,
        );
        let widths: Vec<f64> = items
            .iter()
            .map(|a| {
                let label = label(*a, zh);
                let (w, _) = fonts.measure(label, &style, None);
                labels.push(label.to_owned());
                f64::from(w) + 2.0 * ITEM_PAD
            })
            .collect();
        let total: f64 = widths.iter().sum();
        let top = lines.iter().map(|r| r.y0).fold(f64::INFINITY, f64::min);
        let bottom = lines.iter().map(|r| r.y1).fold(f64::NEG_INFINITY, f64::max);
        let left = lines.iter().map(|r| r.x0).fold(f64::INFINITY, f64::min);
        let right = lines.iter().map(|r| r.x1).fold(f64::NEG_INFINITY, f64::max);
        let mut y = top - MENU_GAP - MENU_H;
        if y < canvas.y0 + MENU_GAP {
            // No room above: below the selection, clear of its handles.
            y = bottom + HANDLE_HIT + MENU_GAP;
        }
        let min_x = canvas.x0 + MENU_GAP;
        let max_x = (canvas.x1 - MENU_GAP - total).max(min_x);
        let mut x = ((left + right) / 2.0 - total / 2.0).clamp(min_x, max_x);
        for (action, w) in items.into_iter().zip(widths) {
            menu.push((action, Rect::new(x, y, x + w, y + MENU_H)));
            x += w;
        }
    }

    let magnifier = sel
        .drag
        .zip(sel.finger)
        .map(|(_, f)| magnifier_rect(f, theme));
    state.touch_geom = Some(TouchSelection {
        target: sel.target,
        range,
        start_handle,
        end_handle,
        menu,
        magnifier,
        labels,
        lines,
    });
}

/// The magnifier's frame: centered over the finger, above it.
pub(crate) fn magnifier_rect(finger: Point, theme: &Theme) -> Rect {
    let (w, h) = match theme.selection_controls {
        crate::theme::SelectionControls::Material => (100.0, 48.0),
        crate::theme::SelectionControls::Cupertino => (110.0, 44.0),
    };
    let cy = finger.y - 40.0 - h / 2.0;
    Rect::new(
        finger.x - w / 2.0,
        cy - h / 2.0,
        finger.x + w / 2.0,
        cy + h / 2.0,
    )
}

/// A menu item's label in the UI language.
pub(crate) fn label(action: EditAction, zh: bool) -> &'static str {
    match (action, zh) {
        (EditAction::Cut, true) => "剪切",
        (EditAction::Copy, true) => "复制",
        (EditAction::Paste, true) => "粘贴",
        (EditAction::SelectAll, true) => "全选",
        (EditAction::Cut, false) => "Cut",
        (EditAction::Copy, false) => "Copy",
        (EditAction::Paste, false) => "Paste",
        (EditAction::SelectAll, false) => "Select All",
    }
}

/// The selected byte range and its line boxes in window coordinates (a
/// bare caret's one box), or `None` when the target or the range is gone.
fn selection_lines(
    frame: &Frame,
    state: &mut FrameState,
    fonts: &mut Fonts,
    sel: TouchSel,
) -> Option<((usize, usize), Vec<Rect>)> {
    let rect = frame.rect_of(sel.target)?;
    if sel.input {
        let data = frame.input_paint_of(sel.target)?;
        let editor = state.editors.get_mut(&sel.target)?;
        let origin = crate::input::text_origin(editor, fonts, data, rect);
        let range = editor.editor.raw_selection().text_range();
        let boxes: Vec<Rect> = if range.is_empty() {
            editor
                .editor
                .cursor_geometry(1.0)
                .into_iter()
                .map(|b| Rect::new(b.x0, b.y0, b.x0, b.y1))
                .collect()
        } else {
            editor
                .editor
                .selection_geometry()
                .into_iter()
                .map(|(b, _)| Rect::new(b.x0, b.y0, b.x1, b.y1))
                .collect()
        };
        let boxes = boxes.into_iter().map(|b| b + origin.to_vec2()).collect();
        return Some(((range.start, range.end), boxes));
    }
    let (sid, psel, _) = state.static_sel?;
    if sid != sel.target {
        return None;
    }
    let (text, style) = frame.static_text_of(sel.target)?;
    let full = text.to_text();
    let range = psel.text_range();
    if range.is_empty()
        || range.end > full.len()
        || !full.is_char_boundary(range.start)
        || !full.is_char_boundary(range.end)
    {
        return None;
    }
    #[expect(clippy::cast_possible_truncation, reason = "widths fit in f32")]
    let boxes = fonts.static_selection_rects(&text, style, Some(rect.width() as f32), psel);
    let origin = rect.origin().to_vec2();
    Some((
        (range.start, range.end),
        boxes.into_iter().map(|b| b + origin).collect(),
    ))
}
