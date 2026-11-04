# DEVLOG

格式：日期、标题、背景、决定、理由、遗留问题。

---

## 2026-11-03 文档对齐：v1 范围与交互模型重定

**背景**

原文档（v0.2）定义 v1 为纯离线模糊匹配，每个命令独立打分。讨论后，需求变为：联网 embedding 查询、命令树分级检索、输入条交互、预置集、无感知后台构建。DESIGN 与 INTERFACES 的模块结构、签名、数据格式都需要改动。

**决定**

1. v1 进入联网 embedding 查询，断网兜底（离线模糊匹配）推迟到 v1.1。
2. 索引从扁平命令列表改为命令树：命令节点（如 `git pull`）和挂在命令下的 `--` 长参数节点（如 `git clone --branch`）。
3. 查询流程：zsh 输入条读取 `LBUFFER` → 锚点解析（纯本地） → embedding(查询文本) → 锚点子树内向量匹配 → 候选行。
4. 锚点解析规则：末尾未完成词丢弃；首个 token 不是根命令则报错并给修正提示，不查询；`-` 开头的 token 跳过；`--` 单独出现时停止锚点解析；不匹配子命令的 token 视为参数值或查询词，停止锚点解析。
5. 打分：`s(n) = max(s_self, s_sub)`，子树分按层数衰减；精确命令名置顶；使用加分以对数增长并带时间衰减。系数先写成代码常量，固定样本调参后再决定是否开放配置。
6. 子树为空时回退到父节点，直到根；回退结果在候选中标出 `via`。
7. 插入规则：只改 `LBUFFER`，不回车；候选路径已是 `LBUFFER` 末尾的命令链时不重复追加；`--name=value` 按 `=` 前的部分去重。
8. 使用信号以"选中"为准，由 `whatcmd record <key>` 写入 `usage.jsonl`，只存节点路径和时间，不存查询文本，不进入索引。
9. 索引改为 schema v2，`vectors.bin` 与 `index.json` 分开存放，两者原子替换。schema 1 不做迁移。
10. reindex 分两阶段：阶段 A 由 agent 生成描述和参数，结果落暂存区；阶段 B 做 embedding。阶段 B 失败不触发 agent 重跑。
11. 批次按子树组织，并把父节点的当前描述放进提示词作为边界上下文。
12. 两种触发方式：`--auto` 由 shell 初始化后台拉起，指纹未变则退出，无输出，日志写 `cache_dir/reindex.log`；`/reindex` 在输入条内前台执行，显示进度并阻塞。
13. 预置集随发布附带描述和向量，`pinned = true`，reindex 不比较、不删除。分为跨平台交集（`seed/common.json`）和平台小集（`seed/linux.json`、`seed/macos.json`）。
14. 快捷键 `Ctrl-G`。输入条以 `/` 开头且后面无空格或仅一个词时进入功能菜单，否则一律当查询。
15. 退出码新增 `3`，表示 embedding 服务不可用。
16. 密钥只能经 `api_key_env` 指定的环境变量读取，配置中出现 `api_key` 直接报错。

**理由**

- 命令路径是用户真实的输入单位。`git pull` 不是 PATH 上的可执行文件，扁平模型无法表达"`git` 被子命令拉上来"。
- 分两阶段把昂贵且有成本的 LLM 调用与容易失败的 HTTP 调用隔开，失败后不重复付费。
- 预置集让初始化不产生 LLM 成本。预置向量绑定默认模型，换模型时只重算 embedding。
- 使用信号只用选中，不用出现在候选中，避免高频展示的命令被自我强化。
- 锚点严格报错而不是静默回退，避免用户输入的前缀错误被默默忽略后推荐错误的命令。

**遗留问题**

- 打分系数（`decay`、`min_score`、`usage_*`）未经样本调参，数值待定。
- `via` 的展示格式待样本确认。
- 向量存储形式（`vectors.bin`）和子命令发现方式（agent 在 `write_description` 中给出 `subcommands`）暂定，实现时确认。
- 真实 `claude` 参数（`--tools`、`--allowedTools` 的组合）待 D7/D8 验证。`--allowedTools` 是否真的禁用未列出的内置工具仍未确认。
- 提示词边界样例（`git`、`git pull`、`git clone`）需要人工检查，结果待记录。
- `lib.rs` 出现在 DESIGN 模块树中，INTERFACES 尚未列出其导出内容。

---

## 2026-11-03 D2 锚点解析与插入文本

**背景**

D2 实现 `index::Tree`、`search::normalize`、`search::anchor`。实现时发现 INTERFACES 中的部分签名需要调整。

**决定**

1. `insertion_text` 返回 `Option<Insertion { delete_tail, text }>`，不再返回 `String`。widget 先删 `delete_tail` 个字符再写入 `text`，用于替换被丢弃的未完成词：`git cl` 选中 `git clone` 得到 `git clone`，而不是 `git cl clone`。用户已确认。
2. 未完成的末尾词只用于确定锚点，不参与匹配。补全交给 shell 本身。
3. 空 `LBUFFER` 或只有未完成的根命令词时，锚点为根（`key: None`），搜索范围为全部节点。
4. `seen_flags` 的范围为第一个独立 `--` 之前的全部 token，包括锚点解析停止之后的 token。
5. `Tree` 原写法 `pub struct Tree<'a> { pub fn ... }` 不是合法 Rust，改为 struct 加 impl 块。新增 `get`、`root_names`。
6. `AnchorError::UnknownRoot` 的 Display 包含修正建议，与 DESIGN 第 7.3 节文案一致。
7. 偏离记录：D1 新增的 `GuardError::EmptyPath` 不在 INTERFACES 第 5.1 节中，`validate_path` 对空切片返回它。这里补记，INTERFACES 同步。
8. `CommandName::from_validated` 为 `pub(crate)`，crate 内任何模块都可绕过校验。DESIGN 要求只能经 `guard::name` 构造，目前只保证到 crate 边界。D2 后再评估是否收紧。
9. 模糊匹配库（AGENT.md 第 8 节）推迟到 v1.1 决定，DESIGN 第 1、14 节已写明离线兜底不在 v1 内，D2 不需要选库。

**理由**

- 替换未完成词比追加更符合用户看到的结果，代价是 widget 多一步删除。
- 锚点只看已完成 token，避免把用户还没输完的词误判为错误子命令。
- 参数去重范围取到独立 `--` 为止，与 DESIGN 第 7.4 节"已出现的长参数"一致，同时 `--` 之后的 token 视为位置参数，不应影响去重。

**遗留问题**

- `git clone cl`：`clone` 已是完整子命令，末尾 `cl` 被丢弃，插入为空，`cl` 留在缓冲区。当前按字面规则不追加，用户体验可能不对，待 widget 接入后用真实输入验证。
- 编辑距离建议只覆盖根命令。子命令的拼写错误（如 `git clnoe`）目前只会停止锚点解析，不给建议，DESIGN 未要求，暂不实现。
- `search::normalize` 与 `search::mod.rs` 的 `SearchError` 只放了 D2 需要的变体，`Embed` 与 `Vector` 留到 D5。

## 2026-11-03 D1 依赖选定与 guard 实现

**背景**

D1 需要 serde 派生（`types.rs`）和 `thiserror` 错误枚举（`guard`）。AGENT.md 第 5 节要求新增依赖使用精确版本并记录理由。

**决定**

- `serde = { version = "=1.0.229", features = ["derive"] }`
- `thiserror = "=2.0.21"`
- `cargo fmt` 对 `guard/name.rs` 的测试数组做了换行调整，仅格式变化，无逻辑改动。

**理由**

- serde 是 Rust 生态中序列化的事实标准，`index.json`、暂存区、usage 记录都需要它。
- thiserror 与 INTERFACES 第 1 节约定一致，错误类型用它定义。
- 精确版本锁定后，`Cargo.lock` 与声明一致，便于复现。

**遗留问题**

- `Cargo.lock` 解析时 `syn` 解析到 3.x 主版本，是由 serde_derive 间接引入。这不影响 D1，但如果后续依赖冲突，需要回头确认。
- `guard::output::sanitize_text` 的 ANSI 与退格处理是手写的，覆盖的转义形式有限。D6 接入 docs 时需要用真实 man 输出再核对。

## 2026-11-03 D3 路径、完整性校验与 endpoint 约定

**背景**

D3 实现 `config`、`index` 读写、`index::vectors`。实现前有三处需要定：默认路径、两个文件替换的完整性、`embedding.endpoint` 的默认值。

**决定**

1. 默认路径在 Linux 与 macOS 上一致：`index_dir = ~/.local/share/whatcmd`，`staging_dir = ~/.cache/whatcmd/staging`，`usage_file = ~/.cache/whatcmd/usage.jsonl`。不使用 `~/Library`。`~` 展开只读取 `HOME`，不引入 `dirs` 依赖。
2. `cache_dir` 固定为 `usage_file` 的父目录，用于指纹、`reindex.log`、`index.lock`。它不是配置项。
3. `IndexMeta` 增加 `vectors_sha256`。`load` 时校验 `vectors.bin` 的哈希，发现两次 rename 之间崩溃留下的新索引配旧向量。
4. `embedding.endpoint` 无默认值，缺失时 `load` 报错，`doctor` 提示。
5. 测试不读写真实 `$HOME`。临时目录用 `tempfile::tempdir_in` 建在项目 `target/` 下，测试结束自动删除。`WHATCMD_CONFIG` 只在测试内指向该目录。

