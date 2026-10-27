//! Surface / Material bundle: one typed primitive per elevation role.
//!
//! A [`Surface`] is a semantic *material* (Geist/Apple "materials"): it bundles
//! a corner radius, a fill role, a border role, a shadow token, and an optional
//! top highlight into a single value that resolves against a [`Theme`] into a
//! [`Style`] overlay. The kit's elevated surfaces — cards, popovers, menus,
//! modals, glass panes, the slider thumb, tooltips — all derive their look from
//! this one table, so "every floating thing matches" is a structural property
//! of the bundle, not a convention re-typed at each call site.

use crate::style::{AdaptiveTint, Border, CornerRadius, Paint, Sheen, SpecularEdge, Style};
use crate::theme::Theme;
use crate::tokens::{Elevation, R_FULL, R_LG, R_SM, R_XL, RadiusScale, ShadowToken};

/// One of the kit's elevation *materials*: a semantic role that bundles a
/// corner radius, a fill role, a border role, a shadow token, and an optional
/// top highlight into one typed primitive, resolved against a [`Theme`] into a
/// [`Style`] overlay. Floating roles (`Popover`/`Menu`/`Modal`/`Glass`) carry
/// radii `>=` and shadow depth `>=` the resting roles (`Card`/`Raised`), so
/// "every floating thing matches" is structural, not a convention re-typed at
/// each call site. `Thumb` (a control handle) and `Tooltip` (an inverted chip)
/// are deliberately exempt from that ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Surface {
    /// A resting card: raised surface, subtle border, large radius, a small
    /// shadow.
    Card,
    /// A resting panel with the card's surface and radius but no shadow — a
    /// flat grouping that still reads as a distinct surface.
    Raised,
    /// A floating popover anchored to the page: elevated fill, deep shadow.
    Popover,
    /// A floating menu/listbox: same recipe as [`Surface::Popover`].
    Menu,
    /// A floating modal dialog: the deepest, roundest surface.
    Modal,
    /// A control handle (slider thumb): a pill on a raised surface with a firm
    /// border and a small shadow. Exempt from the elevation ordering.
    Thumb,
    /// An inverted high-contrast chip (tooltip): the neutral ramp's darkest
    /// step, a tight radius, no border. Exempt from the elevation ordering.
    Tooltip,
    /// A frosted-glass floating panel (command palette / glass popover): an
    /// elevated fill rendered as a translucent vibrancy [`Material`] so content
    /// behind shows through, with a deep shadow, a hairline edge, and a 1px top
    /// highlight. Floating (ranks with [`Popover`](Surface::Popover)).
    Glass,
    /// A bar content runs under or beside — a top app bar, a header, a bottom
    /// navigation bar. Solid: the page surface with a hairline; the glass
    /// styles frost it.
    Bar,
    /// A floating dock of controls — a composer, a floating tab bar. Solid:
    /// as [`Bar`](Surface::Bar).
    Dock,
    /// The window's base: the page colour (Acrylic's Mica: tinted by the
    /// accent, grained).
    Window,
}

/// A named material style: how every [`Surface`] role is rendered —
/// material, edge, radius, shadow — set on the [`Theme`]
/// ([`Theme::with_material_style`]).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MaterialStyle {
    /// Solid surfaces: the stock look (glass only on [`Surface::Glass`]).
    #[default]
    Solid,
    /// Apple's Liquid Glass: thin, vivid glass on what floats over content —
    /// bars, docks, popovers, menus, modals — over solid content.
    LiquidGlass,
    /// Fluent's Acrylic over Mica: thick, grained in-app glass on bars and
    /// transient surfaces, over an accent-tinted, grained window base.
    Acrylic,
    /// Material 3's tonal surfaces: no glass — the accent mixed into the
    /// surface by elevation, shadows only on what floats.
    Tonal,
}

impl MaterialStyle {
    /// Whether this is the default, [`MaterialStyle::Solid`] (a spec leaves
    /// the default out when it is written).
    pub fn is_solid(&self) -> bool {
        *self == Self::Solid
    }

    /// The four styles.
    pub const ALL: [Self; 4] = [Self::Solid, Self::LiquidGlass, Self::Acrylic, Self::Tonal];

    /// Whether bars float over content, which runs on under them (the glass
    /// styles).
    #[must_use]
    pub const fn bars_overlay_content(self) -> bool {
        matches!(self, Self::LiquidGlass | Self::Acrylic)
    }

    /// Whether the bottom bar floats as a dock clear of the screen's edges
    /// (Liquid Glass).
    #[must_use]
    pub const fn floating_dock(self) -> bool {
        matches!(self, Self::LiquidGlass)
    }
}

/// The corner-radius shape of a surface bundle: one `Uniform` radius on every
/// corner. A nested rounded child derives its concentric radius from this via
/// [`SurfaceRadius::inner`] (passing the padding between them). The enum is
/// `#[non_exhaustive]` to leave room for future shapes (e.g. per-corner radii).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum SurfaceRadius {
    /// The same radius on every corner.
    Uniform(f32),
}

impl SurfaceRadius {
    /// The outer corner radius in logical px.
    pub const fn outer(self) -> f32 {
        match self {
            SurfaceRadius::Uniform(r) => r,
        }
    }

    /// The concentric inner radius for a child inset by `inset` logical px on
    /// every side: `max(0, outer - inset)`. Nested rounded rectangles must
    /// share a center, so the child's radius is the parent's outer radius
    /// minus the padding between them — otherwise the inner corner visibly
    /// bulges away from the outer one. Use this for any rounded child of a
    /// rounded surface (menu items inside a menu panel, an inset thumbnail in
    /// a card) instead of hand-typing a second radius that can silently desync
    /// when the surface radius changes.
    pub fn inner(self, inset: f32) -> f32 {
        (self.outer() - inset).max(0.0)
    }
}

/// Which theme role a surface fill resolves to — a bundle is defined in
/// semantic roles, never literal colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SurfaceFill {
    /// [`Theme::surface_raised`] — resting cards and control surfaces.
    SurfaceRaised,
    /// [`Theme::elevated_surface`] at the given level — floating panels (lifts
    /// in dark mode).
    Elevated(u8),
    /// `Theme::neutrals.step(12)` — the inverted high-contrast chip (tooltips).
    Inverted,
    /// [`Theme::surface`] — bars and docks.
    Surface,
    /// [`Theme::bg`] — the page itself (the window).
    Page,
}

impl SurfaceFill {
    /// Resolves this fill role to a concrete color for `theme`.
    pub fn color(self, theme: &Theme) -> crate::Color {
        match self {
            SurfaceFill::SurfaceRaised => theme.surface_raised,
            SurfaceFill::Elevated(level) => theme.elevated_surface(level),
            SurfaceFill::Inverted => theme.neutrals.step(12),
            SurfaceFill::Surface => theme.surface,
            SurfaceFill::Page => theme.bg,
        }
    }
}

