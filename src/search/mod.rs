//! 查询编排：规范化 → 锚点 → 嵌入 → 子树打分 → 回退 → 候选。
//!
//! 只读索引，不写索引。嵌入器由调用方注入；锚点前缀有误时不会调用嵌入器。

pub mod anchor;
pub mod normalize;
pub mod score;

use crate::embed::{EmbedError, Embedder};
use crate::index::vectors::{VectorError, Vectors, normalize_vector};
use crate::index::{Index, Tree};
use crate::types::{Candidate, Node, NodeKind};
use crate::usage::UsageSnapshot;
use anchor::{Anchor, insertion_text, resolve_anchor};

/// 回退到整棵树时写入 `via` 的标签。
const ROOT_LABEL: &str = "*";

/// 查询阶段的错误。`Anchor`、`EmptyQuery` 对应退出码 2，`Embed` 对应 3。
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// 规范化后查询文本为空。
    #[error("empty query")]
    EmptyQuery,
    /// 锚点前缀有误，如 `giit clone`。
    #[error(transparent)]
    Anchor(#[from] anchor::AnchorError),
    /// embedding 服务不可用。
    #[error(transparent)]
    Embed(#[from] EmbedError),
    /// 向量维度或归一化失败，通常意味着索引与模型不匹配。
    #[error(transparent)]
    Vector(#[from] VectorError),
}

/// 查询选项。
#[derive(Debug, Clone, Copy)]
pub struct QueryOptions {
    /// 最多返回的候选数。
    pub limit: usize,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self { limit: 10 }
    }
}

/// 查询所需的只读依赖。
pub struct QueryContext<'a> {
    /// 已加载的索引。
    pub index: &'a Index,
    /// 与 `index.nodes` 行一一对应的向量。
    pub vectors: &'a Vectors,
    /// 使用记录快照。
    pub usage: &'a UsageSnapshot,
    /// 查询向量化接口。
    pub embedder: &'a dyn Embedder,
    /// 当前时间（Unix 秒）。由调用方传入，便于测试固定时间。
    pub now_unix: i64,
}

/// 完整查询。
///
/// 1. 规范化 `text`，为空返回 `EmptyQuery`。
/// 2. 从 `lbuffer` 解析锚点，前缀有误直接返回 `Anchor`，不调用嵌入器。
/// 3. 嵌入查询文本并归一化。
/// 4. 依次在锚点子树、父节点子树……整棵树中打分，第一个有命中的层级即结果。
///    命中层级之外的候选 `via` 为该层标签（整棵树为 `*`）。
/// 5. 排序后截取 `opts.limit`。无命中返回空列表。
pub fn query(
    ctx: &QueryContext<'_>,
    lbuffer: &str,
    text: &str,
    opts: &QueryOptions,
) -> Result<Vec<Candidate>, SearchError> {
    let normalized = normalize::normalize(text)?;
    let tree = Tree::build(&ctx.index.nodes);
    let anchor = resolve_anchor(lbuffer, &tree)?;
    let query_vec = embed_query(ctx, &normalized)?;

    let probe = Probe {
        ctx,
        tree: &tree,
        anchor: &anchor,
        lbuffer,
        normalized: &normalized,
        query_vec: &query_vec,
    };
    for (scope_key, via) in levels(&tree, &anchor) {
        let scope = scope_nodes(&tree, &ctx.index.nodes, scope_key.as_deref());
        let mut found: Vec<Candidate> = scope
            .into_iter()
            .filter_map(|node| probe.candidate(node, via.as_deref()))
            .collect();
        if !found.is_empty() {
            score::rank(&mut found);
            found.truncate(opts.limit);
            return Ok(found);
        }
    }
    Ok(Vec::new())
}

/// 一次查询内共享的打分上下文。
struct Probe<'a> {
    ctx: &'a QueryContext<'a>,
    tree: &'a Tree<'a>,
    anchor: &'a Anchor,
    lbuffer: &'a str,
    normalized: &'a str,
    query_vec: &'a [f32],
}

