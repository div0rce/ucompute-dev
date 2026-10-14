//! GGUF v2/v3 header + metadata parser (F-01-02). Reads only the KV section and tensor infos,
//! never the tensor data. Every length in the file is untrusted: it is bounds-checked before any
//! allocation, arithmetic is checked, and every violation is a typed `ModelFileInvalid` error.
//! The parser never panics on corrupt input (see the fuzz test at the bottom).

use common_core::{CommonError, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// Largest header (magic through the last tensor info) we are willing to parse.
pub const MAX_HEADER_BYTES: u64 = 64 * 1024 * 1024;
/// Arrays longer than this are summarized as `"<array of N items>"` instead of being kept.
pub const ARRAY_SUMMARY_THRESHOLD: u64 = 256;
/// First read of [`read_metadata`]; doubled until the header parses or `MAX_HEADER_BYTES` is hit.
const INITIAL_PREFIX_BYTES: u64 = 1024 * 1024;
const MAX_KV_COUNT: u64 = 1_000_000;
const MAX_TENSOR_COUNT: u64 = 1_000_000;
const MAX_DIMS: u32 = 8;
const MAX_ARRAY_DEPTH: u32 = 4;
const DEFAULT_ALIGNMENT: u64 = 32;
const MAGIC: &[u8; 4] = b"GGUF";

/// A GGUF metadata value. Serializes as plain JSON (`"llama"`, `6`, `[1, 2]`); deserializing picks
/// the first fitting variant, so a round trip preserves the value but not necessarily the width.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum GgufValue {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    Bool(bool),
    String(String),
    Array(Vec<GgufValue>),
}

impl GgufValue {
    /// Integer view of any unsigned (or non-negative signed) scalar.
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            GgufValue::U8(v) => Some(v.into()),
            GgufValue::U16(v) => Some(v.into()),
            GgufValue::U32(v) => Some(v.into()),
            GgufValue::U64(v) => Some(v),
            GgufValue::I8(v) => u64::try_from(v).ok(),
            GgufValue::I16(v) => u64::try_from(v).ok(),
            GgufValue::I32(v) => u64::try_from(v).ok(),
            GgufValue::I64(v) => u64::try_from(v).ok(),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            GgufValue::String(s) => Some(s),
            _ => None,
        }
    }
}

/// `general.file_type` values (ggml `llama_ftype`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum QuantType {
    F32,
    F16,
    BF16,
    Q8_0,
    Q4_0,
    Q4_1,
    Q5_0,
    Q5_1,
    Q2K,
    Q2KS,
    Q3KS,
    Q3KM,
    Q3KL,
    Q4KS,
    Q4KM,
    Q5KS,
    Q5KM,
    Q6K,
    IQ2XXS,
    IQ2XS,
    IQ2S,
    IQ2M,
    IQ3XXS,
    IQ3XS,
    IQ3S,
    IQ3M,
    IQ1S,
    IQ1M,
    IQ4NL,
    IQ4XS,
    Other(u32),
}

impl QuantType {
    /// Map a raw `general.file_type` to a variant; unknown ids become `Other(n)`.
    pub fn from_file_type(n: u32) -> Self {
        match n {
            0 => QuantType::F32,
            1 => QuantType::F16,
            2 => QuantType::Q4_0,
            3 => QuantType::Q4_1,
            7 => QuantType::Q8_0,
            8 => QuantType::Q5_0,
            9 => QuantType::Q5_1,
            10 => QuantType::Q2K,
            11 => QuantType::Q3KS,
            12 => QuantType::Q3KM,
            13 => QuantType::Q3KL,
            14 => QuantType::Q4KS,
            15 => QuantType::Q4KM,
            16 => QuantType::Q5KS,
            17 => QuantType::Q5KM,
            18 => QuantType::Q6K,
            19 => QuantType::IQ2XXS,
            20 => QuantType::IQ2XS,
            21 => QuantType::Q2KS,
            22 => QuantType::IQ3XS,
            23 => QuantType::IQ3XXS,
            24 => QuantType::IQ1S,
            25 => QuantType::IQ4NL,
            26 => QuantType::IQ3S,
            27 => QuantType::IQ3M,
            28 => QuantType::IQ2S,
            29 => QuantType::IQ2M,
            30 => QuantType::IQ4XS,
            31 => QuantType::IQ1M,
            32 => QuantType::BF16,
            other => QuantType::Other(other),
        }
    }

    /// Catalog spelling, i.e. the serde name: `"q4_k_m"`, `"q8_0"`, `"f16"`; `Other(n)` prints as JSON.
    pub fn as_name(self) -> String {
        serde_json::to_value(self)
            .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
            .unwrap_or_default()
    }

