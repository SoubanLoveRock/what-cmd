//! Claude Code 适配：命令行参数拼装与输出解析。
//!
//! reindex 不给模型任何工具（`--tools ""`），文档由程序放进提示词，输出结构由 `--json-schema` 约束。
//! [`ClaudeBackend`] 通过 [`crate::process::run_finished`] 启动 `claude`，与 `docs` 共用超时与
//! 进程组清理逻辑。输出解析见 [`parse_output`]。

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::agent::{
    AgentBackend, AgentError, BatchOutcome, BatchRequest, CapabilitySet, missing_capabilities,
};
use crate::process::{ProcError, run_finished};

/// 子进程输出的读取上限（字节）。超出部分读后丢弃。
const OUTPUT_LIMIT_BYTES: usize = 4 * 1024 * 1024;

/// 默认模型别名，交给 `claude` 解析为当前 Haiku 档模型。固定取值，不接受外部输入。
pub const CLAUDE_MODEL: &str = "haiku";

/// 纯函数：拼装 `claude` 的命令行参数，不含程序名。
///
/// 内置工具通过 `--tools ""` 全部禁用，且不传任何 MCP 参数。输出结构由 `--json-schema` 约束。
pub fn build_args(req: &BatchRequest<'_>) -> Vec<OsString> {
    vec![
        "-p".into(),
        req.prompt.into(),
        "--output-format".into(),
        "json".into(),
        "--model".into(),
        CLAUDE_MODEL.into(),
        "--tools".into(),
        "".into(),
        "--json-schema".into(),
        req.schema.into(),
    ]
}

/// `claude -p --output-format json` 的结果字段，只取需要的部分。
#[derive(Deserialize)]
struct ClaudeResult {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: String,
    #[serde(default)]
    structured_output: Option<serde_json::Value>,
}

/// 纯函数：解析 `--output-format json` 的标准输出。
///
/// 指定 `--json-schema` 时，结构化结果在 `structured_output` 中，`result` 只作为兜底。
/// `is_error` 为真时 `ok` 为 false。JSON 不合法时返回 [`AgentError::BadOutput`]。
pub fn parse_output(stdout: &[u8]) -> Result<BatchOutcome, AgentError> {
    let parsed: ClaudeResult =
        serde_json::from_slice(stdout).map_err(|e| AgentError::BadOutput(format!("json: {e}")))?;
    let ok = !parsed.is_error;
    let raw_output = match parsed.structured_output {
        Some(value) => value.to_string(),
        None => parsed.result,
    };
    Ok(BatchOutcome { ok, raw_output })
}

/// Claude Code 后端。`bin` 由探测阶段给出，`timeout` 为单批的时限。
pub struct ClaudeBackend {
    /// `claude` 可执行文件路径。
    pub bin: PathBuf,
    /// 探测得到的能力集合。缺少 `REQUIRED` 中的能力时拒绝运行。
    pub capabilities: CapabilitySet,
    /// 单批的超时。超时后整个进程组被杀死。
    pub timeout: Duration,
}

