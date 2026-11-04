# whatcmd 接口设计

> 状态：草案 v0.2（与 DESIGN.md v0.3 对齐）
> 配套文档：DESIGN.md（架构与行为），本文件只定义模块之间的类型与函数签名。
> 原则：每个模块都能脱离其他模块单独测试。签名写到能编译的程度，实现细节留给各模块的 demo。

## 1. 约定

- 错误：每个模块定义自己的 `thiserror` 枚举；`main.rs` 用 `anyhow` 汇总，并按第 17 节映射退出码。
- 所有权：路径和文本用 `String` / `PathBuf` 存储，函数参数优先借用（`&str`、`&Path`、`&[T]`）。
- 纯函数优先：能不依赖文件系统和网络的逻辑，都写成纯函数，方便快照测试。
- 异步：v1 全部使用同步代码（MCP 与 tokio 已移除）。embedding 的 HTTP 调用同步阻塞即可。
- 新类型：命令名用 `CommandName` 包装，只能由 `guard::name` 构造，避免未校验的字符串流入后续模块。
- 网络：只有 `embed::openai` 与 `agent::claude` 的子进程会离开本机，其余模块不得发起网络请求。

## 2. 模块依赖图

```
cli ─┬─> generate ─┬─> scan
     │             ├─> docs ──────> guard
     │             ├─> agent ─┬──> guard
     │             │          └──> (agent::claude, agent::detect)
     │             ├─> embed ─────> config
     │             ├─> seed
     │             ├─> index
     │             └─> guard
     ├─> search ─┬─> index
     │           ├─> embed
     │           ├─> usage
     │           └─> output
     ├─> auto ───────> scan, config
     ├─> usage
     ├─> index
     └─> config
```

约束：

- `index` 不依赖 `search`、`generate`。
- `guard`、`config`、`types` 不依赖任何上层模块。
- `embed` 只依赖 `config` 与 `types`。
- `search` 只读索引，不写索引。

### 2.1 模块导出（`lib.rs`）

`lib.rs` 只声明模块，不含业务逻辑。`main.rs` 只通过 `whatcmd::` 路径调用这里导出的模块。

```rust
// lib.rs
pub mod agent;
pub mod auto;
pub mod cli;
pub mod config;
pub mod docs;
pub mod embed;
pub mod generate;
pub mod guard;
pub mod index;
pub mod output;
pub mod scan;
pub mod search;
pub mod seed;
pub mod types;
pub mod usage;
```

每个 `pub mod` 都必须有模块级 rustdoc（`//!`），`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` 才能通过。

## 3. 核心数据类型（`src/types.rs`）

```rust
/// 经过校验的命令名。只能通过 guard::name::validate_command 构造。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandName(String);

impl CommandName {
    pub fn as_str(&self) -> &str;
}

/// 节点类型：命令节点或参数节点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Command,
    Param,
}

/// 文档来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocSource {
    Man,
    Help,
    Missing,
}

/// PATH 中发现的一个可执行文件，对应根命令候选。
#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub name: CommandName,
    pub path: PathBuf,           // 第一次在 PATH 中出现的位置
}

/// 命令树中的一个节点（对应 index.json 的 nodes[i]）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub key: String,             // 命令节点为完整路径；参数节点为 "所属路径 + 空格 + 参数名"
    pub kind: NodeKind,
    pub parent: Option<String>,  // 根命令为 None；参数的父是所属命令
    pub name: String,            // 命令节点为最后一段；参数节点为 --flag
    pub description: String,
    pub examples: Vec<String>,   // 参数节点恒为空
    pub doc_source: DocSource,
    pub source_hash: String,     // "sha256:..."
    pub generated_at: String,    // RFC 3339
    pub pinned: bool,            // 预置节点为 true
    pub vector_row: Option<usize>, // 在 vectors.bin 中的行号；无向量时为 None
}

/// 查询结果的一行候选。
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub key: String,             // 完整路径，也是 usage 记录用的 key
    pub kind: NodeKind,
    pub score: f32,              // s(n) + bonus(n)
    pub exact: bool,             // 是否精确置顶
    pub insert: String,          // 插入 LBUFFER 的文本（已去重，见 search::anchor::insertion_text）
    pub description: String,
    pub example: Option<String>,
    pub dangerous: bool,         // 由 guard::output::is_dangerous 设置
    pub via: Option<String>,     // 回退到父节点时，标出实际命中的最佳子路径
}
```

## 4. 配置（`config.rs`）

