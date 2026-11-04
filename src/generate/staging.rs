//! 暂存区：工具写入的已校验记录，每个 run 一个 JSONL 文件。
//!
//! 工具只追加记录。reindex 的阶段 B 用 [`read_all`] 读取，合并后用 [`remove`] 删除（D10）。
//! `run_id` 只允许字母、数字、`-`、`_`，防止路径穿越。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `run_id` 长度上限。
const MAX_RUN_ID_LEN: usize = 64;

/// 暂存区错误。
#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    /// `run_id` 含非法字符或长度超限。
    #[error("invalid run id {0:?}")]
    InvalidRunId(String),
    /// 文件读写失败。
    #[error("staging io: {0}")]
    Io(#[from] io::Error),
    /// 记录序列化失败。
    #[error("staging json: {0}")]
    Json(#[from] serde_json::Error),
}

/// 一条暂存记录。字段已由工具层校验。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum StagedRecord {
    /// 命令节点的描述与示例，以及下一轮要处理的子命令。
    Command {
        /// 命令路径，如 `["git", "clone"]`。
        path: Vec<String>,
        /// 清洗后的描述。
        description: String,
        /// 清洗后的示例，最多 3 条。
        examples: Vec<String>,
        /// 子命令名，每项已通过命名校验。
        subcommands: Vec<String>,
    },
    /// 命令下的长参数描述。
    Params {
        /// 所属命令路径。
        path: Vec<String>,
        /// 参数列表，只含长参数。
        params: Vec<StagedParam>,
    },
}

/// 一个长参数及其描述。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StagedParam {
    /// 长参数名，如 `--branch`。
    pub flag: String,
    /// 清洗后的描述。
    pub description: String,
}

/// 把一条记录追加到 `staging_dir/<run_id>.jsonl`，目录不存在时创建。
///
/// `run_id` 不合法时返回 [`StagingError::InvalidRunId`]，不触碰文件系统。
pub fn append(staging_dir: &Path, run_id: &str, rec: &StagedRecord) -> Result<(), StagingError> {
    if !valid_run_id(run_id) {
        return Err(StagingError::InvalidRunId(run_id.to_owned()));
    }
    fs::create_dir_all(staging_dir)?;
    let mut line = serde_json::to_string(rec)?;
    line.push('\n');
    let path = staging_dir.join(format!("{run_id}.jsonl"));
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

/// 读取 `staging_dir/<run_id>.jsonl` 的全部记录，按写入顺序返回。文件不存在时返回空列表。
///
/// 任一行解析失败即返回 [`StagingError::Json`]，不跳过，避免静默丢失描述。
pub fn read_all(staging_dir: &Path, run_id: &str) -> Result<Vec<StagedRecord>, StagingError> {
    let path = record_path(staging_dir, run_id)?;
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).map_err(StagingError::from))
        .collect()
}

