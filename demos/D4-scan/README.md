# D4：PATH 扫描

覆盖模块：`scan`。

只读取目录项与文件元数据，不读文件内容，不调用 agent，不写索引。测试全部使用 `testutil::tempdir()` 在 `target/test-tmp` 下创建的假目录，PATH 字符串由临时目录拼接，不读真实 `PATH`。

## 运行方式

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

只跑本 demo 的测试：

```bash
cargo test scan::
```

## 预期输出

- 四条命令退出码均为 0。
- `cargo test scan::` 共 11 项通过。

## 验收项

| 验收要求 | 对应测试 |
|---|---|
| 同名去重，保留 PATH 中靠前的 | `duplicate_name_keeps_first_in_path` |
| 非可执行文件不遮蔽后面的可执行文件 | `non_executable_does_not_shadow_later_executable` |
| 目录与命令同名时跳过 | `directory_named_like_command_is_skipped` |
| 符号链接指向可执行文件时跟随 | `symlink_to_executable_is_followed` |
| 断开的符号链接跳过 | `broken_symlink_is_skipped` |
| 非法文件名（如 `-x`、`.hidden`、含空格）跳过 | `invalid_names_are_skipped` |
| 相对路径项跳过 | `relative_and_empty_entries_are_skipped` |
| 不存在的目录跳过 | `missing_directory_is_skipped` |
| `find_in_path` 与 `commands_from_path` 结果一致 | `find_in_path_agrees_with_commands_from_path` |

## 实际结果

- fmt、clippy（`-D warnings`）、test、rustdoc（`-D warnings`）退出码均为 0，`cargo test` 共 92 项通过，其中 `scan::tests` 11 项。
- 测试名与断言已逐条核对。

## 已知未覆盖

- 空 PATH 项（如 `::`）与相对项走同一分支，但测试只断言了相对项，没有单独断言空项。
- 相对路径与空项的 `warn` 日志没有测试捕获，见 DEVLOG 2026-11-03 D4 条目。
- 可执行判断只看"任一执行位"，不检查当前用户是否真能执行（例如 root 之外的 `0o100` 文件），与 DESIGN 的规则一致，但不是 `access(X_OK)`。
