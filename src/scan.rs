//! PATH 扫描：找出根命令候选。
//!
//! 只读取目录项与文件元数据，不读文件内容，不调用 agent，不写索引。
//! 规则：
//! - 按 PATH 顺序遍历，目录不可读时跳过。
//! - 文件名不通过 `guard::name::validate_command` 的跳过。
//! - 只接受跟随符号链接后为普通文件且有任一执行位的条目；断链跳过。
//! - 同名取 PATH 中第一个通过上述检查的条目。非可执行文件不遮蔽后面的可执行文件。
//! - 相对路径的 PATH 项（包括空项）跳过并记 `warn`。

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use crate::guard::name::validate_command;
use crate::types::{CommandEntry, CommandName};

/// 纯逻辑：输入 PATH 字符串，返回去重后的根命令列表，顺序与 PATH 中首次出现一致。
///
/// 文件系统只读取目录项与各条目的元数据，见模块文档中的规则。
pub fn commands_from_path(path_var: &str) -> Vec<CommandEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for dir in search_dirs(path_var) {
        let Ok(entries) = fs::read_dir(dir) else {
            debug!(dir = %dir.display(), "PATH dir unreadable, skipped");
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(raw) = file_name.to_str() else {
                continue;
            };
            let Ok(name) = validate_command(raw) else {
                continue;
            };
            if seen.contains(raw) {
                continue;
            }
            let path = entry.path();
            if !is_executable(&path) {
                continue;
            }
            seen.insert(raw.to_owned());
            out.push(CommandEntry { name, path });
        }
    }
    info!(count = out.len(), "PATH scanned");
    out
}

/// 读取环境变量 `PATH` 后调用 [`commands_from_path`]。`PATH` 未设置时返回空列表。
pub fn current_path_commands() -> Vec<CommandEntry> {
    commands_from_path(&std::env::var("PATH").unwrap_or_default())
}

/// 在 PATH 中查找单个根命令，返回第一个可执行文件的完整路径。
///
/// 判断规则与 [`commands_from_path`] 相同，二者结果一致。
pub fn find_in_path(path_var: &str, name: &CommandName) -> Option<PathBuf> {
    search_dirs(path_var)
        .into_iter()
        .map(|dir| dir.join(name.as_str()))
        .find(|candidate| is_executable(candidate))
}

/// 把 PATH 拆成绝对路径的目录列表。相对路径与空项跳过并记 `warn`。
fn search_dirs(path_var: &str) -> Vec<&Path> {
    if path_var.is_empty() {
        return Vec::new();
    }
    path_var
        .split(':')
        .filter_map(|entry| {
            let path = Path::new(entry);
            if path.is_absolute() {
                Some(path)
            } else {
                warn!(entry, "relative PATH entry skipped");
                None
            }
        })
        .collect()
}

/// 跟随符号链接后判断：普通文件且有任一执行位。断链与目录返回 `false`。
fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};

    use super::{commands_from_path, find_in_path};
    use crate::guard::name::validate_command;
    use crate::testutil::tempdir;

    const EXEC: u32 = 0o755;
    const PLAIN: u32 = 0o644;

    /// 在 `dir` 下创建指定权限的文件，返回其路径。
    fn write_file(dir: &Path, name: &str, mode: u32) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    /// 用 `:` 拼接目录列表，模拟 PATH 字符串。
    fn path_var<P: AsRef<Path>>(dirs: &[P]) -> String {
        dirs.iter()
            .map(|d| d.as_ref().display().to_string())
            .collect::<Vec<_>>()
            .join(":")
    }

    #[test]
    fn duplicate_name_keeps_first_in_path() {
        let root = tempdir();
        let a = root.path().join("a");
        let b = root.path().join("b");
        let first = write_file(&a, "foo", EXEC);
        write_file(&b, "foo", EXEC);

        let found = commands_from_path(&path_var(&[&a, &b]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name.as_str(), "foo");
        assert_eq!(found[0].path, first);
    }

    #[test]
    fn non_executable_does_not_shadow_later_executable() {
        let root = tempdir();
        let a = root.path().join("a");
        let b = root.path().join("b");
        write_file(&a, "foo", PLAIN);
        let second = write_file(&b, "foo", EXEC);

        let found = commands_from_path(&path_var(&[&a, &b]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, second);
    }

    #[test]
    fn directory_named_like_command_is_skipped() {
        let root = tempdir();
        let a = root.path().join("a");
        fs::create_dir_all(a.join("bar")).unwrap();

        assert!(commands_from_path(&path_var(&[&a])).is_empty());
    }

    #[test]
    fn symlink_to_executable_is_followed() {
        let root = tempdir();
        let a = root.path().join("a");
        let b = root.path().join("b");
        let target = write_file(&b, "real", EXEC);
        fs::create_dir_all(&a).unwrap();
        symlink(&target, a.join("lnk")).unwrap();

        let found = commands_from_path(&path_var(&[&a]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name.as_str(), "lnk");
        assert_eq!(found[0].path, a.join("lnk"));
    }

    #[test]
    fn broken_symlink_is_skipped() {
        let root = tempdir();
        let a = root.path().join("a");
        fs::create_dir_all(&a).unwrap();
        symlink(a.join("missing-target"), a.join("dangling")).unwrap();

        assert!(commands_from_path(&path_var(&[&a])).is_empty());
    }

    #[test]
    fn invalid_names_are_skipped() {
        let root = tempdir();
        let a = root.path().join("a");
        for bad in ["-x", ".hidden", "a b", "bad;name"] {
            write_file(&a, bad, EXEC);
        }

        assert!(commands_from_path(&path_var(&[&a])).is_empty());
    }

    #[test]
    fn relative_and_empty_entries_are_skipped() {
        let root = tempdir();
        let a = root.path().join("a");
        write_file(&a, "foo", EXEC);
        // 相对项与空项不参与扫描，只保留绝对路径中的 foo。
        let pv = format!("relative/dir::{}", a.display());

        let found = commands_from_path(&pv);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name.as_str(), "foo");
    }

    #[test]
    fn missing_directory_is_skipped() {
        let root = tempdir();
        let a = root.path().join("a");
        write_file(&a, "foo", EXEC);
        let pv = format!("{}:{}", root.path().join("nope").display(), a.display());

        let found = commands_from_path(&pv);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn find_in_path_agrees_with_commands_from_path() {
        let root = tempdir();
        let a = root.path().join("a");
        let b = root.path().join("b");
        write_file(&a, "foo", EXEC);
        write_file(&b, "foo", EXEC);
        write_file(&b, "bar", EXEC);
        let pv = path_var(&[&a, &b]);

        let found = commands_from_path(&pv);
        assert_eq!(found.len(), 2);
        for entry in &found {
            assert_eq!(find_in_path(&pv, &entry.name), Some(entry.path.clone()));
        }
        let missing = validate_command("nope").unwrap();
        assert_eq!(find_in_path(&pv, &missing), None);
    }
}
