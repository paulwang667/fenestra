//! Chips: compact labeled tokens — a toggleable choice/filter pill and a
//! dismissible input pill, on the M3 chip model but fenestra-styled (state
//! layer hover, press scale, accent tint when selected).
//!
//! ```
//! use fenestra_kit::chip;
//!
//! #[derive(Clone)]
//! enum Msg {
//!     Toggle(bool),
//!     Remove,
//! }
//!
//! let el: fenestra_core::Element<Msg> = fenestra_core::row().children([
//!     chip("Rust").selected(true).on_toggle(Msg::Toggle),
//!     chip("Draft").on_remove(Msg::Remove),
//! ]);
//! ```

use fenestra_core::{Cursor, Element, Semantics, Theme, Transition, Weight, col, row, text};

use super::ControlSize;
use crate::icons;

/// Height and label size per control size: Xs 24px, Sm 32px (the M3 chip
/// height), Md/Lg follow the shared grid.
fn metrics(size: ControlSize) -> (f32, fenestra_core::TextSize) {
    match size {
        ControlSize::Xs => (24.0, fenestra_core::TextSize::Xs),
        ControlSize::Sm | ControlSize::Md => (32.0, fenestra_core::TextSize::Sm),
        ControlSize::Lg => (40.0, fenestra_core::TextSize::Sm),
    }
}

/// A chip under construction; converts into an [`Element`].
pub struct Chip<Msg> {
    label: String,
    selected: bool,
    disabled: bool,
    icon: Option<Element<Msg>>,
    on_toggle: Option<Box<dyn Fn(bool) -> Msg>>,
    on_remove: Option<Msg>,
    size: ControlSize,
    key: Option<String>,
}

/// A compact labeled pill. Clicking toggles it (when
/// [`on_toggle`](Chip::on_toggle) is wired) with the accent tint as the
/// selected state; a trailing × appears when [`on_remove`](Chip::on_remove)
/// is set.
pub fn chip<Msg>(label: impl Into<String>) -> Chip<Msg> {
    Chip {
        label: label.into(),
        selected: false,
        disabled: false,
        icon: None,
        on_toggle: None,
        on_remove: None,
        size: ControlSize::Sm,
        key: None,
    }
}

impl<Msg> Chip<Msg> {
    /// Renders the chip in the selected (accent-tinted) state.
    #[must_use]
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Disables the chip (dimmed, inert).
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A leading icon (before the label; a selected chip draws a check
    /// instead, M3 filter-chip style).
    #[must_use]
    pub fn icon(mut self, icon: impl Into<Element<Msg>>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Maps a click to a message carrying the new selected state. Wiring
    /// this makes the chip a toggle (an M3 filter chip); without it the
    /// chip is a static token.
    #[must_use]
    pub fn on_toggle(mut self, f: impl Fn(bool) -> Msg + 'static) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }

    /// Shows a trailing × that emits this message (an M3 input chip).
    #[must_use]
    pub fn on_remove(mut self, msg: Msg) -> Self {
        self.on_remove = Some(msg);
        self
    }

    /// Sets the control size (Xs 24 / Sm 32 / Md 32 / Lg 40 px tall).
    #[must_use]
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// Stable identity key.
    #[must_use]
    pub fn id(mut self, key: &str) -> Self {
        self.key = Some(key.to_owned());
        self
    }
}

impl<Msg: Clone + 'static> From<Chip<Msg>> for Element<Msg> {
    fn from(c: Chip<Msg>) -> Self {
        let (h, font) = metrics(c.size);
        let selected = c.selected;
        let disabled = c.disabled;

        // Leading slot: the check wins when selected (M3 filter chip);
        // otherwise the caller's icon.
        let lead: Option<Element<Msg>> = if selected {
            Some(
                icons::check()
                    .w(14.0)
                    .h(14.0)
                    .themed(move |t: &Theme, s| s.color(t.accent_text)),
            )
        } else {
            c.icon
        };

        let mut kids: Vec<Element<Msg>> = Vec::with_capacity(3);
        kids.extend(lead);
        kids.push(
            text(c.label.clone())
                .size(font)
                .weight(Weight::Medium)
                .themed(move |t: &Theme, s| {
                    if selected {
                        s.color(t.accent_text)
                    } else if disabled {
                        s.color(t.text_disabled)
                    } else {
                        s.color(t.text)
                    }
                }),
        );

        let mut el = row()
            .items_center()
            .gap(6.0)
            .pl(10.0)
            .pr(if c.on_remove.is_some() { 4.0 } else { 10.0 })
            .h(h)
            .rounded_full()
            .shrink0()
            .transition(Transition::colors())
            .themed(move |t: &Theme, s| {
                if selected {
                    s.bg(t.accent_bg).border(1.0, t.accent_border)
                } else {
                    s.bg(t.element).border(1.0, t.border_subtle)
                }
            })
            .children(kids);

        let has_remove = c.on_remove.is_some();
        if let Some(remove) = c.on_remove {
            // The dismiss affordance is its own accessible button so the ×
            // never reads as part of the label.
            let x = icons::x().w(12.0).h(12.0).themed(move |t: &Theme, s| {
                if selected {
                    s.color(t.accent_text)
                } else {
                    s.color(t.text_muted)
                }
            });
            let remove_btn = col()
                .items_center()
                .justify_center()
                .w(22.0)
                .h(22.0)
                .rounded_full()
                .shrink0()
                .cursor(Cursor::Pointer)
                .transition(Transition::colors())
                .state_layer(|t: &Theme| t.text)
                .semantics(Semantics::Button)
                .label(format!("Remove {}", c.label))
                .on_click(remove)
                .child(x);
            el = el.child(remove_btn);
        }

        // Toggle chips are checkboxes to assistive tech (M3 maps filter
        // chips the same way); static tokens are plain labels.
        if let Some(f) = c.on_toggle {
            el = el
                .focusable(true)
                .cursor(Cursor::Pointer)
                .disabled(c.disabled)
                .semantics(Semantics::Checkbox {
                    checked: selected,
                    mixed: false,
                })
                .label(c.label.clone())
                .on_click(f(!selected))
                .state_layer(|t: &Theme| t.text)
                .press_scale();
        } else if has_remove {
            el = el.disabled(c.disabled);
        }
        if c.disabled {
            el = el.opacity(0.5);
        }
        if let Some(key) = &c.key {
            el = el.id(key);
        }
        el
    }
}
