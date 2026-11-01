//! Image files of a session's project, for the chat to show: only inside the project folder, only image types.
use std::path::{Path, PathBuf};

pub const MAX_FILE: u64 = 20 * 1024 * 1024;

/// The content type for the image extensions the chat shows (SVG is left out on purpose: it can carry script).
pub fn image_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The real path of `wanted` (absolute, or relative to the project) when it is a small image file inside the project, links resolved.
pub fn resolve(project: &Path, wanted: &str) -> Option<PathBuf> {
    let root = project.canonicalize().ok()?;
    let joined = if Path::new(wanted).is_absolute() { PathBuf::from(wanted) } else { root.join(wanted) };
    let real = joined.canonicalize().ok()?;
    if !real.starts_with(&root) || !real.is_file() || image_type(&real).is_none() {
        return None;
    }
    if std::fs::metadata(&real).ok()?.len() > MAX_FILE {
        return None;
    }
    Some(real)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_images_inside_the_project_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("p");
        let outside = dir.path().join("o");
        std::fs::create_dir_all(project.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(project.join("a.png"), b"x").unwrap();
        std::fs::write(project.join("sub/b.JPG"), b"x").unwrap();
        std::fs::write(project.join("note.txt"), b"x").unwrap();
        std::fs::write(project.join("v.svg"), b"<svg/>").unwrap();
        std::fs::write(outside.join("secret.png"), b"x").unwrap();
        std::os::unix::fs::symlink(outside.join("secret.png"), project.join("link.png")).unwrap();

        assert!(resolve(&project, "a.png").is_some());
        assert!(resolve(&project, "sub/b.JPG").is_some());
        assert!(resolve(&project, project.join("a.png").to_str().unwrap()).is_some());
        assert!(resolve(&project, "note.txt").is_none());
        assert!(resolve(&project, "v.svg").is_none());
        assert!(resolve(&project, "../o/secret.png").is_none());
        assert!(resolve(&project, outside.join("secret.png").to_str().unwrap()).is_none());
        assert!(resolve(&project, "link.png").is_none(), "a link that leaves the project is not followed");
        assert!(resolve(&project, "missing.png").is_none());
        assert!(resolve(&project, "sub").is_none());
    }
}