impl Probe<'_> {
    /// 对单个节点打分。不是候选（低于阈值且非精确匹配，或已在命令行中出现的参数）时返回 `None`。
    fn candidate(&self, node: &Node, via: Option<&str>) -> Option<Candidate> {
        if node.kind == NodeKind::Param && self.anchor.seen_flags.contains(&node.name) {
            return None;
        }
        let exact = node.key.to_lowercase() == self.normalized;
        // 参数节点只在锚点正是其所属命令时单独作为候选；其余情况只通过子树分抬高所属命令。
        // 参数数量约为命令的 5 倍、文本短，不折叠会挤占命令的名次。
        if node.kind == NodeKind::Param
            && !exact
            && node.parent.as_deref() != self.anchor.key.as_deref()
        {
            return None;
        }
        let descendants: Vec<(f32, usize)> = self
            .tree
            .descendants(&node.key)
            .into_iter()
            .map(|(desc, depth)| (self.score_of(desc), depth))
            .collect();
        let (total, _) = score::subtree_score(self.score_of(node), &descendants);
        if !exact && total < score::MIN_SCORE {
            return None;
        }
        let bonus = self
            .ctx
            .usage
            .stat(&node.key, self.ctx.now_unix)
            .map_or(0.0, |(count, age)| score::usage_bonus(count, age));
        // 只有锚点子树内的候选才计算插入文本，回退结果只给出完整路径。
        let (insert, delete_tail) =
            match (via, insertion_text(self.lbuffer, self.anchor, &node.key)) {
                (None, Some(ins)) => (ins.text, ins.delete_tail),
                _ => (node.key.clone(), 0),
            };
        Some(Candidate {
            key: node.key.clone(),
            kind: node.kind,
            score: total + bonus,
            exact,
            insert,
            delete_tail,
            description: node.description.clone(),
            example: node.examples.first().cloned(),
            dangerous: node
                .examples
                .iter()
                .map(String::as_str)
                .any(crate::guard::output::is_dangerous),
            via: via.map(str::to_owned),
        })
    }

    /// 节点的语义分。没有向量的节点记为 0，它的后代仍可通过子树分命中。
    fn score_of(&self, node: &Node) -> f32 {
        node.vector_row
            .filter(|row| *row < self.ctx.vectors.rows())
            .map_or(0.0, |row| {
                score::self_score(self.query_vec, self.ctx.vectors.row(row))
            })
    }
}

/// 回退层级，从锚点子树开始，逐级向上，最后是整棵树。
///
/// 每项为 `(范围的节点 key，None 表示整棵树；via 标签，None 表示锚点本层)`。
fn levels(tree: &Tree<'_>, anchor: &Anchor) -> Vec<(Option<String>, Option<String>)> {
    let Some(key) = anchor.key.as_deref() else {
        return vec![(None, None)];
    };
    let mut out = vec![(Some(key.to_owned()), None)];
    for ancestor in tree.ancestors(key) {
        out.push((Some(ancestor.key.clone()), Some(ancestor.key.clone())));
    }
    out.push((None, Some(ROOT_LABEL.to_owned())));
    out
}

/// 某层范围内的全部节点（不含范围节点本身）。`None` 为整棵树。
fn scope_nodes<'a>(tree: &Tree<'a>, nodes: &'a [Node], scope: Option<&str>) -> Vec<&'a Node> {
    match scope {
        None => nodes.iter().collect(),
        Some(key) => tree
            .descendants(key)
            .into_iter()
            .map(|(node, _)| node)
            .collect(),
    }
}

/// 嵌入查询文本，检查维度并归一化。
fn embed_query(ctx: &QueryContext<'_>, text: &str) -> Result<Vec<f32>, SearchError> {
    let mut vectors = ctx.embedder.embed(&[text.to_owned()])?;
    let mut query_vec = vectors
        .pop()
        .ok_or_else(|| EmbedError::BadResponse("no vector returned for query".to_owned()))?;
    let expected = ctx.vectors.dimensions;
    if query_vec.len() != expected {
        return Err(VectorError::Dimension {
            expected,
            actual: query_vec.len(),
        }
        .into());
    }
    normalize_vector(&mut query_vec)?;
    Ok(query_vec)
}

/// 测试用固定索引：`sample_nodes()` 的 9 个节点，每个节点独占一个维度的 one-hot 向量。
#[cfg(test)]
pub(crate) mod testing {
    use crate::index::testing::sample_nodes;
    use crate::index::vectors::Vectors;
    use crate::index::{EmbeddingMeta, GeneratorMeta, Index, IndexMeta, SCHEMA_VERSION};

    /// 向量维度，与节点数相同。
    pub(crate) const DIMS: usize = 9;

    /// 第 `row` 维为 1 的单位向量。
    pub(crate) fn one_hot(row: usize) -> Vec<f32> {
        let mut v = vec![0.0; DIMS];
        v[row] = 1.0;
        v
    }

