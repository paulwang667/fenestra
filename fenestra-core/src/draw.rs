//! A backend-neutral drawing interface. Everything fenestra paints goes
//! through [`DrawTarget`]; [`SceneTarget`] records it into a vello scene.

use kurbo::{Affine, Rect, Shape, Stroke};
use peniko::{BrushRef, Color, Fill, FontData, ImageData, Mix};

/// One positioned glyph of a [`GlyphRun`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    /// Glyph id within the run's font.
    pub id: u32,
    /// Pen x, in the run's coordinate space.
    pub x: f32,
    /// Baseline y, in the run's coordinate space.
    pub y: f32,
}

/// The shared parameters of a run of glyphs drawn with one font and color.
#[derive(Clone, Copy, Debug)]
pub struct GlyphRun<'a> {
    /// The font the glyph ids index into.
    pub font: &'a FontData,
    /// Font size in pixels per em.
    pub font_size: f32,
    /// Whether outlines are hinted.
    pub hint: bool,
    /// Variable-font axis coordinates (normalized, 2.14 fixed point).
    pub normalized_coords: &'a [i16],
    /// Maps the run's coordinate space to the target's.
    pub transform: Affine,
    /// Per-glyph transform, e.g. the skew of a synthesized italic.
    pub glyph_transform: Option<Affine>,
    /// Fill color of every glyph.
    pub color: Color,
}

/// A backend that fenestra paints into: the drawing operations fenestra
/// uses, plus a transform stack.
///
/// Every op's own `transform` is applied first, then the current stack.
pub trait DrawTarget {
    /// Fills `shape` with `brush`.
    fn fill<'b>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    );
    /// Strokes `shape` with `brush`.
    fn stroke<'b>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    );
    /// Starts a layer clipped to `clip` and composited at `alpha` (normal
    /// blending) when popped.
    fn push_layer(&mut self, clip_style: Fill, alpha: f32, transform: Affine, clip: &impl Shape);
    /// Starts a layer that only clips to `clip`.
    fn push_clip_layer(&mut self, clip_style: Fill, transform: Affine, clip: &impl Shape);
    /// Ends the most recently pushed layer.
    fn pop_layer(&mut self);
    /// Draws `image` with its pixel grid mapped through `transform`.
    fn draw_image(&mut self, image: &ImageData, transform: Affine);
    /// Draws a gaussian-blurred rounded rectangle (a drop shadow).
    fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: Rect,
        color: Color,
        radius: f64,
        std_dev: f64,
    );
    /// Draws `glyphs` with the parameters of `run`.
    fn draw_glyphs(&mut self, run: &GlyphRun<'_>, glyphs: impl Iterator<Item = Glyph>);
    /// Everything drawn until the matching `pop_transform` is mapped through
    /// `transform` (applied after each op's own transform).
    fn push_transform(&mut self, transform: Affine);
    /// Ends the most recent `push_transform`.
    fn pop_transform(&mut self);
}

/// The vello implementation of [`DrawTarget`], recording into a [`vello::Scene`].
pub struct SceneTarget<'s> {
    scene: &'s mut vello::Scene,
    stack: Vec<Affine>,
}

impl<'s> SceneTarget<'s> {
    /// Draws into `scene`, appending to whatever it already holds.
    pub fn new(scene: &'s mut vello::Scene) -> Self {
        Self {
            scene,
            stack: Vec::new(),
        }
    }

    /// The composed transform currently applied to every op.
    pub fn current_transform(&self) -> Affine {
        self.stack.last().copied().unwrap_or(Affine::IDENTITY)
    }

    /// Ends painting; debug-asserts every `push_transform` was popped.
    pub fn finish(self) {
        debug_assert!(self.stack.is_empty(), "unbalanced push_transform");
    }
}

