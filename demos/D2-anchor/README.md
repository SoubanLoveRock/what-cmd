# D2：锚点解析与插入文本

覆盖模块：`index::Tree`、`search::normalize`、`search::anchor`。

纯本地，不联网，不读文件。测试使用 `index::testing::sample_nodes` 构造的固定小树。

## 运行方式

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

只跑本 demo 的测试：

```bash
cargo test search::
cargo test index::tests
```

## 预期输出

- 四条命令退出码均为 0。
- `cargo test` 共 56 项通过，其中 D2 新增 35 项（`index::tests` 8 项，`search::normalize` 2 项，`search::anchor` 25 项）。

## 验收项

| 验收要求（DESIGN 第 7.1 节与第 20 节） | 对应测试 |
|---|---|
| 末尾未完成词丢弃 | `trailing_partial_word_is_dropped`、`bare_partial_root_is_dropped_too` |
| `-` 开头的 token 跳过 | `short_flag_is_skipped_without_affecting_anchor`、`long_flag_is_skipped_and_descent_continues` |
| `--` 单独出现时停止 | `double_dash_stops_anchor_and_hides_later_flags` |
| 非子命令 token 停止锚点解析 | `unknown_token_stops_descent`（`git -C repo clone` 锚点为 `git`） |
| `giit` 报错并给建议 | `unknown_root_reports_typo_with_suggestion`、`transposed_root_gets_suggestion`、`unknown_root_without_close_match_has_no_suggestion` |
| 插入替换未完成词（`git cl` → `git clone`） | `insert_replaces_partial_word_with_subcommand` |
| 已在命令链末尾不重复追加 | `chain_already_at_end_is_not_appended_again`、`param_already_typed_is_not_appended_again` |
| 参数 `=` 前比较去重 | `flags_are_recorded_by_name_before_equals` |

## 实际结果

- fmt、clippy（`-D warnings`）、test（56 项）、rustdoc（`-D warnings`）退出码均为 0。
- 以上结果是在 D2 代码最后一次改动之后运行的。之后只修改了文档（INTERFACES、DEVLOG、CHANGELOG、本 README），代码未变，结果仍然有效。
- 本次验收只使用内存数据，没有文件 IO，也不依赖本机 PATH。

## 已知遗留

见 DEVLOG 2026-11-03 D2 条目：`git clone cl` 的插入行为待 widget 接入后验证；子命令拼写错误不给建议。
