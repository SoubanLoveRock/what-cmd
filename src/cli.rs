//! 命令行的参数定义与子命令执行。D5 只实现 `query`。
//!
//! stdout 只输出候选行，状态与错误写 stderr。退出码见 [`ExitKind`]。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};

use crate::agent::claude::ClaudeBackend;
use crate::agent::{detect, missing_capabilities};
use crate::config;
use crate::docs;
use crate::embed::UnavailableEmbedder;
use crate::embed::openai::OpenAiEmbedder;
use crate::generate::merge::Stamp;
use crate::generate::staging::{self, StagedRecord};
use crate::generate::{self, Inputs, ReindexOptions};
use crate::guard::name::validate_command;
use crate::index;
use crate::output::{format_json, format_line};
use crate::process::run_captured;
use crate::scan::{self, find_in_path};
use crate::search::{self, QueryContext, QueryOptions, SearchError};
use crate::types::{CommandName, NodeKind};
use crate::usage;

/// `claude --help` 探测的超时（DESIGN 第 9.8 节）。
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// `claude --help` 输出的读取上限（字节）。
const PROBE_OUTPUT_BYTES: usize = 64 * 1024;
/// 单批 agent 调用的超时。
const AGENT_TIMEOUT: Duration = Duration::from_secs(600);

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
    /// 扫描 PATH，生成描述与参数，写索引。
    Reindex {
        /// 忽略旧 `source_hash`，非预置节点全部重跑。
        #[arg(long)]
        full: bool,
        /// 只重跑列出的节点，可含预置节点，逗号分隔。
        #[arg(long, value_delimiter = ',')]
        names: Vec<String>,
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
        Command::Reindex { full, names } => reindex(cli.config.as_deref(), full, names),
    }
}

/// `reindex` 子命令：探测 `claude`，建立 embedding 客户端，生成描述并写索引。
///
/// `--auto` 与 PATH 指纹尚未实现。
fn reindex(config_path: Option<&Path>, full: bool, names: Vec<String>) -> ExitKind {
    let cfg = match config::load(config_path) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("whatcmd: {err}");
            return ExitKind::Config;
        }
    };
    let backend = match claude_backend(&cfg) {
        Ok(backend) => backend,
        Err(msg) => {
            eprintln!("whatcmd: {msg}");
            return ExitKind::Config;
        }
    };
    let embedder = match OpenAiEmbedder::from_config(&cfg.embedding) {
        Ok(embedder) => embedder,
        Err(err) => {
            eprintln!("whatcmd: {err}");
            return ExitKind::Embed;
        }
    };
    let run_id = new_run_id();
    let roots = scan::current_path_commands();
    let stamp = Stamp {
        built_at: rfc3339_utc(now_unix()),
        generator: index::GeneratorMeta {
            backend: "claude-code".to_owned(),
            agent_version: agent_version(&backend.bin),
            prompt_version: generate::PROMPT_VERSION,
        },
    };
    let fetch = |path: &[CommandName]| {
        docs::fetch(path, &cfg.docs)
            .ok()
            .map(|doc| (doc.source, doc.text))
    };
    let log_path = reindex_log_path(&cfg);
    let opts = ReindexOptions {
        full,
        names: (!names.is_empty()).then_some(names),
    };
    let result = generate::reindex(&Inputs {
        cfg: &cfg,
        backend: &backend,
        embedder: &embedder,
        roots: &roots,
        fetch: &fetch,
        opts: &opts,
        run_id: &run_id,
        stamp: &stamp,
        log: log_path.as_deref(),
    });
    match result {
        Ok(report) => {
            eprintln!(
                "whatcmd: generated {}, reused {}, removed {}, failed {}",
                report.generated,
                report.reused,
                report.removed,
                report.failed.len()
            );
            for (key, reason) in &report.failed {
                eprintln!("whatcmd: failed {key}: {reason}");
            }
            if report.failed.is_empty() {
                ExitKind::Ok
            } else {
                ExitKind::NoResult
            }
        }
        Err(err) => {
            eprintln!("whatcmd: {err}");
            ExitKind::Config
        }
    }
}

/// 在 PATH 中找 `claude`，运行 `--help` 探测能力。缺少 `reindex` 所需能力时拒绝运行。
fn claude_backend(cfg: &config::Config) -> Result<ClaudeBackend, String> {
    if cfg.agent.backend == config::BackendChoice::None {
        return Err("agent.backend is \"none\"; reindex needs an agent".to_owned());
    }
    let name = validate_command("claude").map_err(|err| err.to_string())?;
    let path_var = std::env::var("PATH").unwrap_or_default();
    let bin = find_in_path(&path_var, &name).ok_or("claude not found in PATH")?;
    let help = run_captured(&bin, &["--help"], &[], PROBE_TIMEOUT, PROBE_OUTPUT_BYTES)
        .map_err(|err| err.to_string())?
        .ok_or("claude --help produced no output")?;
    let capabilities = detect::parse_capabilities(&String::from_utf8_lossy(&help));
    let missing = missing_capabilities(&capabilities);
    if !missing.is_empty() {
        return Err(format!(
            "claude is missing required capabilities: {missing:?}"
        ));
    }
    Ok(ClaudeBackend {
        bin,
        capabilities,
        timeout: AGENT_TIMEOUT,
    })
}

