use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use crate::config::cache_dir;

const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "tif", "tiff", "gif", "webp", "bmp", // common
    "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "dng", "pef", "srw", // raw
];

const VOLUMES: &str = "/Volumes";
const CAMERA_DIR: &str = "DCIM";

#[derive(Clone, Copy)]
pub enum Kind {
    Thumb,
    Preview,
}

impl Kind {
    fn max_px(self) -> u32 {
        match self {
            Kind::Thumb => 320,
            Kind::Preview => 2400,
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Kind::Thumb => "thumbs",
            Kind::Preview => "previews",
        }
    }
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Recursive, sorted by path so camera folders keep shooting order.
pub fn scan(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() && is_image(&path) => out.push(path),
                _ => {}
            }
        }
    }

    out.sort();
    out
}

/// Mounted volumes that look like camera cards.
pub fn camera_volumes() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(VOLUMES) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(CAMERA_DIR).is_dir())
        .collect();
    out.sort();
    out
}

/// Key includes size and mtime so edited files get a new render.
fn cache_file(src: &Path, kind: Kind) -> PathBuf {
    let mut h = DefaultHasher::new();
    src.hash(&mut h);
    if let Ok(meta) = fs::metadata(src) {
        meta.len().hash(&mut h);
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
        mtime.map(|d| d.as_secs()).hash(&mut h);
    }
    cache_dir().join(kind.dir()).join(format!("{:016x}.jpg", h.finish()))
}

/// Uses `sips` (macOS ImageIO) so HEIC and camera RAW decode without extra crates.
pub fn render(src: &Path, kind: Kind) -> Option<PathBuf> {
    let out = cache_file(src, kind);
    if out.exists() {
        return Some(out);
    }
    fs::create_dir_all(out.parent()?).ok()?;

    let status = Command::new("sips")
        .args(["-s", "format", "jpeg", "-Z", &kind.max_px().to_string()])
        .arg(src)
        .arg("--out")
        .arg(&out)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;

    (status.success() && out.exists()).then_some(out)
}
