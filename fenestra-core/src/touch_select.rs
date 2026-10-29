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

use kurbo::{Affine, BezPath, Circle, Point, Rect, RoundedRect, Stroke, Vec2};
use peniko::Fill;

use crate::draw::DrawTarget;
use crate::frame::Frame;
use crate::frame_state::FrameState;
use crate::style::{Shadow, TextStyle};
use crate::text::{Fonts, ResolvedText, resolve_text};
use crate::theme::{SelectionControls, Theme};
use crate::tokens::ShadowToken;
use crate::{Color, WidgetId};

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
    /// What the magnifier centers on: the dragged point.
    pub(crate) focus: Option<Point>,
    pub(crate) look: Look,
}

/// What is being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// The end that stays put while a handle drags the other.
    pub fixed: usize,
    /// From the finger to the selection end it drags (the handle hangs
    /// below the text, the finger on the handle).
    pub offset: Vec2,
    /// The finger that long-pressed is still down: moving it drags.
    pub held: bool,
    /// The word a long press selected in text: a drag after it keeps the
    /// word inside the range, whichever way it goes.
    pub word: Option<(usize, usize)>,
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
        fixed: 0,
        offset: Vec2::ZERO,
        held: false,
        word: None,
    });
}
/// Honors a `select_all_on` token that came new this frame: everything in
/// that text or field is selected, with handles and the menu.
fn select_requested(frame: &Frame, state: &mut FrameState, fonts: &mut Fonts) {
    let Some(id) = state.pending_select.take() else {
        return;
    };
    // Under the mouse it only selects (Cmd/Ctrl+C copies it): the touch
    // handles and menu are a finger's.
    let touch = state.pointer_source == crate::events::PointerSource::Touch;
    let Some(rect) = frame.rect_of(id) else {
        return;
    };
    if let Some((text, style)) = frame.static_text_of(id) {
        let len = text.to_text().len();
        if len == 0 {
            return;
        }
        #[expect(clippy::cast_possible_truncation, reason = "widths fit in f32")]
        let all = fonts.static_range(&text, style, Some(rect.width() as f32), 0, len);
        state.static_sel = Some((id, all, false));
        // As a press on static text does: typing into a field ends, so the
        // field does not take Cmd/Ctrl+C with its own (empty) selection.
        if state.focus.is_some_and(|f| state.editors.contains_key(&f)) {
            state.focus = None;
            state.focus_visible = false;
        }
        if touch {
            start(state, id, false);
        }
    } else if let Some(editor) = state.editors.get_mut(&id) {
        let len = editor.editor.raw_text().len();
        crate::input::select_range(editor, fonts, 0, len);
        state.focus = Some(id);
        if touch {
            start(state, id, true);
        }
    }
}

/// The edit menu item under a touch press, if any.
pub(crate) fn menu_at(state: &FrameState, point: Point) -> Option<EditAction> {
    state
        .touch_geom
        .as_ref()?
        .menu
        .iter()
        .find(|(_, r)| r.contains(point))
        .map(|(a, _)| *a)
}

/// The finger that started this selection by a long press is still down.
pub(crate) fn hold(state: &mut FrameState) {
    if let Some(sel) = &mut state.touch_sel {
        sel.held = true;
    }
}

/// A touch press: on a handle it starts dragging that end (and takes the
/// press). Whether it took it.
pub(crate) fn press(state: &mut FrameState, point: Point) -> bool {
    let (Some(sel), Some(geom)) = (&mut state.touch_sel, &state.touch_geom) else {
        return false;
    };
    let (start, end) = geom.range;
    let hit = |h: Option<Rect>| h.is_some_and(|h| h.contains(point));
    let (drag, fixed, anchor) = if hit(geom.end_handle) {
        let last = geom.lines.last().copied().unwrap_or_default();
        let x = if start == end { last.x0 } else { last.x1 };
        (Drag::End, start, Point::new(x, (last.y0 + last.y1) / 2.0))
    } else if hit(geom.start_handle) {
        let first = geom.lines.first().copied().unwrap_or_default();
        (
            Drag::Start,
            end,
            Point::new(first.x0, (first.y0 + first.y1) / 2.0),
        )
    } else {
        return false;
    };
    sel.drag = Some(if start == end { Drag::Caret } else { drag });
    sel.fixed = fixed;
    sel.offset = anchor - point;
    sel.finger = Some(point);
    true
}

