//! Swiper: a paged container — one page visible at a time, navigated by
//! swipe flicks, ←/→ (Home/End jump), one tab stop. The app owns `current`
//! and echoes `on_change` back; pair with [`page_control`](crate::page_control)
//! for the dots.
//!
//! Page changes crossfade (the incoming page fades in, the outgoing ghost
//! fades out). A directional slide needs the previous index — state an
//! Elm-pure widget cannot own — so the honest default is the crossfade the
//! primitives express exactly.
//!
//! ```
//! use fenestra_core::text;
//! use fenestra_kit::swiper;
//!
//! #[derive(Clone)]
//! enum Msg {
//!     Page(usize),
//! }
//!
//! let el: fenestra_core::Element<Msg> = swiper(0)
//!     .page(text("First"))
//!     .page(text("Second"))
//!     .on_change(Msg::Page)
//!     .into();
//! ```

use fenestra_core::{
    Cursor, Element, Key, MotionDuration, Semantics, SwipeDir, Transition, col,
};

/// A swiper under construction; converts into an [`Element`].
pub struct Swiper<Msg> {
    pages: Vec<Element<Msg>>,
    current: usize,
    on_change: Option<std::rc::Rc<dyn Fn(usize) -> Msg>>,
    disabled: bool,
    key: Option<String>,
}

/// A paged container showing the page at `current`.
pub fn swiper<Msg>(current: usize) -> Swiper<Msg> {
    Swiper {
        pages: Vec::new(),
        current,
        on_change: None,
        disabled: false,
        key: None,
    }
}

impl<Msg> Swiper<Msg> {
    /// Appends a page (in build order).
    #[must_use]
    pub fn page(mut self, page: impl Into<Element<Msg>>) -> Self {
        self.pages.push(page.into());
        self
    }

    /// Maps the new page index to a message.
    #[must_use]
    pub fn on_change(mut self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(f));
        self
    }

    /// Disables navigation (the page still renders).
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Stable identity key (page enter/exit detection keys off it).
    #[must_use]
    pub fn id(mut self, key: &str) -> Self {
        self.key = Some(key.to_owned());
        self
    }
}

impl<Msg: Clone + 'static> From<Swiper<Msg>> for Element<Msg> {
    fn from(mut s: Swiper<Msg>) -> Self {
        let n = s.pages.len();
        if n == 0 {
            return col();
        }
        let current = s.current.min(n - 1);
        let interactive = s.on_change.is_some() && !s.disabled;

        let key = s.key.clone().unwrap_or_else(|| "swiper".to_owned());
        let page = std::mem::replace(&mut s.pages[current], col())
            .id(&format!("{key}-page-{current}"))
            // Crossfade: the incoming page fades in; the outgoing ghost fades
            // out in place (a directional slide needs the previous index —
            // state an Elm-pure widget cannot own).
            .enter(Transition::colors().duration_ms(MotionDuration::Base.ms()))
            .exit_to(0.0, 1.0, 0.0, 0.0);

        let mut el = col().child(page);
        if interactive {
            let f = s.on_change.expect("interactive implies on_change");
            let swipe = std::rc::Rc::clone(&f);
            let up = std::rc::Rc::clone(&f);
            let down = f;
            el = col()
                .focusable(true)
                .cursor(Cursor::Default)
                .on_swipe(move |dir| match dir {
                    SwipeDir::Left => (current + 1 < n).then(|| swipe(current + 1)),
                    SwipeDir::Right => current.checked_sub(1).map(|i| swipe(i)),
                    _ => None,
                })
                .on_key(move |k| match k.key {
                    Key::ArrowRight => (current + 1 < n).then(|| up(current + 1)),
                    Key::ArrowLeft => current.checked_sub(1).map(|i| down(i)),
                    Key::Home => Some(down(0)),
                    Key::End => Some(up(n - 1)),
                    _ => None,
                })
                .child(el);
        }
        el.semantics(Semantics::Label)
            .label(format!("Page {} of {}", current + 1, n))
            .id(&key)
    }
}