/// The border role of a surface bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceBorder {
    /// No border.
    None,
    /// 1px [`Theme::border_subtle`] — the hairline that pairs with elevation.
    Subtle,
    /// 1px [`Theme::border`] — a firmer edge for resting control surfaces
    /// (thumbs).
    Default,
}

/// A translucent "frosted glass" material (Apple "materials" / Linear &
/// Raycast command palettes): a pane that reads as floating glass over the
/// content behind it. Describes the three perceptual levers of glass — how much
/// shows through (`fill_alpha`), how strongly the content behind would be
/// blurred (`blur_radius`), and how much the wash-out is re-saturated
/// (`saturation`, "vibrancy"). Resolved against a [`Theme`] (never a raw color)
/// by [`Material::tint`], and carried by a [`SurfaceBundle`] via the
/// [`Surface::Glass`] role.
///
/// Renderer note: `blur_radius` now drives a real backdrop blur. vello 0.9
/// still exposes no GPU backdrop filter, so the shell renders glass in two
/// passes — it reads the scene back with the pane skipped and blurs the region
/// behind it on the CPU (a deterministic integer box blur), then composites the
/// frosted backdrop under the vibrancy tint. It is realized in headless
/// rendering (the golden source of truth); the single-pass live-window path
/// falls back to the translucent tint alone. See ARCHITECTURE.md
/// ("Real frosted-glass backdrop blur").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// Fill opacity `0.0..=1.0`: how much of the content behind shows through.
    /// Lower is glassier; higher protects legibility of text painted on the
    /// pane (especially important while the backdrop blur is unrendered).
    pub fill_alpha: f32,
    /// Backdrop blur radius in logical px applied to the content behind the
    /// pane. Resolved into [`Style::backdrop_blur`](crate::Style::backdrop_blur)
    /// by [`SurfaceBundle::apply`] and realized by the shell's two-pass renderer;
    /// see the type-level note.
    pub blur_radius: f32,
    /// Vibrancy: OKLCH chroma multiplier on the tint (`>= 1.0` re-saturates what
    /// a real backdrop blur washes out). `1.0` leaves the tint's chroma alone.
    pub saturation: f32,
}

impl Material {
    /// A material from explicit levers. `fill_alpha` is clamped to `0.0..=1.0`
    /// and `saturation` floored at `0.0` at resolution time (in [`tint`](Self::tint)),
    /// so this constructor is `const` and stores the raw levers verbatim.
    pub const fn new(fill_alpha: f32, blur_radius: f32, saturation: f32) -> Self {
        Self {
            fill_alpha,
            blur_radius,
            saturation,
        }
    }

    /// The popover / command-palette recipe: a frosted floating pane
    /// (`fill_alpha` 0.82, `blur_radius` 18, `saturation` 1.5). The defaults
    /// behind [`Surface::Glass`].
    pub const fn popover() -> Self {
        Self::new(0.82, 18.0, 1.5)
    }

    /// The material for a pane meant to read as glass.
    ///
    /// **Not the popover's, which it used to borrow.** A popover's job is to
    /// carry text legibly over anything and 0.82 fill is how it does that — so
    /// a glass pane inherited an opacity chosen to stop you seeing through it,
    /// and came out milky. Glass has the opposite job: the blur and the
    /// bevelled rim are what make it a material, and they only work if there
    /// is something visible behind them.
    pub const fn glass() -> Self {
        Self::new(0.58, 12.0, 1.5)
    }

    /// Resolves the translucent, vibrancy-tinted fill color for `base` (a solid
    /// surface-role color from the theme): keeps `base`'s OKLCH lightness and
    /// hue, multiplies its chroma by `saturation` (gamut-mapped via
    /// [`oklch`](crate::oklch), never clipped), then applies `fill_alpha`. Pure;
    /// derives entirely from the passed theme color (no raw literal).
    #[must_use]
    pub fn tint(self, base: crate::Color) -> crate::Color {
        let [l, c, h] = crate::oklch_of(base);
        let saturated = crate::theme::oklch(l, c * self.saturation.max(0.0), h);
        saturated.with_alpha(self.fill_alpha.clamp(0.0, 1.0))
    }
}

/// The role-resolved description of a [`Surface`]: radius, fill/border *roles*,
/// shadow token, optional top-highlight alpha, and an optional translucent
/// [`Material`]. [`Surface::bundle`] returns it (pure, no theme — so
/// radius/shadow/role ordering is unit-testable); [`SurfaceBundle::apply`] turns
/// it into a concrete [`Style`]. Public so apps and Looks can compose custom
/// materials from the fill/border roles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceBundle {
    /// Outer corner radius.
    pub radius: SurfaceRadius,
    /// Fill role.
    pub fill: SurfaceFill,
    /// Border role.
    pub border: SurfaceBorder,
    /// Shadow elevation token (`None` for a flat resting surface).
    pub shadow: Option<ShadowToken>,
    /// 1px top-highlight alpha (white sheen), or `None` for no highlight.
    pub highlight: Option<f32>,
    /// When set, [`SurfaceBundle::apply`] renders `fill` as this translucent
    /// frosted [`Material`] (its [`tint`](Material::tint) of the resolved fill
    /// color) instead of a solid. `None` for every opaque role.
    pub material: Option<Material>,
}

impl Surface {
    /// The role's bundle: the radius, fill/border roles, shadow token, and
    /// highlight that define this material. Pure and `const`.
    pub const fn bundle(self) -> SurfaceBundle {
        match self {
            // Resting roles: the card's surface and 14px radius.
            Surface::Card => SurfaceBundle {
                radius: SurfaceRadius::Uniform(R_LG),
                fill: SurfaceFill::SurfaceRaised,
                border: SurfaceBorder::Subtle,
                shadow: Some(ShadowToken::Sm),
                highlight: None,
                material: None,
            },
            Surface::Raised => SurfaceBundle {
                radius: SurfaceRadius::Uniform(R_LG),
                fill: SurfaceFill::SurfaceRaised,
                border: SurfaceBorder::Subtle,
                shadow: None,
                highlight: None,
                material: None,
            },
            // Floating roles: elevated fill, deep shadow, radius >= the card's.
            Surface::Popover | Surface::Menu => SurfaceBundle {
                radius: SurfaceRadius::Uniform(R_LG),
                fill: SurfaceFill::Elevated(2),
                border: SurfaceBorder::Subtle,
                shadow: Some(ShadowToken::Lg),
                highlight: None,
                material: None,
            },
            Surface::Modal => SurfaceBundle {
                radius: SurfaceRadius::Uniform(R_XL),
                fill: SurfaceFill::Elevated(2),
                border: SurfaceBorder::Subtle,
                shadow: Some(ShadowToken::Xl),
                highlight: None,
                material: None,
            },
            // A frosted floating pane: the elevated fill it is a translucent
            // vibrancy tint OF, plus a deep shadow, a hairline edge, and a 1px
            // top sheen. The first shipped role to set a highlight or material.
            Surface::Glass => SurfaceBundle {
                radius: SurfaceRadius::Uniform(R_LG),
                fill: SurfaceFill::Elevated(2),
                border: SurfaceBorder::Subtle,
                shadow: Some(ShadowToken::Lg),
                // **Nearly nothing, and that was the correction.** I took this
                // to 0.24 on the theory that a sharper, brighter lip reads as
                // a cut edge rather than a rounded-over one. Held against the
                // chrome this is measured by — a floating macOS toolbar — the
                // premise was wrong: that surface has *no* visible lip at all,
                // and is told from the content under it by a small step of
                // fill and a hairline. Refinement here is subtraction. What is
                // left keeps a translucent pane from vanishing into a dark
                // backdrop entirely.
                highlight: Some(0.08),
                material: Some(Material::glass()),
            },
            // Exempt materials: a pill control handle and an inverted chip.
            Surface::Thumb => SurfaceBundle {
                radius: SurfaceRadius::Uniform(R_FULL),
                fill: SurfaceFill::SurfaceRaised,
                border: SurfaceBorder::Default,
                shadow: Some(ShadowToken::Sm),
                highlight: None,
                material: None,
            },
            Surface::Tooltip => SurfaceBundle {
                radius: SurfaceRadius::Uniform(R_SM),
                fill: SurfaceFill::Inverted,
                border: SurfaceBorder::None,
                shadow: Some(ShadowToken::Md),
                highlight: None,
                material: None,
            },
            // Bars and docks rest on the page surface with a hairline.
            Surface::Bar | Surface::Dock => SurfaceBundle {
                radius: SurfaceRadius::Uniform(0.0),
                fill: SurfaceFill::Surface,
                border: SurfaceBorder::Subtle,
                shadow: None,
                highlight: None,
                material: None,
            },
            Surface::Window => SurfaceBundle {
                radius: SurfaceRadius::Uniform(0.0),
                fill: SurfaceFill::Page,
                border: SurfaceBorder::None,
                shadow: None,
                highlight: None,
                material: None,
            },
        }
    }

