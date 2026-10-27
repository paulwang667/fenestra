//! App chrome laid out by the theme's material style: a top [`app_bar`], a
//! bottom [`tab_bar`], and a [`scaffold`] around the content. In the glass
//! styles the bars float over the content, which runs on under them (Liquid
//! Glass's bottom bar a dock clear of the screen's edges); in solid and tonal
//! they sit beside it. The tree is the same in every style — only themed
//! padding and surfaces change — so switching styles keeps focus and scroll.

use fenestra_core::{
    Element, SP2, SP3, SP4, Semantics, Surface, TextSize, Theme, Vibrancy, Weight, col, row, text,
};

/// A top app bar under construction; converts into an [`Element`].
pub struct AppBar<Msg> {
    title: String,
    leading: Option<Element<Msg>>,
    trailing: Option<Element<Msg>>,
}

/// A top app bar: the title (in the bar's label colour — vibrant on Liquid
/// Glass), with optional controls either side, on a [`Surface::Bar`].
pub fn app_bar<Msg>(title: impl Into<String>) -> AppBar<Msg> {
    AppBar {
        title: title.into(),
        leading: None,
        trailing: None,
    }
}

impl<Msg> AppBar<Msg> {
    /// A control before the title (a back button).
    #[must_use]
    pub fn leading(mut self, el: impl Into<Element<Msg>>) -> Self {
        self.leading = Some(el.into());
        self
    }

    /// Controls after the title (actions).
    #[must_use]
    pub fn trailing(mut self, el: impl Into<Element<Msg>>) -> Self {
        self.trailing = Some(el.into());
        self
    }
}

impl<Msg: 'static> From<AppBar<Msg>> for Element<Msg> {
    fn from(bar: AppBar<Msg>) -> Self {
        let title = text(bar.title)
            .size(TextSize::Lg)
            .weight(Weight::Semibold)
            .grow()
            .truncate()
            .themed(|t: &Theme, s| s.color(t.label_on(Surface::Bar, Vibrancy::Primary)));
        let mut row = row()
            .w_full()
            .h(APP_BAR_H)
            .px(SP4)
            .gap(SP2)
            .items_center()
            .shrink0()
            .surface(Surface::Bar);
        if let Some(leading) = bar.leading {
            row = row.child(leading);
        }
        row = row.child(title);
        if let Some(trailing) = bar.trailing {
            row = row.child(trailing);
        }
        row
    }
}

/// The app bar's height.
pub const APP_BAR_H: f32 = 48.0;

/// The tab bar's height.
pub const TAB_BAR_H: f32 = 56.0;

/// One tab of a [`tab_bar`].
pub struct TabItem<Msg> {
    label: String,
    icon: Element<Msg>,
    active: bool,
    on_select: Msg,
}

/// A tab: its icon over its label, the accent when `active`; a button named
/// `label` that emits `on_select`.
pub fn tab_item<Msg>(
    label: impl Into<String>,
    icon: impl Into<Element<Msg>>,
    active: bool,
    on_select: Msg,
) -> TabItem<Msg> {
    TabItem {
        label: label.into(),
        icon: icon.into(),
        active,
        on_select,
    }
}

/// A bottom navigation bar of `items`, evenly spread, on a
/// [`Surface::Bar`] (in a [`scaffold`], a floating [`Surface::Dock`] under
/// Liquid Glass).
pub fn tab_bar<Msg: Clone + 'static>(
    items: impl IntoIterator<Item = TabItem<Msg>>,
) -> Element<Msg> {
    row()
        .w_full()
        .h(TAB_BAR_H)
        .items_center()
        .shrink0()
        .surface(Surface::Bar)
        .children(items.into_iter().map(|item| {
            let active = item.active;
            col()
                .grow()
                .h_full()
                .items_center()
                .justify_center()
                .gap(2.0)
                .focusable(true)
                .semantics(Semantics::Button)
                .label(item.label.clone())
                .on_click(item.on_select)
                .children([
                    item.icon,
                    text(item.label)
                        .size(TextSize::Xs)
                        .themed(move |t: &Theme, s| {
                            s.color(if active {
                                t.accent
                            } else {
                                t.label_on(Surface::Bar, Vibrancy::Secondary)
                            })
                        }),
                ])
        }))
}

/// A scaffold under construction; converts into an [`Element`].
pub struct Scaffold<Msg> {
    content: Element<Msg>,
    top: Option<(Element<Msg>, f32)>,
    bottom: Option<(Element<Msg>, f32)>,
}

/// A page: `content` (usually a scroll container) with an optional top bar
/// and bottom bar, laid out by the theme's material style (see the module).
pub fn scaffold<Msg>(content: impl Into<Element<Msg>>) -> Scaffold<Msg> {
    Scaffold {
        content: content.into(),
        top: None,
        bottom: None,
    }
}

impl<Msg> Scaffold<Msg> {
    /// The top bar and its height.
    #[must_use]
    pub fn top(mut self, bar: impl Into<Element<Msg>>, height: f32) -> Self {
        self.top = Some((bar.into(), height));
        self
    }

    /// The bottom bar and its height.
    #[must_use]
    pub fn bottom(mut self, bar: impl Into<Element<Msg>>, height: f32) -> Self {
        self.bottom = Some((bar.into(), height));
        self
    }
}

impl<Msg: 'static> From<Scaffold<Msg>> for Element<Msg> {
    fn from(page: Scaffold<Msg>) -> Self {
        let top_h = page.top.as_ref().map_or(0.0, |(_, h)| *h);
        let bottom_h = page.bottom.as_ref().map_or(0.0, |(_, h)| *h);
        // Over the bars: the content runs on under them, its own padding
        // keeping its first and last rows clear; beside them: the body
        // makes room.
        let content = page.content.themed(move |t: &Theme, s| {
            if t.material_style.bars_overlay_content() {
                let dock_gap = if t.material_style.floating_dock() {
                    SP2
                } else {
                    0.0
                };
                s.pt(top_h).pb(bottom_h + dock_gap)
            } else {
                s
            }
        });
        let body = col()
            .grow()
            .w_full()
            .overflow_hidden()
            .themed(move |t: &Theme, s| {
                if t.material_style.bars_overlay_content() {
                    s
                } else {
                    s.pt(top_h).pb(bottom_h)
                }
            })
            .child(content);
        let mut root = col().w_full().h_full().overflow_hidden().child(body);
        // The bars paint after the body, so above it.
        if let Some((bar, _)) = page.bottom {
            let bar = bar.themed(|t: &Theme, s| {
                if t.material_style.floating_dock() {
                    t.apply_surface(Surface::Dock, s)
                } else {
                    s
                }
            });
            root = root.child(
                col()
                    .absolute()
                    .left(0.0)
                    .bottom(0.0)
                    .w_full()
                    .themed(|t: &Theme, s| {
                        if t.material_style.floating_dock() {
                            s.px(SP3).pb(SP2)
                        } else {
                            s
                        }
                    })
                    .child(bar),
            );
        }
        if let Some((bar, _)) = page.top {
            root = root.child(bar.absolute().top(0.0).left(0.0).w_full());
        }
        root
    }
}
