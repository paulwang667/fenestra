//! Select: a Secondary-styled trigger with a chevron and a toggle-overlay
//! listbox. Keyboard: arrows step the value, Enter/Space toggles the menu,
//! and typing a letter jumps to the first matching option.
//!
//! ```
//! use fenestra_kit::select;
//!
//! #[derive(Clone)]
//! enum Msg {
//!     Pick(usize),
//! }
//!
//! let el: fenestra_core::Element<Msg> =
//!     select(0, ["Daily", "Weekly", "Monthly"]).on_change(Msg::Pick).into();
//! ```

use fenestra_core::{
    Cursor, Element, Key, Overlay, SP1, SP2, SP3, Semantics, Surface, TextSize, Theme, Transition,
    col, row, spacer, text,
};

use super::{ControlSize, Density};
use crate::icons;

/// A select under construction; converts into an [`Element`].
pub struct Select<Msg> {
    selected: usize,
    options: Vec<SelectOption>,
    size: ControlSize,
    density: Density,
    width: f32,
    fill: bool,
    disabled: bool,
    on_change: Option<std::rc::Rc<dyn Fn(usize) -> Msg>>,
    key: Option<String>,
}

/// A select over `options` showing the `selected` index.
pub fn select<Msg>(
    selected: usize,
    options: impl IntoIterator<Item = impl Into<SelectOption>>,
) -> Select<Msg> {
    Select {
        selected,
        options: options.into_iter().map(Into::into).collect(),
        size: ControlSize::default(),
        density: Density::default(),
        width: 200.0,
        fill: false,
        disabled: false,
        on_change: None,
        key: None,
    }
}

/// One entry in a [`select`]: a primary label and an optional secondary
/// line (metadata, a hint). The label is what the trigger shows when the
/// entry is selected; the detail is a smaller, muted line under it in the
/// open listbox.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectOption {
    /// The primary label.
    pub label: String,
    /// An optional secondary line under the label (a hint, metadata).
    pub detail: Option<String>,
}

impl From<String> for SelectOption {
    fn from(label: String) -> Self {
        Self {
            label,
            detail: None,
        }
    }
}

impl From<&str> for SelectOption {
    fn from(label: &str) -> Self {
        Self {
            label: label.to_owned(),
            detail: None,
        }
    }
}

impl<Msg> Select<Msg> {
    /// Sets the control size.
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the packing density ([`Density`]). `Comfortable` (default) is
    /// byte-identical to no call.
    pub fn density(mut self, density: Density) -> Self {
        self.density = density;
        self
    }

    /// Sets the trigger width in logical px (200 by default).
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Makes the trigger fill its row instead of taking a fixed width. The
    /// listbox follows the trigger's width, so a select in a label-left row
    /// spans the space the row gives it.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// Disables interaction.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Maps a newly selected option index to a message.
    pub fn on_change(mut self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(f));
        self
    }

    /// Stable identity key (recommended: the open state is kept per id).
    pub fn id(mut self, key: &str) -> Self {
        self.key = Some(key.to_owned());
        self
    }
}

/// Maximum listbox height before it scrolls.
const MAX_MENU_HEIGHT: f32 = 240.0;

