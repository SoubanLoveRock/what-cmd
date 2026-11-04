//! 打分纯函数：语义分、子树分、使用加分、排序。
//!
//! 系数先写成常量，固定样本调参前不开放配置（见 DEVLOG 2026-11-03 D5）。

use crate::index::vectors::dot;
use crate::types::Candidate;

/// 子树分的层级衰减。后代比自身低一层时乘以该系数。
pub const DECAY: f32 = 0.8;
/// 候选阈值。无精确置顶时，分数低于它的节点不算候选。
pub const MIN_SCORE: f32 = 0.30;
/// 使用加分上限。
pub const USAGE_CAP: f32 = 0.10;
/// 使用加分的对数系数：`bonus = min(cap, alpha × ln(1 + count))`。
pub const USAGE_ALPHA: f32 = 0.05;
/// 使用加分的半衰期（天）。
pub const USAGE_HALF_LIFE_DAYS: f32 = 30.0;

/// `s_self(n) = cos(q, v_n)`。两者都已归一化时即点积。
pub fn self_score(query_vec: &[f32], node_vec: &[f32]) -> f32 {
    dot(query_vec, node_vec)
}

/// `s(n) = max(s_self(n), max over 后代 d of DECAY^depth(d) × s_self(d))`。
///
/// `descendants` 为 `(s_self(d), 相对深度)`，直接子节点深度为 1。
/// 返回 `(最终分, 取到最大值的后代在 descendants 中的索引)`。后代没有超过自身时索引为 `None`。
pub fn subtree_score(
    self_score_of_node: f32,
    descendants: &[(f32, usize)],
) -> (f32, Option<usize>) {
    let mut best = self_score_of_node;
    let mut best_index = None;
    for (index, (score, depth)) in descendants.iter().enumerate() {
        let decayed = DECAY.powi(*depth as i32) * score;
        if decayed > best {
            best = decayed;
            best_index = Some(index);
        }
    }
    (best, best_index)
}

/// `bonus = min(cap, alpha × ln(1 + count)) × 2^(−age_days / half_life)`。`count` 为 0 时为 0。
pub fn usage_bonus(count: u32, age_days: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let base = (USAGE_ALPHA * (1.0 + count as f32).ln()).min(USAGE_CAP);
    base * 2f32.powf(-age_days.max(0.0) / USAGE_HALF_LIFE_DAYS)
}

/// 排序：精确置顶优先，其余按 `score`（已含使用加分）降序，同分按 key 字典序。
pub fn rank(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.exact
            .cmp(&a.exact)
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| a.key.cmp(&b.key))
    });
}

#[cfg(test)]
mod tests {
    use super::{DECAY, USAGE_CAP, USAGE_HALF_LIFE_DAYS, rank, subtree_score, usage_bonus};
    use crate::types::{Candidate, NodeKind};

    fn cand(key: &str, score: f32, exact: bool) -> Candidate {
        Candidate {
            key: key.to_owned(),
            kind: NodeKind::Command,
            score,
            exact,
            insert: String::new(),
            delete_tail: 0,
            description: String::new(),
            example: None,
            dangerous: false,
            via: None,
        }
    }

    #[test]
    fn own_score_wins_over_weaker_descendant() {
        let (score, index) = subtree_score(0.9, &[(1.0, 1)]);
        assert!((score - 0.9).abs() < 1e-6);
        assert_eq!(index, None);
    }

    #[test]
    fn descendant_is_decayed_per_level() {
        let (score, index) = subtree_score(0.1, &[(1.0, 1)]);
        assert!((score - DECAY).abs() < 1e-6);
        assert_eq!(index, Some(0));

        let (deeper, _) = subtree_score(0.0, &[(1.0, 2)]);
        assert!((deeper - DECAY * DECAY).abs() < 1e-6);
    }

    #[test]
    fn descendant_above_own_score_returns_its_index() {
        // 第一个后代 0.5 × 0.8 = 0.4 超过自身 0.2；第二个 0.9 × 0.8² = 0.576 更大，取它。
        let (score, index) = subtree_score(0.2, &[(0.5, 1), (0.9, 2)]);
        assert!((score - 0.9 * DECAY * DECAY).abs() < 1e-6);
        assert_eq!(index, Some(1));
    }

    #[test]
    fn usage_bonus_is_zero_without_use_and_capped() {
        assert!(usage_bonus(0, 0.0).abs() < 1e-6);
        assert!((usage_bonus(10_000, 0.0) - USAGE_CAP).abs() < 1e-6);
    }

    #[test]
    fn usage_bonus_halves_per_half_life() {
        let fresh = usage_bonus(1, 0.0);
        let aged = usage_bonus(1, USAGE_HALF_LIFE_DAYS);
        assert!((aged - fresh / 2.0).abs() < 1e-6);
    }

    #[test]
    fn rank_puts_exact_first_then_score_then_key() {
        let mut found = vec![
            cand("b", 0.9, false),
            cand("a", 0.5, true),
            cand("c", 0.9, false),
        ];
        rank(&mut found);
        let keys: Vec<&str> = found.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, ["a", "b", "c"]);
    }
}
