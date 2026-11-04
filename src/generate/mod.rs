//! reindex 生成流程：锁、增量计划、暂存区、阶段 A（分批调用 agent）与阶段 B（embedding 与合并）。
//! 入口为 [`reindex`]。

pub mod merge;
pub mod output;
pub mod plan;
pub mod staging;

use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, TryLockError};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use tracing::warn;

use crate::agent::{AgentBackend, AgentError, BatchOutcome, BatchRequest};
use crate::config::Config;
use crate::embed::Embedder;
use crate::generate::merge::{NodeSource, Stamp, merge_staged};
use crate::generate::plan::{Plan, make_plan, source_hash};
use crate::generate::staging::StagingError;
use crate::guard::name::validate_command;
use crate::index::{self, IndexError};
use crate::types::{CommandEntry, CommandName, DocSource};

/// 描述提示词，嵌入二进制。内容变化时须同步提升 [`PROMPT_VERSION`]。
const DESCRIBE_PROMPT: &str = include_str!("prompts/describe.md");

/// 提示词版本，参与 `source_hash`，变化即触发增量重建。
pub const PROMPT_VERSION: u32 = 6;

/// 提示词中单个命令文档的字节上限。超出部分截断；校验仍用完整文档。
const PROMPT_DOC_BYTES: usize = 16 * 1024;

/// 返回提示词中使用的文档片段，在字符边界处截断。
fn prompt_doc(text: &str) -> &str {
    if text.len() <= PROMPT_DOC_BYTES {
        return text;
    }
    let mut end = PROMPT_DOC_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// 生成与合并阶段的错误。单批 embedding 失败不是错误，见 [`MergeSummary::failed`]。
#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    /// 索引读写失败。
    #[error(transparent)]
    Index(#[from] IndexError),
    /// 暂存区读写失败。
    #[error(transparent)]
    Staging(#[from] StagingError),
    /// 锁文件读写失败。
    #[error("lock io: {0}")]
    Io(#[from] io::Error),
    /// 已有另一次 reindex 持有 `index.lock`。
    #[error("another reindex is running (lock held)")]
    Locked,
    /// 旧索引的 embedding 模型与当前不同，拒绝混用向量。
    #[error("embedding model changed from {old} to {new}; the existing index cannot be merged")]
    ModelChanged { old: String, new: String },
    /// 旧索引的向量维度与当前不同，拒绝混用向量。
    #[error(
        "embedding dimensions changed from {expected} to {actual}; the existing index cannot be merged"
    )]
    Dimension { expected: usize, actual: usize },
}

/// 阶段 B 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeSummary {
    /// 本次写入索引的节点数。
    pub merged: usize,
    /// 未能合并的 `(key, reason)`。对应记录保留在暂存区，下次直接 embedding。
    pub failed: Vec<(String, String)>,
}

/// 阶段 A：按 `batch_size` 分批调用 agent。程序把每个命令的文档放进提示词，agent 返回 JSON，
/// 经 [`output::to_records`] 校验后写入暂存区。
///
/// 单批失败只影响该批的节点，其余批次继续。返回失败的 `(key, reason)`。
/// 描述本身不从返回值读取，以暂存区为准；成功但暂存区无记录的节点由调用方判为失败。
pub fn generate_descriptions(
    cfg: &Config,
    backend: &dyn AgentBackend,
    plan: &Plan,
    run_id: &str,
    fetch: &DocFetch<'_>,
    log: Option<&Path>,
) -> Vec<(String, String)> {
    let batch_size = cfg.agent.batch_size.max(1);
    let schema = output::schema();
    // 文档获取不是线程安全的（reindex 在闭包里记录来源），先串行准备好每批的提示词。
    let prepared: Vec<PreparedBatch<'_>> = plan
        .regenerate
        .chunks(batch_size)
        .map(|batch| prepare_batch(batch, fetch))
        .collect();
    let outcomes = run_batches(backend, &prepared, run_id, &schema, cfg.agent.concurrency);
    let mut failed = Vec::new();
    // 结果按批次顺序处理，日志、暂存与失败列表的顺序与串行执行一致。
    for (batch, result) in prepared.iter().zip(outcomes) {
        let PreparedBatch {
            paths, keys, docs, ..
        } = batch;
        let reason = match result {
            Ok(outcome) => {
                append_log(
                    log,
                    run_id,
                    &format!(
                        "batch ok={} result={}",
                        outcome.ok,
                        preview(&outcome.raw_output)
                    ),
                );
                if outcome.ok {
                    match output::parse_batch(&outcome.raw_output) {
                        Ok(parsed) => {
                            for rec in output::to_records(&parsed, keys, docs) {
                                if let Err(err) =
                                    staging::append(&cfg.paths.staging_dir, run_id, &rec)
                                {
                                    append_log(log, run_id, &format!("stage-failed: {err}"));
                                }
                            }
                        }
                        Err(err) => {
                            append_log(log, run_id, &format!("batch unparseable: {err}"));
                        }
                    }
                    continue;
                }
                "agent reported an error".to_owned()
            }
            Err(err) => {
                append_log(log, run_id, &format!("batch error: {err}"));
                err.to_string()
            }
        };
        failed.extend(paths.iter().map(|path| (key_of(path), reason.clone())));
    }
    failed
}