    /// Approximate bits per weight, used only for estimates and ranking.
    pub fn bits_per_weight(self) -> f32 {
        match self {
            QuantType::F32 => 32.0,
            QuantType::F16 | QuantType::BF16 => 16.0,
            QuantType::Q8_0 => 8.5,
            QuantType::Q6K => 6.56,
            QuantType::Q5KM | QuantType::Q5_1 => 5.69,
            QuantType::Q5KS | QuantType::Q5_0 => 5.52,
            QuantType::Q4KM | QuantType::Q4_1 => 4.85,
            QuantType::Q4KS | QuantType::Q4_0 | QuantType::IQ4NL | QuantType::IQ4XS => 4.56,
            QuantType::Q3KL => 4.03,
            QuantType::IQ3M => 3.95,
            QuantType::Q3KM | QuantType::IQ3S => 3.91,
            QuantType::IQ3XS => 3.7,
            QuantType::Q3KS | QuantType::IQ3XXS => 3.5,
            QuantType::Q2K | QuantType::IQ2S => 3.35,
            QuantType::Q2KS => 3.2,
            QuantType::IQ2M => 2.7,
            QuantType::IQ2XS => 2.31,
            QuantType::IQ2XXS => 2.06,
            QuantType::IQ1S | QuantType::IQ1M => 1.56,
            QuantType::Other(_) => 8.0,
        }
    }
}

