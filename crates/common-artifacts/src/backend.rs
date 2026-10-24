//! Pinned inference-engine provisioning (ADR-0002). Pins live in `catalog/backends.json`.
//!
//! Trust flows from the pinned SHA-256 of the release archive: the archive is verified before
//! anything is extracted, extraction refuses escaping paths and anything that is not a regular
//! file, directory or in-tree relative symlink, and every extracted file is re-hashed against the
//! archive itself (never against a mutable local manifest).

use crate::download::{fetch, require_https, Progress};
use crate::hash::{is_sha256_hex, sha256_file, sha256_reader};
use common_core::paths::safe_name;
use common_core::{CommonError, ErrorCode, Paths, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

pub const BACKENDS_JSON: &str = include_str!("../../../catalog/backends.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendPin {
    pub name: String,
    pub release: String,
    pub source_commit: String,
    /// `macos-arm64`, `linux-x64`, `win-x64`...
    pub platform: String,
    pub archive: String,
    /// Directory inside the archive that holds the binaries.
    pub directory: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub license: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendPins {
    pub schema_version: u32,
    pub backends: Vec<BackendPin>,
}

impl BackendPins {
    pub fn builtin() -> Result<Self> {
        Ok(serde_json::from_str(BACKENDS_JSON)?)
    }
    /// The pin for this OS/arch, if one exists.
    pub fn for_current_platform(&self, name: &str) -> Option<&BackendPin> {
        let plat = current_platform();
        self.backends.iter().find(|b| b.name == name && b.platform == plat)
    }
}

pub fn current_platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "win",
        o => o,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        a => a,
    };
    format!("{os}-{arch}")
}

/// Directory where the verified binaries for `pin` live.
pub fn backend_dir(paths: &Paths, pin: &BackendPin) -> PathBuf {
    paths.backends().join(&pin.name).join(&pin.release)
}

/// Where the pinned release archive is kept: `backends/<name>/<archive>`.
pub fn archive_path(paths: &Paths, pin: &BackendPin) -> PathBuf {
    paths.backends().join(&pin.name).join(&pin.archive)
}

/// Download (if needed), verify the archive hash, extract safely, verify every extracted file
/// against the archive. Returns the binary directory.
pub async fn ensure_backend(paths: &Paths, pin: &BackendPin, progress: &mut dyn FnMut(Progress)) -> Result<PathBuf> {
    check_pin(pin)?;
    let archive = archive_path(paths, pin);
    if !archive.is_file() {
        require_https(&pin.url)?;
        let partial = paths.partial().join(format!("{}.part", pin.sha256));
        fetch(
            &pin.url,
            &pin.sha256,
            pin.bytes,
            &partial,
            &archive,
            true,
            common_hardware::disk_free,
            progress,
        )
        .await?;
    }
    let (paths, pin) = (paths.clone(), pin.clone());
    tokio::task::spawn_blocking(move || {
        verify_archive(&archive, &pin)?;
        let dir = backend_dir(&paths, &pin);
        if !dir.is_dir() {
            extract(&archive, &pin, &dir)?;
        }
        verify_tree(&archive, &pin, &dir)?;
        Ok(dir)
    })
    .await
    .map_err(|e| CommonError::internal(e.to_string()))?
}

/// Verify an already-extracted backend directory against its pinned archive without network.
pub fn verify_backend(paths: &Paths, pin: &BackendPin) -> Result<PathBuf> {
    check_pin(pin)?;
    let archive = archive_path(paths, pin);
    if !archive.is_file() {
        return Err(unavailable(format!("backend archive {} is missing", pin.archive))
            .with_hint("run `common setup` to download the pinned engine"));
    }
    verify_archive(&archive, pin)?;
    let dir = backend_dir(paths, pin);
    if !dir.is_dir() {
        return Err(unavailable(format!("backend directory {} is missing", dir.display()))
            .with_hint("run `common setup` to extract the pinned engine"));
    }
    verify_tree(&archive, pin, &dir)?;
    Ok(dir)
}

/// Path of a named binary inside a backend dir (adds `.exe` on Windows).
pub fn binary(dir: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        dir.join(format!("{name}.exe"))
    } else {
        dir.join(name)
    }
}

fn unavailable(msg: String) -> CommonError {
    CommonError::new(ErrorCode::BackendUnavailable, msg)
}

