//! The windowed runners: winit event loop + wgpu surface + vello renderer.
//!
//! [`run_scene`] paints via a raw scene callback (no input). [`run_static`]
//! runs an element view with scrolling and animation frames; the full `App`
//! runner with messages arrives in M4 and builds on the same plumbing.

use std::sync::Arc;
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use crate::gpu::{GpuContext, GpuSurface};
#[cfg(not(target_arch = "wasm32"))]
use fenestra_core::Theme;
#[allow(unused_imports)]
pub use fenestra_core::WindowControl; // re-exported for borderless-title-bar apps
use fenestra_core::{
    App, Element, Fonts, FrameState, GesturePhase, InputEvent, Key, KeyInput, build_frame,
    dispatch, refresh_hover,
};
#[cfg(not(target_arch = "wasm32"))]
use image::RgbaImage;
use kurbo::Point;
use vello::peniko::Color;
use vello::wgpu::{
    self, CurrentSurfaceTexture, Extent3d, TextureDescriptor, TextureFormat, TextureUsages,
};
// The read-back target, for the native two-pass render (the web has none).
#[cfg(not(target_arch = "wasm32"))]
use vello::wgpu::{
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, TexelCopyBufferLayout,
};
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{MouseScrollDelta, StartCause, TouchPhase, WindowEvent};
#[cfg(not(target_arch = "wasm32"))]
use winit::event_loop::EventLoopProxy;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
#[cfg(target_os = "android")]
use winit::platform::android::EventLoopBuilderExtAndroid;
use winit::window::{Window, WindowId};

use crate::ShellError;

/// One wheel "line" in logical pixels.
pub(crate) const LINE_SCROLL_PX: f64 = 40.0;

/// winit's touch phase as the framework's gesture lifecycle. A cancelled
/// gesture ends like any other: an app that undid its in-progress effect on
/// `Ended` would need a distinct variant, and nothing here does.
pub(crate) fn gesture_phase(phase: TouchPhase) -> GesturePhase {
    match phase {
        TouchPhase::Started => GesturePhase::Started,
        TouchPhase::Moved => GesturePhase::Changed,
        TouchPhase::Ended | TouchPhase::Cancelled => GesturePhase::Ended,
    }
}

/// [`crate::ime_sync::Traits`] as the winit fork's type.
#[cfg(all(target_os = "ios", feature = "ios-text-input"))]
fn ios_traits(t: crate::ime_sync::Traits) -> winit::platform::ios::TextInputTraits {
    use crate::ime_sync::{IosKeyboard, IosReturn};
    use winit::platform::ios::{KeyboardType, ReturnKeyType, TextInputTraits};
    TextInputTraits {
        keyboard: match t.keyboard {
            IosKeyboard::Default => KeyboardType::Default,
            IosKeyboard::NumberPad => KeyboardType::NumberPad,
            IosKeyboard::DecimalPad => KeyboardType::DecimalPad,
            IosKeyboard::PhonePad => KeyboardType::PhonePad,
            IosKeyboard::EmailAddress => KeyboardType::EmailAddress,
            IosKeyboard::Url => KeyboardType::Url,
        },
        return_key: match t.return_key {
            IosReturn::Default => ReturnKeyType::Default,
            IosReturn::Send => ReturnKeyType::Send,
            IosReturn::Search => ReturnKeyType::Search,
            IosReturn::Go => ReturnKeyType::Go,
            IosReturn::Done => ReturnKeyType::Done,
            IosReturn::Next => ReturnKeyType::Next,
        },
        secure: t.secure,
        autocorrect: t.autocorrect,
        autocapitalize: t.autocapitalize,
    }
}

/// The `set_ime_allowed` call that keeps a soft keyboard in step with the
/// caret: `want` (a field holds the caret), `shown` (what the keyboard was
/// last asked to be), `tapped` (the focused field was tapped, which brings
/// back a keyboard the user dismissed). `None` leaves it alone.
#[cfg(any(target_os = "android", target_os = "ios", test))]
fn keyboard_change(want: bool, shown: bool, tapped: bool) -> Option<bool> {
    (want != shown || (want && tapped)).then_some(want)
}

/// Whether a finger that went down at `down` and lifted at `up` tapped the
/// focused field — the one gesture that brings back a soft keyboard the user
/// dismissed. A scroll, or a tap anywhere else, leaves it down.
#[cfg(any(target_os = "android", target_os = "ios", test))]
pub(crate) fn tap_on_focus(
    frame: &fenestra_core::Frame,
    focus: Option<fenestra_core::WidgetId>,
    down: Point,
    up: Point,
) -> bool {
    focus.is_some_and(|f| {
        down.distance(up) <= crate::touch::TAP_SLOP && frame.hit_chain(up).contains(&f)
    })
}

/// A winit magnification delta as a finite `f32`, or `None`. winit documents
/// this value as possibly NaN, and a NaN would poison an app's zoom for the
/// rest of the session.
pub(crate) fn pinch_delta(delta: f64) -> Option<f32> {
    #[expect(clippy::cast_possible_truncation, reason = "scale deltas are small")]
    let d = delta as f32;
    d.is_finite().then_some(d)
}

/// Extracts `(dx, dy)` logical-pixel deltas from a winit wheel event, honoring
/// the window scale for pixel deltas.
pub(crate) fn wheel_deltas(delta: MouseScrollDelta, scale: f64) -> (f64, f64) {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => {
            (f64::from(x) * LINE_SCROLL_PX, f64::from(y) * LINE_SCROLL_PX)
        }
        MouseScrollDelta::PixelDelta(pos) => (pos.x / scale, pos.y / scale),
    }
}

/// The window a [`WindowDesc`] asks for.
///
/// **Everything the descriptor says, not just its title and size.** This was
/// `titled(..).with_size(..)` inline at the one call site, with the rest left
/// to the defaults — so a secondary window could not be borderless, let alone
/// transparent, placed on a chosen display, or kept on top, while the main
/// window could already be three of those. A screen-selection overlay is all
/// four at once and none of them were reachable.
///
/// A function rather than four more lines at the call site, because the call
/// site needs an event loop and a GPU and this needs neither: `windows_carry_
/// every_option_they_declare` is the whole of the mapping, tested without
/// opening anything.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn options_for<Msg>(desc: &fenestra_core::WindowDesc<Msg>) -> WindowOptions {
    let options = WindowOptions::titled(desc.title.clone())
        .with_size(desc.size.0, desc.size.1)
        .with_borderless(desc.borderless)
        .with_fullscreen(desc.fullscreen)
        .with_transparent(desc.transparent)
        .with_always_on_top(desc.always_on_top);
    match desc.position {
        Some((x, y)) => options.with_position(x, y),
        None => options,
    }
}

/// A raw paint callback: `(scene, logical_w, logical_h, background)`.
#[cfg(not(target_arch = "wasm32"))]
type PaintFn = Box<dyn FnMut(&mut Scene, f64, f64, Color)>;
/// A message-free element view function.
#[cfg(not(target_arch = "wasm32"))]
type ViewFn = Box<dyn Fn(&Theme) -> Element<()>>;

/// Options for the application window.
#[derive(Debug, Clone)]
pub struct WindowOptions {
    /// Window title.
    pub title: String,
    /// Initial inner size in logical pixels.
    pub inner_size: (f64, f64),
    /// Minimum inner size in logical pixels.
    pub min_size: Option<(f64, f64)>,
    /// Whether the window can be resized (true by default).
    pub resizable: bool,
    /// Open maximized.
    pub maximized: bool,
    /// Open borderless-fullscreen on the current monitor.
    pub fullscreen: bool,
    /// Remove the OS title bar/resize frame. Pair with `.drag_region()`
    /// elements in the view (a custom title bar) so the window can still
    /// be moved; resizing follows `resizable`.
    pub borderless: bool,
    /// Top-left in logical desktop coordinates. `None` lets the platform
    /// place the window.
    ///
    /// Desktop coordinates, not the primary monitor's: a negative or large
    /// value is how a window is put on a second display, which is the only
    /// way to cover one.
    pub position: Option<(f64, f64)>,
    /// Let what is behind the window show through wherever the view does not
    /// paint: the clear becomes transparent and the surface composites with
    /// alpha.
    ///
    /// **A request, not a guarantee.** winit is asked for a transparent
    /// window and the swapchain for a non-opaque `CompositeAlphaMode`, and
    /// neither is available everywhere — a platform that refuses either gives
    /// an opaque window with a transparent clear, which paints the window
    /// black rather than see-through. So the surface only switches when the
    /// adapter reports a mode it can honour, and a view that wants to be
    /// legible either way should say so with its own background.
    pub transparent: bool,
    /// Keep the window above ordinary ones.
    pub always_on_top: bool,
    /// Window icon as straight-alpha RGBA8 `(width, height, pixels)`.
    pub icon: Option<(u32, u32, Vec<u8>)>,
    /// Custom faces registered on the runner's fonts before the first
    /// frame: design languages work in windows, not just headlessly.
    pub fonts: Vec<(fenestra_core::FamilyRole, Vec<u8>)>,
    /// The UI language as a BCP 47 tag (`"zh-CN"`), which picks among
    /// fallback faces that draw the same characters differently — Han in
    /// Simplified, Traditional or Japanese forms. `None` takes the system's
    /// where the runner can read it (Android).
    pub locale: Option<String>,
    /// Android: the `AndroidApp` received in `android_main`, which winit
    /// needs to bind the event loop to the activity.
    #[cfg(target_os = "android")]
    pub android_app: Option<winit::platform::android::activity::AndroidApp>,
    /// Android: what a back press the app leaves alone does — normally
    /// sending the app to the background (`Activity.moveTaskToBack`), which
    /// takes a JNI call this crate (no `unsafe`) leaves to the app. `None`
    /// ignores it.
    #[cfg(target_os = "android")]
    pub android_back: Option<fn(&winit::platform::android::activity::AndroidApp)>,
    /// Android: plays a touch feedback — a long press that acted asks for
    /// [`fenestra_core::Haptic::LongPress`] — normally through
    /// `View.performHapticFeedback`, a JNI call this crate (no `unsafe`)
    /// leaves to the app. `None` plays nothing.
    #[cfg(target_os = "android")]
    pub android_haptic:
        Option<fn(&winit::platform::android::activity::AndroidApp, fenestra_core::Haptic)>,
    /// iOS: plays a touch feedback, normally a `UIImpactFeedbackGenerator`
    /// — an Objective-C call this crate (no `unsafe`) leaves to the app.
    /// `None` plays nothing.
    #[cfg(target_os = "ios")]
    pub ios_haptic: Option<fn(fenestra_core::Haptic)>,
}

impl WindowOptions {
    /// A window with the given title and the default 1024x768 logical size.
    pub fn titled(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            inner_size: (1024.0, 768.0),
            min_size: None,
            resizable: true,
            maximized: false,
            fullscreen: false,
            borderless: false,
            position: None,
            transparent: false,
            always_on_top: false,
            icon: None,
            fonts: Vec::new(),
            locale: None,
            #[cfg(target_os = "android")]
            android_app: None,
            #[cfg(target_os = "android")]
            android_back: None,
            #[cfg(target_os = "android")]
            android_haptic: None,
            #[cfg(target_os = "ios")]
            ios_haptic: None,
        }
    }

    /// Sets the initial inner size in logical pixels.
    pub fn with_size(mut self, width: f64, height: f64) -> Self {
        self.inner_size = (width, height);
        self
    }

    /// Sets the minimum inner size in logical pixels.
    pub fn with_min_size(mut self, width: f64, height: f64) -> Self {
        self.min_size = Some((width, height));
        self
    }

    /// Allows or forbids resizing (allowed by default).
    /// Places the window's top-left at `(x, y)` in logical desktop
    /// coordinates.
    #[must_use]
    pub const fn with_position(mut self, x: f64, y: f64) -> Self {
        self.position = Some((x, y));
        self
    }

    /// Asks for a window that composites with what is behind it. See
    /// [`WindowOptions::transparent`].
    #[must_use]
    pub const fn with_transparent(mut self, transparent: bool) -> Self {
        self.transparent = transparent;
        self
    }

    /// Removes the OS title bar and resize frame.
    #[must_use]
    pub const fn with_borderless(mut self, borderless: bool) -> Self {
        self.borderless = borderless;
        self
    }

    /// Opens borderless-fullscreen on the monitor the window lands on.
    #[must_use]
    pub const fn with_fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    /// Keeps the window above ordinary ones.
    #[must_use]
    pub const fn with_always_on_top(mut self, on_top: bool) -> Self {
        self.always_on_top = on_top;
        self
    }

    pub fn with_resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Opens the window maximized.
    pub fn maximized(mut self) -> Self {
        self.maximized = true;
        self
    }

    /// Opens borderless-fullscreen on the current monitor.
    pub fn fullscreen(mut self) -> Self {
        self.fullscreen = true;
        self
    }

    /// Removes the OS window decorations. Pair with `.drag_region()`
    /// elements acting as a custom title bar, or the window cannot be
    /// moved by its content.
    pub fn borderless(mut self) -> Self {
        self.borderless = true;
        self
    }

    /// Sets the window icon from straight-alpha RGBA8 pixels (ignored on
    /// platforms without window icons, including the web).
    pub fn with_icon(mut self, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        self.icon = Some((width, height, rgba));
        self
    }

    /// Registers a custom face under a family role for this window's
    /// fonts (TTF/OTF bytes; see `Fonts::register`).
    pub fn with_font(mut self, role: fenestra_core::FamilyRole, data: Vec<u8>) -> Self {
        self.fonts.push((role, data));
        self
    }

    /// Sets the UI language (a BCP 47 tag such as `"zh-CN"`); see
    /// [`WindowOptions::locale`].
    pub fn with_locale(mut self, tag: impl Into<String>) -> Self {
        self.locale = Some(tag.into());
        self
    }

    /// The UI language: the one set with [`WindowOptions::with_locale`], else
    /// the system's where the runner can read it.
    fn ui_locale(&self) -> Option<String> {
        if self.locale.is_some() {
            return self.locale.clone();
        }
        #[cfg(target_os = "android")]
        if let Some(app) = &self.android_app {
            let config = app.config();
            let language = config.language()?;
            return Some(match config.country() {
                Some(country) => format!("{language}-{country}"),
                None => language,
            });
        }
        None
    }

    /// Attaches the `AndroidApp` received in `android_main` so winit can
    /// bind the event loop to the activity (Android only).
    #[cfg(target_os = "android")]
    pub fn with_android_app(mut self, app: winit::platform::android::activity::AndroidApp) -> Self {
        self.android_app = Some(app);
        self
    }

    /// Sets what a back press the app leaves alone does (Android only; see
    /// [`WindowOptions::android_back`]).
    #[cfg(target_os = "android")]
    #[must_use]
    pub fn with_android_back(
        mut self,
        f: fn(&winit::platform::android::activity::AndroidApp),
    ) -> Self {
        self.android_back = Some(f);
        self
    }

    /// Sets how a touch feedback plays (Android only; see
    /// [`WindowOptions::android_haptic`]).
    #[cfg(target_os = "android")]
    #[must_use]
    pub fn with_android_haptic(
        mut self,
        f: fn(&winit::platform::android::activity::AndroidApp, fenestra_core::Haptic),
    ) -> Self {
        self.android_haptic = Some(f);
        self
    }

    /// Sets how a touch feedback plays (iOS only; see
    /// [`WindowOptions::ios_haptic`]).
    #[cfg(target_os = "ios")]
    #[must_use]
    pub fn with_ios_haptic(mut self, f: fn(fenestra_core::Haptic)) -> Self {
        self.ios_haptic = Some(f);
        self
    }
}

enum RenderState {
    Active {
        surface: Box<GpuSurface<'static>>,
        valid_surface: bool,
        window: Arc<Window>,
    },
    Suspended(Option<Arc<Window>>),
    /// Window created; the async surface setup is in flight (web only).
    #[cfg(target_arch = "wasm32")]
    Pending(Arc<Window>),
}

/// Shared surface plumbing for every windowed runner.
struct WindowShell {
    context: GpuContext,
    renderers: Vec<Option<Renderer>>,
    state: RenderState,
    scene: Scene,
    options: WindowOptions,
    background: Color,
    /// Whether this window composites with what is behind it. Kept beside the
    /// background because the two are one decision: a transparent window
    /// clears to nothing and lets the view paint what it wants seen.
    transparent: bool,
    /// The first unrecoverable failure (GPU/surface/renderer). Recorded by
    /// [`Self::fail`], which also stops the event loop; the runner entry
    /// function returns it once `run_app` unwinds. Runners with secondary
    /// windows funnel those failures into the main shell's slot too.
    fatal: Option<ShellError>,
    /// The two-pass read-back's off-screen target and staging buffer, kept
    /// while their size holds — a glass header reads the same strip every
    /// frame, and allocating both per frame was part of what it cost.
    #[cfg(not(target_arch = "wasm32"))]
    readback: Option<Readback>,
    /// Set when the main window draws through the raster backend.
    raster: Option<RasterState>,
    /// This window's glass panes' last blurs ([`crate::multi_pass::GlassCache`]).
    #[cfg(not(target_arch = "wasm32"))]
    glass_cache: crate::multi_pass::GlassCache,
    /// This window's GPU glass on the vello backend.
    #[cfg(not(target_arch = "wasm32"))]
    vello_glass: crate::vello_glass::VelloGlass,
    /// Completed async surface setup, parked until the next [`Self::pump`]
    /// (web only; the web is single-threaded so `Rc<RefCell>` suffices).
    #[cfg(target_arch = "wasm32")]
    ready: WasmReady,
    /// The surface kept across a suspend (iOS). Every new surface on a
    /// UIKit view adds a CAMetalLayer sublayer that dropping the surface
    /// never removes, and iOS suspends on every Control Center pull or app
    /// switch — rebuilding would stack a full-screen layer each time.
    #[cfg(target_os = "ios")]
    parked: Option<Box<GpuSurface<'static>>>,
}

/// A reusable read-back target: the texture the backdrop is rendered into and
/// the mappable buffer it is copied to, for one device and size.
#[cfg(not(target_arch = "wasm32"))]
struct Readback {
    dev_id: usize,
    width: u32,
    height: u32,
    view: wgpu::TextureView,
    texture: wgpu::Texture,
    buffer: wgpu::Buffer,
    padded_byte_width: u32,
}

