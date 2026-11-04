//! 批次输出的结构、解析与校验。
//!
//! agent 不调用工具，只按 [`schema`] 返回一个 JSON 对象。程序解析后用 [`to_records`] 转成暂存记录，
//! 校验不通过的字段直接丢弃，不写入暂存区。

use std::collections::HashMap;

use serde::Deserialize;

use crate::generate::staging::{StagedParam, StagedRecord};
use crate::guard::name::{validate_command, validate_flag};
use crate::guard::output::{validate_output, validate_param_description};

/// 一批的输出。
#[derive(Debug, Deserialize)]
pub struct BatchOutput {
    /// 每个命令一条。
    pub commands: Vec<CommandOutput>,
}

/// 单个命令的输出。
#[derive(Debug, Deserialize)]
pub struct CommandOutput {
    /// 命令 key，空格分隔，与提示词中的标题一致。
    pub path: String,
    /// 描述。
    pub description: String,
    /// 示例，每条为完整命令行。
    pub examples: Vec<String>,
    /// 直接子命令名，不含父路径。
    pub subcommands: Vec<String>,
    /// 长参数。
    pub params: Vec<ParamOutput>,
}

/// 单个长参数的输出。
#[derive(Debug, Deserialize)]
pub struct ParamOutput {
    /// 长参数名，如 `--branch`。
    pub flag: String,
    /// 参数描述。
    pub description: String,
}

/// 输出的 JSON Schema，经 `--json-schema` 传给 `claude`。
pub fn schema() -> String {
    serde_json::json!({
        "type": "object",
        "properties": {
            "commands": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string" },
                        "description": { "type": "string" },
                        "examples": { "type": "array", "items": { "type": "string" } },
                        "subcommands": { "type": "array", "items": { "type": "string" } },
                        "params": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "flag": { "type": "string" },
                                    "description": { "type": "string" }
                                },
                                "required": ["flag", "description"]
                            }
                        }
                    },
                    "required": ["path", "description", "examples", "subcommands", "params"]
                }
            }
        },
        "required": ["commands"]
    })
    .to_string()
}

/// 解析一批的输出。结构不符时返回 serde 错误。
pub fn parse_batch(raw: &str) -> Result<BatchOutput, serde_json::Error> {
    serde_json::from_str(raw)
}

/// 把一批输出转为暂存记录。
///
/// 只接受 `keys` 中的路径，防止模型写入本批以外的节点。描述与示例经 `guard::output` 校验。
/// 子命令由提示词约束，这里只检查命名合法且不以 `-` 开头。长参数必须是 `--` 长参数，且在文档中出现。
/// 不合格的字段丢弃，不影响同批的其他命令。
pub fn to_records(
    out: &BatchOutput,
    keys: &[String],
    docs: &HashMap<String, String>,
) -> Vec<StagedRecord> {
    let mut records = Vec::new();
    for cmd in &out.commands {
        if !keys.contains(&cmd.path) {
            continue;
        }
        let Some(doc) = docs.get(&cmd.path) else {
            continue;
        };
        let Ok(checked) = validate_output(&cmd.description, &cmd.examples) else {
            continue;
        };
        // 子节点没有示例时多半是模型把报错里的名字当成了子命令，丢弃。
        if path_depth(&cmd.path) > 1 && checked.examples.is_empty() {
            continue;
        }
        let subcommands: Vec<String> = cmd
            .subcommands
            .iter()
            .filter(|sub| {
                // 子命令由模型按提示词从文档中提取，这里只做安全校验：
                // 名字会作为参数执行 `<父命令> <名字> --help`，长参数不能当成子命令。
                !sub.starts_with('-') && validate_command(sub).is_ok()
            })
            .cloned()
            .collect();
        let params: Vec<StagedParam> = cmd
            .params
            .iter()
            .filter_map(|p| {
                let flag = validate_flag(&p.flag).ok()?;
                if !starts_doc_line(doc, &flag) {
                    return None;
                }
                let description = validate_param_description(&p.description).ok()?;
                Some(StagedParam { flag, description })
            })
            .collect();
        let path: Vec<String> = cmd.path.split(' ').map(str::to_owned).collect();
        records.push(StagedRecord::Command {
            path: path.clone(),
            description: checked.description,
            examples: checked.examples,
            subcommands,
        });
        if !params.is_empty() {
            records.push(StagedRecord::Params { path, params });
        }
    }
    records
}