/// Parse a catalog/user spelling case-insensitively: `"Q4_K_M"`, `"q8_0"`, `"F16"`. Unknown names
/// are an `InvalidRequest` error, never a silent `Other`.
impl std::str::FromStr for QuantType {
    type Err = CommonError;
    fn from_str(s: &str) -> Result<Self> {
        serde_json::from_value(serde_json::Value::String(s.trim().to_ascii_lowercase()))
            .map_err(|_| CommonError::invalid(format!("unknown quantization {s:?}")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TensorInfo {
    pub name: String,
    pub dims: Vec<u64>,
    /// ggml type id.
    pub ggml_type: u32,
    /// Byte offset relative to `GgufMetadata::data_offset`.
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GgufMetadata {
    pub version: u32,
    pub tensor_count: u64,
    pub architecture: String,
    pub name: Option<String>,
    pub block_count: u32,
    pub context_length: u32,
    pub embedding_length: u32,
    /// `{arch}.feed_forward_length`; 0 when absent (the compute-buffer estimate then assumes 4x embedding).
    #[serde(default)]
    pub feed_forward_length: u32,
    pub head_count: u32,
    pub head_count_kv: u32,
    pub vocab_size: Option<u32>,
    pub file_type: Option<QuantType>,
    pub license: Option<String>,
    pub tokenizer_model: Option<String>,
    /// True when `tokenizer.chat_template` is present.
    pub has_chat_template: bool,
    /// Every KV pair except large tokenizer arrays (which are summarized by length).
    pub kv: BTreeMap<String, GgufValue>,
    pub tensors: Vec<TensorInfo>,
    /// Byte offset where tensor data begins.
    pub data_offset: u64,
}

impl GgufMetadata {
    /// Size of one KV-cache token across all layers in bytes for the given element size.
    pub fn kv_bytes_per_token(&self, bytes_per_element: f32) -> f64 {
        let head_dim = self.embedding_length.checked_div(self.head_count).unwrap_or(0);
        2.0 * self.block_count as f64 * self.head_count_kv as f64 * head_dim as f64 * bytes_per_element as f64
    }
}

/// Parse metadata from a file on disk. Reads a bounded prefix (1 MiB, doubling up to
/// [`MAX_HEADER_BYTES`]) and never touches tensor data.
pub fn read_metadata(path: &Path) -> Result<GgufMetadata> {
    let mut file = std::fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut buf = Vec::new();
    let mut target = INITIAL_PREFIX_BYTES;
    loop {
        file.by_ref()
            .take(target.saturating_sub(buf.len() as u64))
            .read_to_end(&mut buf)?;
        let at_end = (buf.len() as u64) < target || buf.len() as u64 >= file_len;
        match parse_inner(&buf) {
            Ok(meta) => {
                if meta.data_offset > file_len {
                    return Err(invalid(format!(
                        "tensor data would start at byte {} but the file is only {file_len} bytes long",
                        meta.data_offset
                    )));
                }
                tracing::debug!(
                    architecture = %meta.architecture,
                    block_count = meta.block_count,
                    tensor_count = meta.tensor_count,
                    data_offset = meta.data_offset,
                    "parsed GGUF metadata"
                );
                return Ok(meta);
            }
            Err(ParseErr::Truncated) if !at_end && target < MAX_HEADER_BYTES => {
                target = target.saturating_mul(2).min(MAX_HEADER_BYTES);
            }
            Err(ParseErr::Truncated) => {
                return Err(invalid(format!(
                    "GGUF header is incomplete: it does not end within the first {} bytes of a {file_len}-byte file",
                    buf.len()
                )))
            }
            Err(ParseErr::Invalid(msg)) => return Err(invalid(msg)),
        }
    }
}

/// Parse metadata from an in-memory prefix of the file (used by fuzz tests). A prefix that ends
/// inside the header is reported as `ModelFileInvalid`, like any other truncation.
pub fn parse_metadata(bytes: &[u8]) -> Result<GgufMetadata> {
    parse_inner(bytes).map_err(|e| match e {
        ParseErr::Truncated => invalid(format!(
            "GGUF header is truncated: it does not end within {} bytes",
            bytes.len()
        )),
        ParseErr::Invalid(msg) => invalid(msg),
    })
}

fn invalid(msg: String) -> CommonError {
    CommonError::new(ErrorCode::ModelFileInvalid, msg)
}

enum ParseErr {
    /// The buffer ended inside the header; a longer prefix of the same file might parse.
    Truncated,
    /// The bytes cannot be a valid GGUF header no matter how many more follow.
    Invalid(String),
}

type PResult<T> = std::result::Result<T, ParseErr>;

/// Smallest encoded size of a value of type `ty`; `Err` for an unknown type id.
fn min_value_size(ty: u32) -> PResult<u64> {
    match ty {
        0 | 1 | 7 => Ok(1),
        2 | 3 => Ok(2),
        4..=6 => Ok(4),
        10..=12 => Ok(8),
        8 => Ok(8),  // string: u64 length
        9 => Ok(12), // array: u32 element type + u64 length
        other => Err(ParseErr::Invalid(format!("unknown GGUF value type {other}"))),
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> u64 {
        self.buf.len().saturating_sub(self.pos) as u64
    }

    /// Take `n` bytes. Running past the buffer is `Truncated` when a longer prefix could still
    /// satisfy the read, `Invalid` when no header may be that large.
    fn take(&mut self, n: u64) -> PResult<&'a [u8]> {
        let end = (self.pos as u64)
            .checked_add(n)
            .filter(|&e| e <= MAX_HEADER_BYTES)
            .ok_or_else(|| {
                ParseErr::Invalid(format!(
                    "length {n} at offset {} exceeds the {MAX_HEADER_BYTES}-byte header limit",
                    self.pos
                ))
            })?;
        let out = self.buf.get(self.pos..end as usize).ok_or(ParseErr::Truncated)?;
        self.pos = end as usize;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> PResult<[u8; N]> {
        self.take(N as u64)?
            .try_into()
            .map_err(|_| ParseErr::Invalid("short read".into()))
    }

    fn u8(&mut self) -> PResult<u8> {
        Ok(u8::from_le_bytes(self.array()?))
    }
    fn i8(&mut self) -> PResult<i8> {
        Ok(i8::from_le_bytes(self.array()?))
    }
    fn u16(&mut self) -> PResult<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn i16(&mut self) -> PResult<i16> {
        Ok(i16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> PResult<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn i32(&mut self) -> PResult<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> PResult<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    fn i64(&mut self) -> PResult<i64> {
        Ok(i64::from_le_bytes(self.array()?))
    }
    fn f32(&mut self) -> PResult<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }
    fn f64(&mut self) -> PResult<f64> {
        Ok(f64::from_le_bytes(self.array()?))
    }

    fn str_bytes(&mut self) -> PResult<&'a [u8]> {
        let len = self.u64()?;
        self.take(len)
    }

    /// Metadata keys are ASCII identifiers; anything else is corruption.
    fn key(&mut self) -> PResult<String> {
        let bytes = self.str_bytes()?;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| ParseErr::Invalid(format!("metadata key at offset {} is not valid UTF-8", self.pos)))
    }

    /// String values (names, templates, tokens) are decoded leniently: a lone bad byte in a
    /// tokenizer string is not a reason to reject a model.
    fn string(&mut self) -> PResult<String> {
        Ok(String::from_utf8_lossy(self.str_bytes()?).into_owned())
    }

    fn read_scalar(&mut self, ty: u32) -> PResult<GgufValue> {
        Ok(match ty {
            0 => GgufValue::U8(self.u8()?),
            1 => GgufValue::I8(self.i8()?),
            2 => GgufValue::U16(self.u16()?),
            3 => GgufValue::I16(self.i16()?),
            4 => GgufValue::U32(self.u32()?),
            5 => GgufValue::I32(self.i32()?),
            6 => GgufValue::F32(self.f32()?),
            7 => GgufValue::Bool(self.u8()? != 0),
            8 => GgufValue::String(self.string()?),
            10 => GgufValue::U64(self.u64()?),
            11 => GgufValue::I64(self.i64()?),
            12 => GgufValue::F64(self.f64()?),
            other => return Err(ParseErr::Invalid(format!("unknown GGUF value type {other}"))),
        })
    }

    fn read_value(&mut self, ty: u32, depth: u32) -> PResult<GgufValue> {
        if ty == 9 {
            self.read_array(depth).map(|(v, _)| v)
        } else {
            self.read_scalar(ty)
        }
    }

    fn skip_value(&mut self, ty: u32, depth: u32) -> PResult<()> {
        match ty {
            8 => self.str_bytes().map(|_| ()),
            9 => self.read_array(depth).map(|_| ()),
            _ => self.take(min_value_size(ty)?).map(|_| ()),
        }
    }

    /// Returns the value (summarized when longer than `ARRAY_SUMMARY_THRESHOLD`) and its length.
    fn read_array(&mut self, depth: u32) -> PResult<(GgufValue, u64)> {
        if depth >= MAX_ARRAY_DEPTH {
            return Err(ParseErr::Invalid(format!(
                "arrays nested deeper than {MAX_ARRAY_DEPTH} levels"
            )));
        }
        let elem_ty = self.u32()?;
        let len = self.u64()?;
        let min_bytes = len
            .checked_mul(min_value_size(elem_ty)?)
            .filter(|&b| b <= MAX_HEADER_BYTES)
            .ok_or_else(|| {
                ParseErr::Invalid(format!(
                    "array of {len} elements cannot fit in a {MAX_HEADER_BYTES}-byte header"
                ))
            })?;
        if min_bytes > self.remaining() {
            return Err(ParseErr::Truncated);
        }
        if len > ARRAY_SUMMARY_THRESHOLD {
            match elem_ty {
                8 | 9 => {
                    for _ in 0..len {
                        self.skip_value(elem_ty, depth + 1)?;
                    }
                }
                _ => {
                    self.take(min_bytes)?;
                }
            }
            return Ok((GgufValue::String(format!("<array of {len} items>")), len));
        }
        let mut items = Vec::with_capacity(len as usize);
        for _ in 0..len {
            items.push(self.read_value(elem_ty, depth + 1)?);
        }
        Ok((GgufValue::Array(items), len))
    }
}

fn parse_inner(bytes: &[u8]) -> PResult<GgufMetadata> {
    let mut r = Reader { buf: bytes, pos: 0 };
    if r.take(4)? != MAGIC {
        return Err(ParseErr::Invalid("not a GGUF file (bad magic)".into()));
    }
    let version = r.u32()?;
    if !(2..=3).contains(&version) {
        return Err(ParseErr::Invalid(format!(
            "unsupported GGUF version {version} (expected 2 or 3)"
        )));
    }
    let tensor_count = r.u64()?;
    if tensor_count > MAX_TENSOR_COUNT {
        return Err(ParseErr::Invalid(format!(
            "tensor count {tensor_count} exceeds the limit of {MAX_TENSOR_COUNT}"
        )));
    }
    let kv_count = r.u64()?;
    if kv_count > MAX_KV_COUNT {
        return Err(ParseErr::Invalid(format!(
            "metadata count {kv_count} exceeds the limit of {MAX_KV_COUNT}"
        )));
    }

    let mut kv = BTreeMap::new();
    let mut tokens_len = None;
    let mut alignment = DEFAULT_ALIGNMENT;
    for _ in 0..kv_count {
        let key = r.key()?;
        let ty = r.u32()?;
        let (value, array_len) = if ty == 9 {
            let (v, n) = r.read_array(0)?;
            (v, Some(n))
        } else {
            (r.read_scalar(ty)?, None)
        };
        if key == "tokenizer.ggml.tokens" {
            tokens_len = array_len;
        }
        if key == "general.alignment" {
            alignment = value
                .as_u64()
                .filter(|a| a.is_power_of_two())
                .ok_or_else(|| ParseErr::Invalid("general.alignment must be a power of two".into()))?;
        }
        if kv.insert(key.clone(), value).is_some() {
            return Err(ParseErr::Invalid(format!("duplicate metadata key {key}")));
        }
    }

    let mut tensors = Vec::new();
    for _ in 0..tensor_count {
        let name = r.string()?;
        let n_dims = r.u32()?;
        if n_dims > MAX_DIMS {
            return Err(ParseErr::Invalid(format!(
                "tensor {name} has {n_dims} dimensions (limit {MAX_DIMS})"
            )));
        }
        let mut dims = Vec::with_capacity(n_dims as usize);
        for _ in 0..n_dims {
            dims.push(r.u64()?);
        }
        let ggml_type = r.u32()?;
        let offset = r.u64()?;
        if offset % alignment != 0 {
            return Err(ParseErr::Invalid(format!(
                "tensor {name} offset {offset} is not aligned to {alignment}"
            )));
        }
        tensors.push(TensorInfo {
            name,
            dims,
            ggml_type,
            offset,
        });
    }

    let header_end = r.pos as u64;
    let pad = (alignment - header_end % alignment) % alignment;
    let data_offset = header_end
        .checked_add(pad)
        .ok_or_else(|| ParseErr::Invalid("data offset overflows".into()))?;

    let str_kv = |k: &str| kv.get(k).and_then(GgufValue::as_str).map(str::to_owned);
    let u32_kv = |k: &str| {
        kv.get(k)
            .and_then(GgufValue::as_u64)
            .and_then(|v| u32::try_from(v).ok())
    };
    let architecture =
        str_kv("general.architecture").ok_or_else(|| ParseErr::Invalid("missing general.architecture".into()))?;
    let arch_kv = |suffix: &str| u32_kv(&format!("{architecture}.{suffix}"));
    let head_count = arch_kv("attention.head_count").unwrap_or(0);
    let meta = GgufMetadata {
        version,
        tensor_count,
        name: str_kv("general.name"),
        block_count: arch_kv("block_count").unwrap_or(0),
        context_length: arch_kv("context_length").unwrap_or(0),
        embedding_length: arch_kv("embedding_length").unwrap_or(0),
        feed_forward_length: arch_kv("feed_forward_length").unwrap_or(0),
        head_count,
        head_count_kv: arch_kv("attention.head_count_kv").unwrap_or(head_count),
        vocab_size: tokens_len
            .and_then(|n| u32::try_from(n).ok())
            .or_else(|| arch_kv("vocab_size")),
        file_type: u32_kv("general.file_type").map(QuantType::from_file_type),
        license: str_kv("general.license"),
        tokenizer_model: str_kv("tokenizer.ggml.model"),
        has_chat_template: kv.contains_key("tokenizer.chat_template"),
        data_offset,
        architecture,
        tensors,
        kv,
    };
    Ok(meta)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---- synthetic GGUF builder (labelled SYNTHETIC: not a real model) ----

    pub(crate) fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }
    pub(crate) fn gstr(s: &str) -> Vec<u8> {
        cat(&[(s.len() as u64).to_le_bytes().to_vec(), s.as_bytes().to_vec()])
    }
    pub(crate) fn kv_u32(key: &str, v: u32) -> Vec<u8> {
        cat(&[gstr(key), 4u32.to_le_bytes().to_vec(), v.to_le_bytes().to_vec()])
    }
    pub(crate) fn kv_str(key: &str, v: &str) -> Vec<u8> {
        cat(&[gstr(key), 8u32.to_le_bytes().to_vec(), gstr(v)])
    }
    pub(crate) fn kv_str_array(key: &str, items: &[&str]) -> Vec<u8> {
        let mut v = cat(&[
            gstr(key),
            9u32.to_le_bytes().to_vec(),
            8u32.to_le_bytes().to_vec(),
            (items.len() as u64).to_le_bytes().to_vec(),
        ]);
        for s in items {
            v.extend(gstr(s));
        }
        v
    }
    pub(crate) fn tensor(name: &str, dims: &[u64], ty: u32, offset: u64) -> Vec<u8> {
        let mut v = cat(&[gstr(name), (dims.len() as u32).to_le_bytes().to_vec()]);
        for d in dims {
            v.extend(d.to_le_bytes());
        }
        v.extend(ty.to_le_bytes());
        v.extend(offset.to_le_bytes());
        v
    }
    pub(crate) fn header(version: u32, tensor_count: u64, kv_count: u64) -> Vec<u8> {
        cat(&[
            b"GGUF".to_vec(),
            version.to_le_bytes().to_vec(),
            tensor_count.to_le_bytes().to_vec(),
            kv_count.to_le_bytes().to_vec(),
        ])
    }
    fn pad_to(mut v: Vec<u8>, align: usize) -> Vec<u8> {
        while !v.len().is_multiple_of(align) {
            v.push(0);
        }
        v
    }

    /// SYNTHETIC llama-arch file with the stories15M shape (6 blocks, 288 embd, 768 ff, 6 heads, ctx 128)
    /// and 300 tokens so the tokens array is summarized. Ends with 64 bytes of fake tensor data.
    pub(crate) fn synthetic_llama() -> Vec<u8> {
        let tokens: Vec<String> = (0..300).map(|i| format!("t{i}")).collect();
        let token_refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
        let kvs = [
            kv_str("general.architecture", "llama"),
            kv_str("general.name", "synthetic"),
            kv_u32("general.file_type", 2),
            kv_u32("llama.block_count", 6),
            kv_u32("llama.context_length", 128),
            kv_u32("llama.embedding_length", 288),
            kv_u32("llama.feed_forward_length", 768),
            kv_u32("llama.attention.head_count", 6),
            kv_str("tokenizer.ggml.model", "llama"),
            kv_str_array("tokenizer.ggml.tokens", &token_refs),
            kv_str("tokenizer.chat_template", "{{ messages }}"),
        ];
        let mut v = header(3, 2, kvs.len() as u64);
        v.extend(cat(&kvs));
        v.extend(tensor("token_embd.weight", &[288, 300], 2, 0));
        v.extend(tensor("output_norm.weight", &[288], 0, 64));
        let mut v = pad_to(v, 32);
        v.extend(std::iter::repeat_n(0u8, 64));
        v
    }

    pub(crate) fn fixture(sha: &str) -> Option<PathBuf> {
        let root = std::env::var_os("COMMON_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.common"));
        let p = root.join("models/blobs").join(format!("sha256-{sha}"));
        if p.exists() {
            Some(p)
        } else {
            eprintln!("SKIP: fixture {} not present", p.display());
            None
        }
    }
    pub(crate) const STORIES15M: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
    const STORIES260K: &str = "270cba1bd5109f42d03350f60406024560464db173c0e387d91f0426d3bd256d";
    const QWEN05_Q4KM: &str = "74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db";

    fn code(r: Result<GgufMetadata>) -> ErrorCode {
        r.expect_err("expected an error").code
    }

    #[test]
    fn synthetic_parses_and_extracts_fields() {
        let bytes = synthetic_llama();
        let m = parse_metadata(&bytes).unwrap();
        assert_eq!(m.version, 3);
        assert_eq!(m.architecture, "llama");
        assert_eq!(m.name.as_deref(), Some("synthetic"));
        assert_eq!(
            (
                m.block_count,
                m.context_length,
                m.embedding_length,
                m.head_count,
                m.head_count_kv
            ),
            (6, 128, 288, 6, 6)
        );
        assert_eq!(m.feed_forward_length, 768);
        assert_eq!(m.vocab_size, Some(300));
        assert_eq!(m.file_type, Some(QuantType::Q4_0));
        assert_eq!(m.tokenizer_model.as_deref(), Some("llama"));
        assert!(m.has_chat_template);
        assert_eq!(
            m.kv["tokenizer.ggml.tokens"],
            GgufValue::String("<array of 300 items>".into())
        );
        assert_eq!(m.tensors.len(), 2);
        assert_eq!(m.tensors[0].dims, vec![288, 300]);
        assert_eq!(m.tensors[1].offset, 64);
        assert_eq!(m.data_offset % 32, 0);
        assert_eq!(m.data_offset as usize, bytes.len() - 64);
        // the untagged enum serializes as plain JSON and round-trips without error
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"general.architecture\":\"llama\""));
        let back: GgufMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(back.architecture, "llama");
    }

    #[test]
    fn read_metadata_grows_prefix_and_checks_file_length() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("synthetic.gguf");
        std::fs::write(&p, synthetic_llama()).unwrap();
        let m = read_metadata(&p).unwrap();
        assert_eq!(m.block_count, 6);
        // a file cut inside the header is ModelFileInvalid
        let bytes = synthetic_llama();
        std::fs::write(&p, &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(code(read_metadata(&p)), ErrorCode::ModelFileInvalid);
        // header complete but file ends before data_offset
        std::fs::write(&p, &bytes[..m.data_offset as usize - 8]).unwrap();
        assert_eq!(code(read_metadata(&p)), ErrorCode::ModelFileInvalid);
        // missing file is Io, not ModelFileInvalid
        assert_eq!(code(read_metadata(&dir.path().join("nope.gguf"))), ErrorCode::Io);
    }

    #[test]
    fn rejects_bad_inputs_with_typed_error() {
        assert_eq!(code(parse_metadata(&[])), ErrorCode::ModelFileInvalid);
        assert_eq!(code(parse_metadata(b"GGU")), ErrorCode::ModelFileInvalid);
        assert_eq!(code(parse_metadata(b"GGML\x03\0\0\0")), ErrorCode::ModelFileInvalid);
        assert_eq!(code(parse_metadata(&header(3, 1, 1))), ErrorCode::ModelFileInvalid); // header only
        assert_eq!(code(parse_metadata(&header(99, 0, 0))), ErrorCode::ModelFileInvalid);
        let huge_kv = header(3, 0, u64::MAX);
        let e = parse_metadata(&huge_kv).unwrap_err();
        assert_eq!(e.code, ErrorCode::ModelFileInvalid);
        assert!(e.message.contains("exceeds the limit"), "{}", e.message);
        assert_eq!(
            code(parse_metadata(&header(3, u64::MAX, 0))),
            ErrorCode::ModelFileInvalid
        );
        // missing architecture
        let mut v = header(3, 0, 1);
        v.extend(kv_u32("general.file_type", 2));
        assert!(parse_metadata(&v).unwrap_err().message.contains("general.architecture"));
        // string length larger than any header
        let mut v = header(3, 0, 1);
        v.extend(u64::MAX.to_le_bytes());
        assert!(parse_metadata(&v).unwrap_err().message.contains("exceeds"));
        // too many dims
        let mut v = header(3, 1, 1);
        v.extend(kv_str("general.architecture", "llama"));
        v.extend(tensor("t", &[1; 9], 0, 0));
        assert!(parse_metadata(&v).unwrap_err().message.contains("dimensions"));
        // misaligned tensor offset
        let mut v = header(3, 1, 1);
        v.extend(kv_str("general.architecture", "llama"));
        v.extend(tensor("t", &[1], 0, 7));
        assert!(parse_metadata(&v).unwrap_err().message.contains("aligned"));
        // bad alignment value
        let mut v = header(3, 0, 2);
        v.extend(kv_str("general.architecture", "llama"));
        v.extend(kv_u32("general.alignment", 24));
        assert!(parse_metadata(&v).unwrap_err().message.contains("power of two"));
        // duplicate key
        let mut v = header(3, 0, 2);
        v.extend(kv_str("general.architecture", "llama"));
        v.extend(kv_str("general.architecture", "llama"));
        assert!(parse_metadata(&v).unwrap_err().message.contains("duplicate"));
    }

    #[test]
    fn nested_arrays_are_bounded() {
        // array<array<array<array<array<u8>>>>> nests 5 deep: rejected at depth 4
        let mut v = header(3, 0, 2);
        v.extend(kv_str("general.architecture", "llama"));
        v.extend(gstr("deep"));
        v.extend(9u32.to_le_bytes());
        for _ in 0..4 {
            v.extend(9u32.to_le_bytes()); // elem type array
            v.extend(1u64.to_le_bytes()); // one element
        }
        v.extend(0u32.to_le_bytes());
        v.extend(1u64.to_le_bytes());
        v.push(7);
        assert!(parse_metadata(&v).unwrap_err().message.contains("nested"));
        // small nested array of u32 is kept
        let mut v = header(3, 0, 2);
        v.extend(kv_str("general.architecture", "llama"));
        v.extend(gstr("small"));
        v.extend(9u32.to_le_bytes());
        v.extend(9u32.to_le_bytes());
        v.extend(1u64.to_le_bytes());
        v.extend(4u32.to_le_bytes());
        v.extend(2u64.to_le_bytes());
        v.extend(5u32.to_le_bytes());
        v.extend(6u32.to_le_bytes());
        let m = parse_metadata(&v).unwrap();
        assert_eq!(
            m.kv["small"],
            GgufValue::Array(vec![GgufValue::Array(vec![GgufValue::U32(5), GgufValue::U32(6)])])
        );
    }

    #[test]
    fn file_type_mapping() {
        assert_eq!(QuantType::from_file_type(0), QuantType::F32);
        assert_eq!(QuantType::from_file_type(15), QuantType::Q4KM);
        assert_eq!(QuantType::from_file_type(32), QuantType::BF16);
        assert_eq!(QuantType::from_file_type(4), QuantType::Other(4));
        assert_eq!(QuantType::from_file_type(1000), QuantType::Other(1000));
        assert!(QuantType::Q8_0.bits_per_weight() > QuantType::Q4KM.bits_per_weight());
        assert_eq!(serde_json::to_string(&QuantType::Q4KM).unwrap(), "\"q4_k_m\"");
        assert_eq!(serde_json::to_string(&QuantType::Q8_0).unwrap(), "\"q8_0\"");
        assert_eq!(QuantType::Q4KM.as_name(), "q4_k_m");
        assert!(QuantType::Other(9999).as_name().contains("9999"));
        // catalog strings round-trip through FromStr, case-insensitively
        for q in [
            QuantType::F32,
            QuantType::Q4_0,
            QuantType::Q4KM,
            QuantType::Q8_0,
            QuantType::IQ2XXS,
        ] {
            assert_eq!(q.as_name().to_uppercase().parse::<QuantType>().unwrap(), q);
        }
        assert_eq!(" q2_k ".parse::<QuantType>().unwrap(), QuantType::Q2K);
        assert_eq!(
            "Q4_K_M_XL".parse::<QuantType>().unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        assert_eq!("".parse::<QuantType>().unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn fixture_stories15m_q4_0() {
        let Some(p) = fixture(STORIES15M) else { return };
        let m = read_metadata(&p).unwrap();
        assert_eq!(m.architecture, "llama");
        assert_eq!(
            (
                m.block_count,
                m.context_length,
                m.embedding_length,
                m.head_count,
                m.head_count_kv
            ),
            (6, 128, 288, 6, 6)
        );
        assert_eq!(m.feed_forward_length, 768);
        assert_eq!(m.file_type, Some(QuantType::Q4_0));
        assert_eq!(m.vocab_size, Some(32000));
        assert_eq!(m.tensor_count, 57);
        assert_eq!(m.tensors.len(), 57);
        assert!(m.data_offset < 19_077_344);
        assert_eq!(
            m.kv["tokenizer.ggml.tokens"],
            GgufValue::String("<array of 32000 items>".into())
        );
        assert_eq!(m.kv_bytes_per_token(2.0) as u64 * 128, 884_736);
    }

    #[test]
    fn fixture_stories260k_f32() {
        let Some(p) = fixture(STORIES260K) else { return };
        let m = read_metadata(&p).unwrap();
        assert_eq!(m.architecture, "llama");
        // this fixture carries no general.file_type; every tensor is ggml F32 (type 0)
        assert_eq!(m.file_type, None);
        assert!(!m.tensors.is_empty() && m.tensors.iter().all(|t| t.ggml_type == 0));
    }

    #[test]
    fn fixture_qwen25_05b_q4_k_m() {
        let Some(p) = fixture(QWEN05_Q4KM) else { return };
        let m = read_metadata(&p).unwrap();
        assert_eq!(m.architecture, "qwen2");
        assert_eq!(
            (
                m.block_count,
                m.context_length,
                m.embedding_length,
                m.head_count,
                m.head_count_kv
            ),
            (24, 32768, 896, 14, 2)
        );
        assert_eq!(m.feed_forward_length, 4864);
        assert_eq!(m.file_type, Some(QuantType::Q4KM));
        assert!(m.has_chat_template);
        assert_eq!(m.vocab_size, Some(151_936));
    }

    /// xorshift64* with a fixed seed: the fuzz run is deterministic.
    pub(crate) struct XorShift64Star(pub u64);
    impl XorShift64Star {
        pub(crate) fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    fn fuzz(base: &[u8], label: &str) -> usize {
        let mut rng = XorShift64Star(0x9E37_79B9_7F4A_7C15);
        let mut ok = 0;
        for i in 0..1000 {
            let mut b = base.to_vec();
            for _ in 0..1 + rng.below(4) {
                if b.is_empty() {
                    break;
                }
                match rng.below(6) {
                    0 => {
                        let i = rng.below(b.len());
                        b[i] ^= 1 << rng.below(8);
                    }
                    1 => {
                        let i = rng.below(b.len().saturating_sub(4).max(1));
                        let v = (rng.next() as u32) | 0x8000_0000;
                        for (k, byte) in v.to_le_bytes().iter().enumerate() {
                            if let Some(slot) = b.get_mut(i + k) {
                                *slot = *byte;
                            }
                        }
                    }
                    2 => {
                        let i = rng.below(b.len().saturating_sub(8).max(1));
                        let v = rng.next() | 0x8000_0000_0000_0000;
                        for (k, byte) in v.to_le_bytes().iter().enumerate() {
                            if let Some(slot) = b.get_mut(i + k) {
                                *slot = *byte;
                            }
                        }
                    }
                    3 => b.truncate(rng.below(b.len() + 1)),
                    4 => b.iter_mut().take(4).for_each(|x| *x = 0),
                    _ => {
                        for (k, byte) in 99u32.to_le_bytes().iter().enumerate() {
                            if let Some(slot) = b.get_mut(4 + k) {
                                *slot = *byte;
                            }
                        }
                    }
                }
            }
            match std::panic::catch_unwind(|| parse_metadata(&b)) {
                Ok(Ok(_)) => ok += 1,
                Ok(Err(e)) => assert_eq!(
                    e.code,
                    ErrorCode::ModelFileInvalid,
                    "{label} iteration {i}: {}",
                    e.message
                ),
                Err(_) => panic!("{label} iteration {i}: parse_metadata panicked"),
            }
        }
        ok
    }

    #[test]
    fn fuzz_1000_mutated_headers_never_panic_synthetic() {
        let ok = fuzz(&synthetic_llama(), "SYNTHETIC");
        assert!(ok > 0, "some mutations should still parse");
    }

    #[test]
    fn fuzz_1000_mutated_headers_never_panic_fixture_prefix() {
        let Some(p) = fixture(STORIES15M) else { return };
        let mut prefix = vec![0u8; 64 * 1024];
        std::fs::File::open(p).unwrap().read_exact(&mut prefix).unwrap();
        fuzz(&prefix, "stories15M-64KiB");
    }
}
