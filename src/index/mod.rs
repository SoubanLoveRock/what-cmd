//! 命令树索引与持久化。
//!
//! `Tree` 是内存中的命令树视图，只借用节点，供锚点解析与子树遍历使用。
//! `load` 与 `save_atomic` 负责 `index.json` 与 `vectors.bin` 的读写：
//! 两个文件一起替换，`index.json` 记录 `vectors.bin` 的哈希，加载时校验。

pub mod vectors;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::types::{Node, NodeKind};
use vectors::{VectorError, Vectors};

/// 索引的 schema 版本，对应 `index.json` 的 `schema_version`。
pub const SCHEMA_VERSION: u32 = 2;

/// `index.json` 的文件名。
const INDEX_FILE: &str = "index.json";
/// `vectors.bin` 的文件名。
const VECTORS_FILE: &str = "vectors.bin";

/// `index.json` 的完整内容：元数据与节点列表。向量单独存放在 `vectors.bin`。
///
/// `meta` 使用 `flatten`，因此 JSON 中的 `schema_version` 等字段与 `nodes` 同级。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    /// 元数据，对应 `index.json` 顶层的各字段。
    #[serde(flatten)]
    pub meta: IndexMeta,
    /// 节点列表，对应 `index.json` 的 `nodes`。
    pub nodes: Vec<Node>,
}

/// 索引元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMeta {
    /// 索引格式版本，当前固定为 [`SCHEMA_VERSION`]。
    pub schema_version: u32,
    /// 构建时间，RFC 3339。
    pub built_at: String,
    /// 生成描述所用的 agent 信息。
    pub generator: GeneratorMeta,
    /// 向量所用的 embedding 模型信息。
    pub embedding: EmbeddingMeta,
    /// `vectors.bin` 全部字节的哈希，`"sha256:..."` 形式。`load` 时校验，
    /// 用于发现两次 rename 之间崩溃留下的不一致文件对。
    pub vectors_sha256: String,
}

/// 生成描述所用 agent 的信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorMeta {
    /// agent 后端标识，如 `"claude-code"`。
    pub backend: String,
    /// agent 版本号。
    pub agent_version: String,
    /// 提示词版本，变化即触发增量重建。
    pub prompt_version: u32,
}

/// 向量所用 embedding 模型的信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingMeta {
    /// 模型名。
    pub model: String,
    /// 向量维度。
    pub dimensions: usize,
}

/// `load` 的返回值：索引与对应的向量矩阵。两者已通过长度与哈希校验。
#[derive(Debug, Clone)]
pub struct Stored {
    /// 索引元数据与节点。
    pub index: Index,
    /// 与 `index.nodes` 行一一对应的向量。
    pub vectors: Vectors,
}

