//! `ucompute models …`: catalog ∪ store listing with license and fit (F-05-04), pull that shows
//! the license before downloading, inspect, import, verify, rm.

use crate::progress;
use clap::Subcommand;
use serde::Serialize;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use ucompute_artifacts::{Catalog, Store};
use ucompute_core::{ErrorCode, Paths, Result, UComputeError};
use ucompute_models::{human_bytes, FitVerdict, KvType, MemoryEstimate, QuantType};

#[derive(Subcommand, Debug)]
pub enum ModelsCmd {
    /// List catalog and local models with license, size, download and fit status
    List,
    /// Search the catalog by id, name, family or tag
    Search { query: String },
    /// Download a catalog model (shows license and size first)
    Pull {
        id: String,
        /// Skip the confirmation prompt
        #[arg(long, short)]
        yes: bool,
    },
    /// Show GGUF metadata and memory estimates for a model id or a .gguf path
    Inspect { target: String },
    /// Import a local GGUF file into the store (hard link, or copy across volumes)
    Import {
        path: PathBuf,
        /// Model id (default: the file stem)
        #[arg(long)]
        id: Option<String>,
        /// License string recorded in the manifest (default: unknown)
        #[arg(long)]
        license: Option<String>,
    },
    /// Re-hash a downloaded model against its manifest
    Verify { id: String },
    /// Remove a model's manifest (and its blob when no other manifest uses it)
    Rm { id: String },
}

pub async fn dispatch(cmd: &ModelsCmd, json: bool) -> Result<()> {
    let store = Store::new(Paths::from_env());
    match cmd {
        ModelsCmd::List => list(&store, json),
        ModelsCmd::Search { query } => search(query, json),
        ModelsCmd::Pull { id, yes } => pull(&store, id, *yes, json).await,
        ModelsCmd::Inspect { target } => inspect(&store, target, json),
        ModelsCmd::Import { path, id, license } => import(&store, path, id.as_deref(), license.as_deref(), json),
        ModelsCmd::Verify { id } => {
            store.verify(id)?;
            out(
                json,
                &serde_json::json!({ "id": id, "verified": true }),
                format!("ok: {id} matches its manifest sha256"),
            )
        }
        ModelsCmd::Rm { id } => {
            store.remove(id)?;
            out(
                json,
                &serde_json::json!({ "id": id, "removed": true }),
                format!("removed {id}"),
            )
        }
    }
}

fn out<T: Serialize>(json: bool, value: &T, text: String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{text}");
    }
    Ok(())
}

#[derive(Serialize)]
struct Row {
    id: String,
    name: String,
    bytes: u64,
    license: String,
    downloaded: bool,
    /// None when the fit cannot be computed (model not downloaded, so no GGUF header to read).
    fits: Option<bool>,
}

fn list(store: &Store, json: bool) -> Result<()> {
    let catalog = Catalog::builtin()?;
    let available = ucompute_hardware::probe()?.memory.available_bytes;
    let local = store.list()?;
    let mut ids: Vec<&str> = catalog
        .models
        .iter()
        .map(|m| m.id.as_str())
        .chain(local.iter().map(|m| m.id.as_str()))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let rows: Vec<Row> = ids
        .into_iter()
        .map(|id| {
            let entry = catalog.get(id);
            let manifest = local.iter().find(|m| m.id == id);
            let blob = manifest.map(|m| store.blob_path(&m.sha256)).filter(|p| p.is_file());
            // ponytail: fit needs the GGUF header, so it is only known for downloaded models ("?" otherwise);
            // upgrade: carry block_count/embedding_length in the catalog and estimate before pulling.
            let fit = blob.as_deref().and_then(|p| fits(p, available).ok());
            let license = manifest
                .map(|m| m.license.as_str())
                .or(entry.map(|e| e.license.as_str()))
                .filter(|l| !l.is_empty())
                .unwrap_or("unknown");
            Row {
                id: id.to_string(),
                name: manifest
                    .map(|m| m.name.clone())
                    .or_else(|| entry.map(|e| e.name.clone()))
                    .unwrap_or_default(),
                bytes: manifest.map(|m| m.bytes).or(entry.map(|e| e.bytes)).unwrap_or(0),
                license: license.to_string(),
                downloaded: blob.is_some(),
                fits: fit,
            }
        })
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<32} {:>9}  {:<14} {:<10} {:<5} NAME",
        "ID", "SIZE", "LICENSE", "DOWNLOADED", "FITS"
    );
    for r in &rows {
        println!(
            "{:<32} {:>9}  {:<14} {:<10} {:<5} {}",
            r.id,
            human_bytes(r.bytes),
            r.license,
            yn(r.downloaded),
            r.fits.map(yn).unwrap_or("?"),
            r.name
        );
    }
    Ok(())
}

