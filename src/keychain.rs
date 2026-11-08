use anyhow::Result;

const SERVICE: &str = "calendar-autoblock";

pub fn save_refresh_token(account: &str, token: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, account)?.set_password(token)?;
    Ok(())
}

pub fn load_refresh_token(account: &str) -> Result<Option<String>> {
    match keyring::Entry::new(SERVICE, account)?.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
