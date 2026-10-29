//! Data directory layout.
//!
//! `UCOMPUTE_HOME` overrides everything (used by tests and by the desktop app's embedded daemon).
//! Otherwise macOS uses `~/Library/Application Support/UCompute`, Linux `$XDG_DATA_HOME/ucompute` or
//! `~/.local/share/ucompute`, Windows `%LOCALAPPDATA%\UCompute`. No former folder is read or moved
//! (D-028): a renamed build starts with a fresh identity and is paired again.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// `UCOMPUTE_HOME`, else the default folder. No side effects.
    pub fn from_env() -> Self {
        Self {
            root: std::env::var_os("UCOMPUTE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(default_root),
        }
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Pinned inference engines: `backends/<name>/<release>/`.
    pub fn backends(&self) -> PathBuf {
        self.root.join("backends")
    }
    /// Content-addressed model blobs: `models/blobs/sha256-<hex>`.
    pub fn model_blobs(&self) -> PathBuf {
        self.root.join("models").join("blobs")
    }
    /// One JSON manifest per cached model: `models/manifests/<id>.json`.
    pub fn model_manifests(&self) -> PathBuf {
        self.root.join("models").join("manifests")
    }
    /// In-progress downloads: `models/partial/<sha256>.part`.
    pub fn partial(&self) -> PathBuf {
        self.root.join("models").join("partial")
    }
    /// Execution records / evidence: `runs/<id>/`.
    pub fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }
    /// Engine process logs.
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }
    /// Device identity, keys (M3).
    pub fn identity(&self) -> PathBuf {
        self.root.join("identity")
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for d in [
            self.backends(),
            self.model_blobs(),
            self.model_manifests(),
            self.partial(),
            self.runs(),
            self.logs(),
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_root() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support").join("UCompute")
    } else if cfg!(target_os = "windows") {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(home);
        base.join("UCompute")
    } else {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local").join("share"));
        base.join("ucompute")
    }
}

/// Reject names that could escape a directory (used for catalog ids and file names).
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && Path::new(name).components().count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        let p = Paths::at("/tmp/x");
        assert_eq!(p.model_blobs(), PathBuf::from("/tmp/x/models/blobs"));
    }

    /// D-028: the default folder is named UCompute, never the former Common one.
    #[test]
    fn default_folder_is_named_ucompute() {
        let name = default_root()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        assert_eq!(name, "ucompute");
    }

    #[test]
    fn safe_names() {
        assert!(safe_name("stories15M-q4_0"));
        assert!(safe_name("Qwen2.5-0.5B-Instruct-Q4_K_M"));
        assert!(!safe_name("../x"));
        assert!(!safe_name("a/b"));
        assert!(!safe_name(".hidden"));
        assert!(!safe_name(""));
    }
}
