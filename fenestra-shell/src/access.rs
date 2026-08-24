//! Maps a frame's accessibility projection ([`Frame::access_tree`]) to an
//! AccessKit tree update for the platform adapter.

use accesskit::{Action, Node, NodeId, Role, Toggled, Tree, TreeId, TreeUpdate};
use fenestra_core::{AccessNode, Frame, Semantics, WidgetId};

/// Builds a full tree update for the current frame. `focus` falls back to
/// the root (AccessKit requires a focus target); `scale` maps the logical
/// rects to physical pixels via a root transform.
///
/// The fallback covers a stale focus as well as an absent one. Focus is held
/// by widget id across frames, and an app can legitimately unmount whatever
/// held it — a list row filtered out by a search, a pane replaced by a route
/// change, a dialog dismissed. Publishing an id that is not among `nodes`
/// panics the consumer with "Focused ID … is not in the node list", which
/// turns an ordinary interaction into a crash. Nothing else prunes it, so the
/// check belongs here, where the node list is already in hand.
pub(crate) fn tree_update(frame: &Frame, focus: Option<WidgetId>, scale: f64) -> TreeUpdate {
    let root = frame.access_tree();
    let root_id = NodeId(root.id.0);
    let mut nodes = Vec::new();
    push_node(&mut nodes, &root, true, scale);
    let focus = focus
        .map(|f| NodeId(f.0))
        .filter(|f| nodes.iter().any(|(id, _)| id == f))
        .unwrap_or(root_id);
    TreeUpdate {
        nodes,
        tree: Some(Tree::new(root_id)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

fn push_node(nodes: &mut Vec<(NodeId, Node)>, an: &AccessNode, is_root: bool, scale: f64) {
    let mut node = Node::new(if is_root { Role::Window } else { role_of(an) });
    if is_root && scale != 1.0 {
        node.set_transform(accesskit::Affine::scale(scale));
    }
    node.set_bounds(accesskit::Rect {
        x0: an.rect.x0,
        y0: an.rect.y0,
        x1: an.rect.x1,
        y1: an.rect.y1,
    });
    if let Some(label) = &an.label {
        node.set_label(label.clone());
    }
    if let Some(value) = &an.value {
        node.set_value(value.clone());
    }
    if an.live {
        node.set_live(accesskit::Live::Polite);
    }
    match an.semantics {
        Some(Semantics::Checkbox { checked, mixed }) => node.set_toggled(if mixed {
            accesskit::Toggled::Mixed
        } else {
            toggled(checked)
        }),
        Some(Semantics::Switch { on }) => node.set_toggled(toggled(on)),
        Some(Semantics::Radio { selected }) => node.set_toggled(toggled(selected)),
        Some(Semantics::Tab { selected }) => node.set_selected(selected),
        Some(Semantics::Slider { value, min, max })
        | Some(Semantics::Spinbutton { value, min, max })
        | Some(Semantics::Meter { value, min, max }) => {
            node.set_numeric_value(f64::from(value));
            node.set_min_numeric_value(f64::from(min));
            node.set_max_numeric_value(f64::from(max));
        }
        Some(Semantics::ProgressBar { value: Some(v) }) => {
            node.set_numeric_value(f64::from(v));
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(1.0);
        }
        _ => {}
    }
    if an.focusable {
        node.add_action(Action::Focus);
    }
    if matches!(
        an.semantics,
        Some(
            Semantics::Button
                | Semantics::Checkbox { .. }
                | Semantics::Switch { .. }
                | Semantics::Radio { .. }
                | Semantics::Tab { .. }
                | Semantics::ComboBox
        )
    ) {
        node.add_action(Action::Click);
    }
    node.set_children(
        an.children
            .iter()
            .map(|c| NodeId(c.id.0))
            .collect::<Vec<_>>(),
    );
    nodes.push((NodeId(an.id.0), node));
    for child in &an.children {
        push_node(nodes, child, false, scale);
    }
}

fn role_of(an: &AccessNode) -> Role {
    match an.semantics {
        Some(Semantics::Button) => Role::Button,
        Some(Semantics::Checkbox { .. }) => Role::CheckBox,
        Some(Semantics::Switch { .. }) => Role::Switch,
        Some(Semantics::Radio { .. }) => Role::RadioButton,
        Some(Semantics::Slider { .. }) => Role::Slider,
        Some(Semantics::TextInput { multiline: false }) => Role::TextInput,
        Some(Semantics::TextInput { multiline: true }) => Role::MultilineTextInput,
        Some(Semantics::ComboBox) => Role::ComboBox,
        Some(Semantics::Dialog) => Role::Dialog,
        Some(Semantics::Tab { .. }) => Role::Tab,
        Some(Semantics::Alert) => Role::Alert,
        Some(Semantics::Label) => Role::Label,
        Some(Semantics::Image) => Role::Image,
        Some(Semantics::Spinbutton { .. }) => Role::SpinButton,
        Some(Semantics::Meter { .. }) => Role::Meter,
        Some(Semantics::ProgressBar { .. }) => Role::ProgressIndicator,
        None => Role::GenericContainer,
    }
}

fn toggled(on: bool) -> Toggled {
    if on { Toggled::True } else { Toggled::False }
}

#[cfg(test)]
mod tests {
    use super::tree_update;
    use fenestra_core::{Element, Fonts, FrameState, Theme, WidgetId, build_frame, col, text};

    fn frame() -> fenestra_core::Frame {
        let view: Element<()> = col().children([text("a"), text("b")]);
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        build_frame(
            &view,
            &Theme::light(),
            &mut fonts,
            &mut state,
            (200.0, 200.0),
            1.0,
        )
    }

    /// A focus that no longer resolves must not reach the consumer: it panics
    /// with "Focused ID … is not in the node list", and an app unmounting the
    /// focused widget — a row filtered out by a search, a pane replaced by a
    /// route change — is an ordinary thing for an app to do.
    #[test]
    fn a_stale_focus_falls_back_to_the_root() {
        let f = frame();
        let update = tree_update(&f, Some(WidgetId(0xdead_beef_dead_beef)), 1.0);
        let root = update.tree.as_ref().expect("a tree").root;
        assert_eq!(
            update.focus, root,
            "a focus that is not in the node list must fall back to the root"
        );
        assert!(
            update.nodes.iter().any(|(id, _)| *id == update.focus),
            "the published focus must be one of the published nodes"
        );
    }

    /// And a focus that does resolve is published unchanged.
    #[test]
    fn a_live_focus_is_published() {
        let f = frame();
        let live = f.access_tree().id;
        let update = tree_update(&f, Some(live), 1.0);
        assert_eq!(update.focus.0, live.0);
    }
}
