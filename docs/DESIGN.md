# whatcmd 设计文档（v1）

> 状态：草案 v0.3（联网 embedding 进入 v1，树状命令节点，交互入口统一）
> 语言：Rust，单 crate，lib + bin
> 目标平台：Linux 与 macOS 同级支持
> 配套文档：INTERFACES.md（类型与签名），取舍记录见 DEVLOG.md

## 1. 目标与范围

用户在命令行里随时按快捷键，输入英文自然语言，例如 `pull romote repo`，whatcmd 返回候选命令或参数。用户选中后，候选内容插入到光标处，由用户自己回车执行。whatcmd 不执行任何命令。

v1 做的事：

- 命令树索引：每个节点是一个命令路径（`git`、`git pull`），以及挂在命令下的长参数（`git clone --branch`）。
- 联网查询：query 文本经 embedding 模型向量化，与本地向量做相似度匹配。
- 锚点匹配：输入到一半的命令前缀决定查询范围，只在该子树内检索。
- reindex：扫描 `$PATH`，读取 man 或 `--help`，由 Claude Code 生成节点描述和参数描述，再对节点做 embedding，写入索引。
- 预置集：常用命令随发布附带描述和向量，初始化时不调用 LLM。
- 交互入口：一个快捷键打开输入条，`/` 开头进入功能菜单（`reindex` 等）。

v1 不做（见第 14 节）：

- 断网时的离线模糊匹配兜底。
- 查询向量缓存。
- web search、stderr 采集、debug 与替换推荐。
- OpenCode 等其他 agent 后端。
- 多语言查询（当前只支持英文）。
- 常驻后台进程。

## 2. 术语

| 术语 | 含义 |
|---|---|
| 命令节点（command node） | 命令树中的一个命令路径，如 `git`、`git pull`。根节点是 PATH 中的可执行文件 |
| 参数节点（param node） | 挂在命令节点下的长参数，如 `git clone --branch`。只收录 `--` 开头的长参数 |
| 子树 | 某节点及其全部后代节点 |
| 锚点（anchor） | 由输入框内已完成的命令前缀解析出的命令节点，决定查询范围 |
| 批次（batch） | reindex 时交给 agent 一次处理的一组节点 |
| 暂存区（staging） | `write_description` / `write_params` 写入的中间结果，校验通过后才合并进索引 |
| 预置集（seed） | 随发布附带的常用命令节点，含描述与向量，reindex 不改动、不删除 |
| 使用记录（usage） | 用户在候选中选中某节点的事件，只存节点路径和时间，存在缓存目录 |

## 3. 架构

两条主链路，外加一个交互入口。共用 `index`、`guard`、`embed`。

```
离线 reindex（一次性子进程，可后台或由 /reindex 前台执行）
  scan($PATH) → docs(man / --help) → agent(claude -p + MCP 工具)
    → staging（描述、参数）→ 校验 → 节点合并
    → embed（描述文本 → 向量）→ index.json + vectors.bin（原子替换）

在线 query（每次快捷键触发）
  zsh 输入条 → whatcmd query --prefix "$LBUFFER" -- <文本>
    → 锚点解析（纯本地）→ embed(文本)（联网）→ 子树内向量匹配
    → 候选行（stdout）→ fzf 选择 → 插入 LBUFFER 光标处（不回车）
    → whatcmd record <key>（选中信号）
```

MCP 由 Claude Code 通过 `--mcp-config` 拉起，运行 `whatcmd mcp --run <run_id>`。

## 4. 子命令与退出码

| 命令 | 作用 |
|---|---|
| `whatcmd query [--prefix <LBUFFER>] [--limit N] [--json] -- <text>` | 锚点解析 + 向量匹配，输出候选行 |
| `whatcmd reindex [--full] [--names a,b] [--auto] [--quiet]` | 扫描 PATH，生成描述与参数，写索引 |
| `whatcmd record <key>` | 记录一次选中，写入使用记录 |
| `whatcmd mcp --run <run_id>` | stdio 运行 MCP server，供 Claude Code 调用 |
| `whatcmd doctor` | 检查配置、索引、agent、embedding 环境变量是否就绪 |