    /// The corner radius for this role resolved against a theme's radius
    /// scale, so a [`RadiusScale::sharp`] theme un-rounds every surface at
    /// once. Pills (`Thumb`) stay fully round. With the default scale this
    /// returns exactly the role's stock radius (`R_LG` / `R_XL` / `R_SM`).
    #[must_use]
    pub fn radius_px(self, radius: &RadiusScale) -> f32 {
        match self {
            Surface::Modal => radius.xl,
            Surface::Thumb => R_FULL,
            Surface::Tooltip => radius.sm,
            Surface::Card | Surface::Raised | Surface::Popover | Surface::Menu | Surface::Glass => {
                radius.lg
            }
            Surface::Bar | Surface::Dock | Surface::Window => 0.0,
        }
    }

    /// Whether this role floats above the page
    /// (`Popover`/`Menu`/`Modal`/`Glass`). `Card` and `Raised` rest in flow;
    /// `Thumb` and `Tooltip` are exempt control/chip materials (both report
    /// `false`).
    pub const fn is_floating(self) -> bool {
        matches!(
            self,
            Surface::Popover | Surface::Menu | Surface::Modal | Surface::Glass
        )
    }
}

impl SurfaceBundle {
    /// Overlays this bundle onto `base`, resolving fill/border against `theme`:
    /// sets corner radius, fill, border, shadow token, and top highlight,
    /// leaving every other field of `base` (layout, text) untouched. The fill is
    /// a solid role color, or — when [`material`](SurfaceBundle::material) is set
    /// — that role color run through [`Material::tint`] into a translucent
    /// frosted paint. The highlight, when set, is a low-alpha pure white from
    /// `oklch(1, 0, 0)` — no raw literal. The shadow token expands to layers
    /// later, during frame resolution, exactly as a hand-set `.shadow(..)` would.
    #[must_use]
    pub fn apply(self, theme: &Theme, base: Style) -> Style {
        let mut style = base;
        style.corner_radius = CornerRadius::all(self.radius.outer());
        // A material renders the resolved fill role as its translucent
        // vibrancy tint; without one, the solid role color is used verbatim
        // (byte-identical to every pre-0.22 role).
        let fill_color = self.fill.color(theme);
        let resolved = match self.material {
            Some(m) => m.tint(fill_color),
            None => fill_color,
        };
        style.fill = Some(Paint::Solid(resolved));
        style.border = match self.border {
            SurfaceBorder::None => None,
            SurfaceBorder::Subtle => Some(Border {
                width: 1.0,
                color: theme.border_subtle,
            }),
            SurfaceBorder::Default => Some(Border {
                width: 1.0,
                color: theme.border,
            }),
        };
        style.shadow_token = self.shadow;
        style.highlight_top = self
            .highlight
            .map(|a| crate::theme::oklch(1.0, 0.0, 0.0).with_alpha(a));
        // A material's blur_radius is no longer reserved: it drives the
        // backdrop-blur pass (the shell reads back the content behind the pane
        // and blurs it). A zero radius leaves `backdrop_blur` None, so a flat-tint
        // material skips the two-pass blur (it still gets the rim + sheen below);
        // an opaque role with no material renders exactly as before.
        style.backdrop_blur = self.material.map(|m| m.blur_radius).filter(|r| *r > 0.0);
        if let Some(m) = self.material {
            style.backdrop_saturate = m.saturation.max(0.0);
        }
        // A frosted material also carries the Liquid Glass edge optics — a
        // luminous specular rim, a directional body sheen, and a backdrop-adaptive
        // vibrancy shift — so the pane reads as lit, lensed glass that picks up the
        // room behind it rather than a flat frosted sticker. Only roles with a
        // material (currently `Surface::Glass`) get them; opaque roles stay
        // byte-identical.
        if self.material.is_some() {
            style.specular_edge = Some(SpecularEdge::glass());
            style.sheen = Some(Sheen::glass());
            style.adaptive_tint = Some(AdaptiveTint::glass());
        }
        style
    }
}

impl Theme {
    /// A fresh [`Style`] carrying `role`'s material (radius + fill + border +
    /// shadow + highlight) resolved for this theme. The theme-in-scope entry
    /// point; [`Element::surface`](crate::Element::surface) is the deferred one
    /// for `view()`.
    pub fn surface_style(&self, role: Surface) -> Style {
        self.apply_surface(role, Style::default())
    }

    /// A label colour for text on `role` at `level`: Liquid Glass's vibrant
    /// labels on its glass roles (the backdrop's colour carries into them),
    /// the theme's text colours everywhere else — other styles, content
    /// roles, and any style with [`Theme::reduce_transparency`].
    #[must_use]
    pub fn label_on(&self, role: Surface, level: crate::Vibrancy) -> crate::Color {
        let vibrant = self.material_style == MaterialStyle::LiquidGlass
            && self.surface_style(role).backdrop_blur.is_some();
        match (vibrant, level) {
            (true, level) => self.vibrant_label(level),
            (false, crate::Vibrancy::Primary) => self.text,
            (false, crate::Vibrancy::Secondary) => self.text_muted,
            (false, crate::Vibrancy::Tertiary) => self.text_subtle,
        }
    }

