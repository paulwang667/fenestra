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
    /// Scene painting (both passes when the frame has filtered regions;
    /// the readback and filters between them are their own phases).
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
    /// The current frame's phases (ms), committed by [`Stats::end`]: a
    /// frame abandoned before then leaves nothing behind.
    current: [Option<f64>; 8],
    /// The current frame's filtered regions when it takes the two-pass
    /// path, committed by [`Stats::end`] like `current`.
    current_specs: Option<usize>,
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
        stats().lock().unwrap().start(Instant::now());
    }
}

/// Records one phase's duration in the current frame.
pub(crate) fn record(phase: Phase, d: Duration) {
    if enabled() {
        stats().lock().unwrap().record(phase, d);
    }
}

/// Notes a two-pass frame with `specs` filtered regions.
pub(crate) fn two_pass(specs: usize) {
    if enabled() {
        stats().lock().unwrap().two_pass(specs);
    }
}

/// Ends a full frame; every [`WINDOW`] frames logs a summary and resets.
pub(crate) fn frame_end() {
    if enabled()
        && let Some(line) = stats().lock().unwrap().end(Instant::now())
    {
        eprintln!("{line}");
    }
}

impl Stats {
    fn start(&mut self, now: Instant) {
        self.frame_start = Some(now);
        self.current = [None; 8];
        self.current_specs = None;
    }

    fn two_pass(&mut self, specs: usize) {
        *self.current_specs.get_or_insert(0) += specs;
    }

    fn record(&mut self, phase: Phase, d: Duration) {
        *self.current[phase as usize].get_or_insert(0.0) += d.as_secs_f64() * 1e3;
    }

    /// Ends the frame at `now`; returns the summary line every [`WINDOW`]
    /// frames.
    fn end(&mut self, now: Instant) -> Option<String> {
        let start = self.frame_start.take()?;
        self.total.push((now - start).as_secs_f64() * 1e3);
        let mut current = std::mem::take(&mut self.current);
        if let Some(specs) = self.current_specs.take() {
            self.two_pass += 1;
            self.specs += specs;
        }
        // Paint is timed around the readback and filters of a two-pass frame.
        let between: f64 = [Phase::Readback, Phase::Filters]
            .iter()
            .filter_map(|p| current[*p as usize])
            .sum();
        if let Some(paint) = &mut current[Phase::Paint as usize] {
            *paint = (*paint - between).max(0.0);
        }
        for (v, ms) in self.phases.iter_mut().zip(current) {
            v.extend(ms);
        }
        if let Some(last) = self.last_end.replace(now) {
            self.interval.push((now - last).as_secs_f64() * 1e3);
        }
        if self.total.len() < WINDOW {
            return None;
        }
        let mut line = format!(
            "FRAMESTATS n={WINDOW} two_pass={} specs={}",
            self.two_pass, self.specs
        );
        self.two_pass = 0;
        self.specs = 0;
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
        for (name, v) in PHASES.iter().zip(self.phases.iter_mut()) {
            fmt(name, v);
        }
        fmt("total", &mut self.total);
        fmt("interval", &mut self.interval);
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    /// Runs one frame through `stats`: `phases` recorded (a two-pass frame
    /// with one filtered region when they include a readback), then ended
    /// at `start + total`.
    fn frame(
        stats: &mut Stats,
        start: Instant,
        total: u64,
        phases: &[(Phase, u64)],
    ) -> Option<String> {
        stats.start(start);
        for (phase, d) in phases {
            stats.record(*phase, ms(*d));
        }
        if phases.iter().any(|(p, _)| matches!(p, Phase::Readback)) {
            stats.two_pass(1);
        }
        stats.end(start + ms(total))
    }

    /// A field of the summary line, e.g. `paint=4.0/4.0/4.0` for "paint".
    fn field<'a>(line: &'a str, name: &str) -> &'a str {
        line.split(' ')
            .find_map(|f| f.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name} in {line}"))
    }

    /// Paint is the painting alone: in a two-pass frame the readback and the
    /// filters it waits on are reported as their own phases.
    #[test]
    fn paint_excludes_readback_and_filters() {
        let mut stats = Stats::default();
        let t0 = Instant::now();
        let mut line = None;
        for i in 0..WINDOW {
            line = frame(
                &mut stats,
                t0 + ms(100 * i as u64),
                20,
                &[
                    (Phase::Readback, 5),
                    (Phase::Filters, 3),
                    (Phase::Paint, 12),
                ],
            );
        }
        let line = line.expect("a summary after WINDOW frames");
        assert_eq!(field(&line, "paint"), "4.0/4.0/4.0");
        assert_eq!(field(&line, "readback"), "5.0/5.0/5.0");
        assert_eq!(field(&line, "two_pass"), WINDOW.to_string());
    }

    /// A frame abandoned before `end` (no surface, a failed readback) leaves
    /// nothing behind: every summary covers exactly WINDOW complete frames.
    #[test]
    fn an_abandoned_frame_is_not_counted() {
        let mut stats = Stats::default();
        let t0 = Instant::now();
        stats.start(t0);
        stats.record(Phase::Build, ms(50));
        stats.two_pass(3);
        let mut line = None;
        for i in 0..WINDOW {
            line = frame(
                &mut stats,
                t0 + ms(100 * (i as u64 + 1)),
                10,
                &[(Phase::Build, 2)],
            );
        }
        let line = line.expect("a summary after WINDOW complete frames");
        assert_eq!(field(&line, "build"), "2.0/2.0/2.0");
        assert_eq!(field(&line, "total"), "10.0/10.0/10.0");
        assert_eq!(field(&line, "two_pass"), "0");
        assert_eq!(field(&line, "specs"), "0");
    }
}
