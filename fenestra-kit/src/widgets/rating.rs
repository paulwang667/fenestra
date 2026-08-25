//! Rating: a star scale (App Store / review model) — filled stars in the
//! warning amber, empty stars as muted outlines, optional half-star
//! precision via a clipped overlay. One tab stop; ←/↓ and →/↑ step by one
//! `precision`, Home/End clear and max.
//!
//! Elm-pure: the app owns `value` and echoes `on_change` back.
//!
//! ```
//! use fenestra_kit::rating;
//!
//! #[derive(Clone)]
//! enum Msg {
//!     Rate(f32),
//! }
//!
//! let el: fenestra_core::Element<Msg> = rating(3.0, 5).on_change(Msg::Rate).into();
//! ```

use fenestra_core::{
    Cursor, Element, Key, SP1, Semantics, Theme, Transition, div, path, row, stack,
};
use kurbo::BezPath;

use crate::icons::lucide;

/// The rating under construction; converts into an [`Element`].
pub struct Rating<Msg> {
    value: f32,
    max: u32,
    precision: f32,
    disabled: bool,
    read_only: bool,
    star_px: f32,
    on_change: Option<std::rc::Rc<dyn Fn(f32) -> Msg>>,
    key: Option<String>,
}

/// A star rating showing `value` of `max` stars (fractional values render
/// as partially filled stars).
pub fn rating<Msg>(value: f32, max: u32) -> Rating<Msg> {
    Rating {
        value: value.clamp(0.0, f32::from(u16::try_from(max).unwrap_or(u16::MAX))),
        max,
        precision: 1.0,
        disabled: false,
        read_only: false,
        star_px: 20.0,
        on_change: None,
        key: None,
    }
}

fn max_f(max: u32) -> f32 {
    f32::from(u16::try_from(max).unwrap_or(u16::MAX))
}

impl<Msg> Rating<Msg> {
    /// Sets the step (1 = whole stars, 0.5 = halves). Clamped into
    /// `0.1..=max`.
    #[must_use]
    pub fn precision(mut self, precision: f32) -> Self {
        self.precision = precision.clamp(0.1, max_f(self.max.max(1)));
        self
    }

    /// Disables the rating (dimmed, inert).
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// A display-only rating: no pointer or keyboard editing.
    #[must_use]
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Sets the star size in logical px (20 by default, floor 12).
    #[must_use]
    pub fn size(mut self, star_px: f32) -> Self {
        self.star_px = star_px.max(12.0);
        self
    }

    /// Maps a new value (already snapped to the precision and clamped) to a
    /// message.
    #[must_use]
    pub fn on_change(mut self, f: impl Fn(f32) -> Msg + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(f));
        self
    }

    /// Stable identity key.
    #[must_use]
    pub fn id(mut self, key: &str) -> Self {
        self.key = Some(key.to_owned());
        self
    }
}

/// One star at `fill` (0..=1): an outline, a filled star, or a clipped
/// filled copy over the outline for fractional fills.
fn star<Msg>(fill: f32, px: f32, msg: Option<Msg>) -> Element<Msg> {
    let bez =
        BezPath::from_svg(lucide::raw_path("star").expect("star is vendored")).unwrap_or_default();
    let outline = path(bez.clone(), (24.0, 24.0), Some(2.0))
        .w(px)
        .h(px)
        .themed(|t: &Theme, s| s.color(t.text_subtle));
    let filled = path(bez, (24.0, 24.0), None)
        .w(px)
        .h(px)
        .themed(|t: &Theme, s| s.color(t.warning.solid));

    let mut el = if fill >= 1.0 {
        stack().w(px).h(px).child(filled)
    } else if fill <= 0.0 {
        stack().w(px).h(px).child(outline)
    } else {
        stack().w(px).h(px).children([
            outline,
            div().w(px * fill).h_full().overflow_hidden().child(filled),
        ])
    };
    if let Some(m) = msg {
        el = el.cursor(Cursor::Pointer).on_click(m);
    }
    el
}

/// `"3"` / `"3.5"` — the accessible value text avoids float noise.
#[expect(clippy::cast_possible_truncation, reason = "whole-star values only")]
fn value_text(v: f32) -> String {
    if v == v.trunc() {
        format!("{}", v as u32)
    } else {
        format!("{v}")
    }
}

fn snap(v: f32, p: f32) -> f32 {
    (v / p).round() * p
}

impl<Msg: Clone + 'static> From<Rating<Msg>> for Element<Msg> {
    fn from(r: Rating<Msg>) -> Self {
        let interactive = r.on_change.is_some() && !r.disabled && !r.read_only;
        let max = max_f(r.max);

        let mut kids: Vec<Element<Msg>> = Vec::with_capacity(r.max as usize);
        for i in 0..r.max {
            let fill = (r.value - max_f(i)).clamp(0.0, 1.0);
            let msg = if interactive {
                let f = r.on_change.as_ref().expect("interactive implies on_change");
                Some(f(snap(max_f(i) + 1.0, r.precision)))
            } else {
                None
            };
            kids.push(star(fill, r.star_px, msg));
        }

        let mut el = row()
            .items_center()
            .gap(SP1)
            .shrink0()
            .semantics(Semantics::Slider {
                value: r.value,
                min: 0.0,
                max,
            })
            .value(format!("{} of {} stars", value_text(r.value), r.max))
            .label("Rating")
            .transition(Transition::colors())
            .children(kids);
        if r.disabled {
            el = el.opacity(0.5);
        }
        if interactive {
            let up = r.on_change.clone();
            let down = r.on_change.clone();
            let (v, p) = (r.value, r.precision);
            el = el.focusable(true).on_key(move |k| match k.key {
                Key::ArrowRight | Key::ArrowUp => up.as_ref().map(|f| f(snap((v + p).min(max), p))),
                Key::ArrowLeft | Key::ArrowDown => {
                    down.as_ref().map(|f| f(snap((v - p).max(0.0), p)))
                }
                Key::Home => down.as_ref().map(|f| f(0.0)),
                Key::End => up.as_ref().map(|f| f(max)),
                _ => None,
            });
        }
        if let Some(key) = &r.key {
            el = el.id(key);
        }
        el
    }
}