/// 索引读写错误。
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// 文件读写失败。
    #[error("index io: {0}")]
    Io(#[from] std::io::Error),
    /// JSON 解析或序列化失败。
    #[error("index json: {0}")]
    Json(#[from] serde_json::Error),
    /// `schema_version` 不是当前版本。
    #[error("unsupported schema_version {0}, rebuild with `whatcmd reindex --full`")]
    SchemaVersion(u32),
    /// `vectors.bin` 的哈希与 `index.json` 记录不符，两者可能不是同一次写入的结果。
    #[error("vectors.bin does not match index.json, rebuild with `whatcmd reindex --full`")]
    VectorHashMismatch,
    /// 向量文件校验或读写失败。
    #[error(transparent)]
    Vector(#[from] VectorError),
}

/// 读取 `index_dir/index.json` 与 `index_dir/vectors.bin`。
///
/// 文件不存在（`index.json` 缺失）返回 `Ok(None)`。以下情况返回错误：
/// `schema_version` 不是 [`SCHEMA_VERSION`]、`vectors.bin` 长度与节点数不符、
/// `vectors.bin` 的哈希与 `index.json` 记录不符。
pub fn load(index_dir: &Path) -> Result<Option<Stored>, IndexError> {
    let text = match fs::read_to_string(index_dir.join(INDEX_FILE)) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    // 先只看 schema_version，避免旧格式因字段不全在反序列化阶段报出无关的错误。
    let raw: serde_json::Value = serde_json::from_str(&text)?;
    let schema = raw
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0);
    if schema != SCHEMA_VERSION {
        warn!(
            found = schema,
            expected = SCHEMA_VERSION,
            "index schema mismatch"
        );
        return Err(IndexError::SchemaVersion(schema));
    }
    let index: Index = serde_json::from_value(raw)?;

    let bytes = fs::read(index_dir.join(VECTORS_FILE))?;
    let vectors = vectors::parse(&bytes, index.meta.embedding.dimensions, index.nodes.len())?;
    if sha256_label(&bytes) != index.meta.vectors_sha256 {
        warn!("vectors.bin does not match index.json sha256");
        return Err(IndexError::VectorHashMismatch);
    }
    info!(nodes = index.nodes.len(), "index loaded");
    Ok(Some(Stored { index, vectors }))
}

/// 写入 `index_dir` 下的 `index.json` 与 `vectors.bin`，两者一起替换。
///
/// 顺序：写 `vectors.bin.tmp` 并 fsync → 计算其哈希 → 写 `index.json.tmp` 并 fsync
/// → rename `vectors.bin` → rename `index.json`。两次 rename 之间崩溃时，
/// 哈希校验会在 `load` 时拒绝这一对文件。`index.meta.vectors_sha256` 的输入值被忽略，
/// 以实际写入的字节为准。
///
/// 行数必须等于节点数，维度必须等于 `index.meta.embedding.dimensions`，否则返回错误且不写文件。
pub fn save_atomic(index_dir: &Path, index: &Index, vectors: &Vectors) -> Result<(), IndexError> {
    let dimensions = index.meta.embedding.dimensions;
    if vectors.dimensions != dimensions {
        return Err(VectorError::Dimension {
            expected: dimensions,
            actual: vectors.dimensions,
        }
        .into());
    }
    if vectors.rows() != index.nodes.len() {
        return Err(VectorError::Length {
            expected: (index.nodes.len() * dimensions * 4) as u64,
            actual: (vectors.rows() * dimensions * 4) as u64,
        }
        .into());
    }

    fs::create_dir_all(index_dir)?;
    let vectors_path = index_dir.join(VECTORS_FILE);
    vectors::write_tmp(&vectors_path, vectors)?;
    let vector_bytes = fs::read(vectors::tmp_path(&vectors_path))?;

    // 借用写出，不复制节点。
    #[derive(Serialize)]
    struct OnDisk<'a> {
        #[serde(flatten)]
        meta: IndexMeta,
        nodes: &'a [Node],
    }
    let on_disk = OnDisk {
        meta: IndexMeta {
            vectors_sha256: sha256_label(&vector_bytes),
            ..index.meta.clone()
        },
        nodes: &index.nodes,
    };
    let body = serde_json::to_vec_pretty(&on_disk)?;

    let index_path = index_dir.join(INDEX_FILE);
    let index_tmp = index_dir.join(format!("{INDEX_FILE}.tmp"));
    let mut file = File::create(&index_tmp)?;
    file.write_all(&body)?;
    file.sync_all()?;

    fs::rename(vectors::tmp_path(&vectors_path), &vectors_path)?;
    fs::rename(&index_tmp, &index_path)?;
    info!(nodes = index.nodes.len(), "index saved");
    Ok(())
}

/// 计算 `"sha256:<hex>"` 形式的哈希标签。
fn sha256_label(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// 按 `parent` 组织的命令树视图。
///
/// 根命令按名字排序，子节点按名字排序（同名时按 key），保证遍历顺序稳定。
pub struct Tree<'a> {
    by_key: HashMap<&'a str, &'a Node>,
    children: HashMap<&'a str, Vec<&'a Node>>,
    roots: BTreeMap<&'a str, &'a Node>,
}

