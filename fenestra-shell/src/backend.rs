//! Which rasterizer draws frames: vello (compute shaders) or the
//! conventional raster backend (`fenestra-raster`: triangles, MSAA, stencil),
//! which mobile GPUs run far better.

use std::ffi::OsStr;

/// Which rasterizer draws frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// vello's compute pipeline (or its CPU fallback, `FENESTRA_CPU`).
    Vello,
    /// `fenestra-raster`: tessellated triangles, 4x MSAA, a glyph atlas.
    Raster,
}

/// Env var choosing the [`Backend`]: `vello` or `raster` (any case);
/// anything else means the platform default — raster on Android and iOS,
/// vello elsewhere.
pub const RENDERER_ENV: &str = "FENESTRA_RENDERER";

/// The platform's default backend.
const PLATFORM_DEFAULT: Backend = if cfg!(any(target_os = "android", target_os = "ios")) {
    Backend::Raster
} else {
    Backend::Vello
};

/// The backend [`RENDERER_ENV`] asks for, else the platform default.
pub fn backend_from_env() -> Backend {
    parse_backend(std::env::var_os(RENDERER_ENV).as_deref(), PLATFORM_DEFAULT)
}

/// [`RENDERER_ENV`]'s value as a backend.
pub(crate) fn parse_backend(v: Option<&OsStr>, platform_default: Backend) -> Backend {
    match v
        .and_then(OsStr::to_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("vello") => Backend::Vello,
        Some("raster") => Backend::Raster,
        _ => platform_default,
    }
}

/// Raster needs Vulkan, Metal, or DX12: on GL drivers it drew black on
/// Adreno, so the shell keeps vello there.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn raster_supported(backend: wgpu::Backend) -> bool {
    matches!(
        backend,
        wgpu::Backend::Vulkan | wgpu::Backend::Metal | wgpu::Backend::Dx12
    )
}

/// The backend to draw with: `requested`, unless that is raster on an
/// adapter raster cannot use ([`raster_supported`]).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn effective_backend(requested: Backend, adapter: wgpu::Backend) -> Backend {
    match requested {
        Backend::Raster if !raster_supported(adapter) => Backend::Vello,
        b => b,
    }
}

/// The backend a window draws with: [`effective_backend`], and then raster
/// instead of vello when the adapter cannot run vello's compute pipeline
/// (`indirect`: it lacks indirect dispatch, as the iOS simulator does) but
/// can run raster. vello's CPU mode (`cpu`) needs no indirect dispatch, and
/// when neither backend fits (GL without indirect dispatch) vello stays.
///
/// Windows only: headless rendering keeps an explicit backend, so a vello
/// golden test never quietly renders with raster on a weak adapter.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn window_backend(
    requested: Backend,
    adapter: wgpu::Backend,
    indirect: bool,
    cpu: bool,
) -> Backend {
    match effective_backend(requested, adapter) {
        Backend::Vello if !indirect && !cpu && raster_supported(adapter) => Backend::Raster,
        b => b,
    }
}

/// Prints once that vello was asked for on an adapter that cannot run it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn warn_vello_fallback() {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "fenestra: this adapter cannot run vello's compute pipeline (no indirect dispatch); rendering with raster"
        );
    }
}

/// Prints once that raster was asked for on an adapter it cannot use.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn warn_raster_fallback(adapter: wgpu::Backend) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "fenestra: the raster backend needs Vulkan, Metal, or DX12; {adapter:?} renders with vello"
        );
    }
}

/// Whether glass composites on the GPU (where the adapter allows): on unless
/// `FENESTRA_GPU_GLASS` is `0`, which forces the CPU path on both backends.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn gpu_glass_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| gpu_glass_from(std::env::var("FENESTRA_GPU_GLASS").ok().as_deref()))
}

/// [`gpu_glass_enabled`] for the variable's value.
#[cfg_attr(target_arch = "wasm32", expect(dead_code, reason = "native only"))]
fn gpu_glass_from(value: Option<&str>) -> bool {
    value != Some("0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_glass_switch_reads_the_environment() {
        assert!(gpu_glass_from(None), "on unless switched off");
        assert!(gpu_glass_from(Some("1")));
        assert!(gpu_glass_from(Some("")));
        assert!(
            !gpu_glass_from(Some("0")),
            "FENESTRA_GPU_GLASS=0 forces the CPU glass"
        );
    }

    #[test]
    fn the_env_picks_a_backend_or_the_default() {
        let parse = |v: Option<&str>, d| parse_backend(v.map(OsStr::new), d);
        for d in [Backend::Vello, Backend::Raster] {
            assert_eq!(parse(None, d), d);
            assert_eq!(parse(Some(""), d), d);
            assert_eq!(parse(Some("x"), d), d);
            assert_eq!(parse(Some("vello"), d), Backend::Vello);
            assert_eq!(parse(Some("raster"), d), Backend::Raster);
            assert_eq!(parse(Some("RASTER"), d), Backend::Raster);
        }
    }

    #[test]
    fn a_gl_adapter_keeps_vello() {
        use wgpu::Backend as Api;
        assert_eq!(
            effective_backend(Backend::Raster, Api::Vulkan),
            Backend::Raster
        );
        assert_eq!(effective_backend(Backend::Raster, Api::Gl), Backend::Vello);
        assert_eq!(
            effective_backend(Backend::Vello, Api::Vulkan),
            Backend::Vello
        );
        assert_eq!(effective_backend(Backend::Vello, Api::Gl), Backend::Vello);
    }

    #[test]
    fn vello_without_indirect_dispatch_falls_back_to_raster() {
        assert_eq!(
            window_backend(Backend::Vello, wgpu::Backend::Metal, false, false),
            Backend::Raster
        );
    }

    #[test]
    fn vello_keeps_vello_when_it_can_run() {
        assert_eq!(
            window_backend(Backend::Vello, wgpu::Backend::Metal, true, false),
            Backend::Vello
        );
        // CPU mode needs no indirect dispatch.
        assert_eq!(
            window_backend(Backend::Vello, wgpu::Backend::Metal, false, true),
            Backend::Vello
        );
    }

    #[test]
    fn neither_backend_fitting_keeps_vello() {
        assert_eq!(
            window_backend(Backend::Vello, wgpu::Backend::Gl, false, false),
            Backend::Vello
        );
        assert_eq!(
            window_backend(Backend::Raster, wgpu::Backend::Gl, true, false),
            Backend::Vello
        );
    }

    #[test]
    fn raster_needs_a_modern_api() {
        assert!(!raster_supported(wgpu::Backend::Gl));
        assert!(raster_supported(wgpu::Backend::Vulkan));
        assert!(raster_supported(wgpu::Backend::Metal));
        assert!(raster_supported(wgpu::Backend::Dx12));
    }
}
