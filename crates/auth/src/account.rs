//! Account model + on-disk store.

use anyhow::{Context, Result};
use pcl_core::uuid::offline_uuid_undashed;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Offline,
    Microsoft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub kind: AccountKind,
    pub username: String,
    /// Undashed 32-char UUID, the form used in `${auth_uuid}`.
    pub uuid: String,
    pub access_token: String,
    /// `msa` for Microsoft accounts, `legacy` otherwise.
    pub user_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xuid: Option<String>,
    /// Unix timestamp (seconds) when `access_token` stops working.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

impl Account {
    pub fn offline(username: &str) -> Self {
        Self {
            kind: AccountKind::Offline,
            username: username.to_string(),
            uuid: offline_uuid_undashed(username),
            access_token: "0".to_string(),
            user_type: "legacy".to_string(),
            xuid: None,
            expires_at: None,
            refresh_token: None,
        }
    }

    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            None => false,
            Some(at) => now_secs() + 60 >= at,
        }
    }

    pub fn is_offline(&self) -> bool {
        self.kind == AccountKind::Offline
    }

    /// `user_properties` argument value; vanilla sends an empty object.
    pub fn user_properties(&self) -> &'static str {
        "{}"
    }

    pub fn client_id(&self) -> &'static str {
        "0"
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// JSON file holding several accounts plus the currently selected one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccountStore {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub selected: Option<String>,
}

impl AccountStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading account store {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing account store {}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
            .with_context(|| format!("writing account store {}", path.display()))
    }

    pub fn upsert(&mut self, account: Account) {
        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|a| a.uuid.eq_ignore_ascii_case(&account.uuid))
        {
            *existing = account.clone();
        } else {
            self.accounts.push(account.clone());
        }
        self.selected = Some(account.uuid);
    }

    pub fn selected_account(&self) -> Option<&Account> {
        let selected = self.selected.as_ref()?;
        self.accounts
            .iter()
            .find(|a| a.uuid.eq_ignore_ascii_case(selected))
    }

    /// Default location: `<root>/pcl-rs/accounts.json`.
    pub fn default_path(game_root: &Path) -> PathBuf {
        game_root.join("pcl-rs").join("accounts.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_account_shape() {
        let a = Account::offline("Steve");
        assert_eq!(a.uuid.len(), 32);
        assert_eq!(a.user_type, "legacy");
        assert_eq!(a.access_token, "0");
        assert!(a.is_offline());
        assert!(!a.is_expired());
    }

    #[test]
    fn store_roundtrip() {
        let dir = std::env::temp_dir().join("pcl-rs-auth-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("accounts.json");

        let mut store = AccountStore::default();
        store.upsert(Account::offline("Alex"));
        store.save(&path).unwrap();

        let loaded = AccountStore::load(&path).unwrap();
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.selected_account().unwrap().username, "Alex");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