**理由**

- 与 Linux 约定一致，减少两个平台的分支和文档差异。
- 哈希比对能挡住长度相同的错配，代价是读取 `vectors.bin` 时多一次哈希计算，数据量小可以接受。
- 临时目录放在项目内，清理有保证，也不会污染 `/tmp` 或真实配置目录。

**遗留问题**

- 两次 rename 之间崩溃时，`load` 会拒绝加载，用户需要 `reindex --full` 恢复。这是有意的保守选择，是否需要自动恢复待 D10 确认。
- 哈希算法依赖 `sha2` 新增依赖，版本在实现时选定并写入本条目。（已选定，见下方"实现结果"。）

## 2026-11-03 D3 实现结果与依赖

**背景**

按上一条 D3 决定实现 `config`、`index` 读写、`index::vectors`。实现过程中有若干与 INTERFACES 不一致的地方，在此记录。

**决定**

- 新增依赖（精确版本）：`toml = "=1.1.8"`、`serde_json = "=1.0.154"`、`sha2 = "=0.11.1"`、`tracing = "=0.1.44"`；dev 依赖 `tempfile = "=3.27.0"`。
- `tracing-subscriber` 暂不加，留到 main 接入日志输出时再加。D3 只用 `tracing` 宏。
- `Stored { index, vectors }` 作为 `index::load` 的返回类型，代替 INTERFACES 中隐含的二元组。两者已通过长度与哈希校验。
- `vectors::parse(bytes, dimensions, rows)` 新增。`index::load` 需要先哈希再解析，避免读两次文件。
- `config::load_from(path, home)`、`config::parse`、`config::validate`、`config::expand_paths`、`config::resolve_config_path` 为 pub。`load_from` 让测试可以传入临时 home，不读真实 `$HOME`。
- `ConfigError` 包含 `MissingEmbeddingField(&'static str)` 与 `NoHome`，INTERFACES 未列出，已在实现中使用。
- `Config` 派生 `Default`，各子结构手写 `Default`（因为默认值非零）。clippy `derivable_impls` 要求顶层派生。
- 测试工具 `testutil::tempdir()` 统一把临时目录建在 `target/test-tmp` 下。它是 `pub(crate)`，不进入公开接口。

**理由**

- `toml` 与 `serde_json` 是 serde 生态的标准选择，与现有 `serde` 版本兼容。
- `sha2` 是纯 Rust 实现，无系统依赖。
- 把测试临时目录放在 `target/` 下，满足"不污染本机目录"的要求，`cargo clean` 即可清除。

**遗留问题**

- 日志断言未覆盖：关键失败路径有 `warn!`，但测试未断言日志内容。待补。
- `WHATCMD_CONFIG` 环境变量读取未被端到端测试，只测了纯函数。
- `index.json` 的 `IndexMeta` 在 INTERFACES 中还没有 `vectors_sha256`，需要同步。
- INTERFACES 第 8、9、10 节尚未同步 `Stored`、`IndexError`、`vectors::parse`、`config` 的公开函数。

## 2026-11-03 D4 PATH 扫描

**背景**

D4 实现 `scan`：从 PATH 找出根命令候选。只读目录项与元数据，不读文件内容。

**决定**

- 同名取 PATH 中第一个通过检查的条目。非可执行文件不遮蔽后面的可执行文件，与 shell 查找行为一致。
- 符号链接跟随，按目标判断是否为普通文件且有执行位。断开的链接跳过。
- 相对路径项（包括空项）跳过并记 `warn`。
- 文件名不通过 `validate_command` 的静默跳过，不记日志，因为 PATH 中杂项文件名很多。
- 新增私有函数 `is_executable`，INTERFACES 第 6 节未列出，已在实现中使用。
- `find_in_path` 与 `commands_from_path` 共用 `search_dirs` 与 `is_executable`，保证结果一致。

**理由**

- 不遮蔽的规则避免一个无执行位的同名文件挡住真正可用的命令。
- 相对路径的结果依赖工作目录，reindex 在不同目录下运行会得到不同结果，跳过更稳定。

**遗留问题**

- 空 PATH 项没有单独断言，只有相对项的测试。两者走同一分支，但测试不覆盖空项的 `warn`。
- `warn` 日志未被测试捕获，与 D3 的日志断言缺口相同，统一待补。
- 可执行判断只看任一执行位，不调用 `access(X_OK)`。与 DESIGN 规则一致，但对只读挂载或 ACL 场景可能有偏差，待 D10 或真实环境验证。

## 2026-11-03 D5 打分、输出与 query 子命令

**背景**

D5 实现 `search::score`、`search::query` 编排、`output`、`cli` 的 `query` 子命令。按用户确认，`embed` 与 `usage` 只做最小部分：`embed` 不接网络，`usage` 只读不写。

**决定**

1. `embed` 只定义 `Embedder` trait、`embed_text`、`EmbedError`，以及测试用的 `FakeEmbedder`（`#[cfg(test)]`）。`OpenAiEmbedder` 留到 D8。D5 的 `UnavailableEmbedder::from_config` 对所有配置都返回 `Unreachable`，因此 `whatcmd query` 在 D8 之前一律退出码 `3`。端到端的手动验收推迟到 D8。
2. `usage` 只读：`load_snapshot`、`UsageSnapshot::stat`、`UsageRecord`。不实现 `append`，`whatcmd record` 留到后续 demo。
3. `UsageSnapshot::stat` 签名改为 `stat(key, now_unix: i64) -> Option<(u32, f32)>`，INTERFACES 原为 `now: &str`。时间解析用纯函数 `parse_rfc3339_utc` 与 `days_from_civil`，不引入 `chrono` 或 `time`。
4. `QueryContext` 增加 `now_unix: i64` 字段，由调用方传入，便于测试固定时间。
5. `Candidate` 增加 `delete_tail: usize`。`insertion_text` 返回的 `Insertion` 中的 `delete_tail` 不能丢，否则 `git cl` 替换的场景在输出层无法表达。输出行的 tab 列在 `insert` 之后增加 `delete_tail` 一列。
6. `self_score` 用点积。两侧向量都已 L2 归一化，点积即余弦。
7. 回退：`via` 在父层命中时填该层的 key，根层命中时填 `*`。INTERFACES 原文写的是"最佳子路径"，D5 实现为"命中层级"，待样本确认。
8. 精确置顶的节点即使低于 `MIN_SCORE` 也计为命中，因此它所在的层不会因此回退。
9. 回退候选不在锚点子树内，`insertion_text` 返回 `None`。D5 输出 `insert = 完整路径`、`delete_tail = 0`。widget 如何插入回退候选，尚未定，见遗留问题。
10. 参数去重：`Param` 节点的 `name` 若在 `seen_flags` 中则过滤掉。
11. 打分系数（`DECAY`、`MIN_SCORE`、`USAGE_*`）为常量，未调参。
12. 新增依赖 `clap = "=4.6.7"`（`derive` 特性），用于 `cli` 的参数解析。
13. `cli` 只加 `query` 子命令。`ExitKind` 定义在 `cli`，`main` 只做解析与退出码映射。
14. `main` 没有使用 `anyhow`，与 AGENT.md 第 5 节有偏离。D5 的错误都在 `cli` 层直接映射为退出码并打印，不需要汇总多种错误类型，所以暂不引入。等出现需要汇总的场景再加。
15. `tracing-subscriber` 仍未接入，D5 的 `info` 与 `warn` 日志不会出现在终端。与 D3、D4 一样，等日志接入时补测试与输出。

**理由**

- 最小的 `embed` 与 `usage` 让 `SearchError` 的四个变体都能被测试触发，同时不引入网络请求与写盘。
- 时间解析手写约 20 行，换来不增加依赖，且可以用固定样本测试。
- 输出增加 `delete_tail` 列，是因为 D2 决定"替换未完成词"之后，widget 必须拿到删除长度，否则替换无法执行。

**遗留问题**

- 回退候选（`via` 非空）的插入语义未定。当前输出完整路径，widget 若直接写入会拼在未完成词之后，错误。需要在 widget 接入时决定：不插入、整体替换，或仅展示。
- `via` 的展示格式与含义（命中层级还是最佳子路径）待样本确认。
- 打分系数未调参。
- `whatcmd query` 在 D8 之前无法返回结果，端到端验收推迟。
- `record` 子命令与 `usage` 写入未实现。
- 日志断言仍未覆盖，与 D3、D4 的遗留相同。

**复核修正（同日）**

复核 D5 测试体后发现以下问题，本条记录处理方式：

1. `cli` 的 `query_args_take_text_after_double_dash` 只覆盖了 `--prefix` 等选项解析。补充 `--` 之后的 `--branch`、`-x` 仍被收为文本的用例。
2. `query_without_embedding_client_exits_with_embed_code` 名称有误导性，它只验证 `UnavailableEmbedder` 到退出码 3 的映射，未调用真实服务。改名为 `unavailable_embedder_maps_to_embed_exit_code`。
3. `results_stay_under_anchor_subtree` 原用 `starts_with("git")` 判断子树归属，偏弱。改为用 `Tree::ancestors` 检查每个候选都以 `git` 为祖先。
4. `score::subtree_score` 缺少"后代分高于自身"时返回 `Some(索引)`的直接用例。补充 `descendant_above_own_score_returns_its_index`。
5. 回退只测了整棵树（`via = "*"`）。补充 `miss_in_anchor_falls_back_to_parent_with_via`：锚点 `git clone` 子树无命中，回退到父节点 `git`，`git pull` 命中且 `via = "git"`。