/// The raster backend for this window ([`crate::Backend::Raster`]): the
/// renderer, on the surface's device, and the texture frames render into
/// before they are blitted to the swapchain (kept, so a clean frame is
/// presented again without repainting).
struct RasterState {
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "the web runner never selects the raster backend")
    )]
    dev_id: usize,
    renderer: fenestra_raster::RasterRenderer,
    target: Option<RasterTarget>,
}

struct RasterTarget {
    size: (u32, u32),
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "read back only by the native two-pass render")
    )]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// A width and height in physical pixels.
type PxSize = (u32, u32);

/// What a painted main-window frame depends on besides the app: logical
/// width, height, scale, and the safe area.
type FrameKey = (f64, f64, f64, crate::safe_area::Insets);

/// Whether the raster target must be (re)built for a `surface`-sized frame.
fn needs_new_target(current: Option<(u32, u32)>, surface: (u32, u32)) -> bool {
    current != Some(surface)
}

/// Whether a freshly created window still has to be shown: winit creates it
/// hidden, and iOS reports no visibility at all (`None`), where waiting for
/// `Some(false)` left the window hidden forever.
fn needs_show(visible: Option<bool>) -> bool {
    visible != Some(true)
}

/// The window geometry [`WindowOptions`] asks of winit.
#[derive(Debug)]
struct Geometry {
    inner_size: Option<(f64, f64)>,
    min_size: Option<(f64, f64)>,
    position: Option<(f64, f64)>,
    maximized: bool,
    fullscreen: bool,
}

/// The geometry to open `options` with. A `mobile` window (iOS) takes none
/// of it: size, position, maximized and fullscreen are desktop notions, and
/// winit on iOS turns an inner size into the UIWindow's frame — a
/// 1024×768-point window on a phone.
fn window_geometry(options: &WindowOptions, mobile: bool) -> Geometry {
    if mobile {
        return Geometry {
            inner_size: None,
            min_size: None,
            position: None,
            maximized: false,
            fullscreen: false,
        };
    }
    Geometry {
        inner_size: Some(options.inner_size),
        min_size: options.min_size,
        position: options.position,
        maximized: options.maximized,
        fullscreen: options.fullscreen,
    }
}

/// Whether a clean frame is presented again from the raster target instead
/// of repainted: nothing changed since it was rendered for `rendered` (the
/// logical size and scale), and the target still holds it at the
/// surface's size (a renderer rebuilt for a new device has no target).
fn reuses_raster_frame<K: PartialEq>(
    dirty: bool,
    rendered: Option<K>,
    now: K,
    target: Option<(u32, u32)>,
    surface: (u32, u32),
) -> bool {
    !dirty && rendered == Some(now) && !needs_new_target(target, surface)
}

/// One frame of an animation (the browser paces the web's frames itself).
#[cfg(not(target_arch = "wasm32"))]
const ANIMATION_FRAME: Duration = Duration::from_millis(16);

/// When an animation's next frame is due: one frame after this one
/// *started*, so its own work counts toward the period (counted from its
/// end, a 31 ms frame ran every 47 ms). One that took longer has the next
/// due at once.
#[cfg(not(target_arch = "wasm32"))]
fn next_animation_frame(started: Instant) -> Instant {
    started + ANIMATION_FRAME
}