/// A touch move: drags the end being dragged (or, after a long press, the
/// caret in a field or the selection's end in text) to the finger.
/// Whether it was a drag.
pub(crate) fn drag_to(
    frame: &Frame,
    state: &mut FrameState,
    fonts: &mut Fonts,
    finger: Point,
) -> bool {
    // The range as it is now (the frame's geometry may not have caught up
    // with the long press that made it).
    let static_range = state
        .static_sel
        .map(|(_, s, _)| s.text_range())
        .map(|r| (r.start, r.end));
    let Some(sel) = state.touch_sel.as_mut() else {
        return false;
    };
    if sel.drag.is_none() {
        if !sel.held {
            return false;
        }
        // Moving the finger that long-pressed: a field's caret follows it;
        // text extends the selection's end.
        sel.drag = Some(if sel.input { Drag::Caret } else { Drag::End });
        sel.word = if sel.input { None } else { static_range };
        sel.offset = Vec2::ZERO;
    }
    sel.finger = Some(finger);
    let sel = *sel;
    let point = finger + sel.offset;
    let Some(rect) = frame.rect_of(sel.target) else {
        return true;
    };
    if sel.input {
        let now = state.now();
        let (Some(data), Some(editor)) = (
            frame.input_paint_of(sel.target),
            state.editors.get_mut(&sel.target),
        ) else {
            return true;
        };
        let origin = crate::input::text_origin(editor, fonts, data, rect);
        let local = point - origin.to_vec2();
        let at = crate::input::byte_at(editor, fonts, local.x, local.y);
        let at = grapheme_floor(editor.editor.raw_text(), at);
        let anchor = if sel.drag == Some(Drag::Caret) {
            at
        } else {
            sel.fixed
        };
        crate::input::select_range(editor, fonts, anchor, at);
        editor.last_activity = now;
    } else if let Some((text, style)) = frame.static_text_of(sel.target)
        && let Some(local) = frame.to_layout_point(sel.target, point)
    {
        #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
        let width = Some(rect.width() as f32);
        #[expect(clippy::cast_possible_truncation, reason = "text coords fit in f32")]
        let at = fonts.static_index_at(
            &text,
            style,
            width,
            (local.x - rect.x0) as f32,
            (local.y - rect.y0) as f32,
        );
        let at = grapheme_floor(&text.to_text(), at);
        // After a long press the pressed word stays in the range: the
        // finger moves whichever end is on its side of it.
        let (anchor, focus) = match sel.word {
            Some((a, b)) if at < a => (b, at),
            Some((a, b)) if at <= b => (a, b),
            Some((a, _)) => (a, at),
            None => (sel.fixed, at),
        };
        // Read-only text keeps at least one character selected: dragged
        // onto the other end, the selection stays as it was.
        if anchor != focus {
            let range = fonts.static_range(&text, style, width, anchor, focus);
            state.static_sel = Some((sel.target, range, false));
        }
    }
    true
}

/// The finger lifted: a drag ends and the menu comes back. Whether one
/// was dragging.
pub(crate) fn release(state: &mut FrameState) -> bool {
    let Some(sel) = &mut state.touch_sel else {
        return false;
    };
    sel.held = false;
    sel.finger = None;
    sel.drag.take().is_some()
}

/// The grapheme boundary at or before `index`: a selection end never
/// splits a user-perceived character (an emoji with its skin tone, a
/// letter with its accents), whatever clusters the font shaped.
fn grapheme_floor(text: &str, index: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    if index >= text.len() {
        return text.len();
    }
    text.grapheme_indices(true)
        .map(|(i, _)| i)
        .take_while(|&i| i <= index)
        .last()
        .unwrap_or(0)
}

/// The edge band a dragged end autoscrolls in (logical px).
const EDGE: f64 = 24.0;