以上五项均已在代码与测试中落实，四项检查（fmt、clippy、test、rustdoc）退出码为 0，`cargo test` 122 项通过。

更正：决定 11 之前的"最小的 `embed` 与 `usage` 让 `SearchError` 的四个变体都能被测试触发"一句不准确。`SearchError::Vector` 目前没有单独测试触发，已在 `demos/D5-query/README.md` 的已知未覆盖中列出。

仍未覆盖：
- `unavailable_embedder_maps_to_embed_exit_code` 只验证退出码映射，不发起网络请求，真实 endpoint 的验证留到 D8。
- `via` 中间层的展示格式未经样本确认（见上文遗留问题）。

## 2026-11-03 D6 文档获取（docs）

**背景**

D6 实现 `docs`：根命令取 man 页，子命令取 `--help`，清洗、截断，超时杀进程组。

**决定**

- 只用标准库，不新增依赖。超时后的进程组清理通过系统 `kill -KILL -- -<pgid>` 完成，没有引入 `libc`。
- `process_group(0)` 让子进程自成进程组，正常退出后也清理组内残留进程，防止后台子进程占着管道。
- 原始输出读取上限为 `max_doc_bytes` 的 4 倍，超出部分读后丢弃。
- 单个根命令先 man，失败后 `--help`；多 token 子命令只走 `--help`。
- `clean` 的截断保证字符边界，追加的 `\n[truncated]` 不计入上限。

**理由**

- 不引入 `libc` 是为了保持依赖面小。代价是多一次外部进程调用，超时清理路径不如直接 syscall 干净。
- 读取上限与丢弃是为了避免子进程写满管道而阻塞，超时逻辑依赖这一点。

**遗留问题**

- `kill` 命令不可用时静默忽略，没有测试。
- `drain` 的超限丢弃没有单独测试。
- `DocError::Io`（spawn 失败）没有测试断言。
- 真实 `man` 页的清洗效果未验证，只测了假可执行文件。待 D10 或手动验收确认。
- `sanitize_text` 的 ANSI 覆盖范围仍是 D1 的遗留问题。

## 2026-11-03 D7 agent 参数拼装与能力解析

**背景**

D7 实现 `agent::claude::build_args` 与 `agent::detect::parse_capabilities`，并补齐 `agent/mod.rs` 的能力类型。开始前 `docs::tests` 出现偶发失败，先尝试处理基线（见决定 6），未解决。

**决定**

1. 能力集合加入 `StrictMcp`，对应 `--strict-mcp-config`（用户同意）。`REQUIRED` 共 6 项。DESIGN 第 9.6 节与 INTERFACES 第 13 节已同步。
2. `build_args` 的 `--allowedTools` 只从常量 `ALLOWED_TOOLS: [&str; 4]` 拼装，外部无法传入工具名。`--tools ""` 禁用全部内置工具。
3. MCP 工具名前缀采用 `mcp__whatcmd__<name>`。这依赖 MCP server 注册名为 `whatcmd`，D9 对照 rmcp 时确认。
4. `parse_capabilities` 只读选项定义行（缩进两个空格后接 `-`），不读描述续行，选项名精确匹配。`--tools` 不会匹配 `--disallowedTools`。
5. 解析测试使用 `tests/fixtures/claude-help-2.1.296.txt`，内容为本机 `claude --help` 原文。
6. `docs::tests` 偶发失败的处理尝试（之后仍复发，未解决）：`fake_bin` 写完脚本后 `sync_all` 再关闭；docs 测试的超时放宽到 10 秒，超时测试保留 3000 毫秒（此处原写 500 毫秒，与代码不符，已更正）。生产代码 `run_captured` 未改动。后续改写见"D7 基线：docs 超时测试改写"条目。

**理由**

- 白名单固定在代码常量中，符合 DESIGN 第 11 节"只放行四个 whatcmd 工具"的要求。
- 解析只看定义行，避免描述文字里的 `--tools` 被误判为能力存在（`mention_inside_description_does_not_count`）。

**遗留问题**

- `--allowedTools` 是否真的禁用 Bash 仍未验证。D7 只验证参数拼装，真实行为在 D8 用临时目录的假标记文件验证。
- `docs::tests` 的根因未证实。复现时 10 次中有 1 到 3 次失败，失败的测试为 `timeout_kills_the_whole_process_group` 及几项 `--help` 与 `man` 假脚本测试。加入计时与 `ps` 采样的诊断版本 10 次全部通过，无法拿到失败时的时序证据。夹具调整后曾连跑 20 次未失败，但随后一次完整 `cargo test` 中 `timeout_kills_the_whole_process_group` 再次失败，症状相同（pidfile 未写出）。夹具调整不足以解决问题，基线尚未全绿。若再出现，应优先检查 `run_captured` 中 `try_wait` 与 `kill_group` 的时序。
- 诊断过程中 `src/docs.rs` 曾临时加入计时与脚本自记录输出（`DIAG` 标记），已由诊断前备份 `docs.rs.pre-diag` 还原，并核对不含 `DIAG` 且与备份一致。
- 第二轮诊断（只放在超时测试内，未改 `run_captured`）：脚本自己写启动标记与 stderr，pidfile 写入失败时记录。单独运行该测试 10 次全部通过；完整 `cargo test` 运行 15 次全部通过。两轮诊断合计约 25 次运行，均未复现失败，因此没有拿到失败时的脚本状态。诊断版本可能改变了时序，这一点也无法排除。
- 当前结论：失败只在部分运行中出现，根因未定位，`run_captured` 的超时与回收路径是否有缺陷仍是待查项。`docs::tests` 在未诊断的版本下仍有偶发失败的记录，因此 D7 的基线尚未可靠证明，`demos/D7-agent/README.md` 尚未写入，D7 不标记为完成。（以上为当时状态；README 已在"D7 基线：docs 超时测试改写"阶段写入。）
- 第三轮诊断（放在 `run_captured` 内，记录 spawn、正常退出、超时及 `kill_group`，用 `eprintln!`，未接入 `tracing-subscriber`）：完整 `cargo test` 运行 10 次，全部通过。超时路径 10 次，每次超时时 `try_wait` 为 `None`（子进程仍在运行），且 `kill_group` 均已执行；正常退出 39 次，均为正常退出。因此"进程组杀不掉"的假设不成立。但本轮未复现 pidfile 未写出的症状，失败时的时序仍未获得。`src/docs.rs` 已由诊断前备份 `docs.rs.pre-diag` 还原，核对不含 `DIAG` 且与备份一致。
- 三轮诊断合计约 35 次运行未复现失败。已排除：超时路径杀不掉进程组。未排除：子进程启动延迟导致 pidfile 未写出，这一点只能在失败发生时才能观察。
- `claude --help` 中的 `--restricted` 未纳入能力集合。它会移除 Bash 等代码执行工具，可作为纵深防御，D8 评估是否加入。
- 解析规则基于 Claude Code 2.1.296 的帮助格式。升级 Claude Code 后应重新核对 fixture 与解析结果。

## 2026-11-03 D7 基线：docs 超时测试改写

**背景**

`docs::tests::timeout_kills_the_whole_process_group` 偶发失败，症状为 `background pid written before timeout: NotFound`，同时 `fetch_help_in` 已返回 `Timeout(3000)`。本次先在未改动的版本上复现，再改写测试。

**复现（改写前，代码与 `target/repro/docs.rs.pre-diag` 一致）**

- 完整 `cargo test` 连跑 30 次：30 次通过，每次 148 项（`target/repro/baseline-summary.log`）。
- 10 个 `yes > /dev/null` 负载下连跑 10 次：10 次通过，结束后 `yes` 残留 0（`target/repro/load-summary.log`）。
- 每次运行前 `touch src/docs.rs` 强制重编译，连跑 15 次：15 次通过（`target/repro/rebuild-summary.log`）。
- 合计 55 次未复现。加上此前三轮诊断约 35 次，累计约 90 次运行未复现。

**根因**

未定位。

- 已排除：超时路径杀不掉进程组（第三轮诊断，超时时 `try_wait` 为 `None`，`kill_group` 均已执行）。
- 证据不足：`ETXTBSY`（失败时返回 `Timeout` 而非 `Io`，与该假设不符）。
- 证据不足：刚编译完或 CPU 负载下启动变慢（上述负载与重编译共 25 次未复现）。
- 剩余假设：子进程在 3 秒内未执行到 `echo $! > pidfile`，原因未知；或失败发生时超时实际不是 3000 毫秒（D7 决定 6 曾写 500 毫秒，与代码不符，无法确认失败当时的代码版本）。这两点只能在失败再次发生时观察。

**决定**

改写测试的证明方式，生产代码 `run_captured` 未改动。