impl<'a> Tree<'a> {
    /// 从节点列表构建树。
    ///
    /// `parent` 为 `None` 的命令节点成为根。`parent` 为 `None` 的参数节点
    /// 不合法，直接忽略。
    pub fn build(nodes: &'a [Node]) -> Tree<'a> {
        let mut by_key = HashMap::with_capacity(nodes.len());
        let mut children: HashMap<&'a str, Vec<&'a Node>> = HashMap::new();
        let mut roots = BTreeMap::new();
        for node in nodes {
            by_key.insert(node.key.as_str(), node);
            match (node.parent.as_deref(), node.kind) {
                (None, NodeKind::Command) => {
                    roots.insert(node.name.as_str(), node);
                }
                (None, NodeKind::Param) => {}
                (Some(parent), _) => children.entry(parent).or_default().push(node),
            }
        }
        for list in children.values_mut() {
            list.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.key.cmp(&b.key)));
        }
        Tree {
            by_key,
            children,
            roots,
        }
    }

    /// 按名字查找根命令。
    pub fn root(&self, name: &str) -> Option<&'a Node> {
        self.roots.get(name).copied()
    }

    /// 列出全部根命令名，按字典序。
    pub fn root_names(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.roots.keys().copied()
    }

    /// 按完整 key 查找节点。
    pub fn get(&self, key: &str) -> Option<&'a Node> {
        self.by_key.get(key).copied()
    }

    /// 直接子节点（命令与参数），按名字排序。
    pub fn children(&self, key: &str) -> Vec<&'a Node> {
        self.children.get(key).cloned().unwrap_or_default()
    }

    /// 子树中所有后代及其相对深度（直接子节点为 1），按先序遍历。
    ///
    /// 对异常数据中的环做了防护，每个节点只访问一次。
    pub fn descendants(&self, key: &str) -> Vec<(&'a Node, usize)> {
        let mut out = Vec::new();
        let mut seen: HashSet<&str> = HashSet::from([key]);
        let mut stack: Vec<(&'a Node, usize)> = self
            .children(key)
            .into_iter()
            .rev()
            .map(|node| (node, 1))
            .collect();
        while let Some((node, depth)) = stack.pop() {
            if !seen.insert(node.key.as_str()) {
                continue;
            }
            out.push((node, depth));
            for child in self.children(&node.key).into_iter().rev() {
                stack.push((child, depth + 1));
            }
        }
        out
    }

    /// 从父到根的祖先链。`key` 本身不在结果中。对环做了防护。
    pub fn ancestors(&self, key: &str) -> Vec<&'a Node> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut current = self.get(key);
        while let Some(node) = current {
            seen.insert(node.key.as_str());
            current = node
                .parent
                .as_deref()
                .and_then(|parent| self.get(parent))
                .filter(|parent| !seen.contains(parent.key.as_str()));
            if let Some(parent) = current {
                out.push(parent);
            }
        }
        out
    }
}

/// 测试用的节点构造工具，供 `index` 与 `search` 的单元测试共用。
#[cfg(test)]
pub(crate) mod testing {
    use crate::types::{DocSource, Node, NodeKind};

    /// 构造一个最小节点。`name` 取 key 的最后一段。
    pub(crate) fn node(key: &str, kind: NodeKind, parent: Option<&str>) -> Node {
        Node {
            key: key.to_owned(),
            kind,
            parent: parent.map(str::to_owned),
            name: key.rsplit(' ').next().unwrap_or(key).to_owned(),
            description: String::new(),
            examples: Vec::new(),
            doc_source: DocSource::Man,
            source_hash: String::new(),
            generated_at: String::new(),
            pinned: false,
            vector_row: None,
        }
    }

