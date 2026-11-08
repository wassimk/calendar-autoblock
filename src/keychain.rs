use anyhow::Result;
use keyring_core::{Entry, Error};

const SERVICE: &str = "calendar-autoblock";

/// Registers the macOS Keychain as the credential store; call once at startup
pub fn init() -> Result<()> {
    #[cfg(target_os = "macos")]
    keyring_core::set_default_store(apple_native_keyring_store::keychain::Store::new()?);
    Ok(())
}

pub fn save_refresh_token(account: &str, token: &str) -> Result<()> {
    Entry::new(SERVICE, account)?.set_password(token)?;
    Ok(())
}

pub fn load_refresh_token(account: &str) -> Result<Option<String>> {
    match Entry::new(SERVICE, account)?.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