/// `reindex` 与 MCP 子进程共用的日志路径：缓存目录（`usage_file` 的父目录）下的 `reindex.log`，与 DESIGN 第 9.1 节一致。
fn reindex_log_path(cfg: &config::Config) -> Option<PathBuf> {
    cfg.paths
        .usage_file
        .parent()
        .map(|dir| dir.join("reindex.log"))
}

/// 运行 `claude --version`，取第一行作为版本号。失败或输出为空时返回 `unknown`。
fn agent_version(bin: &Path) -> String {
    run_captured(bin, &["--version"], &[], PROBE_TIMEOUT, PROBE_OUTPUT_BYTES)
        .ok()
        .flatten()
        .and_then(|out| {
            String::from_utf8_lossy(&out)
                .lines()
                .next()
                .map(|line| line.trim().to_owned())
        })
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// 本次 run 的标识：进程号加纳秒时间戳，只含字母、数字与 `-`。
fn new_run_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("reindex-{}-{nanos}", std::process::id())
}

/// Unix 秒转 RFC 3339（UTC，秒精度）。公历换算采用 Howard Hinnant 的 `civil_from_days`。
fn rfc3339_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
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
    let roots = crate::scan::current_path_commands();
    let root_names: Vec<String> = roots
        .iter()
        .map(|entry| entry.name.as_str().to_owned())
        .collect();
    let known = match known_nodes(
        &cfg.paths.index_dir,
        &cfg.paths.staging_dir,
        &run_id,
        &root_names,
    ) {
        Ok(known) => known,
        Err(msg) => {
            eprintln!("whatcmd: {msg}");
            return ExitKind::Config;
        }
    };
    let ctx = crate::mcp::tools::ToolContext {
        run_id,
        staging_dir: cfg.paths.staging_dir.clone(),
        roots,
        known_nodes: known,
        max_depth: cfg.reindex.max_depth,
        docs: cfg.docs.clone(),
        log: reindex_log_path(&cfg),
        doc_cache: Default::default(),
    };
    match crate::mcp::serve_stdio(ctx) {
        Ok(()) => ExitKind::Ok,
        Err(err) => {
            eprintln!("whatcmd: {err}");
            ExitKind::Config
        }
    }
}

/// 本次 run 可见的节点 key：本次扫描到的根命令，加上索引中的命令节点，再加上本次 run 暂存区里声明过的子命令。
///
/// 根命令必须包含在内：它是子命令的父节点，索引为空（如刚删除旧索引）时只能由扫描结果提供。
/// 子命令来自 `write_description` 的 `subcommands`，即下一轮要处理的节点，因此允许其子节点写入。
/// 索引缺失时只含根命令与暂存区的部分。
fn known_nodes(
    index_dir: &Path,
    staging_dir: &Path,
    run_id: &str,
    roots: &[String],
) -> Result<Vec<String>, String> {
    let mut keys: BTreeSet<String> = roots.iter().cloned().collect();
    if let Some(stored) = index::load(index_dir).map_err(|err| err.to_string())? {
        keys.extend(
            stored
                .index
                .nodes
                .iter()
                .filter(|node| node.kind == NodeKind::Command)
                .map(|node| node.key.clone()),
        );
    }
    let records = staging::read_all(staging_dir, run_id).map_err(|err| err.to_string())?;
    for rec in records {
        if let StagedRecord::Command {
            path, subcommands, ..
        } = rec
        {
            for sub in subcommands {
                keys.insert(format!("{} {sub}", path.join(" ")));
            }
        }
    }
    Ok(keys.into_iter().collect())
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

    use super::{Cli, Command, ExitKind, known_nodes, rfc3339_utc, run};
    use crate::generate::staging::{self, StagedRecord};
    use crate::index::save_atomic;
    use crate::search::testing::fixture;
    use crate::testutil::tempdir;

    #[test]
    fn known_nodes_include_subcommands_staged_in_this_run() {
        let dir = tempdir();
        let staging_dir = dir.path().join("staging");
        staging::append(
            &staging_dir,
            "run-k",
            &StagedRecord::Command {
                path: vec!["git".to_owned()],
                description: "d".to_owned(),
                examples: Vec::new(),
                subcommands: vec!["clone".to_owned()],
            },
        )
        .unwrap();
        let keys = known_nodes(
            &dir.path().join("index"),
            &staging_dir,
            "run-k",
            &["git".to_owned()],
        )
        .unwrap();
        assert_eq!(keys, ["git", "git clone"]);
    }

    #[test]
    fn known_nodes_include_scanned_roots_when_index_is_missing() {
        let dir = tempdir();
        let keys = known_nodes(
            &dir.path().join("index"),
            &dir.path().join("staging"),
            "run-r",
            &["git".to_owned()],
        )
        .unwrap();
        assert_eq!(keys, ["git"]);
    }

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
    fn rfc3339_formats_known_instants() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_000_000_000), "2001-09-09T01:46:40Z");
    }

    #[test]
    fn exit_codes_match_design() {
        assert_eq!(ExitKind::Ok.code(), 0);
        assert_eq!(ExitKind::NoResult.code(), 1);
        assert_eq!(ExitKind::Config.code(), 2);
        assert_eq!(ExitKind::Embed.code(), 3);
    }
}
