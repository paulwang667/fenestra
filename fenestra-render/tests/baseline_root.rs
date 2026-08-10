//! Confinement for baseline paths.
//!
//! The CLI names its baseline on the command line, so it reads wherever the
//! person running it can. The MCP server's baseline path arrives inside a
//! tool call, from an agent that may be acting on something it just read, so
//! it reads only under a root. These tests pin the second posture: what gets
//! in, what does not, and what the refusal is allowed to say.

use fenestra_render::engine::validate_diff_params;
use fenestra_render::{BaselineRoot, engine};
use image::{Rgba, RgbaImage};
use std::path::{Path, PathBuf};

/// A private directory for one test, with `inside/` and `outside/` siblings
/// so "escapes the root" has somewhere to escape to.
fn sandbox(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fenestra-root-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("inside")).expect("create inside");
    std::fs::create_dir_all(dir.join("outside")).expect("create outside");
    // Canonical, so that absolute paths built from it are spelled the way the
    // root is: on macOS the temp dir reaches through a `/var` symlink, and a
    // test comparing the two spellings would be testing the symlink.
    dir.canonicalize().expect("canonical sandbox")
}

/// Writes a tiny valid PNG so that reads which *should* succeed do.
fn png_at(path: &Path) {
    RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255]))
        .save(path)
        .expect("write png");
}

