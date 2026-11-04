//! 阶段 B 的纯逻辑：暂存记录转节点、分批 embedding、与旧索引合并。
//!
//! 不访问文件系统，写盘与暂存区清理在 [`super::embed_and_merge`]。
//! 单批 embedding 失败只影响该批：旧节点与旧向量保留，含未合并节点的暂存记录保留。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::embed::{Embedder, embed_text};
use crate::generate::GenerateError;
use crate::generate::staging::StagedRecord;
use crate::index::vectors::Vectors;
use crate::index::{EmbeddingMeta, GeneratorMeta, Index, IndexMeta, SCHEMA_VERSION, Stored};
use crate::types::{DocSource, Node, NodeKind};

/// 节点的文档来源。暂存记录不含这些字段，由阶段 A 的调用方按 key 提供。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSource {
    /// 文档来源。
    pub doc_source: DocSource,
    /// `sha256:...` 形式的源哈希，见 [`super::plan::source_hash`]。
    pub source_hash: String,
}

/// 本次写入索引的元数据。
#[derive(Debug, Clone)]
pub struct Stamp {
    /// 构建时间，RFC 3339。同时用作新节点的 `generated_at`。
    pub built_at: String,
    /// 生成描述所用 agent 的信息。
    pub generator: GeneratorMeta,
}

/// 合并结果。
#[derive(Debug)]
pub struct Merged {
    /// 合并后的索引，行数与节点数一致。
    pub index: Index,
    /// 与 `index.nodes` 一一对应的向量。
    pub vectors: Vectors,
    /// 本次新写入或替换的节点数。
    pub merged: usize,
    /// 未能合并的 `(key, reason)`。
    pub failed: Vec<(String, String)>,
    /// 需要保留在暂存区的记录（原样）。
    pub keep: Vec<StagedRecord>,
}

/// 待合并的候选节点，记录它来自哪条暂存记录。
struct Candidate {
    node: Node,
    record: usize,
}

