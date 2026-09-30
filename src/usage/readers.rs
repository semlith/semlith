//! One reader per supported client.

use super::Log;
use anyhow::Result;

/// The semlith calls in `client`'s logs changed since `since`.
pub fn read(client: &str, _since: i64) -> Result<Log> {
    Ok(Log::NotRecorded(format!("{client} is not read yet")))
}

/// Where each client's reader looks, on this machine.
pub fn paths() -> Vec<(&'static str, Vec<std::path::PathBuf>)> {
    Vec::new()
}
