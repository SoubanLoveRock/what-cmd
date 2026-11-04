//! Agent 探测与 `--help` 能力解析。
//!
//! D7 只包含 [`parse_capabilities`]。PATH 查找与 `--version` 探针在 D8 中加入。
//! 解析只看选项定义行（缩进两个空格、以 `-` 开头），不看描述文字里提到的参数名，
//! 避免把 "see --tools" 之类的句子误判为能力存在。

use std::collections::BTreeSet;

use super::{Capability, CapabilitySet};

/// 纯函数：从 `--help` 文本解析能力。缺少所需参数时，对应能力不加入集合。
///
/// 选项名按精确匹配判断，`--tools` 不会匹配 `--disallowedTools` 或 `--tools-x`。
pub fn parse_capabilities(help_text: &str) -> CapabilitySet {
    let flags = option_names(help_text);
    let has = |names: &[&str]| names.iter().any(|n| flags.contains(n));

    let mut caps = CapabilitySet::new();
    if has(&["-p", "--print"]) {
        caps.insert(Capability::NonInteractive);
    }
    if has(&["--output-format"]) {
        caps.insert(Capability::JsonOutput);
    }
    if has(&["--mcp-config"]) {
        caps.insert(Capability::Mcp);
    }
    if has(&["--strict-mcp-config"]) {
        caps.insert(Capability::StrictMcp);
    }
    if has(&["--allowedTools", "--allowed-tools"]) {
        caps.insert(Capability::ToolAllowlist);
    }
    if has(&["--tools"]) {
        caps.insert(Capability::RestrictedTools);
    }
    if has(&["--json-schema"]) {
        caps.insert(Capability::JsonSchema);
    }
    caps
}

/// 收集选项定义行中出现的全部选项名。
///
/// 选项定义行的特征：缩进恰好两个空格，随后是 `-`。描述续行缩进更深，被忽略。
/// 名字之后可能是 `<arg>`、`[arg]` 或描述（以两个空格分隔），这些都截掉。
fn option_names(help_text: &str) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    for line in help_text.lines() {
        let Some(rest) = line.strip_prefix("  ") else {
            continue;
        };
        if rest.starts_with(' ') || !rest.starts_with('-') {
            continue;
        }
        let head = rest.split("  ").next().unwrap_or("");
        let head = head.split(['<', '[']).next().unwrap_or("");
        for part in head.split(',') {
            if let Some(name) = part.split_whitespace().next() {
                names.insert(name);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::parse_capabilities;
    use crate::agent::{Capability, REQUIRED, missing_capabilities};

    /// 本机 `claude --help`（2.1.296）的原文，见 tests/fixtures。
    const HELP: &str = include_str!("../../tests/fixtures/claude-help-2.1.296.txt");

    /// 去掉以 `prefix` 开头的行，模拟旧版本缺少某个参数。
    fn without_lines(text: &str, prefix: &str) -> String {
        text.lines()
            .filter(|l| !l.starts_with(prefix))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn real_help_yields_every_required_capability() {
        let caps = parse_capabilities(HELP);
        assert!(missing_capabilities(&caps).is_empty(), "missing: {caps:?}");
    }

    #[test]
    fn missing_tools_option_drops_restricted_tools() {
        let text = without_lines(HELP, "  --tools ");
        let caps = parse_capabilities(&text);
        assert!(!caps.contains(&Capability::RestrictedTools));
        assert_eq!(
            missing_capabilities(&caps),
            vec![Capability::RestrictedTools]
        );
    }

    #[test]
    fn missing_output_format_drops_json_output() {
        let text = without_lines(HELP, "  --output-format ");
        let caps = parse_capabilities(&text);
        assert!(!caps.contains(&Capability::JsonOutput));
        assert_eq!(missing_capabilities(&caps), vec![Capability::JsonOutput]);
    }

    #[test]
    fn missing_json_schema_drops_json_schema() {
        let text = without_lines(HELP, "  --json-schema");
        let caps = parse_capabilities(&text);
        assert_eq!(missing_capabilities(&caps), vec![Capability::JsonSchema]);
    }

    #[test]
    fn lookalike_names_do_not_match() {
        // --disallowedTools 与 --tools-extra 都不应被当成 --tools。
        let text = "  --disallowedTools, --disallowed-tools <tools...>  deny\n  --tools-extra <x>  other\n";
        let caps = parse_capabilities(text);
        assert!(!caps.contains(&Capability::RestrictedTools));
        assert!(!caps.contains(&Capability::ToolAllowlist));
    }

    #[test]
    fn mention_inside_description_does_not_count() {
        // 描述续行缩进更深，里面提到 --tools 不算选项定义。
        let text = "  --other <x>                   see also --tools for details\n";
        let caps = parse_capabilities(text);
        assert!(!caps.contains(&Capability::RestrictedTools));
    }

    #[test]
    fn allowed_tools_accepts_either_spelling() {
        let camel = "  --allowedTools, --allowed-tools <tools...>\n";
        let kebab = "  --allowed-tools <tools...>\n";
        assert!(parse_capabilities(camel).contains(&Capability::ToolAllowlist));
        assert!(parse_capabilities(kebab).contains(&Capability::ToolAllowlist));
    }

    #[test]
    fn print_short_and_long_forms_both_count() {
        assert!(
            parse_capabilities("  -p, --print  Print and exit\n")
                .contains(&Capability::NonInteractive)
        );
        assert!(
            parse_capabilities("  --print  Print and exit\n").contains(&Capability::NonInteractive)
        );
    }

    #[test]
    fn empty_help_yields_nothing() {
        let caps = parse_capabilities("");
        assert_eq!(missing_capabilities(&caps).len(), REQUIRED.len());
    }
}
