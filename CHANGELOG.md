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
