# D9：mcp（rmcp 包装层与 stdio 服务）

覆盖模块：`mcp::tools`（纯函数）、`mcp`（rmcp 注册与 `serve_stdio`）、`generate::staging`（暂存区追加）、`cli` 的 `mcp` 子命令。

## 运行方式

四项检查：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

只跑本 demo 相关的测试：

```bash
cargo test mcp::
cargo test generate::
```

手动 stdio 验收（使用临时配置与临时暂存区，不写真实 `~/.cache/whatcmd`）：

1. 在 `target/test-tmp/mcp-manual` 下写一份配置，`staging_dir` 指向该目录内。
2. 设置 `WHATCMD_CONFIG` 指向该配置，启动 `whatcmd mcp --run manual-1`。
3. 按 stdin 顺序发送 JSON-RPC：`initialize`、`notifications/initialized`、`tools/list`、`tools/call`（`read_help`、`write_description`、`write_params`）。
4. 检查 stdout 的响应，并检查 `staging_dir/manual-1.jsonl`。

这一步只启动本地进程，不调用模型，不消耗额度。

## 预期输出

- 四项检查退出码均为 0。
- `cargo test` 共 189 项通过，集成测试 `tests/process_log.rs` 1 项通过。
- `mcp::tools` 11 项、`generate::staging` 2 项通过。
- 手动验收：`initialize` 返回协议版本与 `tools` 能力；`tools/list` 列出四个工具；根命令的 `write_description` 与 `write_params` 成功，暂存区出现对应记录；子命令的写入返回错误。

## 实际结果

- 四项检查退出码均为 0。`cargo test` 189 项通过，集成测试 1 项通过，0 失败。
- 手动验收结果：
  - `initialize` 返回 `protocolVersion` `2025-06-18`，`capabilities.tools` 存在。`serverInfo.name` 为 `whatcmd`、版本 `0.1.0`。
  - `tools/list` 返回四个工具，`read_help` 的 `inputSchema` 含必填 `path` 数组。
  - `read_help` 对 `["git"]` 返回 `git --help` 文本。
  - `write_description` 对 `git` 返回 `staged`，暂存区新增一行 `command` 记录。
  - `write_params` 对 `git` 传入 `-v` 与 `--bare`：`-v` 被丢弃并记 `warn`，暂存区新增一行 `params` 记录，只含 `--bare`。
  - `write_description` 对 `["git", "clone"]` 返回 `NotInTree` 错误（`git is not in this run's command tree; describe its parent first`）。这是 `known_nodes` 为空的临时限制，见 DEVLOG。

## 已知未覆盖与风险

- 子命令的写入验收未完成。`known_nodes` 在 `mcp` 子命令中为空，需要 D10 接入索引后从命令树填充，再补做验收。
- 错误已拆分：`UnknownCommand`（不在扫描结果中）与 `NotInTree`（父节点或节点不在命令树中）。修正后的手动 stdio 验收已重跑，两种错误文案与 `serverInfo.name` `whatcmd` 均在协议层确认。
- 没有真实 `claude -p` 通过此 server 调用工具。
- `mcp` 子命令的参数解析没有单独测试。
- 非法输入中，`UnknownCommand` 与 `NotInTree` 已在手动验收中覆盖；`TooDeep` 与 guard 校验分支仅有单元测试。
- 偏离 DESIGN 第 18 节的三处（同步 `serve_stdio`、`Arc<ToolContext>` 无 `Mutex`、tokio 直接声明）见 DEVLOG。

## 状态

有保留，未标记完成。子命令写入的验收待 D10 接入索引后补做。
