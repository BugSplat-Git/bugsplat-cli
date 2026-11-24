//! Where a session is kept: the OS keychain, or a `0600` file under the config directory.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bugsplat_core::{Error, Result};

use crate::session::Session;

/// `file` keeps sessions in a file under the config directory and never touches the
/// keychain; `keychain` uses only the keychain; `auto` tries the keychain and falls back
/// to the file. Unset means `auto` in release builds and `file` in debug builds.
pub const TOKEN_STORE_ENV: &str = "BUGSPLAT_TOKEN_STORE";

const KEYCHAIN_SERVICE: &str = "bugsplat-cli";

pub(crate) trait TokenStore: Send + Sync {
    fn load(&self, api_url: &str) -> Result<Option<Session>>;
    fn save(&self, api_url: &str, session: &Session) -> Result<()>;
    fn delete(&self, api_url: &str) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreChoice {
    Auto,
    File,
    Keychain,
}

impl StoreChoice {
    pub(crate) fn parse(value: Option<&str>, debug_build: bool) -> Result<Self> {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            // macOS ties a keychain item to the binary that wrote it, so every rebuild of
            // a development binary would prompt for the login password.
            None if debug_build => Ok(Self::File),
            None | Some("auto") => Ok(Self::Auto),
            Some("file") => Ok(Self::File),
            Some("keychain") => Ok(Self::Keychain),
            Some(other) => Err(Error::Usage(format!(
                "Invalid {TOKEN_STORE_ENV} `{other}`: expected auto, keychain or file"
            ))),
        }
    }
}

pub(crate) fn open(config_dir: &Path, choice: StoreChoice, quiet: bool) -> Arc<dyn TokenStore> {
    match choice {
        StoreChoice::File => Arc::new(FileStore::new(config_dir)),
        StoreChoice::Keychain => Arc::new(KeychainStore),
        StoreChoice::Auto => Arc::new(FallbackStore {
            primary: Box::new(KeychainStore),
            file: FileStore::new(config_dir),
            quiet,
        }),
    }
}

pub(crate) fn sessions_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("sessions")
}

pub(crate) fn file_stem(api_url: &str) -> String {
    api_url
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Writes through a temporary file so a reader never sees half a session.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)
}

pub(crate) struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub(crate) fn new(config_dir: &Path) -> Self {
        Self {
            dir: sessions_dir(config_dir),
        }
    }

    pub(crate) fn path(&self, api_url: &str) -> PathBuf {
        self.dir.join(format!("{}.json", file_stem(api_url)))
    }
}

impl TokenStore for FileStore {
    fn load(&self, api_url: &str) -> Result<Option<Session>> {
        match fs::read(self.path(api_url)) {
            // A session that can't be read is treated as none, so `login` can replace it.
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn save(&self, api_url: &str, session: &Session) -> Result<()> {
        create_private_dir(&self.dir)?;
        write_private(&self.path(api_url), &serde_json::to_vec(session)?)?;
        Ok(())
    }

    fn delete(&self, api_url: &str) -> Result<()> {
        match fs::remove_file(self.path(api_url)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        }
    }
}

/// One keychain item per API URL, holding the session as JSON.
pub(crate) struct KeychainStore;

impl KeychainStore {
    fn entry(api_url: &str) -> Result<keyring::Entry> {
        // Unit tests must never read or write the developer's keychain.
        if cfg!(test) {
            return Err(Error::Other(
                "The keychain is off limits in tests".to_string(),
            ));
        }
        keyring::Entry::new(KEYCHAIN_SERVICE, api_url).map_err(keychain_error)
    }
}

// `Display` only: the `Debug` form of some keyring errors carries the stored bytes.
fn keychain_error(err: keyring::Error) -> Error {
    Error::Other(format!("Keychain: {err}"))
}

impl TokenStore for KeychainStore {
    fn load(&self, api_url: &str) -> Result<Option<Session>> {
        match Self::entry(api_url)?.get_secret() {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(keychain_error(err)),
        }
    }

    fn save(&self, api_url: &str, session: &Session) -> Result<()> {
        // Bytes, not a string: Windows stores strings as UTF-16, which would halve the
        // 2560 bytes a credential can hold, and an access token alone is about 1100.
        Self::entry(api_url)?
            .set_secret(&serde_json::to_vec(session)?)
            .map_err(keychain_error)
    }

    fn delete(&self, api_url: &str) -> Result<()> {
        match Self::entry(api_url)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(keychain_error(err)),
        }
    }
}

/// The keychain when it works, otherwise the file. The file exists only while the last
/// save went to it, so it is the newer copy whenever it is there.
pub(crate) struct FallbackStore {
    primary: Box<dyn TokenStore>,
    file: FileStore,
    quiet: bool,
}

impl TokenStore for FallbackStore {
    fn load(&self, api_url: &str) -> Result<Option<Session>> {
        if let Some(session) = self.file.load(api_url)? {
            return Ok(Some(session));
        }
        Ok(self.primary.load(api_url).unwrap_or(None))
    }

    fn save(&self, api_url: &str, session: &Session) -> Result<()> {
        if self.primary.save(api_url, session).is_ok() {
            return self.file.delete(api_url);
        }
        let path = self.file.path(api_url);
        let first_use = !path.exists();
        self.file.save(api_url, session)?;
        if first_use && !self.quiet {
            eprintln!(
                "note: no system keychain is available, so the session is stored in {} (readable only by you).",
                path.display()
            );
        }
        Ok(())
    }

