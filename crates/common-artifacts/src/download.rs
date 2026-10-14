//! Verified, resumable downloads (F-01-05, F-05-02).
//!
//! One downloader serves model blobs and backend archives: stream to `<partial>`, resume with
//! HTTP `Range`, enforce the pinned byte count, hash, then rename into place. A hash mismatch or
//! an oversize body deletes the partial; a dropped connection keeps it for the next attempt.

use crate::catalog::ModelEntry;
use crate::hash::{is_sha256_hex, sha256_file};
use crate::store::{utc_now, ModelManifest, Store};
use common_core::paths::safe_name;
use common_core::{CommonError, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Progress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// Bytes fetched over the network during this call (0 on cache hit).
    pub network_bytes: u64,
    pub resumed_from: u64,
    pub done: bool,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum gap between two body reads before the attempt fails (the partial stays resumable).
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Download `entry` into the store. Resumes a partial file via HTTP Range, verifies SHA-256,
/// deletes the file on mismatch, and is a no-op (zero network bytes) when the blob is already
/// present and valid. Requires `https://` URLs.
pub async fn pull(store: &Store, entry: &ModelEntry, progress: &mut dyn FnMut(Progress)) -> Result<ModelManifest> {
    pull_inner(store, entry, progress, true).await
}

async fn pull_inner(
    store: &Store,
    entry: &ModelEntry,
    progress: &mut dyn FnMut(Progress),
    https_only: bool,
) -> Result<ModelManifest> {
    if !safe_name(&entry.id) {
        return Err(CommonError::invalid(format!("invalid model id {:?}", entry.id)));
    }
    if !is_sha256_hex(&entry.sha256) {
        return Err(CommonError::invalid(format!(
            "model {} has a malformed sha256",
            entry.id
        )));
    }
    let blob = store.blob_path(&entry.sha256);
    if blob.is_file() {
        let actual = hash_blocking(blob.clone()).await?;
        if actual != entry.sha256 {
            tokio::fs::remove_file(&blob).await?;
            return Err(CommonError::new(
                ErrorCode::IntegrityMismatch,
                format!(
                    "cached blob for {} hashed to {actual}, expected {}; deleted",
                    entry.id, entry.sha256
                ),
            ));
        }
        progress(Progress {
            downloaded_bytes: entry.bytes,
            total_bytes: entry.bytes,
            network_bytes: 0,
            resumed_from: 0,
            done: true,
        });
        if let Some(m) = store.get(&entry.id)?.filter(|m| m.sha256 == entry.sha256) {
            return Ok(m);
        }
        let m = manifest_from(entry);
        store.write_manifest(&m)?;
        return Ok(m);
    }
    if https_only {
        require_https(&entry.url)?;
    }
    let partial = store.paths.partial().join(format!("{}.part", entry.sha256));
    fetch(
        &entry.url,
        &entry.sha256,
        entry.bytes,
        &partial,
        &blob,
        https_only,
        progress,
    )
    .await?;
    let m = manifest_from(entry);
    store.write_manifest(&m)?;
    Ok(m)
}

pub(crate) fn require_https(url: &str) -> Result<()> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(CommonError::new(
            ErrorCode::PolicyDenied,
            "downloads require an https:// URL",
        ))
    }
}