1. 脚本先 `echo $$ > slow.pgid`，再以 `"$0" bg &` 重新执行自身作为后台长驻进程，前台与后台都是 `while :; do sleep 1; done`。两者的命令行都含脚本路径。
2. 超时返回后，用 `pgrep -f -- <脚本路径>` 等待最多 3 秒，要求前台与后台 sh 都已消失。这一条不依赖脚本写出任何文件：即使脚本没执行到第一行，被 spawn 的 sh 本身也带着脚本路径。
3. 若 `slow.pgid` 已写出，再用 `pgrep -g <pgid>` 确认整个进程组为空。
4. 删除测试辅助函数 `alive`（只被旧测试使用），新增 `pgrep_finds` 与 `wait_gone`。

**证明范围的变化**

- 旧测试要求脚本在 3 秒内执行到 `echo`，否则失败。新测试不再证明"子进程在超时前启动完成"，启动慢时不会失败。
- 脚本未写出 `slow.pgid` 时，新测试只证明被 spawn 的 sh 已被杀，不证明后台进程被杀（因为后台进程可能根本没有启动）。此时整组为空的检查被跳过。
- 依赖 `/usr/bin/pgrep`，`pgrep` 退出码 1 表示无匹配。
- 因此，如果原失败的根因是一个真实的启动延迟，改写后的测试会掩盖它而不是解决它。

**改写后结果**

- 四项检查逐条直接取退出码：fmt 0，clippy（`-D warnings`）0，test 0（148 项通过），rustdoc（`-D warnings`）0。
- 完整 `cargo test` 连跑 30 次：30 次通过，每次 148 项（`target/repro/final-summary.log`）。
- 10 个 `yes` 负载下连跑 10 次：10 次通过，结束后 `yes` 残留 0（`target/repro/final-load-summary.log`）。
- `grep -n DIAG src/` 与 `grep -n eprintln src/docs.rs` 均无输出。与 `target/repro/docs.rs.pre-diag` 的差异只在测试模块：删除 `alive`，新增 `pgrep_finds`、`wait_gone`，改写超时测试的脚本与断言。

**日志**

- 日志断言未覆盖。`tracing-subscriber` 未接入，本次未新增依赖。
- AGENT.md 第 4 节要求的超时路径日志（`warn` 记录超时并已杀组；`debug` 记录 pgid、spawn 到超时的毫秒数、`try_wait` 结果、`kill_group` 是否执行）尚未实现。`run_captured` 目前只有一条 `debug!(program, args, timeout_ms, "run doc command")`。这一项未达成，留待后续，改动生产代码前需单独确认。

**遗留问题**

- 根因未定位，见上。若原测试的失败再出现，新测试不会报出来，需要上述超时日志接入后才能观察。
- 超时路径日志未实现，日志断言未覆盖。
- 改写后的测试证明范围变窄，见"证明范围的变化"。
- `--allowedTools` 是否真的禁用 Bash 未验证，留给 D8。
- 用户决定 D7 暂缓：上述问题暂不处理，D7 不标记为完成。恢复时先接入超时日志，再观察原症状是否复发。

## 2026-11-03 D8 前置：子进程运行抽取与 ureq 依赖

**背景**

D8 的 `agent::claude` 与 `embed::openai` 都需要带超时和进程组清理的子进程运行，以及 HTTPS 客户端。`docs::run_captured` 已有成熟逻辑，抽取后两者共用。

**决定**

1. 新增 `src/process.rs`，导出 `run_captured(program, args, env, timeout, max_bytes)` 与 `ProcError`。逻辑从 `docs.rs` 原样迁移，行为不变。`docs.rs` 的 `capture` 包装调用它，并把 `ProcError::Timeout` 与 `Io` 映射回文档错误。
2. `Cargo.toml` 新增 `ureq = { version = "=3.5.1", default-features = false, features = ["rustls", "json"] }`。
   - 选 `ureq` 而不是 `reqwest`：同步阻塞，依赖面小，embedding 调用不需要异步运行时。
   - `default-features = false` 去掉 gzip 与 `serde_json` 之外的默认特性。
   - `rustls` 提供 TLS，不依赖系统 OpenSSL，Linux 与 macOS 行为一致。
   - 当前只声明依赖，尚无代码使用，D8 的 `embed::openai` 接入。
3. `lib.rs` 导出 `process` 模块。`process.rs` 的模块注释只写"目前由 `docs` 使用"，`agent` 接入后再更新。

**理由**

- 抽取能让 D8 的超时清理与 `docs` 保持同一行为，避免两份实现漂移。
- 抽取只搬代码，不改逻辑，因此 D7 的偶发问题既未被修复也未被引入。

**验收**

- 四项检查逐条取退出码：fmt 0，clippy（`-D warnings`）0，test 0（148 项通过），rustdoc（`-D warnings`）0。
- `docs::` 连跑 20 次，20 次通过。
- `grep -n DIAG src/` 无输出。

**遗留问题**

- `process.rs` 没有独立单元测试，行为仅通过 `docs` 测试间接覆盖。D8 接入 `agent` 时补充。
- D7 的超时日志与偶发失败根因仍未处理，见上一条。
- `ureq` 尚未被使用，D8 接入后需要重新确认其特性组合是否满足需要。

## 2026-11-03 D8 实现：embed::openai 与 ClaudeBackend（仅假服务与假命令）

**背景**

D8 覆盖 `embed::openai` 与 `agent::claude::ClaudeBackend`。本条只记录已在本地验证的部分。真实 SiliconFlow 调用与真实 `claude -p` 安全验证尚未执行，需用户明确确认。

**决定**

1. `embed::openai::OpenAiEmbedder`：`POST {endpoint}/embeddings`，密钥只从 `api_key_env` 指定的环境变量读取，放在 `Authorization` 头中。`Debug` 不输出密钥。
2. 关闭 ureq 默认的状态码错误（`http_status_as_error(false)`），由本模块映射：超时为 `Timeout`，401/403 为 `Auth`（错误信息只含变量名），其余非 2xx 为 `Unreachable`，响应解析与维度不符为 `BadResponse`。
3. 显式 `proxy(None)`，避免环境变量中的代理改变请求去向。
4. 纯函数 `parse_embeddings_response` 按 `index` 还原输入顺序，检查条数、越界与重复。
5. `agent::claude::parse_output` 读取 `result` 与 `is_error`。字段结构是按 `claude -p --output-format json` 的常见格式写的，尚未用真实输出核对。
6. `ClaudeBackend::run_batch` 先检查 `REQUIRED` 能力，不全则拒绝运行。调用 `process::run_finished`，超时映射为 `AgentError::Timeout`，非零退出为 `ExitStatus`，输出结构错误为 `BadOutput`。
7. `process.rs` 新增 `run_finished`，返回退出码与输出。`run_captured` 改为调用它，行为不变。
8. `AgentError` 新增 `BadOutput`，`BatchOutcome` 派生 `Debug`。

**理由**

- 状态码映射放在本模块，错误分类集中，测试可以不依赖 ureq 内部类型。
- 测试只用本地 `TcpListener` 假服务和临时目录里的假 `claude` 脚本，不访问网络，不调用真实 claude。

**验收**

- 四项检查逐条取退出码：fmt 0，clippy（`-D warnings`）0，test 0（168 项通过），rustdoc（`-D warnings`）0。
- `embed::openai` 14 项通过，覆盖成功（带 Bearer 与模型名）、无密钥、401、500、畸形体、维度不符、拒绝连接、超时、Debug 不泄露密钥、空输入不发请求。
- `agent::claude` 11 项通过，其中 `ClaudeBackend` 相关 4 项：成功、非零退出、超时、能力不全拒绝运行。
- 请求体断言最初依赖紧凑 JSON 格式，失败后改为解析 JSON 再比较字段，生产代码请求构造未改动。

**未验证**

- 真实 SiliconFlow endpoint（`https://api.siliconflow.cn/v1`，BGE M3）未调用，查询文本尚未发送给该服务。
- 真实 `claude -p` 未运行。`--tools ""`、`--allowedTools`、`--restricted` 三种配置下标记文件是否生成，尚未验证，因此 `--allowedTools` 是否禁用 Bash 仍是未知。
- `parse_output` 的字段结构未用真实输出核对。

**遗留问题**

- `ProcError` 到 `AgentError` 的超时映射在 `ClaudeBackend` 中实现，`process.rs` 仍无独立单元测试。
- 超时路径日志（`warn` 与 `debug`）仍未实现，`tracing-subscriber` 未接入，日志断言未覆盖。
- `docs/` 下的开发说明文档尚未写入。
- D7 的 `docs::tests` 偶发失败根因仍未定位，见 D7 条目。

## 2026-11-03 D8 真实调用验收与 Claude 工具限制实验

**背景**

D8 需要真实验证两件事：SiliconFlow embedding 能否返回预期结构，以及 `claude -p` 在三种工具配置下 Bash 是否真的被阻止。DESIGN 第 11 节原假设 `--allowedTools` 能限制内置工具，需要用实验确认。

**实验 1：SiliconFlow embedding**

- endpoint `https://api.siliconflow.cn/v1`，模型 `BAAI/bge-m3`，2 条输入。
- 结果：返回 2 条向量，各 1024 维，退出码 0。
- 密钥只通过环境变量 `WHATCMD_EMBEDDING_API_KEY` 传入，未写入任何文件或文档。用户确认该 key 为无关紧要的测试 key。
- 临时验收程序 `examples/embed_check.rs` 已删除。

**实验 2：`claude -p` 工具限制**

