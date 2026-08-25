//! Time picker: 24-hour `HH:MM` (optionally `:SS`) as three focusable
//! segments on the macOS field model — ↑/↓ step the focused segment with
//! wraparound, Tab moves between segments, each segment projects as an
//! ARIA `spinbutton`.
//!
//! Elm-pure: the app owns `(h, m, s)` and echoes the picker's `on_change`
//! back. Typing digits needs a per-segment buffer the kit can't own
//! statelessly, so v1 is arrows-only (documented).
//!
//! ```
//! use fenestra_kit::time_picker;
//!
//! #[derive(Clone)]
//! enum Msg {
//!     At(u32, u32, u32),
//! }
//!
//! let el: fenestra_core::Element<Msg> =
//!     time_picker(9, 30, 0).on_change(Msg::At).into();
//! ```

use fenestra_core::{
    Cursor, Element, Key, SP2, Semantics, TextSize, Theme, Transition, Weight, row, text,
};

use super::ControlSize;

/// The shared edit callback: all three fields, whichever segment moved.
type OnChange<Msg> = std::rc::Rc<dyn Fn(u32, u32, u32) -> Msg>;

/// A time picker under construction; converts into an [`Element`].
pub struct TimePicker<Msg> {
    hour: u32,
    minute: u32,
    second: u32,
    seconds: bool,
    disabled: bool,
    size: ControlSize,
    on_change: Option<OnChange<Msg>>,
    key: Option<String>,
}

/// A 24-hour time picker showing `(hour, minute)` — pass the current second
/// too when [`with_seconds`](TimePicker::with_seconds) is on.
pub fn time_picker<Msg>(hour: u32, minute: u32, second: u32) -> TimePicker<Msg> {
    TimePicker {
        hour: hour.min(23),
        minute: minute.min(59),
        second: second.min(59),
        seconds: false,
        disabled: false,
        size: ControlSize::Md,
        on_change: None,
        key: None,
    }
}

impl<Msg> TimePicker<Msg> {
    /// Shows (and edits) the seconds segment as well.
    #[must_use]
    pub fn with_seconds(mut self, seconds: bool) -> Self {
        self.seconds = seconds;
        self
    }

    /// Disables the picker (dimmed, inert).
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the control height via [`ControlSize`] (Sm 32 / Md 36 / Lg 40).
    #[must_use]
    pub fn size(mut self, size: ControlSize) -> Self {
        self.size = size;
        self
    }

    /// Maps an edit to `(hour, minute, second)` — all three, whichever
    /// segment moved.
    #[must_use]
    pub fn on_change(mut self, f: impl Fn(u32, u32, u32) -> Msg + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(f));
        self
    }

    /// Stable identity key (recommended: segment focus is kept per id).
    #[must_use]
    pub fn id(mut self, key: &str) -> Self {
        self.key = Some(key.to_owned());
        self
    }
}

/// One focusable numeric segment: an ARIA spinbutton that steps with
/// wraparound on ↑/↓. `step_msg` maps a delta (already in wraparound
/// space: `1` = up, `max + 1` = down) to the message.
fn segment<Msg: Clone + 'static>(
    value: u32,
    max: u32,
    name: &str,
    id: String,
    disabled: bool,
    font: TextSize,
    step_msg: std::rc::Rc<dyn Fn(u32) -> Msg>,
) -> Element<Msg> {
    let name = name.to_string();
    let up = std::rc::Rc::clone(&step_msg);
    let down = step_msg;
    row()
        .items_center()
        .justify_center()
        .w(30.0)
        .h_full()
        .px(2.0)
        .themed(|th: &Theme, st| st.rounded(th.radius.sm))
        .shrink0()
        .focusable(true)
        .cursor(Cursor::Default)
        .disabled(disabled)
        .transition(Transition::colors())
        .hover_themed(|th: &Theme, st| st.bg(th.element))
        .focus_themed(|th: &Theme, st| st.bg(th.accent_bg).border(1.0, th.accent_border))
        .semantics(Semantics::Spinbutton {
            value: value as f32,
            min: 0.0,
            max: max as f32,
        })
        .value(format!("{value:02}"))
        .label(name)
        .id(&id)
        .on_key(move |k| match k.key {
            Key::ArrowUp => Some(up(1)),
            Key::ArrowDown => Some(down(max)),
            _ => None,
        })
        .child(
            text(format!("{value:02}"))
                .size(font)
                .weight(Weight::Medium)
                .tabular()
                .themed(move |th: &Theme, st| {
                    st.color(if disabled { th.text_disabled } else { th.text })
                }),
        )
}

impl<Msg: Clone + 'static> From<TimePicker<Msg>> for Element<Msg> {
    fn from(t: TimePicker<Msg>) -> Self {
        let m = t.size.metrics();
        let key = t.key.clone().unwrap_or_else(|| "tp".to_owned());
        let font = TextSize::Sm;

        let Some(f) = t.on_change else {
            // Without a consumer the picker is a read-only readout.
            return readout(t.hour, t.minute, t.second, t.seconds, font);
        };

        let disabled = t.disabled;
        let (h, mi, s) = (t.hour, t.minute, t.second);

        let fh = f.clone();
        let fm = f.clone();
        let seg_h = segment(
            h,
            23,
            "Hour",
            format!("{key}-h"),
            disabled,
            font,
            std::rc::Rc::new(move |d| fh((h + d) % 24, mi, s)),
        );
        let seg_m = segment(
            mi,
            59,
            "Minute",
            format!("{key}-m"),
            disabled,
            font,
            std::rc::Rc::new(move |d| fm(h, (mi + d) % 60, s)),
        );
        let seg_s = t.seconds.then(|| {
            let fs = f.clone();
            segment(
                s,
                59,
                "Second",
                format!("{key}-s"),
                disabled,
                font,
                std::rc::Rc::new(move |d| fs(h, mi, (s + d) % 60)),
            )
        });

        let colon = |disabled| {
            text(":").size(font).themed(move |th: &Theme, st| {
                st.color(if disabled {
                    th.text_disabled
                } else {
                    th.text_muted
                })
            })
        };

        let mut kids: Vec<Element<Msg>> = vec![seg_h, colon(disabled), seg_m];
        if let Some(seg) = seg_s {
            kids.push(colon(disabled));
            kids.push(seg);
        }

        row()
            .items_center()
            .gap(1.0)
            .px(SP2)
            .h(m.height)
            .themed(|th: &Theme, st| {
                st.rounded(th.radius.md)
                    .bg(th.surface_raised)
                    .border(1.0, th.border)
            })
            .shrink0()
            .transition(Transition::colors())
            .children(kids)
            .opacity(if disabled { 0.5 } else { 1.0 })
    }
}

/// A passive `HH:MM[:SS]` readout for when no `on_change` is wired.
fn readout<Msg>(h: u32, mi: u32, s: u32, seconds: bool, font: TextSize) -> Element<Msg> {
    let body = if seconds {
        format!("{h:02}:{mi:02}:{s:02}")
    } else {
        format!("{h:02}:{mi:02}")
    };
    row()
        .items_center()
        .px(SP2)
        .h(36.0)
        .themed(|th: &Theme, st| {
            st.rounded(th.radius.md)
                .bg(th.element)
                .border(1.0, th.border_subtle)
        })
        .shrink0()
        .child(text(body).size(font).weight(Weight::Medium).tabular())
}