/// Pin fields become path components, so they must be safe names.
fn check_pin(pin: &BackendPin) -> Result<()> {
    for s in [&pin.name, &pin.release, &pin.archive] {
        if !safe_name(s) {
            return Err(CommonError::invalid(format!(
                "backend pin field {s:?} is not a safe name"
            )));
        }
    }
    // Empty `directory`: the binaries sit at the archive root (llama.cpp's Windows .zip releases).
    for s in [&pin.directory].into_iter().filter(|d| !d.is_empty()) {
        if !safe_name(s) {
            return Err(CommonError::invalid(format!(
                "backend pin field {s:?} is not a safe name"
            )));
        }
    }
    if !is_sha256_hex(&pin.sha256) {
        return Err(CommonError::invalid(format!(
            "backend pin {} has a malformed sha256",
            pin.name
        )));
    }
    Ok(())
}

/// Archive hash must match the pin; a mismatching archive is deleted so the next run re-downloads.
fn verify_archive(archive: &Path, pin: &BackendPin) -> Result<()> {
    let actual = sha256_file(archive)?;
    if actual != pin.sha256 {
        std::fs::remove_file(archive)?;
        return Err(CommonError::new(
            ErrorCode::IntegrityMismatch,
            format!(
                "backend archive {} hashed to {actual}, expected {}; deleted",
                pin.archive, pin.sha256
            ),
        ));
    }
    Ok(())
}

/// The two release formats llama.cpp ships: `.tar.gz` (macOS, Linux) and `.zip` (Windows).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    TarGz,
    Zip,
}

fn archive_kind(pin: &BackendPin) -> Result<Kind> {
    if pin.archive.ends_with(".tar.gz") {
        Ok(Kind::TarGz)
    } else if pin.archive.ends_with(".zip") {
        Ok(Kind::Zip)
    } else {
        Err(CommonError::invalid(format!(
            "backend archive {} is neither .tar.gz nor .zip",
            pin.archive
        )))
    }
}

type Tar = tar::Archive<flate2::read::GzDecoder<std::io::BufReader<std::fs::File>>>;

fn open_archive(archive: &Path) -> Result<Tar> {
    let f = std::fs::File::open(archive)?;
    Ok(tar::Archive::new(flate2::read::GzDecoder::new(
        std::io::BufReader::new(f),
    )))
}

/// Entry path relative to `directory` (empty for the directory entry itself). Every entry must be
/// a relative path under `directory` with no `..`; anything else is rejected.
fn rel_path(p: &Path, directory: &str) -> Result<PathBuf> {
    let bad = || unavailable(format!("unsafe or unexpected archive entry {}", p.display()));
    let mut comps = p.components().filter(|c| !matches!(c, Component::CurDir)).peekable();
    if !directory.is_empty() {
        match comps.next() {
            Some(Component::Normal(top)) if top == directory => {}
            _ => return Err(bad()),
        }
    } else if comps.peek().is_none() {
        return Err(bad());
    }
    let mut rel = PathBuf::new();
    for c in comps {
        match c {
            Component::Normal(n) => rel.push(n),
            _ => return Err(bad()),
        }
    }
    Ok(rel)
}

