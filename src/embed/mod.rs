//! Embedding 抽象：`Embedder` trait、节点到嵌入文本的转换、错误类型。
//!
//! D5 只提供 `UnavailableEmbedder` 占位实现（不发起请求，恒返回 `Unreachable`），
//! 真实的 `OpenAiEmbedder` 在 D8 加入。测试替身 `FakeEmbedder` 仅在 `cfg(test)` 下编译。

pub mod openai;

use crate::config::EmbeddingConfig;
use crate::types::{Node, NodeKind};

/// 向量化接口。`embed` 返回与输入等长、顺序一致的向量。
pub trait Embedder {
    /// 模型名，用于索引元数据。
    fn model(&self) -> &str;
    /// 向量维度。
    fn dimensions(&self) -> usize;
    /// 批量嵌入。
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// 节点转嵌入文本。命令节点为 `路径 + 描述 + 示例`；参数节点为 `所属路径 + 参数名 + 参数描述`。
pub fn embed_text(node: &Node) -> String {
    match node.kind {
        NodeKind::Command => {
            let mut parts = vec![node.key.as_str(), node.description.as_str()];
            parts.extend(node.examples.iter().map(String::as_str));
            join_nonempty(&parts)
        }
        NodeKind::Param => join_nonempty(&[
            node.parent.as_deref().unwrap_or(""),
            node.name.as_str(),
            node.description.as_str(),
        ]),
    }
}

/// 去掉空白段后以空格连接。
fn join_nonempty(parts: &[&str]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// embedding 错误。`Unreachable`、`Timeout`、`Auth` 对应退出码 3。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EmbedError {
    /// 网络失败、DNS、连接超时。
    #[error("embedding unreachable: {0}")]
    Unreachable(String),
    /// 请求超时，单位毫秒。
    #[error("embedding timed out after {0} ms")]
    Timeout(u64),
    /// 鉴权失败，或环境变量缺失。
    #[error("embedding auth failed (check env {0})")]
    Auth(String),
    /// 响应格式不符。
    #[error("embedding response malformed: {0}")]
    BadResponse(String),
}

/// D5 占位实现：不发起任何请求，每次调用都返回 `Unreachable`。D8 接入真实客户端后不再使用。
pub struct UnavailableEmbedder {
    model: String,
    dimensions: usize,
}

impl UnavailableEmbedder {
    /// 从配置构造占位实现，只保留模型名与维度。
    pub fn from_config(cfg: &EmbeddingConfig) -> Self {
        Self {
            model: cfg.model.clone(),
            dimensions: cfg.dimensions,
        }
    }
}

impl Embedder for UnavailableEmbedder {
    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Err(EmbedError::Unreachable(
            "embedding client not implemented yet (D8)".to_owned(),
        ))
    }
}

/// 测试替身：按文本返回预设向量，可整体注入失败，并记录调用次数。
#[cfg(test)]
pub(crate) mod fake {
    use std::cell::Cell;
    use std::collections::HashMap;

    use super::{EmbedError, Embedder};

    /// 按文本返回预设向量。未预设的文本返回 `BadResponse`。
    pub(crate) struct FakeEmbedder {
        dimensions: usize,
        answers: HashMap<String, Vec<f32>>,
        fail: bool,
        calls: Cell<usize>,
    }

    impl FakeEmbedder {
        /// 空替身，所有文本都无预设答案。
        pub(crate) fn new(dimensions: usize) -> Self {
            Self {
                dimensions,
                answers: HashMap::new(),
                fail: false,
                calls: Cell::new(0),
            }
        }

        /// 为 `text` 预设向量。
        pub(crate) fn with(mut self, text: &str, vector: Vec<f32>) -> Self {
            self.answers.insert(text.to_owned(), vector);
            self
        }

        /// 每次调用都返回 `Unreachable`。
        pub(crate) fn failing(dimensions: usize) -> Self {
            Self {
                fail: true,
                ..Self::new(dimensions)
            }
        }

        /// `embed` 被调用的次数。
        pub(crate) fn calls(&self) -> usize {
            self.calls.get()
        }
    }

    impl Embedder for FakeEmbedder {
        fn model(&self) -> &str {
            "fake-model"
        }

        fn dimensions(&self) -> usize {
            self.dimensions
        }

        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            self.calls.set(self.calls.get() + 1);
            if self.fail {
                return Err(EmbedError::Unreachable("fake failure".to_owned()));
            }
            texts
                .iter()
                .map(|text| {
                    self.answers.get(text).cloned().ok_or_else(|| {
                        EmbedError::BadResponse(format!("no fake answer for {text}"))
                    })
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EmbedError, Embedder, UnavailableEmbedder, embed_text};
    use crate::config::EmbeddingConfig;
    use crate::index::testing::node;
    use crate::types::NodeKind;

    #[test]
    fn command_text_joins_path_description_and_examples() {
        let mut pull = node("git pull", NodeKind::Command, Some("git"));
        pull.description = "Fetch from a remote.".to_owned();
        pull.examples = vec!["git pull origin main".to_owned()];
        assert_eq!(
            embed_text(&pull),
            "git pull Fetch from a remote. git pull origin main"
        );
    }

    #[test]
    fn param_text_joins_parent_name_and_description() {
        let mut branch = node("git clone --branch", NodeKind::Param, Some("git clone"));
        branch.description = "Check out the given branch.".to_owned();
        assert_eq!(
            embed_text(&branch),
            "git clone --branch Check out the given branch."
        );
    }

    #[test]
    fn unavailable_embedder_never_succeeds() {
        let cfg = EmbeddingConfig {
            endpoint: "http://127.0.0.1:9".to_owned(),
            model: "m".to_owned(),
            dimensions: 3,
            ..EmbeddingConfig::default()
        };
        let embedder = UnavailableEmbedder::from_config(&cfg);
        assert_eq!(embedder.model(), "m");
        assert_eq!(embedder.dimensions(), 3);
        assert!(matches!(
            embedder.embed(&["x".to_owned()]),
            Err(EmbedError::Unreachable(_))
        ));
    }
}
