# 开发说明

面向后续开发者的简短说明。设计见 `DESIGN.md`，签名见 `INTERFACES.md`，取舍见 `../DEVLOG.md`。

## 常用命令

四项检查，必须逐条直接取退出码，不要用管道接 `tail` 或 `grep`：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

## 模块速查

| 模块 | 作用 | 备注 |
|---|---|---|
| `guard` | 命令名、参数名、描述校验 | 所有外部输入的入口 |
| `docs` | man 与 `--help` 获取 | 通过 `process::run_captured` 运行子进程 |
| `process` | 带超时的子进程运行 | `run_captured`（只要输出）与 `run_finished`（要退出码） |
| `embed::openai` | OpenAI 兼容 embedding 客户端 | 用 `ureq`，关闭默认的状态码错误，自行映射 |
| `agent::claude` | `build_args`、`parse_output`、`ClaudeBackend` | 工具白名单固定在常量中 |
| `agent::detect` | `--help` 能力解析 | 只读选项定义行 |

## 配置与密钥

- embedding 的密钥只从环境变量读取，变量名由 `embedding.api_key_env` 指定，默认 `WHATCMD_EMBEDDING_API_KEY`。
- 配置中出现 `api_key` 字段会直接报错。
- `OpenAiEmbedder` 的 `Debug` 输出不含密钥，错误信息只包含变量名。
- 查询文本会发送给 `embedding.endpoint`。换成第三方服务前要确认这一点。

## 运行真实 embedding 验收

真实调用会把文本发送到 endpoint。运行前确认 endpoint 与模型。

```bash
WHATCMD_EMBEDDING_API_KEY=<你的密钥> cargo run --example embed_check
```

示例程序只打印向量条数与维度，不打印密钥。验收完成后删除 `examples/embed_check.rs`，不要把它提交进仓库。

## 运行 claude 安全验证

每次实验都会产生模型请求，计入额度。

- 每种配置在 `target/test-tmp/claude-safety-*` 下的独立临时目录里运行，提示模型执行一条 `touch` 标记文件的 Bash 命令。实验结束后的目录清理需要用户确认。
- 结果以标记文件是否生成为准，不依赖模型的文字回答。
- `--setting-sources local` 排除用户级设置，避免用户已有的 Bash 放行规则干扰结果。
- `--permission-prompts none` 让需要确认的操作直接被拒绝，不会挂起。
- 阳性对照（`--tools Bash`）必须生成标记，否则说明测试方法本身有问题。

## 已知限制

- `process.rs` 没有独立单元测试，行为通过 `docs` 与 `agent::claude` 的测试间接覆盖。
- 日志断言未覆盖，`tracing-subscriber` 尚未接入。
- `docs::tests::timeout_kills_the_whole_process_group` 有偶发失败记录，根因未定位，见 DEVLOG。
- `parse_output` 的字段结构按 `claude -p --output-format json` 的常见格式编写，以真实输出为准。