impl DrawTarget for SceneTarget<'_> {
    fn fill<'b>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let t = self.current_transform() * transform;
        self.scene.fill(style, t, brush, brush_transform, shape);
    }

    fn stroke<'b>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let t = self.current_transform() * transform;
        self.scene.stroke(style, t, brush, brush_transform, shape);
    }

    fn push_layer(&mut self, clip_style: Fill, alpha: f32, transform: Affine, clip: &impl Shape) {
        let t = self.current_transform() * transform;
        self.scene
            .push_layer(clip_style, Mix::Normal, alpha, t, clip);
    }

    fn push_clip_layer(&mut self, clip_style: Fill, transform: Affine, clip: &impl Shape) {
        let t = self.current_transform() * transform;
        self.scene.push_clip_layer(clip_style, t, clip);
    }

    fn pop_layer(&mut self) {
        self.scene.pop_layer();
    }

    fn draw_image(&mut self, image: &ImageData, transform: Affine) {
        let t = self.current_transform() * transform;
        self.scene.draw_image(image, t);
    }

    fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: Rect,
        color: Color,
        radius: f64,
        std_dev: f64,
    ) {
        let t = self.current_transform() * transform;
        self.scene
            .draw_blurred_rounded_rect(t, rect, color, radius, std_dev);
    }

    fn draw_glyphs(&mut self, run: &GlyphRun<'_>, glyphs: impl Iterator<Item = Glyph>) {
        let t = self.current_transform() * run.transform;
        self.scene
            .draw_glyphs(run.font)
            .brush(run.color)
            .hint(run.hint)
            .transform(t)
            .glyph_transform(run.glyph_transform)
            .font_size(run.font_size)
            .normalized_coords(run.normalized_coords)
            .draw(
                Fill::NonZero,
                glyphs.map(|g| vello::Glyph {
                    id: g.id,
                    x: g.x,
                    y: g.y,
                }),
            );
    }

    fn push_transform(&mut self, transform: Affine) {
        let t = self.current_transform() * transform;
        self.stack.push(t);
    }

    fn pop_transform(&mut self) {
        self.stack.pop();
    }
}

/// A [`DrawTarget`] that records ops (with the transform stack composed in),
/// for tests that check what was painted without rasterizing.
#[cfg(test)]
pub(crate) mod recorder {
    use super::*;

    /// One recorded op; every transform already includes the stack.
    #[derive(Clone, Debug)]
    #[expect(dead_code, reason = "`bbox` is only read through Debug output")]
    pub(crate) enum Op {
        Fill {
            transform: Affine,
            bbox: Rect,
        },
        Stroke {
            transform: Affine,
            bbox: Rect,
        },
        PushLayer {
            alpha: f32,
            transform: Affine,
        },
        PushClip {
            transform: Affine,
        },
        PopLayer,
        Image {
            transform: Affine,
        },
        Shadow {
            transform: Affine,
        },
        Glyphs {
            transform: Affine,
            glyph_transform: Option<Affine>,
            coords: Vec<i16>,
            count: usize,
        },
    }

    impl Op {
        /// The op's composed transform; `None` for `PopLayer`.
        pub(crate) fn transform(&self) -> Option<Affine> {
            match self {
                Op::Fill { transform, .. }
                | Op::Stroke { transform, .. }
                | Op::PushLayer { transform, .. }
                | Op::PushClip { transform }
                | Op::Image { transform }
                | Op::Shadow { transform }
                | Op::Glyphs { transform, .. } => Some(*transform),
                Op::PopLayer => None,
            }
        }
    }

    #[derive(Default)]
    pub(crate) struct Recorder {
        pub(crate) ops: Vec<Op>,
        stack: Vec<Affine>,
    }

    impl Recorder {
        /// Depth of the transform stack.
        pub(crate) fn depth(&self) -> usize {
            self.stack.len()
        }

        fn current(&self) -> Affine {
            self.stack.last().copied().unwrap_or(Affine::IDENTITY)
        }
    }

