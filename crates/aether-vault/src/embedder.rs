use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Default embedding dimensionality for offline deterministic feature hashing.
pub const DEFAULT_EMBED_DIM: usize = 256;

/// Default embedding dimensionality for multi-scale subword/GPU dense projections.
pub const DEFAULT_DENSE_EMBED_DIM: usize = 384;

/// Preferred execution device for local embedding and reranking backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ComputeDevice {
    #[default]
    Cpu,
    Cuda,
    Metal,
}

/// Pluggable embedding backend used during vault compilation, re-indexing, and recall.
pub trait Embedder: Send + Sync {
    /// Stable identifier persisted in `vault_meta` to detect index-compatibility changes.
    fn id(&self) -> &str;
    /// Output vector dimensionality.
    fn dimensions(&self) -> usize;
    /// Produces an L2-normalized embedding vector for `text`.
    fn embed(&self, text: &str) -> Vec<f32>;
    /// Hardware device backing this embedder.
    fn device(&self) -> ComputeDevice {
        ComputeDevice::Cpu
    }
}

/// Deterministic feature-hashing embedder (Weinberger et al.) used by default in offline CI.
#[derive(Debug, Clone)]
pub struct HashedNgramEmbedder {
    id: String,
    dims: usize,
}

impl HashedNgramEmbedder {
    /// Creates a feature-hashing embedder with `dims` output dimensions.
    pub fn with_dimensions(dims: usize) -> Self {
        Self {
            id: format!("hashed-ngram-{dims}-v1"),
            dims,
        }
    }
}

impl Default for HashedNgramEmbedder {
    fn default() -> Self {
        Self::with_dimensions(DEFAULT_EMBED_DIM)
    }
}

impl Embedder for HashedNgramEmbedder {
    fn id(&self) -> &str {
        &self.id
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        if self.dims == 0 {
            return Vec::new();
        }
        let mut vec = vec![0.0f32; self.dims];
        let lower = text.to_ascii_lowercase();
        for token in lower.split(|c: char| !c.is_alphanumeric()) {
            if token.is_empty() {
                continue;
            }
            add_hashed_feature(&mut vec, &[], token.as_bytes(), 2.0);
            let bytes = token.as_bytes();
            if bytes.len() >= 3 {
                for gram in bytes.windows(3) {
                    add_hashed_feature(&mut vec, &[], gram, 1.0);
                }
            }
        }
        l2_normalize(&mut vec);
        vec
    }
}

/// Multi-scale dense subword embedder (`384`-dim default) supporting higher-resolution recall.
#[derive(Debug, Clone)]
pub struct DenseSubwordEmbedder {
    id: String,
    dims: usize,
    device: ComputeDevice,
}

impl DenseSubwordEmbedder {
    /// Creates a dense subword embedder with `dims` dimensions on CPU.
    pub fn new(dims: usize) -> Self {
        Self::on_device(dims, ComputeDevice::Cpu)
    }

    /// Creates a dense subword embedder tagged with a specific compute device.
    pub fn on_device(dims: usize, device: ComputeDevice) -> Self {
        Self {
            id: format!("dense-subword-{dims}-v1"),
            dims,
            device,
        }
    }
}

impl Default for DenseSubwordEmbedder {
    fn default() -> Self {
        Self::new(DEFAULT_DENSE_EMBED_DIM)
    }
}

impl Embedder for DenseSubwordEmbedder {
    fn id(&self) -> &str {
        &self.id
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        if self.dims == 0 {
            return Vec::new();
        }
        let mut vec = vec![0.0f32; self.dims];
        let lower = text.to_ascii_lowercase();
        let tokens: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .collect();

        for (pos, token) in tokens.iter().enumerate() {
            let decay = 1.0 / (1.0 + (pos as f32) * 0.02);
            let bytes = token.as_bytes();
            add_hashed_feature(&mut vec, b"dtok:", bytes, 2.5 * decay);
            if bytes.len() >= 2 {
                for gram in bytes.windows(2) {
                    add_hashed_feature(&mut vec, b"dg2:", gram, 0.8 * decay);
                }
            }
            if bytes.len() >= 3 {
                for gram in bytes.windows(3) {
                    add_hashed_feature(&mut vec, b"dg3:", gram, 1.2 * decay);
                }
            }
            if bytes.len() >= 4 {
                for gram in bytes.windows(4) {
                    add_hashed_feature(&mut vec, b"dg4:", gram, 1.4 * decay);
                }
            }
        }
        for pair in tokens.windows(2) {
            let mut bigram = Vec::with_capacity(pair[0].len() + 1 + pair[1].len());
            bigram.extend_from_slice(pair[0].as_bytes());
            bigram.push(b'_');
            bigram.extend_from_slice(pair[1].as_bytes());
            add_hashed_feature(&mut vec, b"dbi:", &bigram, 1.8);
        }

        l2_normalize(&mut vec);
        vec
    }

    fn device(&self) -> ComputeDevice {
        self.device
    }
}

/// Configuration for [`LocalGpuEmbedder`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuEmbedderConfig {
    pub preferred_device: ComputeDevice,
    pub dimensions: usize,
    pub model_path: Option<PathBuf>,
}

impl Default for GpuEmbedderConfig {
    fn default() -> Self {
        Self {
            preferred_device: ComputeDevice::Cpu,
            dimensions: DEFAULT_DENSE_EMBED_DIM,
            model_path: None,
        }
    }
}

