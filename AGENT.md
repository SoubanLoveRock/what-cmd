# AGENT.md

本文件是给参与开发的 agent（包括 Claude Code、Kiro 等）的工作说明。开始任何改动前先读完本文件，再读 `docs/DESIGN.md` 和 `docs/INTERFACES.md`。

## 1. 项目概览

whatcmd 是一个用英文自然语言查找终端命令的 CLI。用户输入类似 `find which process holds port 8080`，工具返回候选命令，选中后写入 zsh 命令行，由用户自己回车执行。

- 语言：Rust，单 crate，lib + bin，edition 2024。
- 目标平台：Linux 与 macOS 同级支持，不分主次。
- v1 范围：离线模糊查询、reindex（扫描 PATH，用 Claude Code 生成描述）、zsh widget、`doctor`。
- 详细架构见 `docs/DESIGN.md`，模块签名见 `docs/INTERFACES.md`。

## 2. 目录约定

```
whatcmd-demo/
├── AGENT.md          # 本文件
├── CHANGELOG.md      # 每个 demo 或里程碑一条
├── DEVLOG.md         # 取舍、理由、未决问题
├── docs/
│   ├── DESIGN.md     # 架构与行为
│   └── INTERFACES.md # 模块签名
├── demos/
│   └── Dn-<name>/    # 每个 demo 一个目录，含 README.md
├── src/
│   ├── main.rs       # 只做解析、调用、映射退出码
│   └── lib.rs
└── tests/
    └── fixtures/     # 假 man、假 --help、假 agent 输出
```

`demos/` 只放 demo 的说明和验收记录，demo 的代码仍在 `src/` 中。

## 3. 文档规则

- `docs/DESIGN.md` 管架构和行为，`docs/INTERFACES.md` 管签名。
- 代码与文档不一致时，以代码为准，并在同一次改动中更新文档。
- 所有 `pub` 项都要有 rustdoc 注释。`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` 必须通过。
- 做出取舍、推翻原设计、遇到未解决问题时，写入 `DEVLOG.md`，格式为日期、标题、背景、决定、理由、遗留问题。
- `CHANGELOG.md` 每条一行，写明 demo 编号和影响的模块。
- 不要为了记录进展而新建 markdown 文件，只更新上面列出的文件。

## 4. 日志规则

框架使用 `tracing` 与 `tracing-subscriber`。

- 日志只写 stderr。stdout 只输出候选行，这是 DESIGN.md 第 4 节的硬约束，任何情况下都不能破。
- 默认级别 `warn`。`-v` 为 `info`，`-vv` 为 `debug`。`WHATCMD_LOG` 环境变量可覆盖，语法与 `RUST_LOG` 相同。
- 级别含义：
  - `error`：用户需要处理的失败，如配置错误、索引损坏。
  - `warn`：降级或跳过，如某命令的 man 与 `--help` 都没有，某批 agent 失败。
  - `info`：阶段边界，如开始扫描、批次完成、索引替换。
  - `debug`：子进程命令行、耗时、超时、暂存区读写细节。
  - `trace`：仅用于排查 MCP 协议或模糊评分。
- 使用结构化字段，不拼接字符串。例如 `info!(count = n, batch = i, "batch done")`。命令名统一用字段 `name`。
- man 和 `--help` 原文、agent 原始输出只在 `debug` 记录，并截断到固定长度。这些内容可能含提示注入，不进入 `info` 及以上级别。
- 关键失败路径要有测试，断言返回的错误类型，或用测试订阅器断言日志，防止重构时日志悄悄消失。

## 5. 代码质量规则

每完成一步，以下命令必须全部通过：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

- 纯函数优先。`docs/INTERFACES.md` 第 19 节列出的函数必须有单元测试。
- 涉及 agent 和 MCP 的测试使用 `FakeBackend`，不调用真实 `claude`。
- 文件系统相关函数接受目录参数，测试用 `tempfile`，不读写真实 `$HOME`。
- 错误类型用 `thiserror` 定义，`main.rs` 用 `anyhow` 汇总。
- 新增依赖使用精确版本（`=x.y.z`），并在 DEVLOG 记录选它的理由。
- 子进程用 `Command::new(bin).args([...])`，不经过 shell。
- 命令名只能通过 `guard::name::validate` 构造为 `CommandName`。

