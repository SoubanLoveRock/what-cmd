//! 配置文件读取、默认值、`~` 展开与校验。
//!
//! 只读取与解析，不写文件。密钥不进配置：配置只记录环境变量名，
//! 出现 `api_key` 字段直接报错。

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

/// 覆盖配置路径的环境变量名。
pub const CONFIG_ENV: &str = "WHATCMD_CONFIG";

/// 配置中禁止出现的字段名。
const FORBIDDEN_KEY: &str = "api_key";

/// 完整配置，对应 `config.toml`。缺失的表与字段使用默认值。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Config {
    /// 路径配置。
    pub paths: PathsConfig,
    /// agent 配置。
    pub agent: AgentConfig,
    /// reindex 配置。
    pub reindex: ReindexConfig,
    /// embedding 配置。`endpoint`、`model`、`dimensions` 无默认值，由 [`validate`] 检查。
    pub embedding: EmbeddingConfig,
    /// 文档获取配置。
    pub docs: DocsConfig,
}

/// 路径配置。`~` 由 [`expand_paths`] 展开。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    /// 索引目录，默认 `~/.local/share/whatcmd`。
    pub index_dir: PathBuf,
    /// 暂存区目录，默认 `~/.cache/whatcmd/staging`。
    pub staging_dir: PathBuf,
    /// 使用记录文件，默认 `~/.cache/whatcmd/usage.jsonl`。其父目录即缓存目录。
    pub usage_file: PathBuf,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            index_dir: PathBuf::from("~/.local/share/whatcmd"),
            staging_dir: PathBuf::from("~/.cache/whatcmd/staging"),
            usage_file: PathBuf::from("~/.cache/whatcmd/usage.jsonl"),
        }
    }
}

/// agent 配置。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// 后端选择，默认 `auto`。
    pub backend: BackendChoice,
    /// 每批的节点数上限，默认 20。
    pub batch_size: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            backend: BackendChoice::Auto,
            batch_size: 20,
        }
    }
}

/// agent 后端选择。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BackendChoice {
    /// 自动探测，目前即 `claude`。
    #[default]
    Auto,
    /// 固定使用 Claude Code。
    ClaudeCode,
    /// 不使用 agent，reindex 无法生成描述。
    None,
}

/// reindex 配置。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct ReindexConfig {
    /// shell 初始化时是否自动后台重建，默认 `true`。
    pub auto: bool,
    /// 命令树最大深度，默认 3。
    pub max_depth: usize,
}

impl Default for ReindexConfig {
    fn default() -> Self {
        Self {
            auto: true,
            max_depth: 3,
        }
    }
}

/// embedding 配置。只存环境变量名，密钥本身不进配置。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    /// OpenAI 兼容接口地址。无默认值，必须由用户配置。
    pub endpoint: String,
    /// 模型名。无默认值，必须由用户配置。
    pub model: String,
    /// 向量维度。无默认值，必须由用户配置。
    pub dimensions: usize,
    /// 存放密钥的环境变量名，默认 `WHATCMD_EMBEDDING_API_KEY`。
    pub api_key_env: String,
    /// 单次请求超时（毫秒），默认 3000。
    pub timeout_ms: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            model: String::new(),
            dimensions: 0,
            api_key_env: "WHATCMD_EMBEDDING_API_KEY".to_owned(),
            timeout_ms: 3000,
        }
    }
}

/// 文档获取配置。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct DocsConfig {
    /// `man` 超时（毫秒），默认 3000。
    pub man_timeout_ms: u64,
    /// `--help` 超时（毫秒），默认 2000。
    pub help_timeout_ms: u64,
    /// 文档原文截断上限（字节），默认 65536。
    pub max_doc_bytes: usize,
}

impl Default for DocsConfig {
    fn default() -> Self {
        Self {
            man_timeout_ms: 3000,
            help_timeout_ms: 2000,
            max_doc_bytes: 65536,
        }
    }
}