    impl DrawTarget for Recorder {
        fn fill<'b>(
            &mut self,
            _style: Fill,
            transform: Affine,
            _brush: impl Into<BrushRef<'b>>,
            _brush_transform: Option<Affine>,
            shape: &impl Shape,
        ) {
            let transform = self.current() * transform;
            let bbox = shape.bounding_box();
            self.ops.push(Op::Fill { transform, bbox });
        }

        fn stroke<'b>(
            &mut self,
            _style: &Stroke,
            transform: Affine,
            _brush: impl Into<BrushRef<'b>>,
            _brush_transform: Option<Affine>,
            shape: &impl Shape,
        ) {
            let transform = self.current() * transform;
            let bbox = shape.bounding_box();
            self.ops.push(Op::Stroke { transform, bbox });
        }

        fn push_layer(
            &mut self,
            _clip_style: Fill,
            alpha: f32,
            transform: Affine,
            _clip: &impl Shape,
        ) {
            let transform = self.current() * transform;
            self.ops.push(Op::PushLayer { alpha, transform });
        }

        fn push_clip_layer(&mut self, _clip_style: Fill, transform: Affine, _clip: &impl Shape) {
            let transform = self.current() * transform;
            self.ops.push(Op::PushClip { transform });
        }

        fn pop_layer(&mut self) {
            self.ops.push(Op::PopLayer);
        }

        fn draw_image(&mut self, _image: &ImageData, transform: Affine) {
            let transform = self.current() * transform;
            self.ops.push(Op::Image { transform });
        }

        fn draw_blurred_rounded_rect(
            &mut self,
            transform: Affine,
            _rect: Rect,
            _color: Color,
            _radius: f64,
            _std_dev: f64,
        ) {
            let transform = self.current() * transform;
            self.ops.push(Op::Shadow { transform });
        }

        fn draw_glyphs(&mut self, run: &GlyphRun<'_>, glyphs: impl Iterator<Item = Glyph>) {
            self.ops.push(Op::Glyphs {
                transform: self.current() * run.transform,
                glyph_transform: run.glyph_transform,
                coords: run.normalized_coords.to_vec(),
                count: glyphs.count(),
            });
        }

        fn push_transform(&mut self, transform: Affine) {
            let t = self.current() * transform;
            self.stack.push(t);
        }

        fn pop_transform(&mut self) {
            self.stack.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::recorder::{Op, Recorder};
    use super::*;
    use crate::{
        Color, Fonts, Frame, FrameState, ShadowToken, Theme, build_frame, col, div, image_rgba8,
        raw_input, rich_text, span,
    };

    #[test]
    fn transform_stack_composes_outer_first() {
        let mut scene = vello::Scene::new();
        let mut t = SceneTarget::new(&mut scene);
        let a = Affine::translate((10.0, 0.0));
        let b = Affine::scale(2.0);
        t.push_transform(a);
        t.push_transform(b);
        assert_eq!(t.current_transform(), a * b);
        t.pop_transform();
        assert_eq!(t.current_transform(), a);
        t.pop_transform();
        assert_eq!(t.current_transform(), Affine::IDENTITY);
        t.finish();
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn finish_panics_on_an_unpopped_transform() {
        let mut scene = vello::Scene::new();
        let mut t = SceneTarget::new(&mut scene);
        t.push_transform(Affine::scale(2.0));
        t.finish();
    }

    #[test]
    fn recorder_composes_like_scene_target() {
        let mut r = Recorder::default();
        r.push_transform(Affine::translate((5.0, 5.0)));
        r.fill(
            peniko::Fill::NonZero,
            Affine::scale(2.0),
            peniko::Color::BLACK,
            None,
            &kurbo::Rect::new(0.0, 0.0, 1.0, 1.0),
        );
        r.pop_transform();
        assert_eq!(r.depth(), 0);
        assert!(
            matches!(r.ops[0], Op::Fill { transform, .. } if transform == Affine::translate((5.0, 5.0)) * Affine::scale(2.0))
        );
    }

    /// Rotation of an affine's linear part, in degrees (unaffected by a positive scale).
    fn angle_deg(a: Affine) -> f64 {
        let c = a.as_coeffs();
        c[1].atan2(c[0]).to_degrees()
    }

    /// A container rotated 30° holding a shadowed half-opacity card, an image,
    /// synthesized italic text, and an input field.
    fn rotated_fixture() -> (Frame, Fonts, FrameState) {
        rotated_fixture_with(|_, _| {})
    }

    /// [`rotated_fixture`], built a second time after `setup` (e.g. focus)
    /// has run on the first build.
    fn rotated_fixture_with(setup: impl Fn(&Frame, &mut FrameState)) -> (Frame, Fonts, FrameState) {
        let root = col::<()>().w(300.0).h(400.0).rotate(30.0).children([
            col::<()>()
                .w(200.0)
                .h(80.0)
                .bg(Color::WHITE)
                .rounded(12.0)
                .shadow(ShadowToken::Md)
                .opacity(0.5),
            image_rgba8(4, 4, vec![255; 64]).w(40.0).h(40.0),
            rich_text([span("italic").italic()]),
            raw_input("hello", "").id("field").w(200.0).h(32.0),
        ]);
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        let mut build = |state: &mut FrameState| {
            build_frame(
                &root,
                &Theme::light(),
                &mut fonts,
                state,
                (400.0, 500.0),
                1.0,
            )
        };
        let first = build(&mut state);
        setup(&first, &mut state);
        let frame = build(&mut state);
        (frame, fonts, state)
    }

    #[test]
    fn transform_wraps_every_op_in_a_transformed_subtree() {
        let (frame, mut fonts, mut state) = rotated_fixture();
        let mut r = Recorder::default();
        frame.paint_into(&mut r, &mut fonts, &mut state);
        for op in &r.ops {
            if let Some(t) = op.transform() {
                assert!(
                    (angle_deg(t) - 30.0).abs() < 1e-3,
                    "{op:?} not rotated with its node"
                );
            }
        }
        assert!(r.ops.iter().any(|o| matches!(o, Op::Image { .. })));
        assert!(r.ops.iter().any(|o| matches!(o, Op::Shadow { .. })));
        assert!(
            r.ops
                .iter()
                .any(|o| matches!(o, Op::PushLayer { alpha, .. } if (*alpha - 0.5).abs() < 1e-6))
        );
        assert!(r.ops.iter().any(|o| matches!(o, Op::PushClip { .. })));
    }

    #[test]
    fn transform_stack_is_balanced_after_a_frame() {
        let (frame, mut fonts, mut state) = rotated_fixture();
        let mut r = Recorder::default();
        frame.paint_into(&mut r, &mut fonts, &mut state);
        assert_eq!(r.depth(), 0);
        let pushes = r
            .ops
            .iter()
            .filter(|o| matches!(o, Op::PushLayer { .. } | Op::PushClip { .. }))
            .count();
        let pops = r.ops.iter().filter(|o| matches!(o, Op::PopLayer)).count();
        assert_eq!(pushes, pops);
    }

    #[test]
    fn glyph_runs_carry_skew_and_coords() {
        let (frame, mut fonts, mut state) = rotated_fixture();
        let mut r = Recorder::default();
        frame.paint_into(&mut r, &mut fonts, &mut state);
        // Fonts::embedded() has no italic face, so .italic() is synthesized as a skew.
        assert!(r.ops.iter().any(|o| matches!(
            o,
            Op::Glyphs {
                glyph_transform: Some(_),
                ..
            }
        )));
        assert!(
            r.ops
                .iter()
                .all(|o| !matches!(o, Op::Glyphs { count: 0, .. }))
        );
    }

    #[test]
    fn exit_ghost_scales_inside_its_fade_layer() {
        // tests/motion.rs `exit_view`, with the ghost also scaling to 0.5.
        let view = |present: bool| {
            let mut kids = vec![div::<()>().id("keep").w(100.0).h(20.0).shrink0()];
            if present {
                kids.push(
                    div::<()>()
                        .id("gone")
                        .w(100.0)
                        .h(20.0)
                        .shrink0()
                        .bg(Color::BLACK)
                        .exit_to(0.0, 0.5, 0.0, 0.0),
                );
            }
            col::<()>().w(200.0).h(400.0).children(kids)
        };
        let theme = Theme::light();
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        let _ = build_frame(
            &view(true),
            &theme,
            &mut fonts,
            &mut state,
            (400.0, 500.0),
            1.0,
        );
        let f2 = build_frame(
            &view(false),
            &theme,
            &mut fonts,
            &mut state,
            (400.0, 500.0),
            1.0,
        );
        state.tick(0.05); // mid-exit
        let mut r = Recorder::default();
        f2.paint_into(&mut r, &mut fonts, &mut state);
        let layer = r
            .ops
            .iter()
            .position(|o| matches!(o, Op::PushLayer { alpha, .. } if *alpha < 1.0))
            .expect("the ghost fades in a layer");
        // The fade layer clips in screen space: it is pushed before the
        // ghost's scale, so the scale must not reach its clip.
        assert_eq!(r.ops[layer].transform(), Some(Affine::IDENTITY));
        let scaled = r.ops[layer + 1..]
            .iter()
            .find_map(Op::transform)
            .expect("the ghost paints inside it");
        assert!(
            scaled.as_coeffs()[0] < 1.0 && scaled.as_coeffs()[0] > 0.5,
            "mid-exit scale between 1 and 0.5"
        );
        assert_eq!(r.depth(), 0);
    }

    /// The focused field's caret paints inside the field's clip and turns
    /// with its rotated container, like the rest of the field.
    #[test]
    fn a_focused_fields_caret_turns_with_its_node() {
        let (frame, mut fonts, mut state) = rotated_fixture();
        let mut unfocused = Recorder::default();
        frame.paint_into(&mut unfocused, &mut fonts, &mut state);
        let (frame, mut fonts, mut state) = rotated_fixture_with(|frame, state| {
            state.set_focus(Some(frame.get(&crate::by::id("field")).id));
        });
        let mut focused = Recorder::default();
        frame.paint_into(&mut focused, &mut fonts, &mut state);
        let fills = |r: &Recorder| {
            r.ops
                .iter()
                .filter(|o| matches!(o, Op::Fill { .. }))
                .count()
        };
        assert!(fills(&focused) > fills(&unfocused), "the caret paints");
        for op in &focused.ops {
            if let Some(t) = op.transform() {
                assert!((angle_deg(t) - 30.0).abs() < 1e-3, "{op:?} not rotated");
            }
        }
        assert!(state.ime_caret().is_some());
    }

    /// A transformed node that is `display: none` paints nothing and leaves
    /// no transform behind for its siblings.
    #[test]
    fn a_hidden_transformed_node_leaves_its_siblings_untransformed() {
        let mut hidden = div::<()>().w(50.0).h(50.0).bg(Color::BLACK).rotate(30.0);
        hidden.style.display = crate::style::Display::None;
        let root = col::<()>()
            .w(200.0)
            .h(200.0)
            .children([hidden, div::<()>().w(50.0).h(50.0).bg(Color::WHITE)]);
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        let frame = build_frame(
            &root,
            &Theme::light(),
            &mut fonts,
            &mut state,
            (200.0, 200.0),
            1.0,
        );
        let mut r = Recorder::default();
        frame.paint_into(&mut r, &mut fonts, &mut state);
        assert_eq!(r.depth(), 0);
        let fills: Vec<_> = r
            .ops
            .iter()
            .filter(|o| matches!(o, Op::Fill { .. }))
            .collect();
        assert_eq!(fills.len(), 1, "only the visible sibling paints");
        assert_eq!(angle_deg(fills[0].transform().unwrap()), 0.0);
    }
}