职责：读取 TOML、填默认值、校验。不做任何 IO 以外的副作用。

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub paths: PathsConfig,
    pub agent: AgentConfig,
    pub reindex: ReindexConfig,
    pub embedding: EmbeddingConfig,
    pub docs: DocsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathsConfig {
    pub index_dir: PathBuf,      // 默认 ~/.local/share/whatcmd
    pub staging_dir: PathBuf,    // 默认 ~/.cache/whatcmd/staging
    pub usage_file: PathBuf,     // 默认 ~/.cache/whatcmd/usage.jsonl
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    pub backend: BackendChoice,  // auto | claude-code | none
    pub batch_size: usize,       // 默认 20，节点数上限
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendChoice {
    Auto,
    ClaudeCode,
    None,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReindexConfig {
    pub auto: bool,              // 默认 true，shell 初始化时触发 --auto
    pub max_depth: usize,        // 默认 3
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    pub endpoint: String,        // OpenAI 兼容接口，可指向本地服务；无默认值，缺失时 load 报错
    pub model: String,
    pub dimensions: usize,
    pub api_key_env: String,     // 环境变量名，不是密钥本身
    pub timeout_ms: u64,         // 默认 3000
}

#[derive(Debug, Clone, Deserialize)]
pub struct DocsConfig {
    pub man_timeout_ms: u64,     // 默认 3000
    pub help_timeout_ms: u64,    // 默认 2000
    pub max_doc_bytes: usize,    // 默认 65536
}

/// 从 path 加载；path 为 None 时使用 WHATCMD_CONFIG 或默认路径。
/// 文件不存在时返回默认配置，不报错。
/// 出现 api_key 字段时返回 ConfigError::ForbiddenKey。
pub fn load(path: Option<&Path>) -> Result<Config, ConfigError>;

/// 解析字符串，供测试使用。
pub fn parse(toml_text: &str) -> Result<Config, ConfigError>;

/// 读取 embedding 密钥。环境变量缺失时返回 None，由调用方决定报错或 doctor 提示。
pub fn embedding_api_key(cfg: &EmbeddingConfig) -> Option<String>;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("forbidden key `{0}` in config; keys must come from the environment")]
    ForbiddenKey(String),
}
```

## 5. guard（`guard/`）

### 5.1 命名校验（`guard/name.rs`）

```rust
/// 命令名规则：^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$，不接受路径、空格、参数。
pub fn validate_command(raw: &str) -> Result<CommandName, GuardError>;

/// 子命令路径：按空格切分后逐 token 调用 validate_command。
/// 返回的 Vec 至少含一个 token。
pub fn validate_path(raw: &[&str]) -> Result<Vec<CommandName>, GuardError>;

/// 参数名规则：^--[A-Za-z0-9][A-Za-z0-9-]*$，长度上限与命令名一致。
/// 短参数（-x）返回 Err(GuardError::ShortFlag)。
pub fn validate_flag(raw: &str) -> Result<String, GuardError>;

#[derive(Debug, thiserror::Error)]
pub enum GuardError {
    #[error("invalid command name: {0:?}")]
    InvalidName(String),
    #[error("invalid flag: {0:?}")]
    InvalidFlag(String),
    #[error("short flag not allowed: {0:?}")]
    ShortFlag(String),
    #[error("description too long: {0} > 400")]
    DescriptionTooLong(usize),
    #[error("example too long or too many examples")]
    ExampleLimit,
    #[error("empty command path")]
    EmptyPath,
}
```

命令名是否存在于 PATH 或命令树中的判断不在这里做，由调用方传入集合后检查。

### 5.2 输出校验与危险标记（`guard/output.rs`）

```rust
pub struct ValidatedDescription {
    pub description: String,     // 已去控制字符，长度 ≤ 400
    pub examples: Vec<String>,   // 最多 3 条，每条 ≤ 200
}

/// 去除 \n \t 与其他控制字符（替换为空格后折叠），再检查长度。
/// 示例超过 3 条时返回 ExampleLimit（不截断，见 DEVLOG）。
pub fn validate_output(desc: &str, examples: &[String]) -> Result<ValidatedDescription, GuardError>;

/// 参数描述校验，规则与 validate_output 的描述部分相同，示例固定为空。
pub fn validate_param_description(desc: &str) -> Result<String, GuardError>;

/// 示例按词匹配 rm、dd、mkfs，子串匹配 "> /dev/"、"chmod -R"、"chown -R"。
pub fn is_dangerous(example: &str) -> bool;

/// 输出清洗：去 ANSI 转义与退格，统一换行。供 docs 使用。
pub fn sanitize_text(raw: &str) -> String;
```

## 6. 扫描 PATH（`scan.rs`）

```rust
/// 纯函数：输入 PATH 字符串，返回去重后的根命令列表。
/// 同名以 PATH 中靠前的为准；跳过非文件、非可执行、非法命令名。
/// 文件系统读取只发生在 is_executable 中。
pub fn commands_from_path(path_var: &str) -> Vec<CommandEntry>;

/// 调用方便利函数：读取环境变量 PATH。
pub fn current_path_commands() -> Vec<CommandEntry>;

/// 查找单个根命令，用于 read_man 之类的按名访问。
pub fn find_in_path(path_var: &str, name: &CommandName) -> Option<PathBuf>;
```

## 7. 文档获取（`docs.rs`）

```rust
pub struct Doc {
    pub source: DocSource,       // Man 或 Help；都没有时返回 Err
    pub text: String,            // 已清洗、已截断
    pub truncated: bool,
}

