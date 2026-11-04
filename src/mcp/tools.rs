//! MCP 工具的纯函数实现。每个函数先经过 `guard` 校验，再调用文档获取或写入暂存区。
//!
//! 工具只写暂存区，不改索引。`path` 必须能在本次 run 的命令树中找到父节点，
//! 不接受任意路径。rmcp 的注册只做参数转发，见 `mcp` 模块。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use tracing::warn;

use crate::config::DocsConfig;
use crate::docs::{self, DocError};
use crate::generate::staging::{self, StagedParam, StagedRecord, StagingError};
use crate::guard::GuardError;
use crate::guard::name::{validate_command, validate_flag, validate_path};
use crate::guard::output::{validate_output, validate_param_description};
use crate::types::{CommandEntry, CommandName};

/// 工具错误。
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// 命令不在本次扫描结果中。
    #[error("unknown command {0}: not found in this run's PATH scan")]
    UnknownCommand(String),
    /// 命令在扫描结果中，但父节点或节点本身不在本次命令树中。
    #[error("{0} is not in this run's command tree; describe its parent first")]
    NotInTree(String),
    /// 路径超过 `max_depth`。
    #[error("path exceeds max_depth {0}")]
    TooDeep(usize),
    /// 输入未通过 `guard` 校验。
    #[error(transparent)]
    Guard(#[from] GuardError),
    /// 文档获取失败。
    #[error(transparent)]
    Doc(#[from] DocError),
    /// 暂存区写入失败。
    #[error(transparent)]
    Staging(#[from] StagingError),
}

/// 一次 run 的工具上下文。
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// 本次 run 的标识，决定暂存区文件名。
    pub run_id: String,
    /// 暂存区目录。
    pub staging_dir: PathBuf,
    /// run 开始时扫描得到的根命令快照。
    pub roots: Vec<CommandEntry>,
    /// 本次 run 可见的节点 key（完整路径，空格分隔），根命令也应在其中。
    pub known_nodes: Vec<String>,
    /// 命令路径的最大深度。
    pub max_depth: usize,
    /// 文档获取配置。
    pub docs: DocsConfig,
    /// 日志文件。工具拒绝输入时追加一行，`None` 时不写。
    pub log: Option<PathBuf>,
    /// 本次 run 内已获取的文档全文，键为 `man <key>` 或 `help <key>`。分页读取时复用，避免每页都执行一次 man 或 --help。
    pub doc_cache: Arc<Mutex<HashMap<String, String>>>,
}

/// `write_description` 的参数。
#[derive(Debug, Clone, Deserialize)]
pub struct WriteDescriptionArgs {
    /// 命令路径，如 `["git", "clone"]`。
    pub path: Vec<String>,
    /// 描述文本。
    pub description: String,
    /// 示例，最多 3 条。
    pub examples: Vec<String>,
    /// 下一轮要处理的子命令名，每项须通过命名校验。
    pub subcommands: Vec<String>,
}

/// `write_params` 的参数。
#[derive(Debug, Clone, Deserialize)]
pub struct WriteParamsArgs {
    /// 所属命令路径。
    pub path: Vec<String>,
    /// 参数列表。
    pub params: Vec<ParamArg>,
}

/// 单个参数的输入。
#[derive(Debug, Clone, Deserialize)]
pub struct ParamArg {
    /// 参数名，必须是 `--` 长参数。
    pub flag: String,
    /// 参数描述。
    pub description: String,
}

/// 读取根命令 man 页的一页。命令必须在本次扫描结果中。`offset` 为字节偏移，首次读取传 0。
pub fn read_man(name: &str, offset: usize, ctx: &ToolContext) -> Result<String, ToolError> {
    let name = validate_command(name)?;
    if !ctx.roots.iter().any(|entry| entry.name == name) {
        return Err(ToolError::UnknownCommand(name.as_str().to_owned()));
    }
    let text = cached(ctx, format!("man {}", name.as_str()), || {
        docs::fetch_man(&name, &ctx.docs).map(|doc| doc.text)
    })?;
    Ok(page(&text, offset))
}

/// 读取子命令 `--help` 输出的一页。父节点必须在命令树中。`offset` 为字节偏移，首次读取传 0。
pub fn read_help(path: &[&str], offset: usize, ctx: &ToolContext) -> Result<String, ToolError> {
    let names = validate_path(path)?;
    ensure_depth(names.len(), ctx)?;
    ensure_parent_visible(&names, ctx)?;
    let text = cached(ctx, format!("help {}", key_of(&names)), || {
        docs::fetch_help(&names, &ctx.docs).map(|doc| doc.text)
    })?;
    Ok(page(&text, offset))
}

/// 每页最多的字节数。
pub const PAGE_BYTES: usize = 8192;

/// 返回 `text` 从 `offset` 开始的一页。末尾附一行标记：还有后续时给出下一页的 offset，否则标明已到末尾。
///
/// `offset` 落在字符中间时向后对齐到字符边界，结束位置向前对齐，保证不切断多字节字符。
pub fn page(text: &str, offset: usize) -> String {
    let len = text.len();
    let mut start = offset.min(len);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + PAGE_BYTES).min(len);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let body = &text[start..end];
    if end < len {
        format!("{body}\n[more: {len} bytes total; call again with offset {end} for the next part]")
    } else {
        format!("{body}\n[end of document: {len} bytes total]")
    }
}