/// 一批已准备好的输入：节点、key、完整文档（供校验）与提示词。
struct PreparedBatch<'a> {
    paths: &'a [Vec<CommandName>],
    keys: Vec<String>,
    docs: HashMap<String, String>,
    prompt: String,
}

/// 获取一批节点的文档并拼出提示词。无文档的节点不出现在提示词里。
fn prepare_batch<'a>(batch: &'a [Vec<CommandName>], fetch: &DocFetch<'_>) -> PreparedBatch<'a> {
    let keys: Vec<String> = batch.iter().map(|path| key_of(path)).collect();
    let mut docs: HashMap<String, String> = HashMap::new();
    let mut prompt = String::from(DESCRIBE_PROMPT);
    for (path, key) in batch.iter().zip(&keys) {
        if let Some((_, text)) = fetch(path.as_slice()) {
            prompt.push_str(&format!("\n## {key}\n{}\n", prompt_doc(&text)));
            docs.insert(key.clone(), text);
        }
    }
    PreparedBatch {
        paths: batch,
        keys,
        docs,
        prompt,
    }
}

/// 以最多 `concurrency` 个线程并发执行各批，结果按输入顺序返回。
///
/// 每次 agent 调用有固定的启动开销，且大部分时间在等待模型，并发能显著缩短总耗时。
fn run_batches(
    backend: &dyn AgentBackend,
    prepared: &[PreparedBatch<'_>],
    run_id: &str,
    schema: &str,
    concurrency: usize,
) -> Vec<Result<BatchOutcome, AgentError>> {
    let workers = concurrency.max(1).min(prepared.len());
    let run = |batch: &PreparedBatch<'_>| {
        backend.run_batch(&BatchRequest {
            run_id,
            paths: batch.paths,
            prompt: &batch.prompt,
            schema,
        })
    };
    if workers <= 1 {
        return prepared.iter().map(run).collect();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<Result<BatchOutcome, AgentError>>>> =
        prepared.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(batch) = prepared.get(i) else {
                        break;
                    };
                    let result = run(batch);
                    *slots[i].lock().unwrap_or_else(PoisonError::into_inner) = Some(result);
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .unwrap_or_else(PoisonError::into_inner)
                .unwrap_or_else(|| Err(AgentError::BadOutput("batch worker panicked".to_owned())))
        })
        .collect()
}

/// 阶段 B：读取暂存区，分批 embedding，与旧索引合并后原子写入，再清理已合并的记录。
///
/// 先写索引、后改暂存区：中途崩溃时暂存区仍完整，下次可重做。
/// `removed` 为 plan 中要删除的节点 key；`sources` 为阶段 A 节点的文档来源。
pub fn embed_and_merge(
    cfg: &Config,
    embedder: &dyn Embedder,
    index_dir: &Path,
    run_id: &str,
    removed: &[String],
    sources: &HashMap<String, NodeSource>,
    stamp: &Stamp,
) -> Result<MergeSummary, GenerateError> {
    let staging_dir = &cfg.paths.staging_dir;
    let records = staging::read_all(staging_dir, run_id)?;
    let old = index::load(index_dir)?;
    let removed: HashSet<String> = removed.iter().cloned().collect();
    let merged = merge_staged(
        old.as_ref(),
        &removed,
        &records,
        sources,
        embedder,
        cfg.agent.batch_size,
        stamp,
    )?;
    index::save_atomic(index_dir, &merged.index, &merged.vectors)?;
    staging::replace(staging_dir, run_id, &merged.keep)?;
    Ok(MergeSummary {
        merged: merged.merged,
        failed: merged.failed,
    })
}

/// 按路径返回文档来源与原文的回调，`None` 表示无文档。
pub type DocFetch<'a> = dyn Fn(&[CommandName]) -> Option<(DocSource, String)> + 'a;

/// `index.lock` 的文件名。
const INDEX_LOCK: &str = "index.lock";

/// `reindex` 的选项。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReindexOptions {
    /// 忽略旧 `source_hash`，非预置节点全部重跑。
    pub full: bool,
    /// 只重跑列出的节点（可含预置节点），并展开它们发现的子命令，范围由 `max_depth` 限制。
    pub names: Option<Vec<String>>,
}

