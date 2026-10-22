//! Accounts and login.
//!
//! * [`Account::offline`] — the classic offline profile, UUID derived exactly like
//!   the vanilla launcher so worlds keep working.
//! * [`microsoft`] — Microsoft → Xbox Live → XSTS → Minecraft Services via the
//!   OAuth 2.0 device-code flow (no redirect URI, works from GUI or terminal).

pub mod account;
pub mod microsoft;

pub use account::{Account, AccountKind, AccountStore};
pub use microsoft::{DeviceCodeFlow, MicrosoftAuth, MsToken};