impl<Msg: 'static> From<Select<Msg>> for Element<Msg> {
    fn from(sel: Select<Msg>) -> Self {
        let selected = sel.selected.min(sel.options.len().saturating_sub(1));
        let label = sel
            .options
            .get(selected)
            .map(|o| o.label.clone())
            .unwrap_or_default();
        // Density scales the trigger height on the shared grid; the label font
        // is held (density is spacing, not type), so `m.font == text_size()`.
        let m = sel.size.metrics_at(sel.density);

        // The trigger and the listbox share the width: a fixed one, or the
        // row's full width when `fill` is set.
        let width = sel.width;
        let fill = sel.fill;

        // The listbox: options on a raised surface, selected one tinted.
        // When `fill` it matches the trigger's width (laid out against the
        // anchor), so the dropdown aligns with the select instead of
        // sizing to the whole canvas.
        let menu = if fill {
            Overlay::menu().match_anchor_width()
        } else {
            Overlay::menu()
        };
        let mut listbox = col()
            .id("listbox")
            .overlay(menu)
            .scroll_y()
            .max_h(MAX_MENU_HEIGHT)
            .p(SP1)
            .gap(2.0)
            .surface(Surface::Menu)
            .children(sel.options.iter().enumerate().map(|(i, opt)| {
                let is_selected = i == selected;
                // The label and an optional muted detail line (metadata) sit in
                // a column; the checkmark (if selected) is pushed to the right
                // by the spacer.
                let mut label_col =
                    col()
                        .items_start()
                        .gap(1.0)
                        .children([text(opt.label.clone()).size(m.font).themed(
                            move |t: &Theme, s| {
                                if is_selected {
                                    s.color(t.accent_text)
                                } else {
                                    s.color(t.text)
                                }
                            },
                        )]);
                if let Some(detail) = &opt.detail {
                    label_col = label_col.children([text(detail.clone())
                        .size(TextSize::Xs)
                        .themed(|t: &Theme, s| s.color(t.text_muted))]);
                }
                let mut option = row()
                    .items_center()
                    .gap(SP2)
                    .px(SP2)
                    .py(5.0)
                    // Concentric with the listbox panel (Surface::Menu): the
                    // option radius is the panel outer radius minus its SP1
                    // padding, so options nest cleanly inside the panel.
                    .themed(|t: &Theme, s| s.rounded((t.radius.lg - SP1).max(0.0)))
                    .w_full()
                    .semantics(Semantics::ListItem {
                        selected: is_selected,
                    })
                    .cursor(Cursor::Pointer)
                    .children([label_col])
                    .children([spacer()])
                    .transition(Transition::colors())
                    .state_layer(|t| t.text);
                if is_selected {
                    // A checkmark, not just a tint, marks the option in use -
                    // the tint alone is easy to miss in a long list.
                    option = option.children([icons::check()
                        .themed(|t: &Theme, s| s.color(t.accent_text))
                        .shrink0()]);
                    option = option.themed(|t: &Theme, s| s.bg(t.accent_bg));
                }
                if let Some(f) = &sel.on_change {
                    let f = f.clone();
                    option = option.on_click(f(i));
                }
                option
            }));

        listbox = if fill {
            listbox.w_full()
        } else {
            listbox.w(width)
        };

        let chevron = icons::chevron_down().themed(|t: &Theme, s| s.color(t.text_muted));

        let mut trigger = row()
            .items_center()
            .gap(SP2)
            .h(m.height)
            .px(SP3)
            .themed(|t: &Theme, s| s.rounded(t.radius.md))
            .focusable(true)
            .cursor(Cursor::Pointer)
            .disabled(sel.disabled)
            .transition(Transition::colors())
            .themed(|t: &Theme, s| s.bg(t.surface_raised).border(1.0, t.border))
            .semantics(Semantics::ComboBox)
            .label(label.clone())
            .children([text(label).size(m.font)])
            .children([spacer(), chevron])
            .children([listbox]);

        trigger = if fill {
            trigger.w_full()
        } else {
            trigger.w(width).shrink0()
        };

        if let Some(f) = sel.on_change {
            let options: Vec<String> = sel.options.iter().map(|o| o.label.clone()).collect();
            let count = options.len();
            let nav = std::rc::Rc::new(f);
            let pick = std::rc::Rc::clone(&nav);
            trigger = trigger.on_key(move |k| match k.key {
                Key::ArrowDown => (selected + 1 < count).then(|| nav(selected + 1)),
                Key::ArrowUp => (selected > 0).then(|| nav(selected - 1)),
                Key::Home => (count > 0).then(|| nav(0)),
                Key::End => count.checked_sub(1).map(|i| nav(i)),
                _ => None,
            });
            // Type-ahead, both idioms: a single letter cycles through
            // entries with that initial (excluding the current one, so
            // repeats advance), while a growing buffer prefix-matches
            // from the current selection inclusive — "ce" finds Cedar
            // without bouncing off Cherry.
            trigger = trigger.on_type_ahead(move |buffer| {
                let needle = buffer.to_lowercase();
                let start = if needle.chars().count() == 1 { 1 } else { 0 };
                (start..start + count)
                    .map(|step| (selected + step) % count)
                    .find(|i| options[*i].to_lowercase().starts_with(&needle))
                    .map(|i| pick(i))
            });
        }
        if sel.disabled {
            // Disabled keeps the simple subtree dim; the state layer (which
            // would otherwise also fade the container) is for the live trigger.
            trigger = trigger.opacity(0.5);
        } else {
            // A button-like trigger: the tactile press-shrink reads like the
            // kit's buttons (state layer for hover, scale for the press).
            trigger = trigger.state_layer(|t| t.text).press_scale();
        }
        if let Some(key) = &sel.key {
            trigger = trigger.id(key);
        }
        trigger
    }
}
