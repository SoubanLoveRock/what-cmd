//! 锚点解析与插入文本计算。纯函数，不联网、不读文件。
//!
//! 规则见 DESIGN.md 第 7.1 与 7.4 节。锚点解析只读取 `LBUFFER`，
//! 插入计算返回的 `Insertion` 由 widget 应用到 `LBUFFER`。

use crate::index::Tree;
use crate::types::NodeKind;

/// 修正建议的编辑距离上限。超过则不给建议。
const SUGGEST_MAX_DISTANCE: usize = 2;

/// 锚点解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// 锚点命令节点的 key。没有任何已完成 token 时为 `None`，表示搜索范围是全部节点。
    pub key: Option<String>,
    /// 根命令名。没有任何已完成 token 时为空串。
    pub root: String,
    /// 末尾未完成的词是否已丢弃。
    pub dropped_partial: bool,
    /// 已输入的长参数名（`=` 前的部分），用于候选去重。
    pub seen_flags: Vec<String>,
}

/// 插入动作：widget 先删除 `LBUFFER` 末尾 `delete_tail` 个字符，再写入 `text`。
///
/// `delete_tail` 用于替换被丢弃的未完成词，例如 `git cl` 插入 `git clone` 时，
/// 删除 `cl` 再写入 `clone`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    /// 需要先删除的末尾字符数（按 `char` 计）。
    pub delete_tail: usize,
    /// 要写入 `LBUFFER` 光标处的文本，可能为空。
    pub text: String,
}

/// 锚点解析错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnchorError {
    /// 第一个 token 不是已知根命令。`suggestion` 为编辑距离不超过 2 的最近根命令。
    #[error("no command named {typed}{}", hint(.suggestion))]
    UnknownRoot {
        /// 用户输入的根命令名。
        typed: String,
        /// 唯一的修正建议，不自动替换。
        suggestion: Option<String>,
    },
}

/// 从 `LBUFFER` 解析锚点。
///
/// - 末尾非空白的 token 视为未完成，丢弃。
/// - 第一个 token 必须是根命令，否则返回 `UnknownRoot`。
/// - 之后逐个处理：`--` 单独出现时停止锚点解析；`-` 开头的 token 跳过；
///   是当前节点的子命令则下降；其他 token 停止锚点解析。
/// - 长参数名（`=` 前部分）记入 `seen_flags`，范围是第一个独立 `--` 之前的全部 token。
pub fn resolve_anchor(lbuffer: &str, tree: &Tree<'_>) -> Result<Anchor, AnchorError> {
    let dropped_partial = !lbuffer.is_empty() && !lbuffer.ends_with(char::is_whitespace);
    let all: Vec<&str> = lbuffer.split_whitespace().collect();
    let completed: &[&str] = if dropped_partial {
        &all[..all.len() - 1]
    } else {
        &all
    };

    let Some((&first, rest)) = completed.split_first() else {
        return Ok(Anchor {
            key: None,
            root: String::new(),
            dropped_partial,
            seen_flags: Vec::new(),
        });
    };
    let root = tree.root(first).ok_or_else(|| AnchorError::UnknownRoot {
        typed: first.to_owned(),
        suggestion: suggest_root(first, tree),
    })?;

    let mut node = root;
    let mut descending = true;
    for &token in rest {
        if token == "--" {
            break;
        }
        if token.starts_with('-') || !descending {
            continue;
        }
        match child_command(tree, &node.key, token) {
            Some(child) => node = child,
            None => descending = false,
        }
    }

    Ok(Anchor {
        key: Some(node.key.clone()),
        root: first.to_owned(),
        dropped_partial,
        seen_flags: collect_flags(rest),
    })
}

/// 计算选中某个候选后要对 `LBUFFER` 做的修改。纯函数。
///
/// `candidate_key` 为候选的完整路径（命令或参数 key）。返回值含义：
/// - `None`：候选不在锚点子树内（如回退到父节点的结果），本函数不计算插入，由调用方处理。
/// - `Some` 且 `text` 为空：候选已是 `LBUFFER` 末尾的命令链，widget 不做任何修改。
/// - 否则：先删除 `delete_tail` 个字符，再写入 `text`。
///
/// 写入的是候选路径去掉锚点路径后的剩余部分，前面不补空格：
/// 被丢弃的未完成词之前的前缀总以空白结尾，或为空。
pub fn insertion_text(lbuffer: &str, anchor: &Anchor, candidate_key: &str) -> Option<Insertion> {
    let (prefix, partial) = split_partial(lbuffer, anchor.dropped_partial);
    let anchor_tokens: Vec<&str> = anchor
        .key
        .as_deref()
        .map_or_else(Vec::new, |key| key.split(' ').collect());
    let candidate_tokens: Vec<&str> = candidate_key.split(' ').collect();
    if !candidate_tokens.starts_with(&anchor_tokens) {
        return None;
    }

    let prefix_tokens: Vec<&str> = prefix.split_whitespace().collect();
    let remainder = &candidate_tokens[anchor_tokens.len()..];
    if remainder.is_empty() || prefix_tokens.ends_with(&candidate_tokens) {
        return Some(Insertion {
            delete_tail: 0,
            text: String::new(),
        });
    }
    Some(Insertion {
        delete_tail: partial.chars().count(),
        text: remainder.join(" "),
    })
}

