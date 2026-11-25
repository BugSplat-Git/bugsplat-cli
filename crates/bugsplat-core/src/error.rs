/// Errors every crate reports the same way. The CLI maps them to exit codes.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "Not signed in. Run `bugsplat login`, or supply a Client ID and Client Secret for the database."
    )]
    NotSignedIn,

    #[error("{0}")]
    Usage(String),

    /// BugSplat answered, and the answer was an error.
    #[error("{message}")]
    Api {
        status: Option<u16>,
        message: String,
    },

    #[error(transparent)]
    Http(#[from] reqwest::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// 1 failure, 2 usage, 3 not signed in.
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Usage(_) => 2,
            Error::NotSignedIn => 3,
            _ => 1,
        }
    }

    pub fn api(status: impl Into<Option<u16>>, message: impl Into<String>) -> Self {
        Error::Api {
            status: status.into(),
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        assert_eq!(Error::Usage("x".into()).exit_code(), 2);
        assert_eq!(Error::NotSignedIn.exit_code(), 3);
        assert_eq!(Error::api(500, "x").exit_code(), 1);
    }
}
