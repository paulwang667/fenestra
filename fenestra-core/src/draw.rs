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
    #[allow(
        dead_code,
        reason = "fields are read selectively by tests and in Debug output"
    )]
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
    use super::*;

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
        let mut r = recorder::Recorder::default();
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
            matches!(r.ops[0], recorder::Op::Fill { transform, .. } if transform == Affine::translate((5.0, 5.0)) * Affine::scale(2.0))
        );
    }
}
