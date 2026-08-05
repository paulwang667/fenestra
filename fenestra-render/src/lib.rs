//! The fenestra engine behind the `fenestra` CLI and the `fenestra-mcp`
//! server: render a serialized `Description` to pixels, drive it through
//! scripted interactions, and compare against baselines. One engine, two
//! front doors.

pub mod described_app;
pub mod engine;

// `A2uiRenderOut::notes` is a `Vec<fenestra_a2ui::Note>`, so a caller who
// reads that field has to be able to name its type without taking on the
// dependency — and keeping its version in lockstep with ours — themselves.
pub use fenestra_a2ui;
pub mod preview_app;
pub mod scenario;
pub mod theme_input;

pub use described_app::DescribedApp;
pub use engine::{
    EngineError, FilmOut, InteractOut, RenderOut, ScreenshotDiff, Step, diff_images, film,
    interact, match_screenshot, parse_size, render, validate_masks,
};
pub use preview_app::{PreviewApp, PreviewMsg};
pub use scenario::{
    AriaExpect, CheckOutcome, Expect, QueryExpect, Scenario, ScreenshotExpect, VerifyOut,
    VerifyReport, bless, verify,
};
pub use theme_input::resolve_theme;
