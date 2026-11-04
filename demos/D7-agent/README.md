# D7：agent 参数拼装与能力解析

覆盖模块：`agent::claude::build_args`、`agent::detect::parse_capabilities`，以及 `agent/mod.rs` 中的能力类型（`Capability`、`REQUIRED`、`missing_capabilities`）。

只做纯函数，不启动 `claude`，不访问网络。`claude --help` 的真实输出保存在 `tests/fixtures/claude-help-2.1.296.txt`，解析测试直接读取它。

## 运行方式

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

只跑本 demo 的测试：

```bash
cargo test agent::
```

## 预期输出

- 四条命令退出码均为 0。
- `cargo test` 共 148 项通过，其中 D7 新增 17 项（`agent::detect::tests` 9 项，`agent::claude::tests` 5 项，`agent::tests` 3 项）。D6 结束时为 131 项，差值 17 与此一致。

## 验收项

| 验收要求（DESIGN 第 9.6、11、12 节） | 对应测试 |
|---|---|
| 参数快照：完整顺序与取值 | `args_snapshot` |
| `--allowedTools` 只含四个 whatcmd 工具 | `allowed_tools_value_is_exactly_the_four_whatcmd_tools` |
| 整条参数列表不含 Bash 或其他内置工具名 | `no_bash_or_builtin_tool_name_appears_anywhere` |
| `--tools ""` 禁用内置工具 | `builtin_tools_are_disabled_with_empty_tools_value` |
| prompt 作为单个参数传递，不经 shell | `prompt_is_passed_as_a_single_argument` |
| 真实 `--help` 解析出全部必需能力 | `real_help_yields_every_required_capability` |
| 缺 `--tools` 时缺 `RestrictedTools` | `missing_tools_option_drops_restricted_tools` |
| 缺 `--output-format` 时缺 `JsonOutput` | `missing_output_format_drops_json_output` |
| 缺 `--strict-mcp-config` 时缺 `StrictMcp` | `missing_strict_mcp_drops_strict_mcp` |
| `--disallowedTools`、`--tools-extra` 不匹配 `--tools` | `lookalike_names_do_not_match` |
| 描述文字中提到 `--tools` 不算选项 | `mention_inside_description_does_not_count` |
| `--allowedTools` 与 `--allowed-tools` 两种写法都识别 | `allowed_tools_accepts_either_spelling` |
| `-p` 与 `--print` 都识别为非交互 | `print_short_and_long_forms_both_count` |
| 空 help 文本不产生任何能力 | `empty_help_yields_nothing` |
| 能力集合缺口计算 | `agent::tests` 下 3 项 |

## 实际结果

- 四项检查逐条直接取退出码：fmt、clippy（`-D warnings`）、test、rustdoc（`-D warnings`）均为 0。`cargo test` 148 项通过，0 失败。
- 改写超时测试前：完整 `cargo test` 连跑 30 次全部通过；10 个 `yes` 负载下 10 次全部通过；每次强制重编译 15 次全部通过。未复现偶发失败。
- 改写超时测试后：完整 `cargo test` 连跑 30 次全部通过，每次 148 项；10 个 `yes` 负载下 10 次全部通过，负载进程已清理，残留 0。
- 记录见 `target/repro/` 下的 `baseline-summary.log`、`load-summary.log`、`rebuild-summary.log`、`final-summary.log`、`final-load-summary.log`。

## 已知未覆盖与风险

- `--allowedTools` 是否真的禁用 Bash 未验证。D7 只检查参数拼装，真实行为在 D8 用临时目录的假标记文件验证。
- `claude --help` 中的 `--restricted` 未纳入能力集合，D8 评估是否加入。
- 解析规则基于 Claude Code 2.1.296 的帮助格式。升级后需重新核对 fixture。
- `docs::tests::timeout_kills_the_whole_process_group` 曾偶发失败（pidfile 未写出），根因未定位，累计约 90 次运行未复现。测试已改写为用 `pgrep` 检查进程组，不再要求脚本在超时前写出文件。新测试不再证明子进程在超时前启动完成，若原问题是真实的启动延迟，会被掩盖。详见 DEVLOG"D7 基线：docs 超时测试改写"条目。
- 超时路径的 `warn`/`debug` 日志尚未实现，日志断言未覆盖（未接入 `tracing-subscriber`）。
- 因为根因未定位，本 demo 的基线稳定只有运行次数作为依据，没有原因上的证明。

## 状态

暂缓，未标记完成。未解决的问题见上文与 DEVLOG"D7 基线：docs 超时测试改写"条目的遗留问题。
