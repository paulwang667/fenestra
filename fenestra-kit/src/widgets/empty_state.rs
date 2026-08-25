//! Empty state: the placeholder for a list, search, or dashboard section
//! with nothing to show — a muted icon, a headline, an optional guidance
//! line, and an optional action.
//!
//! ```
//! use fenestra_kit::empty_state;
//!
//! #[derive(Clone)]
//! enum Msg {
//!     Clear,
//! }
//!
//! let el: fenestra_core::Element<Msg> = empty_state("No results")
//!     .message("Try different keywords.")
//!     .action("Clear filters", Msg::Clear)
//!     .into();
//! ```

use fenestra_core::{Element, SP3, SP6, Semantics, TextSize, Theme, Weight, col, text};

use super::button::{ButtonVariant, button};
use crate::icons;

/// An empty state under construction; converts into an [`Element`].
pub struct EmptyState<Msg> {
    title: String,
    message: Option<String>,
    icon: Option<Element<Msg>>,
    action: Option<(String, Msg)>,
    key: Option<String>,
}

/// An empty state with the headline `title`.
pub fn empty_state<Msg>(title: impl Into<String>) -> EmptyState<Msg> {
    EmptyState {
        title: title.into(),
        message: None,
        icon: None,
        action: None,
        key: None,
    }
}

impl<Msg> EmptyState<Msg> {
    /// Secondary guidance under the title.
    #[must_use]
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Overrides the default muted search icon.
    #[must_use]
    pub fn icon(mut self, icon: impl Into<Element<Msg>>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// A single action button under the message.
    #[must_use]
    pub fn action(mut self, label: impl Into<String>, msg: Msg) -> Self {
        self.action = Some((label.into(), msg));
        self
    }

    /// Stable identity key.
    #[must_use]
    pub fn id(mut self, key: &str) -> Self {
        self.key = Some(key.to_owned());
        self
    }
}

impl<Msg: Clone + 'static> From<EmptyState<Msg>> for Element<Msg> {
    fn from(e: EmptyState<Msg>) -> Self {
        let icon = e
            .icon
            .unwrap_or_else(|| icons::lucide::by_name("search").expect("search is vendored"))
            .w(36.0)
            .h(36.0)
            .themed(|t: &Theme, s| s.color(t.text_subtle));

        let mut kids = col().items_center().gap(SP3).children([
            col().items_center().child(icon),
            text(e.title)
                .size(TextSize::Base)
                .weight(Weight::Medium)
                .semantics(Semantics::Label),
        ]);

        if let Some(message) = e.message {
            kids = kids.child(
                text(message)
                    .size(TextSize::Sm)
                    .themed(|t: &Theme, s| s.color(t.text_muted)),
            );
        }
        if let Some((label, msg)) = e.action {
            kids = kids.child(
                button(label)
                    .variant(ButtonVariant::Secondary)
                    .on_click(msg),
            );
        }

        let mut el = col()
            .items_center()
            .justify_center()
            .p(SP6)
            .gap(SP3)
            .children([kids]);
        if let Some(key) = &e.key {
            el = el.id(key);
        }
        el
    }
}
