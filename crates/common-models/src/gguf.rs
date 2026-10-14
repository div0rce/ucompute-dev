//! GGUF v2/v3 header + metadata parser. Reads only the KV section and tensor infos, never the
//! tensor data. Must return a typed `ModelFileInvalid` error (never panic) on any corrupt input.
//! STUB — implementation pending.

use common_core::{CommonError, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum GgufValue {
    U8(u8), I8(i8), U16(u16), I16(i16), U32(u32), I32(i32), U64(u64), I64(i64),
    F32(f32), F64(f64), Bool(bool), String(String),
    Array(Vec<GgufValue>),
}

/// `general.file_type` values (ggml `llama_ftype`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum QuantType {
    F32, F16, BF16, Q8_0, Q4_0, Q4_1, Q5_0, Q5_1, Q2K, Q3KS, Q3KM, Q3KL, Q4KS, Q4KM, Q5KS, Q5KM, Q6K,
    IQ2XXS, IQ2XS, IQ3XXS, IQ1S, IQ4NL, IQ3S, IQ2S, IQ4XS, IQ1M, Other(u32),
}

impl QuantType {
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
            QuantType::Q3KM | QuantType::IQ3S => 3.91,
            QuantType::Q3KS | QuantType::IQ3XXS => 3.5,
            QuantType::Q2K | QuantType::IQ2S => 3.35,
            QuantType::IQ2XS => 2.31,
            QuantType::IQ2XXS => 2.06,
            QuantType::IQ1S | QuantType::IQ1M => 1.56,
            QuantType::Other(_) => 8.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TensorInfo {
    pub name: String,
    pub dims: Vec<u64>,
    /// ggml type id.
    pub ggml_type: u32,
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
        let head_dim = if self.head_count > 0 { self.embedding_length / self.head_count } else { 0 };
        2.0 * self.block_count as f64 * self.head_count_kv as f64 * head_dim as f64 * bytes_per_element as f64
    }
}

/// Parse metadata from a file on disk.
pub fn read_metadata(path: &Path) -> Result<GgufMetadata> {
    let _ = path;
    Err(CommonError::new(ErrorCode::NotImplemented, "gguf parser not implemented"))
}

/// Parse metadata from an in-memory prefix of the file (used by fuzz tests).
pub fn parse_metadata(bytes: &[u8]) -> Result<GgufMetadata> {
    let _ = bytes;
    Err(CommonError::new(ErrorCode::NotImplemented, "gguf parser not implemented"))
}
