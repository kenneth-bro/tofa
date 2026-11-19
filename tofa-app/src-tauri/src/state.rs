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
            vault_path,
        }
    }

    // 切换保险库时不沿用旧库口令；只修改外观时保留会话。
    pub fn select_vault(&mut self, path: PathBuf) -> bool {
        if path == self.vault_path {
            return false;
        }
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
        };
        state.cache.unlock(String::from("fixture-password"));
        assert!(!state.select_vault("a.enc".into()));
        assert!(state.cache.with_passphrase(|_| ()).is_some());
        assert!(state.select_vault("b.enc".into()));
        assert!(state.cache.with_passphrase(|_| ()).is_none());
    }
}
