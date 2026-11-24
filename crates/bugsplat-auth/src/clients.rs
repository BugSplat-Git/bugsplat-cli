//! The client id `login` registered with each API URL, so later sign-ins reuse it.
//! It identifies a public client and is not a secret.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bugsplat_core::Result;
use serde::{Deserialize, Serialize};

use crate::store::{create_private_dir, write_private};

#[derive(Serialize, Deserialize)]
struct Client {
    client_id: String,
}

fn path(config_dir: &Path) -> PathBuf {
    config_dir.join("clients.json")
}

fn read(config_dir: &Path) -> BTreeMap<String, Client> {
    std::fs::read(path(config_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write(config_dir: &Path, clients: &BTreeMap<String, Client>) -> Result<()> {
    create_private_dir(config_dir)?;
    write_private(&path(config_dir), &serde_json::to_vec_pretty(clients)?)?;
    Ok(())
}

pub(crate) fn load(config_dir: &Path, api_url: &str) -> Option<String> {
    read(config_dir).remove(api_url).map(|c| c.client_id)
}

pub(crate) fn save(config_dir: &Path, api_url: &str, client_id: &str) -> Result<()> {
    let mut clients = read(config_dir);
    clients.insert(
        api_url.to_string(),
        Client {
            client_id: client_id.to_string(),
        },
    );
    write(config_dir, &clients)
}

pub(crate) fn forget(config_dir: &Path, api_url: &str) -> Result<()> {
    let mut clients = read(config_dir);
    if clients.remove(api_url).is_some() {
        write(config_dir, &clients)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_one_client_per_api_url() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path(), "http://a"), None);

        save(dir.path(), "http://a", "client-a").unwrap();
        save(dir.path(), "http://b", "client-b").unwrap();
        assert_eq!(load(dir.path(), "http://a").as_deref(), Some("client-a"));
        assert_eq!(load(dir.path(), "http://b").as_deref(), Some("client-b"));

        forget(dir.path(), "http://a").unwrap();
        forget(dir.path(), "http://a").unwrap();
        assert_eq!(load(dir.path(), "http://a"), None);
        assert_eq!(load(dir.path(), "http://b").as_deref(), Some("client-b"));
    }
}
