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

/// Holds a lock across read, change and write: two sign-ins at once, for different API
/// URLs, would otherwise each write a file without the other's entry.
fn update(
    config_dir: &Path,
    change: impl FnOnce(&mut BTreeMap<String, Client>) -> bool,
) -> Result<()> {
    create_private_dir(config_dir)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(config_dir.join("clients.lock"))?;
    <std::fs::File as fs4::FileExt>::lock(&lock)?;
    let mut clients = read(config_dir);
    if change(&mut clients) {
        write(config_dir, &clients)?;
    }
    Ok(())
}

pub(crate) fn save(config_dir: &Path, api_url: &str, client_id: &str) -> Result<()> {
    update(config_dir, |clients| {
        clients.insert(
            api_url.to_string(),
            Client {
                client_id: client_id.to_string(),
            },
        );
        true
    })
}

pub(crate) fn forget(config_dir: &Path, api_url: &str) -> Result<()> {
    update(config_dir, |clients| clients.remove(api_url).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_ins_at_the_same_time_keep_every_entry() {
        let dir = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for index in 0..16 {
                let dir = dir.path();
                scope.spawn(move || {
                    save(
                        dir,
                        &format!("http://host-{index}"),
                        &format!("client-{index}"),
                    )
                    .unwrap();
                });
            }
        });
        for index in 0..16 {
            assert_eq!(
                load(dir.path(), &format!("http://host-{index}")),
                Some(format!("client-{index}"))
            );
        }
    }

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