/// 一次 reindex 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReindexReport {
    /// 本次写入索引的节点数。
    pub generated: usize,
    /// 复用旧条目的节点数。
    pub reused: usize,
    /// 因根命令已不在 PATH 而删除的节点数（含子树）。
    pub removed: usize,
    /// 失败的 `(key, reason)`，包括 agent 失败与 embedding 失败。
    pub failed: Vec<(String, String)>,
}

/// `reindex` 的输入。
pub struct Inputs<'a> {
    /// 配置，决定索引、暂存区路径与批次大小。
    pub cfg: &'a Config,
    /// agent 后端。
    pub backend: &'a dyn AgentBackend,
    /// embedding 客户端。
    pub embedder: &'a dyn Embedder,
    /// 本次扫描得到的根命令。
    pub roots: &'a [CommandEntry],
    /// 返回某路径的文档来源与原文，`None` 表示无文档。
    pub fetch: &'a DocFetch<'a>,
    /// 选项。
    pub opts: &'a ReindexOptions,
    /// 本次 run 的标识，决定暂存区文件名。
    pub run_id: &'a str,
    /// 写入索引时使用的构建信息。
    pub stamp: &'a Stamp,
    /// 日志文件。写入暂存记录、失败原因与合并结果；`None` 时不写。
    pub log: Option<&'a Path>,
}

/// 完整流程：锁 → 计划 → 阶段 A → 阶段 B（合并、原子写索引、清理暂存区）→ 释放锁。
///
/// 锁为 `index_dir/index.lock`，用标准库 `File::try_lock`，进程退出时自动释放。
/// 锁被占用时返回 [`GenerateError::Locked`]，不做任何改动。
pub fn reindex(input: &Inputs<'_>) -> Result<ReindexReport, GenerateError> {
    let index_dir = &input.cfg.paths.index_dir;
    let staging_dir = &input.cfg.paths.staging_dir;
    let _lock = acquire_lock(index_dir)?;
    let old = index::load(index_dir)?;
    let model = input.embedder.model().to_owned();

    // 比较时顺便记下每个节点的来源，供阶段 B 使用。
    let fetched: RefCell<HashMap<String, NodeSource>> = RefCell::new(HashMap::new());
    let hash_of = |path: &[CommandName]| {
        let source = fetch_source(input, path, &model)?;
        let hash = source.source_hash.clone();
        fetched.borrow_mut().insert(key_of(path), source);
        Some(hash)
    };
    let plan = make_plan(
        input.roots,
        old.as_ref().map(|stored| &stored.index),
        input.opts.names.as_deref(),
        &hash_of,
        input.opts.full,
    );
    let mut sources = fetched.into_inner();

    // names 模式不经过比较，这里补齐待生成节点的来源。
    for path in &plan.regenerate {
        let key = key_of(path);
        if let Entry::Vacant(entry) = sources.entry(key)
            && let Some(source) = fetch_source(input, path, &model)
        {
            entry.insert(source);
        }
    }

    // 每一轮处理上一轮发现的节点；第 0 轮是计划中的根命令。
    // names 模式同样展开子命令，范围仍由 max_depth 限制。
    let mut failed = Vec::new();
    let mut next = plan.regenerate.clone();
    let mut seen: HashSet<String> = next.iter().map(|path| key_of(path)).collect();
    loop {
        let round = Plan {
            regenerate: std::mem::take(&mut next),
            ..Plan::default()
        };
        if round.regenerate.is_empty() {
            break;
        }
        failed.extend(generate_descriptions(
            input.cfg,
            input.backend,
            &round,
            input.run_id,
            input.fetch,
            input.log,
        ));
        let records = staging::read_all(staging_dir, input.run_id)?;
        // agent 报告成功但没有暂存描述的节点视为失败，否则会静默丢失。
        let staged = staged_keys(&records);
        for path in &round.regenerate {
            let key = key_of(path);
            if !staged.contains(&key) && !failed.iter().any(|(k, _)| *k == key) {
                failed.push((
                    key,
                    "agent reported success but staged no description".to_owned(),
                ));
            }
        }
        next = discover_children(input, &records, &mut seen, &mut sources, &model);
        append_log(
            input.log,
            input.run_id,
            &format!("discovered {} subcommands", next.len()),
        );
    }

    for rec in staging::read_all(staging_dir, input.run_id)? {
        append_log(input.log, input.run_id, &format!("staged {rec:?}"));
    }
    for (key, reason) in &failed {
        append_log(
            input.log,
            input.run_id,
            &format!("agent-failed {key}: {reason}"),
        );
    }
    let summary = embed_and_merge(
        input.cfg,
        input.embedder,
        index_dir,
        input.run_id,
        &plan.remove,
        &sources,
        input.stamp,
    )?;
    for (key, reason) in &summary.failed {
        append_log(
            input.log,
            input.run_id,
            &format!("embed-failed {key}: {reason}"),
        );
    }
    append_log(
        input.log,
        input.run_id,
        &format!("merged {}", summary.merged),
    );
    failed.extend(summary.failed);
    Ok(ReindexReport {
        generated: summary.merged,
        reused: plan.keep.len(),
        removed: plan.remove.len(),
        failed,
    })
}

