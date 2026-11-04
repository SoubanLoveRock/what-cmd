//! 文档获取：根命令的 man 页，以及子命令的 `--help` 输出。
//!
//! 子进程的超时与进程组清理由 [`crate::process`] 负责。这里只固定文档子进程的环境，
//! 并把输出清洗、截断。
//!
//! 可执行文件通过 [`crate::scan::find_in_path`] 在 PATH 中查找。`*_in` 版本接收
//! PATH 字符串，供测试放入假可执行文件。

use std::io;
use std::path::Path;
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::config::DocsConfig;
use crate::guard::name::validate_command;
use crate::guard::output::sanitize_text;
use crate::process::{ProcError, run_captured};
use crate::scan::find_in_path;
use crate::types::{CommandName, DocSource};

/// 截断后追加的标记。
const TRUNCATED_MARK: &str = "[truncated]";
/// debug 日志中原文的最大字节数。
const DEBUG_SNIPPET_BYTES: usize = 512;
/// 文档子进程的固定环境。
const DOC_ENV: [(&str, &str); 4] = [
    ("LC_ALL", "C"),
    ("MANPAGER", "cat"),
    ("PAGER", "cat"),
    ("MANWIDTH", "80"),
];

/// 一份已清洗的文档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Doc {
    /// 来源：man 页或 `--help`。
    pub source: DocSource,
    /// 已清洗、已截断的文本。
    pub text: String,
    /// 是否因超过 `max_doc_bytes` 被截断。
    pub truncated: bool,
}

