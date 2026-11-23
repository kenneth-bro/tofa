use crate::state::AppState;
use std::io;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};
use tofa_core::mcp_bridge::{self, Reply, Request};
use tofa_core::{generate_code_at, seconds_remaining, Vault};

pub fn start(app: AppHandle) -> io::Result<()> {
    let config = dirs::config_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no TOFA config directory"))?;
    let (listener, guard) = mcp_bridge::listen(&config)?;
    std::thread::Builder::new()
        .name("tofa-mcp".into())
        .spawn(move || {
            let _guard = guard;
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let _ = mcp_bridge::respond(&stream, |request| {
                    let state = app.state::<Mutex<AppState>>();
                    let Ok(locked) = state.lock() else {
                        return Reply::Error("无法读取 TOFA 桌面会话。".into());
                    };
                    issue_code(&locked, request)
                        .unwrap_or_else(|message| Reply::Error(message.into()))
                });
            }
        })?;
    Ok(())
}

fn issue_code(state: &AppState, request: Request) -> Result<Reply, &'static str> {
    if request
        .expected_vault
        .as_ref()
        .is_some_and(|path| path != &state.vault_path)
    {
        return Err("MCP 指定的保险库与 TOFA CN 当前保险库不一致。");
    }
    let vault = state
        .cache
        .with_passphrase(|passphrase| {
            if state.mcp_account_ids.is_empty() {
                return Err("在 TOFA CN 的设置中勾选本次会话允许 Codex 取码的账号并保存。");
            }
            Vault::load(&state.vault_path, passphrase).map_err(|_| "无法读取当前 TOFA 保险库。")
        })
        .ok_or("请先在 TOFA CN 菜单栏解锁保险库。")??;
    let account = request.account;
    let denied = "该账号未在本次会话授权，或账号名称有重名。请核对授权和完整账号 ID。";
    // Resolve an exact ID or an unambiguous full name before checking the selected ID.
    let mut matches = vault
        .entries()
        .iter()
        .filter(|e| e.id == account || e.name == account);
    let entry = matches.next().ok_or(denied)?;
    if matches.next().is_some() || !state.mcp_account_ids.contains(&entry.id) {
        return Err(denied);
    }
    if entry.period == 0 || !(6..=8).contains(&entry.digits) {
        return Err("TOFA 账号的验证码参数无效。");
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "本机时钟无效。")?
        .as_secs();
    let code = generate_code_at(entry, now).map_err(|_| "无法生成 TOFA 验证码。")?;
    Ok(Reply::Code {
        code,
        expires_in_seconds: seconds_remaining(entry, now),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PassphraseCache;
    use tofa_core::VaultEntry;

    #[test]
    fn reuses_the_unlocked_session_only_for_the_selected_accounts() {
        let temp = tempfile::Builder::new()
            .prefix("temp-tofa-mcp-")
            .tempdir()
            .unwrap();
        let path = temp.path().join("test.enc");
        let mut vault = Vault::new();
        for name in ["enabled", "disabled", "collision", "collision"] {
            vault.add_entry(VaultEntry::new(
                name.into(),
                "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ".into(),
                "2026-01-01".into(),
            ));
        }
        vault.save(&path, "fixture-password").unwrap();
        let mut state = AppState {
            cache: PassphraseCache::default(),
            vault_path: path,
            mcp_account_ids: Default::default(),
        };
        fn code_for(state: &AppState, account: &str) -> Result<Reply, &'static str> {
            issue_code(
                state,
                Request {
                    account: account.into(),
                    expected_vault: None,
                },
            )
        }
        assert!(code_for(&state, "enabled").is_err());
        state.unlock("fixture-password".into());
        assert!(code_for(&state, "enabled").is_err());
        state.mcp_account_ids.insert(vault.entries()[0].id.clone());
        state.mcp_account_ids.insert(vault.entries()[2].id.clone());
        for _ in 0..2 {
            let Reply::Code {
                code,
                expires_in_seconds,
            } = code_for(&state, "enabled").unwrap()
            else {
                panic!("expected a TOTP result");
            };
            assert_eq!(code.len(), 6);
            assert!((1..=30).contains(&expires_in_seconds));
        }
        assert!(code_for(&state, "disabled").is_err());
        assert!(code_for(&state, "collision").is_err());
        assert!(code_for(&state, &vault.entries()[2].id).is_ok());
        state.mcp_account_ids.clear();
        assert!(code_for(&state, "enabled").is_err());
        state.mcp_account_ids.insert(vault.entries()[0].id.clone());
        state.select_vault(temp.path().join("other.enc"));
        assert!(code_for(&state, "enabled").is_err());
    }
}