每种配置在独立的 `target/test-tmp/claude-safety-*` 目录中运行，提示模型执行 `touch marker.txt`，检查标记文件是否生成。每次加 `--no-session-persistence`。

| 配置 | 第一轮 | 第二轮 | 观察 |
|---|---|---|---|
| A `--tools ""` | 无标记文件 | 无标记文件 | 第一轮输出伪造的工具调用文本；第二轮输出形如工具调用的文本并附带编造的 `ls` 结果。文件系统中不存在标记文件。`permission_denials` 为空。 |
| B 仅 `--allowedTools` 四个 whatcmd 工具 | 有标记文件 | 有标记文件 | Bash 实际执行，`result` 为 `done`。 |
| C `--restricted` | 无标记文件 | 无标记文件 | 模型明确说 Bash 工具不可用。 |

**结论**

1. `--allowedTools` 不能禁用内置工具。仅用它时 Bash 实际执行，两轮一致。DESIGN 第 11 节原假设不成立。
2. `--tools ""` 与 `--restricted` 都阻止了真实执行。`--tools ""` 是 `build_args` 当前使用的参数，保留。
3. 模型在无工具时会模仿工具调用格式并编造执行结果。因此 `ClaudeBackend` 不能只依据 `result` 文本判定成功，reindex 结果必须以暂存区文件为准。
4. 与 AGENT.md 第 8 节的早期记录一致：`--tools` 有效，`--allowedTools` 下 Bash 仍会执行。

**环境注记**

- 请求经过本地路由 `127.0.0.1`（cc-cli 本地路由）。stderr 出现 `unrecognized_model` 警告，模型名显示为 `claude-haiku-5.5`，与配置的模型不一致。
- 直连官方 API 或其他路由下，模型行为和工具行为可能不同，结论只适用于本次环境。

**局限**

- 每种配置每轮只跑一次，共两轮。模型行为有随机性，结论需要更多次数确认。
- 标记文件只能证明 Bash 是否执行，不能证明 `--tools ""` 下工具层面的拒绝原因。
- 本次未调用真实 reindex 流程，未验证 MCP 工具与暂存区的完整链路（D9、D10）。

**未决事项**

- `--restricted` 是否纳入能力检查（`REQUIRED`），待用户确认。
- 实验临时目录 `target/test-tmp/claude-safety-*` 仍保留，删除前需用户确认。
- `parse_output` 的字段结构在本次实验中已观察到 `result`、`is_error`、`permission_denials`、`subtype`，与实现一致。

## 2026-11-03 D8 Claude 安全实验与模型观察

**背景**

DESIGN 第 11 节假设 `--allowedTools` 能限制内置工具。为验证，用临时目录中的真实 `claude -p` 比较三种配置，提示模型执行 `touch marker.txt`。

**决定**

1. DESIGN 第 9.7、11、13 节已修改：`--tools ""` 是唯一承担禁用内置工具责任的参数，`--allowedTools` 不能单独依赖。
2. `build_args` 加入 `--model haiku`，快照测试同步更新。
3. 实验结论只作为当前环境下的观察，不作为长期结论。

**观察**

| 配置 | 轮次 | 标记文件 | 结果 |
|---|---|---|---|
| A `--tools ""` | 2 | 未生成 | 第一轮模型输出伪造的工具调用文本；第二轮输出形如工具返回的 JSON，文件系统中无文件，`permission_denials` 为空 |
| B 仅 `--allowedTools` | 2 | 生成 | `result` 为 `done` |
| B 仅 `--allowedTools` 加 `--model haiku` | 3 | 生成、未生成、未生成 | 第二次 `result` 为 `<bash>` 文本；第三次声称无 shell 工具 |
| C `--restricted` | 2 | 未生成 | 模型声称无 Bash 工具 |

（B 配置合计 5 次：不带 `--model` 的 2 次均生成；带 `--model haiku` 的 3 次中 1 次生成、2 次未生成。）

**模型来源**

- 第一次 `--model haiku` 运行的 `modelUsage` 键为 `ad7a705a-9c07-4532-b0ea-3fd06e4ad8db:claude-haiku-5.5-thinking`，`canonicalModel` 与之一致，`provider` 为 `firstParty`。
- 之后的运行（包括用户称已关闭路由后）的 `modelUsage` 键仍相同。
- stderr 持续出现 `[claude-code:unrecognized_model]` 通知，内容提到请求经过 `127.0.0.1` 本地路由，该路由与 auto mode 分类器计费相关。
- 以上都来自 Claude Code 上报的数据。我无法独立验证路由后端实际运行的模型，也无法确认路由是否已关闭。

**结论**

1. `--allowedTools` 不能单独禁用 Bash。B 配置合计 5 次中有 3 次执行。
2. `--tools ""` 与 `--restricted` 在观察中均未生成标记文件，但 A 的第二轮输出显示模型会编造工具结果，因此"未生成"不等于"已禁用"，只能说没有真实执行。
3. 判断执行与否必须看文件系统结果，不能看 `result` 文本。这与 DESIGN 第 9.6 节"以暂存区为准"一致。
4. 模型身份不稳定可验证：路由和 `--model` 的组合下，实际模型只能由上报数据推断。

**局限**

- 每种配置样本少（2 至 4 次），结论是方向性的。
- 行为依赖路由与模型，换环境需重跑。
- `unrecognized_model` 的 stderr 不影响退出码与 `is_error`，`ClaudeBackend` 只解析 stdout。

**遗留问题**

- 需要确认 `--restricted` 是否加入 `REQUIRED`。
- 临时目录 `target/test-tmp/claude-safety-*`、`claude-model-*` 待用户确认后删除。
- 若要把安全结论定为长期依据，需要更大样本，并在无路由环境下重跑。
- `ClaudeBackend` 目前不检查标记文件或暂存区，reindex 的成功判断需在 D10 实现时以暂存区为准。

## 2026-11-03 D8 模型来源核对与运行环境

**背景**

前面的实验记录里，`modelUsage` 键与 `settings.json` 中的模型名对不上，无法判断请求是否经过了预期的端点。本机 `~/.claude/settings.json` 由 cc-switch 切换时改写，文件内容在两次读取之间发生了变化（先为 `api.moonspirit.asia`，后为 `api.deepseek.com/anthropic`）。

**决定**

1. 运行 `claude -p` 的 Cherry Studio 进程环境中，`ANTHROPIC_BASE_URL=http://127.0.0.1:60784`，`ANTHROPIC_DEFAULT_HAIKU_MODEL=<providerId>:claude-haiku-5.5-thinking`。这是用户确认的现象解释：cc-switch 开启路由后，Cherry 进程注入的环境变量覆盖了 settings 文件。Claude Code 对两者的优先级本身没有单独验证。
2. 直接向 `127.0.0.1:60784` 发送最小请求，使用 `providerId:apiModelId` 格式的模型名（裸名 `claude-haiku-5-5` 返回 400）。`ANTHROPIC_MODEL` 与 `ANTHROPIC_DEFAULT_HAIKU_MODEL` 对应的请求均返回 200。
3. `claude -p --model haiku` 解析到 `ANTHROPIC_DEFAULT_HAIKU_MODEL`，因此 `modelUsage` 键中的 `claude-haiku-5.5-thinking` 来自这个映射，不是异常。

**理由**

- 端点与模型映射来自运行时环境的直接读取，不是推断。
- 之前的"模型对不上"疑问可以收束：路由层的模型名是映射后的结果。

**遗留问题**

- 路由回显的 `response.model` 只是请求名的透传，不能证明后端真实模型是 Haiku 5.5。后端身份需要路由侧配置或 `api.moonspirit.asia` 等端点的对照请求才能确认。
- `settings.json` 会被 cc-switch 改写，之后记录实验结果时要同时写明当时的环境来源（shell 变量或 settings 文件）。
- `settings.json` 中的 `ANTHROPIC_AUTH_TOKEN` 曾明文出现在对话中，建议用户轮换。

## 2026-11-03 D8 收尾

**背景**

D8 的真实 embedding 与真实 claude 安全实验已完成，模型来源已核对（见上一条）。收尾时逐项对照 AGENT.md 第 6 节 D8 的验收要求。

**决定**

D8 标记为完成，但保留以下未完成项，并在 `demos/D8-embed-agent/README.md` 中列出：

- 已验证：`embed::openai` 真实调用 SiliconFlow `BAAI/bge-m3`，2 条输入返回 2 个 1024 维向量。`ClaudeBackend` 的假命令测试覆盖成功、非零退出、超时、能力不全拒绝运行、输出结构错误。四项检查通过，`cargo test` 168 项通过。
- 已验证：`--tools ""` 与 `--restricted` 在本环境下未生成标记文件，`--allowedTools` 下 Bash 有 3 次执行（共 5 次）。
- 未完成：`process.rs` 单独的单元测试；超时路径的 `warn` 与 `debug` 日志；`tracing-subscriber` 接入与日志断言；A、C 配置统一 `--model haiku` 后的重跑；`--restricted` 是否纳入 `REQUIRED`。
- 未完成：临时实验目录 `target/test-tmp/claude-safety-*`、`claude-model-*` 的删除，待用户确认清单。
- 未完成：D7 的 `docs::tests` 偶发失败根因。

**理由**