/// 命令路径的段数（空格分隔）。
fn path_depth(path: &str) -> usize {
    path.split(' ').count()
}

/// 文档中是否有某行以 `token` 开头（忽略前导空白）。参数列表项是这种形式，
/// 描述正文里提到的词不算。
fn starts_doc_line(doc: &str, token: &str) -> bool {
    doc.lines().any(|line| {
        line.trim_start().strip_prefix(token).is_some_and(|rest| {
            rest.is_empty()
                || rest
                    .starts_with(|c: char| c.is_whitespace() || matches!(c, ',' | '=' | '[' | '<'))
        })
    })
}

/// 测试用：生成一批合法的输出。`root_subcommands` 只挂在单段路径上。
#[cfg(test)]
pub fn test_batch_json(
    paths: &[Vec<crate::types::CommandName>],
    description: &str,
    root_subcommands: &[&str],
) -> String {
    let commands: Vec<serde_json::Value> = paths
        .iter()
        .map(|path| {
            let key = path
                .iter()
                .map(crate::types::CommandName::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            let subs: &[&str] = if path.len() == 1 {
                root_subcommands
            } else {
                &[]
            };
            serde_json::json!({
                "path": key,
                "description": description,
                "examples": [],
                "subcommands": subs,
                "params": []
            })
        })
        .collect();
    serde_json::json!({ "commands": commands }).to_string()
}

#[cfg(test)]
mod tests {
    use super::{parse_batch, to_records};
    use crate::generate::staging::{StagedParam, StagedRecord};
    use std::collections::HashMap;

    #[test]
    fn subcommands_drop_flags_and_params_must_start_a_doc_line() {
        let doc = "COMMANDS\n   clone   Clone a repo.\n   Use --merged to filter refs.\n   --all  Pack all.\n";
        let json = r#"{"commands":[{"path":"git","description":"Git tool.","examples":[],"subcommands":["clone","--version","-v"],"params":[{"flag":"--all","description":"Pack all."},{"flag":"--merged","description":"Merged."}]}]}"#;
        let out = parse_batch(json).unwrap();
        let keys = vec!["git".to_owned()];
        let docs = HashMap::from([("git".to_owned(), doc.to_owned())]);
        let records = to_records(&out, &keys, &docs);
        assert_eq!(
            records[0],
            StagedRecord::Command {
                path: vec!["git".to_owned()],
                description: "Git tool.".to_owned(),
                examples: Vec::new(),
                subcommands: vec!["clone".to_owned()],
            }
        );
        assert_eq!(
            records[1],
            StagedRecord::Params {
                path: vec!["git".to_owned()],
                params: vec![StagedParam {
                    flag: "--all".to_owned(),
                    description: "Pack all.".to_owned(),
                }],
            }
        );
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn subcommands_are_not_matched_against_doc_format() {
        // git(1) 的列表项是 `git-add(1)`，没有以裸 `add` 开头的行；子命令以模型输出为准。
        let doc = "GIT COMMANDS\n     git-add(1)\n\t Add file contents.\n     git-bisect(1)\n\t Find a bug.\n";
        let json = r#"{"commands":[{"path":"git","description":"Git tool.","examples":[],"subcommands":["add","bisect"],"params":[]}]}"#;
        let out = parse_batch(json).unwrap();
        let keys = vec!["git".to_owned()];
        let docs = HashMap::from([("git".to_owned(), doc.to_owned())]);
        let records = to_records(&out, &keys, &docs);
        let StagedRecord::Command { subcommands, .. } = &records[0] else {
            panic!("expected command record");
        };
        assert_eq!(subcommands, &["add".to_owned(), "bisect".to_owned()]);
    }
}