`--auto` 供 shell 初始化调用：指纹未变则直接退出，不输出任何内容（见第 9.1 节）。

退出码：

| 码 | 含义 |
|---|---|
| `0` | 成功，有结果 |
| `1` | 无结果，或 reindex 有失败 |
| `2` | 配置、索引、参数错误，或锚点前缀有误（如 `giit clone`） |
| `3` | embedding 服务不可用（网络失败、超时、鉴权失败） |

stdout 只输出候选行，其余信息一律写 stderr。

## 5. 模块划分

```
src/
├── main.rs          # 解析参数后分发，不含业务逻辑
├── lib.rs           # 模块导出
├── cli.rs           # clap 定义
├── config.rs        # 读取 TOML 配置，校验，展开 ~
├── types.rs         # CommandName、NodeKind、Candidate 等共享类型
├── scan.rs          # PATH 扫描、去重、可执行判断
├── docs.rs          # 获取 man 或 --help，清洗、截断，支持子命令路径
├── index/
│   ├── mod.rs       # index.json 读写，原子替换，schema 版本，Tree 构建
│   └── vectors.rs   # vectors.bin 读写，维度与行数校验
├── embed/
│   ├── mod.rs       # trait Embedder、embed_text（节点 → 嵌入文本）
│   └── openai.rs    # OpenAI 兼容接口客户端
├── search/
│   ├── mod.rs       # query 编排：锚点 → 嵌入 → 打分 → 候选
│   ├── normalize.rs # 查询规范化
│   ├── anchor.rs    # 前缀解析：锚点、末尾词丢弃、错误与修正提示
│   └── score.rs     # 语义分、子树分、置顶、使用加分（纯函数）
├── usage.rs         # 使用记录读写，使用加分
├── output.rs        # 候选行格式（tab 分隔）
├── seed.rs          # 预置集加载与校验
├── auto.rs          # PATH 指纹，自动 reindex 的触发判断
├── generate/
│   ├── mod.rs       # reindex 编排：分批、两阶段、合并、校验、原子替换
│   ├── plan.rs      # 增量计划（纯逻辑）
│   └── staging.rs   # 暂存区读写
├── agent/
│   ├── mod.rs       # trait AgentBackend、Capability、CapabilitySet
│   ├── detect.rs    # 探测入口、--help 能力解析
│   └── claude.rs    # Claude Code：参数拼装、输出解析
├── mcp/
│   ├── mod.rs       # rmcp stdio server，注册四个工具
│   └── tools.rs     # 工具函数（不依赖 rmcp，便于测试）
└── guard/
    ├── mod.rs       # 集中导出，便于审计
    ├── name.rs      # 命令名、子命令路径、参数名校验
    └── output.rs    # 描述与示例校验、危险标记、输出清洗
```

依赖方向：

- `cli`、`mcp` → `generate`、`search`、`index`、`config`、`usage`
- `generate` → `agent`、`docs`、`scan`、`index`、`embed`、`guard`、`seed`
- `search` → `index`、`embed`、`usage`、`output`
- `auto` → `scan`、`config`
- `guard`、`config`、`index`、`embed`、`types` 不依赖上层模块

`search` 只读索引，不写索引。`index` 不依赖 `search`、`generate`。

v1 不建 `cache/`、`search/fuzzy.rs`（离线兜底，见第 14 节）。探测逻辑放在 `agent/detect.rs`。

## 6. 数据格式

### 6.1 配置 `~/.config/whatcmd/config.toml`

```toml
[paths]
index_dir = "~/.local/share/whatcmd"
staging_dir = "~/.cache/whatcmd/staging"
usage_file = "~/.cache/whatcmd/usage.jsonl"

[agent]
backend = "auto"            # auto | claude-code | none
batch_size = 20

[reindex]
auto = true                 # shell 初始化时检测 PATH 变化并后台重建
max_depth = 3               # 命令树最大深度

[embedding]
endpoint = "https://api.example.com/v1"   # OpenAI 兼容接口，endpoint 可指向本地服务
model = "text-embedding-3-small"
dimensions = 1536
api_key_env = "WHATCMD_EMBEDDING_API_KEY" # 只存环境变量名，密钥本身不进配置
timeout_ms = 3000

[docs]
man_timeout_ms = 3000
help_timeout_ms = 2000
max_doc_bytes = 65536
```

