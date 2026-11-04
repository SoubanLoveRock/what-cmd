# D8：embed::openai 与 agent::claude::ClaudeBackend

覆盖模块：`embed::openai::OpenAiEmbedder`、`agent::claude::ClaudeBackend`、`agent::claude::parse_output`、`process::run_finished`。

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
cargo test embed::openai
cargo test agent::claude
```

真实 embedding 与真实 claude 调用不在 `cargo test` 中执行。真实 embedding 需要手动设置环境变量并产生网络请求，真实 claude 每次运行都会计入模型额度。

## 预期输出

- 四项检查退出码均为 0。
- `cargo test` 共 176 项通过，另有集成测试 `tests/process_log.rs` 1 项通过。
- `embed::openai` 14 项、`agent::claude` 11 项（其中 `ClaudeBackend` 相关 4 项）、`process` 8 项。

## 实际结果

- 四项检查退出码均为 0，`cargo test` 176 项与集成测试 1 项通过，0 失败。
- 完整套件连跑 10 次，10 次退出码均为 0。
- 真实 embedding：SiliconFlow `https://api.siliconflow.cn/v1`，模型 `BAAI/bge-m3`，2 条输入，返回 2 个 1024 维向量，退出码 0。密钥只通过环境变量传入，未写入文件。
- claude 工具限制实验（每种配置在独立临时目录，以标记文件是否生成为准）：

| 配置 | 次数 | 标记文件生成 | 备注 |
|---|---|---|---|
| A `--tools ""` | 2 | 0 | 模型输出伪造的工具调用文本与编造结果，`permission_denials` 为空 |
| B 仅 `--allowedTools` 四个工具 | 5 | 3 | 不带 `--model` 的 2 次均生成；带 `--model haiku` 的 3 次中 1 次生成 |
| C `--restricted` | 2 | 0 | 模型声称无 Bash 工具 |
| A `--tools ""`（`--model haiku` 重跑） | 2 | 0 | 与上表 A 合计 4 次，0 次生成 |
| C `--restricted`（`--model haiku` 重跑） | 2 | 1 | 与上表 C 合计 4 次，1 次生成。`--restricted` 不能作为禁用手段 |
| P `--tools Bash`（阳性对照，`--model haiku`） | 2 | 2 | 方法能检出执行 |
| A `--tools ""`（独立一批，新提示词，`--model haiku`） | 3 | 0 | 不与上表合并计算 |
| B 仅 `--allowedTools`（独立一批，新提示词，`--model haiku`） | 3 | 3 | 不与上表合并计算 |
| C `--restricted`（独立一批，新提示词，`--model haiku`） | 3 | 0 | 不与上表合并计算。全部记录合计 7 次生成 1 次 |
| P `--tools Bash`（独立一批，新提示词，`--model haiku`） | 3 | 3 | 阳性对照稳定 |

独立一批 A 配置的 `out.json` 已逐条检查（A-1 至 A-3）。A-1、A-3 明确说没有 Bash 工具；A-2 的 `result` 含形似工具调用的 XML 与编造的 `no output`，属于伪造的工具调用文本，但标记未生成，没有真实执行。判断仍只看标记文件。

- 逐条检查 `out.json`（独立一批，12 次）：A-2 的 `result` 含形似工具调用的 XML 与编造的 `no output`，无真实执行；其余 A、C 的 `result` 只说明没有 Bash 工具。B、P 全部 `result` 为 `done` 且生成标记。判断执行与否只看标记文件。
- 结论（当前环境）：`--allowedTools` 不能单独禁用 Bash。`--tools ""` 是承担禁用责任的参数。判断执行与否只看文件系统结果，不看 `result` 文本。

## 模型来源

- 运行时的端点为 Cherry Studio 进程环境中的 `ANTHROPIC_BASE_URL=http://127.0.0.1:60784`，不是 `~/.claude/settings.json` 中的地址。cc-switch 会改写 settings 文件。
- `claude --model haiku` 解析到 `ANTHROPIC_DEFAULT_HAIKU_MODEL`，即路由侧的 `claude-haiku-5.5-thinking`。
- 路由回显的模型名只是透传，不能证明后端真实模型。

## 已知未覆盖与风险

- 超时日志只断言了 WARN 与 `kill group sent`，`timeout reached` 与正常退出路径的日志未断言。
- 日志断言位于 `tests/process_log.rs`。并行测试下的捕获竞争已通过移到独立二进制规避，竞争机制未直接证实。
- 安全实验样本少（2 至 5 次），未在无路由环境下重跑。A、C 配置未统一为 `--model haiku` 重跑。
- `--restricted` 不纳入能力检查 `REQUIRED`（已决定）。重跑中它生成过标记文件。
- `parse_output` 的字段结构在实验中观察到 `result`、`is_error`、`permission_denials`、`subtype`，与实现一致；其余字段未核对。
- D7 的 `docs::tests::timeout_kills_the_whole_process_group` 偶发失败根因未定位，见 DEVLOG。

## 状态

完成（有保留）。保留项见上方"已知未覆盖与风险"，详见 DEVLOG "D8 保留项"、"D8 收尾" 与 "D8 安全实验独立一批" 条目。安全实验的临时目录保留在 `target/test-tmp/claude-safety-final/`，未删除。
