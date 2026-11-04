//! whatcmd: 用英文自然语言查找终端命令的库。
//!
//! 模块：`guard`、`types`、`index`、`search`、`config`、`scan`、`embed`、`usage`、
//! `output`、`cli`（`query`、`record`、`reindex`）、`agent`、`docs`、`process`、
//! `generate`（reindex 流程与暂存区）、`seed`（内置预置集）。

pub mod agent;
pub mod cli;
pub mod config;
pub mod docs;
pub mod embed;
pub mod generate;
pub mod guard;
pub mod index;
pub mod output;
pub mod process;
pub mod scan;
pub mod search;
pub mod seed;
pub mod types;
pub mod usage;

#[cfg(test)]
mod testutil;