- 配置路径可由 `WHATCMD_CONFIG` 覆盖，便于测试。
- 默认路径在 Linux 与 macOS 上一致，都使用 `~/.config`、`~/.local/share`、`~/.cache`，不使用 `~/Library`。`~` 展开只读取 `HOME`。
- `cache_dir` 不是配置项，固定为 `usage_file` 的父目录（默认 `~/.cache/whatcmd`），用于指纹、`reindex.log` 与 `index.lock`。
- `embedding.endpoint` 无默认值。缺失时 `load` 报错，`doctor` 给出提示。
- 测试不读写真实 `$HOME`。临时目录用 `tempfile::tempdir_in` 建在项目的 `target/` 下，`WHATCMD_CONFIG` 只在测试内指向该目录。
- 出现 `api_key` 字段时直接报错。密钥只能通过 `api_key_env` 指定的环境变量读取。
- `embedding.endpoint` 默认指向第三方服务时，查询文本会发送给该服务。文档和 `doctor` 输出都要提示这一点。支持本地 endpoint 是配置层面的选择，不需要改代码。

### 6.2 索引 `index.json`（schema v2）

```json
{
  "schema_version": 2,
  "built_at": "2026-11-03T10:00:00Z",
  "generator": {
    "backend": "claude-code",
    "agent_version": "2.1.296",
    "prompt_version": 2
  },
  "embedding": {
    "model": "text-embedding-3-small",
    "dimensions": 1536
  },
  "vectors_sha256": "sha256:...",
  "nodes": [
    {
      "key": "git pull",
      "kind": "command",
      "parent": "git",
      "name": "pull",
      "description": "Fetch from and integrate with a remote repository into the current branch.",
      "examples": ["git pull origin main"],
      "doc_source": "man",
      "source_hash": "sha256:...",
      "generated_at": "2026-11-03T10:00:00Z",
      "pinned": false,
      "vector_row": 17
    },
    {
      "key": "git clone --branch",
      "kind": "param",
      "parent": "git clone",
      "name": "--branch",
      "description": "Check out the given branch instead of the remote HEAD.",
      "examples": [],
      "doc_source": "help",
      "source_hash": "sha256:...",
      "generated_at": "2026-11-03T10:00:00Z",
      "pinned": false,
      "vector_row": 42
    }
  ]
}
```

- `key`：命令节点为完整路径；参数节点为所属命令路径加空格加参数名。
- `parent`：命令的父是上一级命令，根命令为 `null`；参数的父是所属命令。
- `pinned`：预置集节点为 `true`，reindex 不比较、不删除、不重新生成。
- 写入走 `index.json.tmp` + rename，`vectors.bin` 同样处理，两个文件要一起替换。
- `vectors_sha256`：`vectors.bin` 全部字节的 sha256，写入 `index.json`。`load` 时校验。两次 rename 之间崩溃会留下新 `index.json` 配旧 `vectors.bin`，行数相同时长度校验挡不住，这个字段用于发现此情况。
- `schema_version` 不匹配时拒绝加载，提示重建。v1 之前的 schema 1 索引不做迁移，直接提示重建。

### 6.3 向量 `vectors.bin`

- 每行是一个节点的向量，`f32` 小端序，行长 `dimensions` 个分量。
- 写入前做 L2 归一化，查询时点积即余弦相似度。
- 行号由 `vector_row` 指定。读取时校验文件长度等于 `nodes 数量 × dimensions × 4`，不一致则拒绝加载。

### 6.4 暂存区 `~/.cache/whatcmd/staging/<run_id>.jsonl`

每行一条通过校验的记录，字段为 `key`、`kind`、`description`、`examples`、`written_at`。同一 key 以最后一条为准。描述合并进索引并完成 embedding 后删除该记录。embedding 失败的记录保留，下次 reindex 直接做 embedding，不再调用 LLM。

### 6.5 使用记录 `~/.cache/whatcmd/usage.jsonl`