/// 从本次 run 的缓存取文档全文，未命中时调用 `fetch` 并写入缓存。失败不缓存。
fn cached<F>(ctx: &ToolContext, key: String, fetch: F) -> Result<String, ToolError>
where
    F: FnOnce() -> Result<String, DocError>,
{
    let mut cache = ctx
        .doc_cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(text) = cache.get(&key) {
        return Ok(text.clone());
    }
    let text = fetch()?;
    cache.insert(key, text.clone());
    Ok(text)
}

/// 写入命令节点的描述、示例与子命令到暂存区。
pub fn write_description(args: WriteDescriptionArgs, ctx: &ToolContext) -> Result<(), ToolError> {
    let names = validate_path(&tokens(&args.path))?;
    ensure_depth(names.len(), ctx)?;
    ensure_parent_visible(&names, ctx)?;
    let checked = validate_output(&args.description, &args.examples)?;
    let subcommands = args
        .subcommands
        .iter()
        .map(|sub| validate_command(sub).map(|c| c.as_str().to_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    let record = StagedRecord::Command {
        path: owned(&names),
        description: checked.description,
        examples: checked.examples,
        subcommands,
    };
    staging::append(&ctx.staging_dir, &ctx.run_id, &record)?;
    Ok(())
}

/// 写入命令下长参数的描述到暂存区。短参数被丢弃并记 `warn`。
pub fn write_params(args: WriteParamsArgs, ctx: &ToolContext) -> Result<(), ToolError> {
    let names = validate_path(&tokens(&args.path))?;
    ensure_depth(names.len(), ctx)?;
    ensure_node_visible(&names, ctx)?;
    let mut params = Vec::with_capacity(args.params.len());
    for param in &args.params {
        match validate_flag(&param.flag) {
            Ok(flag) => params.push(StagedParam {
                flag,
                description: validate_param_description(&param.description)?,
            }),
            Err(GuardError::ShortFlag(_)) => {
                warn!(name = %key_of(&names), "short flag dropped");
            }
            Err(err) => return Err(err.into()),
        }
    }
    let record = StagedRecord::Params {
        path: owned(&names),
        params,
    };
    staging::append(&ctx.staging_dir, &ctx.run_id, &record)?;
    Ok(())
}

/// 深度检查。
fn ensure_depth(depth: usize, ctx: &ToolContext) -> Result<(), ToolError> {
    if depth > ctx.max_depth {
        return Err(ToolError::TooDeep(ctx.max_depth));
    }
    Ok(())
}

/// 根命令必须在扫描结果中；多于一个 token 时，父节点必须在命令树中可见。
fn ensure_parent_visible(names: &[CommandName], ctx: &ToolContext) -> Result<(), ToolError> {
    let root = names[0].as_str();
    if !ctx.roots.iter().any(|entry| entry.name.as_str() == root) {
        return Err(ToolError::UnknownCommand(root.to_owned()));
    }
    if names.len() > 1 {
        let parent = key_of(&names[..names.len() - 1]);
        if !ctx.known_nodes.contains(&parent) {
            return Err(ToolError::NotInTree(parent));
        }
    }
    Ok(())
}

/// 父节点可见，且节点自身在命令树中可见。用于写入参数。
fn ensure_node_visible(names: &[CommandName], ctx: &ToolContext) -> Result<(), ToolError> {
    ensure_parent_visible(names, ctx)?;
    let key = key_of(names);
    if names.len() > 1 && !ctx.known_nodes.contains(&key) {
        return Err(ToolError::NotInTree(key));
    }
    Ok(())
}

/// 完整路径的 key，空格分隔。
fn key_of(names: &[CommandName]) -> String {
    names
        .iter()
        .map(CommandName::as_str)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 已校验路径转为暂存区使用的 `String` 列表。
fn owned(names: &[CommandName]) -> Vec<String> {
    names.iter().map(|n| n.as_str().to_owned()).collect()
}

/// 输入路径转为 `&str` 切片，供 `validate_path` 使用。
fn tokens(path: &[String]) -> Vec<&str> {
    path.iter().map(String::as_str).collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::guard::name::validate_command;
    use crate::testutil::tempdir;

    fn ctx(staging: &Path) -> ToolContext {
        ToolContext {
            run_id: "run-1".to_owned(),
            staging_dir: staging.to_path_buf(),
            roots: vec![CommandEntry {
                name: validate_command("git").unwrap(),
                path: PathBuf::from("/usr/bin/git"),
            }],
            known_nodes: vec!["git".to_owned(), "git clone".to_owned()],
            max_depth: 3,
            docs: DocsConfig::default(),
            log: None,
            doc_cache: Default::default(),
        }
    }

    fn staged(dir: &Path) -> String {
        fs::read_to_string(dir.join("run-1.jsonl")).unwrap_or_default()
    }

    fn desc_args(path: &[&str], description: &str, examples: &[&str]) -> WriteDescriptionArgs {
        WriteDescriptionArgs {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            description: description.to_owned(),
            examples: examples.iter().map(|s| (*s).to_owned()).collect(),
            subcommands: Vec::new(),
        }
    }

    fn params_args(path: &[&str], params: &[(&str, &str)]) -> WriteParamsArgs {
        WriteParamsArgs {
            path: path.iter().map(|s| (*s).to_owned()).collect(),
            params: params
                .iter()
                .map(|(flag, description)| ParamArg {
                    flag: (*flag).to_owned(),
                    description: (*description).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn page_splits_long_text_and_reports_next_offset() {
        let text = "a".repeat(PAGE_BYTES + 10);
        let first = page(&text, 0);
        assert!(first.ends_with(&format!(
            "[more: {} bytes total; call again with offset {PAGE_BYTES} for the next part]",
            PAGE_BYTES + 10
        )));
        let second = page(&text, PAGE_BYTES);
        assert_eq!(
            second,
            "a".repeat(10) + "\n[end of document: 8202 bytes total]"
        );
    }

    #[test]
    fn page_never_splits_a_multibyte_character() {
        // "é" 占 2 字节，偏移 1 落在字符中间，应向后对齐到 2。
        let text = "é".repeat(5000);
        let got = page(&text, 1);
        assert!(got.starts_with('é'));
        assert!(got.contains("[more:"));
    }

    #[test]
    fn page_past_end_is_end_marker_only() {
        assert_eq!(page("abc", 99), "\n[end of document: 3 bytes total]");
    }

    #[test]
    fn read_man_rejects_unknown_and_invalid_names_before_docs() {
        let dir = tempdir();
        let c = ctx(dir.path());
        assert!(matches!(
            read_man("nope", 0, &c),
            Err(ToolError::UnknownCommand(_))
        ));
        assert!(matches!(
            read_man("-x", 0, &c),
            Err(ToolError::Guard(GuardError::InvalidName(_)))
        ));
    }

    #[test]
    fn read_help_enforces_max_depth() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let err = read_help(&["git", "clone", "a", "b"], 0, &c).unwrap_err();
        assert!(matches!(err, ToolError::TooDeep(3)));
    }

    #[test]
    fn read_help_requires_visible_parent() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let err = read_help(&["git", "pull", "x"], 0, &c).unwrap_err();
        assert!(matches!(err, ToolError::NotInTree(ref k) if k == "git pull"));
    }

    #[test]
    fn description_rejects_too_many_examples() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let args = desc_args(&["git", "clone"], "d", &["a", "b", "c", "d"]);
        assert!(matches!(
            write_description(args, &c),
            Err(ToolError::Guard(GuardError::ExampleLimit))
        ));
        assert!(
            staged(dir.path()).is_empty(),
            "rejected input must not be staged"
        );
    }

    #[test]
    fn description_rejects_overlong_text() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let long = "a".repeat(401);
        let args = desc_args(&["git"], &long, &[]);
        assert!(matches!(
            write_description(args, &c),
            Err(ToolError::Guard(GuardError::DescriptionTooLong(401)))
        ));
    }

    #[test]
    fn description_rejects_bad_subcommand_name() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let mut args = desc_args(&["git"], "d", &[]);
        args.subcommands = vec!["clone x".to_owned()];
        assert!(matches!(
            write_description(args, &c),
            Err(ToolError::Guard(GuardError::InvalidName(_)))
        ));
    }

    #[test]
    fn description_is_staged_as_one_record() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let mut args = desc_args(&["git", "clone"], "Clone a repo", &["git clone URL"]);
        args.subcommands = vec!["shallow".to_owned()];
        write_description(args, &c).unwrap();
        let rec: StagedRecord = serde_json::from_str(staged(dir.path()).trim()).unwrap();
        assert_eq!(
            rec,
            StagedRecord::Command {
                path: vec!["git".to_owned(), "clone".to_owned()],
                description: "Clone a repo".to_owned(),
                examples: vec!["git clone URL".to_owned()],
                subcommands: vec!["shallow".to_owned()],
            }
        );
    }

    #[test]
    fn short_flags_are_dropped_and_long_flags_kept() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let args = params_args(
            &["git", "clone"],
            &[("-v", "verbose"), ("--branch", "Branch name")],
        );
        write_params(args, &c).unwrap();
        let rec: StagedRecord = serde_json::from_str(staged(dir.path()).trim()).unwrap();
        assert_eq!(
            rec,
            StagedRecord::Params {
                path: vec!["git".to_owned(), "clone".to_owned()],
                params: vec![StagedParam {
                    flag: "--branch".to_owned(),
                    description: "Branch name".to_owned(),
                }],
            }
        );
    }

    #[test]
    fn params_reject_non_flag_tokens() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let args = params_args(&["git", "clone"], &[("branch", "x")]);
        assert!(matches!(
            write_params(args, &c),
            Err(ToolError::Guard(GuardError::InvalidFlag(_)))
        ));
    }

    #[test]
    fn params_require_the_node_itself_to_be_visible() {
        let dir = tempdir();
        let c = ctx(dir.path());
        let args = params_args(&["git", "pull"], &[("--rebase", "x")]);
        assert!(matches!(
            write_params(args, &c),
            Err(ToolError::NotInTree(ref k)) if k == "git pull"
        ));
    }

    #[test]
    fn invalid_run_id_surfaces_as_staging_error() {
        let dir = tempdir();
        let mut c = ctx(dir.path());
        c.run_id = "../x".to_owned();
        let args = params_args(&["git"], &[]);
        assert!(matches!(
            write_params(args, &c),
            Err(ToolError::Staging(StagingError::InvalidRunId(_)))
        ));
    }
}
