//! Arrow roving (APG roving tabindex): a roving scope leaves the Tab order,
//! its container becomes the single tab stop, the axis arrows move real
//! focus between the scope's focusable candidates (wraparound), and Enter
//! activates whichever item holds focus. Focus position is probed
//! behaviorally — Enter fires the focused item's click.

use fenestra_core::{
    Element, Fonts, FrameState, InputEvent, Key, KeyInput, RovingAxis, Theme, build_frame, col,
    dispatch, row, text,
};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Pick(usize),
}

fn view() -> Element<Msg> {
    col().children([
        col()
            .focusable(true)
            .on_click(Msg::Pick(9))
            .child(text("Before")),
        col()
            .focusable(true)
            .roving(RovingAxis::Vertical)
            .children([
                row()
                    .focusable(true)
                    .on_click(Msg::Pick(0))
                    .child(text("One")),
                row()
                    .focusable(true)
                    .on_click(Msg::Pick(1))
                    .child(text("Two")),
                row()
                    .focusable(true)
                    .on_click(Msg::Pick(2))
                    .child(text("Three")),
            ]),
        col()
            .focusable(true)
            .on_click(Msg::Pick(3))
            .child(text("After")),
    ])
}

/// The scope's items are not tab stops; the container is. Six focusable
/// nodes exist — only Before, the container, and After are tab stops.
#[test]
fn roving_items_leave_the_tab_order() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let v = view();
    let frame = build_frame(&v, &theme, &mut fonts, &mut state, (200.0, 300.0), 1.0);
    assert_eq!(frame.focusables().len(), 3);
}

/// Arrows move focus within the scope: entering from the container at the
/// near end, wrapping at both ends. Each arrow is probed with Enter — the
/// activation reveals which item holds focus.
#[test]
fn arrows_move_focus_probed_by_enter() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let mut picked: Vec<usize> = Vec::new();
    let key = |k: Key| InputEvent::Key(KeyInput::plain(k));
    let steps = [
        (InputEvent::Tab, None),     // → Before
        (InputEvent::Tab, None),     // → scope container
        (key(Key::ArrowDown), None), // container enters at One
        (key(Key::Enter), Some(0)),
        (key(Key::ArrowDown), None),
        (key(Key::Enter), Some(1)),
        (key(Key::ArrowDown), None),
        (key(Key::Enter), Some(2)),
        (key(Key::ArrowDown), None), // wraps: Three → One
        (key(Key::Enter), Some(0)),
        (key(Key::ArrowUp), None), // wraps back: One → Three
        (key(Key::Enter), Some(2)),
    ];
    for (ev, expect) in steps {
        let v = view();
        let frame = build_frame(&v, &theme, &mut fonts, &mut state, (200.0, 300.0), 1.0);
        let out = dispatch(&v, &frame, &mut state, &mut fonts, ev.clone());
        for msg in out.msgs {
            let Msg::Pick(i) = msg;
            picked.push(i);
        }
        if let Some(i) = expect {
            assert_eq!(picked.last(), Some(&i), "focus probe after {ev:?}");
        }
    }
    assert_eq!(picked, vec![0, 1, 2, 0, 2]);
}

/// Tab from a focused item stands in for the container: focus lands past
/// the scope (on After), and Enter there activates After — not an item.
#[test]
fn tab_from_an_item_stands_in_for_the_container() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let mut picked: Vec<usize> = Vec::new();
    let key = |k: Key| InputEvent::Key(KeyInput::plain(k));
    let steps = [
        InputEvent::Tab,     // → Before
        InputEvent::Tab,     // → scope container
        key(Key::ArrowDown), // → One
        InputEvent::Tab,     // container stands in → After
        key(Key::Enter),     // activates After (3), not an item
    ];
    for ev in steps {
        let v = view();
        let frame = build_frame(&v, &theme, &mut fonts, &mut state, (200.0, 300.0), 1.0);
        let out = dispatch(&v, &frame, &mut state, &mut fonts, ev);
        for msg in out.msgs {
            let Msg::Pick(i) = msg;
            picked.push(i);
        }
    }
    assert_eq!(picked, vec![3]);
}

#[test]
fn roving_focus_query_finds_the_scope() {
    let theme = Theme::light();
    let mut fonts = Fonts::embedded();
    let mut state = FrameState::new();
    let v = view();
    let frame = build_frame(&v, &theme, &mut fonts, &mut state, (200.0, 300.0), 1.0);
    let order = frame.focusables();
    let container = order[1];
    let scope = frame.roving_focus(container).expect("scope found");
    assert_eq!(scope.candidates.len(), 3, "{scope:?}");
    let item_one = scope.candidates[0];
    let scope2 = frame.roving_focus(item_one).expect("scope from an item");
    assert_eq!(scope2.container, container);
}
