//! Building MCP results: lead with a typed `structuredContent` value and a text
//! serialization, attach a *downscaled* inline preview image, and add a
//! `resource_link` to the full-resolution PNG (a `file://` temp path) so a large
//! image never bloats every response as base64 yet stays one fetch away.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Cursor};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use base64::Engine as _;
use image::{ImageFormat, RgbaImage};
use rmcp::model::{CallToolResult, Content, RawResource};
use serde_json::Value;

/// Longest-edge cap, in pixels, for the inline (token-cheap) preview image.
const PREVIEW_CAP: u32 = 768;

/// A successful result: a text serialization, the structured value as
/// `structuredContent`, and (when an image is given) a downscaled inline preview
/// plus a `resource_link` to the full-resolution PNG.
pub fn ok(text: String, structured: Value, image: Option<&RgbaImage>) -> CallToolResult {
    let mut content = vec![Content::text(text)];
    if let Some(png) = image {
        content.push(inline_image(png));
        if let Some(link) = full_res_link(png) {
            content.push(link);
        }
    }
    let mut result = CallToolResult::success(content);
    result.structured_content = Some(structured);
    result
}

/// An `isError` result carrying a text message and a structured payload. Used
/// for tool-level failures the agent should self-correct (e.g. an invalid
/// description from `validate`), as opposed to protocol errors (`ErrorData`).
pub fn error(text: String, structured: Value) -> CallToolResult {
    let mut result = CallToolResult::error(vec![Content::text(text)]);
    result.structured_content = Some(structured);
    result
}

/// A downscaled PNG as a base64 image content block.
fn inline_image(png: &RgbaImage) -> Content {
    let (w, h) = png.dimensions();
    let longest = w.max(h);
    let scaled = if longest > PREVIEW_CAP {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "preview dimensions are small and clamped to >= 1"
        )]
        let (nw, nh) = {
            let s = f64::from(PREVIEW_CAP) / f64::from(longest);
            (
                ((f64::from(w) * s) as u32).max(1),
                ((f64::from(h) * s) as u32).max(1),
            )
        };
        image::imageops::thumbnail(png, nw, nh)
    } else {
        png.clone()
    };
    let mut bytes = Vec::new();
    scaled
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .expect("encoding an in-memory PNG cannot fail");
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Content::image(b64, "image/png")
}

/// How many full-resolution temp PNGs to retain per process. Each render writes
/// one; once there are more than this, the oldest goes, so a long-lived server
/// session keeps at most this many on disk instead of leaking unbounded. A
/// client would have to lag this many renders behind to miss a file it still
/// wants — implausible for a synchronous agent.
const KEEP_FULL_RES: usize = 64;

/// The full-resolution renders still on disk, oldest first, so the GC deletes
/// what was actually written rather than a name it reconstructed. The names
/// are no longer predictable, which is the point — and it means they have to
/// be remembered.
static RETAINED: Mutex<VecDeque<PathBuf>> = Mutex::new(VecDeque::new());

/// Writes the full-resolution PNG to a fresh temp file and returns a
/// `resource_link` content block pointing at it (a `file://` URI), or `None` if
/// the write fails (the inline preview still goes back).
///
/// The file is created with `create_new`, which is `O_CREAT | O_EXCL` — it
/// fails rather than following whatever is already at that path. The old name
/// was `fenestra-mcp-<pid>-<counter>.png`, every part of it predictable, and
/// on a system whose temp directory is shared (a Linux `/tmp`, where this
/// server also runs) anyone able to guess it could have left a symlink there
/// and had the server write a PNG through it. The exclusive create is the
/// actual fix; the unpredictable suffix means an attacker cannot even squat
/// the name to make renders fail. On Unix the mode is `0o600` besides,
/// because a rendered surface can have someone's data on it.
fn full_res_link(png: &RgbaImage) -> Option<Content> {
    let (path, file) = create_temp_png()?;
    let mut writer = BufWriter::new(file);
    png.write_to(&mut writer, ImageFormat::Png).ok()?;
    writer.into_inner().ok()?.sync_all().ok()?;

    // Bound the temp footprint: drop the render from KEEP_FULL_RES calls ago.
    if let Ok(mut retained) = RETAINED.lock() {
        retained.push_back(path.clone());
        while retained.len() > KEEP_FULL_RES {
            if let Some(old) = retained.pop_front() {
                let _ = std::fs::remove_file(old);
            }
        }
    }

    let mut resource = RawResource::new(
        format!("file://{}", path.display()),
        "full-resolution-render.png",
    );
    resource.mime_type = Some("image/png".to_string());
    Some(Content::resource_link(resource))
}

/// Creates a new temp file nothing else can already own, returning it and its
/// path. `None` when a run of attempts all collide, which means something is
/// generating names alongside us and giving up beats looping.
///
/// Public to the crate's tests only in the sense that they call it directly;
/// nothing outside this module needs it.
fn create_temp_png() -> Option<(PathBuf, File)> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    for _ in 0..16 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        // Nanosecond-scale wall clock, mixed with the counter: enough that a
        // name cannot be predicted from the outside, while the exclusive
        // create below is what actually enforces the guarantee.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let path = dir.join(format!("fenestra-mcp-{pid}-{n}-{stamp:09}.png"));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600);
        }
        if let Ok(file) = opts.open(&path) {
            return Some((path, file));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two renders never land on the same path, and neither reuses a name a
    /// third party could have prepared in advance.
    #[test]
    fn temp_renders_get_fresh_unpredictable_names() {
        let (a, _fa) = create_temp_png().expect("a temp file");
        let (b, _fb) = create_temp_png().expect("another temp file");
        assert_ne!(a, b);
        for p in [&a, &b] {
            assert!(p.exists(), "{} was not created", p.display());
        }
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    /// The exclusive create is the guarantee: if the path is already taken —
    /// by a file, or by a symlink someone left pointing at something they
    /// want overwritten — the open fails rather than following it.
    #[test]
    fn an_existing_path_is_never_written_through() {
        let (path, file) = create_temp_png().expect("a temp file");
        drop(file);
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        assert!(
            opts.open(&path).is_err(),
            "create_new must refuse a path that already exists"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A rendered surface can have someone's data on it, so the file is not
    /// world-readable in a shared temp directory.
    #[cfg(unix)]
    #[test]
    fn temp_renders_are_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt as _;

        let (path, _file) = create_temp_png().expect("a temp file");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "mode {mode:o} lets others read it");
        let _ = std::fs::remove_file(&path);
    }
}
