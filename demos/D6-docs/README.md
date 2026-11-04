# D6：文档获取（docs）

覆盖模块：`docs`。

只用标准库，不新增依赖。子进程用 `Command::new(bin).args([...])` 启动，不经过 shell。测试使用 `testutil::tempdir()` 下的假可执行文件，PATH 由临时目录拼接，不读真实 `PATH`，也不调用真实 `man`。

## 运行方式

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

只跑本 demo 的测试：

```bash
cargo test docs::
```

## 预期输出

- 四条命令退出码均为 0。
- `cargo test docs::` 共 9 项通过。

## 验收项

| 验收要求（AGENT.md 第 6 节） | 对应测试 |
|---|---|
| `clean` 去除 ANSI 转义与退格 | `clean_strips_ansi_and_backspace_overstrike` |
| `clean` 按字符边界截断并追加标记 | `clean_truncates_at_char_boundary_and_marks` |
| man 子进程的固定环境变量（`LC_ALL`、`MANPAGER`、`PAGER`、`MANWIDTH`） | `man_page_runs_with_fixed_environment` |
| PATH 中无 `man` 时返回 `Missing` | `missing_man_binary_is_missing` |
| 子命令 `--help` 传入完整路径加 `--help` | `help_for_subcommand_passes_path_then_flag` |
| man 失败后回退到 `--help` | `fetch_falls_back_to_help_when_man_missing` |
| 无可执行文件或输出为空时返回 `Missing` | `help_for_missing_binary_is_missing`、`empty_output_is_missing` |
| 超时返回 `Timeout`，并杀死整个进程组（含后台子进程） | `timeout_kills_the_whole_process_group` |

## 实际结果

- fmt、clippy（`-D warnings`）、test、rustdoc（`-D warnings`）退出码均为 0。
- `cargo test` 共 131 项通过，其中 `docs::tests` 9 项。
- 以上结论基于 `cargo fmt` 之后的最后一次完整检查。

## 行为说明

- `clean` 的截断保证在字符边界进行，但追加的 `\n[truncated]` 不计入 `max_bytes`，结果可能略长于上限。这是有意的。
- 回退到 `--help` 只针对单个根命令。多于一个 token 的子命令路径直接走 `--help`，不尝试 man，因为 man 页按根命令组织。
- 超时与正常退出都会执行 `kill -KILL -- -<pgid>` 清理进程组。这里借用系统 `kill` 命令，没有引入 `libc`。
- 原始输出读取上限为 `max_doc_bytes` 的 4 倍，超出部分读后丢弃，避免子进程因管道写满而阻塞。

## 已知未覆盖

- 超出读取上限（`RAW_CAP_FACTOR`）后的丢弃逻辑没有单独测试。`drain` 的行为只通过正常输出间接覆盖。
- `DocError::Io`（`spawn` 失败）没有测试断言。
- `clean` 的 OSC 序列只在 `guard::output` 的测试中覆盖，docs 层没有单独测。
- `kill` 命令不可用时的行为（静默忽略）没有测试。
- 真实 `man` 页的输出没有测试，只测了假可执行文件。真实环境的清洗效果待 D10 或手动验收确认，这也是 DEVLOG 中 `sanitize_text` 遗留问题的一部分。