/// 把暂存记录合并进旧索引。
///
/// - `removed` 中的 key 及其旧向量被丢弃。
/// - 同一 key 以最后一条记录为准。
/// - 旧索引的模型名或维度与 `embedder` 不符时拒绝合并，避免新旧向量混用。
/// - 预置节点（`pinned`）被重新生成时保留 `pinned` 标记。
pub fn merge_staged(
    old: Option<&Stored>,
    removed: &HashSet<String>,
    records: &[StagedRecord],
    sources: &HashMap<String, NodeSource>,
    embedder: &dyn Embedder,
    batch_size: usize,
    stamp: &Stamp,
) -> Result<Merged, GenerateError> {
    let dims = embedder.dimensions();
    if let Some(stored) = old {
        let meta = &stored.index.meta.embedding;
        if meta.model != embedder.model() {
            return Err(GenerateError::ModelChanged {
                old: meta.model.clone(),
                new: embedder.model().to_owned(),
            });
        }
        if meta.dimensions != dims {
            return Err(GenerateError::Dimension {
                expected: meta.dimensions,
                actual: dims,
            });
        }
    }
    let pinned: HashSet<&str> = old.map_or_else(HashSet::new, |stored| {
        stored
            .index
            .nodes
            .iter()
            .filter(|n| n.pinned)
            .map(|n| n.key.as_str())
            .collect()
    });

    let mut failed = Vec::new();
    let mut keep = BTreeSet::new();
    let mut cands: BTreeMap<String, Candidate> = BTreeMap::new();
    for (idx, rec) in records.iter().enumerate() {
        match nodes_of(rec, sources, stamp, &pinned) {
            Ok(nodes) => {
                for node in nodes {
                    cands.insert(node.key.clone(), Candidate { node, record: idx });
                }
            }
            Err(pair) => {
                failed.push(pair);
                keep.insert(idx);
            }
        }
    }
    cands.retain(|key, _| !removed.contains(key));

    // 分批 embedding。整批失败只影响该批的节点。
    let entries: Vec<(&String, &Candidate)> = cands.iter().collect();
    let mut fresh: Vec<(Node, Vec<f32>)> = Vec::new();
    for chunk in entries.chunks(batch_size.max(1)) {
        let texts: Vec<String> = chunk.iter().map(|(_, c)| embed_text(&c.node)).collect();
        let outcome = match embedder.embed(&texts) {
            Ok(vecs) if vecs.len() == chunk.len() && vecs.iter().all(|v| v.len() == dims) => {
                Ok(vecs)
            }
            Ok(_) => Err("embedding response malformed".to_owned()),
            Err(err) => Err(err.to_string()),
        };
        match outcome {
            Ok(vecs) => {
                for ((_, cand), vector) in chunk.iter().zip(vecs) {
                    fresh.push((cand.node.clone(), vector));
                }
            }
            Err(reason) => {
                for (key, cand) in chunk.iter() {
                    failed.push(((*key).clone(), reason.clone()));
                    keep.insert(cand.record);
                }
            }
        }
    }

    // 旧节点：被替换或被删除的跳过，其余保留旧向量。
    let fresh_keys: HashSet<&str> = fresh.iter().map(|(n, _)| n.key.as_str()).collect();
    let mut nodes: Vec<Node> = Vec::new();
    let mut data: Vec<f32> = Vec::new();
    if let Some(stored) = old {
        for node in &stored.index.nodes {
            if removed.contains(&node.key) || fresh_keys.contains(node.key.as_str()) {
                continue;
            }
            let Some(row) = node.vector_row else {
                continue;
            };
            let Some(slice) = stored.vectors.data.get(row * dims..(row + 1) * dims) else {
                continue;
            };
            let mut kept = node.clone();
            kept.vector_row = Some(nodes.len());
            nodes.push(kept);
            data.extend_from_slice(slice);
        }
    }

    let merged = fresh.len();
    for (mut node, vector) in fresh {
        node.vector_row = Some(nodes.len());
        nodes.push(node);
        data.extend(vector);
    }

    let index = Index {
        meta: IndexMeta {
            schema_version: SCHEMA_VERSION,
            built_at: stamp.built_at.clone(),
            generator: stamp.generator.clone(),
            embedding: EmbeddingMeta {
                model: embedder.model().to_owned(),
                dimensions: dims,
            },
            vectors_sha256: String::new(),
        },
        nodes,
    };
    let keep = keep.into_iter().map(|i| records[i].clone()).collect();
    Ok(Merged {
        index,
        vectors: Vectors {
            dimensions: dims,
            data,
        },
        merged,
        failed,
        keep,
    })
}

/// 一条暂存记录展开为节点。缺少来源信息时返回 `(key, reason)`。
fn nodes_of(
    rec: &StagedRecord,
    sources: &HashMap<String, NodeSource>,
    stamp: &Stamp,
    pinned: &HashSet<&str>,
) -> Result<Vec<Node>, (String, String)> {
    match rec {
        StagedRecord::Command {
            path,
            description,
            examples,
            ..
        } => {
            let key = path.join(" ");
            let source = source_of(sources, &key)?;
            Ok(vec![Node {
                kind: NodeKind::Command,
                parent: parent_of(path),
                name: path.last().cloned().unwrap_or_default(),
                description: description.clone(),
                examples: examples.clone(),
                doc_source: source.doc_source,
                source_hash: source.source_hash.clone(),
                generated_at: stamp.built_at.clone(),
                pinned: pinned.contains(key.as_str()),
                vector_row: None,
                key,
            }])
        }
        StagedRecord::Params { path, params } => {
            let base = path.join(" ");
            let source = source_of(sources, &base)?;
            let mut out = Vec::with_capacity(params.len());
            for param in params {
                let key = format!("{base} {}", param.flag);
                out.push(Node {
                    kind: NodeKind::Param,
                    parent: Some(base.clone()),
                    name: param.flag.clone(),
                    description: param.description.clone(),
                    examples: Vec::new(),
                    doc_source: source.doc_source,
                    source_hash: source.source_hash.clone(),
                    generated_at: stamp.built_at.clone(),
                    pinned: pinned.contains(key.as_str()),
                    vector_row: None,
                    key,
                });
            }
            Ok(out)
        }
    }
}

/// 查找节点的文档来源。
fn source_of<'a>(
    sources: &'a HashMap<String, NodeSource>,
    key: &str,
) -> Result<&'a NodeSource, (String, String)> {
    sources
        .get(key)
        .ok_or_else(|| (key.to_owned(), "no source info for this node".to_owned()))
}

