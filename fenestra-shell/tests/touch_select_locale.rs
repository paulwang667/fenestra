//! The edit menu speaks the UI language. Its own binary: the locale is the
//! process-wide font system's.

use fenestra_core::{App, Element, Theme, by, col, text};
use fenestra_shell::{Harness, with_fonts};

struct Page;

impl App for Page {
    type Msg = ();
    fn update(&mut self, _msg: ()) {}
    fn view(&self) -> Element<()> {
        col()
            .p(20.0)
            .children([text("hello wide world").size_px(20.0).selectable()])
    }
}

#[test]
fn menu_labels_follow_the_locale() {
    let labels = |h: &mut Harness<Page>| {
        let r = h.get(&by::label("hello wide world")).rect;
        #[expect(clippy::cast_possible_truncation)]
        h.touch_down_at(((r.x0 + 75.0) as f32, r.center().y as f32));
        h.pump(500.0);
        h.touch_up();
        h.touch_selection().expect("selected").labels.clone()
    };
    let mut h = Harness::new(Page, Theme::light(), (400, 200));
    assert_eq!(labels(&mut h), vec!["Copy", "Select All"]);
    with_fonts(|f| f.set_locale(Some("zh-CN")));
    let mut h = Harness::new(Page, Theme::light(), (400, 200));
    assert_eq!(labels(&mut h), vec!["复制", "全选"]);
    with_fonts(|f| f.set_locale(None));
}