impl AgentBackend for ClaudeBackend {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    fn run_batch(&self, req: &BatchRequest<'_>) -> Result<BatchOutcome, AgentError> {
        let missing = missing_capabilities(&self.capabilities);
        if !missing.is_empty() {
            return Err(AgentError::MissingCapability(missing));
        }
        let args: Vec<OsString> = build_args(req);
        let text: Vec<&str> = args
            .iter()
            .map(|a| {
                a.to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "argument is not utf-8")
                })
            })
            .collect::<Result<_, _>>()?;

        let done = run_finished(&self.bin, &text, &[], self.timeout, OUTPUT_LIMIT_BYTES).map_err(
            |err| match err {
                ProcError::Timeout(_) => AgentError::Timeout,
                ProcError::Io(e) => AgentError::Io(e),
            },
        )?;
        match done.code {
            Some(0) => parse_output(&done.stdout),
            code => Err(AgentError::ExitStatus(code.unwrap_or(-1))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use super::{ClaudeBackend, build_args, parse_output};
    use crate::agent::{AgentBackend, AgentError, BatchRequest, REQUIRED};
    use crate::testutil::tempdir;

    const SCHEMA: &str = "{\"type\":\"object\"}";

    fn request(prompt: &str) -> BatchRequest<'_> {
        BatchRequest {
            run_id: "run-test",
            paths: &[],
            prompt,
            schema: SCHEMA,
        }
    }

    fn strings(args: &[std::ffi::OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn args_snapshot() {
        let args = strings(&build_args(&request("DESCRIBE")));
        assert_eq!(
            args,
            vec![
                "-p",
                "DESCRIBE",
                "--output-format",
                "json",
                "--model",
                "haiku",
                "--tools",
                "",
                "--json-schema",
                SCHEMA,
            ]
        );
    }

    #[test]
    fn no_mcp_or_bash_appears_in_args() {
        let args = strings(&build_args(&request("describe git")));
        for arg in &args {
            let lower = arg.to_ascii_lowercase();
            assert!(!lower.contains("mcp"), "mcp found in {arg:?}");
            assert!(!lower.contains("bash"), "bash found in {arg:?}");
        }
    }

    #[test]
    fn builtin_tools_are_disabled_with_empty_tools_value() {
        let args = strings(&build_args(&request("p")));
        let idx = args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(args[idx + 1], "");
    }

    #[test]
    fn prompt_is_passed_as_a_single_argument() {
        let prompt = "line one\nline two; $(not run)";
        let args = strings(&build_args(&request(prompt)));
        assert_eq!(args[1], prompt);
    }

    #[test]
    fn parse_output_prefers_structured_output() {
        let out = parse_output(
            br#"{"is_error":false,"result":"text","structured_output":{"commands":[]}}"#,
        )
        .unwrap();
        assert!(out.ok);
        assert_eq!(out.raw_output, r#"{"commands":[]}"#);
    }

    #[test]
    fn parse_output_falls_back_to_result_text() {
        let out = parse_output(br#"{"is_error":false,"result":"plain"}"#).unwrap();
        assert_eq!(out.raw_output, "plain");
    }

    #[test]
    fn parse_output_reads_error_flag() {
        let bad = parse_output(br#"{"is_error":true,"result":"nope"}"#).unwrap();
        assert!(!bad.ok);
    }

    #[test]
    fn parse_output_rejects_non_json() {
        assert!(matches!(
            parse_output(b"not json"),
            Err(AgentError::BadOutput(_))
        ));
    }

    /// 在 `dir` 下写一个假 `claude`，写完后落盘再赋予执行权限，避免 ETXTBSY 类的竞争。
    fn fake_claude(dir: &Path, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join("claude");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(format!("#!/bin/sh\n{body}\n").as_bytes())
            .unwrap();
        file.sync_all().unwrap();
        drop(file);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn backend(bin: PathBuf, timeout: Duration) -> ClaudeBackend {
        ClaudeBackend {
            bin,
            capabilities: REQUIRED.into_iter().collect(),
            timeout,
        }
    }

    #[test]
    fn successful_run_passes_fixed_args_and_parses_output() {
        let dir = tempdir();
        let args_file = dir.path().join("args.txt");
        let bin = fake_claude(
            &dir.path().join("bin"),
            &format!(
                "printf '%s\\n' \"$@\" > '{}'\nprintf '%s' '{{\"is_error\":false,\"structured_output\":{{\"ok\":true}}}}'",
                args_file.display()
            ),
        );
        let req = request("DESCRIBE");
        let outcome = backend(bin, Duration::from_secs(5))
            .run_batch(&req)
            .unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.raw_output, r#"{"ok":true}"#);

        let recorded = fs::read_to_string(&args_file).unwrap();
        assert!(recorded.contains("--json-schema"));
        assert!(!recorded.to_ascii_lowercase().contains("mcp"));
    }

    #[test]
    fn nonzero_exit_is_exit_status() {
        let dir = tempdir();
        let bin = fake_claude(&dir.path().join("bin"), "exit 3");
        let err = backend(bin, Duration::from_secs(5))
            .run_batch(&request("p"))
            .unwrap_err();
        assert!(matches!(err, AgentError::ExitStatus(3)));
    }

    #[test]
    fn slow_run_is_timeout() {
        let dir = tempdir();
        let bin = fake_claude(&dir.path().join("bin"), "sleep 30");
        let err = backend(bin, Duration::from_millis(300))
            .run_batch(&request("p"))
            .unwrap_err();
        assert!(matches!(err, AgentError::Timeout));
    }

    #[test]
    fn missing_capability_refuses_to_run() {
        let dir = tempdir();
        let marker = dir.path().join("ran");
        let bin = fake_claude(
            &dir.path().join("bin"),
            &format!("touch '{}'", marker.display()),
        );
        let mut b = backend(bin, Duration::from_secs(5));
        b.capabilities.clear();
        let err = b.run_batch(&request("p")).unwrap_err();
        assert!(matches!(err, AgentError::MissingCapability(_)));
        assert!(!marker.exists(), "claude must not be spawned");
    }
}
