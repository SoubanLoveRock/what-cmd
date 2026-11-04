//! 命令行的参数定义与子命令执行。D5 只实现 `query`。
//!
//! stdout 只输出候选行，状态与错误写 stderr。退出码见 [`ExitKind`]。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};

use crate::config;
use crate::embed::UnavailableEmbedder;
use crate::index;
use crate::output::{format_json, format_line};
use crate::search::{self, QueryContext, QueryOptions, SearchError};
use crate::usage;

/// `whatcmd` 命令行。
#[derive(Debug, Parser)]
#[command(
    name = "whatcmd",
    about = "Find terminal commands by describing them in English"
)]
pub struct Cli {
    /// 配置文件路径，覆盖 `WHATCMD_CONFIG` 与默认路径。
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// 子命令。
    #[command(subcommand)]
    pub cmd: Command,
}

/// 子命令。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 锚点解析 + 向量匹配，输出候选行到 stdout。
    Query {
        /// 光标左侧的命令文本（LBUFFER），用于锚点解析。
        #[arg(long, default_value = "")]
        prefix: String,
        /// 最多输出的候选数。
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// 每个候选输出一个 JSON 对象，而不是 tab 分隔行。
        #[arg(long)]
        json: bool,
        /// 自然语言查询文本，放在 `--` 之后。
        #[arg(last = true)]
        text: Vec<String>,
    },
    /// 以 stdio 运行 MCP server，供 `claude -p` 在 reindex 期间调用。
    Mcp {
        /// 本次 run 的标识，决定暂存区文件名。
        #[arg(long)]
        run: String,
    },
}

/// 进程退出码，见 DESIGN 第 4 节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// 成功，有结果。
    Ok,
    /// 无结果。
    NoResult,
    /// 配置、索引、参数错误，或锚点前缀有误。
    Config,
    /// embedding 服务不可用。
    Embed,
}

impl ExitKind {
    /// 对应的进程退出码。
    pub fn code(self) -> i32 {
        match self {
            ExitKind::Ok => 0,
            ExitKind::NoResult => 1,
            ExitKind::Config => 2,
            ExitKind::Embed => 3,
        }
    }
}

/// 执行解析后的命令并返回退出码。
pub fn run(cli: Cli) -> ExitKind {
    match cli.cmd {
        Command::Query {
            prefix,
            limit,
            json,
            text,
        } => query(cli.config.as_deref(), &prefix, limit, json, &text.join(" ")),
        Command::Mcp { run } => mcp(cli.config.as_deref(), run),
    }
}

/// `mcp` 子命令：加载配置，以本次 run 的上下文启动 stdio MCP server。
fn mcp(config_path: Option<&Path>, run_id: String) -> ExitKind {
    let cfg = match config::load(config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("whatcmd: {err}");
            return ExitKind::Config;
        }
    };
    let ctx = crate::mcp::tools::ToolContext {
        run_id,
        staging_dir: cfg.paths.staging_dir.clone(),
        roots: crate::scan::current_path_commands(),
        // D10 接入索引后从本次 run 的命令树填充；此前只包含根命令。
        known_nodes: Vec::new(),
        max_depth: cfg.reindex.max_depth,
        docs: cfg.docs.clone(),
    };
    match crate::mcp::serve_stdio(ctx) {
        Ok(()) => ExitKind::Ok,
        Err(err) => {
            eprintln!("whatcmd: {err}");
            ExitKind::Config
        }
    }
}

