//! 增量计划：比较旧索引与本次扫描结果，决定复用、重跑与删除。纯逻辑，不访问文件系统。
//!
//! 预置节点（`pinned`）不参与比较与删除，除非显式出现在 `names` 中。

use std::collections::HashSet;

use sha2::{Digest, Sha256};

use crate::guard::name::validate_command;
use crate::index::{Index, Tree};
use crate::types::{CommandEntry, CommandName, NodeKind};

/// 本次 reindex 的计划。节点以完整 key（空格分隔）表示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// `source_hash` 未变或为预置节点，直接复用旧条目。
    pub keep: Vec<String>,
    /// 需要交给 agent 的节点路径。
    pub regenerate: Vec<Vec<CommandName>>,
    /// 根命令已不在 PATH 的非预置子树，按 key。
    pub remove: Vec<String>,
}

/// 生成计划。
///
/// - `names` 为 `Some` 时只重跑列出的节点，可含预置节点；不比较、不复用、不删除。
/// - `full` 为真时忽略旧 hash，非预置节点全部重跑。
/// - `docs_hash` 返回某路径的 `source_hash`，`None` 表示无文档，该节点跳过。
pub fn make_plan(
    roots: &[CommandEntry],
    old: Option<&Index>,
    names: Option<&[String]>,
    docs_hash: &dyn Fn(&[CommandName]) -> Option<String>,
    full: bool,
) -> Plan {
    let mut plan = Plan::default();

    if let Some(list) = names {
        let wanted: HashSet<&str> = list.iter().map(String::as_str).collect();
        for root in roots {
            if wanted.contains(root.name.as_str()) {
                plan.regenerate.push(vec![root.name.clone()]);
            }
        }
        if let Some(index) = old {
            for node in index.nodes.iter().filter(|n| {
                n.kind == NodeKind::Command && n.parent.is_some() && wanted.contains(n.key.as_str())
            }) {
                if let Some(path) = path_of(&node.key) {
                    plan.regenerate.push(path);
                }
            }
        }
        return plan;
    }

    let tree = old.map(|index| Tree::build(&index.nodes));
    let root_names: HashSet<&str> = roots.iter().map(|r| r.name.as_str()).collect();

    for root in roots {
        let path = vec![root.name.clone()];
        let key = root.name.as_str();
        let Some(hash) = docs_hash(&path) else {
            continue;
        };
        let old_node = tree.as_ref().and_then(|t| t.get(key));
        match old_node {
            Some(node) if node.pinned => plan.keep.push(key.to_owned()),
            Some(node) if !full && node.source_hash == hash => plan.keep.push(key.to_owned()),
            _ => plan.regenerate.push(path),
        }
    }

    if let (Some(index), Some(tree)) = (old, tree.as_ref()) {
        for node in index.nodes.iter().filter(|n| {
            n.parent.is_none()
                && n.kind == NodeKind::Command
                && !n.pinned
                && !root_names.contains(n.key.as_str())
        }) {
            plan.remove.push(node.key.clone());
            for (child, _) in tree.descendants(&node.key) {
                if !child.pinned {
                    plan.remove.push(child.key.clone());
                }
            }
        }
    }
    plan
}

/// 把 key 拆成已校验的路径。任一 token 不合法时返回 `None`。
fn path_of(key: &str) -> Option<Vec<CommandName>> {
    key.split(' ').map(|t| validate_command(t).ok()).collect()
}

