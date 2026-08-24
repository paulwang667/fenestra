//! Faithfulness of the SVG→path-d conversion used to build the vendored
//! Lucide glyphs. The data in `data.rs` was generated from the lucide-static
//! v1.17.0 SVGs with a documented converter (circle/rect → arc commands,
//! leading relative moveto of each concatenated path absolutized, implicit
//! linetos kept relative, everything else verbatim). This test proves that
//! converter's *rules* by re-deriving the path-d of every proven glyph from
//! its source SVG and asserting it renders pixel-identical to the committed
//! glyph. New glyphs added from the same source set inherit those same rules,
//! so a faithful derivation here is the faithfulness guarantee for them.
//!
//! Why render-and-diff instead of string-compare: the source SVGs contain
//! native `<circle>`/`<rect>` elements, which fenestra cannot render directly
//! (there is no circle/rect element builder — only `Kind::Path`), so both the
//! committed glyph and the re-derived path-d must already be path data. They
//! are, so they go through the same rasterizer and a faithful conversion is
//! provably pixel-identical.
//!
//! Source SVGs are under `tests/fixtures/icons/` (lucide-static v1.17.0,
//! ISC license).

use fenestra_core::{path, Element};
use fenestra_shell::render_element;
use kurbo::BezPath;

/// The proven glyphs, keyed by their committed data.rs name, with the source
/// SVG fixture filename (minus `.svg`). These exercise every conversion rule
/// (circle/rect -> arcs, <line> -> moveto+line, leading-relative-moveto
/// absolutization); each must re-derive from its fixture and render
/// identically to the committed glyph.
const PROVEN: &[(&str, &str)] = &[
    ("clock", "clock"),
    ("eye", "eye"),
    ("info", "info"),
    ("search", "search"),
    ("sun", "sun"),
    ("settings", "settings"),
    ("copy", "copy"),
    ("calendar", "calendar"),
    ("credit-card", "credit-card"),
    ("lock", "lock"),
    ("calendar-days", "calendar-days"),
    ("heart", "heart"),
    ("star", "star"),
    ("triangle-alert", "triangle-alert"),
    ("refresh-cw", "refresh-cw"),
    ("folder", "folder"),
    ("bell", "bell"),
    ("user", "user"),
    ("house", "house"),
    ("download", "download"),
    ("upload", "upload"),
    ("plus", "plus"),
    ("minus", "minus"),
    ("menu", "menu"),
    ("check", "check"),
    ("x", "x"),
];

fn fixtures_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/icons")
}

fn fmt(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 {
        format!("{}", x as i64)
    } else {
        format!("{}", x)
    }
}

fn circle(cx: f64, cy: f64, r: f64) -> String {
    // Two semicircular arcs reproduce the circle; a trailing Z closes it,
    // matching the committed lucide data.rs representation (which ends every
    // circle glyph's arc with Z before any following sub-path).
    format!(
        "M{} {} A{} {} 0 1 0 {} {} A{} {} 0 1 0 {} {}Z",
        fmt(cx - r), fmt(cy), fmt(r), fmt(r), fmt(cx + r), fmt(cy), fmt(r), fmt(r), fmt(cx - r), fmt(cy)
    )
}

fn rect(x: f64, y: f64, w: f64, h: f64, rx: f64) -> String {
    if rx == 0.0 {
        return format!(
            "M{} {} L{} {} L{} {} L{} {} Z",
            fmt(x), fmt(y), fmt(x + w), fmt(y), fmt(x + w), fmt(y + h), fmt(x), fmt(y + h)
        );
    }
    let r = rx;
    format!(
        "M{} {} L{} {} A{} {} 0 0 1 {} {} L{} {} A{} {} 0 0 1 {} {} L{} {} A{} {} 0 0 1 {} {} L{} {} A{} {} 0 0 1 {} {} Z",
        fmt(x + r), fmt(y),
        fmt(x + w - r), fmt(y), fmt(r), fmt(r), fmt(x + w), fmt(y + r),
        fmt(x + w), fmt(y + h - r), fmt(r), fmt(r), fmt(x + w - r), fmt(y + h),
        fmt(x + r), fmt(y + h), fmt(r), fmt(r), fmt(x), fmt(y + h - r),
        fmt(x), fmt(y + r), fmt(r), fmt(r), fmt(x + r), fmt(y)
    )
}

/// Skip one path-data number (optional leading whitespace, sign, mantissa,
/// exponent) starting at `start`; return the offset just past it.
fn skip_number(s: &str, start: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    i
}