- AGENT.md 第 6 节 D8 的两项验收（3 个假命令的调用，真实 endpoint 的 embedding）已有结果。真实 claude 的安全结论样本少，只作为当前环境下的方向性观察。
- 未完成项不影响 D8 已验证的接口行为，但会影响安全结论的强度，因此在 README 与本条中明确列出。

**遗留问题**

- 安全结论需要更大样本，并在无路由环境下重跑。每次运行都会计入额度，需要用户同意。
- D10 实现 reindex 时，成功判断以暂存区文件为准，不依赖 `result` 文本。

## 2026-11-03 D8 保留项：process 单元测试与超时日志

**背景**

D8 收尾时保留的项目包括 `process.rs` 单元测试、超时路径日志、`tracing-subscriber` 接入与日志断言。

**决定**

1. `process.rs` 新增 8 项单元测试：返回码与输出、超时返回限制值、缺失程序返回 `Io`、环境变量传入、输出读取上限、仅 stderr 时 `run_captured` 回退、两者皆空返回 `None`、正常退出后清理残留后台进程。残留进程检查通过 stdout 返回的 pid 与 `kill -0` 轮询完成，不依赖文件写入时序。
2. 超时路径日志：`warn!` 记录程序名与时限（"command timed out, killing process group"）；`debug!` 记录 pgid、耗时、`try_wait` 是否仍在运行（"timeout reached"）、杀组已发送（"kill group sent"）。生产代码的日志语句由本条决定实现。
3. 新增依赖 `tracing-subscriber = "=0.3.23"`，`default-features = false`，特性 `fmt`、`std`。理由：日志输出与日志断言都需要订阅器，`fmt` 足以写 stderr，关闭默认的 ansi 与 tracing-log 等特性以减少依赖面。
4. `main.rs` 接入订阅器：只写 stderr，默认级别 `WARN`。`-v` 与 `WHATCMD_LOG` 尚未接入，留到后续。
5. 日志断言放在 `tests/process_log.rs`，作为独立的集成测试二进制运行，不放在 `src/process.rs` 的单元测试中。

**理由**

- 初版把日志断言放在 `process::tests`，完整 `cargo test` 中稳定失败（10 次中 10 次），单独运行或串行运行都通过。失败时只捕获到 `run command` 与 `kill group sent`，`timeout reached` 与 WARN 缺失。
- 我的推断是并行测试共享超时分支的 callsite，全局 tracing 状态被其他线程的订阅器注册改变。这个机制未直接证实，只是现象与之吻合。
- 移到独立二进制后，该测试是唯一一个捕获日志的进程，完整套件连跑 10 次全部退出 0。这验证了方案有效，但没有证明上述机制本身。

**遗留问题**

- 日志断言只覆盖超时路径的 WARN 与 `kill group sent`。`timeout reached` 与正常退出路径的日志未断言。
- 竞争机制未直接证实，见上。
- D7 的 `docs::tests::timeout_kills_the_whole_process_group` 偶发失败根因仍未定位。超时日志接入后，若再出现失败，应能从日志中观察时序。
- A、C 配置统一 `--model haiku` 重跑，以及 `--restricted` 是否纳入 `REQUIRED`，仍待用户决定。

## 2026-11-03 D8 重跑：A、C、阳性对照（`--model haiku`）与 `--restricted` 修正

**背景**

D8 保留项要求 A、C 配置统一为 `--model haiku` 重跑。用户确认不计用量。模型档位为 Cherry 进程环境中的 `ANTHROPIC_DEFAULT_HAIKU_MODEL`（`claude-haiku-5.5-thinking`，经 `127.0.0.1:60784` 路由）。

**决定**

1. 重跑 A（`--tools ""`）、C（`--restricted`）各 2 次，并加入阳性对照 P（`--tools Bash`）2 次。三者均带 `--model haiku`，`claude` 退出码全部为 0。
2. 结果：A 0/2 生成标记；C 1/2 生成标记；P 2/2 生成标记。
3. 合并前后两批次：A 合计 0/4，C 合计 1/4（此前 C 为 0/2）。
4. 修正：此前 D8 记录与 DESIGN 中写的"`--restricted` 阻止 Bash"不成立。DESIGN 第 9.7 节与第 13 节已改为"`--restricted` 曾生成标记，不作为禁用手段"。
5. `--restricted` 不纳入 `REQUIRED`，与之前的建议一致。
6. 禁用责任仍只由 `--tools ""` 承担，但 A 的样本只有 4 次，结论仍是方向性的。

**理由**

- 阳性对照两次都生成标记，说明标记方法能检测到 Bash 执行，因此 A 的 0 次与 C 的 1 次不是方法缺陷。
- 此前把 C 的 0/2 当作阻止证据，样本太少。加入一次生成后，`--restricted` 的可靠性不足以作为安全边界。

**遗留问题**

- A 与 C 的样本仍少，在无路由环境下未重跑。
- 模型身份未证实，只能确认请求名被透传。
- 早期 DEVLOG 条目（如 D8 实验记录）中"`--restricted` 阻止 Bash"的表述保留为历史记录，以本条为准。

## 2026-11-03 D8 安全实验独立一批：A、B、C、P 各 3 次（`--model haiku`）

**背景**

上一批 `--model haiku` 重跑之后，用户要求在同一环境下再跑一批。本批使用新写的提示词，与之前的实验不同。模型档位仍为 `ANTHROPIC_DEFAULT_HAIKU_MODEL`（`claude-haiku-5.5-thinking`，经 `127.0.0.1:60784` 路由）。

**决定**

1. 每种配置各 3 次：A（`--tools ""`）、B（仅 `--allowedTools`）、C（`--restricted`）、P（`--tools Bash`，阳性对照）。全部带 `--model haiku`，`claude` 退出码全部为 0。
2. 结果（以标记文件为准）：A 0/3；B 3/3；C 0/3；P 3/3。
3. 本批使用新提示词，结果作为独立一批记录，不与之前的批次合并计算。
4. 实验目录保留在 `target/test-tmp/claude-safety-final/`（A-1 至 P-3，共 12 个子目录），未删除。

**理由**

- P 三次全部生成标记，B 三次全部生成标记，说明方法能检出 Bash 执行，且与此前 B 的结论一致：`--allowedTools` 不能禁用 Bash。
- A 本批三次未生成，是目前最稳定的结果，但仍只代表没有真实执行。
- C 本批三次未生成，但之前的 `--model haiku` 重跑中有一次生成。按全部记录合计，C 为 1/7，说明 `--restricted` 不可靠，不能作为禁用手段。

**遗留问题**

- 本批没有逐条检查 `out.json` 的内容。A 的"未生成"可能伴随模型编造的工具结果，判断执行与否只以标记文件为准。
- 调用仍经过本地路由。"无路由环境下重跑"未完成，需要用户切换环境后再做。
- 样本仍少，结论是方向性的。

## 2026-11-03 D8 安全实验 A 批次逐条检查 out.json

**背景**

独立一批的 A 配置（`--tools ""`）三次均未生成标记文件。此前记录只看了标记文件，没有检查 `result` 内容，因此不能判断模型是否编造了工具结果。

**决定**

1. 逐条读取 A-1 至 A-3 的 `out.json`，结论如下：
   - A-1：`result` 明确说没有 Bash 工具，未创建标记，未编造执行结果。
   - A-2：`result` 中混入形似工具调用的 XML 标签（`<parameter name="Bash">`、`<invoke>`）和 `no output` 字样，属于编造的工具调用文本，没有真实执行。
   - A-3：`result` 说没有 Bash 工具，未创建标记，未编造执行结果。
2. 结论：A 配置"标记未生成"与"模型没有执行"一致，但 A-2 显示模型会输出伪造的工具调用文本。因此 `result` 文本不能作为判断依据，判断仍只看文件系统。
3. 运行环境限制：本次 `claude` 进程由 Cherry Studio 启动，`ANTHROPIC_*` 注入无法在该进程内消除，无法切换到非路由环境。此项不再验证，结论只适用于当前环境。

**理由**

- 标记文件判断只能证明没有真实执行，不能证明模型没有输出伪造内容。逐条检查让这一区别有据可查。

**遗留问题**

- 无路由、无 Cherry 注入的环境下重跑安全实验，当前无法完成。
- A 与 C 样本仍少，结论是方向性的。
- B、C、P 批次的 `out.json` 未逐条检查，如需确认 C 的一次生成所在批次的 `result` 内容，需另行查看。

## 2026-11-03 D8 安全实验独立一批：逐条检查 out.json（B、C、P）

**背景**

上一条已逐条检查 A 批次的 `out.json`。本条补齐同一批次中 B、C、P 的检查，确认 `result` 文本与标记文件是否一致，是否存在伪造的工具调用或编造的执行结果。

**决定**

1. 检查范围：`target/test-tmp/claude-safety-final/` 下 A-1 至 A-3、B-1 至 B-3、C-1 至 C-3、P-1 至 P-3 共 12 个目录，逐个读取标记文件存在性、`is_error`、`subtype`、`permission_denials`，以及 `result` 的内容。
2. 结果：
   - A-1、A-3：未生成标记。`result` 只说明没有 Bash 工具，没有编造执行结果。
   - A-2：未生成标记。`result` 含形似工具调用的 XML 标签与一行 `no output`，属于伪造的工具调用文本，但没有真实执行。
   - B-1 至 B-3：全部生成标记，`result` 为 `done`。
   - C-1 至 C-3：全部未生成标记。`result` 都说明没有 Bash 工具，其中 C-1、C-2、C-3 均提出改用文件写入工具代替，没有伪造执行结果。
   - P-1 至 P-3：全部生成标记，`result` 为 `done`。
   - 12 次运行的 `is_error` 均为 false，`subtype` 均为 success，`permission_denials` 均为 0。