/// The handoff slot for the web's async surface creation: the setup result,
/// carrying the error when WebGPU is unavailable.
#[cfg(target_arch = "wasm32")]
type WasmReady = std::rc::Rc<
    std::cell::RefCell<Option<Result<(GpuContext, Box<GpuSurface<'static>>), ShellError>>>,
>;

impl WindowShell {
    fn new(options: WindowOptions, background: Color) -> Self {
        let transparent = options.transparent;
        Self {
            context: GpuContext::new(),
            renderers: Vec::new(),
            state: RenderState::Suspended(None),
            scene: Scene::new(),
            options,
            background,
            transparent,
            fatal: None,
            #[cfg(not(target_arch = "wasm32"))]
            readback: None,
            raster: None,
            #[cfg(not(target_arch = "wasm32"))]
            glass_cache: crate::multi_pass::GlassCache::default(),
            #[cfg(not(target_arch = "wasm32"))]
            vello_glass: crate::vello_glass::VelloGlass::default(),
            #[cfg(target_arch = "wasm32")]
            ready: WasmReady::default(),
            #[cfg(target_os = "ios")]
            parked: None,
        }
    }

    /// Records the first unrecoverable failure and stops the event loop; the
    /// runner entry function surfaces it as its `Err` return. Adversarial
    /// review 2026-07 (finding B): these paths used to be `expect`s — a VM
    /// without GPU drivers or a mid-run device loss took the process down
    /// instead of returning an actionable error.
    fn fail(&mut self, event_loop: &ActiveEventLoop, err: ShellError) {
        // The web loop cannot return an error (`run_app` already returned):
        // surface the failure in the browser console before stopping.
        #[cfg(target_arch = "wasm32")]
        web_sys::console::error_1(&format!("fenestra: {err}").into());
        // iOS never returns from `run_app`, so nobody would read `fatal`:
        // the app would sit on a black screen. Panic instead, which puts
        // the error in the console and the crash report.
        #[cfg(target_os = "ios")]
        {
            let _ = event_loop;
            panic!("fenestra: {err}");
        }
        #[cfg(not(target_os = "ios"))]
        {
            if self.fatal.is_none() {
                self.fatal = Some(err);
            }
            event_loop.exit();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) -> Result<(), ShellError> {
        self.resumed_with(event_loop, |_, _| {})
    }

    /// Like [`Self::resumed`], but runs `before_visible` between window
    /// creation and the first `set_visible(true)` — the AccessKit adapter
    /// must attach while the window is still hidden.
    fn resumed_with(
        &mut self,
        event_loop: &ActiveEventLoop,
        before_visible: impl FnOnce(&ActiveEventLoop, &Arc<Window>),
    ) -> Result<(), ShellError> {
        let cached = match &mut self.state {
            RenderState::Suspended(cached_window) => cached_window.take(),
            _ => return Ok(()),
        };
        let window = match cached {
            Some(window) => window,
            None => {
                let geometry = window_geometry(&self.options, cfg!(target_os = "ios"));
                let attrs = Window::default_attributes()
                    .with_decorations(!self.options.borderless)
                    .with_title(self.options.title.clone())
                    .with_resizable(self.options.resizable)
                    .with_maximized(geometry.maximized)
                    .with_visible(false);
                let attrs = match geometry.inner_size {
                    Some((w, h)) => attrs.with_inner_size(LogicalSize::new(w, h)),
                    None => attrs,
                };
                let attrs = match geometry.min_size {
                    Some((w, h)) => attrs.with_min_inner_size(LogicalSize::new(w, h)),
                    None => attrs,
                };
                let attrs = if geometry.fullscreen {
                    attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(None)))
                } else {
                    attrs
                };
                // Position before fullscreen would be ignored; after it, the
                // window is placed and *then* filled, which is how a
                // fullscreen window lands on a chosen monitor. Winit has no
                // "fullscreen on the monitor at this point" — placing it there
                // first is the portable way to say it.
                let attrs = match geometry.position {
                    Some((x, y)) => attrs.with_position(LogicalPosition::new(x, y)),
                    None => attrs,
                };
                let attrs = attrs.with_transparent(self.options.transparent);
                let attrs = attrs.with_window_level(match self.options.always_on_top {
                    true => winit::window::WindowLevel::AlwaysOnTop,
                    false => winit::window::WindowLevel::Normal,
                });
                #[cfg(not(target_arch = "wasm32"))]
                let attrs = match self.options.icon.clone() {
                    Some((w, h, rgba)) => match winit::window::Icon::from_rgba(rgba, w, h) {
                        Ok(icon) => attrs.with_window_icon(Some(icon)),
                        // Malformed icon data: open without one, never panic.
                        Err(_) => attrs,
                    },
                    None => attrs,
                };
                #[cfg(target_arch = "wasm32")]
                let attrs = {
                    use winit::platform::web::WindowAttributesExtWebSys;
                    // winit creates the canvas; have it inserted into the page.
                    attrs.with_append(true)
                };
                Arc::new(
                    event_loop
                        .create_window(attrs)
                        .map_err(ShellError::WindowCreate)?,
                )
            }
        };
        before_visible(event_loop, &window);
        let was_hidden = needs_show(window.is_visible());
        self.activate(window.clone())?;
        if was_hidden {
            window.set_visible(true);
        }
        if std::env::var("FENESTRA_DEBUG_INPUT").is_ok() {
            let pos = window.outer_position().ok();
            let size = window.outer_size();
            eprintln!(
                "[fenestra-window] outer_position={pos:?} outer_size={size:?} decorated={}",
                window.is_decorated()
            );
        }
        Ok(())
    }

    /// Switches a transparent window's swapchain to a compositing alpha mode.
    ///
    /// **A transparent clear over an opaque swapchain is a black window, not a
    /// see-through one**, which is the whole failure this exists to avoid.
    /// vello's `create_surface` configures `CompositeAlphaMode::Auto`, and
    /// `Auto` means "whatever the platform prefers" — which is opaque wherever
    /// opaque is cheaper, i.e. almost everywhere.
    ///
    /// Asked of the adapter rather than assumed: `PreMultiplied` first because
    /// that is what vello's output already is, then `PostMultiplied`, and if
    /// the surface reports neither the window stays opaque. Nothing is
    /// reconfigured in that case — a swapchain configured with a mode it did
    /// not advertise is a validation error, and an opaque window is a worse
    /// picture but still a picture.
    #[cfg(not(target_arch = "wasm32"))]
    fn composite_with_alpha(&self, surface: &mut GpuSurface<'static>) {
        if !self.options.transparent {
            return;
        }
        let device = &self.context.devices[surface.dev_id];
        let modes = surface
            .surface
            .get_capabilities(device.adapter())
            .alpha_modes;
        let Some(mode) = [
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ]
        .into_iter()
        .find(|m| modes.contains(m)) else {
            return;
        };
        surface.config.alpha_mode = mode;
        surface.surface.configure(&device.device, &surface.config);
    }

    /// Builds (or rebuilds, after a lost surface) the swapchain for `window`
    /// and enters the active state.
    #[cfg(not(target_arch = "wasm32"))]
    fn activate(&mut self, window: Arc<Window>) -> Result<(), ShellError> {
        let size = crate::safe_area::surface_size(&window);
        #[cfg(target_os = "ios")]
        let parked = self.parked.take().map(|mut surface| {
            self.context
                .resize_surface(&mut surface, size.width.max(1), size.height.max(1));
            if std::env::var("FENESTRA_DEBUG_INPUT").is_ok() {
                eprintln!("[fenestra-window] resumed on the parked surface");
            }
            *surface
        });
        #[cfg(not(target_os = "ios"))]
        let parked = None;
        let mut surface = match parked {
            Some(surface) => surface,
            None => pollster::block_on(self.context.create_surface(
                window.clone(),
                size.width.max(1),
                size.height.max(1),
                wgpu::PresentMode::AutoVsync,
            ))
            .map_err(ShellError::Surface)?,
        };
        self.composite_with_alpha(&mut surface);

        let requested = crate::backend::backend_from_env();
        let device = self.context.devices[surface.dev_id].adapter();
        let adapter = device.get_info().backend;
        let indirect = device
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        let backend = crate::backend::window_backend(
            requested,
            adapter,
            indirect,
            crate::headless::cpu_compute_requested(),
        );
        match (requested, backend) {
            (crate::Backend::Raster, crate::Backend::Vello) => {
                crate::backend::warn_raster_fallback(adapter);
            }
            (crate::Backend::Vello, crate::Backend::Raster) => {
                crate::backend::warn_vello_fallback();
            }
            _ => {}
        }
        if backend == crate::Backend::Raster
            && self
                .raster
                .as_ref()
                .is_none_or(|r| r.dev_id != surface.dev_id)
        {
            self.raster = Some(RasterState {
                dev_id: surface.dev_id,
                renderer: fenestra_raster::RasterRenderer::new(
                    &self.context.devices[surface.dev_id].device,
                    TextureFormat::Rgba8Unorm,
                ),
                target: None,
            });
        }

        // vello is built only to draw with it (or for a scene runner's or a
        // secondary window's first frame): a raster window never pays for
        // compiling its compute pipelines, nor fails with them.
        if backend != crate::Backend::Raster {
            self.ensure_vello(surface.dev_id)?;
        }
        self.state = RenderState::Active {
            surface: Box::new(surface),
            valid_surface: size.width != 0 && size.height != 0,
            window,
        };
        Ok(())
    }

    /// Builds vello's renderer for device `dev_id` if it has none yet.
    #[cfg(not(target_arch = "wasm32"))]
    fn ensure_vello(&mut self, dev_id: usize) -> Result<(), ShellError> {
        self.renderers
            .resize_with(self.context.devices.len(), || None);
        if self.renderers[dev_id].is_none() {
            self.renderers[dev_id] = Some(
                Renderer::new(
                    &self.context.devices[dev_id].device,
                    RendererOptions {
                        // `FENESTRA_CPU` runs vello's compute stages on the
                        // CPU — the live-window switch for software adapters
                        // (see `CPU_ENV`).
                        use_cpu: crate::headless::cpu_compute_requested(),
                        antialiasing_support: AaSupport::area_only(),
                        ..Default::default()
                    },
                )
                .map_err(ShellError::Vello)?,
            );
        }
        Ok(())
    }

    /// The web builds vello's renderer when its surface lands ([`Self::pump`]).
    #[cfg(target_arch = "wasm32")]
    fn ensure_vello(&mut self, _dev_id: usize) -> Result<(), ShellError> {
        Ok(())
    }

    /// The active surface's device.
    fn surface_device(&self) -> Option<usize> {
        match &self.state {
            RenderState::Active { surface, .. } => Some(surface.dev_id),
            _ => None,
        }
    }

    /// Web: surface/device setup is async — kick it off and park in
    /// `Pending`; [`Self::pump`] finishes the activation when it lands.
    /// Setup failure (no WebGPU) travels through the `ready` slot and
    /// surfaces on the next pump. Always `Ok` here.
    #[cfg(target_arch = "wasm32")]
    fn activate(&mut self, window: Arc<Window>) -> Result<(), ShellError> {
        let size = window.inner_size();
        let ready = std::rc::Rc::clone(&self.ready);
        let win = window.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut context = GpuContext::new();
            let surface = context
                .create_surface(
                    win.clone(),
                    size.width.max(1),
                    size.height.max(1),
                    wgpu::PresentMode::AutoVsync,
                )
                .await
                .map(|surface| (context, Box::new(surface)))
                .map_err(ShellError::Surface);
            *ready.borrow_mut() = Some(surface);
            win.request_redraw();
        });
        self.state = RenderState::Pending(window);
        Ok(())
    }

    /// Completes a pending web activation once the async setup finished.
    /// No-op on native and while nothing is pending.
    fn pump(&mut self) -> Result<(), ShellError> {
        #[cfg(target_arch = "wasm32")]
        if let RenderState::Pending(window) = &self.state
            && let Some(setup) = self.ready.borrow_mut().take()
        {
            let (context, surface) = setup?;
            let window = window.clone();
            self.context = context;
            self.renderers.clear();
            self.renderers
                .resize_with(self.context.devices.len(), || None);
            if self.renderers[surface.dev_id].is_none() {
                self.renderers[surface.dev_id] = Some(
                    Renderer::new(
                        &self.context.devices[surface.dev_id].device,
                        RendererOptions {
                            // No env vars on the web; WebGPU implies a real
                            // adapter, so the CPU pipeline has no use here.
                            use_cpu: false,
                            antialiasing_support: AaSupport::area_only(),
                            ..Default::default()
                        },
                    )
                    .map_err(ShellError::Vello)?,
                );
            }
            let size = window.inner_size();
            self.state = RenderState::Active {
                surface,
                valid_surface: size.width != 0 && size.height != 0,
                window,
            };
        }
        Ok(())
    }

    fn suspended(&mut self) {
        if let RenderState::Active { window, .. } = &self.state {
            let window = window.clone();
            let state = std::mem::replace(&mut self.state, RenderState::Suspended(Some(window)));
            #[cfg(target_os = "ios")]
            if let RenderState::Active { surface, .. } = state {
                self.parked = Some(surface);
            }
            #[cfg(not(target_os = "ios"))]
            drop(state);
        }
    }

    fn window(&self) -> Option<&Arc<Window>> {
        match &self.state {
            RenderState::Active { window, .. } => Some(window),
            _ => None,
        }
    }

    fn resized(&mut self, width: u32, height: u32) {
        let RenderState::Active {
            surface,
            valid_surface,
            window,
        } = &mut self.state
        else {
            return;
        };
        if width != 0 && height != 0 {
            self.context.resize_surface(surface, width, height);
            *valid_surface = true;
        } else {
            *valid_surface = false;
        }
        window.request_redraw();
    }

    /// Logical size and scale factor of the active window.
    fn logical_size(&self) -> Option<(f64, f64, f64)> {
        match &self.state {
            RenderState::Active {
                surface, window, ..
            } => {
                let scale = window.scale_factor();
                Some((
                    f64::from(surface.config.width) / scale,
                    f64::from(surface.config.height) / scale,
                    scale,
                ))
            }
            _ => None,
        }
    }

    /// Scales the logical fragment to physical pixels and presents it.
    /// Recoverable surface states (lost/outdated/occluded/timeout) are
    /// handled in place; anything else is an error for [`Self::fail`].
    #[cfg(not(target_arch = "wasm32"))]
    fn present(&mut self, fragment: &Scene) -> Result<(), ShellError> {
        if let Some(dev_id) = self.surface_device() {
            self.ensure_vello(dev_id)?;
        }
        let RenderState::Active {
            surface,
            valid_surface,
            window,
        } = &mut self.state
        else {
            return Ok(());
        };
        if !*valid_surface {
            return Ok(());
        }
        let width = surface.config.width;
        let height = surface.config.height;
        let scale = window.scale_factor();

        self.scene.reset();
        self.scene
            .append(fragment, Some(vello::kurbo::Affine::scale(scale)));

        let handle = &self.context.devices[surface.dev_id];
        self.renderers[surface.dev_id]
            .as_mut()
            .expect("renderer exists for surface device")
            .render_to_texture(
                &handle.device,
                &handle.queue,
                &self.scene,
                &surface.target_view,
                &RenderParams {
                    // A transparent window clears to nothing regardless of
                    // the theme: `background` is refreshed from `theme.bg`
                    // whenever the theme changes, and an opaque clear would
                    // paint over the very thing the window was made to show.
                    base_color: match self.transparent {
                        true => Color::TRANSPARENT,
                        false => self.background,
                    },
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(ShellError::Vello)?;

        let surface_texture = match surface.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(texture) => texture,
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Suboptimal(_) => {
                self.context.configure_surface(surface);
                window.request_redraw();
                return Ok(());
            }
            CurrentSurfaceTexture::Occluded => {
                // Hidden window: skip the frame; WindowEvent::Occluded(false)
                // requests the next redraw when it becomes visible again.
                return Ok(());
            }
            CurrentSurfaceTexture::Timeout => {
                window.request_redraw();
                return Ok(());
            }
            CurrentSurfaceTexture::Lost => {
                // Recoverable (GPU reset, driver update, display change):
                // rebuild the swapchain on the same window and repaint.
                let window = window.clone();
                window.request_redraw();
                return self.activate(window);
            }
            CurrentSurfaceTexture::Validation => return Err(ShellError::SurfaceValidation),
        };

        let mut encoder = handle
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fenestra surface blit"),
            });
        surface.blitter.copy(
            &handle.device,
            &mut encoder,
            &surface.target_view,
            &surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        handle.queue.submit([encoder.finish()]);
        surface_texture.present();
        handle
            .device
            .poll(wgpu::PollType::Poll)
            .map_err(ShellError::Poll)?;
        Ok(())
    }

    /// Renders `scene` at the given pixel size to the off-screen backdrop
    /// texture — the two-pass path's first pass (every glass subtree
    /// skipped). The GPU glass reads it in place; [`Self::read_region`]
    /// reads it back for the CPU filters. Returns `None` when no surface is
    /// active (window not yet resumed or already suspended).
    #[cfg(not(target_arch = "wasm32"))]
    fn render_region(
        &mut self,
        scene: &Scene,
        width: u32,
        height: u32,
        base_color: Color,
    ) -> Option<Result<(), ShellError>> {
        if let Err(e) = self.ensure_vello(self.surface_device()?) {
            return Some(Err(e));
        }
        let RenderState::Active { surface, .. } = &self.state else {
            return None;
        };
        let handle = &self.context.devices[surface.dev_id];
        let renderer = self.renderers[surface.dev_id].as_mut()?;
        let (device, queue) = (&handle.device, &handle.queue);

        let dev_id = surface.dev_id;
        let reusable = self
            .readback
            .as_ref()
            .is_some_and(|r| (r.dev_id, r.width, r.height) == (dev_id, width, height));
        if !reusable {
            let texture = crate::vello_glass::render_target(device, (width, height));
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            // wgpu requires copy rows padded to 256 bytes.
            let padded_byte_width = (width * 4).next_multiple_of(256);
            let buffer = device.create_buffer(&BufferDescriptor {
                label: Some("fenestra windowed readback"),
                size: u64::from(padded_byte_width) * u64::from(height),
                usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.readback = Some(Readback {
                dev_id,
                width,
                height,
                view,
                texture,
                buffer,
                padded_byte_width,
            });
        }
        let Readback { view, .. } = self.readback.as_ref()?;
        if let Err(e) = renderer.render_to_texture(
            device,
            queue,
            scene,
            view,
            &RenderParams {
                base_color,
                width,
                height,
                antialiasing_method: AaConfig::Area,
            },
        ) {
            return Some(Err(ShellError::Vello(e)));
        }
        Some(Ok(()))
    }

    /// Reads the backdrop texture [`Self::render_region`] rendered back as an
    /// [`RgbaImage`], for the CPU filters.
    #[cfg(not(target_arch = "wasm32"))]
    fn read_region(&self) -> Option<Result<RgbaImage, ShellError>> {
        let RenderState::Active { surface, .. } = &self.state else {
            return None;
        };
        let handle = &self.context.devices[surface.dev_id];
        let (device, queue) = (&handle.device, &handle.queue);
        let Readback {
            texture: target,
            buffer,
            padded_byte_width,
            width,
            height,
            ..
        } = self.readback.as_ref()?;
        let (padded_byte_width, width, height) = (*padded_byte_width, *width, *height);
        let size = Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("fenestra windowed readback copy"),
        });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_byte_width),
                    rows_per_image: None,
                },
            },
            size,
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        if let Err(e) = device.poll(wgpu::PollType::wait_indefinitely()) {
            return Some(Err(ShellError::Poll(e)));
        }
        let map_result = match rx.recv() {
            Ok(r) => r,
            Err(_) => return Some(Err(ShellError::Readback)),
        };
        if map_result.is_err() {
            return Some(Err(ShellError::Readback));
        }
        let data = slice.get_mapped_range();
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * padded_byte_width) as usize;
            pixels.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        drop(data);
        buffer.unmap();

        Some(Ok(RgbaImage::from_raw(width, height, pixels)
            .expect("readback buffer matches image dimensions")))
    }

    /// The surface's adapter backend.
    #[cfg(not(target_arch = "wasm32"))]
    fn surface_backend(&self) -> Option<wgpu::Backend> {
        let RenderState::Active { surface, .. } = &self.state else {
            return None;
        };
        Some(
            self.context.devices[surface.dev_id]
                .adapter()
                .get_info()
                .backend,
        )
    }

    /// Composites glass `panes` on the GPU from the backdrop texture
    /// [`Self::render_region`] rendered (its top-left at `origin` in the
    /// frame), handing each result to vello; returns their placeholders.
    #[cfg(not(target_arch = "wasm32"))]
    fn vello_glass(
        &mut self,
        origin: (u32, u32),
        panes: &[fenestra_raster::GlassPane],
    ) -> Option<std::collections::HashMap<fenestra_core::WidgetId, vello::peniko::ImageData>> {
        let RenderState::Active { surface, .. } = &self.state else {
            return None;
        };
        let handle = &self.context.devices[surface.dev_id];
        let source = &self.readback.as_ref()?.texture;
        Some(self.vello_glass.composite(
            (surface.dev_id, &handle.device, &handle.queue),
            &mut self.renderers,
            source,
            origin,
            panes,
        ))
    }

    /// Withdraws this window's GPU glass from vello (a frame that draws none).
    #[cfg(not(target_arch = "wasm32"))]
    fn release_vello_glass(&mut self) {
        if self.vello_glass.registered() > 0 {
            self.vello_glass.release(&mut self.renderers);
        }
    }

    /// The final scene of a two-pass vello frame (physical coordinates): the
    /// backdrop region rendered, glass composited on the GPU where the
    /// adapter and `FENESTRA_GPU_GLASS` allow (only the other filters read
    /// back), else every region read back and filtered on the CPU.
    /// `record` notes the phases in the frame stats. `None` with no surface.
    #[cfg(not(target_arch = "wasm32"))]
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame and the window's caches"
    )]
    fn two_pass_scene(
        &mut self,
        frame: &fenestra_core::Frame,
        specs: &[fenestra_core::MultiPassSpec],
        backdrop: &Scene,
        (fonts, state): (&mut Fonts, &mut FrameState),
        base_color: Color,
        custom: (
            &crate::multi_pass::CustomRender,
            &mut std::collections::HashMap<(u64, u64), vello::peniko::ImageData>,
        ),
        record: bool,
    ) -> Option<Result<Scene, ShellError>> {
        let scale = frame.scale();
        let (pw, ph) = self.physical_size()?;
        let note = |phase, d| {
            if record {
                crate::frame_stats::record(phase, d);
            }
        };
        let (region, origin_scene) =
            crate::multi_pass::region_scene(backdrop, specs, scale, (pw, ph));
        let gpu = crate::backend::gpu_glass_enabled()
            && self
                .surface_backend()
                .is_some_and(crate::backend::raster_supported);
        let (panes, mut rest) = if gpu {
            crate::multi_pass::glass_panes(specs, frame, scale, (pw, ph))
        } else {
            (Vec::new(), specs.to_vec())
        };
        if panes.is_empty() {
            self.release_vello_glass();
        }
        let tr = Instant::now();
        if let Err(e) = self.render_region(&origin_scene, region.2, region.3, base_color)? {
            return Some(Err(e));
        }
        let tg = Instant::now();
        let mut injected = if panes.is_empty() {
            std::collections::HashMap::new()
        } else {
            self.vello_glass((region.0, region.1), &panes)?
        };
        let baked: std::collections::HashSet<_> = injected.keys().copied().collect();
        if !panes.is_empty() {
            note(crate::frame_stats::Phase::Filters, tg.elapsed());
            rest.extend(crate::multi_pass::uncomposited(
                specs,
                &injected,
                scale,
                (pw, ph),
            ));
        }
        if !rest.is_empty() {
            let backdrop = match self.read_region()? {
                Ok(image) => image,
                Err(e) => return Some(Err(e)),
            };
            note(crate::frame_stats::Phase::Readback, tr.elapsed());
            let tf = Instant::now();
            injected.extend(crate::multi_pass::process_specs_in(
                &backdrop,
                (region.0, region.1),
                (pw, ph),
                &rest,
                scale,
                custom.0,
                custom.1,
                &mut self.glass_cache,
            ));
            note(crate::frame_stats::Phase::Filters, tf.elapsed());
        }
        Some(Ok(Self::at_scale(
            crate::multi_pass::paint_final_baked(frame, fonts, state, &injected, &baked),
            scale,
        )))
    }

    /// Presents a scene already in physical-pixel coordinates (no logical→physical
    /// scale transform applied). Used by the two-pass render path: `paint_backdrop`
    /// and `paint_final` return logical-space scenes that the two-pass caller
    /// pre-scales via [`Self::at_scale`] before passing here.
    fn present_physical(&mut self, fragment: &Scene) -> Result<(), ShellError> {
        if let Some(dev_id) = self.surface_device() {
            self.ensure_vello(dev_id)?;
        }
        let RenderState::Active {
            surface,
            valid_surface,
            window,
        } = &mut self.state
        else {
            return Ok(());
        };
        if !*valid_surface {
            return Ok(());
        }
        let width = surface.config.width;
        let height = surface.config.height;

        self.scene.reset();
        self.scene.append(fragment, None);

        let handle = &self.context.devices[surface.dev_id];
        let t = Instant::now();
        self.renderers[surface.dev_id]
            .as_mut()
            .expect("renderer exists for surface device")
            .render_to_texture(
                &handle.device,
                &handle.queue,
                &self.scene,
                &surface.target_view,
                &RenderParams {
                    base_color: match self.transparent {
                        true => Color::TRANSPARENT,
                        false => self.background,
                    },
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
            .map_err(ShellError::Vello)?;
        crate::frame_stats::record(crate::frame_stats::Phase::Render, t.elapsed());
        if crate::frame_stats::sync() {
            let t = Instant::now();
            handle
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(ShellError::Poll)?;
            crate::frame_stats::record(crate::frame_stats::Phase::Gpu, t.elapsed());
        }

        let t = Instant::now();
        let surface_texture = match surface.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(texture) => texture,
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Suboptimal(_) => {
                self.context.configure_surface(surface);
                window.request_redraw();
                return Ok(());
            }
            CurrentSurfaceTexture::Occluded => return Ok(()),
            CurrentSurfaceTexture::Timeout => {
                window.request_redraw();
                return Ok(());
            }
            CurrentSurfaceTexture::Lost => {
                let window = window.clone();
                window.request_redraw();
                return self.activate(window);
            }
            CurrentSurfaceTexture::Validation => return Err(ShellError::SurfaceValidation),
        };

        crate::frame_stats::record(crate::frame_stats::Phase::Acquire, t.elapsed());
        let t = Instant::now();
        let mut encoder = handle
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fenestra surface blit (physical)"),
            });
        surface.blitter.copy(
            &handle.device,
            &mut encoder,
            &surface.target_view,
            &surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        handle.queue.submit([encoder.finish()]);
        surface_texture.present();
        handle
            .device
            .poll(wgpu::PollType::Poll)
            .map_err(ShellError::Poll)?;
        crate::frame_stats::record(crate::frame_stats::Phase::Present, t.elapsed());
        Ok(())
    }

    /// Whether this window draws through the raster backend.
    fn uses_raster(&self) -> bool {
        self.raster.is_some()
    }

    /// The raster target's size (if built) and the surface's, while a
    /// surface is active.
    fn raster_sizes(&self) -> Option<(Option<PxSize>, PxSize)> {
        let RenderState::Active { surface, .. } = &self.state else {
            return None;
        };
        let target = self.raster.as_ref()?.target.as_ref().map(|t| t.size);
        Some((target, (surface.config.width, surface.config.height)))
    }

    /// The active surface's size in physical pixels.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "two-pass renders are native-only")
    )]
    fn physical_size(&self) -> Option<(u32, u32)> {
        match &self.state {
            RenderState::Active { surface, .. } => {
                Some((surface.config.width, surface.config.height))
            }
            _ => None,
        }
    }

    /// Records a frame with `paint` and renders it into the raster target
    /// (the surface's size); returns what `paint` returned and how long the
    /// GPU encoding took. `None` with no drawable surface.
    fn raster_render<R>(
        &mut self,
        paint: impl FnOnce(&mut fenestra_raster::RasterFrame<'_>) -> R,
    ) -> Option<(R, Duration)> {
        let RenderState::Active {
            surface,
            valid_surface: true,
            ..
        } = &self.state
        else {
            return None;
        };
        let size = (surface.config.width, surface.config.height);
        let handle = &self.context.devices[surface.dev_id];
        let raster = self.raster.as_mut()?;
        if needs_new_target(raster.target.as_ref().map(|t| t.size), size) {
            let texture = handle.device.create_texture(&TextureDescriptor {
                label: Some("fenestra raster target"),
                size: Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TextureFormat::Rgba8Unorm,
                usage: TextureUsages::RENDER_ATTACHMENT
                    | TextureUsages::TEXTURE_BINDING
                    | TextureUsages::COPY_SRC,
                // The GPU glass reads it in linear light.
                view_formats: &[TextureFormat::Rgba8UnormSrgb],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            raster.target = Some(RasterTarget {
                size,
                texture,
                view,
            });
        }
        let target = raster.target.as_ref().expect("just ensured");
        let base = if self.transparent {
            Color::TRANSPARENT
        } else {
            self.background
        };
        let mut frame = raster.renderer.frame(size.0, size.1);
        let out = paint(&mut frame);
        let t = Instant::now();
        frame.render(&handle.device, &handle.queue, &target.view, base);
        Some((out, t.elapsed()))
    }

    /// Composites glass `panes` on the GPU straight from the raster target
    /// (no readback); returns each pane's placeholder image for the final
    /// pass. `None` with no drawable surface.
    #[cfg(not(target_arch = "wasm32"))]
    fn raster_glass(
        &mut self,
        panes: &[fenestra_raster::GlassPane],
    ) -> Option<std::collections::HashMap<fenestra_core::WidgetId, vello::peniko::ImageData>> {
        let RenderState::Active { surface, .. } = &self.state else {
            return None;
        };
        let handle = &self.context.devices[surface.dev_id];
        let raster = self.raster.as_mut()?;
        let target = raster.target.as_ref()?;
        let (out, _) = raster
            .renderer
            .glass(&handle.device, &handle.queue, &target.texture, panes);
        Some(out)
    }

    /// Reads the `(x, y, width, height)` part of the raster target back (the
    /// backdrop of a two-pass frame).
    #[cfg(not(target_arch = "wasm32"))]
    fn raster_read(&self, region: (u32, u32, u32, u32)) -> Option<Result<RgbaImage, ShellError>> {
        let RenderState::Active { surface, .. } = &self.state else {
            return None;
        };
        let handle = &self.context.devices[surface.dev_id];
        let target = self.raster.as_ref()?.target.as_ref()?;
        Some(crate::headless::read_texture(
            &handle.device,
            &handle.queue,
            &target.texture,
            region,
        ))
    }

    /// Blits the raster target to the swapchain and presents it.
    fn raster_present(&mut self) -> Result<(), ShellError> {
        let RenderState::Active {
            surface,
            valid_surface,
            window,
        } = &mut self.state
        else {
            return Ok(());
        };
        let Some(target) = self.raster.as_ref().and_then(|r| r.target.as_ref()) else {
            return Ok(());
        };
        if !*valid_surface || target.size != (surface.config.width, surface.config.height) {
            return Ok(());
        }
        let handle = &self.context.devices[surface.dev_id];
        if crate::frame_stats::sync() {
            let t = Instant::now();
            handle
                .device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(ShellError::Poll)?;
            crate::frame_stats::record(crate::frame_stats::Phase::Gpu, t.elapsed());
        }
        let t = Instant::now();
        let surface_texture = match surface.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(texture) => texture,
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Suboptimal(_) => {
                self.context.configure_surface(surface);
                window.request_redraw();
                return Ok(());
            }
            CurrentSurfaceTexture::Occluded => return Ok(()),
            CurrentSurfaceTexture::Timeout => {
                window.request_redraw();
                return Ok(());
            }
            CurrentSurfaceTexture::Lost => {
                let window = window.clone();
                window.request_redraw();
                return self.activate(window);
            }
            CurrentSurfaceTexture::Validation => return Err(ShellError::SurfaceValidation),
        };
        crate::frame_stats::record(crate::frame_stats::Phase::Acquire, t.elapsed());
        let t = Instant::now();
        let mut encoder = handle
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fenestra raster blit"),
            });
        surface.blitter.copy(
            &handle.device,
            &mut encoder,
            &target.view,
            &surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
        );
        handle.queue.submit([encoder.finish()]);
        surface_texture.present();
        handle
            .device
            .poll(wgpu::PollType::Poll)
            .map_err(ShellError::Poll)?;
        crate::frame_stats::record(crate::frame_stats::Phase::Present, t.elapsed());
        Ok(())
    }

    /// Wraps a logical-space scene for a physical target. Returns the scene
    /// untouched at scale 1.0 so reference goldens stay byte-identical.
    fn at_scale(scene: Scene, scale: f64) -> Scene {
        if (scale - 1.0).abs() < f64::EPSILON {
            return scene;
        }
        let mut scaled = Scene::new();
        scaled.append(&scene, Some(vello::kurbo::Affine::scale(scale)));
        scaled
    }
}