    /// 固定索引与向量。节点顺序见 `sample_nodes()`，第 i 个节点的向量为 `one_hot(i)`。
    pub(crate) fn fixture() -> (Index, Vectors) {
        let mut nodes = sample_nodes();
        let mut data = Vec::with_capacity(nodes.len() * DIMS);
        for (row, node) in nodes.iter_mut().enumerate() {
            node.vector_row = Some(row);
            data.extend(one_hot(row));
        }
        let index = Index {
            meta: IndexMeta {
                schema_version: SCHEMA_VERSION,
                built_at: "2026-11-03T10:00:00Z".to_owned(),
                generator: GeneratorMeta {
                    backend: "test".to_owned(),
                    agent_version: "test".to_owned(),
                    prompt_version: 2,
                },
                embedding: EmbeddingMeta {
                    model: "fake-model".to_owned(),
                    dimensions: DIMS,
                },
                vectors_sha256: String::new(),
            },
            nodes,
        };
        (
            index,
            Vectors {
                dimensions: DIMS,
                data,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{DIMS, fixture, one_hot};
    use super::{QueryContext, QueryOptions, SearchError, query};
    use crate::embed::fake::FakeEmbedder;
    use crate::index::Tree;
    use crate::types::Candidate;
    use crate::usage::UsageSnapshot;

    const NOW: i64 = 1_800_000_000;

    fn run(
        lbuffer: &str,
        text: &str,
        embedder: &FakeEmbedder,
    ) -> Result<Vec<Candidate>, SearchError> {
        let (index, vectors) = fixture();
        let usage = UsageSnapshot::empty();
        let ctx = QueryContext {
            index: &index,
            vectors: &vectors,
            usage: &usage,
            embedder,
            now_unix: NOW,
        };
        query(&ctx, lbuffer, text, &QueryOptions::default())
    }

    #[test]
    fn results_stay_under_anchor_subtree() {
        // "branch" 的向量与 git clone --branch 相同。锚点为 git，参数折叠进 git clone，ls 不应出现。
        let fake = FakeEmbedder::new(DIMS).with("branch", one_hot(2));
        let got = run("git ", "branch", &fake).unwrap();
        assert_eq!(got[0].key, "git clone");
        assert_eq!(got[0].insert, "clone");
        assert_eq!(got[0].delete_tail, 0);
        assert!(got[0].via.is_none());
        assert!(got.iter().all(|c| c.kind != crate::types::NodeKind::Param));
        // 每个候选都必须以 git 为祖先，按节点树判断，而不是按字符串前缀。
        let (index, _) = fixture();
        let tree = Tree::build(&index.nodes);
        assert!(
            got.iter()
                .all(|c| tree.ancestors(&c.key).iter().any(|n| n.key == "git"))
        );
    }

    #[test]
    fn params_are_candidates_when_anchor_is_their_command() {
        let fake = FakeEmbedder::new(DIMS).with("branch", one_hot(2));
        let got = run("git clone ", "branch", &fake).unwrap();
        assert_eq!(got[0].key, "git clone --branch");
        assert_eq!(got[0].insert, "--branch");
        assert!(got[0].via.is_none());
    }

    #[test]
    fn ancestor_gets_decayed_subtree_score() {
        // git clone 没有自身向量命中，只靠子节点 --branch 得到 DECAY × 1.0。
        let fake = FakeEmbedder::new(DIMS).with("branch", one_hot(2));
        let got = run("git ", "branch", &fake).unwrap();
        let clone = got.iter().find(|c| c.key == "git clone").unwrap();
        assert!((clone.score - super::score::DECAY).abs() < 1e-5);
    }

    #[test]
    fn miss_in_anchor_falls_back_to_root_with_via() {
        // 锚点 git pull 与 git 子树都无命中，回退到整棵树，ls 标 via "*"。
        let fake = FakeEmbedder::new(DIMS).with("ls", one_hot(7));
        let got = run("git pull ", "ls", &fake).unwrap();
        assert_eq!(got[0].key, "ls");
        assert_eq!(got[0].via.as_deref(), Some("*"));
        assert_eq!(got[0].insert, "ls");
    }

    #[test]
    fn miss_in_anchor_falls_back_to_parent_with_via() {
        // 锚点 git clone 的子树无命中；父节点 git 的子树中 git pull 命中，via 标父节点。
        let fake = FakeEmbedder::new(DIMS).with("pull", one_hot(4));
        let got = run("git clone ", "pull", &fake).unwrap();
        assert_eq!(got[0].key, "git pull");
        assert_eq!(got[0].via.as_deref(), Some("git"));
        assert_eq!(got[0].delete_tail, 0);
    }

    #[test]
    fn exact_key_match_ranks_first_without_similarity() {
        // 查询向量与 git pull 无关，但文本与 key 完全相等，精确置顶。
        let fake = FakeEmbedder::new(DIMS).with("git pull", one_hot(0));
        let got = run("", "git pull", &fake).unwrap();
        assert_eq!(got[0].key, "git pull");
        assert!(got[0].exact);
    }

    #[test]
    fn typed_param_is_not_recommended_again() {
        let fake = FakeEmbedder::new(DIMS).with("branch", one_hot(2));
        let got = run("git clone --branch x ", "branch", &fake).unwrap();
        assert!(got.iter().all(|c| c.key != "git clone --branch"));
        assert!(!got.is_empty());
    }

    #[test]
    fn anchor_error_does_not_call_embedder() {
        let fake = FakeEmbedder::new(DIMS);
        let result = run("giit clone ", "x", &fake);
        assert!(matches!(result, Err(SearchError::Anchor(_))));
        assert_eq!(fake.calls(), 0);
    }

    #[test]
    fn embed_failure_is_reported() {
        let fake = FakeEmbedder::failing(DIMS);
        assert!(matches!(run("", "ls", &fake), Err(SearchError::Embed(_))));
    }

    #[test]
    fn blank_query_is_rejected() {
        let fake = FakeEmbedder::new(DIMS);
        assert!(matches!(
            run("", "   ", &fake),
            Err(SearchError::EmptyQuery)
        ));
    }
}