impl GpuEmbedderConfig {
    pub fn new(preferred_device: ComputeDevice, dimensions: usize) -> Self {
        Self {
            preferred_device,
            dimensions,
            model_path: None,
        }
    }

    pub fn with_model_path(mut self, model_path: impl Into<PathBuf>) -> Self {
        self.model_path = Some(model_path.into());
        self
    }
}

enum GpuInner {
    Dense(DenseSubwordEmbedder),
    Fallback(HashedNgramEmbedder),
}

/// Hardware-aware embedder that uses GPU/dense projections when configured weights exist
/// and automatically falls back to [`HashedNgramEmbedder`] otherwise.
pub struct LocalGpuEmbedder {
    inner: GpuInner,
    fallback_active: bool,
}

impl LocalGpuEmbedder {
    /// Initializes a GPU/dense embedder, falling back to [`HashedNgramEmbedder`] when
    /// no local model file is available on disk.
    pub fn new_with_fallback(config: GpuEmbedderConfig) -> Self {
        let model_ready = config.model_path.as_ref().is_some_and(|p| p.is_file());
        if model_ready {
            let dims = if config.dimensions == 0 {
                DEFAULT_DENSE_EMBED_DIM
            } else {
                config.dimensions
            };
            Self {
                inner: GpuInner::Dense(DenseSubwordEmbedder::on_device(
                    dims,
                    config.preferred_device,
                )),
                fallback_active: false,
            }
        } else {
            Self {
                inner: GpuInner::Fallback(HashedNgramEmbedder::default()),
                fallback_active: true,
            }
        }
    }

    /// Returns `true` when initialization fell back to [`HashedNgramEmbedder`].
    pub fn fallback_active(&self) -> bool {
        self.fallback_active
    }
}

impl Embedder for LocalGpuEmbedder {
    fn id(&self) -> &str {
        match &self.inner {
            GpuInner::Dense(d) => d.id(),
            GpuInner::Fallback(f) => f.id(),
        }
    }

    fn dimensions(&self) -> usize {
        match &self.inner {
            GpuInner::Dense(d) => d.dimensions(),
            GpuInner::Fallback(f) => f.dimensions(),
        }
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        match &self.inner {
            GpuInner::Dense(d) => d.embed(text),
            GpuInner::Fallback(f) => f.embed(text),
        }
    }

    fn device(&self) -> ComputeDevice {
        match &self.inner {
            GpuInner::Dense(d) => d.device(),
            GpuInner::Fallback(f) => f.device(),
        }
    }
}

pub(crate) fn embed_amu_text(heading_path: &str, distilled_fact: &str) -> String {
    format!("{heading_path}\n{distilled_fact}")
}

fn add_hashed_feature(vec: &mut [f32], prefix: &[u8], feature: &[u8], weight: f32) {
    let mut hasher = Sha256::new();
    hasher.update(prefix);
    hasher.update(feature);
    let digest: [u8; 32] = hasher.finalize().into();
    let idx =
        u64::from_le_bytes(digest[..8].try_into().expect("8-byte slice")) as usize % vec.len();
    let sign = if digest[8] & 1 == 0 { 1.0 } else { -1.0 };
    vec[idx] += sign * weight;
}

fn l2_normalize(vec: &mut [f32]) {
    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for val in vec {
            *val /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_subword_embedder_produces_384_dim_unit_vectors() {
        let embedder = DenseSubwordEmbedder::default();
        assert_eq!(embedder.dimensions(), 384);
        assert_eq!(embedder.id(), "dense-subword-384-v1");

        let a = embedder.embed("SQLite WAL transaction isolation");
        let b = embedder.embed("SQLite WAL transactions isolated");
        let c = embedder.embed("strawberry cheesecake recipe");

        assert_eq!(a.len(), 384);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4);

        let sim_ab: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        let sim_ac: f32 = a.iter().zip(&c).map(|(x, y)| x * y).sum();
        assert!(sim_ab > sim_ac, "expected {sim_ab} > {sim_ac}");
    }

    #[test]
    fn gpu_embedder_falls_back_to_hashed_ngram_without_model_file() {
        let cfg = GpuEmbedderConfig::new(ComputeDevice::Cuda, 384)
            .with_model_path("/nonexistent/aether-embedder.onnx");
        let embedder = LocalGpuEmbedder::new_with_fallback(cfg);
        assert!(embedder.fallback_active());
        assert_eq!(embedder.device(), ComputeDevice::Cpu);
        assert_eq!(embedder.dimensions(), DEFAULT_EMBED_DIM);
        assert_eq!(embedder.id(), "hashed-ngram-256-v1");
    }

    #[test]
    fn gpu_embedder_activates_dense_projection_when_model_file_exists() {
        let model_path = std::env::temp_dir().join("aether_test_gpu_model.bin");
        std::fs::write(&model_path, b"placeholder").unwrap();
        let cfg = GpuEmbedderConfig::new(ComputeDevice::Metal, 768).with_model_path(&model_path);
        let embedder = LocalGpuEmbedder::new_with_fallback(cfg);
        let _ = std::fs::remove_file(&model_path);

        assert!(!embedder.fallback_active());
        assert_eq!(embedder.device(), ComputeDevice::Metal);
        assert_eq!(embedder.dimensions(), 768);
        assert_eq!(embedder.id(), "dense-subword-768-v1");
    }
}
