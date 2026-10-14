//! Model understanding without running anything: GGUF metadata (F-01-02), memory feasibility
//! (F-01-03), context/KV planning (F-01-08), quantization recommendation (F-01-07).
//! STUB — implementation pending.

pub mod gguf;
pub mod feasibility;
pub mod planning;
pub mod quant;

pub use gguf::{GgufMetadata, GgufValue, QuantType};
pub use feasibility::{MemoryEstimate, FitVerdict};
pub use planning::{ContextPlan, KvType};