/// 从本轮的暂存记录中取出子命令，返回下一轮要处理的路径。
///
/// 只接受不超过 `max_depth` 的路径，已见过的 key 跳过，且必须能取到文档。
/// 子节点不做 `source_hash` 比较，本轮发现即重新生成。
fn discover_children(
    input: &Inputs<'_>,
    records: &[staging::StagedRecord],
    seen: &mut HashSet<String>,
    sources: &mut HashMap<String, NodeSource>,
    model: &str,
) -> Vec<Vec<CommandName>> {
    let max_depth = input.cfg.reindex.max_depth;
    let mut next = Vec::new();
    for rec in records {
        let staging::StagedRecord::Command {
            path, subcommands, ..
        } = rec
        else {
            continue;
        };
        for sub in subcommands {
            let mut child: Vec<String> = path.clone();
            child.push(sub.clone());
            let key = child.join(" ");
            if child.len() > max_depth {
                append_log(input.log, input.run_id, &format!("skipped {key}: depth"));
                continue;
            }
            // 同一子命令可能被多个父节点列出，重复项静默跳过
            if seen.contains(&key) {
                continue;
            }
            let Ok(child_path) = child
                .iter()
                .map(|token| validate_command(token))
                .collect::<Result<Vec<CommandName>, _>>()
            else {
                continue;
            };
            seen.insert(key.clone());
            let Some(source) = fetch_source(input, &child_path, model) else {
                append_log(input.log, input.run_id, &format!("skipped {key}: no doc"));
                continue;
            };
            append_log(input.log, input.run_id, &format!("queued {key}"));
            sources.insert(key, source);
            next.push(child_path);
        }
    }
    next
}

/// 追加一行日志到 `log`（若给出），行首为 `run_id`。写入失败只记 `warn`，不影响结果。
pub fn append_log(log: Option<&Path>, run_id: &str, msg: &str) {
    let Some(path) = log else {
        return;
    };
    let result = (|| -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut file = File::options().create(true).append(true).open(path)?;
        file.write_all(format!("{run_id} {msg}\n").as_bytes())
    })();
    if let Err(err) = result {
        warn!(path = %path.display(), error = %err, "reindex log write failed");
    }
}

/// 获取 `index_dir/index.lock` 的独占锁。锁已被占用时返回 [`GenerateError::Locked`]。
fn acquire_lock(index_dir: &Path) -> Result<File, GenerateError> {
    fs::create_dir_all(index_dir)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(index_dir.join(INDEX_LOCK))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(GenerateError::Locked),
        Err(TryLockError::Error(err)) => Err(err.into()),
    }
}

/// 调用 `input.fetch` 并算出 `source_hash`。无文档时返回 `None`。
fn fetch_source(input: &Inputs<'_>, path: &[CommandName], model: &str) -> Option<NodeSource> {
    let (doc_source, text) = (input.fetch)(path)?;
    Some(NodeSource {
        doc_source,
        source_hash: source_hash(&text, PROMPT_VERSION, model),
    })
}