/// While an end is dragged into the edge band of the scroller it is in,
/// scrolls that scroller a step (more the deeper) and drags the end along.
/// Whether it scrolled (the frame keeps animating while it does).
fn autoscroll(frame: &Frame, state: &mut FrameState, fonts: &mut Fonts) -> bool {
    let Some(sel) = state.touch_sel else {
        return false;
    };
    let (Some(_), Some(finger), Some(rect)) = (sel.drag, sel.finger, frame.rect_of(sel.target))
    else {
        return false;
    };
    let Some(scroller) = frame
        .scrollable_y_at(finger)
        .or_else(|| frame.scrollable_y_at(rect.center()))
    else {
        return false;
    };
    let Some(bounds) = frame.rect_of(scroller) else {
        return false;
    };
    let view = frame
        .clip_of(scroller)
        .flatten()
        .map_or(bounds, |c| c.intersect(bounds));
    let step = if finger.y > view.y1 - EDGE {
        (EDGE - (view.y1 - finger.y).max(0.0)).min(EDGE) * 0.5
    } else if finger.y < view.y0 + EDGE {
        -(EDGE - (finger.y - view.y0).max(0.0)).min(EDGE) * 0.5
    } else {
        return false;
    };
    let offset = state.scroll_offset(scroller);
    #[expect(clippy::cast_possible_truncation, reason = "offsets fit in f32")]
    let next = (offset + step as f32).max(0.0);
    if (next - offset).abs() < f32::EPSILON {
        return false;
    }
    state.scroll_to(scroller, next);
    drag_to(frame, state, fonts, finger);
    true
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
/// Returns whether the frame should keep animating (an autoscroll).
pub(crate) fn refresh(
    frame: &Frame,
    state: &mut FrameState,
    fonts: &mut Fonts,
    theme: &Theme,
) -> bool {
    select_requested(frame, state, fonts);
    let scrolling = autoscroll(frame, state, fonts);
    let Some(sel) = state.touch_sel else {
        state.touch_geom = None;
        return false;
    };
    let Some((range, lines)) = selection_lines(frame, state, fonts, sel) else {
        clear(state);
        if !sel.input {
            state.static_sel = None;
        }
        return false;
    };
    let Some(first) = lines.first().copied() else {
        clear(state);
        return false;
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
        let style = Look::of(theme).label;
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
        // Placed against what is on screen of the selection: a tall one
        // (Select All on a long message) runs past the view.
        let view = clip.map_or(canvas, |c| c.intersect(canvas));
        let shown: Vec<Rect> = lines
            .iter()
            .map(|r| r.intersect(view))
            .filter(|r| r.height() > 0.0)
            .collect();
        let shown = if shown.is_empty() {
            lines.clone()
        } else {
            shown
        };
        let top = shown.iter().map(|r| r.y0).fold(f64::INFINITY, f64::min);
        let bottom = shown.iter().map(|r| r.y1).fold(f64::NEG_INFINITY, f64::max);
        let left = shown.iter().map(|r| r.x0).fold(f64::INFINITY, f64::min);
        let right = shown.iter().map(|r| r.x1).fold(f64::NEG_INFINITY, f64::max);
        let mut y = top - MENU_GAP - MENU_H;
        if y < canvas.y0 + MENU_GAP {
            // No room above: below the selection, clear of its handles.
            y = bottom + HANDLE_HIT + MENU_GAP;
        }
        if y + MENU_H > canvas.y1 - MENU_GAP {
            // Nor below: over the middle of what shows of it.
            y = ((top + bottom) / 2.0 - MENU_H / 2.0).clamp(
                canvas.y0 + MENU_GAP,
                (canvas.y1 - MENU_GAP - MENU_H).max(canvas.y0),
            );
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
    let focus = sel.drag.zip(sel.finger).map(|(_, f)| f + sel.offset);
    state.touch_geom = Some(TouchSelection {
        target: sel.target,
        range,
        start_handle,
        end_handle,
        menu,
        magnifier,
        labels,
        lines,
        focus,
        look: Look::of(theme),
    });
    scrolling
}

/// The magnifier's frame: centered over the finger, above it.
pub(crate) fn magnifier_rect(finger: Point, theme: &Theme) -> Rect {
    let (w, h) = match theme.selection_controls {
        SelectionControls::Material => (100.0, 48.0),
        SelectionControls::Cupertino => (110.0, 44.0),
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

/// What the controls look like, resolved from the theme when the frame is
/// built (painting has no theme).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Look {
    pub controls: SelectionControls,
    pub accent: Color,
    pub surface: Color,
    pub bg: Color,
    pub text: Color,
    pub shadow: Vec<Shadow>,
    pub label: ResolvedText,
}

impl Look {
    fn of(theme: &Theme) -> Self {
        Self {
            controls: theme.selection_controls,
            accent: theme.accent,
            surface: theme.surface_raised,
            bg: theme.bg,
            text: theme.text,
            shadow: theme.shadow(ShadowToken::Md),
            label: resolve_text(
                &TextStyle {
                    size_px: Some(LABEL_PX),
                    ..TextStyle::default()
                },
                theme,
            ),
        }
    }
}

/// The magnifier's zoom.
pub(crate) const ZOOM: f64 = 1.25;

/// Draws the magnifier's frame and clips to it; the caller paints the
/// content (zoomed by [`magnifier_transform`]) and then calls
/// [`end_magnifier`].
pub(crate) fn begin_magnifier<T: DrawTarget>(target: &mut T, geom: &TouchSelection, rect: Rect) {
    let shape = magnifier_shape(geom, rect);
    for s in shadows(&geom.look, ShadowToken::Sm) {
        draw_shadow(target, &s, rect, radius(&shape));
    }
    target.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &shape);
    target.fill(Fill::NonZero, Affine::IDENTITY, geom.look.bg, None, &rect);
}

/// Maps content around the dragged point into the magnifier, zoomed.
pub(crate) fn magnifier_transform(rect: Rect, focus: Point) -> Affine {
    Affine::translate(rect.center().to_vec2())
        * Affine::scale(ZOOM)
        * Affine::translate(-focus.to_vec2())
}

/// Ends [`begin_magnifier`] and outlines the frame.
pub(crate) fn end_magnifier<T: DrawTarget>(target: &mut T, geom: &TouchSelection, rect: Rect) {
    target.pop_layer();
    let shape = magnifier_shape(geom, rect);
    target.stroke(
        &Stroke::new(0.5),
        Affine::IDENTITY,
        geom.look.text.with_alpha(0.2),
        None,
        &shape,
    );
}

fn magnifier_shape(geom: &TouchSelection, rect: Rect) -> RoundedRect {
    let r = match geom.look.controls {
        SelectionControls::Material => 8.0,
        SelectionControls::Cupertino => rect.height() / 2.0,
    };
    RoundedRect::from_rect(rect, r)
}

fn radius(shape: &RoundedRect) -> f64 {
    shape.radii().top_left
}

/// The theme's shadow for `token` (the menu's is resolved at build time).
fn shadows(look: &Look, token: ShadowToken) -> Vec<Shadow> {
    match token {
        ShadowToken::Md => look.shadow.clone(),
        // A lighter one for the magnifier: the menu's, halved.
        _ => look
            .shadow
            .iter()
            .map(|s| Shadow {
                blur: s.blur / 2.0,
                dy: s.dy / 2.0,
                ..*s
            })
            .collect(),
    }
}

fn draw_shadow<T: DrawTarget>(target: &mut T, s: &Shadow, rect: Rect, radius: f64) {
    let r = rect.inflate(f64::from(s.spread), f64::from(s.spread))
        + kurbo::Vec2::new(f64::from(s.dx), f64::from(s.dy));
    target.draw_blurred_rounded_rect(
        Affine::IDENTITY,
        r,
        s.color,
        radius,
        f64::from(s.blur) / 2.0,
    );
}

/// Paints the handles and the edit menu (above everything else).
pub(crate) fn paint<T: DrawTarget>(target: &mut T, fonts: &mut Fonts, geom: &TouchSelection) {
    let look = &geom.look;
    let collapsed = geom.range.0 == geom.range.1;
    let first = geom.lines.first().copied().unwrap_or_default();
    let last = geom.lines.last().copied().unwrap_or_default();
    match look.controls {
        SelectionControls::Material => {
            if let Some(h) = geom.start_handle {
                teardrop(target, look.accent, anchor_of(h), Lean::Left);
            }
            if let Some(h) = geom.end_handle {
                let lean = if collapsed { Lean::Center } else { Lean::Right };
                teardrop(target, look.accent, anchor_of(h), lean);
            }
        }
        SelectionControls::Cupertino => {
            if geom.start_handle.is_some() {
                lollipop(
                    target,
                    look.accent,
                    Point::new(first.x0, first.y0),
                    first.y1,
                    true,
                );
            }
            // iOS shows no handle on a bare caret.
            if geom.end_handle.is_some() && !collapsed {
                let x = last.x1;
                lollipop(target, look.accent, Point::new(x, last.y0), last.y1, false);
            }
        }
    }
    paint_menu(target, fonts, geom);
}

/// Where a handle's touch area hangs from (see [`handle_rect`]).
fn anchor_of(hit: Rect) -> Point {
    Point::new(hit.center().x, hit.y0 + 4.0)
}

#[derive(Clone, Copy)]
enum Lean {
    Left,
    Center,
    Right,
}

/// Material's handle: a disc with one square corner pointing at the
/// anchor (the start's leans left, the end's right; a bare caret's
/// points straight up).
fn teardrop<T: DrawTarget>(target: &mut T, color: Color, p: Point, lean: Lean) {
    const R: f64 = 11.0;
    let (center, corner) = match lean {
        Lean::Left => (
            Point::new(p.x - R, p.y + R),
            Rect::new(p.x - R, p.y, p.x, p.y + R),
        ),
        Lean::Right => (
            Point::new(p.x + R, p.y + R),
            Rect::new(p.x, p.y, p.x + R, p.y + R),
        ),
        Lean::Center => {
            let c = Point::new(p.x, p.y + R * std::f64::consts::SQRT_2);
            let mut diamond = BezPath::new();
            diamond.move_to(p);
            let d = R * std::f64::consts::FRAC_1_SQRT_2;
            diamond.line_to((c.x + d, c.y - d));
            diamond.line_to((c.x - d, c.y - d));
            diamond.close_path();
            target.fill(Fill::NonZero, Affine::IDENTITY, color, None, &diamond);
            target.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                color,
                None,
                &Circle::new(c, R),
            );
            return;
        }
    };
    target.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color,
        None,
        &Circle::new(center, R),
    );
    target.fill(Fill::NonZero, Affine::IDENTITY, color, None, &corner);
}

/// Cupertino's handle: a 2px bar down the line with a knob above (start)
/// or below (end).
fn lollipop<T: DrawTarget>(target: &mut T, color: Color, top: Point, bottom: f64, start: bool) {
    let bar = Rect::new(top.x - 1.0, top.y, top.x + 1.0, bottom);
    target.fill(Fill::NonZero, Affine::IDENTITY, color, None, &bar);
    let knob = if start {
        Point::new(top.x, top.y - 4.0)
    } else {
        Point::new(top.x, bottom + 4.0)
    };
    target.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        color,
        None,
        &Circle::new(knob, 4.0),
    );
}