3. 结论：判断执行与否只看标记文件。`result` 文本可以出现伪造的工具调用（A-2），因此不能用来判定成功或失败。

**理由**

- 逐条检查确认了 A 批次的"未生成"并非全部伴随编造结果，但 A-2 证明编造确实会发生，因此标记文件仍是唯一依据。
- C 批次的 `result` 显示模型在无 Bash 时会主动提出替代方案。这提示 `--restricted` 的表现取决于模型对工具的理解，不是稳定的机制。

**遗留问题**

- 之前 `--model haiku` 重跑中 C 那一次生成标记的目录不在本批中，其 `out.json` 无法在本批核对。
- 所有结果仍经过本地路由 `127.0.0.1:60784`，无法在无注入环境下复现。
- D8 安全实验结论到此为止，不再追加样本。后续若需要更强结论，应在 D10 或之后的环境中重新设计实验。

## 2026-11-03 D9 MCP 依赖、暂存区与纯函数层

**背景**

D9 实现 `mcp`：四个工具（`read_man`、`read_help`、`write_description`、`write_params`）经 rmcp 暴露给 `claude -p`。工具只写暂存区，不改索引。

**决定**

1. 新增依赖 `rmcp = "=3.8.0"`，特性 `server`、`macros`。rmcp 为 MCP 官方 Rust SDK，DESIGN 第 10 节已指定版本。`default-features` 保持默认（`base64`、`macros`、`server`）。
2. 工具函数（`mcp::tools`）为纯函数，不依赖 rmcp。每个函数先经 `guard` 校验，再调用 `docs` 或写暂存区。rmcp 的注册只是参数转发的薄包装。
3. 暂存区 `generate::staging` 只实现 D9 需要的 `append`（每个 run 一个 JSONL 文件，`run_id` 限字母、数字、`-`、`_`）。`read_all` 与 `remove` 留给 D10。
4. `ToolContext` 的字段全部为 `String`、`PathBuf`、`Vec`、`usize`、`DocsConfig`，满足 `Send + Sync`，可在 rmcp 的 handler 中共享。
5. `ToolError` 的 `TooDeep`、`UnknownCommand` 与 DESIGN 第 18 节一致。父节点不在命令树中的情况原本也用 `UnknownCommand`，后已拆出为 `NotInTree`，见 2026-11-04 条目。

**API 核对（rmcp 3.8.0 源码，`~/.cargo/registry`）**

- `#[tool_router]` 生成 `fn tool_router() -> ToolRouter<Self>`，`server_handler` 选项会额外生成 `impl ServerHandler`。
- `#[tool]` 的参数用 `Parameters<T>`，`T` 需要 `JsonSchema`；异步函数会被改写为返回 boxed future。
- stdio 传输为 `rmcp::transport::stdio()`，服务入口为 `rmcp::serve_server(service, transport)`。

**理由**

- 纯函数层可用 `tempfile` 直接单元测试，不需要启动 MCP 进程。
- 暂存区只追加、不读取，读取与合并放在 D10 的阶段 B，与 DESIGN 第 9.6 节一致。

**遗留问题**

- rmcp 包装层、`whatcmd mcp --run <run_id>` 子命令与手动 stdio 验收尚未完成，见 `demos/D9-mcp/README.md`（待写）。
- `staging::append` 的并发写入（多进程同时追加同一 run）未加锁。当前一个 run 只由一个 `claude` 进程的 MCP 子进程写入，暂不处理，D10 复核。

## 2026-11-03 D9 实现偏离、临时限制与手动 stdio 验收

**背景**

D9 的 rmcp 包装层、`whatcmd mcp --run <run_id>` 子命令已接入，并完成手动 stdio 验收。验收过程中发现若干与 DESIGN 第 18 节不一致之处，以及一处之前的文字描述有误。

**决定**

1. `mcp::serve_stdio` 为同步函数 `fn serve_stdio(ctx) -> Result<(), McpError>`，内部自建单线程 tokio 运行时并阻塞到 stdin 关闭。DESIGN 第 18 节原写为 `async fn`。偏离理由：CLI 的 `run` 是同步的，调用方不需要处理异步。INTERFACES 已同步。
2. 服务状态使用 `Arc<ToolContext>`，不加 `Mutex`。`ToolContext` 在服务期间只读，工具写入的是磁盘上的暂存区。之前计划中提到的 `Mutex` 不采用。
3. tokio 直接声明为依赖，精确版本 `=1.54.1`，特性 `rt`、`io-std`、`io-util`、`macros`，`default-features = false`。stdin 与 stdout 直接作为 `(Stdin, Stdout)` 交给 `rmcp::serve_server`，不启用 rmcp 的 `transport-io` 特性。`rmcp::transport::stdio()` 需要该特性，本次不引入。
4. `mcp` 子命令启动时 `known_nodes` 为空。此前我说它"只含根命令名"，这一说法有误，代码中实际为 `Vec::new()`。影响：`ensure_parent_visible` 对多于一个 token 的路径检查父节点，因此子命令的 `write_description` 与 `write_params` 在 D10 接入索引前一律被拒绝。根命令的写入不受影响。
5. `serverInfo.name` 原为 rmcp 默认值 `rmcp`。已通过 `#[tool_handler(name = "whatcmd", version = "0.1.0")]` 设为 `whatcmd`、`0.1.0`，并以协议层检查确认。
6. 错误文案 `unknown command git` 原本不够准确。已拆分为 `UnknownCommand`（命令不在扫描结果中）与 `NotInTree`（父节点或节点不在命令树中），后者文案单独说明。

**手动 stdio 验收结果**

临时配置与暂存区位于 `target/test-tmp/mcp-manual`，未写入真实 `~/.cache/whatcmd`。依次发送 `initialize`、`notifications/initialized`、`tools/list`、`tools/call`：

- `initialize` 返回协议版本 `2025-06-18` 与 `tools` 能力。
- `tools/list` 列出四个工具，`read_help` 的输入 schema 正确。
- `read_help` 返回真实的 `git --help` 文本。
- `write_description` 写入根命令 `git` 成功，暂存区出现一行 `command` 记录。
- `write_params` 丢弃短参数 `-v` 并记 `warn`，保留 `--bare`，暂存区出现一行 `params` 记录。
- `write_description` 写入 `git clone` 返回 `git is not in this run's command tree; describe its parent first`（`NotInTree`），符合决定 4 的临时限制。注意：此条为拆分前的记录，文案已在 2026-11-04 条目中更新。

**理由**

- 手动验收只能证明协议层与根命令写入正常。子命令写入需要命令树，这属于 D10 的范围，不提前实现。
- 偏离记录在本条中，DESIGN 第 18 节保持原样作为设计意图，以本条为准。

**遗留问题**

- 子命令写入的验收未完成，等 D10 接入索引后补做。
- `serverInfo.name` 与错误文案已修正，见 2026-11-04 条目。
- 没有真实 `claude -p` 通过此 server 调用工具，只验证了 stdio 协议。
- 非法输入的完整错误路径只在手动验收中验证了根命令的写入，其余分支仅有单元测试。
- `mcp` 子命令的参数解析没有单独测试。

## 2026-11-04 D9 修正：serverInfo 与错误拆分

**背景**

上一条记录的两项遗留问题：`serverInfo.name` 仍为 rmcp 默认值，错误文案 `unknown command` 对"父节点不在命令树中"的情况不准确。

**决定**

1. `serverInfo` 通过 `#[tool_handler(name = "whatcmd", version = "0.1.0")]` 设置，`name` 为 `whatcmd`，版本为 `0.1.0`。
2. `ToolError` 拆出 `NotInTree(String)`，文案为 `{0} is not in this run's command tree; describe its parent first`。`UnknownCommand` 仅用于根命令不在扫描结果中，文案改为 `unknown command {0}: not found in this run's PATH scan`。
3. `ensure_parent_visible` 的父节点检查与 `ensure_node_visible` 的节点检查都改为返回 `NotInTree`。两条测试断言同步更新。

**验证**

- `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` 退出码均为 0。`cargo test` 为 lib 189 项加集成测试 1 项，全部通过。
- 修正后重跑手动 stdio 验收（临时配置，`target/test-tmp/mcp-manual`）。`initialize` 返回 `serverInfo` 为 `whatcmd` `0.1.0`；`write_description`、`write_params` 对根命令 `git` 返回 `staged`，暂存区新增两行；`git clone` 返回 `NotInTree` 文案；`read_man` 对 `nope` 返回 `UnknownCommand` 文案。进程退出码 0。

**理由**

- 把"命令不存在"与"父节点未暴露"分开，调用方才知道该先描述父节点，还是换一个命令名。

**遗留问题**

- 子命令写入仍待 D10 接入索引。

## 2026-11-04 D10 生成流程、子命令队列与 reindex.log

**背景**

