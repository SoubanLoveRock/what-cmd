//! 查询文本规范化：去首尾空白、转小写、连续空白折叠。纯函数。

use crate::search::SearchError;

/// trim → 转小写 → 连续空白折叠为单个空格。规范化后为空返回 `EmptyQuery`。
pub fn normalize(raw: &str) -> Result<String, SearchError> {
    let folded = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if folded.is_empty() {
        Err(SearchError::EmptyQuery)
    } else {
        Ok(folded)
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;
    use crate::search::SearchError;

    #[test]
    fn trims_folds_case_and_collapses_whitespace() {
        assert_eq!(
            normalize("  Pull   ROMOTE \t repo ").unwrap(),
            "pull romote repo"
        );
    }

    #[test]
    fn empty_and_blank_input_is_rejected() {
        assert!(matches!(normalize(""), Err(SearchError::EmptyQuery)));
        assert!(matches!(normalize(" \t\n "), Err(SearchError::EmptyQuery)));
    }
}