每行一条：`{"key": "git clone", "at": "2026-11-03T10:00:00Z"}`。只记录被选中的节点路径和时间，不记录查询文本。文件位于缓存目录，不进入索引，reindex 不清空它，也不发送给 agent 或 embedding 服务。

## 7. 查询

### 7.1 锚点解析

输入：`LBUFFER`（光标左侧文本）。纯本地，不联网。

1. 按空白切分为 token。
2. 若 `LBUFFER` 末尾不是空白，最后一个 token 视为未完成的词，丢弃。
3. 第一个 token 必须是根命令节点。否则返回错误，见 7.3。
4. 从第二个 token 起逐个处理：
   - `--`（单独）：分隔符，停止锚点解析。
   - 以 `-` 开头：短参数或长参数，跳过，不影响锚点。长参数记入"已输入参数"，用于去重（见 7.4）。
   - 是当前节点的子命令名：下降到该子节点。
   - 其他：视为参数值或查询词，停止锚点解析，后面的 token 都不参与。
5. 锚点即最后下降到的命令节点。

示例：

- `git clone ` → 锚点 `git clone`。
- `git cl` → 末尾词 `cl` 丢弃，锚点 `git`。
- `git -C repo clone ` → `-C` 跳过，`repo` 不是子命令，停止，锚点 `git`。
- `giit clone ` → 第一个 token 不是根命令，报错。

### 7.2 向量匹配与打分

查询文本是 `query` 的 `<text>` 参数，即输入条里的自然语言。

1. 规范化：去首尾空白、转小写、连续空白折叠。为空则退出码 `2`。
2. 调用 embedding 接口得到查询向量。失败返回退出码 `3`。
3. 候选范围为锚点的子树（锚点本身除外），包括子命令和参数节点。锚点为根时，范围是全部节点。
4. 语义分 `s_self(n) = cos(q, v_n)`。
5. 子树分：`s_sub(n) = max over 后代 d (decay^depth(d)−depth(n) × s_self(d))`。节点最终分 `s(n) = max(s_self(n), s_sub(n))`。
6. 节点只有 `s(n) ≥ min_score` 才算候选。
7. 使用加分：`bonus(n) = min(usage_cap, usage_alpha × ln(1 + count(n)))`，再乘时间衰减 `2^(−age_days / half_life_days)`。
8. 精确置顶：规范化后的查询文本与某个节点的 `key` 完全相等，该节点排第一，不受上述分数影响。
9. 最终排序：精确置顶优先，其余按 `s(n) + bonus(n)` 降序，截取 `--limit`。

所有系数（`decay`、`min_score`、`usage_cap`、`usage_alpha`、`usage_half_life_days`）先写成代码中的常量，用固定样本调参后再决定是否开放到配置。

### 7.3 回退与报错

- 前缀错误（第一个 token 不是根命令）：stderr 输出 `no command named giit, did you mean git?`，退出码 `2`，不查询，不推荐。修正提示只基于编辑距离（≤ 2）给出一个候选，不自动替换。找不到相近候选时只输出 `no command named giit`。
- 子树为空：锚点子树内没有节点超过 `min_score` 时，回退到父节点重新检索。父节点仍为空则继续回退，直到根。根的范围是全部节点，结果为空则退出码 `1`。
- 回退时，候选的 `via` 字段标出回退层级，widget 可以提示"结果不在当前锚点下"。

### 7.4 候选与插入

每个候选行包含：插入文本、完整路径、类型、分数、危险标记、父节点的最佳子路径（`via`）、描述、示例。

插入文本的计算（纯函数，在 Rust 侧完成，便于测试）：

- 候选路径去掉锚点路径前缀后的剩余部分，前面补一个空格（若 `LBUFFER` 不以空白结尾）。
- 去重：若候选的完整路径已经是 `LBUFFER` 末尾的命令链，返回空插入，widget 不做任何修改。例如已输入 `git clone`，候选 `git clone` 不再追加。
- 参数去重：`LBUFFER` 中已出现的长参数（按 `=` 前的部分比较）不再作为候选推荐。`--name=value` 只比较 `--name`。
- `--` 分隔符不入库，不推荐。

选中后只改 `LBUFFER`，`RBUFFER` 保持不动，光标停在插入内容之后。

### 7.5 使用信号

