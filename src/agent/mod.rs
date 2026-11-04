//! Agent 适配层：能力集合、批次请求与后端 trait。
//!
//! D7 只包含能力定义与两个纯函数（参数拼装、`--help` 解析），不启动子进程。
//! 具体后端见 [`claude`]，探测入口见 [`detect`]。

pub mod claude;
pub mod detect;

use std::collections::BTreeSet;
use std::path::Path;

use crate::types::CommandName;

/// 后端需要具备的能力。`reindex` 要求 [`REQUIRED`] 中的全部能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// `-p`，非交互运行。
    NonInteractive,
    /// `--output-format`，输出 JSON。
    JsonOutput,
    /// `--mcp-config`，加载 whatcmd 的 MCP server。
    Mcp,
    /// `--strict-mcp-config`，忽略用户已有的 MCP 配置。
    StrictMcp,
    /// `--allowedTools`，放行指定工具。
    ToolAllowlist,
    /// `--tools`，限定内置工具集。
    RestrictedTools,
}

/// 后端报告的能力集合。
pub type CapabilitySet = BTreeSet<Capability>;

/// `reindex` 要求的全部能力。
pub const REQUIRED: [Capability; 6] = [
    Capability::NonInteractive,
    Capability::JsonOutput,
    Capability::Mcp,
    Capability::StrictMcp,
    Capability::ToolAllowlist,
    Capability::RestrictedTools,
];

/// 返回 [`REQUIRED`] 中不在 `set` 里的能力，保持声明顺序。
pub fn missing_capabilities(set: &CapabilitySet) -> Vec<Capability> {
    REQUIRED
        .iter()
        .copied()
        .filter(|c| !set.contains(c))
        .collect()
}

/// 交给后端的一批节点及提示词。
pub struct BatchRequest<'a> {
    /// 本次 reindex 运行的标识，用于暂存区文件名。
    pub run_id: &'a str,
    /// 本批次的节点路径（命令节点或其父命令）。
    pub paths: &'a [Vec<CommandName>],
    /// 来自 `prompts/describe.md`，已附父节点上下文。
    pub prompt: &'a str,
    /// 指向临时生成的 `mcp.json`。
    pub mcp_config: &'a Path,
}

/// 一批执行的结果。描述与参数以暂存区为准，这里只用于日志。
#[derive(Debug)]
pub struct BatchOutcome {
    /// 后端是否报告成功。
    pub ok: bool,
    /// 原始输出，仅用于 debug 日志，调用方需截断。
    pub raw_output: String,
}

/// Agent 后端的统一接口。
pub trait AgentBackend {
    /// 后端标识，如 `claude-code`。
    fn id(&self) -> &'static str;
    /// 已探测到的能力。
    fn capabilities(&self) -> &CapabilitySet;
    /// 执行一批节点。
    fn run_batch(&self, req: &BatchRequest<'_>) -> Result<BatchOutcome, AgentError>;
}

/// Agent 层错误。
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// 后端缺少 `reindex` 所需的能力。
    #[error("missing capabilities: {0:?}")]
    MissingCapability(Vec<Capability>),
    /// 子进程以非零状态退出。
    #[error("agent exited with status {0}")]
    ExitStatus(i32),
    /// 子进程超时。
    #[error("agent timed out")]
    Timeout,
    /// 输出不是预期的 JSON 结构。
    #[error("unexpected agent output: {0}")]
    BadOutput(String),
    /// 启动或读写子进程失败。
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::{Capability, CapabilitySet, REQUIRED, missing_capabilities};

    #[test]
    fn empty_set_reports_every_required_capability() {
        let missing = missing_capabilities(&CapabilitySet::new());
        assert_eq!(missing, REQUIRED.to_vec());
    }

    #[test]
    fn full_set_reports_nothing_missing() {
        let full: CapabilitySet = REQUIRED.into_iter().collect();
        assert!(missing_capabilities(&full).is_empty());
    }

    #[test]
    fn partial_set_reports_only_the_gap() {
        let mut set = CapabilitySet::new();
        set.insert(Capability::NonInteractive);
        set.insert(Capability::StrictMcp);
        let missing = missing_capabilities(&set);
        assert!(!missing.contains(&Capability::NonInteractive));
        assert!(!missing.contains(&Capability::StrictMcp));
        assert!(missing.contains(&Capability::RestrictedTools));
        assert_eq!(missing.len(), REQUIRED.len() - 2);
    }
}
