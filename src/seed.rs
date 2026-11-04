//! 预置集：随发布附带的常用命令节点与向量。
//!
//! 第一版只有一份跨平台集合（`seed/index.json` 与 `seed/vectors.bin`），编译时嵌入。
//! 索引不存在时由 [`install_if_missing`] 原样写入，初始化不调用 LLM。

use std::path::Path;

use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::index::vectors::{self, Vectors};
use crate::index::{self, Index, IndexError};

const SEED_INDEX: &str = include_str!("../seed/index.json");
const SEED_VECTORS: &[u8] = include_bytes!("../seed/vectors.bin");

/// 解析后的预置集。节点均为 `pinned = true`。
#[derive(Debug, Clone)]
pub struct SeedSet {
    /// 索引元数据与节点。
    pub index: Index,
    /// 与节点一一对应的向量。
    pub vectors: Vectors,
}

/// 预置集错误。
#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    /// JSON 解析失败。
    #[error("seed json: {0}")]
    Json(#[from] serde_json::Error),
    /// 向量与节点、哈希或标记不一致。
    #[error("seed vector mismatch: {0}")]
    Mismatch(String),
    /// 写入索引失败。
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// 解析编译时嵌入的预置集。
pub fn load_seed() -> Result<SeedSet, SeedError> {
    parse(SEED_INDEX, SEED_VECTORS)
}

/// 解析预置集文本与向量字节，校验行数、哈希与 `pinned` 标记。
fn parse(text: &str, bytes: &[u8]) -> Result<SeedSet, SeedError> {
    let index: Index = serde_json::from_str(text)?;
    let dims = index.meta.embedding.dimensions;
    let vectors = vectors::parse(bytes, dims, index.nodes.len())
        .map_err(|err| SeedError::Mismatch(err.to_string()))?;
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if format!("sha256:{hex}") != index.meta.vectors_sha256 {
        return Err(SeedError::Mismatch("vectors_sha256".to_owned()));
    }
    for (row, node) in index.nodes.iter().enumerate() {
        if !node.pinned || node.vector_row != Some(row) {
            return Err(SeedError::Mismatch(format!("node {}", node.key)));
        }
    }
    Ok(SeedSet { index, vectors })
}

/// 纯函数：预置向量是否与当前 embedding 模型不一致。
pub fn needs_reembed(seed: &SeedSet, model: &str, dimensions: usize) -> bool {
    let meta = &seed.index.meta.embedding;
    meta.model != model || meta.dimensions != dimensions
}

/// `index_dir` 下没有索引时写入预置集，返回是否写入。
///
/// 已有索引时不做任何改动。当前模型与预置向量不一致时跳过并记 warn，
/// 第一版不做重新 embedding。
pub fn install_if_missing(
    index_dir: &Path,
    model: &str,
    dimensions: usize,
) -> Result<bool, SeedError> {
    if index_dir.join("index.json").exists() {
        return Ok(false);
    }
    let seed = load_seed()?;
    if needs_reembed(&seed, model, dimensions) {
        warn!(
            seed_model = %seed.index.meta.embedding.model,
            model,
            "seed skipped: embedding model differs"
        );
        return Ok(false);
    }
    index::save_atomic(index_dir, &seed.index, &seed.vectors)?;
    info!(nodes = seed.index.nodes.len(), "seed installed");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{install_if_missing, load_seed, needs_reembed};
    use crate::index;
    use crate::testutil::tempdir;

    #[test]
    fn embedded_seed_is_consistent() {
        let seed = load_seed().unwrap();
        assert!(!seed.index.nodes.is_empty());
        assert_eq!(seed.vectors.rows(), seed.index.nodes.len());
        assert!(seed.index.nodes.iter().any(|n| n.key == "ls"));
    }

    #[test]
    fn needs_reembed_compares_model_and_dimensions() {
        let seed = load_seed().unwrap();
        let meta = seed.index.meta.embedding.clone();
        assert!(!needs_reembed(&seed, &meta.model, meta.dimensions));
        assert!(needs_reembed(&seed, "other", meta.dimensions));
        assert!(needs_reembed(&seed, &meta.model, meta.dimensions + 1));
    }

    #[test]
    fn install_writes_seed_once_and_loads() {
        let dir = tempdir();
        let index_dir = dir.path().join("index");
        let meta = load_seed().unwrap().index.meta.embedding;
        assert!(install_if_missing(&index_dir, &meta.model, meta.dimensions).unwrap());
        let stored = index::load(&index_dir).unwrap().unwrap();
        assert!(stored.index.nodes.iter().all(|n| n.pinned));
        // 第二次不覆盖。
        assert!(!install_if_missing(&index_dir, &meta.model, meta.dimensions).unwrap());
    }

    #[test]
    fn install_skips_on_model_mismatch() {
        let dir = tempdir();
        let index_dir = dir.path().join("index");
        assert!(!install_if_missing(&index_dir, "other-model", 8).unwrap());
        assert!(!index_dir.join("index.json").exists());
    }
}