- 信号以"选中"为准。出现在候选列表中不计数。
- widget 选中后调用 `whatcmd record <key>`，由 `usage.rs` 追加到 `usage.jsonl`。
- 只记录命令或参数的 `key`，不记录查询文本。
- 使用记录与索引分开存放，reindex 不改动它。

### 7.6 断网

v1 没有离线兜底。`embedding` 请求失败时，`query` 退出码 `3`，stderr 给出原因，widget 显示提示，不返回伪造结果。离线模糊匹配作为后续阶段的兜底，见第 14 节。

## 8. 交互入口（zsh）

`shell/whatcmd.zsh` 只做三件事：打开输入条、调用 `whatcmd`、把结果插入 `LBUFFER`。

- 快捷键 `Ctrl-G`，在命令行下方打开输入条，实现使用 `fzf --print-query`，输入框即查询条。光标留在输入条内。
- 输入条内直接输入的文本是自然语言查询。结果在条下方列出，tab 选中，选中后关闭输入条，把插入文本写入 `LBUFFER` 光标处，`RBUFFER` 不变，不回车。
- 任意时刻都可以触发，触发时读取 `LBUFFER` 作为锚点前缀。
- 输入条第一个字符是 `/`，且后面没有空格或只有一个词时，进入功能菜单，列出 `reindex`、`doctor` 等动作。tab 选中后执行。其余以 `/` 开头的文本（如 `/etc 下找大文件`）一律当作自然语言查询。
- 菜单项与 CLI 子命令一致。`debug` 属于 v2，不出现在菜单中。
- 含 `rm`、`dd`、`mkfs`、`> /dev/`、`chmod -R`、`chown -R` 的示例，在列表中加危险标记，选中后要求二次确认。
- 没有 fzf 时退化为 zsh 内置 `select`。
- `whatcmd` 不在 PATH 时，`zle -M` 提示后返回。
- widget 只改 `LBUFFER`，不执行任何命令。

### 8.1 `/reindex` 的执行方式

- 在输入条内前台执行，显示进度（stderr 的 info 行回显在条下方），执行期间阻塞输入条，结束后显示汇总。
- 这是用户主动触发，与自动后台构建（第 9.1 节）是两个入口，行为不同。

## 9. reindex 与后台构建

### 9.1 两种触发方式

- 自动后台构建：shell 初始化时执行 `whatcmd reindex --auto`。程序先计算 PATH 指纹（已排除预置节点），与缓存目录中的上次指纹比较。指纹未变或 `reindex.auto = false` 则直接退出。有变化则在后台拉起一次性子进程。
  - 不输出到终端。stderr 重定向到 `cache_dir/reindex.log`，日志只写该文件。
  - 不常驻，跑完退出。
  - 不阻塞 `query` 和 widget。`query` 不做任何索引过期检查。
  - 与 flock 配合：若已有 reindex 持有锁，自动触发直接退出。
- `/reindex`：见 8.1 节，前台阻塞，显示进度。

指纹只在 `--auto` 且成功完成后写入。失败时不更新，下次 shell 初始化会重试。

### 9.2 流程

1. 获取 `index.lock`（flock），已存在则退出。
2. `scan` 得到命令集合，构造根命令列表。
3. 第 0 轮：对根命令调用 `docs` 取文档，计算 `source_hash = sha256(文档 + prompt_version)`。hash 与旧条目一致的复用。
4. 变化或新增的节点进入队列。预置节点不进入队列，除非通过 `--names` 显式指定。
5. 若旧索引中某根命令已不在 PATH，删除它及其非预置子树。预置节点不删除。
6. 按树结构分批调用 agent（见 9.3）。
7. 读取暂存区，经 `guard` 校验后合并进内存中的节点集合。agent 发现的子命令进入下一轮队列，直到 `max_depth`。
8. 对所有待写入节点做 embedding（见 9.3）。
9. 写 `index.json.tmp` 与 `vectors.bin.tmp`，fsync，rename 成正式文件。
10. 清理已合并的暂存区记录，释放锁。

### 9.3 分层与批次