    /// Overlays `role`'s material in this theme's [`MaterialStyle`] onto
    /// `base` (its layout and text untouched) — the one place every surface
    /// resolves, for [`Theme::surface_style`] and
    /// [`Element::surface`](crate::Element::surface) alike.
    #[must_use]
    pub fn apply_surface(&self, role: Surface, base: Style) -> Style {
        let stock = role
            .bundle()
            .apply(self, base)
            .rounded(role.radius_px(&self.radius));
        let mut style = match self.material_style {
            MaterialStyle::Solid => stock,
            MaterialStyle::LiquidGlass => self.liquid_glass(role, stock),
            MaterialStyle::Acrylic => self.acrylic(role, stock),
            MaterialStyle::Tonal => self.tonal(role, stock),
        };
        // Flat elevation: resting cards lean on their border + tone-step rather
        // than a shadow; floating roles always keep theirs.
        if self.elevation == Elevation::Flat && matches!(role, Surface::Card | Surface::Raised) {
            style.shadow_token = None;
        }
        if self.reduce_transparency {
            style = self.opaque(role, style);
        }
        style
    }

    /// Liquid Glass: thin, vivid glass with a bright rim and an adaptive
    /// tint on what floats over content; content roles stay solid.
    fn liquid_glass(&self, role: Surface, stock: Style) -> Style {
        // (fill alpha, blur, shadow, radius)
        let (alpha, blur, shadow, radius) = match role {
            Surface::Bar => (0.46, 22.0, None, 0.0),
            Surface::Dock => (0.54, 20.0, Some(ShadowToken::Lg), R_FULL),
            Surface::Popover | Surface::Menu => (0.64, 20.0, Some(ShadowToken::Lg), self.radius.xl),
            Surface::Glass => (0.5, 16.0, Some(ShadowToken::Lg), self.radius.xl),
            Surface::Modal => (0.72, 20.0, Some(ShadowToken::Xl), self.radius.xl),
            _ => return stock,
        };
        let mut s = glass(
            stock,
            role.bundle().fill.color(self),
            Material::new(alpha, blur, LIQUID_SATURATION),
        );
        s.specular_edge = Some(SpecularEdge {
            intensity: 0.5,
            shade: 0.14,
            ..SpecularEdge::glass()
        });
        s.adaptive_tint = Some(AdaptiveTint::glass());
        // A raking sheen suits a floating card, not a bar or a menu.
        s.sheen = (role == Surface::Glass).then(Sheen::glass);
        s.highlight_top = None;
        if matches!(role, Surface::Bar | Surface::Dock) {
            s.border = None;
        }
        s.shadow_token = shadow;
        s.rounded(radius)
    }

    /// Acrylic over Mica: thick, slightly desaturated, grained glass on bars
    /// and transient surfaces; modals opaque; the window an accent-tinted,
    /// grained Mica.
    fn acrylic(&self, role: Surface, stock: Style) -> Style {
        match role {
            Surface::Bar | Surface::Dock | Surface::Popover | Surface::Menu | Surface::Glass => {
                let mut s = glass(
                    stock,
                    role.bundle().fill.color(self),
                    Material::new(0.78, 30.0, ACRYLIC_SATURATION),
                );
                s.glass_noise = 0.02;
                s.specular_edge = None;
                s.sheen = None;
                s.adaptive_tint = None;
                s.highlight_top = None;
                if role == Surface::Bar {
                    s.border = None;
                    s.shadow_token = None;
                    s
                } else {
                    s.border = Some(Border {
                        width: 1.0,
                        color: self.border_subtle,
                    });
                    s.shadow_token = Some(ShadowToken::Sm);
                    s.rounded(self.radius.sm)
                }
            }
            Surface::Window => {
                let mut s = stock;
                s.fill = Some(Paint::Solid(mix(self.bg, self.accent, 0.05)));
                s.surface_noise = 0.02;
                s
            }
            _ => stock,
        }
    }

    /// Tonal (Material 3): no glass — the accent mixed into the surface by
    /// elevation, no borders, shadows only on what floats.
    fn tonal(&self, role: Surface, stock: Style) -> Style {
        // (accent share, shadow, radius)
        let (share, shadow, radius) = match role {
            Surface::Card => (0.05, Some(ShadowToken::Sm), self.radius.lg),
            Surface::Raised => (0.05, None, self.radius.lg),
            Surface::Bar => (0.08, None, 0.0),
            Surface::Dock => (0.11, None, R_FULL),
            Surface::Popover | Surface::Menu => (0.12, Some(ShadowToken::Md), self.radius.lg),
            Surface::Glass => (0.14, Some(ShadowToken::Md), self.radius.xl),
            Surface::Modal => (0.14, Some(ShadowToken::Xl), self.radius.xl),
            Surface::Window | Surface::Thumb | Surface::Tooltip => return stock,
        };
        let mut s = clear_glass(stock);
        s.fill = Some(Paint::Solid(mix(self.surface, self.accent, share)));
        s.border = None;
        s.shadow_token = shadow;
        s.highlight_top = None;
        s.rounded(radius)
    }

    /// `style` with any glass resolved opaque: the role's own fill colour,
    /// no blur, vibrancy, grain or optics — radius, border and shadow kept.
    fn opaque(&self, role: Surface, style: Style) -> Style {
        let translucent = match style.fill {
            Some(Paint::Solid(c)) => c.components[3] < 1.0,
            _ => false,
        };
        if style.backdrop_blur.is_none() && !translucent {
            return style;
        }
        let mut s = clear_glass(style);
        s.fill = Some(Paint::Solid(role.bundle().fill.color(self)));
        s
    }
}

/// Liquid Glass lifts the colour a blur washes out.
const LIQUID_SATURATION: f32 = 1.7;

/// Acrylic's luminosity layer keeps the backdrop's colour in check.
const ACRYLIC_SATURATION: f32 = 0.95;

/// `style` frosted with `material` over the content behind it, its fill the
/// material's tint of `color`.
fn glass(mut style: Style, color: crate::Color, material: Material) -> Style {
    style.fill = Some(Paint::Solid(material.tint(color)));
    style.backdrop_blur = Some(material.blur_radius);
    style.backdrop_saturate = material.saturation;
    style
}

/// `style` with every glass property cleared.
fn clear_glass(mut style: Style) -> Style {
    style.backdrop_blur = None;
    style.backdrop_saturate = 1.0;
    style.glass_noise = 0.0;
    style.specular_edge = None;
    style.sheen = None;
    style.adaptive_tint = None;
    style
}

/// `a` moved `amount` of the way to `b` in sRGB (alpha from `a`).
fn mix(a: crate::Color, b: crate::Color, amount: f32) -> crate::Color {
    let (x, y) = (a.components, b.components);
    crate::Color::new([
        x[0] + (y[0] - x[0]) * amount,
        x[1] + (y[1] - x[1]) * amount,
        x[2] + (y[2] - x[2]) * amount,
        x[3],
    ])
}

#[cfg(test)]
mod tests {