/// Absolutize a leading relative moveto ("mX Y...") to "MX Y...". When several
/// paths are concatenated into one path-d string, a relative moveto following
/// an earlier sub-path would be taken relative to that sub-path's end point;
/// the source intends it relative to the origin, whose destination is already
/// the coordinate itself, so only the case flips. A following implicit relative
/// line is marked explicit ("l") so it stays relative to that (now absolute)
/// moveto, exactly as the committed data.rs representation does.
fn absolutize_leading_moveto(d: &str) -> String {
    if d.as_bytes().first() != Some(&b'm') {
        return d.to_string();
    }
    let p2 = skip_number(d, skip_number(d, 1));
    let mut out = String::with_capacity(d.len() + 1);
    out.push('M');
    out.push_str(&d[1..p2]);
    let mut j = p2;
    while j < d.len() && d.as_bytes()[j].is_ascii_whitespace() {
        j += 1;
    }
    if j < d.len()
        && (d.as_bytes()[j].is_ascii_digit() || matches!(d.as_bytes()[j], b'.' | b'+' | b'-'))
    {
        out.push('l');
    }
    out.push_str(&d[j..]);
    out
}

/// Minimal SVG scanner: walk elements left-to-right, emit a path-d fragment for
/// each circle/rect/path in document order. Deliberately not a real XML parser
/// — lucide SVGs are flat `<circle>`/`<rect>`/`<path>` tags and this is enough
/// to reproduce the documented conversion.
fn convert_svg(svg: &str) -> String {
    let mut parts = Vec::new();
    let bytes = svg.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let after = &svg[i + 1..];
        if after.starts_with('/') {
            if let Some(gt) = after.find('>') {
                i = i + 1 + gt + 1;
            } else {
                break;
            }
            continue;
        }
        let name: String = after.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let gt = match after.find('>') { Some(gt) => gt, None => break };
        let inner = &after[..gt];
        match name.as_str() {
            "circle" => {
                if let (Some(cx), Some(cy), Some(r)) =
                    (attr(inner, "cx"), attr(inner, "cy"), attr(inner, "r"))
                {
                    if let (Ok(cx), Ok(cy), Ok(r)) = (cx.parse(), cy.parse(), r.parse()) {
                        parts.push(circle(cx, cy, r));
                    }
                }
            }
            "rect" => {
                if let (Some(x), Some(y), Some(w), Some(h), Some(rx)) =
                    (attr(inner, "x"), attr(inner, "y"), attr(inner, "width"), attr(inner, "height"), attr(inner, "rx"))
                {
                    let rx: f64 = rx.parse().unwrap_or(0.0);
                    if let (Ok(x), Ok(y), Ok(w), Ok(h)) = (x.parse(), y.parse(), w.parse(), h.parse()) {
                        parts.push(rect(x, y, w, h, rx));
                    }
                }
            }
            "line" => {
                if let (Some(x1), Some(y1), Some(x2), Some(y2)) =
                    (attr(inner, "x1"), attr(inner, "y1"), attr(inner, "x2"), attr(inner, "y2"))
                {
                    if let (Ok(x1), Ok(y1), Ok(x2), Ok(y2)) =
                        (x1.parse(), y1.parse(), x2.parse(), y2.parse())
                    {
                        parts.push(format!("M{} {}L{} {}", fmt(x1), fmt(y1), fmt(x2), fmt(y2)));
                    }
                }
            }
            "path" => {
                if let Some(d) = attr(inner, "d") {
                    parts.push(absolutize_leading_moveto(&d));
                }
            }
            _ => {}
        }
        i = i + 1 + gt + 1;
    }
    parts.join(" ")
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{}=", name);
    let idx = tag.find(&key)?;
    let rest = &tag[idx + key.len()..];
    let q = *rest.as_bytes().first()?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(q as char)?;
    Some(rest[..end].to_string())
}

fn render(d: &str, size: (u32, u32)) -> image::RgbaImage {
    let bez = BezPath::from_svg(d).expect("derived path-d must parse");
    let el: Element<()> = path(bez, (24.0, 24.0), Some(2.0));
    render_element(el, &fenestra_core::Theme::light(), size)
}

fn diff(a: &image::RgbaImage, b: &image::RgbaImage) -> usize {
    let mut n = 0;
    for (x, y) in a.chunks(4).zip(b.chunks(4)) {
        if x != y {
            n += 1;
        }
    }
    n
}

#[test]
fn every_proven_glyph_reconstructs_from_source() {
    let size = (24, 24);
    let mut failed: Vec<(&str, usize, String)> = Vec::new();
    for (name, fixture) in PROVEN {
        let src = std::fs::read_to_string(fixtures_dir().join(format!("{fixture}.svg")))
            .unwrap_or_else(|e| panic!("{name}: read fixture: {e}"));
        let derived = convert_svg(&src);
        let committed = fenestra_kit::icons::lucide::by_name::<()>(name)
            .expect("proven glyph must be vendored");
        let committed_img = render_element(committed, &fenestra_core::Theme::light(), size);
        let derived_img = render(&derived, size);
        let d = diff(&committed_img, &derived_img);
        if d != 0 {
            failed.push((name, d, derived));
        }
    }
    assert!(failed.is_empty(), "{}",
        failed.iter()
            .map(|(n, d, p)| format!("{n}: {d} pixels differ; derived {p:?}"))
            .collect::<Vec<_>>()
            .join("; "));
}
