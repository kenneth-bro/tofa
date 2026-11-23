use crate::state::AppState;
use std::io;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};
use tofa_core::mcp_bridge::{self, Account, Action, Operation, Reply, Request};
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
                    handle_request(&locked, request)
                        .unwrap_or_else(|message| Reply::Error(message.into()))
                });
            }
        })?;
    Ok(())
}

fn handle_request(state: &AppState, request: Request) -> Result<Reply, &'static str> {
    if request
        .expected_vault
        .as_ref()
        .is_some_and(|path| path != &state.vault_path)
    {
        return Err("MCP 指定的保险库与 TOFA CN 当前保险库不一致。");
    }
    state
        .cache
        .with_passphrase(|passphrase| {
            if matches!(request.operation, Operation::GetCode { .. })
                && state.mcp_account_ids.is_empty()
            {
                return Err("在 TOFA CN 的设置中勾选本次会话允许 Codex 取码的账号并保存。");
            }
            let vault = Vault::load(&state.vault_path, passphrase)
                .map_err(|_| "无法读取当前 TOFA 保险库。")?;
            match request.operation {
                Operation::GetCode { account } => issue_code(state, &vault, &account),
                Operation::Action(Action::List) => Ok(Reply::Accounts(
                    vault
                        .entries()
                        .iter()
                        .map(|entry| Account {
                            id: entry.id.clone(),
                            name: entry.name.clone(),
                        })
                        .collect(),
                )),
            }
        })
        .ok_or("请先在 TOFA CN 菜单栏解锁保险库。")?
}

fn issue_code(state: &AppState, vault: &Vault, account: &str) -> Result<Reply, &'static str> {
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
            handle_request(
                state,
                Request {
                    operation: Operation::GetCode {
                        account: account.into(),
                    },
                    expected_vault: None,
                },
            )
        }
        fn list(
            state: &AppState,
            expected_vault: Option<std::path::PathBuf>,
        ) -> Result<Reply, &'static str> {
            handle_request(
                state,
                Request {
                    operation: Operation::Action(Action::List),
                    expected_vault,
                },
            )
        }
        assert!(code_for(&state, "enabled").is_err());
        assert!(list(&state, None).is_err());
        state.unlock("fixture-password".into());
        assert!(code_for(&state, "enabled").is_err());
        assert!(matches!(list(&state, None), Ok(Reply::Accounts(accounts)) if accounts.len() == 4));
        state.mcp_account_ids.insert(vault.entries()[0].id.clone());
        state.mcp_account_ids.insert(vault.entries()[2].id.clone());
        assert!(list(&state, Some(temp.path().join("other.enc"))).is_err());
        let listed =
            serde_json::to_value(list(&state, Some(state.vault_path.clone())).unwrap()).unwrap();
        assert_eq!(
            listed,
            serde_json::json!({ "Accounts": [
                { "id": vault.entries()[0].id, "name": "enabled" },
                { "id": vault.entries()[1].id, "name": "disabled" },
                { "id": vault.entries()[2].id, "name": "collision" },
                { "id": vault.entries()[3].id, "name": "collision" }
            ] })
        );
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
        assert!(list(&state, None).is_err());
    }
}