/// 配置错误。
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// 读取配置文件失败。
    #[error("config io: {0}")]
    Io(#[from] io::Error),
    /// TOML 解析失败。
    #[error("config parse: {0}")]
    Parse(#[from] toml::de::Error),
    /// 配置中出现禁止的字段。
    #[error("forbidden key `{0}` in config; keys must come from the environment")]
    ForbiddenKey(String),
    /// embedding 的必填字段缺失或为空。
    #[error("embedding.{0} is required")]
    MissingEmbeddingField(&'static str),
    /// 未设置 `HOME`，无法确定默认路径。
    #[error("cannot determine home directory: HOME is not set")]
    NoHome,
}

/// 从路径加载配置。`path` 为 `None` 时依次取 `WHATCMD_CONFIG` 与默认路径。
///
/// 文件不存在时返回默认配置（不做 embedding 校验），由调用方在需要时检查。
/// 文件存在时解析、校验，并展开路径中的 `~`。
pub fn load(path: Option<&Path>) -> Result<Config, ConfigError> {
    let home = home_dir()?;
    let env = std::env::var_os(CONFIG_ENV);
    let path = resolve_config_path(path, env, &home);
    load_from(&path, &home)
}

/// 从指定文件加载配置，`home` 用于展开 `~`。测试时传临时目录，不读真实 `$HOME`。
pub fn load_from(path: &Path, home: &Path) -> Result<Config, ConfigError> {
    let mut cfg = match fs::read_to_string(path) {
        Ok(text) => {
            let cfg = parse(&text)?;
            validate(&cfg)?;
            info!(path = %path.display(), "config loaded");
            cfg
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Config::default(),
        Err(err) => return Err(err.into()),
    };
    expand_paths(&mut cfg, home);
    Ok(cfg)
}

/// 解析 TOML 字符串。任意层级出现 `api_key` 键即返回 `ForbiddenKey`。不做校验，不展开路径。
pub fn parse(toml_text: &str) -> Result<Config, ConfigError> {
    let table: toml::Table = toml::from_str(toml_text)?;
    if contains_forbidden_key(&table) {
        warn!(key = FORBIDDEN_KEY, "forbidden key in config");
        return Err(ConfigError::ForbiddenKey(FORBIDDEN_KEY.to_owned()));
    }
    Ok(toml::from_str(toml_text)?)
}

/// 检查必填的 embedding 字段：`endpoint`、`model` 非空，`dimensions` 大于 0。
pub fn validate(cfg: &Config) -> Result<(), ConfigError> {
    let embedding = &cfg.embedding;
    if embedding.endpoint.trim().is_empty() {
        return Err(ConfigError::MissingEmbeddingField("endpoint"));
    }
    if embedding.model.trim().is_empty() {
        return Err(ConfigError::MissingEmbeddingField("model"));
    }
    if embedding.dimensions == 0 {
        return Err(ConfigError::MissingEmbeddingField("dimensions"));
    }
    Ok(())
}

/// 把 `paths` 中的 `~` 替换为 `home`。不以 `~` 开头的路径保持不变。
pub fn expand_paths(cfg: &mut Config, home: &Path) {
    let paths = &mut cfg.paths;
    paths.index_dir = expand_home(&paths.index_dir, home);
    paths.staging_dir = expand_home(&paths.staging_dir, home);
    paths.usage_file = expand_home(&paths.usage_file, home);
}

/// 纯函数：确定配置文件路径。优先级为显式路径、`WHATCMD_CONFIG`、`home/.config/whatcmd/config.toml`。
pub fn resolve_config_path(explicit: Option<&Path>, env: Option<OsString>, home: &Path) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if let Some(path) = env {
        return PathBuf::from(path);
    }
    home.join(".config").join("whatcmd").join("config.toml")
}

/// 读取 embedding 密钥。环境变量未设置或为空时返回 `None`，由调用方决定报错或 `doctor` 提示。
///
/// 密钥值不写入日志。
pub fn embedding_api_key(cfg: &EmbeddingConfig) -> Option<String> {
    std::env::var(&cfg.api_key_env)
        .ok()
        .filter(|value| !value.is_empty())
}

/// 把以 `~` 开头的路径展开为 `home` 下的路径。
fn expand_home(path: &Path, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// 读取 `HOME` 环境变量。
fn home_dir() -> Result<PathBuf, ConfigError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(ConfigError::NoHome)
}

/// 递归检查表中是否含 `api_key` 键。
fn contains_forbidden_key(table: &toml::Table) -> bool {
    table
        .iter()
        .any(|(key, value)| key.as_str() == FORBIDDEN_KEY || value_contains_forbidden_key(value))
}

/// 递归检查值（表或数组中的表）。
fn value_contains_forbidden_key(value: &toml::Value) -> bool {
    match value {
        toml::Value::Table(table) => contains_forbidden_key(table),
        toml::Value::Array(items) => items.iter().any(value_contains_forbidden_key),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        BackendChoice, ConfigError, embedding_api_key, expand_paths, load_from, parse,
        resolve_config_path, validate,
    };
    use crate::testutil::tempdir;

    const FULL: &str = r#"
[paths]
index_dir = "~/data/whatcmd"
staging_dir = "~/cache/staging"
usage_file = "~/cache/usage.jsonl"

[agent]
backend = "claude-code"
batch_size = 5

[reindex]
auto = false
max_depth = 2

[embedding]
endpoint = "http://localhost:8080/v1"
model = "local-model"
dimensions = 384
api_key_env = "MY_KEY"
timeout_ms = 1500

[docs]
man_timeout_ms = 100
help_timeout_ms = 200
max_doc_bytes = 1024
"#;

    #[test]
    fn parses_full_example() {
        let cfg = parse(FULL).unwrap();
        assert_eq!(cfg.agent.backend, BackendChoice::ClaudeCode);
        assert_eq!(cfg.agent.batch_size, 5);
        assert!(!cfg.reindex.auto);
        assert_eq!(cfg.reindex.max_depth, 2);
        assert_eq!(cfg.embedding.dimensions, 384);
        assert_eq!(cfg.embedding.api_key_env, "MY_KEY");
        assert_eq!(cfg.docs.max_doc_bytes, 1024);
    }

    #[test]
    fn partial_file_fills_defaults() {
        let cfg = parse("[agent]\nbatch_size = 7\n").unwrap();
        assert_eq!(cfg.agent.batch_size, 7);
        assert_eq!(cfg.agent.backend, BackendChoice::Auto);
        assert_eq!(cfg.docs.man_timeout_ms, 3000);
    }

    #[test]
    fn top_level_api_key_is_rejected() {
        assert!(matches!(
            parse("api_key = \"x\"\n"),
            Err(ConfigError::ForbiddenKey(key)) if key == "api_key"
        ));
    }

    #[test]
    fn nested_api_key_is_rejected() {
        assert!(matches!(
            parse("[embedding]\nendpoint = \"e\"\napi_key = \"x\"\n"),
            Err(ConfigError::ForbiddenKey(_))
        ));
    }

    #[test]
    fn api_key_inside_array_of_tables_is_rejected() {
        assert!(matches!(
            parse("[extra.deep]\nitems = [{ api_key = \"x\" }]\n"),
            Err(ConfigError::ForbiddenKey(_))
        ));
    }

    #[test]
    fn tilde_expands_with_given_home() {
        let mut cfg = parse(FULL).unwrap();
        expand_paths(&mut cfg, Path::new("/home/test"));
        assert_eq!(
            cfg.paths.index_dir,
            PathBuf::from("/home/test/data/whatcmd")
        );
        assert_eq!(
            cfg.paths.usage_file,
            PathBuf::from("/home/test/cache/usage.jsonl")
        );
    }

    #[test]
    fn missing_file_returns_expanded_defaults() {
        let dir = tempdir();
        let cfg = load_from(&dir.path().join("absent.toml"), dir.path()).unwrap();
        assert_eq!(cfg.paths.index_dir, dir.path().join(".local/share/whatcmd"));
        assert_eq!(cfg.agent.batch_size, 20);
        assert!(cfg.embedding.endpoint.is_empty());
    }

    #[test]
    fn full_file_loads_and_expands_under_given_home() {
        let dir = tempdir();
        let path = dir.path().join("config.toml");
        fs::write(&path, FULL).unwrap();
        let cfg = load_from(&path, dir.path()).unwrap();
        assert_eq!(cfg.paths.index_dir, dir.path().join("data/whatcmd"));
    }

    #[test]
    fn missing_endpoint_fails_validation() {
        let dir = tempdir();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[agent]\nbatch_size = 3\n").unwrap();
        assert!(matches!(
            load_from(&path, dir.path()),
            Err(ConfigError::MissingEmbeddingField("endpoint"))
        ));
    }

    #[test]
    fn zero_dimensions_fails_validation() {
        let cfg = parse("[embedding]\nendpoint = \"e\"\nmodel = \"m\"\ndimensions = 0\n").unwrap();
        assert!(matches!(
            validate(&cfg),
            Err(ConfigError::MissingEmbeddingField("dimensions"))
        ));
    }

    #[test]
    fn config_path_prefers_explicit_then_env_then_default() {
        let home = Path::new("/h");
        assert_eq!(
            resolve_config_path(
                Some(Path::new("/a.toml")),
                Some(OsString::from("/b.toml")),
                home
            ),
            PathBuf::from("/a.toml")
        );
        assert_eq!(
            resolve_config_path(None, Some(OsString::from("/b.toml")), home),
            PathBuf::from("/b.toml")
        );
        assert_eq!(
            resolve_config_path(None, None, home),
            PathBuf::from("/h/.config/whatcmd/config.toml")
        );
    }

    #[test]
    fn unset_api_key_env_yields_none() {
        let mut cfg = parse(FULL).unwrap().embedding;
        cfg.api_key_env = "WHATCMD_TEST_UNSET_KEY_8F3A".to_owned();
        assert_eq!(embedding_api_key(&cfg), None);
    }
}
