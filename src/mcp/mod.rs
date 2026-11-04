//! MCP 层。`tools` 放纯函数，不依赖 rmcp；本模块只做 rmcp 注册与参数转发，不含业务逻辑。
//!
//! 服务以 stdio 运行，直到 stdin 关闭。工具只写暂存区，不改索引。

pub mod tools;

use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::schemars;
use rmcp::schemars::JsonSchema;
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;

use tools::{ParamArg, ToolContext, ToolError, WriteDescriptionArgs, WriteParamsArgs};

/// MCP 服务错误。
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// 无法创建异步运行时。
    #[error("runtime: {0}")]
    Runtime(#[source] std::io::Error),
    /// rmcp 初始化失败。
    #[error("server init: {0}")]
    Init(String),
    /// 服务运行时的任务失败。
    #[error("server task: {0}")]
    Task(String),
}

/// 以 stdio 运行 MCP server，直到 stdin 关闭。
///
/// 内部创建仅含一个线程的 tokio 运行时，阻塞调用方直到服务结束。
pub fn serve_stdio(ctx: ToolContext) -> Result<(), McpError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(McpError::Runtime)?;
    runtime.block_on(async {
        let server = WhatcmdServer::new(ctx);
        // stdin 与 stdout 直接作为 AsyncRead + AsyncWrite 交给 rmcp，不依赖 transport-io 特性。
        let io = (tokio::io::stdin(), tokio::io::stdout());
        let service = rmcp::serve_server(server, io)
            .await
            .map_err(|err| McpError::Init(err.to_string()))?;
        service
            .waiting()
            .await
            .map_err(|err| McpError::Task(err.to_string()))?;
        Ok(())
    })
}

/// 工具参数：单个命令名。
#[derive(Debug, Deserialize, JsonSchema)]
pub struct NameArgs {
    /// 根命令名，如 `git`。
    pub name: String,
}

/// 工具参数：命令路径。
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PathArgs {
    /// 子命令路径，如 `["git", "clone"]`。
    pub path: Vec<String>,
}

/// `write_description` 的工具参数。
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DescriptionInput {
    /// 命令路径。
    pub path: Vec<String>,
    /// 描述文本，最多 400 字符。
    pub description: String,
    /// 示例，最多 3 条，每条最多 200 字符。
    pub examples: Vec<String>,
    /// 下一轮要处理的子命令名。
    pub subcommands: Vec<String>,
}

/// `write_params` 的工具参数。
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ParamsInput {
    /// 所属命令路径。
    pub path: Vec<String>,
    /// 长参数列表。
    pub params: Vec<ParamInput>,
}

/// 单个长参数的工具参数。
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ParamInput {
    /// 长参数名，如 `--branch`。
    pub flag: String,
    /// 参数描述。
    pub description: String,
}

impl From<DescriptionInput> for WriteDescriptionArgs {
    fn from(input: DescriptionInput) -> Self {
        Self {
            path: input.path,
            description: input.description,
            examples: input.examples,
            subcommands: input.subcommands,
        }
    }
}

impl From<ParamsInput> for WriteParamsArgs {
    fn from(input: ParamsInput) -> Self {
        Self {
            path: input.path,
            params: input
                .params
                .into_iter()
                .map(|p| ParamArg {
                    flag: p.flag,
                    description: p.description,
                })
                .collect(),
        }
    }
}

/// 把工具错误映射为 MCP 错误。输入问题为 `invalid_params`，文档或暂存区故障为 `internal_error`。
fn to_error(err: ToolError) -> ErrorData {
    match err {
        ToolError::Doc(_) | ToolError::Staging(_) => {
            ErrorData::internal_error(err.to_string(), None)
        }
        _ => ErrorData::invalid_params(err.to_string(), None),
    }
}

/// rmcp 服务实现。每个工具方法只做参数转发。
#[derive(Clone)]
pub struct WhatcmdServer {
    /// 本次 run 的上下文，只读。工具写入的是磁盘上的暂存区。
    ctx: Arc<ToolContext>,
}

impl WhatcmdServer {
    /// 用一次 run 的上下文创建服务。
    pub fn new(ctx: ToolContext) -> Self {
        Self { ctx: Arc::new(ctx) }
    }
}

#[tool_router]
impl WhatcmdServer {
    /// 读取根命令的 man 页纯文本。
    #[tool(
        name = "read_man",
        description = "Return the man page text of a root command"
    )]
    fn read_man(&self, Parameters(args): Parameters<NameArgs>) -> Result<String, ErrorData> {
        tools::read_man(&args.name, &self.ctx).map_err(to_error)
    }

    /// 读取子命令的 `--help` 输出。
    #[tool(
        name = "read_help",
        description = "Return the --help output of a command path such as [\"git\", \"clone\"]"
    )]
    fn read_help(&self, Parameters(args): Parameters<PathArgs>) -> Result<String, ErrorData> {
        let path: Vec<&str> = args.path.iter().map(String::as_str).collect();
        tools::read_help(&path, &self.ctx).map_err(to_error)
    }

    /// 写入命令节点的描述、示例与子命令。
    #[tool(
        name = "write_description",
        description = "Stage a description, examples and subcommands for a command node"
    )]
    fn write_description(
        &self,
        Parameters(input): Parameters<DescriptionInput>,
    ) -> Result<String, ErrorData> {
        tools::write_description(input.into(), &self.ctx).map_err(to_error)?;
        Ok("staged".to_owned())
    }

    /// 写入命令下长参数的描述。
    #[tool(
        name = "write_params",
        description = "Stage descriptions for long flags of a command node"
    )]
    fn write_params(
        &self,
        Parameters(input): Parameters<ParamsInput>,
    ) -> Result<String, ErrorData> {
        tools::write_params(input.into(), &self.ctx).map_err(to_error)?;
        Ok("staged".to_owned())
    }
}

/// 服务信息。`serverInfo.name` 为 `whatcmd`，版本与 crate 版本一致。
#[tool_handler(name = "whatcmd", version = "0.1.0")]
impl ServerHandler for WhatcmdServer {}
