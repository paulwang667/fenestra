//! Shapes to triangles: kurbo paths tessellated by lyon in the shape's own
//! space, each vertex mapped to the device through the draw's transform
//! (so a gradient or shadow can still be evaluated in local coordinates).

use bytemuck::{Pod, Zeroable};
use kurbo::{Affine, PathEl, Point, Shape, Stroke};
use lyon_tessellation::path::Path;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, LineCap, LineJoin,
    StrokeOptions, StrokeTessellator, StrokeVertex, TessellationError, VertexBuffers,
};
use peniko::Fill;

/// A vertex of a filled or stroked shape.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub(crate) struct PaintV {
    /// Device position.
    pub pos: [f32; 2],
    /// Position in the shape's own space, before the transform.
    pub local: [f32; 2],
    /// Premultiplied color (solid brushes) or tint.
    pub color: [f32; 4],
    /// Paint geometry (see `paint.rs`).
    pub grad: [f32; 4],
    /// Paint kind, ramp row, extend.
    pub grad2: [f32; 4],
}

/// The per-shape part of a [`PaintV`]: everything but the two positions.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PaintParams {
    pub color: [f32; 4],
    pub grad: [f32; 4],
    pub grad2: [f32; 4],
}

/// Triangles for every paint vertex of a frame.
pub(crate) type Mesh = VertexBuffers<PaintV, u32>;

/// Flattening tolerance in the shape's units: a quarter device pixel under
/// `transform`.
pub(crate) fn tolerance(transform: Affine) -> f32 {
    let scale = transform.determinant().abs().sqrt();
    #[expect(clippy::cast_possible_truncation, reason = "a tolerance fits in f32")]
    let tol = (0.25 / scale.max(1e-6)) as f32;
    tol.clamp(1e-3, 16.0)
}

fn point(p: Point) -> lyon_tessellation::math::Point {
    #[expect(clippy::cast_possible_truncation, reason = "coordinates fit in f32")]
    lyon_tessellation::math::point(p.x as f32, p.y as f32)
}

/// A lyon path from kurbo path elements; open subpaths stay open.
pub(crate) fn lyon_path(elements: impl Iterator<Item = PathEl>) -> Path {
    let mut b = Path::builder();
    let mut open = false;
    for el in elements {
        match el {
            PathEl::MoveTo(a) => {
                if open {
                    b.end(false);
                }
                b.begin(point(a));
                open = true;
            }
            PathEl::LineTo(a) if open => {
                b.line_to(point(a));
            }
            PathEl::QuadTo(a, c) if open => {
                b.quadratic_bezier_to(point(a), point(c));
            }
            PathEl::CurveTo(a, c, d) if open => {
                b.cubic_bezier_to(point(a), point(c), point(d));
            }
            PathEl::ClosePath if open => {
                b.end(true);
                open = false;
            }
            // A segment with no current point: nothing to draw from.
            _ => {}
        }
    }
    if open {
        b.end(false);
    }
    b.build()
}

fn vertex(transform: Affine, params: PaintParams, local: lyon_tessellation::math::Point) -> PaintV {
    let device = transform * Point::new(f64::from(local.x), f64::from(local.y));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "device coordinates fit in f32"
    )]
    let pos = [device.x as f32, device.y as f32];
    PaintV {
        pos,
        local: [local.x, local.y],
        color: params.color,
        grad: params.grad,
        grad2: params.grad2,
    }
}

/// Appends the triangles filling `shape` to `mesh`; on a tessellation error
/// the mesh is left as it was.
pub(crate) fn fill(
    tess: &mut FillTessellator,
    shape: &impl Shape,
    rule: Fill,
    transform: Affine,
    params: PaintParams,
    mesh: &mut Mesh,
) -> Result<(), TessellationError> {
    let tol = tolerance(transform);
    let path = lyon_path(shape.path_elements(f64::from(tol)));
    let rule = match rule {
        Fill::NonZero => FillRule::NonZero,
        Fill::EvenOdd => FillRule::EvenOdd,
    };
    let options = FillOptions::tolerance(tol).with_fill_rule(rule);
    let mark = (mesh.vertices.len(), mesh.indices.len());
    let result = tess.tessellate_path(
        &path,
        &options,
        &mut BuffersBuilder::new(mesh, |v: FillVertex| {
            vertex(transform, params, v.position())
        }),
    );
    if result.is_err() {
        mesh.vertices.truncate(mark.0);
        mesh.indices.truncate(mark.1);
    }
    result
}

/// Appends the triangles stroking `shape` with `style` (dashes expanded
/// first) to `mesh`; on a tessellation error the mesh is left as it was.
pub(crate) fn stroke(
    tess: &mut StrokeTessellator,
    style: &Stroke,
    shape: &impl Shape,
    transform: Affine,
    params: PaintParams,
    mesh: &mut Mesh,
) -> Result<(), TessellationError> {
    // Also skips a NaN width.
    if style.width.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Ok(());
    }
    let tol = tolerance(transform);
    let elements = shape.path_elements(f64::from(tol));
    let path = if style.dash_pattern.is_empty() {
        lyon_path(elements)
    } else {
        lyon_path(kurbo::dash(
            elements,
            style.dash_offset,
            &style.dash_pattern,
        ))
    };
    let cap = |c: kurbo::Cap| match c {
        kurbo::Cap::Butt => LineCap::Butt,
        kurbo::Cap::Square => LineCap::Square,
        kurbo::Cap::Round => LineCap::Round,
    };
    let join = match style.join {
        kurbo::Join::Bevel => LineJoin::Bevel,
        kurbo::Join::Miter => LineJoin::Miter,
        kurbo::Join::Round => LineJoin::Round,
    };
    #[expect(clippy::cast_possible_truncation, reason = "stroke widths fit in f32")]
    let options = StrokeOptions::tolerance(tol)
        .with_line_width(style.width as f32)
        .with_start_cap(cap(style.start_cap))
        .with_end_cap(cap(style.end_cap))
        .with_line_join(join)
        .with_miter_limit((style.miter_limit as f32).max(StrokeOptions::MINIMUM_MITER_LIMIT));
    let mark = (mesh.vertices.len(), mesh.indices.len());
    let result = tess.tessellate_path(
        &path,
        &options,
        &mut BuffersBuilder::new(mesh, |v: StrokeVertex| {
            vertex(transform, params, v.position())
        }),
    );
    if result.is_err() {
        mesh.vertices.truncate(mark.0);
        mesh.indices.truncate(mark.1);
    }
    result
}