/// Stream `url` into `partial`, resuming from its current length, then verify `bytes` and
/// `sha256` and rename to `dest`. Returns the bytes fetched over the network by this call.
pub(crate) async fn fetch(
    url: &str,
    sha256: &str,
    bytes: u64,
    partial: &Path,
    dest: &Path,
    https_only: bool,
    progress: &mut dyn FnMut(Progress),
) -> Result<u64> {
    for p in [partial.parent(), dest.parent()].into_iter().flatten() {
        tokio::fs::create_dir_all(p).await?;
    }
    let mut have = tokio::fs::metadata(partial).await.map(|m| m.len()).unwrap_or(0);
    if have >= bytes {
        // ponytail: a partial at or past the pinned size is an unverified leftover; restart instead of hash-and-promote.
        tokio::fs::remove_file(partial).await?;
        have = 0;
    }
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STALL_TIMEOUT)
        .https_only(https_only)
        .build()
        .map_err(net)?;
    let mut req = client.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req.send().await.map_err(net)?;
    let mut file = match resp.status().as_u16() {
        206 if have > 0 => {
            let range = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if !range.starts_with(&format!("bytes {have}-")) {
                return Err(CommonError::new(
                    ErrorCode::DownloadFailed,
                    format!("server resumed at the wrong offset (Content-Range {range:?})"),
                ));
            }
            tokio::fs::OpenOptions::new().append(true).open(partial).await?
        }
        200 => {
            have = 0;
            tokio::fs::File::create(partial).await?
        }
        s => {
            return Err(CommonError::new(
                ErrorCode::DownloadFailed,
                format!("server returned HTTP {s}"),
            ))
        }
    };
    let resumed_from = have;
    let mut downloaded = have;
    let mut network = 0u64;
    let report = |downloaded, network, done| Progress {
        downloaded_bytes: downloaded,
        total_bytes: bytes,
        network_bytes: network,
        resumed_from,
        done,
    };
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                file.flush().await?;
                return Err(net(e));
            }
        };
        downloaded += chunk.len() as u64;
        network += chunk.len() as u64;
        if downloaded > bytes {
            drop(file);
            tokio::fs::remove_file(partial).await?;
            return Err(CommonError::new(
                ErrorCode::DownloadFailed,
                format!("body exceeds the pinned size of {bytes} bytes; partial deleted"),
            ));
        }
        file.write_all(&chunk).await?;
        progress(report(downloaded, network, false));
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    if downloaded < bytes {
        return Err(CommonError::new(
            ErrorCode::DownloadFailed,
            format!("connection closed after {downloaded} of {bytes} bytes"),
        )
        .with_hint("run the same command again to resume"));
    }
    let actual = hash_blocking(partial.to_path_buf()).await?;
    if actual != sha256 {
        tokio::fs::remove_file(partial).await?;
        return Err(CommonError::new(
            ErrorCode::IntegrityMismatch,
            format!("downloaded file hashed to {actual}, expected {sha256}; partial deleted"),
        ));
    }
    tokio::fs::rename(partial, dest).await?;
    progress(report(downloaded, network, true));
    Ok(network)
}

async fn hash_blocking(path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || sha256_file(&path))
        .await
        .map_err(|e| CommonError::internal(e.to_string()))?
}

/// reqwest errors without the URL (which may carry access tokens).
fn net(e: reqwest::Error) -> CommonError {
    let code = if e.is_timeout() {
        ErrorCode::Timeout
    } else {
        ErrorCode::DownloadFailed
    };
    CommonError::new(code, e.without_url().to_string())
}