/// 文档获取错误。
#[derive(Debug, thiserror::Error)]
pub enum DocError {
    /// 既没有可用的 man 页，也没有非空的 `--help` 输出。
    #[error("no man page or help output for {0}")]
    Missing(String),
    /// 子进程超过时限，进程组已被杀死。
    #[error("timed out after {0} ms")]
    Timeout(u64),
    /// 启动子进程等 IO 失败。
    #[error("io: {0}")]
    Io(#[from] io::Error),
}

/// 根命令的 man 页。PATH 中找不到 `man` 或该命令无 man 页时返回 `Missing`。
pub fn fetch_man(name: &CommandName, cfg: &DocsConfig) -> Result<Doc, DocError> {
    fetch_man_in(&current_path(), name, cfg)
}

/// 运行 `<path...> --help`。`path` 至少一个 token。只追加 `--help`，不接受其他参数。
pub fn fetch_help(path: &[CommandName], cfg: &DocsConfig) -> Result<Doc, DocError> {
    fetch_help_in(&current_path(), path, cfg)
}

/// 优先 man，失败后尝试 `--help`。子命令（多于一个 token）只尝试 `--help`，
/// 因为 man 页按根命令组织。
pub fn fetch(path: &[CommandName], cfg: &DocsConfig) -> Result<Doc, DocError> {
    fetch_in(&current_path(), path, cfg)
}

/// 清洗原始字节：去除 ANSI 转义与退格，统一换行，再按字节截断。
///
/// 截断在字符边界处进行，之后追加 `\n[truncated]`，因此结果可能比 `max_bytes` 多出标记的长度。
/// 返回 `(文本, 是否截断)`。
pub fn clean(raw: &[u8], max_bytes: usize) -> (String, bool) {
    let text = sanitize_text(&String::from_utf8_lossy(raw));
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_owned();
    out.push('\n');
    out.push_str(TRUNCATED_MARK);
    (out, true)
}

/// 以 PATH 字符串为参数的 man 获取。
fn fetch_man_in(path_var: &str, name: &CommandName, cfg: &DocsConfig) -> Result<Doc, DocError> {
    let missing = || DocError::Missing(name.as_str().to_owned());
    let man_name = validate_command("man").map_err(|_| missing())?;
    let man = find_in_path(path_var, &man_name).ok_or_else(missing)?;
    let timeout = Duration::from_millis(cfg.man_timeout_ms);
    let raw = capture(&man, &[name.as_str()], timeout, cfg.max_doc_bytes)?.ok_or_else(missing)?;
    Ok(doc_from(&raw, DocSource::Man, cfg))
}

/// 以 PATH 字符串为参数的 `--help` 获取。
fn fetch_help_in(path_var: &str, path: &[CommandName], cfg: &DocsConfig) -> Result<Doc, DocError> {
    let Some((root, rest)) = path.split_first() else {
        return Err(DocError::Missing("empty command path".to_owned()));
    };
    let joined = path
        .iter()
        .map(CommandName::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let missing = || DocError::Missing(joined.clone());
    let bin = find_in_path(path_var, root).ok_or_else(missing)?;
    let mut args: Vec<&str> = rest.iter().map(CommandName::as_str).collect();
    args.push("--help");
    let timeout = Duration::from_millis(cfg.help_timeout_ms);
    let raw = capture(&bin, &args, timeout, cfg.max_doc_bytes)?.ok_or_else(missing)?;
    Ok(doc_from(&raw, DocSource::Help, cfg))
}

/// 以 PATH 字符串为参数的优先级获取：单个根命令先 man，失败后 `--help`。
fn fetch_in(path_var: &str, path: &[CommandName], cfg: &DocsConfig) -> Result<Doc, DocError> {
    if let [root] = path {
        match fetch_man_in(path_var, root, cfg) {
            Ok(doc) => {
                info!(name = root.as_str(), source = "man", "doc fetched");
                return Ok(doc);
            }
            Err(err) => {
                warn!(name = root.as_str(), error = %err, "man unavailable, falling back to --help");
            }
        }
    }
    let doc = fetch_help_in(path_var, path, cfg)?;
    info!(name = path[0].as_str(), source = "help", "doc fetched");
    Ok(doc)
}

/// 清洗原始输出并记录 debug 日志。原文只在 debug 级别出现，且截断。
fn doc_from(raw: &[u8], source: DocSource, cfg: &DocsConfig) -> Doc {
    let snippet_len = raw.len().min(DEBUG_SNIPPET_BYTES);
    debug!(
        source = ?source,
        bytes = raw.len(),
        snippet = %String::from_utf8_lossy(&raw[..snippet_len]),
        "raw doc output"
    );
    let (text, truncated) = clean(raw, cfg.max_doc_bytes);
    Doc {
        source,
        text,
        truncated,
    }
}

/// 以文档环境运行子进程，错误映射为 [`DocError`]。
fn capture(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Option<Vec<u8>>, DocError> {
    run_captured(program, args, &DOC_ENV, timeout, max_bytes).map_err(|err| match err {
        ProcError::Timeout(ms) => DocError::Timeout(ms),
        ProcError::Io(err) => DocError::Io(err),
    })
}

/// 当前进程的 PATH，未设置时为空串。
fn current_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use super::{DocError, TRUNCATED_MARK, clean, fetch_help_in, fetch_in, fetch_man_in};
    use crate::config::DocsConfig;
    use crate::guard::name::validate_command;
    use crate::testutil::tempdir;
    use crate::types::{CommandName, DocSource};

    /// 在 `dir` 下写一个假可执行文件。写完并落盘后再关闭，避免刚写完就执行时内容未就绪。
    fn fake_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        {
            let mut file = fs::File::create(&path).unwrap();
            file.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
                .unwrap();
            file.sync_all().unwrap();
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn name(raw: &str) -> CommandName {
        validate_command(raw).unwrap()
    }

    fn path_of(raw: &[&str]) -> Vec<CommandName> {
        raw.iter().map(|t| name(t)).collect()
    }

    /// 这些测试不检查时序，超时放宽，避免机器负载下假失败。时序由 `timeout_kills_the_whole_process_group` 单独验证。
    fn cfg() -> DocsConfig {
        DocsConfig {
            man_timeout_ms: 10_000,
            help_timeout_ms: 10_000,
            ..DocsConfig::default()
        }
    }

    #[test]
    fn clean_strips_ansi_and_backspace_overstrike() {
        // man 粗体：字符 退格 字符，外加 CSI 颜色序列。
        let raw = b"\x1b[1mB\x08Bold\x1b[0m";
        assert_eq!(clean(raw, 1024), ("Bold".to_owned(), false));
    }

    #[test]
    fn clean_truncates_at_char_boundary_and_marks() {
        // "中" 占 3 字节，上限 4 字节时只能保留一个完整字符。
        let (text, truncated) = clean("中文中文".as_bytes(), 4);
        assert!(truncated);
        assert!(text.starts_with("中\n"));
        assert!(text.ends_with(TRUNCATED_MARK));
    }

    #[test]
    fn man_page_runs_with_fixed_environment() {
        let dir = tempdir();
        let bin = dir.path().join("bin");
        fake_bin(
            &bin,
            "man",
            r#"echo "GIT PAGE $MANWIDTH $MANPAGER $LC_ALL $1""#,
        );
        let doc = fetch_man_in(&bin.display().to_string(), &name("git"), &cfg()).unwrap();
        assert_eq!(doc.source, DocSource::Man);
        assert!(doc.text.contains("GIT PAGE 80 cat C git"));
    }

    #[test]
    fn missing_man_binary_is_missing() {
        let dir = tempdir();
        let err = fetch_man_in(&dir.path().display().to_string(), &name("git"), &cfg());
        assert!(matches!(err, Err(DocError::Missing(_))));
    }

    #[test]
    fn help_for_subcommand_passes_path_then_flag() {
        let dir = tempdir();
        let bin = dir.path().join("bin");
        fake_bin(&bin, "foo", r#"echo "args: $*""#);
        let doc = fetch_help_in(
            &bin.display().to_string(),
            &path_of(&["foo", "clone"]),
            &cfg(),
        )
        .unwrap();
        assert_eq!(doc.source, DocSource::Help);
        assert!(doc.text.contains("args: clone --help"));
    }

    #[test]
    fn fetch_falls_back_to_help_when_man_missing() {
        let dir = tempdir();
        let bin = dir.path().join("bin");
        fake_bin(&bin, "foo", r#"echo "usage: $*""#);
        let doc = fetch_in(&bin.display().to_string(), &path_of(&["foo"]), &cfg()).unwrap();
        assert_eq!(doc.source, DocSource::Help);
        assert!(doc.text.contains("usage: --help"));
    }

    #[test]
    fn help_for_missing_binary_is_missing() {
        let dir = tempdir();
        let err = fetch_help_in(
            &dir.path().display().to_string(),
            &path_of(&["nope"]),
            &cfg(),
        );
        assert!(matches!(err, Err(DocError::Missing(_))));
    }

    #[test]
    fn empty_output_is_missing() {
        let dir = tempdir();
        let bin = dir.path().join("bin");
        fake_bin(&bin, "quiet", "exit 0");
        let err = fetch_help_in(&bin.display().to_string(), &path_of(&["quiet"]), &cfg());
        assert!(matches!(err, Err(DocError::Missing(_))));
    }

    /// `pgrep <args>` 是否找到进程。退出码 0 为找到，1 为没有。
    fn pgrep_finds(args: &[&str]) -> bool {
        Command::new("pgrep")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// 在 `deadline` 前等待 `pgrep <args>` 找不到进程，返回最终是否已清空。
    fn wait_gone(args: &[&str], deadline: Instant) -> bool {
        while pgrep_finds(args) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    #[test]
    fn timeout_kills_the_whole_process_group() {
        let dir = tempdir();
        let bin = dir.path().join("bin");
        let pgid_file = dir.path().join("slow.pgid");
        // 后台任务通过 `"$0" bg` 重新执行脚本自身，因此前台与后台的长驻进程命令行都含脚本路径，
        // 可以用 `pgrep -f` 找到，不依赖脚本在超时前写出任何文件。
        // 两者都只 `sleep 1` 循环，不含路径的 sleep 子进程最多存活 1 秒。
        fake_bin(
            &bin,
            "slow",
            &format!(
                "if [ \"$1\" = bg ]; then while :; do sleep 1; done; fi\n\
                 echo $$ > '{}'\n\
                 \"$0\" bg &\n\
                 while :; do sleep 1; done",
                pgid_file.display()
            ),
        );
        let script = bin.join("slow").display().to_string();
        let cfg = DocsConfig {
            help_timeout_ms: 3000,
            ..DocsConfig::default()
        };
        let started = Instant::now();
        let err = fetch_help_in(&bin.display().to_string(), &path_of(&["slow"]), &cfg);
        assert!(matches!(err, Err(DocError::Timeout(3000))));
        assert!(started.elapsed() < Duration::from_secs(5));

        let deadline = Instant::now() + Duration::from_secs(3);
        // 由脚本启动的长驻进程（前台 sh 与后台 sh）都必须消失。即使脚本在超时前没执行到第一行，
        // 被 spawn 的 sh 本身也带着脚本路径，这一条仍然有效。
        assert!(
            wait_gone(&["-f", "--", &script], deadline),
            "process running {script} survived group kill"
        );
        // 脚本写出了自身 pid（即 pgid）时，进一步确认整个进程组为空。
        if let Ok(raw) = fs::read_to_string(&pgid_file) {
            let pgid = raw.trim().to_owned();
            assert!(
                wait_gone(&["-g", &pgid], deadline),
                "process group {pgid} not empty after kill"
            );
        }
    }
}
