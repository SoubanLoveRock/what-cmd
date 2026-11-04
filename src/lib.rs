//! whatcmd: 用英文自然语言查找终端命令的库。
//!
//! 当前导出 D1 至 D9 阶段的模块：`guard`、`types`、`index`、`search`、`config`、`scan`、
//! `embed`（trait 与占位实现）、`usage`（只读）、`output`、`cli`（仅 `query`）、
//! `agent`（能力与参数拼装，无子进程）、`docs`、`process`（带超时的子进程运行）、
//! `generate`（暂存区写入）、`mcp`（工具的纯函数层，尚未接入 rmcp）。
//! 其余模块随 demo 推进逐步加入。

pub mod agent;
pub mod cli;
pub mod config;
pub mod docs;
pub mod embed;
pub mod generate;
pub mod guard;
pub mod index;
pub mod mcp;
pub mod output;
pub mod process;
pub mod scan;
pub mod search;
pub mod types;
pub mod usage;

#[cfg(test)]
mod testutil;