#[test]
fn a_root_opens_a_baseline_inside_it() {
    let dir = sandbox("open");
    png_at(&dir.join("inside/shot.png"));
    let root = BaselineRoot::within(dir.join("inside")).expect("root");

    let img = root
        .open("shot.png")
        .expect("a baseline inside the root opens");
    assert_eq!(img.dimensions(), (2, 2));

    // An absolute path landing inside the root is fine too — the rule is
    // about where it resolves, not how it was spelled.
    let abs = dir.join("inside/shot.png");
    assert!(
        root.open(&abs).is_ok(),
        "an absolute path inside the root opens"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_root_refuses_a_parent_escape() {
    let dir = sandbox("escape");
    png_at(&dir.join("outside/secret.png"));
    let root = BaselineRoot::within(dir.join("inside")).expect("root");

    let err = root
        .open("../outside/secret.png")
        .expect_err("climbing out of the root is refused");
    assert!(err.contains("outside the permitted root"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_root_refuses_an_absolute_path_outside_it() {
    let dir = sandbox("absolute");
    png_at(&dir.join("outside/secret.png"));
    let root = BaselineRoot::within(dir.join("inside")).expect("root");

    let err = root
        .open(dir.join("outside/secret.png"))
        .expect_err("an absolute path outside the root is refused");
    assert!(err.contains("outside the permitted root"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The lexical pass runs before any filesystem access precisely so that the
/// refusal cannot double as a way to ask what exists elsewhere on the disk.
#[test]
fn a_refusal_does_not_reveal_whether_the_file_exists() {
    let dir = sandbox("oracle");
    png_at(&dir.join("outside/real.png"));
    let root = BaselineRoot::within(dir.join("inside")).expect("root");

    let present = root
        .open(dir.join("outside/real.png"))
        .expect_err("refused");
    let absent = root
        .open(dir.join("outside/not-here.png"))
        .expect_err("refused");
    assert_eq!(
        present
            .replace("real.png", "X")
            .replace("not-here.png", "X"),
        absent.replace("real.png", "X").replace("not-here.png", "X"),
        "the refusal differs depending on whether the target exists"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// What lexical analysis cannot see: a link inside the root whose target is
/// outside it.
#[cfg(unix)]
#[test]
fn a_root_refuses_a_symlink_that_escapes() {
    let dir = sandbox("symlink");
    png_at(&dir.join("outside/secret.png"));
    std::os::unix::fs::symlink(dir.join("outside/secret.png"), dir.join("inside/link.png"))
        .expect("symlink");
    let root = BaselineRoot::within(dir.join("inside")).expect("root");

    let err = root
        .open("link.png")
        .expect_err("a symlink out of the root is refused");
    assert!(err.contains("outside the permitted root"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nowhere_refuses_every_path() {
    let dir = sandbox("nowhere");
    png_at(&dir.join("inside/shot.png"));

    let err = BaselineRoot::Nowhere
        .open(dir.join("inside/shot.png"))
        .expect_err("Nowhere reads nothing");
    assert!(err.contains("no permitted directory"), "{err}");
    assert!(
        BaselineRoot::Nowhere.resolve_write("x.png").is_err(),
        "Nowhere writes nothing either"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn anywhere_opens_any_readable_path() {
    let dir = sandbox("anywhere");
    png_at(&dir.join("outside/shot.png"));
    let img = BaselineRoot::Anywhere
        .open(dir.join("outside/shot.png"))
        .expect("the CLI posture reads any path");
    assert_eq!(img.dimensions(), (2, 2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn resolve_write_stays_inside_the_root() {
    let dir = sandbox("write");
    let root = BaselineRoot::within(dir.join("inside")).expect("root");

    // The file need not exist yet — that is the whole point of the write
    // resolver — but its directory must be inside the root.
    let ok = root.resolve_write("new.png").expect("a new file inside");
    assert!(ok.ends_with("new.png"));
    assert!(
        root.resolve_write("../outside/new.png").is_err(),
        "a write that climbs out is refused"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The write side had the same hole the read side was fixed for: the parent
/// was canonicalized and contained, then the file name was joined back on
/// unexamined — so a symlink sitting at that name aimed `File::create`, which
/// follows links and truncates, at anything on the disk.
#[cfg(unix)]
#[test]
fn a_symlink_at_the_write_target_is_refused() {
    let dir = sandbox("writelink");
    let victim = dir.join("outside/victim.txt");
    std::fs::write(&victim, "do not overwrite me").expect("write victim");
    std::os::unix::fs::symlink(&victim, dir.join("inside/base.png")).expect("symlink");
    let root = BaselineRoot::within(dir.join("inside")).expect("root");

    let err = root
        .resolve_write("base.png")
        .expect_err("a symlink at the write target is refused");
    assert!(err.contains("symbolic link"), "{err}");
    assert_eq!(
        std::fs::read_to_string(&victim).expect("victim still readable"),
        "do not overwrite me",
        "the file outside the root was written through"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A root has to confine something. Every absolute path starts with `/`, so a
/// root of the filesystem root is spelled like a restriction and is none.
#[test]
fn a_root_that_confines_nothing_is_refused() {
    let err = BaselineRoot::within("/").expect_err("the filesystem root is not a root");
    assert!(err.contains("confines nothing"), "{err}");

    // The home directory is the same trap one level down, and the likelier
    // accident: a server launched by a desktop client inherits whatever
    // working directory it was given.
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from)
        && let Ok(home) = home.canonicalize()
    {
        let err = BaselineRoot::within(&home).expect_err("the home directory is too broad");
        assert!(err.contains("home directory"), "{err}");
    }
}

#[test]
fn a_root_must_be_a_directory_that_exists() {
    let dir = sandbox("badroot");
    png_at(&dir.join("inside/shot.png"));
    assert!(BaselineRoot::within(dir.join("nope")).is_err());
    assert!(
        BaselineRoot::within(dir.join("inside/shot.png")).is_err(),
        "a file is not a root"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_comparison_parameters_that_made_the_leak_one_call_are_rejected() {
    use fenestra_describe::dto::Bounds;

    // A negative budget reports failure for a comparison in which nothing
    // differed, which is what released the diff image.
    assert!(validate_diff_params(0, -1.0, &[]).is_err());
    assert!(validate_diff_params(0, f64::NAN, &[]).is_err());
    assert!(validate_diff_params(0, 1.5, &[]).is_err());
    // A tolerance nothing can exceed compares nothing at all.
    assert!(validate_diff_params(255, 0.0, &[]).is_err());
    // Still accepts the parameters a real comparison uses.
    assert!(validate_diff_params(3, 0.002, &[]).is_ok());
    assert!(validate_diff_params(0, 0.0, &[]).is_ok());
    assert!(validate_diff_params(0, 1.0, &[]).is_ok());
    // And still rejects the masks it always did.
    let bad = vec![Bounds {
        x: f64::INFINITY,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    }];
    assert!(validate_diff_params(0, 0.0, &bad).is_err());
}

/// The engine module re-exports what the front doors need; keep both spellings
/// working so a caller is not forced through `engine::`.
#[test]
fn the_root_type_is_reachable_from_the_crate_root() {
    let _: fn(&Path) -> _ = |p| engine::BaselineRoot::within(p);
}