- 两阶段拆开：阶段 A 生成描述（agent，耗时长、有 LLM 成本）；阶段 B 做 embedding（HTTP，便宜但会失败）。阶段 A 的结果落盘到暂存区，阶段 B 失败不会导致 agent 重跑。
- 批次按子树组织。同一批尽量包含同一父节点的子节点，并把父节点的当前描述放进提示词作为上下文，让 agent 能对照边界写描述。
- `batch_size` 是批次的节点数上限，不是纯数量切分。
- 参数抽取由 agent 从 `--help` 中结构化抽取，只保留 `--` 长参数，短参数在提示词和校验两处都过滤掉。
- embedding 按 `batch_size` 分批调用。
- embedding 文本的组成：
  - 命令节点：`路径 + 描述 + 示例`。
  - 参数节点：`所属命令路径 + 参数名 + 参数描述`。
- 每批 embedding 失败只影响该批的节点。已有旧向量的节点保留旧向量，新节点不写入索引，并在 stderr 汇总。

### 9.4 预置集（seed）

- 常用成熟命令（如 `ls`、`grep`、`find`、`git`、`ps`、`lsof` 等）的节点树，在发布前用 LLM 生成一次描述，随二进制附带。
- 预置集分两部分：跨平台交集（`seed/common.json`），加平台专属小集（`seed/linux.json`、`seed/macos.json`）。编译时按目标系统选择。
- 预置节点的子树一并固定。例如收录 `git`，`git pull` 等子节点同时收录。
- 预置节点 `pinned = true`，初始化时写入索引。reindex 不比较、不删除、不因为"不在 PATH"而移除。
- 预置节点的向量随发布附带，对应默认 embedding 模型。使用其他模型时，只对预置描述重新做 embedding，不调用 LLM。
- 预置集的更新只随发布进行，不由用户的 reindex 改写。

### 9.5 失败处理

- 每批独立。某批 agent 失败只影响该批节点，其他批次继续。
- 失败的节点保留旧条目，结束时在 stderr 汇总。
- 全部成功退出码 `0`，有失败退出码 `1`。
- `--names` 只重跑指定节点，可以指定预置节点。
- `--auto` 下的失败只写日志，不打扰用户，下次 shell 初始化会重试。

### 9.6 Agent 能力

```rust
pub trait AgentBackend {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> &CapabilitySet;
    fn run_batch(&self, req: &BatchRequest) -> Result<BatchOutcome, AgentError>;
}

pub enum Capability {
    NonInteractive,   // claude -p
    JsonOutput,       // --output-format json
    Mcp,              // --mcp-config
    StrictMcp,        // --strict-mcp-config，忽略用户已有的 MCP 配置
    ToolAllowlist,    // --allowedTools
    RestrictedTools,  // --tools，限定内置工具集
}
```

`reindex` 要求上述能力全部具备。缺任何一项，拒绝运行并指出缺的是哪一项。

### 9.7 Claude Code 调用形态

```
claude -p "<批次提示词>"
  --output-format json
  --strict-mcp-config
  --mcp-config <临时 mcp.json>
  --tools ""                # 具体取值待 D7/D8 验证
  --allowedTools mcp__whatcmd__read_man,mcp__whatcmd__read_help,mcp__whatcmd__write_description,mcp__whatcmd__write_params
```

- 内置工具的禁用只由 `--tools ""` 承担。实验（2026-11-03）显示：仅 `--allowedTools` 时 Bash 实际执行；`--tools ""` 时标记文件未生成。`--restricted` 在后续重跑中有一次生成了标记文件，不能作为禁用手段。`--allowedTools` 不承担禁用责任，详见第 11 节与 DEVLOG。
- `--allowedTools` 只放行四个 whatcmd 工具，作为白名单使用，不作为禁用手段。
- `--mcp-config` 指向临时文件，只包含 `whatcmd mcp --run <run_id>`。
- 提示词来自 `prompts/describe.md`，通过 `include_str!` 嵌入。提示词带版本号，变化即触发增量重建。
- 边界约束写在提示词中：命令节点的描述只覆盖本节点的行为，子命令的细节留给子节点；参数节点只描述该参数。样例需要人工检查（见第 12 节）。
- 具体参数在实现前用 `claude --help` 核对，见第 13 节。

### 9.8 探测（agent/detect.rs）

