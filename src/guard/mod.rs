//! 输入与输出的集中校验入口，便于审计。
//!
//! 所有来自 agent、MCP、文档或用户输入的命令名、参数名与描述，
//! 都必须经过这里的校验才能进入后续模块。

pub mod name;
pub mod output;

/// 校验失败的错误类型。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuardError {
    /// 命令名不符合 `^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$`。
    #[error("invalid command name: {0:?}")]
    InvalidName(String),
    /// 参数名不符合 `^--[A-Za-z0-9][A-Za-z0-9-]*$`，或总长度超限。
    #[error("invalid flag: {0:?}")]
    InvalidFlag(String),
    /// 短参数（如 `-x`）不允许进入索引。
    #[error("short flag not allowed: {0:?}")]
    ShortFlag(String),
    /// 子命令路径为空。
    #[error("empty command path")]
    EmptyPath,
    /// 描述字符数超过上限。
    #[error("description too long: {0} > 400")]
    DescriptionTooLong(usize),
    /// 示例条数超过 3 条，或单条超过 200 字符。
    #[error("example too long or too many examples")]
    ExampleLimit,
}
