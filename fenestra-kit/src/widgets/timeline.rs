//! Timeline: a vertical sequence of events — a status-colored dot per item,
//! a hairline rail connecting them, title + optional time on the line and
//! an optional body below.
//!
//! ```
//! use fenestra_kit::{timeline, timeline_item, Status};
//!
//! let el: fenestra_core::Element<()> = timeline([
//!     timeline_item("Deploy started").time("09:12"),
//!     timeline_item("Tests passed").time("09:14").status(Status::Success),
//!     timeline_item("Rollback").time("09:20").status(Status::Danger),
//! ])
//! .into();
//! ```

use fenestra_core::{
    Element, SP1, SP2, SP3, TextSize, Theme, Transition, Weight, col, div, row, text,
};

use super::display::Status;

/// One event of a [`timeline`].
pub struct TimelineItem<Msg> {
    title: String,
    time: Option<String>,
    body: Option<Element<Msg>>,
    status: Status,
}

/// A timeline event: a title (with an optional timestamp), an optional
/// body, and a dot color.
pub fn timeline_item<Msg>(title: impl Into<String>) -> TimelineItem<Msg> {
    TimelineItem {
        title: title.into(),
        time: None,
        body: None,
        status: Status::Accent,
    }
}

impl<Msg> TimelineItem<Msg> {
    /// A timestamp rendered after the title.
    #[must_use]
    pub fn time(mut self, time: impl Into<String>) -> Self {
        self.time = Some(time.into());
        self
    }

    /// Content below the title line.
    #[must_use]
    pub fn body(mut self, body: impl Into<Element<Msg>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// The dot color (Accent default; Success/Danger/Warning for outcomes).
    #[must_use]
    pub fn status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }
}

/// A vertical timeline of `items`.
pub fn timeline<Msg>(items: impl IntoIterator<Item = TimelineItem<Msg>>) -> Timeline<Msg> {
    Timeline {
        items: items.into_iter().collect(),
    }
}

/// A timeline under construction; converts into an [`Element`].
pub struct Timeline<Msg> {
    items: Vec<TimelineItem<Msg>>,
}

/// Rail geometry: the dot and the hairline that connects to the next item.
const RAIL: f32 = 20.0;
const DOT: f32 = 10.0;
const LINE: f32 = 2.0;

impl<Msg> From<Timeline<Msg>> for Element<Msg> {
    fn from(t: Timeline<Msg>) -> Self {
        let n = t.items.len();
        let rows: Vec<Element<Msg>> = t
            .items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let last = i + 1 == n;
                let dot = div()
                    .w(DOT)
                    .h(DOT)
                    .rounded_full()
                    .shrink0()
                    .transition(Transition::colors())
                    .themed(move |t: &Theme, s| {
                        s.bg(item.status.colors(t).solid).border(
                            3.0,
                            item.status.colors(t).bg,
                        )
                    });
                // The rail stretches to the item's content height; the last
                // item's rail ends at its dot.
                let line = (!last).then(|| {
                    div()
                        .w(LINE)
                        .grow()
                        .themed(|t: &Theme, s| s.bg(t.border_subtle))
                });

                let mut head = row().items_center().gap(SP2).child(
                    text(item.title)
                        .size(TextSize::Sm)
                        .weight(Weight::Medium)
                        .themed(|t: &Theme, s| s.color(t.text)),
                );
                if let Some(time) = item.time {
                    head = head.child(
                        text(time)
                            .size(TextSize::Xs)
                            .tabular()
                            .themed(|t: &Theme, s| s.color(t.text_muted)),
                    );
                }

                let mut content = col().gap(SP1).min_h(32.0).child(head);
                if let Some(body) = item.body {
                    content = content.child(body);
                }

                row()
                    .gap(SP2)
                    .shrink0()
                    .children([
                        col().w(RAIL).items_center().shrink0().children({
                            let mut rail = vec![dot];
                            rail.extend(line);
                            rail
                        }),
                        content,
                    ])
            })
            .collect();
        col().gap(SP3).children(rows)
    }
}