/// Builds the event loop for a windowed runner. On Android, winit requires
/// the `AndroidApp` from `android_main` so the loop can bind to the activity.
fn build_event_loop<E>(
    options: &WindowOptions,
) -> Result<EventLoop<E>, winit::error::EventLoopError> {
    // Read on every target: the field it carries today is cfg'd out
    // everywhere except Android.
    let _ = options;
    let mut builder = EventLoop::<E>::with_user_event();
    #[cfg(target_os = "android")]
    if let Some(app) = options.android_app.clone() {
        builder.with_android_app(app);
    }
    builder.build()
}

// ------------------------------------------------------------- run_scene

/// Opens a window and repaints via `paint(scene, logical_w, logical_h, bg)`
/// on every redraw. Blocks until the window closes. Low-level escape hatch;
/// element views should prefer [`run_static`] (or the M4 `App` runner).
#[cfg(not(target_arch = "wasm32"))]
pub fn run_scene(
    options: WindowOptions,
    background: Color,
    paint: impl FnMut(&mut Scene, f64, f64, Color) + 'static,
) -> Result<(), ShellError> {
    let event_loop = build_event_loop(&options).map_err(ShellError::EventLoop)?;
    let mut app = SceneApp {
        shell: WindowShell::new(options, background),
        fragment: Scene::new(),
        paint: Box::new(paint),
    };
    let run = event_loop.run_app(&mut app).map_err(ShellError::EventLoop);
    match app.shell.fatal.take() {
        Some(err) => Err(err),
        None => run,
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct SceneApp {
    shell: WindowShell,
    fragment: Scene,
    paint: PaintFn,
}

#[cfg(not(target_arch = "wasm32"))]
impl ApplicationHandler for SceneApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(e) = self.shell.resumed(event_loop) {
            self.shell.fail(event_loop, e);
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.shell.suspended();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.shell.window().is_none_or(|w| w.id() != window_id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.shell.resized(size.width, size.height),
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(w) = self.shell.window() {
                    w.request_redraw();
                }
            }
            WindowEvent::Occluded(occluded) => {
                if !occluded && let Some(w) = self.shell.window() {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let Some((lw, lh, _scale)) = self.shell.logical_size() else {
                    return;
                };
                self.fragment.reset();
                let bg = self.shell.background;
                (self.paint)(&mut self.fragment, lw, lh, bg);
                let fragment = std::mem::replace(&mut self.fragment, Scene::new());
                if let Err(e) = self.shell.present(&fragment) {
                    self.shell.fail(event_loop, e);
                }
                self.fragment = fragment;
            }
            _ => {}
        }
    }
}

/// A [`FrameState`] for a live window, seeded with the OS "reduce motion"
/// accessibility setting so animations snap for users who asked for it.
#[cfg(not(target_arch = "wasm32"))]
fn live_state() -> FrameState {
    let mut state = FrameState::new();
    state.reduced_motion = crate::reduce_motion::os_reduce_motion();
    state
}

/// The wasm build reads `prefers-reduced-motion` through the browser, so the
/// live state is the plain default here.
#[cfg(target_arch = "wasm32")]
fn live_state() -> FrameState {
    FrameState::new()
}

// ------------------------------------------------------------- run_static

/// Opens a window showing a message-free element view. The view is rebuilt
/// on every redraw; scroll state persists in a [`FrameState`]. Blocks until
/// the window closes.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_static(
    options: WindowOptions,
    theme: Theme,
    view: impl Fn(&Theme) -> Element<()> + 'static,
) -> Result<(), ShellError> {
    let event_loop = build_event_loop(&options).map_err(ShellError::EventLoop)?;
    let background = theme.bg;
    let mut fonts = Fonts::with_system();
    for (role, data) in &options.fonts {
        fonts.register(*role, data.clone());
    }
    fonts.set_locale(options.ui_locale().as_deref());
    let mut app = StaticApp {
        shell: WindowShell::new(options, background),
        theme,
        fonts,
        state: live_state(),
        view: Box::new(view),
        cursor: Point::ORIGIN,
        started: Instant::now(),
        last_frame: None,
    };
    let run = event_loop.run_app(&mut app).map_err(ShellError::EventLoop);
    match app.shell.fatal.take() {
        Some(err) => Err(err),
        None => run,
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct StaticApp {
    shell: WindowShell,
    theme: Theme,
    fonts: Fonts,
    state: FrameState,
    view: ViewFn,
    /// Cursor position in logical coordinates.
    cursor: Point,
    started: Instant,
    /// The frame from the last redraw, used to route input between frames.
    last_frame: Option<fenestra_core::Frame>,
}

#[cfg(not(target_arch = "wasm32"))]
impl StaticApp {
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let frame_started = Instant::now();
        let Some((lw, lh, scale)) = self.shell.logical_size() else {
            return;
        };
        self.state.tick(self.started.elapsed().as_secs_f64());
        let el = (self.view)(&self.theme);
        #[expect(clippy::cast_possible_truncation, reason = "window sizes fit in f32")]
        let frame = build_frame(
            &el,
            &self.theme,
            &mut self.fonts,
            &mut self.state,
            (lw as f32, lh as f32),
            scale,
        );
        // Live-window limitation: the swapchain path renders a single pass, so
        // frosted glass shows its translucent tint without the CPU backdrop
        // blur (which needs a read-back). Headless rendering — the golden source
        // of truth — uses the two-pass `render_plan`. See ARCHITECTURE.md
        // ("Real frosted-glass backdrop blur").
        let scene = frame.paint(&mut self.fonts, &mut self.state);
        if let Err(e) = self.shell.present(&scene) {
            self.shell.fail(event_loop, e);
            return;
        }
        if frame.animating {
            event_loop
                .set_control_flow(ControlFlow::WaitUntil(next_animation_frame(frame_started)));
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
        self.last_frame = Some(frame);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ApplicationHandler for StaticApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(e) = self.shell.resumed(event_loop) {
            self.shell.fail(event_loop, e);
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.shell.suspended();
    }

    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        if matches!(cause, StartCause::ResumeTimeReached { .. })
            && let Some(w) = self.shell.window()
        {
            w.request_redraw();
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.shell.window().is_none_or(|w| w.id() != window_id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.shell.resized(size.width, size.height),
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(w) = self.shell.window() {
                    w.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.shell.window().map_or(1.0, |w| w.scale_factor());
                self.cursor = Point::new(position.x / scale, position.y / scale);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = self.shell.window().map_or(1.0, |w| w.scale_factor());
                let (dx, dy) = wheel_deltas(delta, scale);
                if let Some(frame) = &self.last_frame {
                    let y_target = (dy.abs() > 1e-3)
                        .then(|| frame.scrollable_y_at(self.cursor))
                        .flatten();
                    let x_target = (dx.abs() > 1e-3)
                        .then(|| frame.scrollable_x_at(self.cursor))
                        .flatten();
                    #[expect(clippy::cast_possible_truncation, reason = "scroll deltas fit in f32")]
                    {
                        if let Some(id) = y_target {
                            self.state.scroll_by(id, -dy as f32);
                        }
                        if let Some(id) = x_target {
                            self.state.scroll_by_x(id, -dx as f32);
                        }
                    }
                    if (y_target.is_some() || x_target.is_some())
                        && let Some(w) = self.shell.window()
                    {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }
}

// ------------------------------------------------------------- run_app

/// User events crossing into the app runner's loop: type-erased app
/// messages from a [`fenestra_core::Proxy`] (any thread), and AccessKit's
/// activation/action events.
enum RunnerEvent {
    App(Box<dyn std::any::Any + Send>),
    #[cfg(not(target_arch = "wasm32"))]
    Access(accesskit_winit::Event),
    /// A native menu item was chosen (the muda item id).
    #[cfg(target_os = "macos")]
    Menu(String),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<accesskit_winit::Event> for RunnerEvent {
    fn from(event: accesskit_winit::Event) -> Self {
        Self::Access(event)
    }
}

/// Runs an [`App`]: the full Elm-shaped loop with hit testing, hover/active/
/// focus, keyboard navigation, message dispatch, and event-driven repaint
/// (animation frames only while something animates). Calls [`App::init`]
/// with a [`fenestra_core::Proxy`] before the first frame; proxied messages
/// wake the loop and repaint. Blocks until the window closes.
pub fn run_app<A: App + 'static>(mut app: A, options: WindowOptions) -> Result<(), ShellError>
where
    A::Msg: Send,
{
    let event_loop = build_event_loop::<RunnerEvent>(&options).map_err(ShellError::EventLoop)?;
    #[cfg(not(target_arch = "wasm32"))]
    let access_proxy = event_loop.create_proxy();
    let proxy = event_loop.create_proxy();
    // Dropped silently once the loop is gone (window closed).
    let msg_proxy = fenestra_core::Proxy::new(move |msg: A::Msg| {
        let _ = proxy.send_event(RunnerEvent::App(Box::new(msg)));
    });
    app.init(msg_proxy.clone());
    let background = app.theme().bg;
    let mut fonts = Fonts::with_system();
    for (role, data) in &options.fonts {
        fonts.register(*role, data.clone());
    }
    fonts.set_locale(options.ui_locale().as_deref());
    let mut state = live_state();
    #[cfg(not(target_arch = "wasm32"))]
    state.set_clipboard(Box::new(os_clipboard(&options)));
    // On the web, copy-out reaches the system clipboard; paste-in from
    // other apps stays in-app (see `WebClipboard`).
    #[cfg(target_arch = "wasm32")]
    state.set_clipboard(Box::new(crate::WebClipboard::default()));
    let custom_render = app.custom_render();
    let runner = AppRunner {
        shell: WindowShell::new(options, background),
        msg_proxy,
        #[cfg(not(target_arch = "wasm32"))]
        subs: std::collections::HashMap::new(),
        #[cfg(target_os = "macos")]
        menu: None,
        app,
        fonts,
        state,
        cursor: Point::ORIGIN,
        touch: crate::touch::Touch::new(crate::touch::GESTURES),
        #[cfg(any(target_os = "android", target_os = "ios"))]
        keyboard_shown: false,
        #[cfg(any(target_os = "android", target_os = "ios"))]
        keyboard_tap: false,
        #[cfg(any(target_os = "android", target_os = "ios"))]
        touch_down: None,
        #[cfg(all(target_os = "android", feature = "android-game-activity"))]
        ime: crate::ime_sync::ImeSync::default(),
        #[cfg(all(target_os = "ios", feature = "ios-text-input"))]
        ime: crate::ime_sync::ImeSync::synchronous(),
        #[cfg(all(target_os = "ios", feature = "ios-text-input"))]
        ios_traits: None,
        started: Instant::now(),
        last: None,
        dirty: true,
        cached_scene: None,
        raster_key: None,
        modifiers: winit::keyboard::ModifiersState::empty(),
        #[cfg(not(target_arch = "wasm32"))]
        adapter: None,
        #[cfg(not(target_arch = "wasm32"))]
        proxy: access_proxy,
        #[cfg(not(target_arch = "wasm32"))]
        secondary: std::collections::HashMap::new(),
        custom_cache: std::collections::HashMap::new(),
        custom_render,
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut runner = runner;
        // Startup effect + initial subscriptions: results arrive as user
        // events once the loop is running.
        let cmd = runner.app.init_cmd();
        runner.run_cmd(cmd);
        runner.reconcile_subs();
        // muda is a macOS-only dependency (Cargo.toml), so this block is too.
        #[cfg(target_os = "macos")]
        {
            let menu_proxy = event_loop.create_proxy();
            muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
                let _ = menu_proxy.send_event(RunnerEvent::Menu(event.id.0.clone()));
            }));
            runner.reconcile_menu();
        }
        let run = event_loop
            .run_app(&mut runner)
            .map_err(ShellError::EventLoop);
        match runner.shell.fatal.take() {
            Some(err) => Err(err),
            None => run,
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        let mut runner = runner;
        let cmd = runner.app.init_cmd();
        runner.run_cmd(cmd);
        // Non-blocking on the web: the loop keeps running after main returns.
        event_loop.spawn_app(runner);
        Ok(())
    }
}

struct AppRunner<A: App> {
    shell: WindowShell,
    app: A,
    fonts: Fonts,
    state: FrameState,
    /// Message proxy shared with [`App::init`]: effect results and
    /// subscription ticks are delivered through it, waking the loop.
    msg_proxy: fenestra_core::Proxy<A::Msg>,
    /// Running subscription timers, reconciled against
    /// [`App::subscriptions`] after every update (native only).
    #[cfg(not(target_arch = "wasm32"))]
    subs: std::collections::HashMap<String, SubHandle>,
    /// The attached native menu, reconciled against [`App::menu`] by
    /// structural fingerprint (macOS only).
    #[cfg(target_os = "macos")]
    menu: Option<MenuHandle<A::Msg>>,
    cursor: Point,
    /// The touch driving the pointer.
    touch: crate::touch::Touch,
    /// Whether the soft keyboard was last asked to show (Android, iOS).
    #[cfg(any(target_os = "android", target_os = "ios"))]
    keyboard_shown: bool,
    /// The focused field was tapped since the last frame: re-shows a
    /// keyboard the user dismissed (Android, iOS).
    #[cfg(any(target_os = "android", target_os = "ios"))]
    keyboard_tap: bool,
    /// Where the current touch went down (logical px), to tell a tap from a
    /// scroll when it lifts.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    touch_down: Option<Point>,
    /// The focused field as the IME last knew it (GameActivity mirrors the
    /// field; a difference after a frame is sent back to it), and which of
    /// its reports to believe while it moves to another field.
    #[cfg(any(
        all(target_os = "android", feature = "android-game-activity"),
        all(target_os = "ios", feature = "ios-text-input")
    ))]
    ime: crate::ime_sync::ImeSync,
    /// The traits the iOS keyboard was last given (it keeps them until told).
    #[cfg(all(target_os = "ios", feature = "ios-text-input"))]
    ios_traits: Option<crate::ime_sync::Traits>,
    started: Instant,
    /// View and frame from the last redraw, for input routing.
    last: Option<(Element<A::Msg>, fenestra_core::Frame)>,
    /// Anything changed since the last full frame? OS-driven redraws
    /// (expose, un-occlude) re-present the cached scene when clean —
    /// the whole build/layout/paint pipeline is skipped.
    dirty: bool,
    /// The last painted scene with its (logical w, h, scale, safe area) key.
    cached_scene: Option<(Scene, FrameKey)>,
    /// The (logical w, h, scale, safe area) of the frame in the raster
    /// target, when the window draws through the raster backend.
    raster_key: Option<FrameKey>,
    modifiers: winit::keyboard::ModifiersState,
    /// The AccessKit adapter, created before the window first shows.
    #[cfg(not(target_arch = "wasm32"))]
    adapter: Option<accesskit_winit::Adapter>,
    /// Loop proxy handed to the adapter for activation/action events.
    #[cfg(not(target_arch = "wasm32"))]
    proxy: EventLoopProxy<RunnerEvent>,
    /// Secondary windows declared by [`App::windows`], keyed by their
    /// stable key and reconciled after every update (native only).
    #[cfg(not(target_arch = "wasm32"))]
    secondary: std::collections::HashMap<String, SecondaryWindow<A>>,
    /// Cache for `PassKind::Custom` render specs: keyed by
    /// `(render_key, cache_key)`, reused across frames when the key
    /// matches. Cleared on scene-key change so stale entries don't
    /// leak across window reconfigurations.
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "custom passes need the native two-pass render")
    )]
    custom_cache: std::collections::HashMap<(u64, u64), vello::peniko::ImageData>,
    /// Custom render registry from [`App::custom_render`]. `None` when
    /// the app doesn't register custom renderers (the default).
    #[cfg_attr(
        target_arch = "wasm32",
        expect(dead_code, reason = "custom passes need the native two-pass render")
    )]
    custom_render: Option<fenestra_core::CustomRender>,
}

/// One reconciled secondary window: its own surface, retained state, and
/// accessibility adapter; app state and fonts are shared.
#[cfg(not(target_arch = "wasm32"))]
struct SecondaryWindow<A: App> {
    shell: WindowShell,
    state: FrameState,
    cursor: Point,
    touch: crate::touch::Touch,
    last: Option<(Element<A::Msg>, fenestra_core::Frame)>,
    on_close: A::Msg,
    title: String,
    adapter: Option<accesskit_winit::Adapter>,
}

