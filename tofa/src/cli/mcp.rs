use super::CliResult;
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::json;
use std::io::ErrorKind;
use std::path::PathBuf;
use tofa_core::mcp_bridge::{self, Action, Operation, Reply, Request};

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetCodeRequest {
    /// Exact TOFA account name or ID authorized for AI in the TOFA CN desktop app.
    account: String,
}

#[derive(Clone)]
struct TofaServer {
    config_dir: PathBuf,
    expected_vault: Option<PathBuf>,
    router: ToolRouter<Self>,
}

#[tool_handler(router = self.router)]
impl ServerHandler for TofaServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Use list to identify accounts in the current unlocked TOFA CN vault. Use get_code only for the exact TOFA account requested by the user. Use the code only for the requested login.")
    }
}

#[tool_router(router = router)]
impl TofaServer {
    #[tool(
        description = "Get the current TOTP for one exact TOFA account approved in the unlocked TOFA CN desktop session."
    )]
    async fn get_code(&self, Parameters(args): Parameters<GetCodeRequest>) -> CallToolResult {
        if args.account.trim().is_empty()
            || args.account.chars().count() > 256
            || args.account.chars().any(char::is_control)
        {
            return tool_error("请提供有效的 TOFA 账号全名或 ID。");
        }
        match self
            .request(Operation::GetCode {
                account: args.account,
            })
            .await
        {
            Ok(Reply::Code {
                code,
                expires_in_seconds,
            }) => CallToolResult::structured(
                json!({ "code": code, "expires_in_seconds": expires_in_seconds }),
            ),
            Ok(_) => tool_error("TOFA CN 桌面会话返回了非预期的响应。"),
            Err(message) => tool_error(message),
        }
    }

    #[tool(
        description = "List the ID and name of every account in the current unlocked TOFA CN vault.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list(&self) -> CallToolResult {
        match self.request(Operation::Action(Action::List)).await {
            Ok(Reply::Accounts(accounts)) => {
                CallToolResult::structured(json!({ "accounts": accounts }))
            }
            Ok(_) => tool_error("TOFA CN 桌面会话返回了非预期的响应。"),
            Err(message) => tool_error(message),
        }
    }
}

impl TofaServer {
    async fn request(&self, operation: Operation) -> Result<Reply, String> {
        let config = self.config_dir.clone();
        let request = Request {
            operation,
            expected_vault: self.expected_vault.clone(),
        };
        match tokio::task::spawn_blocking(move || mcp_bridge::request(&config, &request)).await {
            Ok(Ok(Reply::Error(message))) => Err(message),
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(error))
                if matches!(
                    error.kind(),
                    ErrorKind::NotFound | ErrorKind::ConnectionRefused
                ) =>
            {
                Err("请启动支持 MCP 的 TOFA CN 桌面版并解锁保险库。".into())
            }
            _ => Err("无法连接 TOFA CN 桌面会话。".into()),
        }
    }
}

fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.into())])
}

pub(super) fn run(expected_vault: Option<PathBuf>) -> CliResult {
    let server = TofaServer {
        config_dir: dirs::config_dir().ok_or("no TOFA config directory")?,
        expected_vault,
        router: TofaServer::router(),
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            server
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
            Ok(())
        })
}
