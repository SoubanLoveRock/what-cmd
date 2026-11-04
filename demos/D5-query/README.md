# D5：打分、输出与 query 子命令

覆盖模块：`search::score`、`search::query`（编排）、`output`、`cli` 的 `query` 子命令。
`embed` 只含 trait、`FakeEmbedder` 与不可用的占位实现，`usage` 只读。

本 demo 不接真实 embedding 服务。`whatcmd query` 在 D8 之前一律返回退出码 `3`，端到端手动验收推迟到 D8。

## 运行方式

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

只跑本 demo 相关的测试：

```bash
cargo test search::
cargo test output::
cargo test cli::
```

## 预期输出

- 四条命令退出码均为 0。
- `cargo test` 共 122 项通过。

## 验收项

| 验收要求 | 对应测试 |
|---|---|
| 固定向量下的排序（精确置顶、分数、key 字典序） | `rank_puts_exact_first_then_score_then_key` |
| 精确置顶 | `exact_key_match_ranks_first_without_similarity` |
| 子树衰减 | `descendant_is_decayed_per_level`、`ancestor_gets_decayed_subtree_score` |
| 后代分高于自身时返回其索引 | `descendant_above_own_score_returns_its_index` |
| 回退到整棵树，`via = "*"` | `miss_in_anchor_falls_back_to_root_with_via` |
| 回退到父节点，`via = 父节点 key` | `miss_in_anchor_falls_back_to_parent_with_via` |
| 结果限定在锚点子树内 | `results_stay_under_anchor_subtree`（按 `Tree::ancestors` 判断） |
| 已在命令行中出现的参数不再推荐 | `typed_param_is_not_recommended_again` |
| 锚点错误时不调用 embedder | `anchor_error_does_not_call_embedder` |
| embedding 失败映射为 `SearchError::Embed` | `embed_failure_is_reported` |
| 空查询被拒绝 | `blank_query_is_rejected` |
| 使用加分随时间衰减、有上限 | `usage_bonus_halves_per_half_life`、`usage_bonus_is_zero_without_use_and_capped` |
| `--` 之后的 `--branch`、`-x` 收为文本 | `double_dash_keeps_later_dashes_as_text` |
| 不可用的 embedder 映射为退出码 3 | `unavailable_embedder_maps_to_embed_exit_code` |

## 实际结果

- fmt、clippy（`-D warnings`）、test（122 项）、rustdoc（`-D warnings`）退出码均为 0。
- 复核时发现的五项问题已修复，明细见 DEVLOG 2026-11-03 D5 的"复核修正"。

## 已知未覆盖

- `unavailable_embedder_maps_to_embed_exit_code` 只验证退出码映射，不发起网络请求。真实 endpoint 不可达时的退出码 3 要到 D8 手动验收。
- `via` 的展示格式（命中层级还是最佳子路径）未经样本确认。
- 回退候选的插入语义未定，当前 `insert` 为完整路径、`delete_tail = 0`。widget 接入时再决定，见 DEVLOG。
- `SearchError::Vector` 的触发路径没有单独测试。
- 相关日志（`info`、`warn`）没有被测试捕获，与 D3、D4 相同，等 `tracing-subscriber` 接入后统一补。
- 打分系数（`DECAY`、`MIN_SCORE`、`USAGE_*`）未调参。
