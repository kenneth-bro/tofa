use super::{default_vault_path, CliResult};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tofa_core::totp::{generate_code_at, seconds_remaining};
use tofa_core::Vault;
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

const UNLOCK_DIALOG: &str = r#"
on run argv
    set accountName to item 1 of argv
    set response to display dialog ("Codex 请求获取「" & accountName & "」的验证码。输入 TOFA 口令以授权本次取码。") with title "TOFA 取码授权" default answer "" with hidden answer buttons {"取消", "授权取码"} default button "授权取码" cancel button "取消" giving up after 45
    if gave up of response then error number -128
    return text returned of response
end run
"#;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetCodeRequest {
    /// Exact TOFA account name or account ID. A local dialog authorizes each request.
    account: String,
}

#[derive(Clone)]
struct TofaServer {
    vault_path: PathBuf,
    approval: Arc<Semaphore>,
    router: ToolRouter<Self>,
}

#[tool_handler(router = self.router)]
impl ServerHandler for TofaServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Use get_code only for the exact TOFA account requested by the user. The code is short-lived and must be used only for the approved login.")
    }
}

#[tool_router(router = router)]
impl TofaServer {
    #[tool(
        description = "Get the current TOTP for one exact TOFA account after local user authorization."
    )]
    async fn get_code(&self, Parameters(request): Parameters<GetCodeRequest>) -> CallToolResult {
        let Ok(permit) = self.approval.clone().try_acquire_owned() else {
            return tool_error("已有 TOFA 取码授权请求正在等待处理。");
        };
        let path = self.vault_path.clone();
        match tokio::task::spawn_blocking(move || {
            let _permit = permit;
            issue_code(&path, &request.account, prompt_for_passphrase)
        })
        .await
        {
            Ok(Ok((code, seconds))) => {
                CallToolResult::structured(json!({"code": code, "expires_in_seconds": seconds}))
            }
            Ok(Err(message)) => tool_error(message),
            Err(_) => tool_error("TOFA 取码任务执行失败。"),
        }
    }
}

fn tool_error(message: &'static str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

pub(super) fn run(vault_flag: Option<PathBuf>) -> CliResult {
    let server = TofaServer {
        vault_path: match vault_flag {
            Some(path) => path,
            None => desktop_vault_path()?,
        },
        approval: Arc::new(Semaphore::new(1)),
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

#[derive(Deserialize)]
struct DesktopSettings {
    vault_path: Option<PathBuf>,
}

fn desktop_vault_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let default_path = default_vault_path();
    let settings_path = default_path.with_file_name("tofa-app-settings.json");
    match std::fs::read(settings_path) {
        Ok(bytes) => Ok(serde_json::from_slice::<DesktopSettings>(&bytes)?
            .vault_path
            .unwrap_or(default_path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(default_path),
        Err(error) => Err(error.into()),
    }
}

fn prompt_for_passphrase(account: &str) -> Result<Zeroizing<String>, &'static str> {
    // 账号作为参数传入，MCP 标准输入保持用于协议通信。
    let output = Command::new("/usr/bin/osascript")
        .args(["-e", UNLOCK_DIALOG, "--", account])
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "无法打开 TOFA 授权窗口。")?;
    let answer = Zeroizing::new(output.stdout);
    if !output.status.success() {
        return Err("TOFA 授权被取消、超时或未能完成。");
    }
    let answer = std::str::from_utf8(&answer).map_err(|_| "无法读取 TOFA 授权窗口的输入。")?;
    let answer = answer.strip_suffix('\n').unwrap_or(answer);
    if answer.is_empty() {
        return Err("TOFA 口令不能为空。");
    }
    Ok(Zeroizing::new(answer.to_owned()))
}

fn issue_code(
    path: &Path,
    account: &str,
    unlock: impl FnOnce(&str) -> Result<Zeroizing<String>, &'static str>,
) -> Result<(String, u64), &'static str> {
    if account.trim().is_empty()
        || account.chars().count() > 256
        || account.chars().any(|c| {
            c.is_control()
                || matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
        })
    {
        return Err("请提供有效的 TOFA 账号全名或 ID。");
    }
    if !path.is_file() {
        return Err("找不到 TOFA 保险库。");
    }
    let passphrase = unlock(account)?;
    let vault = Vault::load(path, &passphrase).map_err(|_| "无法解锁 TOFA 保险库。")?;
    drop(passphrase);

    let mut matches = vault
        .entries()
        .iter()
        .filter(|entry| entry.name == account || (!entry.id.is_empty() && entry.id == account));
    let entry = matches.next().ok_or("没有完全匹配的 TOFA 账号。")?;
    if matches.next().is_some() {
        return Err("TOFA 账号名有重复，请使用完整账号 ID。");
    }
    if entry.period == 0 || !(6..=8).contains(&entry.digits) {
        return Err("TOFA 账号的验证码参数无效。");
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "本机时钟无效。")?
        .as_secs();
    let code = generate_code_at(entry, now).map_err(|_| "无法生成 TOFA 验证码。")?;
    Ok((code, seconds_remaining(entry, now)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tofa_core::VaultEntry;

    const TEST_PASSWORD: &str = "mcp-fixture-password";
    const TEST_SECRET: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    #[test]
    fn issues_only_an_exact_approved_code() {
        let temp = tempfile::Builder::new()
            .prefix("temp-tofa-mcp-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("fixture.enc");
        let mut vault = Vault::new();
        for name in [
            "test-account",
            "test-account-backup",
            "duplicate",
            "duplicate",
        ] {
            vault.add_entry(VaultEntry::new(
                name.into(),
                TEST_SECRET.into(),
                "2026-01-01".into(),
            ));
        }
        vault.save(&path, TEST_PASSWORD).unwrap();
        let unlock = |_: &str| Ok(Zeroizing::new(TEST_PASSWORD.to_owned()));

        let (code, expires) = issue_code(&path, "test-account", |account| {
            assert_eq!(account, "test-account");
            Ok(Zeroizing::new(TEST_PASSWORD.to_owned()))
        })
        .unwrap();
        assert_eq!(code.len(), 6);
        assert!(code.bytes().all(|byte| byte.is_ascii_digit()));
        assert!((1..=30).contains(&expires));

        assert!(issue_code(&path, "test-acc", unlock).is_err());
        assert!(issue_code(&path, "duplicate", unlock).is_err());
        assert!(issue_code(&path, "test-account", |_| Err("cancelled")).is_err());
        assert!(issue_code(&path, "test-account", |_| Ok(Zeroizing::new(
            "wrong".into()
        )))
        .is_err());
        assert!(issue_code(&path, "test\naccount", |_| panic!(
            "invalid request opened approval"
        ))
        .is_err());
    }
}