/// 命令的父节点 key。根命令为 `None`。
fn parent_of(path: &[String]) -> Option<String> {
    (path.len() > 1).then(|| path[..path.len() - 1].join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::fake::FakeEmbedder;
    use crate::index::testing::node;

    fn stamp() -> Stamp {
        Stamp {
            built_at: "2026-11-04T00:00:00Z".to_owned(),
            generator: GeneratorMeta {
                backend: "fake".to_owned(),
                agent_version: "test".to_owned(),
                prompt_version: 1,
            },
        }
    }

    fn sources() -> HashMap<String, NodeSource> {
        HashMap::from([(
            "git".to_owned(),
            NodeSource {
                doc_source: DocSource::Man,
                source_hash: "sha256:h".to_owned(),
            },
        )])
    }

    fn git_record(description: &str) -> StagedRecord {
        StagedRecord::Command {
            path: vec!["git".to_owned()],
            description: description.to_owned(),
            examples: Vec::new(),
            subcommands: Vec::new(),
        }
    }

    /// 旧索引：`model` 为模型名，`nodes` 与 `data` 按 3 维向量对应。
    fn old_index(model: &str, nodes: Vec<Node>, data: Vec<f32>) -> Stored {
        Stored {
            index: Index {
                meta: IndexMeta {
                    schema_version: SCHEMA_VERSION,
                    built_at: String::new(),
                    generator: stamp().generator,
                    embedding: EmbeddingMeta {
                        model: model.to_owned(),
                        dimensions: 3,
                    },
                    vectors_sha256: String::new(),
                },
                nodes,
            },
            vectors: Vectors {
                dimensions: 3,
                data,
            },
        }
    }

    fn old_git(row: usize) -> Node {
        let mut n = node("git", NodeKind::Command, None);
        n.description = "old".to_owned();
        n.vector_row = Some(row);
        n
    }

    #[test]
    fn new_record_is_embedded_and_appended() {
        let embedder = FakeEmbedder::new(3).with("git new", vec![1.0, 0.0, 0.0]);
        let merged = merge_staged(
            None,
            &HashSet::new(),
            &[git_record("new")],
            &sources(),
            &embedder,
            20,
            &stamp(),
        )
        .unwrap();
        assert_eq!(merged.merged, 1);
        assert!(merged.failed.is_empty() && merged.keep.is_empty());
        assert_eq!(merged.index.nodes[0].vector_row, Some(0));
        assert_eq!(merged.vectors.data, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn failed_embedding_keeps_old_node_and_record() {
        let old = old_index("fake-model", vec![old_git(0)], vec![0.0, 1.0, 0.0]);
        let merged = merge_staged(
            Some(&old),
            &HashSet::new(),
            &[git_record("new")],
            &sources(),
            &FakeEmbedder::failing(3),
            20,
            &stamp(),
        )
        .unwrap();
        assert_eq!(merged.merged, 0);
        assert_eq!(merged.failed.len(), 1);
        assert_eq!(merged.keep, vec![git_record("new")]);
        assert_eq!(merged.index.nodes[0].description, "old");
        assert_eq!(merged.vectors.data, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn removed_keys_are_dropped_with_their_vectors() {
        let old = old_index("fake-model", vec![old_git(0)], vec![0.0, 1.0, 0.0]);
        let removed = HashSet::from(["git".to_owned()]);
        let merged = merge_staged(
            Some(&old),
            &removed,
            &[],
            &sources(),
            &FakeEmbedder::new(3),
            20,
            &stamp(),
        )
        .unwrap();
        assert!(merged.index.nodes.is_empty());
        assert!(merged.vectors.data.is_empty());
    }

    #[test]
    fn model_change_is_refused() {
        let old = old_index("other-model", vec![old_git(0)], vec![0.0, 1.0, 0.0]);
        let err = merge_staged(
            Some(&old),
            &HashSet::new(),
            &[],
            &sources(),
            &FakeEmbedder::new(3),
            20,
            &stamp(),
        )
        .unwrap_err();
        assert!(matches!(err, GenerateError::ModelChanged { .. }));
    }
}