fn yn(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// Does `blob` fit in `available` bytes at min(trained, 4096) context with F16 KV and 10% headroom?
fn fits(blob: &Path, available: u64) -> Result<bool> {
    let meta = ucompute_models::read_metadata(blob)?;
    let est = ucompute_models::estimate(
        &meta,
        std::fs::metadata(blob)?.len(),
        meta.context_length.clamp(1, 4096),
        KvType::F16,
    );
    Ok(matches!(
        ucompute_models::check_fit(&est, available, 0.10),
        FitVerdict::Fits { .. }
    ))
}

fn search(query: &str, json: bool) -> Result<()> {
    let catalog = Catalog::builtin()?;
    let hits = catalog.search(query);
    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }
    for e in hits {
        println!("{:<32} {:>9}  {:<14} {}", e.id, human_bytes(e.bytes), e.license, e.name);
    }
    Ok(())
}

async fn pull(store: &Store, id: &str, yes: bool, json: bool) -> Result<()> {
    store.paths.ensure_dirs()?;
    let catalog = Catalog::builtin()?;
    let entry = catalog.get(id).ok_or_else(|| {
        UComputeError::new(ErrorCode::ModelNotFound, format!("{id} is not in the catalog"))
            .with_hint("ucompute models search <query>")
    })?;
    // F-05-04: the license is shown before any bytes move.
    eprintln!("model    {} ({})", entry.id, entry.name);
    eprintln!(
        "license  {}{}",
        entry.license,
        entry
            .license_url
            .as_deref()
            .map(|u| format!(" ({u})"))
            .unwrap_or_default()
    );
    eprintln!("size     {}", human_bytes(entry.bytes));
    if !yes && std::io::stdin().is_terminal() && !confirm("Download? [y/N] ")? {
        eprintln!("aborted");
        return Ok(());
    }
    let m = ucompute_artifacts::pull(store, entry, &mut progress(id)).await?;
    out(
        json,
        &m,
        format!(
            "downloaded {} ({}) sha256 {} license {}",
            m.id,
            human_bytes(m.bytes),
            m.sha256,
            m.license
        ),
    )
}