    fn delete(&self, api_url: &str) -> Result<()> {
        self.file.delete(api_url)?;
        match self.primary.delete(api_url) {
            // Without a keychain there is nothing left to delete; with one, a session
            // that is still there means the sign-out did not happen.
            Err(err) if matches!(self.primary.load(api_url), Ok(Some(_))) => Err(err),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    pub(crate) struct MemoryStore {
        sessions: Mutex<HashMap<String, Session>>,
    }

    impl TokenStore for MemoryStore {
        fn load(&self, api_url: &str) -> Result<Option<Session>> {
            Ok(self.sessions.lock().unwrap().get(api_url).cloned())
        }

        fn save(&self, api_url: &str, session: &Session) -> Result<()> {
            self.sessions
                .lock()
                .unwrap()
                .insert(api_url.to_string(), session.clone());
            Ok(())
        }

        fn delete(&self, api_url: &str) -> Result<()> {
            self.sessions.lock().unwrap().remove(api_url);
            Ok(())
        }
    }

    /// A keychain that isn't there, as on a headless Linux machine.
    pub(crate) struct UnavailableStore;

    impl TokenStore for UnavailableStore {
        fn load(&self, _: &str) -> Result<Option<Session>> {
            Err(Error::Other("no keychain".into()))
        }

        fn save(&self, _: &str, _: &Session) -> Result<()> {
            Err(Error::Other("no keychain".into()))
        }

        fn delete(&self, _: &str) -> Result<()> {
            Err(Error::Other("no keychain".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{MemoryStore, UnavailableStore};
    use super::*;

    const API: &str = "http://localhost:8085";

    fn session(access_token: &str) -> Session {
        Session {
            access_token: access_token.into(),
            refresh_token: Some("refresh".into()),
            expires_at: 4_000_000_000,
            client_id: "client".into(),
            token_endpoint: format!("{API}/oauth/token"),
            email: None,
        }
    }

    #[test]
    fn store_choice_from_the_environment() {
        assert_eq!(StoreChoice::parse(None, false).unwrap(), StoreChoice::Auto);
        assert_eq!(StoreChoice::parse(None, true).unwrap(), StoreChoice::File);
        assert_eq!(
            StoreChoice::parse(Some(""), true).unwrap(),
            StoreChoice::File
        );
        assert_eq!(
            StoreChoice::parse(Some("file"), false).unwrap(),
            StoreChoice::File
        );
        assert_eq!(
            StoreChoice::parse(Some("keychain"), true).unwrap(),
            StoreChoice::Keychain
        );
        assert_eq!(
            StoreChoice::parse(Some("auto"), true).unwrap(),
            StoreChoice::Auto
        );
        assert!(matches!(
            StoreChoice::parse(Some("vault"), false),
            Err(Error::Usage(_))
        ));
    }

    #[test]
    fn each_api_url_gets_its_own_file() {
        let store = FileStore::new(Path::new("/config"));
        assert_eq!(
            store.path("https://api.bugsplat.com"),
            Path::new("/config/sessions/https___api.bugsplat.com.json")
        );
        assert_ne!(store.path(API), store.path("https://api.bugsplat.com"));
    }

    #[test]
    fn file_store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path());
        assert!(store.load(API).unwrap().is_none());

        store.save(API, &session("one")).unwrap();
        store.save(API, &session("two")).unwrap();
        assert_eq!(store.load(API).unwrap().unwrap().access_token, "two");
        assert!(store.load("https://api.bugsplat.com").unwrap().is_none());

        store.delete(API).unwrap();
        store.delete(API).unwrap();
        assert!(store.load(API).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path());
        store.save(API, &session("one")).unwrap();

        let file = fs::metadata(store.path(API)).unwrap().permissions().mode();
        assert_eq!(file & 0o777, 0o600);
        let folder = fs::metadata(sessions_dir(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(folder & 0o777, 0o700);
    }

    #[test]
    fn an_unreadable_session_file_counts_as_no_session() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::new(dir.path());
        create_private_dir(&sessions_dir(dir.path())).unwrap();
        fs::write(store.path(API), b"not json").unwrap();
        assert!(store.load(API).unwrap().is_none());
    }

    #[test]
    fn falls_back_to_the_file_without_a_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let store = FallbackStore {
            primary: Box::new(UnavailableStore),
            file: FileStore::new(dir.path()),
            quiet: true,
        };
        assert!(store.load(API).unwrap().is_none());

        store.save(API, &session("one")).unwrap();
        assert!(FileStore::new(dir.path()).path(API).exists());
        assert_eq!(store.load(API).unwrap().unwrap().access_token, "one");

        store.delete(API).unwrap();
        assert!(store.load(API).unwrap().is_none());
    }

    #[test]
    fn a_working_keychain_replaces_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileStore::new(dir.path());
        file.save(API, &session("from the file")).unwrap();
        let store = FallbackStore {
            primary: Box::new(MemoryStore::default()),
            file: FileStore::new(dir.path()),
            quiet: true,
        };
        assert_eq!(
            store.load(API).unwrap().unwrap().access_token,
            "from the file"
        );

        store.save(API, &session("from the keychain")).unwrap();
        assert!(!file.path(API).exists());
        assert_eq!(
            store.load(API).unwrap().unwrap().access_token,
            "from the keychain"
        );

        store.delete(API).unwrap();
        assert!(store.load(API).unwrap().is_none());
    }

    #[test]
    fn the_keychain_refuses_to_run_in_tests() {
        assert!(KeychainStore.load(API).is_err());
        assert!(KeychainStore.save(API, &session("one")).is_err());
        assert!(KeychainStore.delete(API).is_err());
    }
}
