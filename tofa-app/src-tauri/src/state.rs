use std::collections::BTreeSet;
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Default)]
pub struct PassphraseCache(Option<Zeroizing<String>>);

impl PassphraseCache {
    pub fn unlock(&mut self, passphrase: String) {
        self.0 = Some(Zeroizing::new(passphrase));
    }

    pub fn with_passphrase<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&str) -> R,
    {
        self.0.as_ref().map(|passphrase| f(passphrase.as_str()))
    }
}

pub struct AppState {
    pub cache: PassphraseCache,
    pub vault_path: PathBuf,
    pub mcp_account_ids: BTreeSet<String>,
}

impl AppState {
    pub fn new() -> Self {
        let vault_path = std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v["vault_path"].as_str().map(PathBuf::from))
            .unwrap_or_else(default_vault_path);
        Self {
            cache: PassphraseCache::default(),
            mcp_account_ids: load_approved(&vault_path),
            vault_path,
        }
    }

    pub fn unlock(&mut self, passphrase: String) {
        self.cache.unlock(passphrase);
    }

    // 切换保险库时不沿用旧库口令；只修改外观时保留会话。
    pub fn select_vault(&mut self, path: PathBuf) -> bool {
        if path == self.vault_path {
            return false;
        }
        self.mcp_account_ids = load_approved(&path);
        self.vault_path = path;
        self.cache = PassphraseCache::default();
        true
    }
}

#[must_use]
pub fn default_vault_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tofa")
        .join("vault.enc")
}

// AI 授权的账号 ID 持久化到独立文件，只存 ID 和保险库路径，不含密钥。
fn approved_path() -> PathBuf {
    settings_path().with_file_name("tofa-ai-approved.json")
}

#[must_use]
pub fn load_approved(vault: &std::path::Path) -> BTreeSet<String> {
    let v: Option<serde_json::Value> = std::fs::read_to_string(approved_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    match v {
        Some(v) if v["vault_path"].as_str().map(PathBuf::from).as_deref() == Some(vault) => v
            ["account_ids"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        _ => BTreeSet::new(),
    }
}

pub fn save_approved(vault: &std::path::Path, ids: &BTreeSet<String>) -> std::io::Result<()> {
    let body = serde_json::json!({"vault_path": vault, "account_ids": ids});
    let path = approved_path();
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&body)?)
}

#[must_use]
pub fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tofa")
        .join("tofa-app-settings.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_starts_locked_and_stays_open_after_unlock() {
        let mut cache = PassphraseCache::default();
        assert!(!cache.with_passphrase(|_| true).unwrap_or(false));
        cache.unlock(String::from("fixture-password"));
        for _ in 0..2 {
            assert_eq!(cache.with_passphrase(str::len), Some(16));
        }
        assert!(PassphraseCache::default().with_passphrase(|_| ()).is_none());
    }

    #[test]
    fn only_switching_vaults_clears_the_current_session() {
        let mut state = AppState {
            vault_path: "a.enc".into(),
            cache: PassphraseCache::default(),
            mcp_account_ids: BTreeSet::new(),
        };
        state.unlock(String::from("fixture-password"));
        state.mcp_account_ids.insert("test-id".into());
        assert!(!state.select_vault("a.enc".into()));
        assert!(state.cache.with_passphrase(|_| ()).is_some());
        assert!(state.mcp_account_ids.contains("test-id"));
        assert!(state.select_vault("b.enc".into()));
        assert!(state.cache.with_passphrase(|_| ()).is_none());
        assert!(state.mcp_account_ids.is_empty());
    }

    #[test]
    fn approved_ids_survive_restart_and_follow_the_vault() {
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", home.path());
        let vault = default_vault_path();
        let ids: BTreeSet<String> = ["a".into(), "b".into()].into();
        save_approved(&vault, &ids).unwrap();
        // 模拟重启：重新构造状态，授权应还在，但口令缓存为空。
        let state = AppState::new();
        assert_eq!(state.mcp_account_ids, ids);
        assert!(state.cache.with_passphrase(|_| ()).is_none());
        // 其他保险库不沿用授权。
        assert!(load_approved(std::path::Path::new("other.enc")).is_empty());
    }
}
