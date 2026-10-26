//! Opt-in frame timing for on-device experiments.
//!
//! `FENESTRA_FRAME_STATS=1` times each full frame of the main window by
//! phase and logs p50/p95/max to stderr every [`WINDOW`] frames;
//! `FENESTRA_FRAME_STATS=sync` also waits for the GPU after vello's render,
//! so `gpu` is the render's execution time rather than zero.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Frames per logged summary.
const WINDOW: usize = 20;

#[derive(Clone, Copy)]
pub(crate) enum Phase {
    /// `build_frame`: view, layout, text shaping.
    Build,
    /// Scene painting (both passes when the frame has filtered regions).
    Paint,
    /// Two-pass only: rendering the backdrop off-screen and reading it back.
    Readback,
    /// Two-pass only: CPU filters (blur, glass, custom renders) on the readback.
    Filters,
    /// vello's `render_to_texture`: encode and submit (CPU compute in CPU mode).
    Render,
    /// Waiting for the GPU to finish the render (`sync` mode only).
    Gpu,
    /// `get_current_texture`: the swapchain image (blocks on vsync when full).
    Acquire,
    /// Blit, submit, present.
    Present,
}

const PHASES: [&str; 8] = [
    "build", "paint", "readback", "filters", "render", "gpu", "acquire", "present",
];

#[derive(Default)]
struct Stats {
    phases: [Vec<f64>; 8],
    /// Frames that took the two-pass path, and filtered regions seen.
    two_pass: usize,
    specs: usize,
    total: Vec<f64>,
    interval: Vec<f64>,
    frame_start: Option<Instant>,
    last_end: Option<Instant>,
}

fn mode() -> Option<&'static str> {
    static MODE: OnceLock<Option<String>> = OnceLock::new();
    MODE.get_or_init(|| {
        std::env::var("FENESTRA_FRAME_STATS")
            .ok()
            .filter(|v| v != "0")
    })
    .as_deref()
}

/// Whether frames are being timed.
pub(crate) fn enabled() -> bool {
    mode().is_some()
}

/// Whether to wait for the GPU after each render.
pub(crate) fn sync() -> bool {
    mode() == Some("sync")
}

fn stats() -> &'static Mutex<Stats> {
    static STATS: OnceLock<Mutex<Stats>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(Stats::default()))
}

/// Marks the start of a full frame.
pub(crate) fn frame_start() {
    if enabled() {
        stats().lock().unwrap().frame_start = Some(Instant::now());
    }
}

/// Records one phase's duration in the current frame.
pub(crate) fn record(phase: Phase, d: Duration) {
    if enabled() {
        stats().lock().unwrap().phases[phase as usize].push(d.as_secs_f64() * 1e3);
    }
}

/// Notes a two-pass frame with `specs` filtered regions.
pub(crate) fn two_pass(specs: usize) {
    if enabled() {
        let mut s = stats().lock().unwrap();
        s.two_pass += 1;
        s.specs += specs;
    }
}

/// Ends a full frame; every [`WINDOW`] frames logs a summary and resets.
pub(crate) fn frame_end() {
    if !enabled() {
        return;
    }
    let mut s = stats().lock().unwrap();
    let now = Instant::now();
    if let Some(start) = s.frame_start.take() {
        s.total.push((now - start).as_secs_f64() * 1e3);
    }
    if let Some(last) = s.last_end.replace(now) {
        s.interval.push((now - last).as_secs_f64() * 1e3);
    }
    if s.total.len() < WINDOW {
        return;
    }
    let mut line = format!(
        "FRAMESTATS n={WINDOW} two_pass={} specs={}",
        s.two_pass, s.specs
    );
    s.two_pass = 0;
    s.specs = 0;
    let mut fmt = |name: &str, v: &mut Vec<f64>| {
        if v.is_empty() {
            return;
        }
        v.sort_by(f64::total_cmp);
        let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        line += &format!(
            " {name}={:.1}/{:.1}/{:.1}",
            at(0.5),
            at(0.95),
            v[v.len() - 1]
        );
        v.clear();
    };
    let Stats {
        phases,
        total,
        interval,
        ..
    } = &mut *s;
    for (name, v) in PHASES.iter().zip(phases.iter_mut()) {
        fmt(name, v);
    }
    fmt("total", total);
    fmt("interval", interval);
    eprintln!("{line}");
}