1. `backend = "claude-code"` 时只探测它；`auto` 时探测 `claude`。
2. PATH 查找二进制，不硬编码路径。
3. `--version` 探针，超时 3 秒。
4. 解析 `--help`，检查第 9.6 节列出的能力是否存在。
5. 全部不可用时，列出检测过的路径并给出安装提示，不自动安装。

`--version` 通过不代表能用。是否已登录，只能由 `doctor` 手动发一次极小请求确认，reindex 不会自动执行。

## 10. MCP 工具

`whatcmd mcp --run <run_id>`，stdio，使用 `rmcp = "=3.8.0"`，`server` 特性。

v1 有四个工具：

| 工具 | 参数 | 行为 | 约束 |
|---|---|---|---|
| `read_man` | `name` | 返回根命令的 man 页纯文本 | `name` 通过校验且在 PATH 中；清洗、截断 |
| `read_help` | `path`（子命令路径，如 `git clone`） | 运行 `<path...> --help`，返回输出 | 每个 token 通过命名校验；只允许 `--help`；超时 2 秒；stdin 为 `/dev/null`；超时杀死进程组 |
| `write_description` | `path`, `description`, `examples[]`, `subcommands[]` | 写入暂存区（命令节点） | 经 `guard` 校验；`subcommands` 每项须通过命名校验，作为下一轮队列；深度不超过 `max_depth` |
| `write_params` | `path`, `params[{flag, description}]` | 写入暂存区（参数节点） | `flag` 必须是 `--` 长参数并通过 `guard`；短参数丢弃并记 `warn` |

约束：

- 子进程用 `Command::new(bin).args([...])`，不经过 shell。
- 文档环境固定：`LC_ALL=C`、`MANPAGER=cat`、`PAGER=cat`、`MANWIDTH=80`。
- 输出清洗：去除 ANSI 转义和退格控制字符，统一换行。截断到 `max_doc_bytes`，末尾加 `[truncated]`。
- 工具只写暂存区，不直接改索引。
- `path` 参数必须能在本次 run 的命令树中找到父节点，不接受任意路径。

## 11. 安全

威胁来源：man、`--help` 输出都可能包含诱导 agent 的指令（提示注入）；agent 可能生成错误或误导性描述；查询文本和 embedding 请求会离开本机。

边界措施：

1. 内置工具用 `--tools ""` 全部禁用。这是唯一经实验验证能阻止 Bash 真实执行的参数。`--allowedTools` 只放行四个 whatcmd 工具，不能单独禁用 Bash。
2. 模型可能在没有工具时模仿工具调用格式并编造执行结果。因此 `ClaudeBackend` 不能只看 `result` 文本判定成功，结果以暂存区文件为准（见 9.6 节）。
2. `--strict-mcp-config` 避免继承用户已有的 MCP 配置。
3. 命令名正则 `^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$`，子命令路径逐 token 校验，且必须存在于本次扫描结果或命令树中。不接受路径、空格、参数。
4. 参数名正则 `^--[A-Za-z0-9][A-Za-z0-9-]*$`，长度上限与命令名一致。
5. 描述长度 ≤ 400 字符，示例每条 ≤ 200 字符，最多 3 条，去除控制字符，不含换行和制表符。
6. 插件不因为 agent 的建议而执行任何东西。widget 只写入命令行，不回车。
7. 危险示例加标记，选中时二次确认。
8. 密钥只从环境变量读取，配置中禁止 `api_key`。
9. 查询文本会发送给 embedding 服务。文档与 `doctor` 输出都写明这一点。使用记录只存在本地。

## 12. 测试

单元测试（纯函数优先）：

