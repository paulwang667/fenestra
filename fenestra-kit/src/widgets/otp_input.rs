//! OTP / PIN input: `len` single-character boxes in a horizontal roving
//! scope — a commit that fills a box advances focus to the next one
//! (core roving), ←/→ move between boxes, and the reassembled code is
//! emitted on every edit.
//!
//! Elm-pure: the app owns `code` and echoes `on_change` back. Clearing a
//! middle box shifts the tail left (a string cannot hold a hole); full-code
//! paste lands in the focused box and truncates to one character.
//!
//! ```
//! use fenestra_kit::otp_input;
//!
//! #[derive(Clone)]
//! enum Msg {
//!     Code(String),
//! }
//!
//! let el: fenestra_core::Element<Msg> =
//!     otp_input("", 6).on_change(Msg::Code).into();
//! ```

use fenestra_core::{Element, RovingAxis, SP2, Semantics, row};

/// An OTP input under construction; converts into an [`Element`].
pub struct OtpInput<Msg> {
    code: String,
    len: usize,
    disabled: bool,
    on_change: Option<std::rc::Rc<dyn Fn(String) -> Msg>>,
    key: Option<String>,
}

/// An `len`-digit code entry showing the app-owned `code`.
pub fn otp_input<Msg>(code: impl Into<String>, len: usize) -> OtpInput<Msg> {
    OtpInput {
        code: code.into(),
        len: len.max(1),
        disabled: false,
        on_change: None,
        key: None,
    }
}

impl<Msg> OtpInput<Msg> {
    /// Maps the reassembled code to a message (fires on every edit).
    #[must_use]
    pub fn on_change(mut self, f: impl Fn(String) -> Msg + 'static) -> Self {
        self.on_change = Some(std::rc::Rc::new(f));
        self
    }

    /// Disables the boxes (dimmed, inert).
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Stable identity key (box editor state is kept per id).
    #[must_use]
    pub fn id(mut self, key: &str) -> Self {
        self.key = Some(key.to_owned());
        self
    }
}

impl<Msg: Clone + 'static> From<OtpInput<Msg>> for Element<Msg> {
    fn from(o: OtpInput<Msg>) -> Self {
        let len = o.len;
        let key = o.key.clone().unwrap_or_else(|| "otp".to_owned());
        let chars = std::rc::Rc::new(o.code.chars().take(len).collect::<Vec<char>>());

        let mut w = row().gap(SP2).focusable(true).roving(RovingAxis::Horizontal);
        for i in 0..len {
            let chars = std::rc::Rc::clone(&chars);
            let box_code: String = chars.get(i).map(|c| c.to_string()).unwrap_or_default();
            let mut b = crate::text_input(box_code)
                .placeholder("")
                .width(44.0)
                .max_chars(1)
                .id(&format!("{key}-{i}"))
                .disabled(o.disabled);
            // Splice the box's single character back into the code; an
            // emptied box drops its character (the tail shifts left).
            if let Some(on_change) = o.on_change.clone() {
                b = b.on_input(move |s| {
                    let mut next: Vec<char> = chars.iter().copied().collect();
                    match s.chars().next() {
                        Some(c) if i < next.len() => next[i] = c,
                        Some(c) => next.push(c),
                        None => {
                            next.truncate(i);
                        }
                    }
                    on_change(next.into_iter().collect())
                });
            }
            if let Some(id) = &o.key {
                b = b.id(&format!("{id}-{i}"));
            }
            w = w.child(b);
        }
        w.semantics(Semantics::Label)
            .label("Verification code")
            .opacity(if o.disabled { 0.5 } else { 1.0 })
    }
}