/// 根命令的 man 页。失败返回 DocError::Missing 由调用方决定是否回退到 --help。
pub fn fetch_man(name: &CommandName, cfg: &DocsConfig) -> Result<Doc, DocError>;

/// 运行 `<path...> --help`。path 至少一个 token，每个 token 已通过 guard 校验。
/// 只允许追加 --help，不接受其他参数。
pub fn fetch_help(path: &[CommandName], cfg: &DocsConfig) -> Result<Doc, DocError>;

/// 优先 man，失败后尝试 --help。
pub fn fetch(path: &[CommandName], cfg: &DocsConfig) -> Result<Doc, DocError>;

/// 纯函数：去除 ANSI 转义、退格（man 粗体/下划线）、统一换行，并按字节截断。
pub fn clean(raw: &[u8], max_bytes: usize) -> (String, bool);

#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("no man page or help output for {0}")]
    Missing(String),
    #[error("timed out after {0} ms")]
    Timeout(u64),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
```

执行环境固定：`LC_ALL=C MANPAGER=cat PAGER=cat MANWIDTH=80`，stdin 为 `/dev/null`，超时后杀死整个进程组。子进程用 `Command::new(bin).args([...])`，不经过 shell。

## 8. 索引与向量（`index/`）

### 8.1 索引（`index/mod.rs`）

```rust
pub struct Index {
    pub meta: IndexMeta,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMeta {
    pub schema_version: u32,     // v1 固定为 2
    pub built_at: String,
    pub generator: GeneratorMeta,
    pub embedding: EmbeddingMeta,
    pub vectors_sha256: String,  // "sha256:..."，vectors.bin 全部字节的哈希，load 时校验
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorMeta {
    pub backend: String,         // "claude-code"
    pub agent_version: String,
    pub prompt_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingMeta {
    pub model: String,
    pub dimensions: usize,
}

pub const SCHEMA_VERSION: u32 = 2;

/// index_dir/index.json 与 index_dir/vectors.bin。文件不存在返回 Ok(None)。
/// schema_version 不匹配、或向量文件长度与节点数不一致时返回 Err。
pub fn load(index_dir: &Path) -> Result<Option<Index>, IndexError>;

/// 写 index.json.tmp 与 vectors.bin.tmp，fsync，两者一起 rename 成正式文件。
pub fn save_atomic(index_dir: &Path, index: &Index, vectors: &Vectors) -> Result<(), IndexError>;

/// 将节点列表按 parent 组织成树，供锚点解析和子树遍历。只借用节点，不拥有数据。
pub struct Tree<'a> { /* 私有字段：按 key 的索引、按 parent 的子节点表、根表 */ }

impl<'a> Tree<'a> {
    /// parent 为 None 的命令节点成为根；parent 为 None 的参数节点忽略。
    pub fn build(nodes: &'a [Node]) -> Tree<'a>;
    /// 按名字查找根命令。
    pub fn root(&self, name: &str) -> Option<&'a Node>;
    /// 全部根命令名，按字典序。
    pub fn root_names(&self) -> impl Iterator<Item = &'a str> + '_;
    /// 按完整 key 查找节点。
    pub fn get(&self, key: &str) -> Option<&'a Node>;
    /// 直接子节点（命令与参数），按名字排序。
    pub fn children(&self, key: &str) -> Vec<&'a Node>;
    /// 子树后代与相对深度（直接子节点为 1），先序遍历，对环防护。
    pub fn descendants(&self, key: &str) -> Vec<(&'a Node, usize)>;
    /// 从父到根的祖先链，对环防护。
    pub fn ancestors(&self, key: &str) -> Vec<&'a Node>;
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("index io: {0}")]
    Io(#[from] std::io::Error),
    #[error("index json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported schema_version {0}, rebuild with `whatcmd reindex --full`")]
    SchemaVersion(u32),
    #[error("vectors.bin length {actual} does not match {expected} bytes")]
    VectorLength { expected: u64, actual: u64 },
}
```

### 8.2 向量（`index/vectors.rs`）

```rust
/// 行优先存储的向量矩阵，f32 小端序，行长 dimensions。写入前已 L2 归一化。
pub struct Vectors {
    pub dimensions: usize,
    pub data: Vec<f32>,
}

impl Vectors {
    pub fn rows(&self) -> usize;
    pub fn row(&self, i: usize) -> &[f32];
}

/// 纯函数：L2 归一化。零向量返回 Err。
pub fn normalize_vector(v: &mut [f32]) -> Result<(), VectorError>;

/// 纯函数：点积即余弦相似度（前提是两者都已归一化）。
pub fn dot(a: &[f32], b: &[f32]) -> f32;

/// 读取 vectors.bin，校验 len == rows × dimensions × 4。
pub fn read(path: &Path, dimensions: usize, rows: usize) -> Result<Vectors, VectorError>;

/// 写入 vectors.bin.tmp（不 rename，由 index::save_atomic 统一替换）。
pub fn write_tmp(path: &Path, vectors: &Vectors) -> Result<(), VectorError>;

#[derive(Debug, thiserror::Error)]
pub enum VectorError {
    #[error("vector io: {0}")]
    Io(#[from] std::io::Error),
    #[error("zero vector cannot be normalized")]
    ZeroVector,
    #[error("dimension mismatch: expected {expected}, got {actual}")]
    Dimension { expected: usize, actual: usize },
}
```

## 9. Embedding（`embed/`）

```rust
// embed/mod.rs
pub trait Embedder {
    fn model(&self) -> &str;
    fn dimensions(&self) -> usize;
    /// 批量嵌入。返回值与输入等长、顺序一致。
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// 纯函数：节点 → 嵌入文本。
/// 命令节点：`路径 + 描述 + 示例`；参数节点：`所属路径 + 参数名 + 参数描述`。
pub fn embed_text(node: &Node) -> String;

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("embedding unreachable: {0}")]
    Unreachable(String),          // 网络失败、DNS、连接超时
    #[error("embedding timed out after {0} ms")]
    Timeout(u64),
    #[error("embedding auth failed (check env {0})")]
    Auth(String),                 // 401/403，或环境变量缺失
    #[error("embedding response malformed: {0}")]
    BadResponse(String),
}
```

```rust
// embed/openai.rs
/// OpenAI 兼容接口客户端，POST {endpoint}/embeddings。
/// 密钥从 api_key_env 指定的环境变量读取，不写入日志。
pub struct OpenAiEmbedder {
    pub endpoint: String,
    pub model: String,
    pub dimensions: usize,
    pub timeout: Duration,
    api_key: Option<String>,      // 私有，构造时从环境变量读入
}

impl OpenAiEmbedder {
    pub fn from_config(cfg: &EmbeddingConfig) -> Result<Self, EmbedError>;
}

impl Embedder for OpenAiEmbedder { /* ... */ }
```

纯函数 `parse_embeddings_response(body: &[u8], expected: usize) -> Result<Vec<Vec<f32>>, EmbedError>` 放在 `embed/openai.rs` 内，供单元测试使用。

## 10. 查询（`search/`）

### 10.1 规范化（`search/normalize.rs`）

```rust
/// trim → 转小写 → 连续空白折叠。规范化后为空返回 Err。
pub fn normalize(raw: &str) -> Result<String, SearchError>;
```

### 10.2 锚点解析（`search/anchor.rs`）

```rust
/// 解析结果。
pub struct Anchor {
    pub key: Option<String>,      // 锚点命令节点的 key；没有已完成 token 时为 None，范围为全部节点
    pub root: String,             // 根命令名；没有已完成 token 时为空串
    pub dropped_partial: bool,    // 末尾未完成词是否已丢弃
    pub seen_flags: Vec<String>,  // 第一个独立 `--` 之前的长参数名（按 = 前的部分），用于去重
}

/// 插入动作：widget 先删除 LBUFFER 末尾 delete_tail 个字符（按 char 计），再写入 text。
pub struct Insertion {
    pub delete_tail: usize,       // 被丢弃的未完成词长度，如 `git cl` 为 2
    pub text: String,             // 可能为空
}

/// 纯函数：从 LBUFFER 解析锚点。不联网，不读文件。
/// - 末尾非空白的 token 视为未完成，丢弃。
/// - 第一个 token 不是根命令：返回 AnchorError::UnknownRoot，附带修正建议。
/// - 其余 token：`--` 单独出现则停止；`-` 开头跳过；子命令名则下降；其他停止。
pub fn resolve_anchor(lbuffer: &str, tree: &Tree<'_>) -> Result<Anchor, AnchorError>;

/// 纯函数：计算选中候选后对 LBUFFER 的修改。
/// - 候选不在锚点子树内：返回 None，由调用方处理（如回退结果）。
/// - 候选已是 LBUFFER 末尾的命令链：返回 text 为空串的 Insertion。
/// - 否则：删除未完成词，写入候选路径去掉锚点前缀后的剩余部分。
pub fn insertion_text(lbuffer: &str, anchor: &Anchor, candidate_key: &str) -> Option<Insertion>;

#[derive(Debug, thiserror::Error)]
pub enum AnchorError {
    #[error("no command named {typed}{}", hint(.suggestion))]
    UnknownRoot { typed: String, suggestion: Option<String> },
}
```

修正建议 `suggestion` 只基于编辑距离（≤ 2）给出一个候选，并列时取字典序靠前者。stderr 的文本为 `no command named giit, did you mean git?`，找不到候选时只输出 `no command named giit`。

### 10.3 打分（`search/score.rs`）

纯函数，系数先写成常量，用固定样本调参后再决定是否开放配置。

```rust
pub const DECAY: f32 = 0.8;               // 子树分的层级衰减
pub const MIN_SCORE: f32 = 0.30;          // 候选阈值
pub const USAGE_CAP: f32 = 0.10;          // 使用加分上限
pub const USAGE_ALPHA: f32 = 0.05;        // bonus = min(cap, alpha × ln(1 + count))
pub const USAGE_HALF_LIFE_DAYS: f32 = 30.0;

/// s_self(n) = cos(q, v_n)。
pub fn self_score(query_vec: &[f32], node_vec: &[f32]) -> f32;

/// s(n) = max(s_self(n), max over 后代 d of decay^(depth(d)−depth(n)) × s_self(d))。
/// `descendants` 为 (节点, 相对深度, 向量) 的列表。
pub fn subtree_score(self_score_of_node: f32, descendants: &[(f32, usize)]) -> (f32, Option<usize>);
// 返回 (最终分, 最佳后代在 descendants 中的索引)，后者用于生成 via。

/// bonus(n) = min(cap, alpha × ln(1 + count)) × 2^(−age_days / half_life)。
pub fn usage_bonus(count: u32, age_days: f32) -> f32;

/// 排序：exact 优先，其余按 score + bonus 降序，同分按 key 字典序。
pub fn rank(candidates: &mut [Candidate]);
```

### 10.4 查询编排（`search/mod.rs`）

```rust
pub struct QueryOptions {
    pub limit: usize,            // 默认 10
}

pub struct QueryContext<'a> {
    pub index: &'a Index,
    pub vectors: &'a Vectors,
    pub usage: &'a UsageSnapshot,
    pub embedder: &'a dyn Embedder,
}

/// 完整查询：规范化 → 锚点 → 嵌入 → 子树打分 → 回退 → 精确置顶 → 候选。
/// 锚点前缀错误时直接返回 SearchError::Anchor，不调用 embedder。
/// 嵌入失败返回 SearchError::Embed，对应退出码 3。
pub fn query(
    ctx: &QueryContext,
    lbuffer: &str,
    text: &str,
    opts: &QueryOptions,
) -> Result<Vec<Candidate>, SearchError>;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("empty query")]
    EmptyQuery,
    #[error(transparent)]
    Anchor(#[from] AnchorError),
    #[error(transparent)]
    Embed(#[from] EmbedError),
    #[error(transparent)]
    Vector(#[from] VectorError),
}
```

回退规则：锚点子树内没有节点超过 `MIN_SCORE` 时，依次回退到父节点，直到根。回退命中时候选的 `via` 填入层级说明。

## 11. 使用记录（`usage.rs`）

```rust
/// 一行使用记录：只有 key 与时间，不含查询文本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub key: String,
    pub at: String,              // RFC 3339
}

/// 内存中的聚合视图，供查询时读取。
pub struct UsageSnapshot {
    pub fn empty() -> Self;
    /// 返回 (次数, 最近一次选中距今的天数)。
    pub fn stat(&self, key: &str, now: &str) -> Option<(u32, f32)>;
}

/// 读取 usage.jsonl 为快照。文件不存在返回空快照。
pub fn load_snapshot(path: &Path) -> Result<UsageSnapshot, UsageError>;

/// 追加一条记录。仅写 key 与当前时间。
pub fn append(path: &Path, key: &str, now: &str) -> Result<(), UsageError>;

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("usage io: {0}")]
    Io(#[from] std::io::Error),
    #[error("usage json: {0}")]
    Json(#[from] serde_json::Error),
}
```

## 12. 输出（`output.rs`）

```rust
/// 一行候选：key \t score \t kind \t insert \t description \t example \t dangerous \t via
/// 字段内的 \t \n 已在生成阶段去除，这里只做兜底替换。
pub fn format_line(c: &Candidate) -> String;

/// --json 模式：每个候选一个 JSON 对象。
pub fn format_json(c: &Candidate) -> serde_json::Value;
```

## 13. Agent 适配层（`agent/`）

```rust
// agent/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    NonInteractive,   // claude -p
    JsonOutput,       // --output-format json
    Mcp,              // --mcp-config
    ToolAllowlist,    // --allowedTools
    RestrictedTools,  // --tools，限定内置工具集
}

pub type CapabilitySet = std::collections::BTreeSet<Capability>;

/// reindex 要求的全部能力（D7 起含 StrictMcp）。
pub const REQUIRED: [Capability; 6] = [/* 与 Capability 变体一一对应 */];

/// 返回 REQUIRED 中不在 set 里的能力，保持声明顺序。
pub fn missing_capabilities(set: &CapabilitySet) -> Vec<Capability>;

pub struct BatchRequest<'a> {
    pub run_id: &'a str,
    pub paths: &'a [Vec<CommandName>], // 本批次的节点路径（命令节点或其父命令）
    pub prompt: &'a str,         // 来自 prompts/describe.md，已附父节点上下文
    pub mcp_config: &'a Path,    // 指向临时生成的 mcp.json
}

pub struct BatchOutcome {
    pub ok: bool,
    pub raw_output: String,      // 仅用于 debug 日志，截断，描述以暂存区为准
}

pub trait AgentBackend {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> &CapabilitySet;
    fn run_batch(&self, req: &BatchRequest) -> Result<BatchOutcome, AgentError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("missing capabilities: {0:?}")]
    MissingCapability(Vec<Capability>),
    #[error("agent exited with status {0}")]
    ExitStatus(i32),
    #[error("agent timed out")]
    Timeout,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
```

```rust
// agent/detect.rs
pub struct Detected {
    pub backend_id: &'static str,
    pub path: PathBuf,
    pub version: String,
    pub capabilities: CapabilitySet,
}

/// 按配置决定探测哪些后端。Auto 时探测 claude。PATH 查找，不硬编码路径。
pub fn detect(cfg: &AgentConfig, path_var: &str) -> Result<Detected, DetectError>;

/// 纯函数：从 --help 文本解析能力。缺少所需参数则对应能力不加入集合。
pub fn parse_capabilities(help_text: &str) -> CapabilitySet;

#[derive(Debug, thiserror::Error)]
pub enum DetectError {
    #[error("no usable agent backend; checked: {0:?}")]
    NoneAvailable(Vec<PathBuf>),
    #[error("version probe failed for {0}")]
    VersionProbe(PathBuf),
}
```

```rust
// agent/claude.rs
/// 纯函数：拼装 claude 命令行参数。快照测试的主要对象。
/// 包含 -p、--output-format json、--strict-mcp-config、--mcp-config、--tools、--allowedTools。
/// --allowedTools 只含四个 whatcmd 工具；不包含任何 Bash 相关工具名。
pub fn build_args(req: &BatchRequest) -> Vec<OsString>;

/// 解析 --output-format json 的输出，只取是否成功与错误信息。
pub fn parse_output(stdout: &[u8]) -> Result<BatchOutcome, AgentError>;

pub struct ClaudeBackend {
    pub bin: PathBuf,
    pub capabilities: CapabilitySet,
    pub timeout: Duration,
}

impl AgentBackend for ClaudeBackend { /* ... */ }
```

`--tools` 的确切取值与 `--allowedTools` 的配合方式，要在 D8 中用真实调用确认后再固定，见第 19 节。

## 14. 预置集（`seed.rs`）

```rust
/// 编译时嵌入的预置集：common.json 与平台专属文件。
/// 返回的节点 pinned = true，vector_row 指向随发布附带的向量。
pub fn load_seed(platform: Platform) -> Result<SeedSet, SeedError>;

pub enum Platform {
    Linux,
    MacOs,
}

pub struct SeedSet {
    pub nodes: Vec<Node>,
    pub vectors: Vectors,        // 对应默认 embedding 模型
    pub model: String,
    pub dimensions: usize,
}

/// 纯函数：预置节点是否与当前 embedding 模型一致。不一致时调用方只对描述重新 embed。
pub fn needs_reembed(seed: &SeedSet, model: &str, dimensions: usize) -> bool;

#[derive(Debug, thiserror::Error)]
pub enum SeedError {
    #[error("seed json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("seed vector mismatch: {0}")]
    Mismatch(String),
}
```

## 15. 自动触发（`auto.rs`）

```rust
/// 纯函数：PATH 指纹。对根命令名（排除预置节点）排序后取 sha256。
pub fn fingerprint(commands: &[CommandEntry], seed_roots: &[CommandName]) -> String;

/// 纯函数：是否需要后台重建。
/// 指纹相同、或 reindex.auto = false 时返回 false。
pub fn should_run(enabled: bool, current: &str, stored: Option<&str>) -> bool;

/// 读写指纹文件（cache_dir/fingerprint）。只在 --auto 成功完成后写入。
pub fn read_stored(cache_dir: &Path) -> Option<String>;
pub fn write_stored(cache_dir: &Path, fp: &str) -> Result<(), std::io::Error>;
```

## 16. reindex（`generate/`）

### 16.1 暂存区（`generate/staging.rs`）

```rust
/// 追加一条已通过校验的记录到 staging_dir/<run_id>.jsonl。
pub fn append(staging_dir: &Path, run_id: &str, rec: &StagedRecord) -> Result<(), StagingError>;

/// 读取全部记录；同一 key 以最后一条为准。
pub fn read_all(staging_dir: &Path, run_id: &str) -> Result<Vec<StagedRecord>, StagingError>;

/// 删除已合并的记录（按 key）。全部处理完后可整体删除文件。
pub fn remove_keys(staging_dir: &Path, run_id: &str, keys: &[String]) -> Result<(), StagingError>;

pub fn remove(staging_dir: &Path, run_id: &str) -> Result<(), StagingError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedRecord {
    pub key: String,
    pub kind: NodeKind,
    pub description: String,
    pub examples: Vec<String>,
    pub written_at: String,
}
```

### 16.2 增量计划（`generate/plan.rs`，纯逻辑）

```rust
pub struct Plan {
    pub keep: Vec<String>,                 // source_hash 未变，直接复用
    pub regenerate: Vec<Vec<CommandName>>, // 需要交给 agent 的节点路径
    pub remove: Vec<String>,               // 根命令已不在 PATH 的非预置子树
}

/// 纯函数。预置节点不参与比较与删除，除非出现在 names 中。
pub fn make_plan(
    roots: &[CommandEntry],
    old: Option<&Index>,
    names: Option<&[String]>,
    docs_hash: &dyn Fn(&[CommandName]) -> Option<String>, // 返回 source_hash 或 None（无文档）
    prompt_version: u32,
) -> Plan;

/// source_hash = sha256(doc.text + prompt_version + embedding model)。
/// 包含模型标识，换模型后增量逻辑不会复用旧向量相关的描述。
pub fn source_hash(doc_text: &str, prompt_version: u32, model: &str) -> String;
```

注：`source_hash` 包含 embedding 模型标识，是为了防止换模型后误复用。描述本身不依赖模型，这里是保守选择，见 DEVLOG。

### 16.3 编排（`generate/mod.rs`）

```rust
pub struct ReindexOptions {
    pub full: bool,
    pub names: Option<Vec<String>>,        // --names 只重跑指定节点，可含预置节点
    pub quiet: bool,
    pub auto: bool,                        // --auto：日志只写文件，失败不打扰用户
}

pub struct ReindexReport {
    pub generated: usize,
    pub reused: usize,
    pub removed: usize,
    pub embed_pending: usize,              // 阶段 B 失败、保留在暂存区的节点数
    pub failed: Vec<(String, String)>,     // (key, reason)
}

/// 阶段 A：分批调用 agent，结果写入暂存区。单批失败只影响该批。
pub fn generate_descriptions(
    cfg: &Config,
    backend: &dyn AgentBackend,
    plan: &Plan,
    run_id: &str,
) -> Result<Vec<(String, String)>, GenerateError>; // 返回失败的 (key, reason)

/// 阶段 B：读取暂存区，校验，按 batch_size 分批做 embedding，合并成新索引并原子替换。
/// embedding 失败的批次保留暂存区记录，新节点不写入索引，旧向量保留。
pub fn embed_and_merge(
    cfg: &Config,
    embedder: &dyn Embedder,
    index_dir: &Path,
    run_id: &str,
) -> Result<ReindexReport, GenerateError>;

/// 完整流程：锁 → 扫描 → plan → 阶段 A → 阶段 B → 原子写索引 → 清理暂存区 → 释放锁。
/// 退出码由 ReindexReport 决定：failed 为空返回 0，否则 1。
pub fn reindex(
    cfg: &Config,
    backend: &dyn AgentBackend,
    embedder: &dyn Embedder,
    opts: &ReindexOptions,
) -> Result<ReindexReport, GenerateError>;

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("another reindex is running (lock held)")]
    Locked,
    #[error(transparent)]
    Index(#[from] IndexError),
    #[error(transparent)]
    Staging(#[from] StagingError),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Vector(#[from] VectorError),
}
```

锁：`index_dir/index.lock`，用 `flock`，进程退出自动释放。`--auto` 遇到已加锁时直接退出，不输出。

批次：按树结构组织。同一批尽量包含同一父节点的子节点，并把父节点当前描述放入提示词作为上下文。`batch_size` 是节点数上限。

## 17. 查询 widget 与 CLI（`cli.rs`、`main.rs`）

```rust
// cli.rs
#[derive(Parser)]
#[command(name = "whatcmd")]
pub struct Cli {
    #[arg(long, env = "WHATCMD_CONFIG")]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// 锚点解析 + 向量匹配，输出候选行到 stdout
    Query {
        /// LBUFFER，光标左侧的命令文本，用于锚点解析
        #[arg(long, default_value = "")]
        prefix: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        json: bool,
        /// 自然语言查询文本；放在 -- 之后
        text: Vec<String>,
    },
    /// 扫描 PATH，生成描述与参数，写索引
    Reindex {
        #[arg(long)]
        full: bool,
        #[arg(long, value_delimiter = ',')]
        names: Vec<String>,
        /// shell 初始化时调用：指纹未变则静默退出
        #[arg(long)]
        auto: bool,
        #[arg(long)]
        quiet: bool,
    },
    /// 记录一次选中，写入使用记录
    Record {
        key: String,
    },
    /// 检查配置、索引、agent 能力、embedding 环境变量
    Doctor,
}
```

退出码（`main.rs` 映射，业务代码不直接 `exit`）：

```rust
pub enum ExitKind {
    Ok = 0,        // 成功，有结果
    NoResult = 1,  // 无结果，或 reindex 有失败
    Config = 2,    // 配置、索引、参数错误，或锚点前缀有误
    Embed = 3,     // embedding 服务不可用
}
```

`main.rs` 只做解析、调用、映射退出码，不含业务逻辑。`query` 的输出只写 stdout，所有状态与警告写 stderr。`--auto` 的 stderr 重定向到 `cache_dir/reindex.log`，日志只写该文件。

## 18. MCP 层（`mcp/`）

> 已移除（2026-11-05）：reindex 改为程序给文档、模型返回 JSON，`mcp/` 模块、`whatcmd mcp` 子命令与 `rmcp`、`tokio` 依赖均已删除。以下仅作历史记录。

```rust
// mcp/tools.rs
/// 每个函数都先经过 guard，再调用底层模块。
/// 工具函数本身不依赖 rmcp，便于单元测试；rmcp 的注册只是薄包装。
pub fn read_man(name: &str, ctx: &ToolContext) -> Result<String, ToolError>;
pub fn read_help(path: &[&str], ctx: &ToolContext) -> Result<String, ToolError>;
pub fn write_description(args: WriteDescriptionArgs, ctx: &ToolContext) -> Result<(), ToolError>;
pub fn write_params(args: WriteParamsArgs, ctx: &ToolContext) -> Result<(), ToolError>;

#[derive(Debug, Deserialize)]
pub struct WriteDescriptionArgs {
    pub path: Vec<String>,        // 命令路径，如 ["git", "clone"]
    pub description: String,
    pub examples: Vec<String>,
    pub subcommands: Vec<String>, // 下一轮队列，每项须通过命名校验
}

#[derive(Debug, Deserialize)]
pub struct WriteParamsArgs {
    pub path: Vec<String>,
    pub params: Vec<ParamArg>,
}

#[derive(Debug, Deserialize)]
pub struct ParamArg {
    pub flag: String,             // 必须是 --长参数
    pub description: String,
}

pub struct ToolContext {
    pub run_id: String,
    pub staging_dir: PathBuf,
    pub roots: Vec<CommandEntry>,  // 本次 run 开始时扫描的根命令快照
    pub known_nodes: Vec<String>,  // 本次 run 可见的节点 key，用于校验 path 的父节点
    pub max_depth: usize,
    pub docs: DocsConfig,
}

// mcp/mod.rs
/// 以 stdio 运行 MCP server，直到 stdin 关闭。同步函数，内部创建单线程 tokio 运行时并阻塞。
pub fn serve_stdio(ctx: ToolContext) -> Result<(), McpError>;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("unknown command {0}: not found in this run's PATH scan")]
    UnknownCommand(String),
    #[error("{0} is not in this run's command tree; describe its parent first")]
    NotInTree(String),
    #[error("path exceeds max_depth {0}")]
    TooDeep(usize),
    #[error(transparent)]
    Guard(#[from] GuardError),
    #[error(transparent)]
    Doc(#[from] DocError),
    #[error(transparent)]
    Staging(#[from] StagingError),
}
```

rmcp 3.x 的 trait 与宏写法在实现 MCP demo 时对照 docs.rs 确认，这里只约定上层看到的函数。

## 19. 测试接口

- 纯函数全部可直接单元测试：`normalize`、`resolve_anchor`、`insertion_text`、`self_score`、`subtree_score`、`usage_bonus`、`rank`、`clean`、`validate_output`、`validate_flag`、`parse_capabilities`、`build_args`、`make_plan`、`commands_from_path`、`fingerprint`、`should_run`、`embed_text`、`parse_embeddings_response`。
- 文件系统相关函数接受目录参数，测试时传 `tempfile` 目录，不依赖真实 `$HOME`。
- `AgentBackend` 有假实现 `FakeBackend`，返回预设的暂存记录，驱动 reindex 全流程。
- `Embedder` 有假实现 `FakeEmbedder`，按文本哈希生成确定性向量，可注入失败，用于阶段 B 的失败隔离测试。
- 所有测试不访问网络，不调用真实 `claude`。

## 20. Demo 拆分与顺序

每个 demo 只依赖前面已经打通的模块，独立运行、独立验收。

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

D1 到 D5 不依赖 agent，可以先完成。`embed` 的真实调用在 D8 前用 `FakeEmbedder` 替代。D7、D8 之前必须完成第 11 节的安全边界验证。

## 21. 待确认

| 项目 | 说明 |
|---|---|
| `--tools` 取值与 `--allowedTools` 组合 | 需要 D7 或 D8 中用真实调用确认 |
| `RestrictedTools` 能力判定 | 依赖 `--help` 解析，需确认 `--tools` 的解析规则 |
| rmcp 3.x API | D9 中对照 docs.rs 确认 |
| embedding HTTP 客户端与精确版本 | 实现时选定，写入 DEVLOG |
| 打分系数 | `DECAY`、`MIN_SCORE`、`USAGE_*` 为常量，待固定样本调参 |
| 子命令发现 | 由 `write_description` 的 `subcommands` 分轮展开，待 D8 样本验证 |
| `via` 展示格式 | 暂定 `git  (pull: ...)`，待样本确认 |
| AGENT.md 第 6 节 demo 表 | 仍引用旧模块（如 `fuzzy_query`、`EnglishTokenizer`），需与第 20 节同步 |
| 快捷键 | `Ctrl-G` |