## 6. Demo 顺序

按顺序推进，每个 demo 独立验收。完成前不要提前实现后面的 demo。

| Demo | 覆盖模块 | 验收 |
|---|---|---|
| D1 | `guard::name`、`guard::output`、`types` | 非法命令名、短参数、控制字符、长度边界被拒绝 |
| D2 | `search::normalize`、`search::anchor`、`index::Tree` | 锚点解析：末尾词丢弃、`-` 跳过、`--` 停止、`giit` 报错并给建议 |
| D3 | `config`、`index` 读写、`index::vectors` | 原子写、schema 校验、向量长度校验、`api_key` 拒绝 |
| D4 | `scan` | 同名去重、非可执行跳过 |
| D5 | `search::score`、`output`、`cli` 的 `query` | 固定向量下的排序、精确置顶、子树衰减、回退 |
| D6 | `docs` | `clean` 去 ANSI；man 与 help 的回退；超时杀进程组 |
| D7 | `agent::claude::build_args`、`agent::detect::parse_capabilities` | 参数快照；`--allowedTools` 只含四个工具，不含 Bash |
| D8 | `agent::claude::ClaudeBackend`、`embed::openai` | 3 个假命令的真实调用，检查暂存区；真实 endpoint 下的 embedding（需要确认后执行） |
| D9 | `mcp` | 用 stdio 手动发送 tool 调用，rmcp API 在此对照确认 |
| D10 | `generate` 全流程、`seed`、`auto` | FakeBackend + FakeEmbedder 跑通增量、删除、预置豁免、单批失败隔离、阶段 B 失败后暂存区保留 |

D1 到 D5 不依赖 agent，可以先完成。`embed` 的真实调用在 D8 前用 `FakeEmbedder` 替代。D7、D8 之前必须先完成 DESIGN.md 第 11 节的安全边界验证。

## 7. 每步验收清单

完成一个 demo 时，逐项确认并在 `demos/Dn-<name>/README.md` 中记录结果：

1. fmt、clippy、test、rustdoc 全部通过。
2. 本 demo 涉及的 `pub` 项有文档注释。
3. 关键路径有 `info` 或 `warn` 日志，敏感内容未进入高级别日志。
4. demo README 写明运行方式、预期输出和实际验收结果。
5. DEVLOG 记录新的取舍或未决问题。
6. CHANGELOG 新增一条。

验收未通过时如实记录，不标记为完成。

## 8. 未决事项

这些事项在对应 demo 之前必须解决，解决结果写入 DEVLOG：

- Claude Code 参数核对：`-p`、`--output-format json`、`--allowedTools`、`--mcp-config`、`--strict-mcp-config`、`--tools`、`--bare`，需用本机 `claude --help` 确认。（D7 前）
- `--allowedTools` 是否真的禁用未列出的内置工具。当前实验结论是 `--tools` 有效，`--allowedTools` 下 Bash 仍会执行。DESIGN.md 第 11 节的安全边界需在 D8 后确认。
- 模糊匹配库在 `nucleo-matcher` 与 `fuzzy-matcher` 之间选定，并写入精确版本。（D2 前）
- rmcp 3.x 的 API，对照 docs.rs 确认。（D9 前）
- 配置目录、索引目录、暂存区目录的最终路径，在 D3 前确认。

## 9. 工作方式

- 一次只推进一个 demo。改动范围尽量收窄，不顺手重构无关代码。
- 写代码前先读相关的设计文档和现有代码，保持命名和风格一致。
- 遇到设计文档与实际情况不符，先记录到 DEVLOG，再决定是改代码还是改文档。
- 涉及删除文件、修改已有配置、引入新的外部服务调用时，先征得确认。