- `normalize`：大小写、空白、空串。
- `anchor`：正常前缀、末尾未完成词丢弃、`-` 跳过、`--` 分隔符停止、`git -C repo clone` 锚点停在 `git`、`giit clone` 报错并给修正提示、空前缀。
- `score`：精确置顶、子树衰减、`min_score` 过滤、回退判定。
- 插入文本：去重（`git clone` 已在末尾时不追加）、参数 `=` 前比较、插入前导空格。
- `guard`：合法与非法命令名、参数名（路径、空格、`..`、`;`、`$()`、短参数）。
- `agent/claude.rs`：参数拼装快照，确认 `--allowedTools` 只含四个 whatcmd 工具，不含 Bash。
- `agent/detect.rs`：用假 `--help` 文本测试能力解析，包括参数缺失。
- `index`：`schema_version` 不匹配时拒绝加载；`vectors.bin` 长度与节点数不一致时拒绝加载。
- `usage`：只记录 key 和时间，不含查询文本。
- `auto`：指纹相同不触发，指纹变化触发，预置节点不计入指纹。

集成测试：

- `tests/fixtures/` 放假 man 输出、假 `--help`、假 agent 输出，驱动 reindex 全流程。
- 使用 `FakeBackend` 与 `FakeEmbedder`，不调用真实 claude，也不访问网络。
- 临时目录放假可执行文件作为 PATH，验证增量、删除、预置豁免、失败隔离、阶段 B 失败后暂存区保留。

提示词样例检查（人工或脚本，不走单元测试）：

- 固定一组命令样例（如 `git`、`git pull`、`git clone`），检查父节点描述是否越界到子命令细节。
- 结果记录在 DEVLOG，提示词版本变化时重新检查。

手动验收：

- 真实 endpoint 下 `query` 返回语义结果；endpoint 不可达时退出码 `3`。
- zsh 中 `Ctrl-G` 打开输入条，选中后插入 `LBUFFER` 光标处，不回车。
- `/reindex` 前台显示进度并阻塞，结束后输出汇总。
- shell 初始化在 PATH 未变时不输出任何内容，PATH 变化后在后台完成重建。
- 真实 `claude` 跑 3 条命令，检查暂存区和工具调用记录。

## 13. 待确认项

| 项目 | 状态 |
|---|---|
| Claude Code 参数（`-p`、`--output-format json`、`--tools`、`--allowedTools`、`--mcp-config`、`--strict-mcp-config`、`--bare`） | 待本机 `claude --help` 完整核对 |
| `--allowedTools` 是否真的禁用未列出的内置工具 | 已验证：不能禁用。`--tools ""` 承担禁用，样本有限，见第 11 节与 DEVLOG |
| `--restricted` 是否纳入能力检查 | 暂不纳入。重跑中出现过标记文件，不可作为禁用手段，见 DEVLOG |
| 向量存储形式 | 暂定 `vectors.bin`，与 `index.json` 分开，待实现时确认 |
| 子命令发现方式 | 暂定由 agent 在 `write_description` 中给出 `subcommands`，分轮展开 |
| 查询结果中 `via` 的展示格式 | 暂定 `git  (pull: ...)`，待样本确认 |
| 打分系数（`decay`、`min_score`、`usage_*`） | 代码常量，待固定样本调参 |
| rmcp 3.x 的 API | 实现时对照 docs.rs |
| embedding 的 HTTP 客户端与精确版本 | 实现时选定，写入 DEVLOG |
| 快捷键 | `Ctrl-G` |

## 14. 后续阶段（v1 之后）

| 阶段 | 内容 |
|---|---|
| v1.1 | 断网兜底：离线模糊匹配（`nucleo-matcher` 或 `fuzzy-matcher`）、查询向量缓存 |
| v1.2 | web_search（条件注册的第二轮） |
| v2 | stderr 采集（`precmd`）、`whatcmd debug`、替换推荐、只读的 `list_dir` / `read_file` |
| 其他 | OpenCode 适配、CJK 分词器、打包与分发、打分系数开放配置 |

## 15. 里程碑（v1）

| 阶段 | 内容 | 验收 |
|---|---|---|
| M0 | 骨架、guard、类型、配置、scan、index 与 vectors 读写 | 生成空索引并通过校验 |
| M1 | 锚点解析、向量匹配、输出协议、embedding 客户端、zsh 输入条 | 用固定向量跑通查询与插入，不回车 |
| M2 | agent 探测、`doctor`、Claude Code 适配、docs | `doctor` 输出能力与 embedding 配置结果 |
| M3 | MCP 四个工具、reindex 两阶段、暂存区、预置集、自动触发 | 3 条命令端到端跑通，失败可隔离，初始化无输出 |