    fn styled(base: Theme, style: crate::MaterialStyle) -> Theme {
        base.with_material_style(style)
    }

    const GLASS_ROLES: [Surface; 6] = [
        Surface::Bar,
        Surface::Dock,
        Surface::Popover,
        Surface::Menu,
        Surface::Glass,
        Surface::Modal,
    ];

    fn fill(s: &Style) -> crate::Color {
        match s.fill {
            Some(Paint::Solid(c)) => c,
            ref other => panic!("a solid fill, got {other:?}"),
        }
    }

    fn rgb_distance(a: crate::Color, b: crate::Color) -> f32 {
        let (x, y) = (a.components, b.components);
        ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
    }

    fn lerp(a: crate::Color, b: crate::Color, t: f32) -> crate::Color {
        let (x, y) = (a.components, b.components);
        crate::Color::new([
            x[0] + (y[0] - x[0]) * t,
            x[1] + (y[1] - x[1]) * t,
            x[2] + (y[2] - x[2]) * t,
            x[3],
        ])
    }

    #[test]
    fn liquid_glass_frosts_only_what_floats() {
        let t = styled(Theme::dark(), crate::MaterialStyle::LiquidGlass);
        for role in GLASS_ROLES {
            let s = t.surface_style(role);
            assert!(s.backdrop_blur.is_some(), "{role:?} frosted");
            assert!(s.adaptive_tint.is_some(), "{role:?} adaptive");
            assert!(s.specular_edge.is_some(), "{role:?} rim");
        }
        for role in [Surface::Card, Surface::Raised, Surface::Window] {
            assert!(
                t.surface_style(role).backdrop_blur.is_none(),
                "{role:?} solid"
            );
        }
        assert!(t.surface_style(Surface::Bar).shadow_token.is_none());
        assert!(t.surface_style(Surface::Dock).shadow_token.is_some());
    }

    #[test]
    fn acrylic_is_grained_thick_glass_over_mica() {
        let t = styled(Theme::dark(), crate::MaterialStyle::Acrylic);
        let pop = t.surface_style(Surface::Popover);
        assert_eq!(pop.backdrop_blur, Some(30.0));
        assert!((pop.glass_noise - 0.02).abs() < 1e-6);
        assert_eq!(
            pop.border,
            Some(Border {
                width: 1.0,
                color: t.border_subtle
            })
        );
        assert!(t.surface_style(Surface::Bar).border.is_none());
        assert!(
            t.surface_style(Surface::Modal).backdrop_blur.is_none(),
            "an opaque modal"
        );
        let w = t.surface_style(Surface::Window);
        assert_eq!(fill(&w), lerp(t.bg, t.accent, 0.05));
        assert!((w.surface_noise - 0.02).abs() < 1e-6);
    }

    #[test]
    fn tonal_has_no_glass_and_rises_toward_the_accent() {
        let t = styled(Theme::dark(), crate::MaterialStyle::Tonal);
        for role in LEGACY_ROLES
            .into_iter()
            .chain([Surface::Bar, Surface::Dock, Surface::Window])
        {
            assert!(t.surface_style(role).backdrop_blur.is_none(), "{role:?}");
        }
        let lift = |role| rgb_distance(fill(&t.surface_style(role)), t.surface);
        let order = [
            Surface::Card,
            Surface::Bar,
            Surface::Dock,
            Surface::Popover,
            Surface::Modal,
        ];
        for pair in order.windows(2) {
            assert!(
                lift(pair[0]) < lift(pair[1]),
                "{:?} < {:?}",
                pair[0],
                pair[1]
            );
        }
        assert!(t.surface_style(Surface::Popover).shadow_token.is_some());
        assert!(t.surface_style(Surface::Bar).shadow_token.is_none());
    }

    #[test]
    fn recipes_follow_the_radius_scale() {
        let mut t = styled(Theme::dark(), crate::MaterialStyle::LiquidGlass);
        let stock = t.surface_style(Surface::Popover).corner_radius;
        assert_eq!(stock, CornerRadius::all(t.radius.xl));
        t.radius = RadiusScale::sharp();
        let sharp = t.surface_style(Surface::Popover).corner_radius;
        assert_eq!(sharp, CornerRadius::all(RadiusScale::sharp().xl));
        assert!(sharp.tl < stock.tl);
    }

    #[test]
    fn flat_elevation_holds_in_every_style() {
        for style in crate::MaterialStyle::ALL {
            let t = styled(Theme::dark(), style).with_elevation(Elevation::Flat);
            assert!(
                t.surface_style(Surface::Card).shadow_token.is_none(),
                "{style:?}"
            );
        }
    }

    #[test]
    fn reduced_transparency_drops_vibrant_labels() {
        use crate::Vibrancy::Primary;
        for style in crate::MaterialStyle::ALL {
            let t = styled(Theme::dark(), style).with_reduce_transparency(true);
            for role in GLASS_ROLES.into_iter().chain(LEGACY_ROLES) {
                let s = t.surface_style(role);
                assert!(s.backdrop_blur.is_none(), "{style:?} {role:?}");
                if role != Surface::Tooltip {
                    assert!(
                        (fill(&s).components[3] - 1.0).abs() < 1e-6,
                        "{style:?} {role:?} opaque"
                    );
                }
            }
            assert_eq!(t.label_on(Surface::Bar, Primary), t.text, "{style:?}");
        }
        let t = styled(Theme::dark(), crate::MaterialStyle::LiquidGlass);
        assert_eq!(t.label_on(Surface::Bar, Primary), t.vibrant_label(Primary));
        assert_eq!(t.label_on(Surface::Card, Primary), t.text);
        assert_eq!(
            t.label_on(Surface::Bar, crate::Vibrancy::Secondary),
            t.vibrant_label(crate::Vibrancy::Secondary)
        );
        for style in [
            crate::MaterialStyle::Solid,
            crate::MaterialStyle::Acrylic,
            crate::MaterialStyle::Tonal,
        ] {
            let t = styled(Theme::dark(), style);
            assert_eq!(t.label_on(Surface::Bar, Primary), t.text, "{style:?}");
            assert_eq!(
                t.label_on(Surface::Bar, crate::Vibrancy::Secondary),
                t.text_muted,
                "{style:?}"
            );
            assert_eq!(
                t.label_on(Surface::Bar, crate::Vibrancy::Tertiary),
                t.text_subtle,
                "{style:?}"
            );
        }
    }

    #[test]
    fn recipes_resolve_in_light_and_dark() {
        use crate::Vibrancy::Primary;
        let light = styled(Theme::light(), crate::MaterialStyle::LiquidGlass);
        assert_eq!(
            light.label_on(Surface::Bar, Primary).components[0],
            0.0,
            "black ink"
        );
        let mica = |base: Theme| {
            fill(&styled(base, crate::MaterialStyle::Acrylic).surface_style(Surface::Window))
        };
        let (l, d) = (mica(Theme::light()), mica(Theme::dark()));
        assert!(
            l.components[0] + l.components[1] + l.components[2]
                > d.components[0] + d.components[1] + d.components[2]
        );
    }