fn manifest_from(e: &ModelEntry) -> ModelManifest {
    ModelManifest {
        schema_version: ModelManifest::SCHEMA_VERSION,
        id: e.id.clone(),
        name: e.name.clone(),
        file_name: e.file_name.clone(),
        sha256: e.sha256.clone(),
        bytes: e.bytes,
        license: e.license.clone(),
        license_url: e.license_url.clone(),
        source_url: e.url.clone(),
        redistribution: e.redistribution.clone(),
        imported_at_utc: utc_now(),
        chat: e.chat,
        tags: e.tags.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256_bytes;
    use common_core::Paths;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;

    const MIB: u64 = 1 << 20;

    /// Minimal HTTP/1.1 file server on 127.0.0.1: honors `Range` (unless `ignore_range`), counts
    /// bytes served, and drops the connection once the absolute file offset reaches `cut`.
    struct Server {
        url: String,
        served: Arc<AtomicU64>,
        cut: Arc<AtomicU64>,
        ignore_range: Arc<AtomicBool>,
    }

    async fn serve(data: Arc<Vec<u8>>) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/model.gguf", listener.local_addr().unwrap());
        let served = Arc::new(AtomicU64::new(0));
        let cut = Arc::new(AtomicU64::new(u64::MAX));
        let ignore_range = Arc::new(AtomicBool::new(false));
        let (s, c, ir) = (served.clone(), cut.clone(), ignore_range.clone());
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let (data, served, cut, ignore_range) = (data.clone(), s.clone(), c.clone(), ir.clone());
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = sock.read(&mut buf).await.unwrap();
                        if n == 0 {
                            return;
                        }
                        head.extend_from_slice(&buf[..n]);
                    }
                    let head = String::from_utf8_lossy(&head).to_lowercase();
                    let mut start = head
                        .lines()
                        .find_map(|l| l.strip_prefix("range: bytes="))
                        .and_then(|r| r.trim_end_matches('-').parse::<u64>().ok())
                        .unwrap_or(0) as usize;
                    if ignore_range.load(Ordering::SeqCst) {
                        start = 0;
                    }
                    let total = data.len();
                    let header = if start > 0 {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                            total - start, start, total - 1, total
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            total
                        )
                    };
                    sock.write_all(header.as_bytes()).await.unwrap();
                    let end = (cut.load(Ordering::SeqCst).min(total as u64) as usize).max(start);
                    sock.write_all(&data[start..end]).await.ok();
                    served.fetch_add((end - start) as u64, Ordering::SeqCst);
                    sock.flush().await.ok();
                    if end == total {
                        sock.shutdown().await.ok();
                    }
                    // dropping the socket early = cut connection
                });
            }
        });
        Server {
            url,
            served,
            cut,
            ignore_range,
        }
    }

    fn data(len: usize) -> Arc<Vec<u8>> {
        Arc::new((0..len).map(|i| (i as u32).wrapping_mul(2_654_435_761) as u8).collect())
    }

    fn entry(url: &str, data: &[u8]) -> ModelEntry {
        ModelEntry {
            id: "test-model".into(),
            name: "Test".into(),
            file_name: "model.gguf".into(),
            url: url.into(),
            sha256: sha256_bytes(data),
            bytes: data.len() as u64,
            license: "unknown".into(),
            license_url: None,
            source: "test".into(),
            family: None,
            parameters: None,
            quantization: None,
            chat: false,
            redistribution: "unknown".into(),
            notes: None,
            tags: vec!["fixture".into()],
        }
    }

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(Paths::at(d.path().join("home")));
        (d, s)
    }

    async fn pull_http(store: &Store, e: &ModelEntry) -> (Result<ModelManifest>, Vec<Progress>) {
        let mut events = Vec::new();
        let r = pull_inner(store, e, &mut |p| events.push(p), false).await;
        (r, events)
    }

    #[tokio::test]
    async fn full_download_then_cache_hit() {
        let data = data(3 * MIB as usize + 123);
        let srv = serve(data.clone()).await;
        let (_d, store) = store();
        let e = entry(&srv.url, &data);

        // (a) full download, hash ok
        let (r, events) = pull_http(&store, &e).await;
        let m = r.unwrap();
        assert_eq!(m.sha256, e.sha256);
        assert_eq!(m.source_url, e.url);
        assert_eq!(m.tags, e.tags);
        let last = events.last().unwrap();
        assert!(last.done);
        assert_eq!(
            (
                last.downloaded_bytes,
                last.total_bytes,
                last.network_bytes,
                last.resumed_from
            ),
            (e.bytes, e.bytes, e.bytes, 0)
        );
        assert_eq!(srv.served.load(Ordering::SeqCst), e.bytes);
        assert!(events.iter().all(|p| p.downloaded_bytes <= p.total_bytes));
        assert_eq!(std::fs::read(store.blob_path(&e.sha256)).unwrap(), *data);
        assert!(!store.paths.partial().join(format!("{}.part", e.sha256)).exists());
        assert_eq!(store.get("test-model").unwrap().unwrap(), m);
        store.verify("test-model").unwrap();

        // (d) second pull: zero network bytes, same manifest
        let (r, events) = pull_http(&store, &e).await;
        assert_eq!(r.unwrap(), m);
        assert_eq!(
            events,
            vec![Progress {
                downloaded_bytes: e.bytes,
                total_bytes: e.bytes,
                network_bytes: 0,
                resumed_from: 0,
                done: true
            }]
        );
        assert_eq!(srv.served.load(Ordering::SeqCst), e.bytes);

        // the https-enforcing entry point also serves from cache without touching the network
        let mut n = 0;
        pull(&store, &e, &mut |_| n += 1).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(srv.served.load(Ordering::SeqCst), e.bytes);
    }

    #[tokio::test]
    async fn corrupted_byte_is_rejected_and_partial_deleted() {
        let data = data(MIB as usize);
        let srv = serve(data.clone()).await;
        let (_d, store) = store();
        let mut flipped = (*data).clone();
        flipped[MIB as usize / 2] ^= 1;
        let mut e = entry(&srv.url, &data);
        e.sha256 = sha256_bytes(&flipped); // server serves the unflipped bytes
        let (r, _) = pull_http(&store, &e).await;
        assert_eq!(r.unwrap_err().code, ErrorCode::IntegrityMismatch);
        assert!(!store.paths.partial().join(format!("{}.part", e.sha256)).exists());
        assert!(!store.blob_path(&e.sha256).exists());
        assert!(store.get("test-model").unwrap().is_none());
    }

    #[tokio::test]
    async fn cut_at_half_then_resume() {
        let data = data(4 * MIB as usize);
        let srv = serve(data.clone()).await;
        let (_d, store) = store();
        let e = entry(&srv.url, &data);
        let half = e.bytes / 2;
        srv.cut.store(half, Ordering::SeqCst);

        let (r, _) = pull_http(&store, &e).await;
        assert_eq!(r.unwrap_err().code, ErrorCode::DownloadFailed);
        let partial = store.paths.partial().join(format!("{}.part", e.sha256));
        assert_eq!(
            std::fs::metadata(&partial).unwrap().len(),
            half,
            "partial kept for resume"
        );

        srv.cut.store(u64::MAX, Ordering::SeqCst);
        srv.served.store(0, Ordering::SeqCst);
        let (r, events) = pull_http(&store, &e).await;
        r.unwrap();
        let last = events.last().unwrap();
        assert!(last.done);
        assert_eq!(last.resumed_from, half);
        assert!(
            last.network_bytes <= (e.bytes - half) + MIB,
            "network {}",
            last.network_bytes
        );
        assert!(srv.served.load(Ordering::SeqCst) <= (e.bytes - half) + MIB);
        assert!(!partial.exists());
        store.verify("test-model").unwrap();
        assert_eq!(std::fs::read(store.blob_path(&e.sha256)).unwrap(), *data);
    }

    #[tokio::test]
    async fn server_ignoring_range_restarts_from_zero() {
        let data = data(2 * MIB as usize);
        let srv = serve(data.clone()).await;
        let (_d, store) = store();
        let e = entry(&srv.url, &data);
        srv.cut.store(MIB, Ordering::SeqCst);
        assert_eq!(
            pull_http(&store, &e).await.0.unwrap_err().code,
            ErrorCode::DownloadFailed
        );
        srv.cut.store(u64::MAX, Ordering::SeqCst);
        srv.ignore_range.store(true, Ordering::SeqCst);
        let (r, events) = pull_http(&store, &e).await;
        r.unwrap();
        let last = events.last().unwrap();
        assert_eq!((last.resumed_from, last.network_bytes), (0, e.bytes));
        store.verify("test-model").unwrap();
    }

    #[tokio::test]
    async fn oversize_body_is_rejected() {
        let data = data(MIB as usize);
        let srv = serve(data.clone()).await;
        let (_d, store) = store();
        let mut e = entry(&srv.url, &data);
        e.bytes -= 10;
        let (r, _) = pull_http(&store, &e).await;
        assert_eq!(r.unwrap_err().code, ErrorCode::DownloadFailed);
        assert!(!store.paths.partial().join(format!("{}.part", e.sha256)).exists());
    }

    #[tokio::test]
    async fn http_url_refused_before_any_network() {
        let (_d, store) = store();
        let data = data(16);
        let e = entry("http://127.0.0.1:1/model.gguf", &data);
        let mut n = 0;
        let err = pull(&store, &e, &mut |_| n += 1).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::PolicyDenied);
        assert_eq!(n, 0);
        assert!(!store.paths.partial().exists());
    }

    #[tokio::test]
    async fn stale_blob_is_deleted_on_cache_check() {
        let (_d, store) = store();
        let data = data(64);
        let e = entry("https://example.invalid/model.gguf", &data);
        std::fs::create_dir_all(store.paths.model_blobs()).unwrap();
        std::fs::write(store.blob_path(&e.sha256), b"not the model").unwrap();
        let err = pull(&store, &e, &mut |_| {}).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::IntegrityMismatch);
        assert!(!store.blob_path(&e.sha256).exists());
    }

    #[tokio::test]
    async fn bad_ids_and_hashes_rejected() {
        let (_d, store) = store();
        let data = data(8);
        let mut e = entry("https://example.invalid/m", &data);
        e.id = "../x".into();
        assert_eq!(
            pull(&store, &e, &mut |_| {}).await.unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        let mut e = entry("https://example.invalid/m", &data);
        e.sha256 = "../../etc".into();
        assert_eq!(
            pull(&store, &e, &mut |_| {}).await.unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }
}