fn paint_menu<T: DrawTarget>(target: &mut T, fonts: &mut Fonts, geom: &TouchSelection) {
    let (Some(first), Some(last)) = (geom.menu.first(), geom.menu.last()) else {
        return;
    };
    let look = &geom.look;
    let bar = Rect::new(first.1.x0, first.1.y0, last.1.x1, last.1.y1);
    let (fill, ink) = match look.controls {
        SelectionControls::Material => (look.surface, look.text),
        SelectionControls::Cupertino => (
            Color::from_rgb8(0x1c, 0x1c, 0x1e).with_alpha(0.92),
            Color::WHITE,
        ),
    };
    let shape = RoundedRect::from_rect(bar, 8.0);
    for s in shadows(look, ShadowToken::Md) {
        draw_shadow(target, &s, bar, 8.0);
    }
    target.fill(Fill::NonZero, Affine::IDENTITY, fill, None, &shape);
    let mut label = look.label;
    label.color = ink;
    let line = f64::from(label.px * label.line_height);
    for (i, ((_, item), text)) in geom.menu.iter().zip(&geom.labels).enumerate() {
        if i > 0 && look.controls == SelectionControls::Cupertino {
            let sep = Rect::new(
                item.x0 - 0.25,
                item.y0 + 10.0,
                item.x0 + 0.25,
                item.y1 - 10.0,
            );
            target.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                ink.with_alpha(0.25),
                None,
                &sep,
            );
        }
        let y = item.y0 + (item.height() - line) / 2.0;
        let rect = Rect::new(item.x0 + ITEM_PAD, y, item.x1 - ITEM_PAD + 1.0, y + line);
        fonts.paint(target, text, &label, rect, None);
    }
}
