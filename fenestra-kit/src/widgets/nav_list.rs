//! Sidebar navigation list: the HIG-sidebar / app-rail row list — icon +
//! label rows with an optional trailing badge, one selected row, and tree
//! keyboard (↑/↓ step the selection, Home/End jump; one tab stop).
//!
//! Elm-pure: the app owns `selected` and echoes `on_select` back.
//!
//! ```
//! use fenestra_kit::{nav_item, nav_list};
//!
//! #[derive(Clone)]
//! enum Msg {
//!     Go(usize),
//! }
//!
//! let el: fenestra_core::Element<Msg> = nav_list(
//!     [nav_item("Inbox"), nav_item("Sent")],
//!     0,
//! )
//! .on_select(Msg::Go)
//! .into();
//! ```

use fenestra_core::{
    Cursor, Element, Key, SP1, SP2, Semantics, TextSize, Theme, Transition, Weight, col, row,
    spacer, text,
};

use super::ControlSize;

/// One row of a [`nav_list`].
pub struct NavItem<Msg> {
    label: String,
    icon: Option<Element<Msg>>,
    badge: Option<String>,
}

/// A navigation row: a label, optionally a leading icon and a trailing
/// badge (a count or status word).
pub fn nav_item<Msg>(label: impl Into<String>) -> NavItem<Msg> {
    NavItem {
        label: label.into(),
        icon: None,
        badge: None,
    }
}

impl<Msg> NavItem<Msg> {
    /// A leading 16px icon.
    #[must_use]
    pub fn icon(mut self, icon: impl Into<Element<Msg>>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// A trailing badge (an unread count, a status word).
    #[must_use]
    pub fn badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = Some(badge.into());
        self
    }
}

/// A nav list under construction; converts into an [`Element`].
pub struct NavList<Msg> {
    items: Vec<NavItem<Msg>>,
    selected: usize,
    on_select: Option<Box<dyn Fn(usize) -> Msg>>,
    width: f32,
    size: ControlSize,
    key: Option<String>,
}

/// A sidebar navigation list with `selected` highlighted. Clicking a row —
/// or stepping onto it with the keyboard — emits `on_select(index)`; the app
/// echoes the new index back.
pub fn nav_list<Msg>(
    items: impl IntoIterator<Item = NavItem<Msg>>,
    selected: usize,
) -> NavList<Msg> {
    NavList {
        items: items.into_iter().collect(),
        selected,
        on_select: None,
        width: 200.0,
        size: ControlSize::Sm,
        key: None,
    }
}

impl<Msg> NavList<Msg> {
    /// Maps a row choice to its index.
    #[must_use]
    pub fn on_select(mut self, f: impl Fn(usize) -> Msg + 'static) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }

    /// Sets the width in logical px (200 by default).
    #[must_use]
    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Sets the row height via [`ControlSize`] (Sm 32 / Md 36 / Lg 40).
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

impl<Msg: Clone + 'static> From<NavList<Msg>> for Element<Msg> {
    fn from(n: NavList<Msg>) -> Self {
        let m = n.size.metrics();
        let count = n.items.len();
        let selected = if count > 0 { n.selected.min(count - 1) } else { 0 };

        let rows: Vec<Element<Msg>> = n
            .items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let is_selected = i == selected;
                let wired = n.on_select.is_some();
                let mut kids: Vec<Element<Msg>> = Vec::with_capacity(3);
                kids.extend(item.icon.map(|icon| {
                    icon.w(16.0)
                        .h(16.0)
                        .shrink0()
                        .themed(move |t: &Theme, s| {
                            s.color(if is_selected || !wired {
                                t.text
                            } else {
                                t.text_muted
                            })
                        })
                }));
                kids.push(
                    text(item.label.clone())
                        .size(m.font)
                        .weight(if is_selected { Weight::Medium } else { Weight::Regular })
                        .themed(move |t: &Theme, s| {
                            s.color(if is_selected || !wired {
                                t.text
                            } else {
                                t.text_muted
                            })
                        }),
                );
                if let Some(badge) = item.badge {
                    kids.push(spacer());
                    kids.push(
                        row()
                            .items_center()
                            .px(6.0)
                            .h(18.0)
                            .rounded_full()
                            .shrink0()
                            .themed(move |t: &Theme, s| {
                                if is_selected {
                                    s.bg(t.accent_bg)
                                } else {
                                    s.bg(t.element)
                                }
                            })
                            .child(
                                text(badge)
                                    .size(TextSize::Xs)
                                    .tabular()
                                    .themed(move |t: &Theme, s| {
                                        s.color(if is_selected {
                                            t.accent_text
                                        } else {
                                            t.text_muted
                                        })
                                    }),
                            ),
                    );
                }

                let mut r = row()
                    .items_center()
                    .gap(SP2)
                    .px(SP2)
                    .h(m.height)
                    .w_full()
                    .themed(move |t: &Theme, s| s.rounded(t.radius.sm))
                    .shrink0()
                    .semantics(Semantics::ListItem { selected: is_selected })
                    .label(item.label.clone())
                    .transition(Transition::colors())
                    .themed(move |t: &Theme, s| {
                        if is_selected {
                            s.bg(t.element)
                        } else {
                            s
                        }
                    })
                    .children(kids);
                if let Some(f) = &n.on_select {
                    r = r.on_click(f(i)).cursor(Cursor::Pointer).state_layer(|t: &Theme| t.text);
                }
                r
            })
            .collect();

        let mut body = col().p(SP1).gap(2.0).w(n.width).children(rows);
        if let Some(f) = n.on_select.filter(|_| count > 0) {
            let f = std::rc::Rc::new(f);
            let nav = std::rc::Rc::clone(&f);
            body = body.focusable(true).on_key(move |k| {
                let step = |i: usize, d: usize| (i + d) % count;
                let back = |i: usize| i.checked_sub(1).unwrap_or(count - 1);
                let target = match k.key {
                    Key::ArrowDown => Some(step(selected, 1)),
                    Key::ArrowUp => Some(back(selected)),
                    Key::Home => Some(0),
                    Key::End => Some(count - 1),
                    _ => None,
                };
                target.map(|i| nav(i))
            });
        }
        if let Some(key) = &n.key {
            body = body.id(key);
        }
        body
    }
}
