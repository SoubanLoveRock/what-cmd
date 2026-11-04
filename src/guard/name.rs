//! 命令名、子命令路径与参数名校验。
//!
//! 规则见 DESIGN.md 第 11 节：不接受路径、空格、参数，长度上限 64。

use crate::guard::GuardError;
use crate::types::CommandName;

/// 命令名与参数名的总长度上限（含前缀 `--` 的参数名同样适用）。
pub const MAX_NAME_LEN: usize = 64;

/// 校验单个命令名，规则 `^[A-Za-z0-9][A-Za-z0-9._+-]{0,63}$`。
///
/// 对应 `CommandName` 的唯一构造入口。
pub fn validate_command(raw: &str) -> Result<CommandName, GuardError> {
    if is_command_name(raw) {
        Ok(CommandName::from_validated(raw.to_owned()))
    } else {
        Err(GuardError::InvalidName(raw.to_owned()))
    }
}

/// 校验子命令路径：每个 token 独立调用 `validate_command`。
///
/// 不做空格切分，含空格的 token 直接被拒绝。空切片返回 `EmptyPath`。
pub fn validate_path(raw: &[&str]) -> Result<Vec<CommandName>, GuardError> {
    if raw.is_empty() {
        return Err(GuardError::EmptyPath);
    }
    raw.iter().map(|token| validate_command(token)).collect()
}

/// 校验长参数名，规则 `^--[A-Za-z0-9][A-Za-z0-9-]*$`，总长度不超过 64。
///
/// 短参数（以单个 `-` 开头）返回 `ShortFlag`。通过时原样返回。
pub fn validate_flag(raw: &str) -> Result<String, GuardError> {
    if raw.starts_with('-') && !raw.starts_with("--") {
        return Err(GuardError::ShortFlag(raw.to_owned()));
    }
    let Some(body) = raw.strip_prefix("--") else {
        return Err(GuardError::InvalidFlag(raw.to_owned()));
    };
    if is_flag_body(body) && raw.len() <= MAX_NAME_LEN {
        Ok(raw.to_owned())
    } else {
        Err(GuardError::InvalidFlag(raw.to_owned()))
    }
}

/// 判断是否符合命令名规则。纯函数，供单元测试直接使用。
fn is_command_name(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    let Some((&first, rest)) = bytes.split_first() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && rest.len() < MAX_NAME_LEN
        && rest
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'))
}

/// 判断参数名去掉 `--` 后的部分是否合法。
fn is_flag_body(body: &str) -> bool {
    let bytes = body.as_bytes();
    let Some((&first, rest)) = bytes.split_first() else {
        return false;
    };
    first.is_ascii_alphanumeric() && rest.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_and_punctuated_names() {
        for name in ["git", "g++", "python3.12", "a_b", "x-y", "7zip"] {
            assert_eq!(validate_command(name).unwrap().as_str(), name);
        }
    }

    #[test]
    fn rejects_paths_spaces_and_injection_shapes() {
        for name in [
            "", "-x", "--help", ".hidden", "../etc", "/bin/ls", "git pull", "a;rm", "$(id)", "a|b",
            "a\tb", "a\nb", "a\u{7}b", "ünicode",
        ] {
            assert!(
                matches!(validate_command(name), Err(GuardError::InvalidName(_))),
                "should reject {name:?}"
            );
        }
    }

    #[test]
    fn name_length_boundary() {
        let max = "a".repeat(MAX_NAME_LEN);
        let over = "a".repeat(MAX_NAME_LEN + 1);
        assert!(validate_command(&max).is_ok());
        assert!(matches!(
            validate_command(&over),
            Err(GuardError::InvalidName(_))
        ));
    }

    #[test]
    fn path_validates_each_token() {
        let path = validate_path(&["git", "clone"]).unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[1].as_str(), "clone");
    }

    #[test]
    fn path_rejects_empty_and_bad_tokens() {
        assert_eq!(validate_path(&[]), Err(GuardError::EmptyPath));
        assert!(matches!(
            validate_path(&["git", "clone --branch"]),
            Err(GuardError::InvalidName(_))
        ));
        assert!(matches!(
            validate_path(&["git", "-C"]),
            Err(GuardError::InvalidName(_))
        ));
    }

    #[test]
    fn accepts_long_flags() {
        assert_eq!(validate_flag("--branch").unwrap(), "--branch");
        assert_eq!(validate_flag("--dry-run").unwrap(), "--dry-run");
    }

    #[test]
    fn rejects_short_flags() {
        assert!(matches!(validate_flag("-x"), Err(GuardError::ShortFlag(_))));
        assert!(matches!(validate_flag("-C"), Err(GuardError::ShortFlag(_))));
    }

    #[test]
    fn rejects_malformed_long_flags() {
        for flag in [
            "--",
            "---x",
            "--a b",
            "--a=b",
            "--a;rm",
            "--$(id)",
            "--../x",
            "--dry_run",
            "---x",
            "branch",
        ] {
            assert!(
                matches!(validate_flag(flag), Err(GuardError::InvalidFlag(_))),
                "should reject {flag:?}"
            );
        }
    }

    #[test]
    fn flag_length_boundary() {
        let max = format!("--{}", "a".repeat(MAX_NAME_LEN - 2));
        let over = format!("--{}", "a".repeat(MAX_NAME_LEN - 1));
        assert_eq!(max.len(), MAX_NAME_LEN);
        assert!(validate_flag(&max).is_ok());
        assert!(matches!(
            validate_flag(&over),
            Err(GuardError::InvalidFlag(_))
        ));
    }
}
