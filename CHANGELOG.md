# CHANGELOG

每个 demo 或里程碑一条，格式：`日期 · Dn 或里程碑 · 影响模块 · 说明`。

- 2026-11-03 · 文档 · 全部 · DESIGN v0.3、INTERFACES v0.2 对齐；新增 DEVLOG.md 与 CHANGELOG.md，无代码变更。
- 2026-11-03 · D1 · guard、types · 命令名、参数名、描述与示例校验，共享类型；21 项单元测试通过。
- 2026-11-03 · D2 · index（Tree）、search（normalize、anchor） · 锚点解析、插入文本（delete_tail 替换未完成词）、命令树内存视图；新增 35 项单元测试，累计 56 项通过；INTERFACES 同步。
- 2026-11-03 · D3 · config、index（load、save_atomic）、index::vectors · 配置读取与校验、api_key 拒绝、~ 展开；index.json 与 vectors.bin 原子替换，vectors_sha256 校验；新增 27 项单元测试，累计 83 项通过。
- 2026-11-03 · D4 · scan · PATH 扫描：同名去重、非可执行跳过、符号链接跟随、相对路径项跳过；新增 9 项单元测试，累计 92 项通过。
- 2026-11-03 · D5 · search（score、query）、output、cli、embed（trait 与 FakeEmbedder）、usage（只读） · 打分与回退、输出行含 delete_tail、query 子命令；embedding 尚未接入，query 在 D8 前退出码 3；复核修正 5 项；累计 122 项通过。
- 2026-11-03 · D6 · docs · man 与 --help 获取、回退、clean 去 ANSI 与退格、超时杀进程组（kill 命令，不引入 libc）；新增 9 项单元测试，累计 131 项通过。
- 2026-11-03 · D7 · agent（mod、claude、detect） · build_args 参数快照与 --allowedTools 白名单、parse_capabilities 解析 --help、能力集合加入 StrictMcp；尝试处理 docs 测试的偶发失败（fake_bin 落盘后再执行，超时放宽），未解决；新增 17 项单元测试，累计 148 项；本次记录时 147 项通过，`docs::tests::timeout_kills_the_whole_process_group` 偶发失败，基线未全绿（见 DEVLOG）；DESIGN 与 INTERFACES 同步。
- 2026-11-03 · D7 基线 · docs（仅测试） · 超时测试改用 pgrep 检查进程组，不再依赖 pidfile；根因未定位；改写后 cargo test 连跑 30 次、负载下 10 次全部通过（148 项）；日志断言未覆盖；D7 暂缓，未标记完成。
- 2026-11-03 · D8 前置 · process（新）、docs、Cargo.toml · 将 run_captured 抽到 process.rs，docs 改为调用，行为不变；新增依赖 ureq =3.5.1（rustls、json，关闭默认特性）；148 项通过，docs 连跑 20 次全部通过；process 无独立测试。
- 2026-11-03 · D8 · embed（openai）、agent（claude、mod）、process · OpenAiEmbedder 与 parse_embeddings_response（假服务测试 14 项）；ClaudeBackend、parse_output 与 run_finished（假命令测试）；AgentError 新增 BadOutput；168 项通过；真实 SiliconFlow 调用通过（2 条，1024 维）；claude 工具限制实验完成（样本少，结论方向性）；build_args 加入 --model haiku。
- 2026-11-03 · D8 收尾 · embed、agent（claude）、docs（DEVELOPMENT） · 标记 D8 完成，保留 process 单元测试、超时日志、tracing-subscriber、A/C 重跑与 --restricted 待定；模型来源核对（路由端点 127.0.0.1，模型名映射已确认，后端身份未证实）；四项检查通过。
- 2026-11-03 · D8 保留项 · process、main、tests/process_log.rs、Cargo.toml · process 新增 8 项单元测试；超时路径 warn 与 debug 日志；新增依赖 tracing-subscriber =0.3.23（fmt、std）；main 接入 stderr 订阅器；日志断言移至集成测试，解决并行测试下的捕获竞争；完整套件 176 项加 1 项集成测试，连跑 10 次全部通过。
- 2026-11-03 · D9（进行中） · generate（staging）、mcp（tools） · 暂存区 append 与四个工具的纯函数层，新增 rmcp =3.8.0（server、macros）；纯函数单元测试 11 项与暂存区测试 2 项通过；rmcp 包装层与 CLI 子命令未完成。
- 2026-11-03 · D9（有保留） · mcp（tools、mod）、generate（staging）、cli（mcp 子命令）、process 无变化 · 四个工具的纯函数层与 rmcp 包装层，`whatcmd mcp --run <id>` 启动 stdio 服务；serve_stdio 为同步函数，tokio 直接声明；手动 stdio 验收通过 initialize、tools/list、tools/call，根命令写入正常；子命令写入因 known_nodes 为空被拒绝，待 D10 接入索引；lib 189 项加集成测试 1 项通过。
- 2026-11-04 · D9（修正） · mcp（tools、mod） · serverInfo 设为 whatcmd 0.1.0；错误拆分为 UnknownCommand 与 NotInTree；四项检查通过，lib 189 项加集成测试 1 项通过；未重跑手动 stdio 验收。
- 2026-11-04 · D10 · generate（mod、merge、plan）、cli（reindex、mcp 的 known_nodes） · 锁（File::try_lock）、增量计划、阶段 A/B、reindex.log；`--names` 模式展开子命令（DESIGN 9.5 已同步）；真实运行 `reindex --names git` 退出码 0，索引 234 个节点，子命令经队列进入索引；lib 219 项、集成测试 1 项通过。
- 2026-11-04 · D10（重跑） · generate（prompts/describe.md）、mcp（tools） · 提示词 v2，描述改为英文并以"做什么"为主；重跑 `reindex --names git` 退出码 0，写入 56 个节点，失败 0；git diff、log、show、rebase 已重新生成英文描述。
- 2026-11-04 · D10（提示词 v5、并发、--help 检查） · generate（mod、prompts/describe.md）、agent（mod、claude、detect）、docs、config · 子命令提取改为只依赖提示词，删除程序侧文档行过滤，PROMPT_VERSION=5；JsonSchema 列为必需能力；`run_batches` 并发调用，`[agent] concurrency` 默认 4；子命令 `--help` 只认 stdout，嵌套路径回退 `-h` 与父命令帮助，无文档不入队；`reindex --names git` 302s，生成 1216、失败 0，仅跳过 7 个未安装子命令；lib 236 项通过，fmt、clippy 通过；未提交。
- 2026-11-04 · D10（收尾） · generate（mod）、DESIGN 第 11、12、13 节 · 去掉 `already seen` 日志行；确认 `agent_version` 为 `2.1.296 (Claude Code)`；DESIGN 安全、测试、待确认项与当前实现同步；lib 236 项通过；未提交。
- 2026-11-05 · 查询与 zsh 前端 · cli（query）、shell（whatcmd.zsh） · `query` 接入真实 OpenAiEmbedder；`shell/whatcmd.zsh` 的 Ctrl-G 用 read-from-minibuffer 读输入、fzf 选候选、插入 LBUFFER 不回车；tmux 隔离环境验证输入、选择、插入流程通过；真实查询返回候选，排序偏差待提示词维护；未提交。
- 2026-11-05 · 描述提示词 v6 · generate（mod、prompts/describe.md） · description 规则改为两段：首句保留文档原有动作与对象用词，再接 "Use it to" 开头的用户口吻用法句，上限约 70 词；PROMPT_VERSION=6；离线评测（改写现有描述，30 条查询）MRR 0.570 → 0.602，词面加分未采用；lib 237 项通过；未重跑 reindex；未提交。
- 2026-11-05 · 预置集、record、移除 MCP · seed（新）、usage（append）、cli（record，删除 mcp）、shell、Cargo.toml、DESIGN、INTERFACES · 内置 POSIX 预置集，索引缺失时写入；`whatcmd record` 写使用记录，widget 选中后后台调用；删除 mcp 模块与 rmcp、tokio 依赖；真实查询与使用加分验证通过；lib 229 项通过。
- 2026-11-05 · 预置集、record、移除 MCP · seed（新）、usage、cli、shell、Cargo.toml、DESIGN、INTERFACES · 内置预置集，索引缺失时写入且不调用 LLM；`whatcmd record <key>` 追加使用记录，widget 选中后后台调用；删除 mcp 模块、子命令与 rmcp、tokio 依赖；真实 embedding 查询通过，record 后加分生效；lib 229 项通过，fmt、clippy 通过。