/// 把 `LBUFFER` 拆成"已完成的前缀"和"被丢弃的末尾未完成词"。
fn split_partial(lbuffer: &str, dropped: bool) -> (&str, &str) {
    if !dropped {
        return (lbuffer, "");
    }
    let cut = lbuffer.trim_end_matches(|c: char| !c.is_whitespace()).len();
    lbuffer.split_at(cut)
}

/// 在 `parent_key` 的直接子命令中查找名为 `token` 的命令节点（参数节点不算）。
fn child_command<'a>(
    tree: &Tree<'a>,
    parent_key: &str,
    token: &str,
) -> Option<&'a crate::types::Node> {
    tree.children(parent_key)
        .into_iter()
        .find(|child| child.kind == NodeKind::Command && child.name == token)
}

/// 收集 `tokens` 中第一个独立 `--` 之前的长参数名，去掉 `=` 后的部分。
fn collect_flags(tokens: &[&str]) -> Vec<String> {
    tokens
        .iter()
        .copied()
        .take_while(|token| *token != "--")
        .filter(|token| token.starts_with("--"))
        .map(|token| {
            token
                .split_once('=')
                .map_or(token, |(name, _)| name)
                .to_owned()
        })
        .collect()
}

/// 为未知根命令找一个修正建议：编辑距离不超过 2，取最近的，并列时取字典序靠前的。
fn suggest_root(typed: &str, tree: &Tree<'_>) -> Option<String> {
    tree.root_names()
        .map(|name| (edit_distance(typed, name), name))
        .filter(|(distance, _)| *distance <= SUGGEST_MAX_DISTANCE)
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, name)| name.to_owned())
}

/// 错误信息中的建议部分。
fn hint(suggestion: &Option<String>) -> String {
    suggestion
        .as_ref()
        .map_or_else(String::new, |name| format!(", did you mean {name}?"))
}

