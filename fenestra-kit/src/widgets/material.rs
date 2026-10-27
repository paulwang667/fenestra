//! Surfaces that keep their own look under the solid material style and take
//! their role's material in the others.

use fenestra_core::{MaterialStyle, Style, Surface, Theme};

/// `s` dressed as `role` in the theme's material style — or, under
/// [`MaterialStyle::Solid`], by `solid`: the widget's own look from before
/// material styles, so the stock kit renders exactly as it did.
pub(crate) fn role_or_own(
    t: &Theme,
    role: Surface,
    s: Style,
    solid: impl FnOnce(Style) -> Style,
) -> Style {
    if t.material_style == MaterialStyle::Solid {
        solid(s)
    } else {
        t.apply_surface(role, s)
    }
}