/// 删除 `staging_dir/<run_id>.jsonl`。文件不存在时视为成功。
pub fn remove(staging_dir: &Path, run_id: &str) -> Result<(), StagingError> {
    let path = record_path(staging_dir, run_id)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// 用 `records` 整体替换 `staging_dir/<run_id>.jsonl`，先写临时文件再 rename。
///
/// 阶段 B 只保留未能合并的记录。`records` 为空时删除文件。
pub fn replace(
    staging_dir: &Path,
    run_id: &str,
    records: &[StagedRecord],
) -> Result<(), StagingError> {
    if records.is_empty() {
        return remove(staging_dir, run_id);
    }
    let path = record_path(staging_dir, run_id)?;
    let tmp = staging_dir.join(format!("{run_id}.jsonl.tmp"));
    let mut body = String::new();
    for rec in records {
        body.push_str(&serde_json::to_string(rec)?);
        body.push('\n');
    }
    let mut file = fs::File::create(&tmp)?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// 校验 `run_id` 并返回记录文件路径。不触碰文件系统。
fn record_path(staging_dir: &Path, run_id: &str) -> Result<PathBuf, StagingError> {
    if !valid_run_id(run_id) {
        return Err(StagingError::InvalidRunId(run_id.to_owned()));
    }
    Ok(staging_dir.join(format!("{run_id}.jsonl")))
}

/// 判断 `run_id` 是否只含允许的字符，且长度在范围内。
fn valid_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_RUN_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::tempdir;

    #[test]
    fn append_writes_one_json_line_per_record() {
        let dir = tempdir();
        let rec = StagedRecord::Params {
            path: vec!["git".to_owned()],
            params: vec![StagedParam {
                flag: "--branch".to_owned(),
                description: "Branch".to_owned(),
            }],
        };
        append(dir.path(), "run-1", &rec).unwrap();
        append(dir.path(), "run-1", &rec).unwrap();
        let text = fs::read_to_string(dir.path().join("run-1.jsonl")).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let back: StagedRecord = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn read_all_returns_records_in_write_order() {
        let dir = tempdir();
        let first = StagedRecord::Params {
            path: vec!["git".to_owned()],
            params: Vec::new(),
        };
        let second = StagedRecord::Command {
            path: vec!["git".to_owned(), "clone".to_owned()],
            description: "Clone".to_owned(),
            examples: Vec::new(),
            subcommands: Vec::new(),
        };
        append(dir.path(), "run-2", &first).unwrap();
        append(dir.path(), "run-2", &second).unwrap();
        assert_eq!(read_all(dir.path(), "run-2").unwrap(), vec![first, second]);
    }

    #[test]
    fn read_all_of_missing_file_is_empty() {
        let dir = tempdir();
        assert!(read_all(dir.path(), "none").unwrap().is_empty());
    }

    #[test]
    fn remove_deletes_the_file_and_is_idempotent() {
        let dir = tempdir();
        let rec = StagedRecord::Params {
            path: vec!["git".to_owned()],
            params: Vec::new(),
        };
        append(dir.path(), "run-3", &rec).unwrap();
        remove(dir.path(), "run-3").unwrap();
        assert!(!dir.path().join("run-3.jsonl").exists());
        remove(dir.path(), "run-3").unwrap();
    }

    #[test]
    fn read_and_remove_reject_invalid_run_id() {
        let dir = tempdir();
        assert!(matches!(
            read_all(dir.path(), "../x"),
            Err(StagingError::InvalidRunId(_))
        ));
        assert!(matches!(
            remove(dir.path(), "a/b"),
            Err(StagingError::InvalidRunId(_))
        ));
    }

    #[test]
    fn replace_keeps_only_given_records_and_empty_removes_file() {
        let dir = tempdir();
        let kept = StagedRecord::Params {
            path: vec!["git".to_owned()],
            params: Vec::new(),
        };
        let dropped = StagedRecord::Command {
            path: vec!["git".to_owned()],
            description: "x".to_owned(),
            examples: Vec::new(),
            subcommands: Vec::new(),
        };
        append(dir.path(), "run-4", &dropped).unwrap();
        append(dir.path(), "run-4", &kept).unwrap();
        replace(dir.path(), "run-4", std::slice::from_ref(&kept)).unwrap();
        assert_eq!(read_all(dir.path(), "run-4").unwrap(), vec![kept]);
        assert!(!dir.path().join("run-4.jsonl.tmp").exists());
        replace(dir.path(), "run-4", &[]).unwrap();
        assert!(!dir.path().join("run-4.jsonl").exists());
    }

    #[test]
    fn invalid_run_id_is_rejected_without_writing() {
        let dir = tempdir();
        let target = dir.path().join("staging");
        let rec = StagedRecord::Params {
            path: vec!["git".to_owned()],
            params: Vec::new(),
        };
        for id in ["", "../x", "a/b", "a.b", "a b"] {
            assert!(
                matches!(
                    append(&target, id, &rec),
                    Err(StagingError::InvalidRunId(_))
                ),
                "should reject {id:?}"
            );
        }
        assert!(!target.exists(), "no directory may be created on rejection");
    }
}
