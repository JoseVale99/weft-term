//! Path sandbox for read-only tools.
//!
//! A tool may read only inside the working directory. Symlinks are resolved
//! before the check, so a link pointing outside is rejected even when its own
//! path looks contained. The check is read-only: it canonicalizes paths but
//! never opens, creates or modifies a file.

use std::env;
use std::io;
use std::path::{Path, PathBuf};

/// Resolves `requested` against the current working directory.
pub fn resolve(requested: &str) -> io::Result<PathBuf> {
    resolve_in(&env::current_dir()?, requested)
}

/// Resolves `requested` against `root` and rejects anything outside it.
pub fn resolve_in(root: &Path, requested: &str) -> io::Result<PathBuf> {
    let root = root.canonicalize()?;
    let candidate = root.join(requested);
    let canonical = candidate
        .canonicalize()
        .map_err(|error| io::Error::other(format!("{requested}: {error}")))?;
    if !canonical.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{requested} is outside the working directory"),
        ));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn workspace(name: &str) -> (PathBuf, PathBuf) {
        let base = env::temp_dir().join(format!("weft-sandbox-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(root.join("inside.txt"), "ok").unwrap();
        fs::write(outside.join("secret.txt"), "no").unwrap();
        (root, outside)
    }

    #[test]
    fn files_inside_are_accepted() {
        let (root, _) = workspace("inside");
        let path = resolve_in(&root, "inside.txt").unwrap();
        assert_eq!(path, root.canonicalize().unwrap().join("inside.txt"));
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn parent_traversal_is_rejected() {
        let (root, _) = workspace("traversal");
        let error = resolve_in(&root, "../outside/secret.txt").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn absolute_paths_outside_are_rejected() {
        let (root, outside) = workspace("absolute");
        let requested = outside.join("secret.txt");
        let error = resolve_in(&root, requested.to_str().unwrap()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn a_missing_file_reports_an_error() {
        let (root, _) = workspace("missing");
        assert!(resolve_in(&root, "nope.txt").is_err());
        let _ = fs::remove_dir_all(root.parent().unwrap());
    }
}