/// A running subscription timer: its period (part of its identity — a
/// changed period restarts the sub) and the cancel flag its thread checks
/// after each sleep. Dropping the handle cancels within one period.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct SubHandle {
    period: std::time::Duration,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for SubHandle {
    fn drop(&mut self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Runs one deferred effect unit off the UI thread, delivering its message
/// through `proxy`.
///
/// If the thread cannot be spawned — a process at its thread limit, a
/// sandbox that forbids threads — the unit runs *here* instead. Blocking
/// the UI thread for the length of one effect is bad; dropping the effect
/// is worse, because the app then waits forever for a reply that is never
/// coming and the user is left with a spinner that never stops. Losing a
/// message is the one outcome this must not have.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn spawn_effect_unit<Msg: 'static>(
    unit: fenestra_core::CmdUnit<Msg>,
    proxy: &fenestra_core::Proxy<Msg>,
) where
    // The message itself never crosses the boundary as a value — the unit
    // produces it on the worker and hands it straight to the proxy — so
    // these are the real requirements, not `Msg: Send`.
    fenestra_core::CmdUnit<Msg>: Send,
    fenestra_core::Proxy<Msg>: Send,
{
    dispatch_effect_unit(unit, proxy, |work| {
        std::thread::Builder::new()
            .name("fenestra-cmd".into())
            .spawn(work)
            .map(drop)
    });
}

/// The spawn-or-fall-back half of [`spawn_effect_unit`], with the thread
/// spawner passed in so the failure path is reachable from a test — an
/// out-of-threads process is not something a test can arrange for real,
/// and this path only matters when it is the one that runs.
#[cfg(not(target_arch = "wasm32"))]
fn dispatch_effect_unit<Msg: 'static, S>(
    unit: fenestra_core::CmdUnit<Msg>,
    proxy: &fenestra_core::Proxy<Msg>,
    spawn: S,
) where
    fenestra_core::CmdUnit<Msg>: Send,
    fenestra_core::Proxy<Msg>: Send,
    S: FnOnce(Box<dyn FnOnce() + Send + 'static>) -> std::io::Result<()>,
{
    // The spawner consumes the closure, and with it the unit, so the unit
    // is parked somewhere both sides can reach: the worker takes it if it
    // starts, and we take it back if it never does.
    let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(unit)));
    let worker_slot = std::sync::Arc::clone(&slot);
    let worker_proxy = proxy.clone();
    let spawned = spawn(Box::new(move || {
        if let Some(unit) = worker_slot.lock().ok().and_then(|mut g| g.take()) {
            worker_proxy.send(unit.block());
        }
    }));
    let Err(e) = spawned else { return };
    // Report the spawn failure on its own, before trying to recover from
    // it. Sharing one condition with the recovery meant that failing to
    // reclaim the unit also swallowed the message about failing to spawn —
    // a silent loss inside the function written to prevent silent losses.
    let recovered = slot.lock().ok().and_then(|mut g| g.take());
    match recovered {
        Some(unit) => {
            eprintln!(
                "fenestra: could not spawn an effect worker thread ({e}); running the effect on \
                 the UI thread instead — the window will not repaint until it finishes"
            );
            proxy.send(unit.block());
        }
        None => eprintln!(
            "fenestra: could not spawn an effect worker thread ({e}), and the effect could not be \
             reclaimed to run here either — its message will never arrive"
        ),
    }
}

/// Reconciles running subscription timers against a desired set: new keys
/// start a timer thread delivering ticks through `proxy`, missing keys
/// cancel theirs (within one period), a changed period restarts. Shared by
/// the app runner and [`crate::Embedded`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn reconcile_subs_into<Msg: 'static>(
    subs: &mut std::collections::HashMap<String, SubHandle>,
    desired: Vec<fenestra_core::Sub<Msg>>,
    proxy: &fenestra_core::Proxy<Msg>,
) {
    subs.retain(|key, handle| {
        desired
            .iter()
            .any(|s| s.key() == key && s.period() == handle.period)
    });
    for sub in desired {
        if subs.contains_key(sub.key()) {
            continue;
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = SubHandle {
            period: sub.period(),
            cancel: std::sync::Arc::clone(&cancel),
        };
        let key = sub.key().to_owned();
        let proxy = proxy.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("fenestra-sub-{key}"))
            .spawn(move || {
                loop {
                    std::thread::sleep(sub.period());
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    proxy.send(sub.tick());
                }
            });
        if let Err(e) = spawned {
            // Deliberately not inserted: leaving the key absent is what
            // makes the next reconcile try again. Say so, because a timer
            // is often the only thing that would *cause* another
            // reconcile, so "later" can mean "never" — unlike a one-shot
            // effect, there is nothing to run inline as a fallback.
            eprintln!(
                "fenestra: could not spawn the timer for subscription {key:?} ({e}); it is not \
                 running, and will only be retried if something else updates the app"
            );
            continue;
        }
        subs.insert(key, handle);
    }
}

/// The attached native menu: its structural fingerprint, the live muda
/// menu (kept alive for the app's lifetime), and item-id → message map.
#[cfg(target_os = "macos")]
struct MenuHandle<Msg> {
    fingerprint: String,
    _menu: muda::Menu,
    actions: std::collections::HashMap<String, Msg>,
}

/// Builds a muda menu from the declarative spec: the platform app menu
/// (with Quit) first, then the declared menus. Returns the id → message
/// map alongside.
#[cfg(target_os = "macos")]
fn build_menu<Msg: Clone>(
    spec: &fenestra_core::MenuSpec<Msg>,
) -> Result<(muda::Menu, std::collections::HashMap<String, Msg>), muda::Error> {
    fn append_items<Msg: Clone>(
        submenu: &muda::Submenu,
        items: &[fenestra_core::MenuItemDesc<Msg>],
        actions: &mut std::collections::HashMap<String, Msg>,
    ) -> Result<(), muda::Error> {
        for item in items {
            match item {
                fenestra_core::MenuItemDesc::Item {
                    title,
                    msg,
                    accelerator,
                    enabled,
                } => {
                    let accel = accelerator.as_ref().and_then(|a| {
                        a.parse::<muda::accelerator::Accelerator>()
                            .map_err(|e| {
                                eprintln!("fenestra: menu accelerator {a:?} did not parse: {e}");
                            })
                            .ok()
                    });
                    let mi = muda::MenuItem::new(title, *enabled, accel);
                    actions.insert(mi.id().0.clone(), msg.clone());
                    submenu.append(&mi)?;
                }
                fenestra_core::MenuItemDesc::Separator => {
                    submenu.append(&muda::PredefinedMenuItem::separator())?;
                }
                fenestra_core::MenuItemDesc::Submenu { title, items } => {
                    let nested = muda::Submenu::new(title, true);
                    append_items(&nested, items, actions)?;
                    submenu.append(&nested)?;
                }
            }
        }
        Ok(())
    }
    let menu = muda::Menu::new();
    let mut actions = std::collections::HashMap::new();
    // macOS: the first submenu becomes the application menu.
    let app_menu = muda::Submenu::new("App", true);
    app_menu.append(&muda::PredefinedMenuItem::quit(None))?;
    menu.append(&app_menu)?;
    for desc in &spec.menus {
        let submenu = muda::Submenu::new(&desc.title, true);
        append_items(&submenu, &desc.items, &mut actions)?;
        menu.append(&submenu)?;
    }
    Ok((menu, actions))
}

/// What one input event did in the main window.
#[derive(Clone, Copy, Debug, Default)]
struct InputOutcome {
    /// It produced messages for the app.
    consumed: bool,
    /// A key nothing took: core left it unhandled and the app's
    /// `window_key` had nothing for it.
    #[cfg_attr(
        not(target_os = "android"),
        expect(dead_code, reason = "read by the Android back key")
    )]
    unhandled: bool,
}

/// Whether a back press goes to the system: only when core left the key
/// unhandled (no overlay, field or focused control took it) and the app's
/// `window_key` had no message for it.
#[cfg(any(target_os = "android", test))]
fn back_to_system(unhandled_by_core: bool, app_took_it: bool) -> bool {
    unhandled_by_core && !app_took_it
}

impl<A: App> AppRunner<A> {
    /// Dispatches the long presses that have come due (a finger held still
    /// for [`crate::touch::LONG_PRESS`]), in every window.
    fn fire_long_presses(&mut self, event_loop: &ActiveEventLoop) {
        let now = self.started.elapsed().as_secs_f64();
        for event in self.touch.tick(now) {
            self.touch_input_main(event_loop, event);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let due: Vec<(String, Vec<InputEvent>)> = self
                .secondary
                .iter_mut()
                .map(|(key, bundle)| (key.clone(), bundle.touch.tick(now)))
                .filter(|(_, events)| !events.is_empty())
                .collect();
            for (key, events) in due {
                for event in events {
                    self.secondary_input_main(&key, event_loop, event);
                }
            }
        }
    }

    /// Makes the event loop wake when a held finger's long press comes due,
    /// keeping an earlier wake (an animation frame) if one is set.
    fn touch_wake(&self, event_loop: &ActiveEventLoop) {
        let main = self.touch.deadline();
        #[cfg(not(target_arch = "wasm32"))]
        let due = self
            .secondary
            .values()
            .filter_map(|b| b.touch.deadline())
            .chain(main)
            .reduce(f64::min);
        #[cfg(target_arch = "wasm32")]
        let due = main;
        let Some(due) = due else {
            return;
        };
        let due = self.started + Duration::from_secs_f64(due.max(0.0));
        match event_loop.control_flow() {
            ControlFlow::Poll => {}
            ControlFlow::WaitUntil(t) if t <= due => {}
            _ => event_loop.set_control_flow(ControlFlow::WaitUntil(due)),
        }
    }

    /// Reconciles the native menu bar against [`App::menu`]: rebuilt only
    /// when the structural fingerprint changes; `None` after a menu was
    /// attached leaves the last menu in place (macOS keeps an app menu
    /// regardless).
    #[cfg(target_os = "macos")]
    fn reconcile_menu(&mut self) {
        let Some(spec) = self.app.menu() else {
            return;
        };
        let fingerprint = spec.fingerprint();
        if self
            .menu
            .as_ref()
            .is_some_and(|m| m.fingerprint == fingerprint)
        {
            return;
        }
        match build_menu(&spec) {
            Ok((menu, actions)) => {
                menu.init_for_nsapp();
                self.menu = Some(MenuHandle {
                    fingerprint,
                    _menu: menu,
                    actions,
                });
            }
            Err(e) => eprintln!("fenestra: building the native menu failed: {e}"),
        }
    }

    /// Applies one message through [`App::update_with`] and executes the
    /// returned effect.
    fn apply(&mut self, msg: A::Msg) {
        let cmd = self.app.update_with(msg);
        self.run_cmd(cmd);
    }

