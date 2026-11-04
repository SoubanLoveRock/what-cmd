# D3：配置、索引读写与向量存储

覆盖模块：`config`、`index`（`load`、`save_atomic`、`Stored`）、`index::vectors`。

不做 embedding 调用，不做 PATH 扫描，不写暂存区。测试全部使用 `target/test-tmp` 下的临时目录，不读写真实 `$HOME`。

## 运行方式

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

只跑本 demo 的测试：

```bash
cargo test config::
cargo test index::persist_tests
cargo test index::vectors
```

## 预期输出

- 四条命令退出码均为 0。
- `cargo test` 共 83 项通过，其中 D3 新增 27 项（`config::tests` 12 项，`index::persist_tests` 7 项，`index::vectors::tests` 8 项）。

## 验收项

| 验收要求（AGENT.md 第 7 节与 DESIGN 第 12 节） | 对应测试 |
|---|---|
| 配置文件缺失返回默认值 | `missing_file_returns_expanded_defaults` |
| 部分配置文件补齐默认值 | `partial_file_fills_defaults` |
| 完整配置解析正确 | `parses_full_example`、`full_file_loads_and_expands_under_given_home` |
| 顶层 `api_key` 报 `ForbiddenKey` | `top_level_api_key_is_rejected` |
| 嵌套 `api_key` 报 `ForbiddenKey` | `nested_api_key_is_rejected`、`api_key_inside_array_of_tables_is_rejected` |
| `~` 按传入的 home 展开 | `tilde_expands_with_given_home` |
| `WHATCMD_CONFIG` 优先级 | `config_path_prefers_explicit_then_env_then_default`（纯函数，见下方说明） |
| embedding 必填字段缺失时报错 | `missing_endpoint_fails_validation`、`zero_dimensions_fails_validation` |
| `api_key_env` 未设置时返回 `None` | `unset_api_key_env_yields_none` |
| `save_atomic` 后 `load` 得到相同内容 | `save_then_load_round_trips` |
| 保存后无残留 `.tmp` | `save_leaves_no_tmp_files` |
| 目录为空返回 `Ok(None)` | `empty_directory_loads_as_none` |
| `schema_version` 不匹配报 `SchemaVersion` | `schema_mismatch_is_rejected` |
| `vectors.bin` 长度不符报 `Length` | `truncated_vectors_are_rejected_by_length` |
| 长度相同但内容被改，报 `VectorHashMismatch` | `same_length_corruption_is_rejected_by_hash` |
| 行数与节点数不符时拒绝写入且不落盘 | `save_rejects_row_count_mismatch_without_writing` |
| 归一化后点积等于余弦 | `normalized_dot_equals_cosine` |
| 零向量报错 | `zero_vector_is_rejected` |
| `write_tmp` 后 `read` 往返一致 | `write_tmp_then_read_round_trips` |
| 长度不符的 `read` 报错 | `read_rejects_wrong_length` |

## 实际结果

- fmt、clippy（`-D warnings`）、test（83 项）、rustdoc（`-D warnings`）退出码均为 0。
- 最后一次代码改动是把 `Config` 的手写 `Default` 改为派生。之后四项检查全部重跑，结果有效。

## 未覆盖或需注意

- 日志断言：关键失败路径有 `warn!`（schema 不匹配、哈希不符、`api_key` 拒绝），但测试没有用订阅器断言日志内容。AGENT.md 第 4 节要求这一点，待补。
- `WHATCMD_CONFIG` 只通过纯函数 `resolve_config_path` 测试，没有在测试中真正设置环境变量。这样做是为了避免测试间共享进程环境。`load` 读取环境变量的那一行未被直接测试。
- `vectors.bin` 与 `index.json` 之间的崩溃（两次 rename 之间）只能通过哈希校验发现，没有做故障注入测试。这一点留给 D10 的端到端流程覆盖。
- 测试目录位于 `target/test-tmp`，由 `testutil::tempdir` 统一创建。
