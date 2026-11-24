use std::time::Duration;

use crate::Result;

/// How long one request may take from start to finish. A server that accepts a connection
/// and then goes quiet would otherwise hold a command, and any lock it has, forever.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// One client for the whole run: shared connection pool, our user agent, and
/// [`REQUEST_TIMEOUT`] on every request. A large transfer sets its own limit on the request.
pub fn http_client() -> Result<reqwest::Client> {
    http_client_with_timeout(REQUEST_TIMEOUT)
}

/// As [`http_client`], for callers whose requests are all small, such as sign-in.
pub fn http_client_with_timeout(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("bugsplat-cli/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .timeout(timeout)
        .build()?)
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::any;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn a_server_that_goes_quiet_times_out() {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let http = http_client_with_timeout(Duration::from_millis(200)).unwrap();

        let error = http.get(server.uri()).send().await.unwrap_err();

        assert!(error.is_timeout(), "{error}");
    }
}
