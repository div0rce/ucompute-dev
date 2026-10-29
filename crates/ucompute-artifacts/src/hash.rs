//! SHA-256 helpers.
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use ucompute_core::Result;

/// Streaming SHA-256 of a file, lower-case hex. 1 MiB read buffer.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    Ok(sha256_reader(&mut f)?)
}

/// Streaming SHA-256 of any reader (used for files and tar entries).
pub fn sha256_reader(r: &mut impl Read) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// True for a 64-char lower/upper-case hex digest.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn known_vector() {
        assert_eq!(sha256_bytes(b"abc"), ABC);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("abc");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), ABC);
        assert_eq!(
            sha256_file(Path::new("/nonexistent/x")).unwrap_err().code,
            ucompute_core::ErrorCode::Io
        );
    }

    #[test]
    fn file_larger_than_buffer_matches_bytes() {
        let data: Vec<u8> = (0..(3 << 20) + 17).map(|i| (i % 251) as u8).collect();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big");
        std::fs::write(&p, &data).unwrap();
        assert_eq!(sha256_file(&p).unwrap(), sha256_bytes(&data));
    }

    #[test]
    fn hex_check() {
        assert!(is_sha256_hex(ABC));
        assert!(!is_sha256_hex(&ABC[1..]));
        assert!(!is_sha256_hex(&format!("{}g", &ABC[1..])));
    }
}
