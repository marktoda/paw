//! The address field of setup, read as the person types.

use crate::sync::address::StoreAddress;

/// An address as paw read it, for the line under the field: "paw will
/// connect to github.com as git".
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AddressReading {
    /// What paw saves and shows, e.g. `git@github.com:you/store.git`.
    pub address: String,
    /// The user paw logs in as, e.g. `git`.
    pub user: String,
    /// The host paw connects to, without brackets, e.g. `github.com`.
    pub host: String,
    pub port: u16,
    /// Set when paw changed what was typed (a GitHub short form or web
    /// address), saying how.
    pub notice: Option<String>,
    /// paw ships this host's keys, so there is no fingerprint to confirm.
    pub host_key_bundled: bool,
}

/// Reads an address as `Setup.set_address` would, without changing
/// anything: for showing where paw will connect while the person types.
/// The error is `SetupError::BadAddress` with what to do instead.
#[uniffi::export]
pub fn read_store_address(text: String) -> Result<AddressReading, crate::setup::SetupError> {
    let parsed = StoreAddress::parse(&text).map_err(|message| crate::setup::SetupError::BadAddress { message })?;
    let address = parsed.address;
    Ok(AddressReading {
        address: address.display(),
        user: address.user.clone(),
        host: address.connect_host().to_owned(),
        port: address.port,
        notice: parsed.notice,
        host_key_bundled: crate::sync::hosts::is_bundled(address.connect_host(), address.port),
    })
}
