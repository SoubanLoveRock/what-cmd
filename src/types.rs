//! 共享类型：命令名、节点、候选等。
//!
//! `CommandName` 只能经 `guard::name` 构造，其他模块只读取它。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// 经过校验的命令名。只能通过 `guard::name::validate_command` 构造。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandName(String);

impl CommandName {
    /// 仅供 `guard::name` 在通过校验后调用，外部模块不得绕过校验构造。
    pub(crate) fn from_validated(raw: String) -> Self {
        Self(raw)
    }

    /// 返回命令名的字符串形式。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 节点类型：命令节点或参数节点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    /// 命令路径，如 `git`、`git pull`。
    Command,
    /// 挂在命令下的长参数，如 `git clone --branch`。
    Param,
}

/// 文档来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocSource {
    /// 来自 man 页。
    Man,
    /// 来自 `--help` 输出。
    Help,
    /// man 与 `--help` 都不可用。
    Missing,
}

/// PATH 中发现的一个可执行文件，对应根命令候选。
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// 已校验的命令名。
    pub name: CommandName,
    /// 第一次在 PATH 中出现的位置。
    pub path: PathBuf,
}

/// 命令树中的一个节点（对应 `index.json` 的 `nodes[i]`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// 命令节点为完整路径；参数节点为"所属路径 + 空格 + 参数名"。
    pub key: String,
    /// 节点类型。
    pub kind: NodeKind,
    /// 命令的父是上一级命令，根命令为 `None`；参数的父是所属命令。
    pub parent: Option<String>,
    /// 命令节点为最后一段；参数节点为 `--flag`。
    pub name: String,
    /// 描述文本。
    pub description: String,
    /// 示例，参数节点恒为空。
    pub examples: Vec<String>,
    /// 文档来源。
    pub doc_source: DocSource,
    /// `sha256:...` 形式的源哈希。
    pub source_hash: String,
    /// RFC 3339 时间戳。
    pub generated_at: String,
    /// 预置节点为 `true`，reindex 不比较、不删除。
    pub pinned: bool,
    /// 在 `vectors.bin` 中的行号；无向量时为 `None`。
    pub vector_row: Option<usize>,
}

/// 查询结果的一行候选。
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// 完整路径，也是使用记录用的 key。
    pub key: String,
    /// 节点类型。
    pub kind: NodeKind,
    /// `s(n) + bonus(n)`。
    pub score: f32,
    /// 是否为精确置顶。
    pub exact: bool,
    /// 插入 `LBUFFER` 的文本，已去重。
    pub insert: String,
    /// 替换 `LBUFFER` 末尾未完成词时，widget 先删除的字符数。
    pub delete_tail: usize,
    /// 描述文本。
    pub description: String,
    /// 一条示例。
    pub example: Option<String>,
    /// 由 `guard::output::is_dangerous` 设置。
    pub dangerous: bool,
    /// 回退到父节点时，标出实际命中的最佳子路径。
    pub via: Option<String>,
}