/// Lexically resolve a symlink target against the link's parent and require it to stay in-tree.
fn check_link(rel: &Path, link: &Path) -> Result<()> {
    let mut depth: Vec<&std::ffi::OsStr> = rel
        .parent()
        .map(|p| {
            p.components()
                .filter_map(|c| if let Component::Normal(n) = c { Some(n) } else { None })
                .collect()
        })
        .unwrap_or_default();
    for c in link.components() {
        match c {
            Component::Normal(n) => depth.push(n),
            Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return Err(unavailable(format!(
                        "symlink {} escapes the backend directory",
                        rel.display()
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(unavailable(format!("symlink {} has an absolute target", rel.display())));
            }
        }
    }
    Ok(())
}

/// Extract into a fresh temp dir next to `dir`, then rename it to `dir`. Any rejected entry
/// aborts the whole extraction and the temp dir is removed, so nothing partial ever lands.
fn extract(archive: &Path, pin: &BackendPin, dir: &Path) -> Result<()> {
    let kind = archive_kind(pin)?;
    let parent = dir
        .parent()
        .ok_or_else(|| CommonError::internal("backend directory has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let tmp = tempfile::Builder::new().prefix(".extract-").tempdir_in(parent)?;
    let root = tmp.path();
    if kind == Kind::Zip {
        extract_zip(archive, pin, root)?;
        std::fs::rename(root, dir)?;
        let _ = tmp.keep();
        return Ok(());
    }
    let mut ar = open_archive(archive)?;
    for entry in ar.entries()? {
        let mut entry = entry?;
        let ty = entry.header().entry_type();
        if ty == tar::EntryType::XGlobalHeader {
            continue;
        }
        let rel = rel_path(&entry.path()?, &pin.directory)?;
        let target = root.join(&rel);
        if ty.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if ty.is_file() {
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            // create_new: never write through a pre-existing path or symlink.
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
            std::io::copy(&mut entry, &mut f)?;
            set_mode(&target, entry.header().mode()?)?;
        } else if ty.is_symlink() {
            let link = entry
                .link_name()?
                .ok_or_else(|| unavailable(format!("symlink {} has no target", rel.display())))?
                .into_owned();
            check_link(&rel, &link)?;
            if let Some(p) = target.parent() {
                std::fs::create_dir_all(p)?;
            }
            symlink(&link, &target)?;
        } else {
            return Err(unavailable(format!(
                "archive entry {} has unsupported type {:?}",
                rel.display(),
                ty
            )));
        }
    }
    std::fs::rename(root, dir)?;
    let _ = tmp.keep();
    Ok(())
}

/// Every archive entry must exist on disk with the same content (regular files), the same target
/// (symlinks) or as a directory. Extra files on disk are not checked.
fn verify_tree(archive: &Path, pin: &BackendPin, dir: &Path) -> Result<()> {
    if archive_kind(pin)? == Kind::Zip {
        return verify_zip_tree(archive, pin, dir);
    }
    let mut ar = open_archive(archive)?;
    for entry in ar.entries()? {
        let mut entry = entry?;
        let ty = entry.header().entry_type();
        if ty == tar::EntryType::XGlobalHeader {
            continue;
        }
        let rel = rel_path(&entry.path()?, &pin.directory)?;
        let on_disk = dir.join(&rel);
        let meta = std::fs::symlink_metadata(&on_disk)
            .map_err(|_| unavailable(format!("{} is missing from {}", rel.display(), dir.display())))?;
        if ty.is_dir() {
            if !meta.is_dir() {
                return Err(unavailable(format!("{} should be a directory", rel.display())));
            }
        } else if ty.is_file() {
            if !meta.is_file() {
                return Err(unavailable(format!("{} should be a regular file", rel.display())));
            }
            if entry.header().mode()? & 0o111 != 0 && !is_executable(&meta) {
                return Err(unavailable(format!("{} lost its executable bit", rel.display())));
            }
            let expected = sha256_reader(&mut entry)?;
            if expected != sha256_file(&on_disk)? {
                return Err(unavailable(format!(
                    "{} does not match the pinned archive",
                    rel.display()
                )));
            }
        } else if ty.is_symlink() {
            let want = entry.link_name()?.map(|l| l.into_owned()).unwrap_or_default();
            let got = std::fs::read_link(&on_disk)
                .map_err(|_| unavailable(format!("{} should be a symlink", rel.display())))?;
            if got != want {
                return Err(unavailable(format!(
                    "symlink {} points to the wrong target",
                    rel.display()
                )));
            }
        } else {
            return Err(unavailable(format!(
                "archive entry {} has unsupported type {:?}",
                rel.display(),
                ty
            )));
        }
    }
    Ok(())
}

/// A zip entry's path relative to the pin's directory. Zip names are `/`-separated; a backslash,
/// a drive letter or a NUL is rejected rather than interpreted, so a name means the same thing on
/// every OS before [`rel_path`] applies the shared rules (relative, no `..`).
fn zip_rel(name: &str, pin: &BackendPin) -> Result<PathBuf> {
    if name.contains('\\') || name.contains(':') || name.contains('\0') {
        return Err(unavailable(format!("unsafe or unexpected archive entry {name}")));
    }
    rel_path(Path::new(name.trim_end_matches('/')), &pin.directory)
}

fn open_zip(archive: &Path) -> Result<zip::ZipArchive<std::io::BufReader<std::fs::File>>> {
    let f = std::io::BufReader::new(std::fs::File::open(archive)?);
    zip::ZipArchive::new(f).map_err(|e| unavailable(format!("cannot read {}: {e}", archive.display())))
}

fn zip_err(e: zip::result::ZipError) -> CommonError {
    unavailable(format!("backend archive: {e}"))
}

/// Same rules as the .tar.gz path: regular files and directories only (symlinks and encrypted
/// entries are rejected), every path relative and inside the tree, `create_new` for every file.
fn extract_zip(archive: &Path, pin: &BackendPin, root: &Path) -> Result<()> {
    let mut z = open_zip(archive)?;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(zip_err)?;
        let name = e.name().map_err(zip_err)?.into_owned();
        let rel = zip_rel(&name, pin)?;
        if e.is_symlink() || e.encrypted() {
            return Err(unavailable(format!("archive entry {name} is a symlink or encrypted")));
        }
        let target = root.join(&rel);
        if e.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&target)?;
        std::io::copy(&mut e, &mut f)?;
        set_mode(&target, e.unix_mode().unwrap_or(0o644))?;
    }
    Ok(())
}

/// Every zip entry must exist on disk with the same content (files) or as a directory.
fn verify_zip_tree(archive: &Path, pin: &BackendPin, dir: &Path) -> Result<()> {
    let mut z = open_zip(archive)?;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(zip_err)?;
        let name = e.name().map_err(zip_err)?.into_owned();
        let rel = zip_rel(&name, pin)?;
        let on_disk = dir.join(&rel);
        let meta = std::fs::symlink_metadata(&on_disk)
            .map_err(|_| unavailable(format!("{} is missing from {}", rel.display(), dir.display())))?;
        if e.is_dir() {
            if !meta.is_dir() {
                return Err(unavailable(format!("{} should be a directory", rel.display())));
            }
            continue;
        }
        if !meta.is_file() {
            return Err(unavailable(format!("{} should be a regular file", rel.display())));
        }
        if e.unix_mode().is_some_and(|m| m & 0o111 != 0) && !is_executable(&meta) {
            return Err(unavailable(format!("{} lost its executable bit", rel.display())));
        }
        if sha256_reader(&mut e)? != sha256_file(&on_disk)? {
            return Err(unavailable(format!(
                "{} does not match the pinned archive",
                rel.display()
            )));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode((mode & 0o777) | 0o600))
}
#[cfg(unix)]
fn is_executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o100 != 0
}
#[cfg(unix)]
fn symlink(link: &Path, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(link, at)
}

// Windows: .zip releases (no symlinks, no mode bits); see extract_zip.
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> std::io::Result<()> {
    Ok(())
}
#[cfg(not(unix))]
fn is_executable(_: &std::fs::Metadata) -> bool {
    true
}
#[cfg(not(unix))]
fn symlink(_: &Path, _: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "symlinks in backend archives are not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256_bytes;
    use std::io::Write;
    use tar::EntryType as T;

    /// Default `COMMON_HOME`, falling back to the repo's `.common`; None when it holds no backends.
    fn common_home() -> Option<Paths> {
        let root = std::env::var_os("COMMON_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.common"));
        let p = Paths::at(root);
        if p.backends().is_dir() {
            return Some(p);
        }
        skip(&format!("no backends directory in {}", p.root.display()));
        None
    }

    /// A missing asset: SKIP, or a failure under `COMMON_REQUIRE_FIXTURES=1` (the evidence job).
    fn skip(why: &str) {
        assert!(
            std::env::var_os("COMMON_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
            "COMMON_REQUIRE_FIXTURES=1: {why}"
        );
        eprintln!("SKIP: {why}");
    }

    /// This platform's llama.cpp pin: the one `fetch-fixtures.sh` / `common setup` provisioned.
    fn llama_pin() -> BackendPin {
        BackendPins::builtin()
            .unwrap()
            .for_current_platform("llama.cpp")
            .expect("llama.cpp is pinned for this platform")
            .clone()
    }

    /// Raw ustar header: `Builder::append` writes it verbatim, so `..`/absolute names get through.
    fn raw_header(name: &str, ty: T, link: &str, size: u64, mode: u32) -> tar::Header {
        let mut h = tar::Header::new_ustar();
        h.as_mut_bytes()[..name.len()].copy_from_slice(name.as_bytes());
        h.as_mut_bytes()[157..157 + link.len()].copy_from_slice(link.as_bytes());
        h.set_entry_type(ty);
        h.set_size(size);
        h.set_mode(mode);
        h.set_cksum();
        h
    }

    fn gz_archive(entries: &[(&str, T, &str, &[u8], u32)]) -> Vec<u8> {
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(enc);
        for (name, ty, link, data, mode) in entries {
            b.append(&raw_header(name, *ty, link, data.len() as u64, *mode), *data)
                .unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn pin_for(archive: &[u8]) -> BackendPin {
        BackendPin {
            name: "test".into(),
            release: "r1".into(),
            source_commit: "0".into(),
            platform: current_platform(),
            archive: "test.tar.gz".into(),
            directory: "pkg".into(),
            url: "https://example.invalid/test.tar.gz".into(),
            sha256: sha256_bytes(archive),
            bytes: archive.len() as u64,
            license: "MIT".into(),
        }
    }

    fn home_with(archive: &[u8]) -> (tempfile::TempDir, Paths, BackendPin) {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::at(d.path().join("home"));
        let pin = pin_for(archive);
        let ap = archive_path(&paths, &pin);
        std::fs::create_dir_all(ap.parent().unwrap()).unwrap();
        std::fs::File::create(&ap).unwrap().write_all(archive).unwrap();
        (d, paths, pin)
    }

    fn only_archive_left(paths: &Paths, pin: &BackendPin) -> bool {
        let names: Vec<_> = std::fs::read_dir(paths.backends().join(&pin.name))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.len() == 1 && names[0] == pin.archive.as_str()
    }

    #[test]
    fn builtin_pins_are_sane() {
        let pins = BackendPins::builtin().unwrap();
        assert!(!pins.backends.is_empty());
        for p in &pins.backends {
            check_pin(p).unwrap();
            assert!(p.url.starts_with("https://"), "{}", p.name);
            assert!(p.bytes > 0 && !p.license.is_empty() && !p.platform.is_empty());
        }
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(current_platform(), "macos-arm64");
            assert!(pins.for_current_platform("llama.cpp").is_some());
        }
        assert!(pins.for_current_platform("nope").is_none());
        assert_eq!(
            binary(Path::new("/b"), "llama-server"),
            PathBuf::from(if cfg!(windows) {
                "/b/llama-server.exe"
            } else {
                "/b/llama-server"
            })
        );
    }

    #[test]
    fn rel_path_rules() {
        assert_eq!(rel_path(Path::new("pkg"), "pkg").unwrap(), PathBuf::new());
        assert_eq!(rel_path(Path::new("./pkg/a/b"), "pkg").unwrap(), PathBuf::from("a/b"));
        for bad in ["../escape", "/absolute", "pkg/../x", "other/x", "", ".", "/pkg/x"] {
            assert_eq!(
                rel_path(Path::new(bad), "pkg").unwrap_err().code,
                ErrorCode::BackendUnavailable,
                "{bad}"
            );
        }
    }

    #[test]
    fn symlink_target_rules() {
        check_link(Path::new("lib.dylib"), Path::new("lib.1.dylib")).unwrap();
        check_link(Path::new("a/b/link"), Path::new("../../x")).unwrap();
        check_link(Path::new("a/link"), Path::new("./c/../d")).unwrap();
        for (rel, link) in [
            ("link", "../../escape"),
            ("a/link", "../../escape"),
            ("link", "/etc/passwd"),
            ("link", ".."),
        ] {
            assert_eq!(
                check_link(Path::new(rel), Path::new(link)).unwrap_err().code,
                ErrorCode::BackendUnavailable,
                "{rel} -> {link}"
            );
        }
    }

    #[tokio::test]
    async fn malicious_archives_extract_nothing() {
        let cases: [(&str, T, &str); 7] = [
            ("../escape", T::Regular, ""),
            ("/absolute", T::Regular, ""),
            ("pkg/link", T::Symlink, "../../escape"),
            ("pkg/abs", T::Symlink, "/etc/passwd"),
            ("pkg/fifo", T::Fifo, ""),
            ("pkg/hard", T::Link, "pkg/safe.txt"),
            ("outside.txt", T::Regular, ""),
        ];
        for (name, ty, link) in cases {
            let archive = gz_archive(&[
                ("pkg", T::Directory, "", b"", 0o755),
                ("pkg/safe.txt", T::Regular, "", b"ok", 0o644),
                (name, ty, link, if ty == T::Regular { b"evil" } else { b"" }, 0o644),
            ]);
            let (d, paths, pin) = home_with(&archive);
            let err = ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::BackendUnavailable, "{name}: {err}");
            assert!(
                !backend_dir(&paths, &pin).exists(),
                "{name}: backend dir must not exist"
            );
            assert!(only_archive_left(&paths, &pin), "{name}: temp dir must be cleaned up");
            assert!(!d.path().join("escape").exists() && !Path::new("/absolute").exists());
            assert_eq!(
                verify_backend(&paths, &pin).unwrap_err().code,
                ErrorCode::BackendUnavailable
            );
        }
    }

    #[tokio::test]
    async fn safe_archive_extracts_verifies_and_detects_tampering() {
        let archive = gz_archive(&[
            ("pkg", T::Directory, "", b"", 0o755),
            ("pkg/lib.1.dylib", T::Regular, "", b"ok", 0o755),
            ("pkg/lib.dylib", T::Symlink, "lib.1.dylib", b"", 0o777),
            ("pkg/sub", T::Directory, "", b"", 0o755),
            ("pkg/sub/deep", T::Symlink, "../lib.1.dylib", b"", 0o777),
            ("pkg/README", T::Regular, "", b"read me", 0o644),
        ]);
        let (_d, paths, pin) = home_with(&archive);
        let mut n = 0;
        let dir = ensure_backend(&paths, &pin, &mut |_| n += 1).await.unwrap();
        assert_eq!(n, 0, "no download when the archive is present");
        assert_eq!(dir, backend_dir(&paths, &pin));
        assert_eq!(std::fs::read(dir.join("lib.dylib")).unwrap(), b"ok");
        assert_eq!(std::fs::read(dir.join("sub/deep")).unwrap(), b"ok");
        assert!(is_executable(&std::fs::metadata(dir.join("lib.1.dylib")).unwrap()));
        assert!(!is_executable(&std::fs::metadata(dir.join("README")).unwrap()));
        assert_eq!(verify_backend(&paths, &pin).unwrap(), dir);
        assert_eq!(
            ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap(),
            dir,
            "idempotent"
        );

        // tamper with a file
        std::fs::write(dir.join("README"), b"read mE").unwrap();
        let err = verify_backend(&paths, &pin).unwrap_err();
        assert_eq!(err.code, ErrorCode::BackendUnavailable);
        assert!(err.message.contains("README"), "{err}");
        std::fs::write(dir.join("README"), b"read me").unwrap();
        verify_backend(&paths, &pin).unwrap();

        // retarget a symlink
        std::fs::remove_file(dir.join("lib.dylib")).unwrap();
        std::os::unix::fs::symlink("README", dir.join("lib.dylib")).unwrap();
        let err = ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap_err();
        assert!(
            err.code == ErrorCode::BackendUnavailable && err.message.contains("lib.dylib"),
            "{err}"
        );
        std::fs::remove_file(dir.join("lib.dylib")).unwrap();
        std::os::unix::fs::symlink("lib.1.dylib", dir.join("lib.dylib")).unwrap();
        verify_backend(&paths, &pin).unwrap();

        // missing file
        std::fs::remove_file(dir.join("sub/deep")).unwrap();
        assert!(verify_backend(&paths, &pin).unwrap_err().message.contains("sub/deep"));

        // corrupt archive -> IntegrityMismatch and the archive is deleted
        let ap = archive_path(&paths, &pin);
        let mut bytes = std::fs::read(&ap).unwrap();
        bytes[10] ^= 0xff;
        std::fs::write(&ap, bytes).unwrap();
        assert_eq!(
            verify_backend(&paths, &pin).unwrap_err().code,
            ErrorCode::IntegrityMismatch
        );
        assert!(!ap.exists());
        assert_eq!(
            verify_backend(&paths, &pin).unwrap_err().code,
            ErrorCode::BackendUnavailable
        );
    }

    #[tokio::test]
    async fn http_pin_is_refused_and_bad_pins_rejected() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::at(d.path());
        let mut pin = pin_for(b"x");
        pin.url = "http://example.invalid/x.tar.gz".into();
        assert_eq!(
            ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap_err().code,
            ErrorCode::PolicyDenied
        );
        assert_eq!(
            verify_backend(&paths, &pin).unwrap_err().code,
            ErrorCode::BackendUnavailable
        );
        let mut pin = pin_for(b"x");
        pin.release = "../r".into();
        assert_eq!(
            verify_backend(&paths, &pin).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        let mut pin = pin_for(b"x");
        pin.sha256 = "zz".into();
        assert_eq!(
            ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }

    /// Against the real pinned llama.cpp b11609 archive + directory in COMMON_HOME (skips if absent).
    #[tokio::test]
    async fn verifies_real_llama_cpp_backend_in_common_home() {
        let Some(paths) = common_home() else { return };
        let pin = llama_pin();
        let dir = verify_backend(&paths, &pin).unwrap();
        assert_eq!(dir, paths.backends().join("llama.cpp").join("b11609"));
        assert!(binary(&dir, "llama-server").is_file());
        assert!(is_executable(&std::fs::metadata(binary(&dir, "llama-server")).unwrap()));
        let mut n = 0;
        assert_eq!(ensure_backend(&paths, &pin, &mut |_| n += 1).await.unwrap(), dir);
        assert_eq!(n, 0, "no network needed");
    }

    /// Extract the real archive into a fresh home (no network) and verify the result.
    #[tokio::test]
    async fn extracts_real_llama_cpp_archive_into_fresh_home() {
        let Some(home) = common_home() else { return };
        let pin = llama_pin();
        let src = archive_path(&home, &pin);
        if !src.is_file() {
            return skip(&format!("archive {} missing", src.display()));
        }
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::at(d.path().join("home"));
        let dst = archive_path(&paths, &pin);
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::copy(&src, &dst).unwrap();
        let dir = ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap();
        assert!(binary(&dir, "llama-server").is_file());
        assert!(is_executable(&std::fs::metadata(binary(&dir, "llama-server")).unwrap()));
        // The archive's library symlink survives extraction (the Windows zip has none). Linux names
        // from the ubuntu-x64 b11609 archive: libllama.so -> libllama.so.0.
        if !cfg!(windows) {
            let (link, target) = if cfg!(target_os = "macos") {
                ("libllama.dylib", "libllama.0.dylib")
            } else {
                ("libllama.so", "libllama.so.0")
            };
            assert!(std::fs::symlink_metadata(dir.join(link))
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read_link(dir.join(link)).unwrap(), PathBuf::from(target));
        }
        assert_eq!(verify_backend(&paths, &pin).unwrap(), dir);
        let leftovers: Vec<_> = std::fs::read_dir(paths.backends().join(&pin.name))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers.len(), 2, "{leftovers:?}");
    }

    /// A .zip release in a fresh home, with the pin's `directory` as given ("" = archive root).
    fn zip_home(entries: &[(&str, &[u8], u32)], directory: &str) -> (tempfile::TempDir, Paths, BackendPin) {
        use zip::write::SimpleFileOptions;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, data, mode) in entries {
            let opts = SimpleFileOptions::default().unix_permissions(*mode);
            if name.ends_with('/') {
                w.add_directory(*name, opts).unwrap();
            } else if *mode & 0o170000 == 0o120000 {
                w.add_symlink(*name, std::str::from_utf8(data).unwrap(), opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
        }
        let bytes = w.finish().unwrap().into_inner();
        let (d, paths, mut pin) = home_with(&bytes);
        let old = archive_path(&paths, &pin);
        pin.archive = "test.zip".into();
        pin.directory = directory.into();
        std::fs::rename(old, archive_path(&paths, &pin)).unwrap();
        (d, paths, pin)
    }

    #[tokio::test]
    async fn flat_zip_extracts_verifies_and_detects_tampering() {
        let (_d, paths, pin) = zip_home(
            &[
                ("llama-server.exe", b"MZ engine", 0o755),
                ("ggml-base.dll", b"dll", 0o644),
                ("sub/", b"", 0o755),
                ("sub/LICENSE", b"MIT", 0o644),
            ],
            "",
        );
        let dir = ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap();
        assert_eq!(
            std::fs::read(binary(&dir, "llama-server").with_extension("exe")).unwrap(),
            b"MZ engine"
        );
        assert_eq!(std::fs::read(dir.join("sub/LICENSE")).unwrap(), b"MIT");
        assert_eq!(verify_backend(&paths, &pin).unwrap(), dir);
        std::fs::write(dir.join("ggml-base.dll"), b"DLL").unwrap();
        let err = verify_backend(&paths, &pin).unwrap_err();
        assert!(
            err.code == ErrorCode::BackendUnavailable && err.message.contains("ggml-base.dll"),
            "{err}"
        );
        std::fs::remove_file(dir.join("sub/LICENSE")).unwrap();
        std::fs::write(dir.join("ggml-base.dll"), b"dll").unwrap();
        assert!(verify_backend(&paths, &pin).unwrap_err().message.contains("LICENSE"));
    }

    #[tokio::test]
    async fn malicious_zips_extract_nothing() {
        for (name, mode) in [
            ("../escape", 0o644),
            ("/absolute", 0o644),
            ("C:/windows/evil.dll", 0o644),
            ("sub\\..\\..\\evil.dll", 0o644),
            ("link", 0o120777),
        ] {
            let data: &[u8] = if mode == 0o120777 { b"../../escape" } else { b"evil" };
            let (d, paths, pin) = zip_home(&[("safe.txt", b"ok", 0o644), (name, data, mode)], "");
            let err = ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap_err();
            assert_eq!(err.code, ErrorCode::BackendUnavailable, "{name}: {err}");
            assert!(!backend_dir(&paths, &pin).exists(), "{name}: nothing extracted");
            assert!(only_archive_left(&paths, &pin), "{name}: temp dir cleaned up");
            assert!(!d.path().join("escape").exists());
        }
    }

    #[test]
    fn the_linux_pin_is_the_cpu_build_in_its_release_directory() {
        // D-024: the Ubuntu CPU build (EXPERIMENTAL platform; the Vulkan and CUDA builds are not pinned).
        let pins = BackendPins::builtin().unwrap();
        let p = pins
            .backends
            .iter()
            .find(|b| b.platform == "linux-x64")
            .expect("no linux-x64 pin");
        assert_eq!(p.archive, "llama-b11609-bin-ubuntu-x64.tar.gz");
        assert!(p.directory == "llama-b11609" && p.url.ends_with(&p.archive), "{p:?}");
        check_pin(p).unwrap();
        assert_eq!(archive_kind(p).unwrap(), Kind::TarGz);
    }

    #[test]
    fn windows_pins_point_at_zip_releases_at_the_archive_root() {
        let pins = BackendPins::builtin().unwrap();
        for plat in ["win-x64", "win-arm64"] {
            let p = pins
                .backends
                .iter()
                .find(|b| b.platform == plat)
                .unwrap_or_else(|| panic!("no {plat} pin"));
            assert!(
                p.archive.ends_with(".zip") && p.directory.is_empty() && p.url.ends_with(&p.archive),
                "{p:?}"
            );
            check_pin(p).unwrap();
            assert_eq!(archive_kind(p).unwrap(), Kind::Zip);
        }
    }

    /// The real llama.cpp Windows release: `COMMON_WIN_ENGINE_ZIP=<downloaded zip> cargo test -p
    /// common-artifacts -- --ignored extracts_real_windows`. Opt-in (no fixture provisions it), so it
    /// is reported as ignored rather than passing without running.
    #[tokio::test]
    #[ignore = "needs COMMON_WIN_ENGINE_ZIP=<downloaded llama.cpp Windows release>"]
    async fn extracts_real_windows_llama_cpp_zip() {
        let zip_path = PathBuf::from(std::env::var_os("COMMON_WIN_ENGINE_ZIP").expect("set COMMON_WIN_ENGINE_ZIP"));
        let name = zip_path.file_name().unwrap().to_str().unwrap().to_string();
        let pin = BackendPins::builtin()
            .unwrap()
            .backends
            .into_iter()
            .find(|b| b.archive == name)
            .expect("a pinned archive");
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::at(d.path());
        std::fs::create_dir_all(archive_path(&paths, &pin).parent().unwrap()).unwrap();
        std::fs::copy(&zip_path, archive_path(&paths, &pin)).unwrap();
        let dir = ensure_backend(&paths, &pin, &mut |_| {}).await.unwrap();
        assert!(dir.join("llama-server.exe").is_file());
        assert!(dir.join("ggml-base.dll").is_file());
        verify_backend(&paths, &pin).unwrap();
    }
}
