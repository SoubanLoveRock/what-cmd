//! 候选行的输出格式。stdout 只输出候选行，字段以 tab 分隔。

use serde_json::{Value, json};

use crate::types::{Candidate, NodeKind};

/// 一行候选，九个字段以 tab 分隔：
/// `key \t score \t kind \t insert \t delete_tail \t description \t example \t dangerous \t via`。
///
/// 字段内的 tab 与换行替换为空格，保证列数稳定。
pub fn format_line(c: &Candidate) -> String {
    let fields = [
        c.key.clone(),
        format!("{:.4}", c.score),
        kind_label(c.kind).to_owned(),
        c.insert.clone(),
        c.delete_tail.to_string(),
        c.description.clone(),
        c.example.clone().unwrap_or_default(),
        u8::from(c.dangerous).to_string(),
        c.via.clone().unwrap_or_default(),
    ];
    fields
        .iter()
        .map(|field| field.replace(['\t', '\n', '\r'], " "))
        .collect::<Vec<_>>()
        .join("\t")
}

/// `--json` 模式：一个候选对应一个 JSON 对象。
pub fn format_json(c: &Candidate) -> Value {
    json!({
        "key": c.key,
        "score": c.score,
        "kind": kind_label(c.kind),
        "exact": c.exact,
        "insert": c.insert,
        "delete_tail": c.delete_tail,
        "description": c.description,
        "example": c.example,
        "dangerous": c.dangerous,
        "via": c.via,
    })
}

/// 节点类型的输出名。
fn kind_label(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Command => "command",
        NodeKind::Param => "param",
    }
}

#[cfg(test)]
mod tests {
    use super::{format_json, format_line};
    use crate::types::{Candidate, NodeKind};

    fn sample() -> Candidate {
        Candidate {
            key: "git clone".to_owned(),
            kind: NodeKind::Command,
            score: 0.8,
            exact: false,
            insert: "clone".to_owned(),
            delete_tail: 2,
            description: "Clone a repository.".to_owned(),
            example: Some("git clone <url>".to_owned()),
            dangerous: false,
            via: None,
        }
    }

    #[test]
    fn line_has_nine_tab_separated_fields() {
        let line = format_line(&sample());
        let fields: Vec<&str> = line.split('\t').collect();
        assert_eq!(fields.len(), 9);
        assert_eq!(fields[0], "git clone");
        assert_eq!(fields[1], "0.8000");
        assert_eq!(fields[3], "clone");
        assert_eq!(fields[4], "2");
        assert_eq!(fields[7], "0");
        assert_eq!(fields[8], "");
    }

    #[test]
    fn control_characters_in_fields_are_flattened() {
        let mut c = sample();
        c.description = "line one\nline\ttwo".to_owned();
        let line = format_line(&c);
        assert_eq!(line.split('\t').count(), 9);
        assert!(line.contains("line one line two"));
    }

    #[test]
    fn json_carries_delete_tail_and_via() {
        let mut c = sample();
        c.via = Some("*".to_owned());
        let value = format_json(&c);
        assert_eq!(value["delete_tail"], 2);
        assert_eq!(value["via"], "*");
        assert_eq!(value["kind"], "command");
    }
}