fn confirm(question: &str) -> Result<bool> {
    eprint!("{question}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

fn inspect(store: &Store, target: &str, json: bool) -> Result<()> {
    let (path, manifest_license) = if Path::new(target).is_file() {
        (PathBuf::from(target), None)
    } else {
        let (m, blob) = store.resolve(target)?;
        (blob, Some(m.license))
    };
    let meta = ucompute_models::read_metadata(&path)?;
    let size = std::fs::metadata(&path)?.len();
    let available = ucompute_hardware::probe()?.memory.available_bytes;
    let at = |n_ctx: u32| {
        let est = ucompute_models::estimate(&meta, size, n_ctx, KvType::F16);
        let fit = ucompute_models::check_fit(&est, available, 0.10);
        (est, fit)
    };
    let trained = at(meta.context_length.max(1));
    let k4 = at(4096);
    let license = manifest_license
        .or_else(|| meta.license.clone())
        .unwrap_or_else(|| "unknown".into());
    if json {
        let mut v = serde_json::to_value(&meta)?;
        v["file"] = path.display().to_string().into();
        v["file_size"] = size.into();
        v["license"] = license.into();
        v["estimates"] = serde_json::json!({
            "trained_ctx": { "estimate": trained.0, "fit": trained.1 },
            "ctx_4096": { "estimate": k4.0, "fit": k4.1 },
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!("file           {}", path.display());
    println!("size           {}", human_bytes(size));
    println!("architecture   {}", meta.architecture);
    println!("name           {}", meta.name.as_deref().unwrap_or("-"));
    println!("license        {license}");
    println!(
        "quantization   {}",
        meta.file_type.map(quant_name).unwrap_or_else(|| "-".into())
    );
    println!("blocks         {}", meta.block_count);
    println!("context        {} (trained)", meta.context_length);
    println!("embedding      {}", meta.embedding_length);
    println!("heads          {} ({} kv)", meta.head_count, meta.head_count_kv);
    println!(
        "vocab          {}",
        meta.vocab_size.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
    );
    println!("tokenizer      {}", meta.tokenizer_model.as_deref().unwrap_or("-"));
    println!("chat template  {}", yn(meta.has_chat_template));
    println!("tensors        {}", meta.tensor_count);
    for (label, (est, fit)) in [("trained", trained), ("4096", k4)] {
        println!(
            "memory @{label:<8} {} ({})",
            human_bytes(est.total_bytes),
            fit_text(&est, &fit)
        );
    }
    Ok(())
}

fn fit_text(est: &MemoryEstimate, fit: &FitVerdict) -> String {
    match fit {
        FitVerdict::Fits { headroom_bytes } => {
            format!("fits at ctx {}, {} headroom", est.n_ctx, human_bytes(*headroom_bytes))
        }
        FitVerdict::TooLarge { reason, .. } => format!("too large at ctx {}: {reason}", est.n_ctx),
    }
}

/// llama.cpp-style spelling via the serde name (`q4_k_m`, `q8_0`); `Other(n)` prints as JSON.
fn quant_name(q: QuantType) -> String {
    serde_json::to_value(q)
        .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
        .unwrap_or_default()
}

fn import(store: &Store, path: &Path, id: Option<&str>, license: Option<&str>, json: bool) -> Result<()> {
    store.paths.ensure_dirs()?;
    // Trust boundary: a file that is not a valid GGUF header is refused before it is hashed or linked.
    let meta = ucompute_models::read_metadata(path)?;
    // F-05-04: an explicit --license wins, else the GGUF's general.license, else "unknown" (store default).
    let m = store.import(path, id, license.or(meta.license.as_deref()))?;
    out(
        json,
        &m,
        format!(
            "imported {} ({}) sha256 {} license {}",
            m.id,
            human_bytes(m.bytes),
            m.sha256,
            m.license
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quant_names_use_llama_spelling() {
        assert_eq!(quant_name(QuantType::from_file_type(2)), "q4_0");
        assert_eq!(quant_name(QuantType::from_file_type(7)), "q8_0");
        assert!(quant_name(QuantType::from_file_type(9999)).contains("9999"));
    }

    #[test]
    fn fit_text_names_context_and_headroom() {
        let est = MemoryEstimate {
            total_bytes: 100,
            n_ctx: 128,
            ..Default::default()
        };
        assert_eq!(
            fit_text(
                &est,
                &FitVerdict::Fits {
                    headroom_bytes: 2_000_000
                }
            ),
            "fits at ctx 128, 2.0 MB headroom"
        );
        let too = FitVerdict::TooLarge {
            needed_bytes: 1,
            available_bytes: 0,
            reason: "needs more".into(),
        };
        assert_eq!(fit_text(&est, &too), "too large at ctx 128: needs more");
    }

    /// SYNTHETIC GGUF v3 (not a real model): header, string/u32 KVs, no tensors.
    fn synthetic_gguf(license: &str) -> Vec<u8> {
        let gstr = |s: &str| [(s.len() as u64).to_le_bytes().to_vec(), s.as_bytes().to_vec()].concat();
        let kv_str = |k: &str, v: &str| [gstr(k), 8u32.to_le_bytes().to_vec(), gstr(v)].concat();
        let kv_u32 = |k: &str, v: u32| [gstr(k), 4u32.to_le_bytes().to_vec(), v.to_le_bytes().to_vec()].concat();
        let mut v = [
            b"GGUF".to_vec(),
            3u32.to_le_bytes().to_vec(),
            0u64.to_le_bytes().to_vec(),
            3u64.to_le_bytes().to_vec(),
        ]
        .concat();
        v.extend(kv_str("general.architecture", "llama"));
        v.extend(kv_str("general.license", license));
        v.extend(kv_u32("llama.block_count", 1));
        v.resize(v.len().next_multiple_of(32), 0);
        v
    }

    #[test]
    fn import_records_the_gguf_license_unless_overridden() {
        let dir = std::env::temp_dir().join(format!("ucompute-cli-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gguf = dir.join("synthetic.gguf");
        std::fs::write(&gguf, synthetic_gguf("apache-2.0")).unwrap();
        let store = Store::new(Paths::at(dir.join("home")));
        import(&store, &gguf, Some("synth-gguf-license"), None, true).unwrap();
        assert_eq!(store.get("synth-gguf-license").unwrap().unwrap().license, "apache-2.0");
        import(&store, &gguf, Some("synth-cli-license"), Some("MIT"), true).unwrap();
        assert_eq!(store.get("synth-cli-license").unwrap().unwrap().license, "MIT");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn list_on_an_empty_store_is_the_catalog() {
        let dir = std::env::temp_dir().join(format!("ucompute-cli-models-{}", std::process::id()));
        let store = Store::new(Paths::at(&dir));
        assert_eq!(store.list().unwrap().len(), 0);
        let catalog = Catalog::builtin().unwrap();
        assert!(catalog.get("stories15M-q4_0").is_some());
        assert!(
            fits(Path::new("/definitely/not/here.gguf"), 1).is_err(),
            "missing blob is an error, shown as ?"
        );
    }
}
