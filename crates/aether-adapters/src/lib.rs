//! Outbound storage & database connectors for Aether SDK.

#[cfg(feature = "contract-suite")]
pub mod contract;
pub mod in_memory;
pub mod kv;
pub mod middleware;
pub mod storage;

pub use in_memory::{FaultPoint, InMemoryBackend};
pub use middleware::{Batcher, Resilient, RetryPolicy, Timeout, Traced};