    /// 固定的小型命令树：`git`（含子命令与参数）、`ls`、`grep`。
    pub(crate) fn sample_nodes() -> Vec<Node> {
        vec![
            node("git", NodeKind::Command, None),
            node("git clone", NodeKind::Command, Some("git")),
            node("git clone --branch", NodeKind::Param, Some("git clone")),
            node("git clone --depth", NodeKind::Param, Some("git clone")),
            node("git pull", NodeKind::Command, Some("git")),
            node("git remote", NodeKind::Command, Some("git")),
            node("git remote add", NodeKind::Command, Some("git remote")),
            node("ls", NodeKind::Command, None),
            node("grep", NodeKind::Command, None),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::Tree;
    use super::testing::{node, sample_nodes};
    use crate::types::NodeKind;

    fn keys<'a>(nodes: &[&'a crate::types::Node]) -> Vec<&'a str> {
        nodes.iter().map(|n| n.key.as_str()).collect()
    }

    #[test]
    fn roots_are_parentless_commands_sorted_by_name() {
        let nodes = sample_nodes();
        let tree = Tree::build(&nodes);
        assert_eq!(tree.root_names().collect::<Vec<_>>(), ["git", "grep", "ls"]);
        assert!(tree.root("clone").is_none());
        assert!(tree.root("git").is_some());
    }

    #[test]
    fn parentless_param_is_ignored() {
        let mut nodes = sample_nodes();
        nodes.push(node("--stray", NodeKind::Param, None));
        let tree = Tree::build(&nodes);
        assert_eq!(tree.root_names().count(), 3);
    }

    #[test]
    fn children_are_sorted_and_include_params() {
        let nodes = sample_nodes();
        let tree = Tree::build(&nodes);
        assert_eq!(
            keys(&tree.children("git")),
            ["git clone", "git pull", "git remote"]
        );
        assert_eq!(
            keys(&tree.children("git clone")),
            ["git clone --branch", "git clone --depth"]
        );
        assert!(tree.children("ls").is_empty());
    }

    #[test]
    fn descendants_are_preorder_with_relative_depth() {
        let nodes = sample_nodes();
        let tree = Tree::build(&nodes);
        let got: Vec<(String, usize)> = tree
            .descendants("git")
            .into_iter()
            .map(|(n, d)| (n.key.clone(), d))
            .collect();
        let want: Vec<(String, usize)> = [
            ("git clone", 1),
            ("git clone --branch", 2),
            ("git clone --depth", 2),
            ("git pull", 1),
            ("git remote", 1),
            ("git remote add", 2),
        ]
        .iter()
        .map(|(k, d)| (k.to_string(), *d))
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn descendants_of_leaf_are_empty() {
        let nodes = sample_nodes();
        let tree = Tree::build(&nodes);
        assert!(tree.descendants("git pull").is_empty());
        assert!(tree.descendants("missing").is_empty());
    }

    #[test]
    fn ancestors_run_from_parent_to_root() {
        let nodes = sample_nodes();
        let tree = Tree::build(&nodes);
        assert_eq!(
            keys(&tree.ancestors("git remote add")),
            ["git remote", "git"]
        );
        assert!(tree.ancestors("git").is_empty());
    }

    #[test]
    fn cyclic_parent_links_terminate() {
        let nodes = vec![
            node("x", NodeKind::Command, None),
            node("x a", NodeKind::Command, Some("x b")),
            node("x b", NodeKind::Command, Some("x a")),
        ];
        let tree = Tree::build(&nodes);
        assert_eq!(keys(&tree.ancestors("x a")), ["x b"]);
        let below: Vec<&str> = tree
            .descendants("x a")
            .iter()
            .map(|(n, _)| n.key.as_str())
            .collect();
        assert_eq!(below, ["x b"]);
    }

    #[test]
    fn get_finds_by_full_key() {
        let nodes = sample_nodes();
        let tree = Tree::build(&nodes);
        assert_eq!(
            tree.get("git clone").map(|n| n.kind),
            Some(NodeKind::Command)
        );
        assert!(tree.get("git cl").is_none());
    }
}

/// `load` 与 `save_atomic` 的文件系统测试。全部使用 `tempfile`，不访问真实 `$HOME`。
#[cfg(test)]
mod persist_tests {
    use std::fs;