/// `query` 子命令：加载配置、索引与使用记录，执行查询，输出候选。
fn query(
    config_path: Option<&Path>,
    prefix: &str,
    limit: usize,
    json: bool,
    text: &str,
) -> ExitKind {
    let cfg = match config::load(config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("whatcmd: {err}");
            return ExitKind::Config;
        }
    };
    let stored = match index::load(&cfg.paths.index_dir) {
        Ok(Some(stored)) => stored,
        Ok(None) => {
            eprintln!(
                "whatcmd: no index in {}; run `whatcmd reindex` first",
                cfg.paths.index_dir.display()
            );
            return ExitKind::Config;
        }
        Err(err) => {
            eprintln!("whatcmd: {err}");
            return ExitKind::Config;
        }
    };
    let history = match usage::load_snapshot(&cfg.paths.usage_file) {
        Ok(history) => history,
        Err(err) => {
            eprintln!("whatcmd: {err}");
            return ExitKind::Config;
        }
    };
    let embedder = UnavailableEmbedder::from_config(&cfg.embedding);
    let ctx = QueryContext {
        index: &stored.index,
        vectors: &stored.vectors,
        usage: &history,
        embedder: &embedder,
        now_unix: now_unix(),
    };
    match search::query(&ctx, prefix, text, &QueryOptions { limit }) {
        Ok(candidates) if candidates.is_empty() => ExitKind::NoResult,
        Ok(candidates) => {
            for candidate in &candidates {
                if json {
                    println!("{}", format_json(candidate));
                } else {
                    println!("{}", format_line(candidate));
                }
            }
            ExitKind::Ok
        }
        Err(SearchError::Embed(err)) => {
            eprintln!("whatcmd: {err}");
            ExitKind::Embed
        }
        Err(err) => {
            eprintln!("whatcmd: {err}");
            ExitKind::Config
        }
    }
}

/// 当前时间，Unix 秒。系统时钟早于 1970 时返回 0。
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use clap::Parser;

    use super::{Cli, Command, ExitKind, run};
    use crate::index::save_atomic;
    use crate::search::testing::fixture;
    use crate::testutil::tempdir;

    /// 在 `dir` 下写一份指向 `dir` 内路径的配置，返回配置路径。
    fn write_config(dir: &Path) -> PathBuf {
        let text = format!(
            "[paths]\nindex_dir = \"{d}/index\"\nusage_file = \"{d}/usage.jsonl\"\n\n\
             [embedding]\nendpoint = \"http://127.0.0.1:9/v1\"\nmodel = \"fake\"\ndimensions = 9\n",
            d = dir.display()
        );
        let path = dir.join("config.toml");
        fs::write(&path, text).unwrap();
        path
    }

    fn query_cli(config: PathBuf, text: &str) -> Cli {
        Cli {
            config: Some(config),
            cmd: Command::Query {
                prefix: String::new(),
                limit: 10,
                json: false,
                text: vec![text.to_owned()],
            },
        }
    }

    #[test]
    fn query_args_take_text_after_double_dash() {
        let cli = Cli::try_parse_from([
            "whatcmd", "query", "--prefix", "git ", "--limit", "5", "--", "pull", "romote",
        ])
        .unwrap();
        let Command::Query {
            prefix,
            limit,
            json,
            text,
        } = cli.cmd
        else {
            panic!("expected query subcommand");
        };
        assert_eq!(prefix, "git ");
        assert_eq!(limit, 5);
        assert!(!json);
        assert_eq!(text, ["pull", "romote"]);
    }

    #[test]
    fn double_dash_keeps_later_dashes_as_text() {
        let cli = Cli::try_parse_from([
            "whatcmd", "query", "--prefix", "git ", "--", "--branch", "-x",
        ])
        .unwrap();
        let Command::Query { text, .. } = cli.cmd else {
            panic!("expected query subcommand");
        };
        assert_eq!(text, ["--branch", "-x"]);
    }

    #[test]
    fn missing_index_exits_with_config_code() {
        let dir = tempdir();
        let config = write_config(dir.path());
        assert_eq!(run(query_cli(config, "ls")), ExitKind::Config);
    }

    #[test]
    fn unavailable_embedder_maps_to_embed_exit_code() {
        let dir = tempdir();
        let (index, vectors) = fixture();
        save_atomic(&dir.path().join("index"), &index, &vectors).unwrap();
        let config = write_config(dir.path());
        assert_eq!(run(query_cli(config, "ls")), ExitKind::Embed);
    }

    #[test]
    fn exit_codes_match_design() {
        assert_eq!(ExitKind::Ok.code(), 0);
        assert_eq!(ExitKind::NoResult.code(), 1);
        assert_eq!(ExitKind::Config.code(), 2);
        assert_eq!(ExitKind::Embed.code(), 3);
    }
}