/// 计算 `"sha256:<hex>"` 形式的源哈希，输入为文档原文、提示词版本与 embedding 模型名。
///
/// 包含模型名是保守选择：换模型后不复用旧条目，避免描述与新向量的来源不一致。
pub fn source_hash(doc_text: &str, prompt_version: u32, model: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(doc_text.as_bytes());
    hasher.update([0u8]);
    hasher.update(prompt_version.to_be_bytes());
    hasher.update([0u8]);
    hasher.update(model.as_bytes());
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Plan, make_plan};
    use crate::guard::name::validate_command;
    use crate::index::testing::node;
    use crate::index::{EmbeddingMeta, GeneratorMeta, Index, IndexMeta, SCHEMA_VERSION};
    use crate::types::{CommandEntry, CommandName, Node, NodeKind};

    fn entry(name: &str) -> CommandEntry {
        CommandEntry {
            name: validate_command(name).unwrap(),
            path: PathBuf::from(format!("/usr/bin/{name}")),
        }
    }

    fn index(nodes: Vec<Node>) -> Index {
        Index {
            meta: IndexMeta {
                schema_version: SCHEMA_VERSION,
                built_at: String::new(),
                generator: GeneratorMeta {
                    backend: String::new(),
                    agent_version: String::new(),
                    prompt_version: 1,
                },
                embedding: EmbeddingMeta {
                    model: String::new(),
                    dimensions: 1,
                },
                vectors_sha256: String::new(),
            },
            nodes,
        }
    }

    /// 旧节点的 hash 与 `hash` 相同的 `git` 根节点。
    fn old_root(hash: &str) -> Node {
        let mut git = node("git", NodeKind::Command, None);
        git.source_hash = hash.to_owned();
        git
    }

    fn fresh(_: &[CommandName]) -> Option<String> {
        Some("new".to_owned())
    }

    fn keys(list: &[String]) -> Vec<&str> {
        list.iter().map(String::as_str).collect()
    }

    #[test]
    fn new_roots_are_regenerated() {
        let plan = make_plan(&[entry("git")], None, None, &fresh, false);
        assert_eq!(
            plan.regenerate,
            vec![vec![validate_command("git").unwrap()]]
        );
        assert!(plan.keep.is_empty() && plan.remove.is_empty());
    }

    #[test]
    fn matching_hash_is_kept() {
        let idx = index(vec![old_root("new")]);
        let plan = make_plan(&[entry("git")], Some(&idx), None, &fresh, false);
        assert_eq!(plan.keep, ["git"]);
        assert!(plan.regenerate.is_empty());
    }

    #[test]
    fn changed_hash_is_regenerated() {
        let idx = index(vec![old_root("old")]);
        let plan = make_plan(&[entry("git")], Some(&idx), None, &fresh, false);
        assert!(plan.keep.is_empty());
        assert_eq!(plan.regenerate.len(), 1);
    }

    #[test]
    fn full_ignores_matching_hash() {
        let idx = index(vec![old_root("new")]);
        let plan = make_plan(&[entry("git")], Some(&idx), None, &fresh, true);
        assert!(plan.keep.is_empty());
        assert_eq!(plan.regenerate.len(), 1);
    }

    #[test]
    fn missing_docs_skips_the_root() {
        let none = |_: &[CommandName]| None;
        let plan = make_plan(&[entry("git")], None, None, &none, false);
        assert_eq!(plan, Plan::default());
    }

    #[test]
    fn pinned_node_is_never_compared_or_removed() {
        let mut pinned_ls = node("ls", NodeKind::Command, None);
        pinned_ls.pinned = true;
        let idx = index(vec![pinned_ls]);
        let plan = make_plan(&[], Some(&idx), None, &fresh, true);
        assert!(plan.remove.is_empty());
        let plan = make_plan(&[entry("ls")], Some(&idx), None, &fresh, false);
        assert_eq!(plan.keep, ["ls"]);
    }

    #[test]
    fn vanished_root_removes_non_pinned_subtree_only() {
        let mut pinned_child = node("old pinned", NodeKind::Command, Some("old"));
        pinned_child.pinned = true;
        let idx = index(vec![
            node("old", NodeKind::Command, None),
            node("old sub", NodeKind::Command, Some("old")),
            node("old sub --flag", NodeKind::Param, Some("old sub")),
            pinned_child,
        ]);
        let plan = make_plan(&[entry("git")], Some(&idx), None, &fresh, false);
        assert_eq!(keys(&plan.remove), ["old", "old sub", "old sub --flag"]);
    }

    #[test]
    fn names_regenerate_only_listed_nodes_including_pinned() {
        let mut pinned = node("git clone", NodeKind::Command, Some("git"));
        pinned.pinned = true;
        let idx = index(vec![
            old_root("new"),
            pinned,
            node("git pull", NodeKind::Command, Some("git")),
        ]);
        let names = ["git clone".to_owned(), "git".to_owned()];
        let plan = make_plan(&[entry("git")], Some(&idx), Some(&names), &fresh, false);
        assert!(plan.keep.is_empty());
        assert!(plan.remove.is_empty());
        let paths: Vec<String> = plan
            .regenerate
            .iter()
            .map(|p| {
                p.iter()
                    .map(CommandName::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        assert_eq!(paths, ["git", "git clone"]);
    }

    #[test]
    fn source_hash_is_stable_and_sensitive_to_every_input() {
        let base = super::source_hash("doc", 1, "model-a");
        assert_eq!(base, super::source_hash("doc", 1, "model-a"));
        assert!(base.starts_with("sha256:"));
        assert_ne!(base, super::source_hash("doc2", 1, "model-a"));
        assert_ne!(base, super::source_hash("doc", 2, "model-a"));
        assert_ne!(base, super::source_hash("doc", 1, "model-b"));
    }
}