    use super::testing::sample_nodes;
    use super::vectors::{VectorError, Vectors};
    use super::{
        EmbeddingMeta, GeneratorMeta, Index, IndexError, IndexMeta, SCHEMA_VERSION, load,
        save_atomic,
    };

    const DIMENSIONS: usize = 2;

    /// 与 `sample_nodes()` 等长的索引与向量。
    fn sample() -> (Index, Vectors) {
        let nodes = sample_nodes();
        let rows = nodes.len();
        let index = Index {
            meta: IndexMeta {
                schema_version: SCHEMA_VERSION,
                built_at: "2026-11-03T10:00:00Z".to_owned(),
                generator: GeneratorMeta {
                    backend: "claude-code".to_owned(),
                    agent_version: "test".to_owned(),
                    prompt_version: 2,
                },
                embedding: EmbeddingMeta {
                    model: "test-model".to_owned(),
                    dimensions: DIMENSIONS,
                },
                vectors_sha256: String::new(),
            },
            nodes,
        };
        let vectors = Vectors {
            dimensions: DIMENSIONS,
            data: (0..rows * DIMENSIONS).map(|i| i as f32 * 0.5).collect(),
        };
        (index, vectors)
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = crate::testutil::tempdir();
        let (index, vectors) = sample();
        save_atomic(dir.path(), &index, &vectors).unwrap();

        let stored = load(dir.path()).unwrap().expect("index should exist");
        assert_eq!(stored.index.nodes.len(), index.nodes.len());
        assert_eq!(stored.index.nodes[0].key, index.nodes[0].key);
        assert_eq!(stored.vectors, vectors);
        assert!(stored.index.meta.vectors_sha256.starts_with("sha256:"));
    }

    #[test]
    fn save_leaves_no_tmp_files() {
        let dir = crate::testutil::tempdir();
        let (index, vectors) = sample();
        save_atomic(dir.path(), &index, &vectors).unwrap();
        assert!(!dir.path().join("index.json.tmp").exists());
        assert!(!dir.path().join("vectors.bin.tmp").exists());
    }

    #[test]
    fn empty_directory_loads_as_none() {
        let dir = crate::testutil::tempdir();
        assert!(load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn schema_mismatch_is_rejected() {
        let dir = crate::testutil::tempdir();
        fs::write(dir.path().join("index.json"), r#"{"schema_version": 1}"#).unwrap();
        assert!(matches!(
            load(dir.path()),
            Err(IndexError::SchemaVersion(1))
        ));
    }

    #[test]
    fn truncated_vectors_are_rejected_by_length() {
        let dir = crate::testutil::tempdir();
        let (index, vectors) = sample();
        save_atomic(dir.path(), &index, &vectors).unwrap();
        fs::write(dir.path().join("vectors.bin"), [0u8; 4]).unwrap();
        assert!(matches!(
            load(dir.path()),
            Err(IndexError::Vector(VectorError::Length { .. }))
        ));
    }

    #[test]
    fn same_length_corruption_is_rejected_by_hash() {
        // 长度正确但内容被改动，只有哈希校验能发现。
        let dir = crate::testutil::tempdir();
        let (index, vectors) = sample();
        save_atomic(dir.path(), &index, &vectors).unwrap();
        let path = dir.path().join("vectors.bin");
        let mut bytes = fs::read(&path).unwrap();
        bytes[0] ^= 0xff;
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            load(dir.path()),
            Err(IndexError::VectorHashMismatch)
        ));
    }

    #[test]
    fn save_rejects_row_count_mismatch_without_writing() {
        let dir = crate::testutil::tempdir();
        let (index, mut vectors) = sample();
        vectors.data.truncate(DIMENSIONS);
        assert!(matches!(
            save_atomic(dir.path(), &index, &vectors),
            Err(IndexError::Vector(VectorError::Length { .. }))
        ));
        assert!(!dir.path().join("index.json").exists());
        assert!(!dir.path().join("vectors.bin").exists());
    }
}