    /// Executes an effect through the shared [`fenestra_core::apply_cmd`]
    /// semantics (immediate messages re-enter `update_with` FIFO, no
    /// recursion); deferred units run on worker threads (native) or the
    /// microtask queue (web), delivering results through the proxy.
    fn run_cmd(&mut self, cmd: fenestra_core::Cmd<A::Msg>) {
        let proxy = self.msg_proxy.clone();
        fenestra_core::apply_cmd(&mut self.app, cmd, &mut |unit| {
            #[cfg(not(target_arch = "wasm32"))]
            {
                spawn_effect_unit(unit, &proxy);
            }
            #[cfg(target_arch = "wasm32")]
            {
                match unit {
                    // No threads on the web: blocking tasks run inline
                    // (keep them light there) …
                    fenestra_core::CmdUnit::Task(f) => proxy.send(f()),
                    // … while futures ride the browser's microtask queue.
                    fenestra_core::CmdUnit::Future(fut) => {
                        let proxy = proxy.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            proxy.send(fut.await);
                        });
                    }
                }
            }
        });
    }

    /// Reconciles running subscription timers against
    /// [`App::subscriptions`] — see [`reconcile_subs_into`].
    #[cfg(not(target_arch = "wasm32"))]
    fn reconcile_subs(&mut self) {
        reconcile_subs_into(&mut self.subs, self.app.subscriptions(), &self.msg_proxy);
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(not(target_arch = "wasm32"))]
        let frame_started = Instant::now();
        if let Err(e) = self.shell.pump() {
            self.shell.fail(event_loop, e);
            return;
        }
        let Some((lw, lh, scale)) = self.shell.logical_size() else {
            return;
        };
        let insets = self
            .shell
            .window()
            .map_or_else(Default::default, |w| crate::safe_area::window_insets(w));
        let key = (lw, lh, scale, insets);
        // Clean frame at the same size: present the last frame again and
        // skip build/layout/paint entirely (expose/un-occlude redraws).
        if self.shell.raster_sizes().is_some_and(|(target, surface)| {
            reuses_raster_frame(self.dirty, self.raster_key, key, target, surface)
        }) {
            if let Err(e) = self.shell.raster_present() {
                self.shell.fail(event_loop, e);
            }
            return;
        }
        if !self.dirty
            && let Some((scene, cached)) = &self.cached_scene
            && *cached == key
        {
            if let Err(e) = self.shell.present_physical(scene) {
                self.shell.fail(event_loop, e);
            }
            return;
        }
        crate::frame_stats::frame_start();
        let t = Instant::now();
        let theme = self.app.theme();
        self.shell.background = theme.bg;
        self.state.tick(self.started.elapsed().as_secs_f64());
        #[expect(clippy::cast_possible_truncation, reason = "window sizes fit in f32")]
        let logical = (lw as f32, lh as f32);
        // The app lays out inside the safe area, and so do its overlays; the
        // frame still covers the whole screen (see `safe_area`).
        self.state.safe_area = insets.into();
        let view = crate::safe_area::inset_root(
            self.app
                .view_at(fenestra_core::MAIN_WINDOW, insets.inside(logical)),
            insets,
            theme.bg,
        );
        let frame = build_frame(
            &view,
            &theme,
            &mut self.fonts,
            &mut self.state,
            logical,
            scale,
        );
        crate::frame_stats::record(crate::frame_stats::Phase::Build, t.elapsed());
        if self.shell.uses_raster() {
            if !self.present_raster_frame(event_loop, &frame, scale) {
                return;
            }
            self.raster_key = Some(key);
            self.dirty = frame.animating;
        } else {
            let t = Instant::now();
            // Two-pass render when the frame has filtered regions (glass blur,
            // element filters, custom GPU renders): paint the backdrop scene
            // (every filtered subtree skipped), read it back, run
            // `process_specs` on the CPU, then paint the final scene with each
            // region composited. A frame with no specs falls through to the
            // single-pass fast path: the backdrop scene IS the final image,
            // byte-identical to the old `frame.paint()` + `present()`.
            let (backdrop_scene, specs) = frame.paint_backdrop(&mut self.fonts, &mut self.state);
            let scene = if specs.is_empty() {
                // Fast path: backdrop scene is the final image, just scale and present.
                #[cfg(not(target_arch = "wasm32"))]
                self.shell.release_vello_glass();
                WindowShell::at_scale(backdrop_scene, scale)
            } else {
                // The web has no synchronous read-back and no CPU filter pass:
                // glass draws its translucent tint unblurred, as `Frame::paint`
                // does, and foreground filters are inert.
                #[cfg(target_arch = "wasm32")]
                {
                    WindowShell::at_scale(
                        frame.paint_final(&mut self.fonts, &mut self.state, &Default::default()),
                        scale,
                    )
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // Clone the Arc so the closure is `'static` (it doesn't borrow
                    // `self`). The closure falls back to `None` when no custom
                    // render is registered — the element paints normally.
                    let custom_arc = self.custom_render.clone();
                    let custom_fn =
                        move |key: u64, w: u32, h: u32| -> Option<vello::peniko::ImageData> {
                            custom_arc
                                .as_ref()
                                .and_then(|f| f(key, w, h).map(|d| d.image))
                        };
                    crate::frame_stats::two_pass(specs.len());
                    let physical_backdrop = WindowShell::at_scale(backdrop_scene, scale);
                    match self.shell.two_pass_scene(
                        &frame,
                        &specs,
                        &physical_backdrop,
                        (&mut self.fonts, &mut self.state),
                        theme.bg,
                        (&custom_fn, &mut self.custom_cache),
                        true,
                    ) {
                        Some(Ok(scene)) => scene,
                        Some(Err(e)) => {
                            self.shell.fail(event_loop, e);
                            return;
                        }
                        None => return, // no active surface; nothing to present
                    }
                }
            };
            crate::frame_stats::record(crate::frame_stats::Phase::Paint, t.elapsed());
            if let Err(e) = self.shell.present_physical(&scene) {
                self.shell.fail(event_loop, e);
                return;
            }
            crate::frame_stats::frame_end();
            // The frame is clean until something changes it; animation and
            // hover refresh keep it dirty so the pipeline runs again.
            self.cached_scene = Some((scene, key));
            self.dirty = frame.animating;
        }
        // Content may have moved under a stationary pointer (scroll,
        // layout change): refresh hover and repaint once more if it did.
        if refresh_hover(&view, &frame, &mut self.state)
            && let Some(w) = self.shell.window()
        {
            self.dirty = true;
            w.request_redraw();
        }
        if frame.animating {
            #[cfg(not(target_arch = "wasm32"))]
            event_loop
                .set_control_flow(ControlFlow::WaitUntil(next_animation_frame(frame_started)));
            // The browser paces frames; just ask for the next one.
            #[cfg(target_arch = "wasm32")]
            if let Some(w) = self.shell.window() {
                w.request_redraw();
            }
        } else {
            #[cfg(not(target_arch = "wasm32"))]
            let secondary_animating = self
                .secondary
                .values()
                .any(|b| b.last.as_ref().is_some_and(|(_, f)| f.animating));
            #[cfg(target_arch = "wasm32")]
            let secondary_animating = false;
            if !secondary_animating {
                event_loop.set_control_flow(ControlFlow::Wait);
            }
        }
        self.last = Some((view, frame));
        // Anchor the IME candidate window to the focused caret.
        if let Some(caret) = self.state.ime_caret()
            && let Some(w) = self.shell.window()
        {
            w.set_ime_cursor_area(
                winit::dpi::LogicalPosition::new(caret.x0, caret.y0),
                winit::dpi::LogicalSize::new(1.0, caret.height()),
            );
        }
        // iOS: the field and its traits reach the view before the keyboard
        // rises, so it comes up as the field's (Send key, layout), not as a
        // default keyboard reloaded a moment later.
        #[cfg(all(target_os = "ios", feature = "ios-text-input"))]
        self.sync_ime_text();
        #[cfg(any(target_os = "android", target_os = "ios"))]
        self.sync_soft_keyboard();
        #[cfg(all(target_os = "android", feature = "android-game-activity"))]
        self.sync_ime_text();
        #[cfg(not(target_arch = "wasm32"))]
        self.push_access_tree();
    }

    /// Paints `frame` through the raster backend and presents it — two
    /// passes when it has filtered regions (the backdrop rendered, the part
    /// the glass reads copied back and filtered on the CPU, then the final
    /// pass). Returns `false` when the redraw must stop (no surface, or a
    /// failure, already reported).
    fn present_raster_frame(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame: &fenestra_core::Frame,
        scale: f64,
    ) -> bool {
        use fenestra_core::draw::DrawTarget;
        let t = Instant::now();
        let transform = kurbo::Affine::scale(scale);
        let (fonts, state) = (&mut self.fonts, &mut self.state);
        let Some((specs, mut render)) = self.shell.raster_render(|f| {
            f.push_transform(transform);
            let specs = frame.paint_backdrop_into(f, fonts, state);
            f.pop_transform();
            specs
        }) else {
            return false;
        };
        if !specs.is_empty() {
            // The web paints glass unblurred and filters inert, as on the
            // vello path: the final pass with nothing injected.
            #[cfg(target_arch = "wasm32")]
            let (injected, baked) = (
                std::collections::HashMap::new(),
                std::collections::HashSet::new(),
            );
            #[cfg(not(target_arch = "wasm32"))]
            let (injected, baked) = {
                crate::frame_stats::two_pass(specs.len());
                let Some((pw, ph)) = self.shell.physical_size() else {
                    return false;
                };
                // Glass composites on the GPU from the target as it is;
                // only the other filters still read their region back.
                let tg = Instant::now();
                let (panes, mut rest) = if crate::backend::gpu_glass_enabled() {
                    crate::multi_pass::glass_panes(&specs, frame, scale, (pw, ph))
                } else {
                    (Vec::new(), specs.clone())
                };
                let mut injected = if panes.is_empty() {
                    std::collections::HashMap::new()
                } else {
                    let Some(glass) = self.shell.raster_glass(&panes) else {
                        return false;
                    };
                    glass
                };
                crate::frame_stats::record(crate::frame_stats::Phase::Filters, tg.elapsed());
                let baked: std::collections::HashSet<_> = injected.keys().copied().collect();
                if !panes.is_empty() {
                    rest.extend(crate::multi_pass::uncomposited(
                        &specs,
                        &injected,
                        scale,
                        (pw, ph),
                    ));
                }
                if rest.is_empty() {
                    (injected, baked)
                } else {
                    let region = crate::multi_pass::backdrop_region(&rest, scale, pw, ph)
                        .unwrap_or((0, 0, 1.min(pw), 1.min(ph)));
                    let tr = Instant::now();
                    let backdrop = match self.shell.raster_read(region) {
                        Some(Ok(image)) => image,
                        Some(Err(e)) => {
                            self.shell.fail(event_loop, e);
                            return false;
                        }
                        None => return false,
                    };
                    crate::frame_stats::record(crate::frame_stats::Phase::Readback, tr.elapsed());
                    let custom_arc = self.custom_render.clone();
                    let custom_fn =
                        move |key: u64, w: u32, h: u32| -> Option<vello::peniko::ImageData> {
                            custom_arc
                                .as_ref()
                                .and_then(|f| f(key, w, h).map(|d| d.image))
                        };
                    let tf = Instant::now();
                    let filtered = crate::multi_pass::process_specs_in(
                        &backdrop,
                        (region.0, region.1),
                        (pw, ph),
                        &rest,
                        scale,
                        &custom_fn,
                        &mut self.custom_cache,
                        &mut self.shell.glass_cache,
                    );
                    crate::frame_stats::record(crate::frame_stats::Phase::Filters, tf.elapsed());
                    injected.extend(filtered);
                    (injected, baked)
                }
            };
            let (fonts, state) = (&mut self.fonts, &mut self.state);
            let Some(((), r)) = self.shell.raster_render(|f| {
                f.push_transform(transform);
                frame.paint_final_baked_into(f, fonts, state, &injected, &baked);
                f.pop_transform();
            }) else {
                return false;
            };
            render += r;
        }
        // Paint as on the vello path (readback and filters inside, taken out
        // by the stats); the raster encode and submit is the Render phase.
        crate::frame_stats::record(
            crate::frame_stats::Phase::Paint,
            t.elapsed().saturating_sub(render),
        );
        crate::frame_stats::record(crate::frame_stats::Phase::Render, render);
        if let Err(e) = self.shell.raster_present() {
            self.shell.fail(event_loop, e);
            return false;
        }
        crate::frame_stats::frame_end();
        true
    }

    /// Tells the IME about a focused field it does not know as it is:
    /// newly focused, caret moved by a tap or key, value set by the app, or
    /// an IME edit the field refused. Only on a difference — every push
    /// restarts the IME's input session, which would drop a composition.
    #[cfg(all(target_os = "android", feature = "android-game-activity"))]
    fn sync_ime_text(&mut self) {
        use winit::platform::android::activity::input::{
            ImeOptions, InputType, TextInputAction, TextInputState, TextSpan,
        };
        let now = self.state.ime_text_state();
        if now.as_ref() == self.ime.synced() {
            return;
        }
        if let Some(ime) = &now
            && let Some(app) = &self.shell.options.android_app
        {
            // The field's keyboard and enter key, when they are not what
            // the IME already holds (GameActivity keeps them until told).
            let info = crate::ime_sync::editor_info(ime);
            if self.ime.held_editor_info() != Some(info) {
                app.set_ime_editor_info(
                    InputType::from_bits_retain(info.input_type),
                    TextInputAction::from(info.action),
                    ImeOptions::from_bits_retain(info.options),
                );
            }
            // Java indexes UTF-16 code units.
            let units = |byte: usize| ime.text[..byte].encode_utf16().count();
            app.set_text_input_state(TextInputState {
                text: ime.text.clone(),
                selection: TextSpan {
                    start: units(ime.selection.0),
                    end: units(ime.selection.1),
                },
                compose_region: ime.compose.map(|(a, b)| TextSpan {
                    start: units(a),
                    end: units(b),
                }),
            });
        }
        self.ime.pushed(now, self.started.elapsed().as_secs_f64());
    }

    /// iOS's half of [`Self::sync_ime_text`] (winit fork, `ios-text-input`):
    /// the view's `UITextInput` composes in a copy of the focused field and
    /// reports each edit as `Ime::TextState`; this keeps that copy and the
    /// keyboard's traits in step with the field. Byte offsets throughout —
    /// the fork converts to UIKit's UTF-16.
    #[cfg(all(target_os = "ios", feature = "ios-text-input"))]
    fn sync_ime_text(&mut self) {
        use winit::platform::ios::{TextInputState, WindowExtIOS};
        let now = self.state.ime_text_state();
        if now.as_ref() == self.ime.synced() {
            return;
        }
        if let Some(w) = self.shell.window() {
            if let Some(ime) = &now {
                let traits = crate::ime_sync::text_input_traits(ime);
                if self.ios_traits != Some(traits) {
                    w.set_text_input_traits(ios_traits(traits));
                    self.ios_traits = Some(traits);
                }
            }
            w.set_text_input_state(now.as_ref().map(|ime| TextInputState {
                text: ime.text.clone(),
                selection: ime.selection,
                compose: ime.compose,
            }));
        }
        self.ime.pushed(now, self.started.elapsed().as_secs_f64());
    }

    /// Re-reads the UI language (the system's may have changed while the app
    /// ran); returns whether it changed, which marks the frame dirty.
    fn refresh_locale(&mut self) -> bool {
        let tag = self.shell.options.ui_locale();
        let before = self.fonts.locale();
        self.fonts.set_locale(tag.as_deref());
        let changed = self.fonts.locale() != before;
        self.dirty |= changed;
        changed
    }

    /// Phones type only through the soft keyboard, so it follows focus:
    /// shown while a text field holds the caret (again after a tap, in case
    /// the user dismissed it), hidden when focus leaves. On iOS allowing the
    /// IME makes the view first responder, which is what raises the
    /// keyboard — enabling it at startup showed a keyboard over every app.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    fn sync_soft_keyboard(&mut self) {
        let want = self.state.ime_caret().is_some();
        let tapped = std::mem::take(&mut self.keyboard_tap);
        if let Some(allowed) = keyboard_change(want, self.keyboard_shown, tapped) {
            if let Some(w) = self.shell.window() {
                w.set_ime_allowed(allowed);
            }
            self.keyboard_shown = allowed;
        }
    }

    /// Pushes the current frame's accessibility projection to the platform
    /// (no-op until assistive technology activates the tree).
    #[cfg(not(target_arch = "wasm32"))]
    fn push_access_tree(&mut self) {
        let scale = self.shell.window().map_or(1.0, |w| w.scale_factor());
        let focus = self.state.focused();
        if let Some(adapter) = &mut self.adapter
            && let Some((_, frame)) = &self.last
        {
            adapter.update_if_active(|| crate::access::tree_update(frame, focus, scale));
        }
    }

    fn input(&mut self, event: InputEvent) -> InputOutcome {
        let Some((view, frame)) = &self.last else {
            return InputOutcome::default();
        };
        let result = dispatch(view, frame, &mut self.state, &mut self.fonts, event);
        if let Some(cursor) = result.cursor
            && let Some(w) = self.shell.window()
        {
            w.set_cursor(winit::window::Cursor::Icon(map_cursor(cursor)));
        }
        let mut msgs = result.msgs;
        let mut unhandled = false;
        if let Some(key) = &result.unhandled {
            match self.app.window_key(key) {
                Some(msg) => msgs.push(msg),
                None => unhandled = true,
            }
        }
        let had_msgs = !msgs.is_empty();
        for msg in msgs {
            self.apply(msg);
        }
        if result.redraw || had_msgs {
            self.dirty = true;
            if let Some(w) = self.shell.window() {
                w.request_redraw();
            }
        }
        InputOutcome {
            consumed: had_msgs,
            unhandled,
        }
    }

    /// Android's back key: the app's (closing an overlay, leaving a field
    /// or a page) when anything takes it, else the system's
    /// ([`WindowOptions::android_back`]).
    #[cfg(target_os = "android")]
    fn back(&mut self, event_loop: &ActiveEventLoop, input: InputEvent) {
        let outcome = self.input(input);
        if outcome.consumed {
            self.after_update(event_loop);
        }
        if back_to_system(outcome.unhandled, outcome.consumed)
            && let Some(app) = &self.shell.options.android_app
            && let Some(back) = self.shell.options.android_back
        {
            back(app);
        }
    }

    /// Routes one input event into the main window, reconciling windows
    /// afterwards if it produced messages.
    fn input_main(&mut self, event_loop: &ActiveEventLoop, event: InputEvent) {
        self.input_main_consumed(event_loop, event);
    }

    /// [`Self::input_main`], returning whether the event produced messages.
    fn input_main_consumed(&mut self, event_loop: &ActiveEventLoop, event: InputEvent) -> bool {
        let consumed = self.input(event).consumed;
        if consumed {
            self.after_update(event_loop);
        }
        consumed
    }

    /// Routes an event a touch produced into the main window, and plays the
    /// haptic it calls for (a long press that acted).
    fn touch_input_main(&mut self, event_loop: &ActiveEventLoop, event: InputEvent) {
        let long_press = event == InputEvent::RightDown;
        let consumed = self.input_main_consumed(event_loop, event);
        if let Some(haptic) = crate::touch::touch_haptic(long_press, consumed) {
            self.play_haptic(haptic);
        }
    }

    /// Plays a touch feedback through the app's hook (Android, iOS);
    /// nothing elsewhere.
    fn play_haptic(&self, haptic: fenestra_core::Haptic) {
        #[cfg(target_os = "android")]
        if let (Some(play), Some(app)) = (
            self.shell.options.android_haptic,
            &self.shell.options.android_app,
        ) {
            play(app, haptic);
        }
        #[cfg(target_os = "ios")]
        if let Some(play) = self.shell.options.ios_haptic {
            play(haptic);
        }
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        let _ = haptic;
    }

    /// Routes one input event into a secondary window, reconciling
    /// windows afterwards if it produced messages.
    #[cfg(not(target_arch = "wasm32"))]
    fn secondary_input_main(&mut self, key: &str, event_loop: &ActiveEventLoop, event: InputEvent) {
        if self.secondary_input(key, event) {
            self.after_update(event_loop);
        }
    }

    /// Reconciles secondary windows against [`App::windows`] and asks
    /// every window for a repaint — called whenever messages were applied.
    fn after_update(&mut self, event_loop: &ActiveEventLoop) {
        self.dirty = true;
        #[cfg(not(target_arch = "wasm32"))]
        self.reconcile_subs();
        #[cfg(target_os = "macos")]
        self.reconcile_menu();
        #[cfg(not(target_arch = "wasm32"))]
        self.reconcile_windows(event_loop);
        #[cfg(target_arch = "wasm32")]
        let _ = event_loop;
        if let Some(w) = self.shell.window() {
            w.request_redraw();
        }
        #[cfg(not(target_arch = "wasm32"))]
        for bundle in self.secondary.values() {
            if let Some(w) = bundle.shell.window() {
                w.request_redraw();
            }
        }
    }

    /// Opens, closes, and retitles secondary windows to match the app's
    /// declared list.
    #[cfg(not(target_arch = "wasm32"))]
    fn reconcile_windows(&mut self, event_loop: &ActiveEventLoop) {
        // The main window's own visibility, declared the same way its
        // secondaries are. Only when it changes: `set_visible` with the value
        // it already has is a no-op everywhere, but asking a window about
        // itself every update is not free either.
        if let RenderState::Active { window, .. } = &self.shell.state {
            let want = self.app.main_visible();
            if window.is_visible() != Some(want) {
                window.set_visible(want);
            }
        }
        let desired = self.app.windows();
        self.secondary
            .retain(|key, _| desired.iter().any(|d| &d.key == key));
        for desc in desired {
            match self.secondary.get_mut(&desc.key) {
                Some(bundle) => {
                    bundle.on_close = desc.on_close;
                    if bundle.title != desc.title {
                        bundle.title.clone_from(&desc.title);
                        if let Some(w) = bundle.shell.window() {
                            w.set_title(&desc.title);
                        }
                    }
                }
                None => {
                    let mut shell = WindowShell::new(options_for(&desc), self.shell.background);
                    let proxy = self.proxy.clone();
                    let mut adapter = None;
                    if let Err(e) = shell.resumed_with(event_loop, |el, window| {
                        adapter = Some(accesskit_winit::Adapter::with_event_loop_proxy(
                            el, window, proxy,
                        ));
                    }) {
                        // Failing to open a declared window is app-fatal: the
                        // GPU/window stack is broken, not just this window.
                        self.shell.fail(event_loop, e);
                        return;
                    }
                    if let Some(w) = shell.window() {
                        w.set_ime_allowed(true);
                        w.request_redraw();
                    }
                    let mut state = live_state();
                    state.set_clipboard(Box::new(os_clipboard(&self.shell.options)));
                    self.secondary.insert(
                        desc.key.clone(),
                        SecondaryWindow {
                            shell,
                            state,
                            cursor: Point::ORIGIN,
                            touch: crate::touch::Touch::new(crate::touch::GESTURES),
                            last: None,
                            on_close: desc.on_close,
                            title: desc.title,
                            adapter,
                        },
                    );
                }
            }
        }
    }

    /// Redraws one secondary window: the same pipeline as the main one,
    /// against its own retained state and `view_for(key)`.
    #[cfg(not(target_arch = "wasm32"))]
    fn secondary_redraw(&mut self, key: &str, event_loop: &ActiveEventLoop) {
        let frame_started = Instant::now();
        let theme = self.app.theme_for(key);
        let now = self.started.elapsed().as_secs_f64();
        let Some(bundle) = self.secondary.get_mut(key) else {
            return;
        };
        if let Err(e) = bundle.shell.pump() {
            self.shell.fail(event_loop, e);
            return;
        }
        let Some((lw, lh, scale)) = bundle.shell.logical_size() else {
            return;
        };
        bundle.shell.background = theme.bg;
        bundle.state.tick(now);
        #[expect(clippy::cast_possible_truncation, reason = "window sizes fit in f32")]
        let logical = (lw as f32, lh as f32);
        let view = self.app.view_at(key, logical);
        let frame = build_frame(
            &view,
            &theme,
            &mut self.fonts,
            &mut bundle.state,
            logical,
            scale,
        );
        // Two-pass render: same pipeline as the main window. Falls through
        // to the single-pass fast path when the frame has no filtered
        // regions (byte-identical to the old `frame.paint()` + `present()`).
        let (backdrop_scene, specs) = frame.paint_backdrop(&mut self.fonts, &mut bundle.state);
        let scene = if specs.is_empty() {
            bundle.shell.release_vello_glass();
            WindowShell::at_scale(backdrop_scene, scale)
        } else {
            // Same custom-render closure as the main window (see redraw).
            let custom_arc = self.custom_render.clone();
            let custom_fn = move |key: u64, w: u32, h: u32| -> Option<vello::peniko::ImageData> {
                custom_arc
                    .as_ref()
                    .and_then(|f| f(key, w, h).map(|d| d.image))
            };
            let physical_backdrop = WindowShell::at_scale(backdrop_scene, scale);
            match bundle.shell.two_pass_scene(
                &frame,
                &specs,
                &physical_backdrop,
                (&mut self.fonts, &mut bundle.state),
                theme.bg,
                (&custom_fn, &mut self.custom_cache),
                false,
            ) {
                Some(Ok(scene)) => scene,
                Some(Err(e)) => {
                    self.shell.fail(event_loop, e);
                    return;
                }
                None => return,
            }
        };
        if let Err(e) = bundle.shell.present_physical(&scene) {
            // Secondary failures stop the app through the main shell's
            // channel: device-level errors are app-fatal, not per-window.
            self.shell.fail(event_loop, e);
            return;
        }
        if refresh_hover(&view, &frame, &mut bundle.state)
            && let Some(w) = bundle.shell.window()
        {
            w.request_redraw();
        }
        if frame.animating {
            event_loop
                .set_control_flow(ControlFlow::WaitUntil(next_animation_frame(frame_started)));
        }
        if let Some(caret) = bundle.state.ime_caret()
            && let Some(w) = bundle.shell.window()
        {
            w.set_ime_cursor_area(
                winit::dpi::LogicalPosition::new(caret.x0, caret.y0),
                winit::dpi::LogicalSize::new(1.0, caret.height()),
            );
        }
        bundle.last = Some((view, frame));
        let focus = bundle.state.focused();
        if let Some(adapter) = &mut bundle.adapter
            && let Some((_, frame)) = &bundle.last
        {
            adapter.update_if_active(|| crate::access::tree_update(frame, focus, scale));
        }
    }

    /// The full event handler for one secondary window — the same arms as
    /// the main window, against the bundle's own surface and state.
    #[cfg(not(target_arch = "wasm32"))]
    fn secondary_window_event(
        &mut self,
        key: &str,
        event_loop: &ActiveEventLoop,
        event: WindowEvent,
    ) {
        if let Some(bundle) = self.secondary.get_mut(key)
            && let Some(window) = bundle.shell.window()
            && let Some(adapter) = &mut bundle.adapter
        {
            adapter.process_event(window, &event);
        }
        match event {
            WindowEvent::CloseRequested => {
                if let Some(msg) = self.secondary.get(key).map(|b| b.on_close.clone()) {
                    self.apply(msg);
                    self.after_update(event_loop);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(bundle) = self.secondary.get_mut(key) {
                    bundle.shell.resized(size.width, size.height);
                }
            }
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Occluded(false) => {
                if let Some(w) = self.secondary.get(key).and_then(|b| b.shell.window()) {
                    w.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                let m = self.modifiers;
                self.secondary_input_main(
                    key,
                    event_loop,
                    InputEvent::Modifiers {
                        shift: m.shift_key(),
                        ctrl: m.control_key(),
                        alt: m.alt_key(),
                        meta: m.super_key(),
                    },
                );
            }
            WindowEvent::DroppedFile(path) => {
                self.secondary_input_main(key, event_loop, InputEvent::FileDrop(path));
            }
            WindowEvent::CursorLeft { .. } => {
                self.secondary_input_main(key, event_loop, InputEvent::PointerLeave);
            }
            WindowEvent::Touch(touch) => {
                let Some(bundle) = self.secondary.get_mut(key) else {
                    return;
                };
                let scale = bundle.shell.window().map_or(1.0, |w| w.scale_factor());
                let (x, y) = (touch.location.x, touch.location.y);
                let at = Point::new(x / scale, y / scale);
                let now = self.started.elapsed().as_secs_f64();
                let owns_drag = touch.phase == TouchPhase::Started
                    && bundle.last.as_ref().is_some_and(|(view, frame)| {
                        fenestra_core::owns_touch_drag(view, frame, &bundle.state, at)
                    });
                let moving = touch.phase == TouchPhase::Started
                    && bundle.last.as_ref().is_some_and(|(_, frame)| {
                        fenestra_core::scroll_moving_at(frame, &bundle.state, at)
                    });
                let Some(events) =
                    bundle
                        .touch
                        .event(touch.id, touch.phase, at, now, owns_drag, moving)
                else {
                    return;
                };
                bundle.cursor = at;
                for event in events {
                    self.secondary_input_main(key, event_loop, event);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let Some(bundle) = self.secondary.get_mut(key) else {
                    return;
                };
                let scale = bundle.shell.window().map_or(1.0, |w| w.scale_factor());
                bundle.cursor = Point::new(position.x / scale, position.y / scale);
                #[expect(clippy::cast_possible_truncation, reason = "positions fit in f32")]
                let (x, y) = (bundle.cursor.x as f32, bundle.cursor.y as f32);
                self.secondary_input_main(key, event_loop, InputEvent::PointerMove { x, y });
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                self.secondary_input_main(
                    key,
                    event_loop,
                    match state {
                        winit::event::ElementState::Pressed => InputEvent::PointerDown,
                        winit::event::ElementState::Released => InputEvent::PointerUp,
                    },
                );
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Right,
                ..
            } => {
                self.secondary_input_main(
                    key,
                    event_loop,
                    match state {
                        winit::event::ElementState::Pressed => InputEvent::RightDown,
                        winit::event::ElementState::Released => InputEvent::RightUp,
                    },
                );
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = self
                    .secondary
                    .get(key)
                    .and_then(|b| b.shell.window())
                    .map_or(1.0, |w| w.scale_factor());
                let (dx, dy) = wheel_deltas(delta, scale);
                #[expect(clippy::cast_possible_truncation, reason = "deltas fit in f32")]
                self.secondary_input_main(
                    key,
                    event_loop,
                    InputEvent::Wheel {
                        dx: dx as f32,
                        dy: dy as f32,
                    },
                );
            }
            WindowEvent::PinchGesture { delta, phase, .. } => {
                if let Some(delta) = pinch_delta(delta) {
                    self.secondary_input_main(
                        key,
                        event_loop,
                        InputEvent::Pinch {
                            delta,
                            phase: gesture_phase(phase),
                        },
                    );
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == winit::event::ElementState::Pressed =>
            {
                let mods = self.modifiers;
                let printable = !mods.control_key()
                    && !mods.super_key()
                    && event
                        .text
                        .as_ref()
                        .is_some_and(|t| !t.is_empty() && t.chars().all(|c| !c.is_control()));
                if printable {
                    if let Some(t) = &event.text {
                        self.secondary_input_main(key, event_loop, InputEvent::Text(t.to_string()));
                    }
                } else if let Some(input) = map_key(&event, mods) {
                    self.secondary_input_main(key, event_loop, input);
                }
            }
            WindowEvent::Ime(ime) => match ime {
                winit::event::Ime::Preedit(text, cursor) => {
                    self.secondary_input_main(
                        key,
                        event_loop,
                        InputEvent::ImePreedit { text, cursor },
                    );
                }
                winit::event::Ime::Commit(text) => {
                    self.secondary_input_main(key, event_loop, InputEvent::Text(text));
                }
                // Enabled/Disabled, and the Android `TextState` a patched
                // winit adds (secondary windows don't exist there).
                _ => {}
            },
            WindowEvent::RedrawRequested => self.secondary_redraw(key, event_loop),
            _ => {}
        }
    }

    /// Dispatches one input event against a secondary window. Returns
    /// whether messages were applied (the caller then reconciles).
    #[cfg(not(target_arch = "wasm32"))]
    fn secondary_input(&mut self, key: &str, event: InputEvent) -> bool {
        let Some(bundle) = self.secondary.get_mut(key) else {
            return false;
        };
        let Some((view, frame)) = &bundle.last else {
            return false;
        };
        let result = dispatch(view, frame, &mut bundle.state, &mut self.fonts, event);
        if let Some(cursor) = result.cursor
            && let Some(w) = bundle.shell.window()
        {
            w.set_cursor(winit::window::Cursor::Icon(map_cursor(cursor)));
        }
        let mut msgs = result.msgs;
        if let Some(key) = &result.unhandled
            && let Some(msg) = self.app.window_key(key)
        {
            msgs.push(msg);
        }
        let had_msgs = !msgs.is_empty();
        if (result.redraw || had_msgs)
            && let Some(w) = bundle.shell.window()
        {
            w.request_redraw();
        }
        for msg in msgs {
            self.apply(msg);
        }
        had_msgs
    }
}

pub(crate) fn map_cursor(cursor: fenestra_core::Cursor) -> winit::window::CursorIcon {
    match cursor {
        fenestra_core::Cursor::Default => winit::window::CursorIcon::Default,
        fenestra_core::Cursor::Pointer => winit::window::CursorIcon::Pointer,
        fenestra_core::Cursor::Text => winit::window::CursorIcon::Text,
        fenestra_core::Cursor::NotAllowed => winit::window::CursorIcon::NotAllowed,
        fenestra_core::Cursor::Crosshair => winit::window::CursorIcon::Crosshair,
    }
}

/// Translates a winit key event into a fenestra [`InputEvent`].
pub(crate) fn map_key(
    event: &winit::event::KeyEvent,
    mods: winit::keyboard::ModifiersState,
) -> Option<InputEvent> {
    map_logical_key(&event.logical_key, mods)
}

/// [`map_key`] on the logical key alone.
pub(crate) fn map_logical_key(
    logical: &winit::keyboard::Key,
    mods: winit::keyboard::ModifiersState,
) -> Option<InputEvent> {
    use winit::keyboard::{Key as WKey, NamedKey};
    let key = match logical {
        // Android reports Tab and Enter as the characters they type, not as
        // named keys; they are the same keys.
        WKey::Named(NamedKey::Tab) => {
            return Some(if mods.shift_key() {
                InputEvent::ShiftTab
            } else {
                InputEvent::Tab
            });
        }
        WKey::Character(s) if s == "\t" => {
            return Some(if mods.shift_key() {
                InputEvent::ShiftTab
            } else {
                InputEvent::Tab
            });
        }
        WKey::Character(s) if s == "\n" || s == "\r" => Key::Enter,
        WKey::Named(named) => match named {
            NamedKey::Enter => Key::Enter,
            NamedKey::Space => Key::Space,
            NamedKey::Escape => Key::Escape,
            // Android's back key: closes the top overlay, then the app's own
            // back. (A desktop keyboard's Browser Back key stays unmapped.)
            #[cfg(target_os = "android")]
            NamedKey::BrowserBack => Key::Escape,
            NamedKey::ArrowLeft => Key::ArrowLeft,
            NamedKey::ArrowRight => Key::ArrowRight,
            NamedKey::ArrowUp => Key::ArrowUp,
            NamedKey::ArrowDown => Key::ArrowDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            _ => return None,
        },
        WKey::Character(s) => Key::Char(s.chars().next()?),
        _ => return None,
    };
    Some(InputEvent::Key(KeyInput {
        key,
        shift: mods.shift_key(),
        ctrl: mods.control_key(),
        alt: mods.alt_key(),
        meta: mods.super_key(),
    }))
}

impl<A: App> ApplicationHandler<RunnerEvent> for AppRunner<A> {
    #[cfg(not(target_arch = "wasm32"))]
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Fresh surface, possibly fresh scale: rebuild rather than trust
        // a scene cached across the suspend.
        self.dirty = true;
        let adapter = &mut self.adapter;
        let proxy = self.proxy.clone();
        if let Err(e) = self.shell.resumed_with(event_loop, |el, window| {
            // The adapter must attach while the window is still hidden.
            if adapter.is_none() {
                *adapter = Some(accesskit_winit::Adapter::with_event_loop_proxy(
                    el, window, proxy,
                ));
            }
        }) {
            self.shell.fail(event_loop, e);
            return;
        }
        // Phones: the keyboard follows focus (`sync_soft_keyboard`); the
        // OS dropped it with the old surface, so the next frame re-shows it
        // if a field still holds focus.
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if let Some(w) = self.shell.window() {
            w.set_ime_allowed(true);
        }
        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            self.keyboard_shown = false;
        }
        for bundle in self.secondary.values_mut() {
            if let Err(e) = bundle.shell.resumed(event_loop) {
                self.shell.fail(event_loop, e);
                return;
            }
        }
        self.reconcile_windows(event_loop);
    }

    #[cfg(target_arch = "wasm32")]
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.dirty = true;
        if let Err(e) = self.shell.resumed(event_loop) {
            self.shell.fail(event_loop, e);
            return;
        }
        if let Some(w) = self.shell.window() {
            w.set_ime_allowed(true);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: RunnerEvent) {
        match event {
            RunnerEvent::App(msg) => {
                if let Ok(msg) = msg.downcast::<A::Msg>() {
                    self.apply(*msg);
                    self.after_update(event_loop);
                }
            }
            #[cfg(target_os = "macos")]
            RunnerEvent::Menu(id) => {
                if let Some(msg) = self.menu.as_ref().and_then(|m| m.actions.get(&id)).cloned() {
                    self.apply(msg);
                    self.after_update(event_loop);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            RunnerEvent::Access(ev) => {
                // Route by window id: `None` is the main window, `Some(key)`
                // a secondary one; unknown ids are stale events.
                let is_main = self.shell.window().is_some_and(|w| w.id() == ev.window_id);
                let skey = (!is_main)
                    .then(|| {
                        self.secondary
                            .iter()
                            .find(|(_, b)| b.shell.window().is_some_and(|w| w.id() == ev.window_id))
                            .map(|(k, _)| k.clone())
                    })
                    .flatten();
                if !is_main && skey.is_none() {
                    return;
                }
                match ev.window_event {
                    accesskit_winit::WindowEvent::InitialTreeRequested => match &skey {
                        None => {
                            if self.last.is_some() {
                                self.push_access_tree();
                            } else if let Some(w) = self.shell.window() {
                                w.request_redraw();
                            }
                        }
                        Some(key) => {
                            if let Some(bundle) = self.secondary.get_mut(key) {
                                let scale = bundle.shell.window().map_or(1.0, |w| w.scale_factor());
                                let focus = bundle.state.focused();
                                if let Some((_, frame)) = &bundle.last {
                                    if let Some(adapter) = &mut bundle.adapter {
                                        adapter.update_if_active(|| {
                                            crate::access::tree_update(frame, focus, scale)
                                        });
                                    }
                                } else if let Some(w) = bundle.shell.window() {
                                    w.request_redraw();
                                }
                            }
                        }
                    },
                    accesskit_winit::WindowEvent::ActionRequested(req) => {
                        let id = fenestra_core::WidgetId(req.target_node.0);
                        match req.action {
                            accesskit::Action::Click => {
                                let msg = match &skey {
                                    None => self.last.as_ref().and_then(|(view, frame)| {
                                        fenestra_core::click_msg_of(view, frame, &self.state, id)
                                    }),
                                    Some(key) => self.secondary.get(key).and_then(|bundle| {
                                        bundle.last.as_ref().and_then(|(view, frame)| {
                                            fenestra_core::click_msg_of(
                                                view,
                                                frame,
                                                &bundle.state,
                                                id,
                                            )
                                        })
                                    }),
                                };
                                if let Some(msg) = msg {
                                    self.apply(msg);
                                    self.after_update(event_loop);
                                }
                            }
                            accesskit::Action::Focus => match &skey {
                                None => {
                                    self.state.set_focus(Some(id));
                                    self.dirty = true;
                                    if let Some(w) = self.shell.window() {
                                        w.request_redraw();
                                    }
                                }
                                Some(key) => {
                                    if let Some(bundle) = self.secondary.get_mut(key) {
                                        bundle.state.set_focus(Some(id));
                                        if let Some(w) = bundle.shell.window() {
                                            w.request_redraw();
                                        }
                                    }
                                }
                            },
                            _ => {}
                        }
                    }
                    accesskit_winit::WindowEvent::AccessibilityDeactivated => {}
                }
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        self.shell.suspended();
        #[cfg(not(target_arch = "wasm32"))]
        for bundle in self.secondary.values_mut() {
            bundle.shell.suspended();
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if !matches!(cause, StartCause::ResumeTimeReached { .. }) {
            return;
        }
        // This wake is spent: a frame still animating asks for the next one,
        // and `about_to_wait` restores a pending long press. Left as it was, a
        // past deadline (a tap that changed nothing) would wake the loop
        // again at once, for ever.
        event_loop.set_control_flow(ControlFlow::Wait);
        self.fire_long_presses(event_loop);
        if let Some(w) = self.shell.window() {
            w.request_redraw();
        }
        #[cfg(not(target_arch = "wasm32"))]
        for bundle in self.secondary.values() {
            if let Some(w) = bundle.shell.window() {
                w.request_redraw();
            }
        }
    }

    // Last in each turn of the loop, after frames set the control flow: a
    // held finger's long press must still wake it.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.touch_wake(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.shell.window().is_none_or(|w| w.id() != window_id) {
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(key) = self
                .secondary
                .iter()
                .find(|(_, b)| b.shell.window().is_some_and(|w| w.id() == window_id))
                .map(|(k, _)| k.clone())
            {
                self.secondary_window_event(&key, event_loop, event);
            }
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(adapter) = &mut self.adapter
            && let Some(window) = self.shell.window()
        {
            adapter.process_event(window, &event);
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                // The cache key also guards size, but coalesced resizes can
                // land back on the cached geometry mid-drag.
                self.dirty = true;
                self.shell.resized(size.width, size.height);
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                // On Android every configuration change arrives as this
                // event, a new system language included.
                self.refresh_locale();
                self.dirty = true;
                if let Some(w) = self.shell.window() {
                    w.request_redraw();
                }
            }
            WindowEvent::Focused(true) => {
                if self.refresh_locale()
                    && let Some(w) = self.shell.window()
                {
                    w.request_redraw();
                }
                // Re-read the OS "reduce motion" setting on focus, so a change
                // made while the app was in the background takes effect.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let reduce = crate::reduce_motion::os_reduce_motion();
                    if reduce != self.state.reduced_motion {
                        self.state.reduced_motion = reduce;
                        self.dirty = true;
                        if let Some(w) = self.shell.window() {
                            w.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods.state();
                let m = self.modifiers;
                self.input_main(
                    event_loop,
                    InputEvent::Modifiers {
                        shift: m.shift_key(),
                        ctrl: m.control_key(),
                        alt: m.alt_key(),
                        meta: m.super_key(),
                    },
                );
            }
            WindowEvent::Occluded(occluded) => {
                if !occluded && let Some(w) = self.shell.window() {
                    w.request_redraw();
                }
            }
            WindowEvent::DroppedFile(path) => {
                self.input_main(event_loop, InputEvent::FileDrop(path))
            }
            WindowEvent::CursorLeft { .. } => self.input_main(event_loop, InputEvent::PointerLeave),
            WindowEvent::Touch(touch) => {
                let scale = self.shell.window().map_or(1.0, |w| w.scale_factor());
                let (x, y) = (touch.location.x, touch.location.y);
                let at = Point::new(x / scale, y / scale);
                let now = self.started.elapsed().as_secs_f64();
                let owns_drag = touch.phase == TouchPhase::Started
                    && self.last.as_ref().is_some_and(|(view, frame)| {
                        fenestra_core::owns_touch_drag(view, frame, &self.state, at)
                    });
                // A finger landing on a gliding or springing scroller
                // catches it instead of pressing what is under it.
                let moving = touch.phase == TouchPhase::Started
                    && self.last.as_ref().is_some_and(|(_, frame)| {
                        fenestra_core::scroll_moving_at(frame, &self.state, at)
                    });
                if let Some(events) =
                    self.touch
                        .event(touch.id, touch.phase, at, now, owns_drag, moving)
                {
                    self.cursor = at;
                    for event in events {
                        self.touch_input_main(event_loop, event);
                    }
                    // Judged after dispatch, so a tap that moves focus to
                    // another field counts as a tap on it.
                    #[cfg(any(target_os = "android", target_os = "ios"))]
                    match touch.phase {
                        TouchPhase::Started => self.touch_down = Some(at),
                        TouchPhase::Ended => {
                            if let Some(down) = self.touch_down.take()
                                && let Some((_, frame)) = &self.last
                                && tap_on_focus(frame, self.state.focused(), down, at)
                            {
                                self.keyboard_tap = true;
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.shell.window().map_or(1.0, |w| w.scale_factor());
                self.cursor = Point::new(position.x / scale, position.y / scale);
                #[expect(clippy::cast_possible_truncation, reason = "positions fit in f32")]
                self.input_main(
                    event_loop,
                    InputEvent::PointerMove {
                        x: self.cursor.x as f32,
                        y: self.cursor.y as f32,
                    },
                );
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                // Borderless drag regions: a press inside one starts the OS
                // window-drag gesture instead of app input. Interactive
                // children win — the region only claims presses nothing
                // deeper claimed first (checked after dispatch).
                if state == winit::event::ElementState::Pressed
                    && let Some(window) = self.shell.window()
                {
                    let debug = std::env::var("FENESTRA_DEBUG_INPUT").is_ok();
                    let decorated = window.is_decorated();
                    if debug {
                        eprintln!(
                            "[fenestra-input] press: decorated={decorated} cursor=({},{})",
                            self.cursor.x, self.cursor.y
                        );
                    }
                    if !decorated {
                        let point = Point::new(self.cursor.x, self.cursor.y);
                        if let Some((_view, frame)) = &self.last {
                            match frame.window_control_at(point) {
                                Some(fenestra_core::WindowControl::Minimize) => {
                                    if debug {
                                        eprintln!("[fenestra-input] -> minimize");
                                    }
                                    window.set_minimized(true);
                                    return;
                                }
                                Some(fenestra_core::WindowControl::Maximize) => {
                                    window.set_maximized(!window.is_maximized());
                                    return;
                                }
                                Some(fenestra_core::WindowControl::Close) => {
                                    if debug {
                                        eprintln!("[fenestra-input] -> close");
                                    }
                                    event_loop.exit();
                                    return;
                                }
                                None => {}
                            }
                            if frame.drag_region_at(point) {
                                let result = window.drag_window();
                                if debug {
                                    eprintln!("[fenestra-input] -> drag_window: {result:?}");
                                }
                                return;
                            }
                            if debug {
                                eprintln!("[fenestra-input] -> plain press (no region)");
                            }
                        }
                    }
                }
                self.input_main(
                    event_loop,
                    match state {
                        winit::event::ElementState::Pressed => InputEvent::PointerDown,
                        winit::event::ElementState::Released => InputEvent::PointerUp,
                    },
                );
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Right,
                ..
            } => {
                self.input_main(
                    event_loop,
                    match state {
                        winit::event::ElementState::Pressed => InputEvent::RightDown,
                        winit::event::ElementState::Released => InputEvent::RightUp,
                    },
                );
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = self.shell.window().map_or(1.0, |w| w.scale_factor());
                let (dx, dy) = wheel_deltas(delta, scale);
                #[expect(clippy::cast_possible_truncation, reason = "deltas fit in f32")]
                self.input_main(
                    event_loop,
                    InputEvent::Wheel {
                        dx: dx as f32,
                        dy: dy as f32,
                    },
                );
            }
            WindowEvent::PinchGesture { delta, phase, .. } => {
                if let Some(delta) = pinch_delta(delta) {
                    self.input_main(
                        event_loop,
                        InputEvent::Pinch {
                            delta,
                            phase: gesture_phase(phase),
                        },
                    );
                }
            }
            WindowEvent::KeyboardInput { event, .. }
                if event.state == winit::event::ElementState::Pressed =>
            {
                {
                    let mods = self.modifiers;
                    // Printable input arrives as Text (it may be multi-char);
                    // named keys and shortcuts go through Key.
                    let printable = !mods.control_key()
                        && !mods.super_key()
                        && event
                            .text
                            .as_ref()
                            .is_some_and(|t| !t.is_empty() && t.chars().all(|c| !c.is_control()));
                    if printable {
                        if let Some(t) = &event.text {
                            self.input_main(event_loop, InputEvent::Text(t.to_string()));
                        }
                    } else if let Some(input) = map_key(&event, mods) {
                        #[cfg(target_os = "android")]
                        if event.logical_key
                            == winit::keyboard::Key::Named(winit::keyboard::NamedKey::BrowserBack)
                        {
                            self.back(event_loop, input);
                            return;
                        }
                        self.input_main(event_loop, input);
                    }
                }
            }
            WindowEvent::Ime(ime) => match ime {
                winit::event::Ime::Preedit(text, cursor) => {
                    self.input_main(event_loop, InputEvent::ImePreedit { text, cursor });
                }
                winit::event::Ime::Commit(text) => {
                    self.input_main(event_loop, InputEvent::Text(text));
                }
                #[cfg(any(
                    all(target_os = "android", feature = "android-game-activity"),
                    all(target_os = "ios", feature = "ios-text-input")
                ))]
                winit::event::Ime::TextState {
                    text,
                    selection,
                    compose,
                } => {
                    // Attributed to the field the IME was last synced for;
                    // a stale echo of the previous field is dropped.
                    let now = self.started.elapsed().as_secs_f64();
                    if let Some(ime) = self.ime.received(text, selection, compose, now) {
                        self.input_main(event_loop, InputEvent::ImeState(ime));
                    }
                }
                // Enabled/Disabled (and `TextState` when a patched winit
                // is in the build without GameActivity).
                _ => {}
            },
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{dispatch_effect_unit, options_for};
    use fenestra_core::{CmdUnit, Proxy, WindowDesc};
    use std::sync::{Arc, Mutex};

    /// A declared window opens as the window that was declared.
    ///
    /// The runner built `WindowOptions::titled(..).with_size(..)` from a
    /// `WindowDesc` and let everything else fall to the defaults, so a
    /// secondary window could not be borderless — let alone transparent,
    /// placed on a chosen display, or kept above other windows — while the
    /// main window could already be three of those. Anything asked for in the
    /// descriptor was silently dropped, which is the worst way to refuse.
    ///
    /// Every field, because the failure was per-field: a mapping that carries
    /// four of five is the same bug one column narrower.
    #[test]
    fn windows_carry_every_option_they_declare() {
        let plain = options_for(&WindowDesc::new("k", "Plain", (300.0, 200.0), ()));
        assert_eq!(plain.title, "Plain");
        assert_eq!(plain.inner_size, (300.0, 200.0));
        assert!(!plain.borderless, "a plain window opened without a frame");
        assert!(!plain.fullscreen);
        assert!(!plain.transparent);
        assert!(!plain.always_on_top);
        assert_eq!(plain.position, None);

        // What a screen-selection overlay asks for: all of it at once, and on
        // a display that is not the primary one.
        let overlay = options_for(
            &WindowDesc::new("pick", "Select", (1920.0, 1080.0), ())
                .borderless()
                .fullscreen()
                .transparent()
                .always_on_top()
                .at(-1920.0, 0.0),
        );
        assert!(overlay.borderless, "the overlay opened with a title bar");
        assert!(overlay.fullscreen);
        assert!(overlay.transparent, "the overlay opened opaque");
        assert!(overlay.always_on_top, "the overlay opened behind the app");
        assert_eq!(
            overlay.position,
            Some((-1920.0, 0.0)),
            "the overlay could not be put on the display it is covering"
        );
    }

    /// A worker thread that cannot be spawned used to mean a dropped
    /// message: the app waited forever for a reply that was never coming.
    /// The effect must still run, and its message must still arrive.
    #[test]
    fn a_failed_spawn_runs_the_effect_instead_of_losing_it() {
        let got: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&got);
        let proxy = Proxy::new(move |m: u32| sink.lock().expect("lock").push(m));
        dispatch_effect_unit(CmdUnit::Task(Box::new(|| 7)), &proxy, |_| {
            Err(std::io::Error::other("no threads today"))
        });
        assert_eq!(
            *got.lock().expect("lock"),
            vec![7],
            "the effect must be delivered even when no worker thread is available"
        );
    }

    /// The unit lives in the shared slot rather than inside the closure,
    /// so a spawner that swallows the closure entirely before failing
    /// still leaves the effect reclaimable. However the spawn fails, the
    /// message arrives.
    #[test]
    fn a_spawner_that_eats_the_closure_still_delivers() {
        let got: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&got);
        let proxy = Proxy::new(move |m: u32| sink.lock().expect("lock").push(m));
        dispatch_effect_unit(CmdUnit::Task(Box::new(|| 7)), &proxy, |work| {
            drop(work);
            Err(std::io::Error::other("no threads today"))
        });
        assert_eq!(
            *got.lock().expect("lock"),
            vec![7],
            "the effect is reclaimed from the slot, not from the dropped closure"
        );
    }

    /// The ordinary path still hands the unit to the spawner, and does not
    /// also run it on the caller's thread.
    #[test]
    fn a_successful_spawn_runs_the_effect_once() {
        let got: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&got);
        let proxy = Proxy::new(move |m: u32| sink.lock().expect("lock").push(m));
        dispatch_effect_unit(CmdUnit::Task(Box::new(|| 7)), &proxy, |work| {
            work();
            Ok(())
        });
        assert_eq!(
            *got.lock().expect("lock"),
            vec![7],
            "delivered exactly once"
        );
    }
}

#[cfg(test)]
mod raster_target_tests {
    use super::{needs_new_target, reuses_raster_frame};

    /// The raster target follows the surface: built on first use, rebuilt
    /// when a resize or rotation changes its size, kept otherwise.
    #[test]
    fn raster_target_tracks_the_surface_size() {
        assert!(needs_new_target(None, (800, 600)));
        assert!(!needs_new_target(Some((800, 600)), (800, 600)));
        assert!(needs_new_target(Some((800, 600)), (600, 800)));
    }

    /// A clean frame is presented again only while the target still holds
    /// it: after a device change the new renderer has no target yet.
    #[test]
    fn a_clean_frame_is_reused_only_from_a_kept_target() {
        let key = (400.0, 300.0, 2.0);
        let (target, surface) = (Some((800, 600)), (800, 600));
        assert!(reuses_raster_frame(false, Some(key), key, target, surface));
        assert!(!reuses_raster_frame(false, Some(key), key, None, surface));
        assert!(!reuses_raster_frame(true, Some(key), key, target, surface));
        assert!(!reuses_raster_frame(
            false,
            Some(key),
            (300.0, 400.0, 2.0),
            target,
            surface
        ));
    }
}

#[cfg(test)]
mod keyboard_tap_tests {
    use super::tap_on_focus;
    use fenestra_core::{Fonts, FrameState, Theme, build_frame, by, col, div, raw_input};
    use kurbo::Point;

    /// A lift re-shows a dismissed keyboard only when it is a tap (not a
    /// scroll) that lands on the field holding focus.
    #[test]
    fn only_a_tap_on_the_focused_field_counts() {
        let root = col::<()>().w(200.0).children([
            raw_input("", "").id("field").w(200.0).h(40.0),
            div::<()>().id("list").w(200.0).h(300.0),
        ]);
        let mut fonts = Fonts::embedded();
        let mut state = FrameState::new();
        let frame = build_frame(
            &root,
            &Theme::light(),
            &mut fonts,
            &mut state,
            (200.0, 400.0),
            1.0,
        );
        let field = Some(frame.get(&by::id("field")).id);
        let on_field = Point::new(100.0, 20.0);
        let on_list = Point::new(100.0, 200.0);
        assert!(tap_on_focus(&frame, field, on_field, on_field));
        assert!(
            !tap_on_focus(&frame, field, on_list, on_list),
            "a tap elsewhere"
        );
        assert!(
            !tap_on_focus(&frame, field, Point::new(100.0, 300.0), on_field),
            "a scroll that ends on the field"
        );
        assert!(
            !tap_on_focus(&frame, None, on_field, on_field),
            "nothing focused"
        );
    }
}

#[cfg(test)]
mod input_mapping_tests {
    use super::map_logical_key;
    use fenestra_core::{InputEvent, Key, KeyInput};
    use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};

    /// Android's back key arrives as `BrowserBack`; it is Escape, which
    /// closes the top overlay and then reaches `App::window_key`. Elsewhere
    /// (a keyboard's Browser Back key) it stays unmapped.
    #[test]
    fn android_back_is_escape() {
        let back = map_logical_key(&WKey::Named(NamedKey::BrowserBack), ModifiersState::empty());
        if cfg!(target_os = "android") {
            assert_eq!(back, Some(InputEvent::Key(KeyInput::plain(Key::Escape))));
        } else {
            assert_eq!(back, None);
        }
    }

    /// Back goes to the system only when nothing took it: core left the key
    /// unhandled and the app's `window_key` had nothing for it. An overlay
    /// that ate it (even without telling the app) or a page the app goes
    /// back from keeps it.
    #[test]
    fn back_goes_to_the_system_only_when_nothing_took_it() {
        assert!(super::back_to_system(true, false));
        assert!(
            !super::back_to_system(true, true),
            "the app went back a page"
        );
        assert!(
            !super::back_to_system(false, false),
            "an overlay or field took it"
        );
    }

    /// Android reports Tab and Enter as characters (`"\t"`, `"\n"`), not as
    /// named keys; they are the same keys.
    #[test]
    fn control_characters_are_the_keys_they_stand_for() {
        let none = ModifiersState::empty();
        let tab = WKey::Character("\t".into());
        assert_eq!(map_logical_key(&tab, none), Some(InputEvent::Tab));
        assert_eq!(
            map_logical_key(&tab, ModifiersState::SHIFT),
            Some(InputEvent::ShiftTab)
        );
        for enter in ["\n", "\r"] {
            assert_eq!(
                map_logical_key(&WKey::Character(enter.into()), none),
                Some(InputEvent::Key(KeyInput::plain(Key::Enter))),
                "{enter:?}"
            );
        }
        // The named forms desktop platforms send are unchanged.
        assert_eq!(
            map_logical_key(&WKey::Named(NamedKey::Tab), none),
            Some(InputEvent::Tab)
        );
        assert_eq!(
            map_logical_key(&WKey::Named(NamedKey::Enter), none),
            Some(InputEvent::Key(KeyInput::plain(Key::Enter)))
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod animation_frame_tests {
    use super::next_animation_frame;
    use std::time::{Duration, Instant};

    /// The next frame of an animation is due one period after this frame
    /// *started*: its own work counts toward the period. A 10 ms frame waits
    /// 6 ms more; a 31 ms frame's next one is already due when it ends.
    #[test]
    fn the_next_animation_frame_counts_from_this_frames_start() {
        let start = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(next_animation_frame(start), start + ms(16));
        assert!(next_animation_frame(start) - (start + ms(10)) == ms(6));
        assert!(next_animation_frame(start) <= start + ms(31));
    }
}

#[cfg(test)]
mod mobile_window_tests {
    use super::{WindowOptions, needs_show, window_geometry};

    #[test]
    fn a_window_of_unknown_visibility_is_shown() {
        assert!(needs_show(None), "iOS never reports visibility");
        assert!(needs_show(Some(false)));
        assert!(!needs_show(Some(true)));
    }

    #[test]
    fn a_mobile_window_ignores_desktop_geometry() {
        let o = WindowOptions::titled("t")
            .with_min_size(960.0, 640.0)
            .with_position(10.0, 20.0)
            .fullscreen();
        let g = window_geometry(&o, true);
        assert_eq!(
            (
                g.inner_size,
                g.min_size,
                g.position,
                g.maximized,
                g.fullscreen
            ),
            (None, None, None, false, false)
        );
        let d = window_geometry(&o, false);
        assert_eq!(d.inner_size, Some((1024.0, 768.0)));
        assert_eq!(d.min_size, Some((960.0, 640.0)));
        assert_eq!(d.position, Some((10.0, 20.0)));
        assert!(d.fullscreen);
    }
}

#[cfg(test)]
mod frame_key_tests {
    use super::reuses_raster_frame;
    use crate::safe_area::Insets;

    #[test]
    fn changed_insets_rebuild_the_frame() {
        let none = Insets::default();
        let notched = Insets {
            top: 62.0,
            right: 0.0,
            bottom: 34.0,
            left: 0.0,
        };
        let size = Some((1206, 2622));
        assert!(reuses_raster_frame(
            false,
            Some((402.0, 874.0, 3.0, none)),
            (402.0, 874.0, 3.0, none),
            size,
            (1206, 2622)
        ));
        assert!(!reuses_raster_frame(
            false,
            Some((402.0, 874.0, 3.0, none)),
            (402.0, 874.0, 3.0, notched),
            size,
            (1206, 2622)
        ));
    }
}

#[cfg(test)]
mod soft_keyboard_tests {
    use super::{keyboard_change, map_logical_key};
    use fenestra_core::{InputEvent, Key, KeyInput};
    use winit::keyboard::{Key as WKey, ModifiersState};

    #[test]
    fn the_keyboard_follows_the_caret() {
        assert_eq!(keyboard_change(true, false, false), Some(true));
        assert_eq!(keyboard_change(false, true, false), Some(false));
        assert_eq!(keyboard_change(false, false, true), None);
    }

    #[test]
    fn a_tap_reshows_a_dismissed_keyboard() {
        assert_eq!(keyboard_change(true, true, true), Some(true));
        assert_eq!(keyboard_change(true, true, false), None);
    }

    #[test]
    fn a_newline_text_is_the_enter_key() {
        // The iOS keyboard's return key arrives as the character "\n".
        let enter = Some(InputEvent::Key(KeyInput::plain(Key::Enter)));
        let none = ModifiersState::empty();
        assert_eq!(map_logical_key(&WKey::Character("\n".into()), none), enter);
        assert_eq!(map_logical_key(&WKey::Character("\r".into()), none), enter);
    }
}

/// The system clipboard for a window: on Android it needs the app's
/// activity, so without one it stays in-app.
#[cfg(not(target_arch = "wasm32"))]
fn os_clipboard(options: &WindowOptions) -> crate::OsClipboard {
    #[cfg(target_os = "android")]
    if let Some(app) = &options.android_app {
        return crate::OsClipboard::android(app.clone());
    }
    let _ = options;
    crate::OsClipboard::default()
}