/// Levenshtein 编辑距离，按 `char` 计算。
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::{AnchorError, Insertion, edit_distance, insertion_text, resolve_anchor};
    use crate::index::Tree;
    use crate::index::testing::sample_nodes;
    use crate::search::anchor::Anchor;

    /// 用固定样本树解析 `lbuffer`。
    fn resolve(lbuffer: &str) -> Result<Anchor, AnchorError> {
        let nodes = sample_nodes();
        let tree = Tree::build(&nodes);
        resolve_anchor(lbuffer, &tree)
    }

    fn key_of(lbuffer: &str) -> Option<String> {
        resolve(lbuffer).expect("anchor should resolve").key
    }

    fn insert(lbuffer: &str, candidate: &str) -> Option<Insertion> {
        let anchor = resolve(lbuffer).expect("anchor should resolve");
        insertion_text(lbuffer, &anchor, candidate)
    }

    #[test]
    fn empty_and_blank_buffers_anchor_at_root() {
        let empty = resolve("").unwrap();
        assert_eq!(empty.key, None);
        assert_eq!(empty.root, "");
        assert!(!empty.dropped_partial);

        let blank = resolve("   ").unwrap();
        assert_eq!(blank.key, None);
        assert!(!blank.dropped_partial);
    }

    #[test]
    fn trailing_partial_word_is_dropped() {
        let anchor = resolve("git cl").unwrap();
        assert_eq!(anchor.key.as_deref(), Some("git"));
        assert!(anchor.dropped_partial);
    }

    #[test]
    fn bare_partial_root_is_dropped_too() {
        // 没有空格的第一个词也是未完成的，锚点回到根。
        let anchor = resolve("git").unwrap();
        assert_eq!(anchor.key, None);
        assert!(anchor.dropped_partial);
    }

    #[test]
    fn completed_subcommand_descends() {
        assert_eq!(key_of("git clone ").as_deref(), Some("git clone"));
        assert_eq!(key_of("git remote add ").as_deref(), Some("git remote add"));
        assert_eq!(key_of("git remote ").as_deref(), Some("git remote"));
    }

    #[test]
    fn short_flag_is_skipped_without_affecting_anchor() {
        assert_eq!(key_of("ls -la ").as_deref(), Some("ls"));
        assert_eq!(key_of("git -c x clone ").as_deref(), Some("git"));
    }

    #[test]
    fn unknown_token_stops_descent() {
        // `-C repo` 中的 repo 不是子命令，停止锚点解析，clone 不参与。
        assert_eq!(key_of("git -C repo clone ").as_deref(), Some("git"));
    }

    #[test]
    fn long_flag_is_skipped_and_descent_continues() {
        let anchor = resolve("git --no-pager clone ").unwrap();
        assert_eq!(anchor.key.as_deref(), Some("git clone"));
        assert_eq!(anchor.seen_flags, ["--no-pager"]);
    }

    #[test]
    fn double_dash_stops_anchor_and_hides_later_flags() {
        assert_eq!(key_of("git -- clone ").as_deref(), Some("git"));
        let anchor = resolve("git -- --branch ").unwrap();
        assert_eq!(anchor.key.as_deref(), Some("git"));
        assert!(anchor.seen_flags.is_empty());
    }

    #[test]
    fn flags_are_recorded_by_name_before_equals() {
        let anchor = resolve("git clone --branch=main ").unwrap();
        assert_eq!(anchor.seen_flags, ["--branch"]);
    }

    #[test]
    fn flags_after_anchor_stopped_are_still_recorded() {
        // repo 停止了锚点解析，但 --branch 在 `--` 之前，仍计入去重。
        let anchor = resolve("git -C repo clone --branch x ").unwrap();
        assert_eq!(anchor.key.as_deref(), Some("git"));
        assert_eq!(anchor.seen_flags, ["--branch"]);
    }

    #[test]
    fn unknown_root_reports_typo_with_suggestion() {
        let err = resolve("giit clone ").unwrap_err();
        assert_eq!(
            err,
            AnchorError::UnknownRoot {
                typed: "giit".to_owned(),
                suggestion: Some("git".to_owned()),
            }
        );
        assert_eq!(err.to_string(), "no command named giit, did you mean git?");
    }

    #[test]
    fn transposed_root_gets_suggestion() {
        let err = resolve("gti ").unwrap_err();
        assert_eq!(err.to_string(), "no command named gti, did you mean git?");
    }

    #[test]
    fn unknown_root_without_close_match_has_no_suggestion() {
        let err = resolve("zzqq ").unwrap_err();
        assert_eq!(err.to_string(), "no command named zzqq");
    }

    #[test]
    fn unfinished_unknown_root_is_treated_as_partial_word() {
        // 还没输入空格的第一个词被视为未完成，不报错。
        let anchor = resolve("giit").unwrap();
        assert_eq!(anchor.key, None);
    }

    #[test]
    fn edit_distance_counts_chars() {
        assert_eq!(edit_distance("git", "git"), 0);
        assert_eq!(edit_distance("giit", "git"), 1);
        assert_eq!(edit_distance("gti", "git"), 2);
        assert_eq!(edit_distance("", "ls"), 2);
        assert_eq!(edit_distance("ü", "u"), 1);
    }

    #[test]
    fn insert_replaces_partial_word_with_subcommand() {
        assert_eq!(
            insert("git cl", "git clone"),
            Some(Insertion {
                delete_tail: 2,
                text: "clone".to_owned(),
            })
        );
    }

    #[test]
    fn insert_after_space_appends_without_deleting() {
        assert_eq!(
            insert("git ", "git clone"),
            Some(Insertion {
                delete_tail: 0,
                text: "clone".to_owned(),
            })
        );
    }

    #[test]
    fn unfinished_completed_word_is_replaced_in_full() {
        // `git clone` 无末尾空格：clone 被丢弃，整体替换为候选。
        assert_eq!(
            insert("git clone", "git clone"),
            Some(Insertion {
                delete_tail: 5,
                text: "clone".to_owned(),
            })
        );
    }

    #[test]
    fn insert_into_empty_buffer_writes_full_path() {
        assert_eq!(
            insert("", "ls"),
            Some(Insertion {
                delete_tail: 0,
                text: "ls".to_owned(),
            })
        );
    }

    #[test]
    fn insert_root_over_partial_root_replaces_it() {
        assert_eq!(
            insert("gi", "git"),
            Some(Insertion {
                delete_tail: 2,
                text: "git".to_owned(),
            })
        );
    }

    #[test]
    fn chain_already_at_end_is_not_appended_again() {
        assert_eq!(
            insert("git clone ", "git clone"),
            Some(Insertion {
                delete_tail: 0,
                text: String::new(),
            })
        );
    }

    #[test]
    fn param_already_typed_is_not_appended_again() {
        assert_eq!(
            insert("git clone --branch ", "git clone --branch"),
            Some(Insertion {
                delete_tail: 0,
                text: String::new(),
            })
        );
    }

    #[test]
    fn param_is_appended_after_its_command() {
        assert_eq!(
            insert("git clone ", "git clone --branch"),
            Some(Insertion {
                delete_tail: 0,
                text: "--branch".to_owned(),
            })
        );
    }

    #[test]
    fn partial_flag_is_replaced_by_full_param() {
        assert_eq!(
            insert("git clone --br", "git clone --branch"),
            Some(Insertion {
                delete_tail: 4,
                text: "--branch".to_owned(),
            })
        );
    }

    #[test]
    fn candidate_outside_anchor_subtree_is_not_inserted() {
        let anchor = resolve("git clone ").unwrap();
        assert_eq!(insertion_text("git clone ", &anchor, "ls"), None);
    }
}
