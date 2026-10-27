//! Touch-scroll physics: the glide after a fling, the rubber band past an
//! edge and the spring that brings it back. Pure functions of time, so a
//! frame advances them by however long it has been.

/// The glide's time constant (seconds): a fling at `v` travels about `v·τ`.
pub(crate) const TAU: f32 = 0.325;

/// A glide slower than this (px/s) stops.
pub(crate) const STOP_SPEED: f32 = 20.0;

/// A release slower than this (px/s) does not glide.
pub(crate) const MIN_FLING: f32 = 50.0;

/// The fastest glide (px/s).
pub(crate) const MAX_FLING: f32 = 8000.0;

/// The spring's angular frequency (1/s): critically damped, settled in
/// about 0.4 s.
const OMEGA: f32 = 12.0;

/// How far an edge may be overshot, as a share of the viewport.
const MAX_OVERSHOOT: f32 = 0.25;

/// The rubber band's stiffness (iOS's constant).
const BAND: f32 = 0.55;

/// A stretch this small (px), moving this slowly (px/s), is at rest.
const REST: f32 = 0.5;
const REST_SPEED: f32 = 5.0;

/// The glide over `dt` seconds from velocity `v`: the distance it covers
/// and its velocity after.
pub(crate) fn glide(v: f32, dt: f32) -> (f32, f32) {
    let decay = (-dt / TAU).exp();
    (v * TAU * (1.0 - decay), v * decay)
}

/// What `raw` px dragged past an edge shows in a `d` px viewport (signed
/// like `raw`).
pub(crate) fn band(raw: f32, d: f32) -> f32 {
    if d <= 0.0 {
        return 0.0;
    }
    let x = raw.abs();
    raw.signum() * (1.0 - 1.0 / (x * BAND / d + 1.0)) * d
}

/// The drag that shows `shown` px past an edge in a `d` px viewport — the
/// band's inverse, so a caught stretch goes on from where it is.
pub(crate) fn unband(shown: f32, d: f32) -> f32 {
    if d <= 0.0 {
        return 0.0;
    }
    // Shown as a fraction of d stays below 1; keep it there.
    let s = (shown.abs() / d).min(0.999);
    shown.signum() * (1.0 / (1.0 - s) - 1.0) * d / BAND
}

/// The spring pulling a stretch `x` (moving at `v`) back to the edge, after
/// `dt` seconds: the new stretch and velocity, both 0 once at rest.
pub(crate) fn spring(x: f32, v: f32, dt: f32) -> (f32, f32) {
    let b = v + OMEGA * x;
    let decay = (-OMEGA * dt).exp();
    let x1 = (x + b * dt) * decay;
    let v1 = (b - OMEGA * (x + b * dt)) * decay;
    if x1.abs() < REST && v1.abs() < REST_SPEED {
        (0.0, 0.0)
    } else {
        (x1, v1)
    }
}

/// The speed handed to the spring when a glide meets an edge, capped so it
/// overshoots at most a quarter of a `d` px viewport (the spring from rest
/// at the edge peaks at `v / (ω·e)`).
pub(crate) fn impact(v: f32, d: f32) -> f32 {
    let cap = MAX_OVERSHOOT * d * OMEGA * std::f32::consts::E;
    v.clamp(-cap, cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glide_covers_v_tau() {
        let (d, v) = glide(1000.0, 10.0);
        assert!((d - 1000.0 * TAU).abs() < 0.01);
        assert!(v.abs() < 1e-6);
    }

    #[test]
    fn unband_inverts_band() {
        for raw in [-300.0, -40.0, 0.0, 12.0, 250.0] {
            let back = unband(band(raw, 400.0), 400.0);
            assert!((back - raw).abs() < 0.01, "{raw} → {back}");
        }
    }

    #[test]
    fn the_spring_peaks_within_the_cap() {
        let v = impact(1.0e6, 400.0);
        let (mut x, mut s, mut peak) = (0.0_f32, v, 0.0_f32);
        for _ in 0..600 {
            (x, s) = spring(x, s, 0.001);
            peak = peak.max(x);
        }
        assert!(peak <= 100.5 && peak > 95.0, "{peak}");
    }
}