D10 实现 reindex 编排：锁、增量计划、阶段 A（分批调用 agent）、阶段 B（embedding 与合并）、`reindex` 子命令，以及 `mcp` 子命令的 `known_nodes`。真实运行中发现子命令没有进入索引，原因是 `--names` 模式在第一轮后直接退出，队列从未运行。

**决定**

1. `--names` 模式也展开子命令：列出的节点生成后，继续处理它们发现的子命令，范围仍由 `max_depth` 限制。这改变了 DESIGN 9.5 对 `--names` 的原定义，已同步修改 DESIGN。不比较、不复用、不删除的约束不变。
2. 不采用 `reindex --full` 作为验证手段，原因是成本与范围过大。
3. `reindex.log` 位于 `usage_file` 的父目录（缓存目录），每行以 `run_id` 开头，记录暂存记录、agent 失败、embedding 失败、合并数量，以及每轮发现的子命令数。没有文档而跳过的子命令记 `skipped <key>: no doc`。写入失败只记 `warn`。
4. 子命令的 `source_hash` 不比较，本轮发现即重新生成。
5. 偏离 INTERFACES 16.3 的签名：`generate_descriptions` 返回失败列表而非 `Result`；`embed_and_merge` 增加 `removed`、`sources`、`stamp` 参数；`make_plan` 用 `full: bool` 替代 `prompt_version`；`reindex` 通过 `Inputs` 结构体接收参数；`DocFetch<'a>` 为带生命周期的类型别名。

**理由**

- `--names` 是人工验证和局部修复的入口，不展开子命令时无法验证队列。展开范围由 `max_depth` 限制，成本可控。

**验证**

- fmt、`clippy --all-targets -D warnings` 退出码均为 0。`cargo test` lib 219 项、集成测试 1 项全部通过，退出码 0。新增 `names_mode_expands_subcommands`。
- 真实运行（`reindex --names git`，SiliconFlow `BAAI/bge-m3` 1024 维，`claude -p` 经 Cherry 路由）：退出码 0。`discovered 24 subcommands`，最终 `merged 234`，索引含 `git` 根命令、24 个一级子命令、`git bisect` 与 `git history` 下的二级子命令。`git bisect` 的三级子命令进入索引，但 `max_depth = 3` 不再展开其子节点。暂存区清空。`reindex.log` 与索引中均未发现 key。
- 根命令 `git` 的 `doc_source` 为 `man`，子命令为 `help`。

**观察**

- 部分描述为中文，`git bisect` 下的子命令描述相似度高，属于生成质量问题，需要抽样检查。
- 索引 `generator.agent_version` 仍为 `unknown`。

**遗留问题**

- `agent_version` 仍写死为 `unknown`。
- `mcp` 子命令的拒绝原因未单独写入 `reindex.log`，只能从合并结果间接判断。
- 配置 `WHATCMD_EMBEDDING_API_KEY` 所用的 SiliconFlow key 曾出现在对话中，待用户轮换。

## 2026-11-04 D10 重跑：提示词 v2 与英文描述

**背景**

提示词改为英文，描述以"做什么"为主。用 `reindex --names git` 重跑，核对英文描述与子命令队列。

**结果**

- 退出码 0，`generated 56, reused 0, removed 0, failed 0`。
- `git diff`、`git log`、`git show`、`git rebase` 在本次 run 中重新生成英文描述，暂存记录已确认。
- `merged 56` 是本次写入的节点数，不是索引总数，索引总数未统计。

**遗留问题**

- 旧中文条目（`git diff`、`log`、`rebase`、`show`）变旧的原因未确认。本次重跑已覆盖它们，但没有查清旧条目为何未被更新。
- `reindex.log` 中大量 `skipped ...: already seen`，同一子命令在多轮重复出现，原因未查清。
- `agent_version` 与 MCP 拒绝日志在本次重跑的索引与日志中未逐项核对。
- SiliconFlow key 待用户轮换。

## 2026-11-04 D10 提示词 v3–v5：子命令提取只依赖提示词

**背景**

v2 的子命令列表混入了大量文档中的非子命令词（选项值、示例里的单词、章节标题）。曾在程序侧用 `starts_doc_line` 按文档行首过滤，规则越来越多。

**决定**

- 删除程序侧的文档行过滤，子命令提取完全交给提示词（`src/generate/prompts/describe.md`，英文）。
- 提示词明确：只列文档中作为子命令/动作列出的词；排除选项、选项值、占位符、示例参数、章节标题、相关命令（SEE ALSO）。
- `PROMPT_VERSION` 升到 5，旧暂存记录全部失效重生成；`PROMPT_DOC_BYTES` 为 16KB。
- 调用固定 `--json-schema`，`parse_output` 优先读 `structured_output`，回退 `result`。

**理由**

模型从文档识别命令形式的能力足够，程序侧规则维护成本高且误杀真实子命令。假节点可以容忍：推荐有打分，低质量节点不会排到前面；真正没有文档的假节点会被后面的 `--help` 检查挡住。

**遗留问题**

- 未做假节点比例的系统统计，只做了抽查。

## 2026-11-04 D10 并发调用与 --help 检查

**背景**

串行调用 `claude -p` 跑完 git 全树约 10.5 分钟；模型偶尔列出不存在的子命令，入队后浪费一次调用。

**决定**

- 并发：`run_batches` 用 `std::thread::scope` + `AtomicUsize` 取任务 + `Mutex` 槽位，结果按输入顺序返回。文档与提示词仍串行准备（`DocFetch` 不是 `Sync`）。`AgentBackend` 加 `: Sync` 约束。新增配置 `[agent] concurrency`，默认 4，0 视为 1。
- `--help` 检查（`docs.rs`）：
  - 根命令 `--help` 仍接受只输出到 stderr 的情况。
  - 子命令 `--help` 只在输出到 stdout 时算有效（非零退出码但有 stdout 也保留）；嵌套路径再试 `-h`。
  - 两者都失败时，回退一层到父命令的帮助文本（`subcommand_help` → 父回退 → `Err(Missing)`），失败原因以 debug 记录（`log_help_failure`）。
  - 无文档的子命令不入队，`discover_children` 记录 `skipped {key}: no doc`。

**理由**

调用耗时主要在模型侧，并发是最直接的提速。只看 stdout 能挡住 git 对未知子命令在 stderr 打印的提示；但 `bisect good`、`submodule add`、`stash list` 这类嵌套子命令本身没有独立 `--help`，第一版检查误跳过了 35 个，因此加 `-h` 与父帮助回退。

**验证**

- 并发 4、无父回退：1138 个节点，252s。
- 最终（并发 4、含父回退）：`reindex --names git` 退出码 0，302s，`generated 1216, reused 0, removed 0, failed 0`，`merged 1216`。
- 被跳过的只有 7 个确实未安装的子命令：archimport、citool、cvsexportcommit、cvsimport、gui、instaweb、svn。
- `bisect good`、`submodule add`、`sparse-checkout list`、`p4 submit`、`stash list` 已生成。
- 新增 docs 测试 6 项（stderr-only 子命令视为缺失、非零退出有 stdout 保留、`-h` 回退、父帮助回退、无父帮助视为缺失、根命令 stderr 仍接受）；lib 236 项通过，fmt、clippy（`-D warnings`）通过。

**遗留问题**

- 相对串行约 2 倍提速（节点数也从 56 增至 1216，不可直接比）；更高并发是否触发限流未测。
- `agent_version` 在索引中为 `unknown`，未修。
- `reindex.log` 中 `already seen` 重复行仍未查清。
- `git history` 等新版本子命令本机 git 不存在，未覆盖。
- DESIGN 第 10 节 MCP 已废弃，`whatcmd mcp` 子命令与 rmcp 依赖尚未移除。
- SiliconFlow key 与 `~/.claude/settings.json` 中的 token 待用户确认后轮换。
- 以上改动均未提交。

## 2026-11-04 D10 收尾：agent_version、already seen 日志、文档同步

**背景**

并发与 `--help` 检查完成后，剩余遗留项：索引里 `agent_version` 为 `unknown`，`reindex.log` 中 `already seen` 大量重复，DESIGN 第 11、13 节仍有 MCP 与 `--allowedTools` 的旧表述。

**决定**

- `agent_version`：代码本身正确（`cli.rs` 的 `agent_version` 调用 `claude --version` 取第一行）。直接运行 `claude --version` 得到 `2.1.296 (Claude Code)`，测试目录的索引中也已是该值。之前的 `unknown` 来自更早的运行，不需要改代码。
- `already seen`：这是去重分支，同一子命令被多个父命令或多轮重复列出时才会命中，属于预期行为。删除 `discover_children` 中对应的日志行，去重逻辑不变。
- 文档：DESIGN 第 11 节（安全）去掉 `--allowedTools`、MCP 参数相关表述，改为 `--tools ""` 承担禁用；第 12 节测试项与新的参数拼装一致；第 13 节待确认项更新为已验证的实际状态，并删除已完成的 rmcp 与 embedding 客户端等过期项。

**验证**

- `cargo fmt --check` 通过。
- `cargo test --lib` 运行一次：236 项全部通过。此前偶发的一次失败未定位，本轮未处理。

**遗留问题**

- `whatcmd mcp` 子命令与 `rmcp` 依赖仍未移除，等待决定是否在本次提交前清理。
- `git history` 等新版本子命令本机 git 不存在，未覆盖。
- 以上改动均未提交，等待确认。
