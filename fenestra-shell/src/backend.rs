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
/// anything else means the platform default — raster on Android, vello
/// elsewhere.
pub const RENDERER_ENV: &str = "FENESTRA_RENDERER";

/// The platform's default backend.
const PLATFORM_DEFAULT: Backend = if cfg!(target_os = "android") {
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
pub(crate) fn raster_supported(backend: wgpu::Backend) -> bool {
    matches!(
        backend,
        wgpu::Backend::Vulkan | wgpu::Backend::Metal | wgpu::Backend::Dx12
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn raster_needs_a_modern_api() {
        assert!(!raster_supported(wgpu::Backend::Gl));
        assert!(raster_supported(wgpu::Backend::Vulkan));
        assert!(raster_supported(wgpu::Backend::Metal));
        assert!(raster_supported(wgpu::Backend::Dx12));
    }
}
