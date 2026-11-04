# D1：guard、types

覆盖模块：`guard::name`、`guard::output`、`types`。

## 运行方式

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

## 预期输出

四条命令全部退出码为 0。`cargo test` 中 `guard::name` 与 `guard::output` 的单元测试全部通过。

## 验收项对照

| 验收项（DESIGN 第 12 节、INTERFACES 第 20 节） | 结果 |
|---|---|
| 非法命令名被拒绝（空串、`-x`、`--help`、`.hidden`、`../etc`、`/bin/ls`、空格、`;`、`$()`、`|`、制控字符、非 ASCII） | 通过：`rejects_paths_spaces_and_injection_shapes` |
| 长度边界：命令名 64 字符通过，65 字符拒绝 | 通过：`name_length_boundary` |
| 短参数被拒绝并返回 `ShortFlag` | 通过：`rejects_short_flags` |
| 长参数格式校验 | 通过：`accepts_long_flags`、`rejects_malformed_long_flags`、`flag_length_boundary` |
| 子命令路径逐 token 校验 | 通过：`path_validates_each_token`、`path_rejects_empty_and_bad_tokens` |
| 描述长度上限 400 字符（按字符计，不按字节） | 通过：`description_length_boundary`、`description_length_counts_chars_not_bytes` |
| 示例 200 字符上限、最多 3 条 | 通过：`example_length_boundary`、`example_count_limit` |
| 控制字符去除（换行、制表符、ANSI CSI/OSC、退格） | 通过：`strips_newlines_and_tabs_from_description`、`sanitize_*` 系列 |
| 危险标记（词匹配与子串匹配） | 通过：`dangerous_words_match_whole_words_only`、`dangerous_substrings` |

实际结果：21 个单元测试全部通过，rustdoc 无警告。

## 未覆盖

- `types` 模块只有数据定义，没有可执行逻辑，靠编译与后续 demo 间接验证。
- 本 demo 不含读写文件或网络，没有 `info` / `warn` 日志路径，验收清单第 3 项不适用。
