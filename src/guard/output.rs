//! 描述与示例校验、危险标记、输出清洗。
//!
//! 描述与示例来自 agent 输出，视为不可信数据：去控制字符、限长度、
//! 限条数。文档原文的清洗（`sanitize_text`）供 docs 与 mcp 共用。

use std::iter::Peekable;
use std::str::Chars;

use crate::guard::GuardError;

/// 描述字符数上限。
pub const MAX_DESCRIPTION_CHARS: usize = 400;
/// 单条示例字符数上限。
pub const MAX_EXAMPLE_CHARS: usize = 200;
/// 示例条数上限。
pub const MAX_EXAMPLES: usize = 3;

/// 通过校验并清洗后的描述与示例。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDescription {
    /// 已去控制字符，长度不超过 400 字符。
    pub description: String,
    /// 最多 3 条，每条不超过 200 字符。
    pub examples: Vec<String>,
}

/// 校验并清洗命令节点的描述与示例。
///
/// 控制字符（含换行、制表符）替换为空格后折叠。超长描述返回 `DescriptionTooLong`。
/// 示例超过 3 条，或清洗后单条超过 200 字符，返回 `ExampleLimit`，不截断。
pub fn validate_output(
    desc: &str,
    examples: &[String],
) -> Result<ValidatedDescription, GuardError> {
    if examples.len() > MAX_EXAMPLES {
        return Err(GuardError::ExampleLimit);
    }
    let description = validate_param_description(desc)?;

    let mut cleaned = Vec::with_capacity(examples.len());
    for example in examples {
        let line = clean_line(example);
        if line.chars().count() > MAX_EXAMPLE_CHARS {
            return Err(GuardError::ExampleLimit);
        }
        if !line.is_empty() {
            cleaned.push(line);
        }
    }

    Ok(ValidatedDescription {
        description,
        examples: cleaned,
    })
}

/// 参数描述校验，规则与 `validate_output` 的描述部分相同，不含示例。
pub fn validate_param_description(desc: &str) -> Result<String, GuardError> {
    let line = clean_line(desc);
    let len = line.chars().count();
    if len > MAX_DESCRIPTION_CHARS {
        return Err(GuardError::DescriptionTooLong(len));
    }
    Ok(line)
}

/// 判断示例是否含危险操作，供候选标记与选中前二次确认。
///
/// `rm`、`dd`、`mkfs` 按词匹配（按非字母数字字符切分），
/// `> /dev/`、`chmod -R`、`chown -R` 按子串匹配。
pub fn is_dangerous(example: &str) -> bool {
    let word_hit = example
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| matches!(word, "rm" | "dd" | "mkfs"));
    word_hit
        || ["> /dev/", "chmod -R", "chown -R"]
            .iter()
            .any(|pattern| example.contains(pattern))
}

/// 输出清洗：去除 ANSI 转义序列与退格，统一换行为 `\n`。
///
/// 退格会删除前一个字符（man 页的粗体与下划线即如此编码）。
pub fn sanitize_text(raw: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            '\u{8}' => {
                out.pop();
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            _ => out.push(c),
        }
    }
    out.into_iter().collect()
}

/// 跳过一个 ESC 序列。调用时 ESC 已被消费。
///
/// CSI（`ESC [`）读到 `@`..=`~` 的终结字节为止；OSC（`ESC ]`）读到 BEL 或 `ESC \` 为止；
/// 其他两字符序列只跳过下一个字符。
fn skip_escape(chars: &mut Peekable<Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
        Some(']') => {
            while let Some(c) = chars.next() {
                if c == '\u{7}' {
                    break;
                }
                if c == '\u{1b}' {
                    chars.next();
                    break;
                }
            }
        }
        _ => {}
    }
}

/// 把控制字符替换为空格，并折叠连续空白。描述与示例共用。
fn clean_line(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn examples(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn strips_newlines_and_tabs_from_description() {
        let out = validate_output("line one\nline\ttwo\r\n  three", &[]).unwrap();
        assert_eq!(out.description, "line one line two three");
    }

    #[test]
    fn description_length_boundary() {
        let max = "a".repeat(MAX_DESCRIPTION_CHARS);
        let over = "a".repeat(MAX_DESCRIPTION_CHARS + 1);
        assert!(validate_output(&max, &[]).is_ok());
        assert_eq!(
            validate_output(&over, &[]),
            Err(GuardError::DescriptionTooLong(MAX_DESCRIPTION_CHARS + 1))
        );
    }

    #[test]
    fn description_length_counts_chars_not_bytes() {
        let max = "中".repeat(MAX_DESCRIPTION_CHARS);
        assert!(validate_param_description(&max).is_ok());
    }

    #[test]
    fn example_count_limit() {
        let three = examples(&["a", "b", "c"]);
        let four = examples(&["a", "b", "c", "d"]);
        assert!(validate_output("d", &three).is_ok());
        assert_eq!(validate_output("d", &four), Err(GuardError::ExampleLimit));
    }

    #[test]
    fn example_length_boundary() {
        let max = vec!["x".repeat(MAX_EXAMPLE_CHARS)];
        let over = vec!["x".repeat(MAX_EXAMPLE_CHARS + 1)];
        assert!(validate_output("d", &max).is_ok());
        assert_eq!(validate_output("d", &over), Err(GuardError::ExampleLimit));
    }

    #[test]
    fn example_control_chars_are_cleaned_before_length_check() {
        let out = validate_output("d", &examples(&["ls\n-l\t/tmp"])).unwrap();
        assert_eq!(out.examples, vec!["ls -l /tmp".to_owned()]);
    }

    #[test]
    fn dangerous_words_match_whole_words_only() {
        assert!(is_dangerous("rm -rf build"));
        assert!(is_dangerous("sudo dd if=/dev/zero of=disk"));
        assert!(is_dangerous("mkfs.ext4 /dev/sda1"));
        assert!(is_dangerous("ls | rm"));
        assert!(is_dangerous("/bin/rm file"));
        assert!(!is_dangerous("git add -A"));
        assert!(!is_dangerous("find . -name form"));
        assert!(!is_dangerous("rmdir empty"));
    }

    #[test]
    fn dangerous_substrings() {
        assert!(is_dangerous("cat x > /dev/null"));
        assert!(is_dangerous("chmod -R 777 ."));
        assert!(is_dangerous("chown -R user ."));
        assert!(!is_dangerous("chmod 644 file"));
    }

    #[test]
    fn sanitize_removes_csi_sequences() {
        assert_eq!(sanitize_text("\u{1b}[31mred\u{1b}[0m text"), "red text");
    }

    #[test]
    fn sanitize_removes_osc_sequences() {
        assert_eq!(sanitize_text("\u{1b}]0;title\u{7}ok"), "ok");
        assert_eq!(sanitize_text("\u{1b}]8;;http://x\u{1b}\\link"), "link");
    }

    #[test]
    fn sanitize_applies_backspace_overstrike() {
        // man 粗体：字符 退格 字符
        assert_eq!(sanitize_text("B\u{8}Bo\u{8}ol\u{8}ld"), "Bold");
        assert_eq!(sanitize_text("\u{8}x"), "x");
    }

    #[test]
    fn sanitize_normalizes_newlines() {
        assert_eq!(sanitize_text("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }
}