/// 完整路径的 key，空格分隔。
fn key_of(path: &[CommandName]) -> String {
    path.iter()
        .map(CommandName::as_str)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 暂存记录中已有描述的命令 key。
fn staged_keys(records: &[staging::StagedRecord]) -> HashSet<String> {
    records
        .iter()
        .filter_map(|rec| match rec {
            staging::StagedRecord::Command { path, .. } => Some(path.join(" ")),
            _ => None,
        })
        .collect()
}

/// 日志用的回复预览：取前 2000 个字符，换行替换为空格。
fn preview(text: &str) -> String {
    text.chars()
        .take(2000)
        .collect::<String>()
        .replace('\n', " ")
}

/// reindex 端到端测试。假后端把描述写入暂存区，embedding 为固定向量，全部在临时目录中运行。
#[cfg(test)]
mod reindex_tests {
    use std::fs::File;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{
        DocFetch, GenerateError, Inputs, PROMPT_VERSION, ReindexOptions, ReindexReport, reindex,
    };
    use crate::agent::{
        AgentBackend, AgentError, BatchOutcome, BatchRequest, CapabilitySet, REQUIRED,
    };
    use crate::config::Config;
    use crate::embed::fake::FakeEmbedder;
    use crate::generate::merge::Stamp;
    use crate::generate::staging::{self, StagedRecord};
    use crate::guard::name::validate_command;
    use crate::index::{self, GeneratorMeta};
    use crate::testutil::tempdir;
    use crate::types::{CommandEntry, CommandName, DocSource};

    /// 假后端：每个路径向暂存区写一条命令记录，并记录调用次数。
    /// 根命令的记录额外带上 `children` 作为子命令。
    struct StagingBackend {
        capabilities: CapabilitySet,
        staging_dir: PathBuf,
        description: &'static str,
        children: &'static [&'static str],
        calls: AtomicUsize,
    }

    impl AgentBackend for StagingBackend {
        fn id(&self) -> &'static str {
            "fake"
        }

        fn capabilities(&self) -> &CapabilitySet {
            &self.capabilities
        }

        fn run_batch(&self, req: &BatchRequest<'_>) -> Result<BatchOutcome, AgentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            for path in req.paths {
                let subcommands = if path.len() == 1 {
                    self.children.iter().map(|s| (*s).to_owned()).collect()
                } else {
                    Vec::new()
                };
                let rec = StagedRecord::Command {
                    path: path.iter().map(|c| c.as_str().to_owned()).collect(),
                    description: self.description.to_owned(),
                    examples: Vec::new(),
                    subcommands,
                };
                staging::append(&self.staging_dir, req.run_id, &rec)
                    .map_err(|err| AgentError::BadOutput(err.to_string()))?;
            }
            Ok(BatchOutcome {
                ok: true,
                raw_output: String::new(),
            })
        }
    }

    fn fetch_v1(_: &[CommandName]) -> Option<(DocSource, String)> {
        Some((DocSource::Man, "GIT v1".to_owned()))
    }

    fn fetch_v2(_: &[CommandName]) -> Option<(DocSource, String)> {
        Some((DocSource::Man, "GIT v2".to_owned()))
    }

    fn git_root() -> Vec<CommandEntry> {
        vec![CommandEntry {
            name: validate_command("git").unwrap(),
            path: PathBuf::from("/usr/bin/git"),
        }]
    }

    fn config(dir: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.paths.index_dir = dir.join("index");
        cfg.paths.staging_dir = dir.join("staging");
        cfg
    }

    fn backend(cfg: &Config) -> StagingBackend {
        StagingBackend {
            capabilities: REQUIRED.into_iter().collect(),
            staging_dir: cfg.paths.staging_dir.clone(),
            description: "Git tool",
            children: &[],
            calls: AtomicUsize::new(0),
        }
    }

    fn stamp() -> Stamp {
        Stamp {
            built_at: "2026-11-04T00:00:00Z".to_owned(),
            generator: GeneratorMeta {
                backend: "fake".to_owned(),
                agent_version: "test".to_owned(),
                prompt_version: PROMPT_VERSION,
            },
        }
    }

    /// 跑一次 reindex。embedding 对 "git Git tool" 返回固定向量，其余文本不被请求。
    fn run(
        cfg: &Config,
        backend: &StagingBackend,
        roots: &[CommandEntry],
        fetch: &DocFetch<'_>,
        run_id: &str,
    ) -> Result<ReindexReport, GenerateError> {
        let embedder = FakeEmbedder::new(3).with("git Git tool", vec![1.0, 0.0, 0.0]);
        let stamp = stamp();
        reindex(&Inputs {
            cfg,
            backend,
            embedder: &embedder,
            roots,
            fetch,
            opts: &ReindexOptions::default(),
            run_id,
            stamp: &stamp,
            log: None,
        })
    }

    #[test]
    fn first_run_generates_and_writes_index() {
        let dir = tempdir();
        let cfg = config(dir.path());
        let backend = backend(&cfg);
        let report = run(&cfg, &backend, &git_root(), &fetch_v1, "run-1").unwrap();
        assert_eq!(report.generated, 1);
        assert_eq!(report.reused, 0);
        assert!(report.failed.is_empty());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        let stored = index::load(&cfg.paths.index_dir)
            .unwrap()
            .expect("index written");
        assert_eq!(stored.index.nodes[0].key, "git");
        assert_eq!(stored.index.nodes[0].description, "Git tool");
        assert!(
            staging::read_all(&cfg.paths.staging_dir, "run-1")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unchanged_root_is_reused_without_agent_call() {
        let dir = tempdir();
        let cfg = config(dir.path());
        let backend = backend(&cfg);
        run(&cfg, &backend, &git_root(), &fetch_v1, "run-1").unwrap();
        let report = run(&cfg, &backend, &git_root(), &fetch_v1, "run-2").unwrap();
        assert_eq!(report.reused, 1);
        assert_eq!(report.generated, 0);
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "no new agent batch"
        );
        let stored = index::load(&cfg.paths.index_dir).unwrap().unwrap();
        assert_eq!(stored.index.nodes.len(), 1);
        assert_eq!(stored.vectors.rows(), 1);
    }

    #[test]
    fn changed_docs_regenerate_the_root() {
        let dir = tempdir();
        let cfg = config(dir.path());
        let backend = backend(&cfg);
        run(&cfg, &backend, &git_root(), &fetch_v1, "run-1").unwrap();
        let report = run(&cfg, &backend, &git_root(), &fetch_v2, "run-2").unwrap();
        assert_eq!(report.generated, 1);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn names_mode_expands_subcommands() {
        let dir = tempdir();
        let cfg = config(dir.path());
        let backend = StagingBackend {
            children: &["clone"],
            ..backend(&cfg)
        };
        let embedder = FakeEmbedder::new(3)
            .with("git Git tool", vec![1.0, 0.0, 0.0])
            .with("git clone Git tool", vec![0.0, 1.0, 0.0]);
        let stamp = stamp();
        let opts = ReindexOptions {
            full: false,
            names: Some(vec!["git".to_owned()]),
        };
        let report = reindex(&Inputs {
            cfg: &cfg,
            backend: &backend,
            embedder: &embedder,
            roots: &git_root(),
            fetch: &fetch_v1,
            opts: &opts,
            run_id: "run-n",
            stamp: &stamp,
            log: None,
        })
        .unwrap();
        assert_eq!(report.generated, 2);
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            2,
            "root and one subcommand"
        );
        let stored = index::load(&cfg.paths.index_dir).unwrap().unwrap();
        let keys: Vec<&str> = stored.index.nodes.iter().map(|n| n.key.as_str()).collect();
        assert!(keys.contains(&"git"));
        assert!(keys.contains(&"git clone"));
    }

    #[test]
    fn ok_batch_without_staged_description_is_a_failure() {
        /// 假后端：报告成功，但不写任何暂存记录。
        struct SilentBackend {
            capabilities: CapabilitySet,
        }

        impl AgentBackend for SilentBackend {
            fn id(&self) -> &'static str {
                "silent"
            }

            fn capabilities(&self) -> &CapabilitySet {
                &self.capabilities
            }

            fn run_batch(&self, _req: &BatchRequest<'_>) -> Result<BatchOutcome, AgentError> {
                Ok(BatchOutcome {
                    ok: true,
                    raw_output: String::new(),
                })
            }
        }

        let dir = tempdir();
        let cfg = config(dir.path());
        let backend = SilentBackend {
            capabilities: REQUIRED.into_iter().collect(),
        };
        let embedder = FakeEmbedder::new(3);
        let stamp = stamp();
        let report = reindex(&Inputs {
            cfg: &cfg,
            backend: &backend,
            embedder: &embedder,
            roots: &git_root(),
            fetch: &fetch_v1,
            opts: &ReindexOptions::default(),
            run_id: "run-s",
            stamp: &stamp,
            log: None,
        })
        .unwrap();
        assert_eq!(report.generated, 0);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].0, "git");
    }

    #[test]
    fn vanished_root_is_removed_from_index() {
        let dir = tempdir();
        let cfg = config(dir.path());
        let backend = backend(&cfg);
        run(&cfg, &backend, &git_root(), &fetch_v1, "run-1").unwrap();
        let report = run(&cfg, &backend, &[], &fetch_v1, "run-2").unwrap();
        assert_eq!(report.removed, 1);
        let stored = index::load(&cfg.paths.index_dir).unwrap().unwrap();
        assert!(stored.index.nodes.is_empty());
        assert!(stored.vectors.data.is_empty());
    }

    #[test]
    fn held_lock_refuses_to_run_and_changes_nothing() {
        let dir = tempdir();
        let cfg = config(dir.path());
        std::fs::create_dir_all(&cfg.paths.index_dir).unwrap();
        let holder = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(cfg.paths.index_dir.join("index.lock"))
            .unwrap();
        holder.try_lock().unwrap();
        let backend = backend(&cfg);
        let err = run(&cfg, &backend, &git_root(), &fetch_v1, "run-3").unwrap_err();
        assert!(matches!(err, GenerateError::Locked));
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert!(index::load(&cfg.paths.index_dir).unwrap().is_none());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::{embed_and_merge, generate_descriptions};
    use crate::agent::{
        AgentBackend, AgentError, BatchOutcome, BatchRequest, CapabilitySet, REQUIRED,
    };
    use crate::config::Config;
    use crate::embed::fake::FakeEmbedder;
    use crate::generate::merge::{NodeSource, Stamp};
    use crate::generate::plan::Plan;
    use crate::generate::staging::{self, StagedRecord};
    use crate::guard::name::validate_command;
    use crate::index;
    use crate::testutil::tempdir;
    use crate::types::{CommandName, DocSource};

    /// 假后端：批次首个路径为 `fail_on` 时返回错误，其余成功，并记录每批的首个路径、路径数与提示词。
    /// 各批可能并发执行，失败按内容而非调用次序决定，记录顺序也不代表批次顺序。
    struct FakeBackend {
        capabilities: CapabilitySet,
        fail_on: Option<&'static str>,
        calls: AtomicUsize,
        seen: Mutex<Vec<(String, usize, String)>>,
        /// 同时在执行的批数与观察到的最大值。
        running: AtomicUsize,
        peak: AtomicUsize,
        /// 每批停留的时间，用于让并发批次重叠。
        delay: Duration,
    }

    impl FakeBackend {
        fn new(fail_on: Option<&'static str>) -> Self {
            Self {
                capabilities: REQUIRED.into_iter().collect(),
                fail_on,
                calls: AtomicUsize::new(0),
                seen: Mutex::new(Vec::new()),
                running: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                delay: Duration::ZERO,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        /// 按批次首个路径排序后的记录，即批次顺序。
        fn seen_in_order(&self, plan: &Plan) -> Vec<(String, usize, String)> {
            let order: Vec<String> = plan.regenerate.iter().map(|p| super::key_of(p)).collect();
            let mut seen = self.seen.lock().unwrap().clone();
            seen.sort_by_key(|(first, _, _)| order.iter().position(|k| k == first));
            seen
        }
    }

    impl AgentBackend for FakeBackend {
        fn id(&self) -> &'static str {
            "fake"
        }

        fn capabilities(&self) -> &CapabilitySet {
            &self.capabilities
        }

        fn run_batch(&self, req: &BatchRequest<'_>) -> Result<BatchOutcome, AgentError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            self.running.fetch_sub(1, Ordering::SeqCst);
            let first = super::key_of(&req.paths[0]);
            self.seen
                .lock()
                .unwrap()
                .push((first.clone(), req.paths.len(), req.prompt.to_owned()));
            if self.fail_on == Some(first.as_str()) {
                return Err(AgentError::ExitStatus(1));
            }
            Ok(BatchOutcome {
                ok: true,
                raw_output: String::new(),
            })
        }
    }

    /// 测试用文档：每个路径都返回固定文本。
    fn no_docs(_: &[CommandName]) -> Option<(DocSource, String)> {
        Some((DocSource::Man, "doc".to_owned()))
    }

    fn path(text: &str) -> Vec<CommandName> {
        text.split(' ')
            .map(|t| validate_command(t).unwrap())
            .collect()
    }

    fn plan_of(paths: &[&str]) -> Plan {
        Plan {
            regenerate: paths.iter().map(|p| path(p)).collect(),
            ..Plan::default()
        }
    }

    fn config_with_batch(size: usize) -> Config {
        let mut cfg = Config::default();
        cfg.agent.batch_size = size;
        cfg
    }

    #[test]
    fn batches_respect_batch_size_and_list_every_path() {
        let backend = FakeBackend::new(None);
        let plan = plan_of(&["git", "git clone", "git pull", "ls", "grep"]);
        let failed = generate_descriptions(
            &config_with_batch(2),
            &backend,
            &plan,
            "run-a",
            &no_docs,
            None,
        );
        assert!(failed.is_empty());
        let seen = backend.seen_in_order(&plan);
        let sizes: Vec<usize> = seen.iter().map(|(_, n, _)| *n).collect();
        assert_eq!(sizes, [2, 2, 1]);
        assert!(seen[0].2.contains("\n## git clone\n"));
        assert!(seen[0].2.contains("\n## git\n"));
    }

    #[test]
    fn batches_run_concurrently_up_to_the_limit() {
        let mut backend = FakeBackend::new(Some("grep"));
        backend.delay = Duration::from_millis(100);
        let plan = plan_of(&["a", "b", "c", "d", "e", "f", "grep"]);
        let mut cfg = config_with_batch(1);
        cfg.agent.concurrency = 3;
        let failed = generate_descriptions(&cfg, &backend, &plan, "run-p", &no_docs, None);
        assert_eq!(backend.calls(), 7);
        let peak = backend.peak.load(Ordering::SeqCst);
        assert!((2..=3).contains(&peak), "peak concurrency {peak}");
        let keys: Vec<&str> = failed.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["grep"]);
    }

    #[test]
    fn concurrency_one_runs_batches_serially() {
        let mut backend = FakeBackend::new(None);
        backend.delay = Duration::from_millis(20);
        let plan = plan_of(&["a", "b", "c"]);
        let mut cfg = config_with_batch(1);
        cfg.agent.concurrency = 1;
        generate_descriptions(&cfg, &backend, &plan, "run-s", &no_docs, None);
        assert_eq!(backend.peak.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failing_batch_only_fails_its_own_nodes() {
        let backend = FakeBackend::new(Some("git pull"));
        let plan = plan_of(&["git", "git clone", "git pull", "ls", "grep"]);
        let failed = generate_descriptions(
            &config_with_batch(2),
            &backend,
            &plan,
            "run-b",
            &no_docs,
            None,
        );
        let keys: Vec<&str> = failed.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["git pull", "ls"]);
        assert_eq!(backend.calls(), 3, "later batches must still run");
    }

    #[test]
    fn zero_batch_size_is_treated_as_one() {
        let backend = FakeBackend::new(None);
        let plan = plan_of(&["git", "ls"]);
        generate_descriptions(
            &config_with_batch(0),
            &backend,
            &plan,
            "run-c",
            &no_docs,
            None,
        );
        assert_eq!(backend.calls(), 2);
    }

    /// 假后端：返回合法的 JSON 输出，走与真实 `claude` 相同的解析与校验路径。
    struct JsonBackend {
        capabilities: CapabilitySet,
    }

    impl AgentBackend for JsonBackend {
        fn id(&self) -> &'static str {
            "json"
        }

        fn capabilities(&self) -> &CapabilitySet {
            &self.capabilities
        }

        fn run_batch(&self, req: &BatchRequest<'_>) -> Result<BatchOutcome, AgentError> {
            Ok(BatchOutcome {
                ok: true,
                raw_output: super::output::test_batch_json(req.paths, "Git tool", &["clone"]),
            })
        }
    }

    #[test]
    fn json_output_is_parsed_checked_and_staged() {
        let dir = tempdir();
        let mut cfg = Config::default();
        cfg.paths.staging_dir = dir.path().join("staging");
        let backend = JsonBackend {
            capabilities: REQUIRED.into_iter().collect(),
        };
        // 文档中出现 clone，子命令校验应通过。
        let fetch = |_: &[CommandName]| {
            Some((
                DocSource::Man,
                "COMMANDS\n   clone   Clone a repo.".to_owned(),
            ))
        };
        let failed =
            generate_descriptions(&cfg, &backend, &plan_of(&["git"]), "run-j", &fetch, None);
        assert!(failed.is_empty());
        let records = staging::read_all(&cfg.paths.staging_dir, "run-j").unwrap();
        assert_eq!(records.len(), 1);
        match &records[0] {
            StagedRecord::Command {
                path,
                description,
                subcommands,
                ..
            } => {
                assert_eq!(path, &["git".to_owned()]);
                assert_eq!(description, "Git tool");
                assert_eq!(subcommands, &["clone".to_owned()]);
            }
            other => panic!("expected a command record, got {other:?}"),
        }
    }

    #[test]
    fn embed_and_merge_writes_index_and_clears_staging() {
        let dir = tempdir();
        let mut cfg = Config::default();
        cfg.paths.staging_dir = dir.path().join("staging");
        let index_dir = dir.path().join("index");
        staging::append(
            &cfg.paths.staging_dir,
            "run-e",
            &StagedRecord::Command {
                path: vec!["git".to_owned()],
                description: "Git".to_owned(),
                examples: Vec::new(),
                subcommands: Vec::new(),
            },
        )
        .unwrap();
        let sources = std::collections::HashMap::from([(
            "git".to_owned(),
            NodeSource {
                doc_source: DocSource::Man,
                source_hash: "sha256:h".to_owned(),
            },
        )]);
        let stamp = Stamp {
            built_at: "2026-11-04T00:00:00Z".to_owned(),
            generator: crate::index::GeneratorMeta {
                backend: "fake".to_owned(),
                agent_version: "test".to_owned(),
                prompt_version: 1,
            },
        };
        let embedder = FakeEmbedder::new(3).with("git Git", vec![1.0, 0.0, 0.0]);

        let summary =
            embed_and_merge(&cfg, &embedder, &index_dir, "run-e", &[], &sources, &stamp).unwrap();

        assert_eq!(summary.merged, 1);
        assert!(summary.failed.is_empty());
        let stored = index::load(&index_dir).unwrap().expect("index written");
        assert_eq!(stored.index.nodes[0].key, "git");
        assert_eq!(stored.vectors.rows(), 1);
        assert!(
            staging::read_all(&cfg.paths.staging_dir, "run-e")
                .unwrap()
                .is_empty()
        );
    }
}