    /// Today's resolution path (before material styles), kept as the
    /// reference the Solid style must match bit for bit.
    fn legacy(theme: &Theme, role: Surface) -> Style {
        let mut s = role
            .bundle()
            .apply(theme, Style::default())
            .rounded(role.radius_px(&theme.radius));
        if theme.elevation == Elevation::Flat && matches!(role, Surface::Card | Surface::Raised) {
            s.shadow_token = None;
        }
        s
    }

    const LEGACY_ROLES: [Surface; 8] = [
        Surface::Card,
        Surface::Raised,
        Surface::Popover,
        Surface::Menu,
        Surface::Modal,
        Surface::Thumb,
        Surface::Tooltip,
        Surface::Glass,
    ];

    #[test]
    fn solid_resolution_is_unchanged() {
        for base in [Theme::light(), Theme::dark()] {
            for radius in [RadiusScale::default(), RadiusScale::sharp()] {
                for elevation in [Elevation::default(), Elevation::Flat] {
                    let mut theme = base.clone();
                    theme.radius = radius;
                    theme.elevation = elevation;
                    assert_eq!(theme.material_style, crate::MaterialStyle::Solid);
                    for role in LEGACY_ROLES {
                        assert_eq!(theme.surface_style(role), legacy(&theme, role), "{role:?}");
                        let via_element = theme.apply_surface(role, Style::default());
                        assert_eq!(
                            via_element,
                            legacy(&theme, role),
                            "{role:?} via apply_surface"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn solid_bar_dock_and_window() {
        let theme = Theme::dark();
        for role in [Surface::Bar, Surface::Dock] {
            let s = theme.surface_style(role);
            assert_eq!(s.fill, Some(Paint::Solid(theme.surface)), "{role:?}");
            assert_eq!(
                s.border,
                Some(Border {
                    width: 1.0,
                    color: theme.border_subtle
                }),
                "{role:?}"
            );
            assert!(
                s.backdrop_blur.is_none() && s.shadow_token.is_none(),
                "{role:?}"
            );
        }
        let w = theme.surface_style(Surface::Window);
        assert_eq!(w.fill, Some(Paint::Solid(theme.bg)));
        assert!(w.border.is_none());
        assert_eq!(w.corner_radius, CornerRadius::all(0.0));
    }

    /// A spec in the default (solid) style is written as before material
    /// styles, so older fenestra (whose spec denies unknown fields) still
    /// reads it; a named style is written in `snake_case`, like `Mode`.
    #[test]
    fn a_spec_writes_its_style_only_when_named() {
        use crate::{MaterialStyle, Mode, ThemeSpec};
        let spec = |material_style| ThemeSpec {
            mode: Mode::Dark,
            accent_hue: Some(150.0),
            duotone: None,
            derive: None,
            material_style,
        };
        let solid = serde_json::to_string(&spec(MaterialStyle::Solid)).expect("json");
        assert_eq!(solid, r#"{"mode":"dark","accent_hue":150.0}"#);
        for (style, name) in [
            (MaterialStyle::LiquidGlass, "liquid_glass"),
            (MaterialStyle::Acrylic, "acrylic"),
            (MaterialStyle::Tonal, "tonal"),
        ] {
            let json = serde_json::to_string(&spec(style)).expect("json");
            assert!(
                json.contains(&format!(r#""material_style":"{name}""#)),
                "{json}"
            );
            let back: ThemeSpec = serde_json::from_str(&json).expect("a spec");
            assert_eq!(back.material_style, style);
        }
    }

    #[test]
    fn material_style_round_trips_through_the_theme_and_spec() {
        use crate::{DeriveSpec, MaterialStyle, Mode, ThemeSpec};
        assert_eq!(Theme::dark().material_style, MaterialStyle::Solid);
        assert!(!Theme::dark().reduce_transparency);
        let acrylic = Theme::dark().with_material_style(MaterialStyle::Acrylic);
        assert_eq!(acrylic.material_style, MaterialStyle::Acrylic);
        assert!(
            Theme::dark()
                .with_reduce_transparency(true)
                .reduce_transparency
        );
        let spec: ThemeSpec = serde_json::from_str(r#"{"mode":"dark"}"#).expect("a spec");
        assert_eq!(spec.material_style, MaterialStyle::Solid);
        let spec: ThemeSpec =
            serde_json::from_str(r#"{"mode":"dark","material_style":"liquid_glass"}"#)
                .expect("a spec");
        assert_eq!(spec.theme().material_style, MaterialStyle::LiquidGlass);
        let high = ThemeSpec {
            derive: Some(DeriveSpec {
                base_hue: 250.0,
                base_chroma: 1.0,
                accent_hue: 150.0,
                contrast: crate::Contrast::High,
            }),
            mode: Mode::Dark,
            accent_hue: None,
            duotone: None,
            material_style: MaterialStyle::Solid,
        };
        assert!(
            high.theme().reduce_transparency,
            "high contrast reduces transparency"
        );
        assert!(
            MaterialStyle::LiquidGlass.bars_overlay_content()
                && MaterialStyle::LiquidGlass.floating_dock()
        );
        assert!(
            MaterialStyle::Acrylic.bars_overlay_content()
                && !MaterialStyle::Acrylic.floating_dock()
        );
        assert!(
            !MaterialStyle::Tonal.bars_overlay_content()
                && !MaterialStyle::Solid.bars_overlay_content()
        );
        assert_eq!(MaterialStyle::ALL.len(), 4);
    }
    use super::*;
    use crate::style::{Border, CornerRadius, Edges, Paint};
    use crate::tokens::{R_FULL, R_LG, R_MD, R_XL, SP1, ShadowToken};

    #[test]
    fn card_bundle_is_resting_raised() {
        // Asserts role identity, not colors.
        let b = Surface::Card.bundle();
        assert_eq!(b.radius, SurfaceRadius::Uniform(R_LG));
        assert_eq!(b.fill, SurfaceFill::SurfaceRaised);
        assert_eq!(b.border, SurfaceBorder::Subtle);
        assert_eq!(b.shadow, Some(ShadowToken::Sm));
        assert_eq!(b.highlight, None);
    }

    #[test]
    fn floating_roles_use_elevated_fill_and_deep_shadows() {
        // Locks the floating recipe.
        for role in [Surface::Popover, Surface::Menu] {
            assert_eq!(role.bundle().fill, SurfaceFill::Elevated(2));
            assert_eq!(role.bundle().shadow, Some(ShadowToken::Lg));
        }
        let m = Surface::Modal.bundle();
        assert_eq!(m.fill, SurfaceFill::Elevated(2));
        assert_eq!(m.shadow, Some(ShadowToken::Xl));
        assert_eq!(m.radius.outer(), R_XL);
    }

    #[test]
    fn ordering_invariant_floating_ge_resting() {
        // The acceptance invariant: every floating role is at least as round
        // and at least as deep as every resting role. Thumb and Tooltip are
        // excluded — they are exempt control/chip materials, not on the ladder.
        for f in [
            Surface::Popover,
            Surface::Menu,
            Surface::Modal,
            Surface::Glass,
        ] {
            for r in [Surface::Card, Surface::Raised] {
                assert!(
                    f.bundle().radius.outer() >= r.bundle().radius.outer(),
                    "{f:?} radius must be >= {r:?}"
                );
                assert!(
                    f.bundle().shadow >= r.bundle().shadow,
                    "{f:?} shadow must be >= {r:?}"
                );
            }
        }
    }

    #[test]
    fn resolve_uses_theme_roles_not_literals() {
        // Proves role-derivation across both modes.
        for t in [Theme::light(), Theme::dark()] {
            let card = Surface::Card.bundle().apply(&t, Style::default());
            assert_eq!(card.fill, Some(Paint::Solid(t.surface_raised)));
            assert_eq!(
                card.border,
                Some(Border {
                    width: 1.0,
                    color: t.border_subtle,
                })
            );
            assert_eq!(card.shadow_token, Some(ShadowToken::Sm));
            assert_eq!(card.corner_radius, CornerRadius::all(R_LG));

            let menu = Surface::Menu.bundle().apply(&t, Style::default());
            assert_eq!(menu.fill, Some(Paint::Solid(t.elevated_surface(2))));

            let tip = Surface::Tooltip.bundle().apply(&t, Style::default());
            assert_eq!(tip.fill, Some(Paint::Solid(t.neutrals.step(12))));
            assert_eq!(tip.border, None);
        }
    }

    #[test]
    fn highlight_resolves_to_low_alpha_white() {
        // Exercises the highlight path directly (the shipped `Surface::Glass`
        // role also sets one; this pins the resolution in isolation).
        let t = Theme::light();
        let bundle = SurfaceBundle {
            highlight: Some(0.14),
            ..Surface::Card.bundle()
        };
        let s = bundle.apply(&t, Style::default());
        let c = s.highlight_top.expect("highlight set");
        let [r, g, b, a] = c.components;
        assert!((a - 0.14).abs() < 1e-4, "alpha {a}");
        assert!(
            (r - g).abs() < 1e-3 && (g - b).abs() < 1e-3,
            "achromatic: {r} {g} {b}"
        );
        assert!(r > 0.9, "near-white: {r}");
    }

    #[test]
    fn thumb_is_a_pill_with_a_firm_border() {
        let b = Surface::Thumb.bundle();
        assert_eq!(b.radius.outer(), R_FULL);
        assert_eq!(b.border, SurfaceBorder::Default);
        assert_eq!(b.shadow, Some(ShadowToken::Sm));
    }

    #[test]
    fn surface_radius_is_concentric_ready() {
        // Documents the forward-compat slot.
        assert!(matches!(
            Surface::Card.bundle().radius,
            SurfaceRadius::Uniform(_)
        ));
    }

    #[test]
    fn concentric_inner_is_outer_minus_inset() {
        // The concentric accessor: a child inset by `inset` derives its radius
        // as `max(0, outer - inset)`.
        assert_eq!(SurfaceRadius::Uniform(14.0).inner(4.0), 10.0);
        // Clamp floor: a child padded more than the outer radius is square.
        assert_eq!(SurfaceRadius::Uniform(2.0).inner(8.0), 0.0);
    }

    #[test]
    fn menu_item_radius_is_concentric_with_panel() {
        // Acceptance: a menu item inset by SP1 derives the panel-concentric
        // radius, which equals the old hand-typed `R_LG - 4.0` == R_MD.
        let r = Surface::Menu.bundle().radius;
        assert_eq!(r.inner(SP1), r.outer() - SP1);
        assert_eq!(r.inner(SP1), R_MD);
    }

    #[test]
    fn theme_surface_style_equals_bundle_apply() {
        let t = Theme::light();
        assert_eq!(
            t.surface_style(Surface::Modal),
            Surface::Modal.bundle().apply(&t, Style::default())
        );
    }

    #[test]
    fn surface_apply_preserves_layout() {
        let t = Theme::light();
        // apply overlays the material (fill/radius/etc.) while leaving layout
        // and text fields of the base style untouched.
        let s = Surface::Card.bundle().apply(&t, Style::default().p(8.0));
        assert_eq!(s.padding, Edges::all(8.0));
        assert!(s.fill.is_some());
    }

    // ---- 0.22 material / translucency (glass) ----

    /// Wraparound-safe hue distance in degrees.
    fn hue_delta(a: f32, b: f32) -> f32 {
        let d = (a - b).abs() % 360.0;
        d.min(360.0 - d)
    }

    /// Straight source-over of a translucent `fg` onto an opaque `bg`, returned
    /// as an opaque color — the no-blur worst case a glass pane composites to.
    fn composite_over_opaque(fg: crate::Color, bg: crate::Color) -> crate::Color {
        let f = fg.components;
        let b = bg.components;
        let a = f[3];
        crate::Color::new([
            f[0] * a + b[0] * (1.0 - a),
            f[1] * a + b[1] * (1.0 - a),
            f[2] * a + b[2] * (1.0 - a),
            1.0,
        ])
    }

    #[test]
    fn material_tint_is_translucent_theme_derived_and_in_gamut() {
        for t in [Theme::light(), Theme::dark()] {
            let base = t.elevated_surface(2);
            let tint = Material::popover().tint(base);
            // alpha == fill_alpha, in 0..=1.
            assert!(
                (tint.components[3] - 0.82).abs() < 1e-6,
                "alpha == fill_alpha: {}",
                tint.components[3]
            );
            let [bl, bc, _] = crate::oklch_of(base);
            let [tl, tc, _] = crate::oklch_of(tint);
            // Keeps the theme token's lightness.
            assert!((tl - bl).abs() < 1e-3, "L kept: {tl} vs {bl}");
            // Chroma is never reduced (vibrancy >= 1; honest about smallness on
            // near-neutral surfaces).
            assert!(tc >= bc - 1e-4, "chroma not reduced: {tc} vs {bc}");
            // Gamut-safe.
            for &ch in &tint.components[..3] {
                assert!((0.0..=1.0).contains(&ch), "channel in gamut: {ch}");
            }
            // Hue is preserved by construction (the levers never touch `h`).
            // The recovered hue of `Elevated(2)` is numerically meaningless at
            // its near-zero chroma (a sub-1e-3 clamp swings atan2 by tens of
            // degrees), so verify hue preservation on a chroma-rich color — the
            // accent — where it is well-defined.
            let accent_h = crate::oklch_of(t.accent)[2];
            let tinted_accent_h = crate::oklch_of(Material::popover().tint(t.accent))[2];
            assert!(
                hue_delta(tinted_accent_h, accent_h) < 1.0,
                "hue kept on a chroma-rich color: {tinted_accent_h} vs {accent_h}"
            );
        }
    }

    #[test]
    fn material_tint_is_deterministic() {
        let t = Theme::dark();
        let base = t.elevated_surface(2);
        let m = Material::popover();
        let m2 = m; // Material is Copy; a copy resolves identically.
        let a = m.tint(base).to_rgba8();
        let b = m.tint(base).to_rgba8();
        let c = m2.tint(base).to_rgba8();
        assert_eq!([a.r, a.g, a.b, a.a], [b.r, b.g, b.b, b.a]);
        assert_eq!([a.r, a.g, a.b, a.a], [c.r, c.g, c.b, c.a]);
    }

    #[test]
    fn material_alpha_clamps() {
        let t = Theme::light();
        let base = t.elevated_surface(2);
        assert!(
            (Material::new(1.5, 0.0, 1.0).tint(base).components[3] - 1.0).abs() < 1e-6,
            "fill_alpha > 1 clamps to 1"
        );
        assert!(
            Material::new(-0.2, 0.0, 1.0).tint(base).components[3].abs() < 1e-6,
            "fill_alpha < 0 clamps to 0"
        );
    }

    #[test]
    fn glass_role_bundle_is_frosted_floating() {
        let b = Surface::Glass.bundle();
        assert_eq!(b.fill, SurfaceFill::Elevated(2));
        assert_eq!(b.shadow, Some(ShadowToken::Lg));
        assert_eq!(b.border, SurfaceBorder::Subtle);
        // **Its own material, and a sharper lip.** These were the popover's
        // 0.82 fill and a 0.16 highlight, and together they read as a slab: a
        // popover's opacity is chosen to carry text over anything, which is
        // the opposite of letting you see through, and a soft wide highlight
        // reads as a rounded-over edge rather than a cut one.
        assert_eq!(b.highlight, Some(0.08));
        assert_eq!(b.material, Some(Material::glass()));
        assert!(
            Material::glass().fill_alpha < Material::popover().fill_alpha,
            "glass has to be the more transparent of the two, or it is not glass"
        );
        assert_eq!(b.radius, SurfaceRadius::Uniform(R_LG));
    }

    #[test]
    fn glass_apply_fill_is_translucent_and_elevated() {
        for t in [Theme::light(), Theme::dark()] {
            let s = Surface::Glass.bundle().apply(&t, Style::default());
            let Some(Paint::Solid(c)) = s.fill else {
                panic!("glass fill should resolve to a solid paint");
            };
            assert!(c.components[3] < 1.0, "translucent: {}", c.components[3]);
            // It is the elevated surface tinted: same OKLCH lightness (stable;
            // unlike hue at this near-zero chroma), chroma never reduced.
            let [bl, bc, _] = crate::oklch_of(t.elevated_surface(2));
            let [cl, cc, _] = crate::oklch_of(c);
            assert!(
                (cl - bl).abs() < 1e-3,
                "lightness of Elevated(2) kept: {cl} vs {bl}"
            );
            assert!(cc >= bc - 1e-4, "chroma not reduced: {cc} vs {bc}");
            let hl = s.highlight_top.expect("glass sets a top highlight");
            let [r, g, b, a] = hl.components;
            assert!(
                r > 0.9 && (r - g).abs() < 1e-3 && (g - b).abs() < 1e-3,
                "near-white sheen: {r} {g} {b}"
            );
            assert!((0.0..0.3).contains(&a), "low-alpha sheen: {a}");
        }
    }

    #[test]
    fn glass_apply_sets_specular_edge_and_sheen() {
        // The frosted material carries the Liquid Glass edge optics (specular rim
        // + body sheen); every opaque role carries none, so they stay
        // byte-identical to before this pass.
        let t = Theme::light();
        let glass = Surface::Glass.bundle().apply(&t, Style::default());
        assert_eq!(
            glass.specular_edge,
            Some(crate::style::SpecularEdge::glass()),
            "glass sets the specular rim"
        );
        assert_eq!(
            glass.sheen,
            Some(crate::style::Sheen::glass()),
            "glass sets the body sheen"
        );
        assert_eq!(
            glass.adaptive_tint,
            Some(crate::style::AdaptiveTint::glass()),
            "glass sets the backdrop-adaptive vibrancy"
        );
        for role in [
            Surface::Card,
            Surface::Raised,
            Surface::Popover,
            Surface::Menu,
            Surface::Modal,
            Surface::Thumb,
            Surface::Tooltip,
        ] {
            let s = role.bundle().apply(&t, Style::default());
            assert_eq!(s.specular_edge, None, "{role:?} has no specular rim");
            assert_eq!(s.sheen, None, "{role:?} has no sheen");
            assert_eq!(s.adaptive_tint, None, "{role:?} has no adaptive tint");
        }
    }

    #[test]
    fn glass_is_floating_satisfies_ordering() {
        assert!(Surface::Glass.is_floating());
        for r in [Surface::Card, Surface::Raised] {
            assert!(
                Surface::Glass.bundle().radius.outer() >= r.bundle().radius.outer(),
                "Glass radius >= {r:?}"
            );
            assert!(
                Surface::Glass.bundle().shadow >= r.bundle().shadow,
                "Glass shadow >= {r:?}"
            );
        }
    }

    #[test]
    fn opaque_roles_unchanged() {
        for t in [Theme::light(), Theme::dark()] {
            for role in [
                Surface::Card,
                Surface::Raised,
                Surface::Popover,
                Surface::Menu,
                Surface::Modal,
                Surface::Thumb,
                Surface::Tooltip,
            ] {
                assert!(role.bundle().material.is_none(), "{role:?} has no material");
                let s = role.bundle().apply(&t, Style::default());
                assert_eq!(
                    s.fill,
                    Some(Paint::Solid(role.bundle().fill.color(&t))),
                    "{role:?} fill is the unmodified solid role color"
                );
            }
        }
    }

    #[test]
    fn glass_text_stays_legible() {
        // Guard the SHIPPED glass-showcase text against its role floors over the
        // real backdrop. The command label is `text` (primary, floor 75) and the
        // shortcut hint is `text_muted` (secondary, floor 55) — the same floors
        // `validate_contrast` enforces. The pane is a translucent tint (no live
        // blur) composited over `accent_gradient(135)` (accents 7..10), so check
        // both gradient endpoints — the worst cases — in both modes. (Secondary
        // text sits near its 55 floor by design; this proves it never drops
        // under it, where the old test only checked the easier `text`@16px.)
        const PRIMARY: f64 = 75.0;
        const SECONDARY: f64 = 55.0;
        for t in [Theme::light(), Theme::dark()] {
            let tint = Material::popover().tint(t.elevated_surface(2));
            for bg in [t.accents.step(7), t.accents.step(10)] {
                let comp = composite_over_opaque(tint, bg);
                let label = crate::lc_abs(t.text, comp);
                let hint = crate::lc_abs(t.text_muted, comp);
                assert!(
                    label >= PRIMARY,
                    "glass label (text) Lc {label:.1} < {PRIMARY} over {:?}",
                    bg.to_rgba8()
                );
                assert!(
                    hint >= SECONDARY,
                    "glass hint (text_muted) Lc {hint:.1} < {SECONDARY} over {:?}",
                    bg.to_rgba8()
                );
            }
        }
    }
}
