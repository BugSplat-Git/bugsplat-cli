//! The loopback listener the browser is redirected to at the end of a sign-in.

use std::net::Ipv4Addr;
use std::time::Duration;

use bugsplat_core::{Error, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use url::Url;

const CALLBACK_PATH: &str = "/callback";
const MAX_REQUEST_HEAD: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct Loopback {
    listener: TcpListener,
    port: u16,
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Code(String),
    Failed(String),
    /// A request that is not the end of this sign-in: another path, or another `state`.
    Ignored(u16),
}

impl Loopback {
    pub(crate) async fn bind() -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    pub(crate) fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.port)
    }

    pub(crate) async fn wait_for_code(self, state: &str) -> Result<String> {
        let (results, mut result) = mpsc::channel(1);
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    let (stream, _) = accepted?;
                    let results = results.clone();
                    let state = state.to_string();
                    // Browsers open connections they never use; one of those must not
                    // hold up the connection that brings the code.
                    tokio::spawn(async move {
                        if let Some(outcome) = serve(stream, &state).await {
                            let _ = results.send(outcome).await;
                        }
                    });
                }
                Some(outcome) = result.recv() => return outcome,
            }
        }
    }
}

async fn serve(mut stream: TcpStream, state: &str) -> Option<Result<String>> {
    let head = tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream))
        .await
        .ok()??;
    let callback = match request_target(&head) {
        Some(target) => route(target, state),
        None => Callback::Ignored(400),
    };
    let (status, page) = match &callback {
        Callback::Code(_) => (
            200,
            "You're signed in to BugSplat. You can close this tab and return to the terminal.",
        ),
        Callback::Failed(_) => (
            200,
            "The sign-in was not completed. Return to the terminal for details.",
        ),
        Callback::Ignored(404) => (404, "Not found."),
        Callback::Ignored(status) => (*status, "This is not the sign-in the CLI is waiting for."),
    };
    respond(&mut stream, status, page).await;
    match callback {
        Callback::Code(code) => Some(Ok(code)),
        Callback::Failed(reason) => Some(Err(Error::Other(format!(
            "The sign-in was not completed: {reason}"
        )))),
        Callback::Ignored(_) => None,
    }
}

async fn read_head(stream: &mut TcpStream) -> Option<String> {
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        if head.len() > MAX_REQUEST_HEAD {
            return None;
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => head.extend_from_slice(&chunk[..read]),
        }
    }
    String::from_utf8(head).ok()
}

fn request_target(head: &str) -> Option<&str> {
    let mut parts = head.lines().next()?.split(' ');
    match (parts.next()?, parts.next()?) {
        ("GET", target) if target.starts_with('/') => Some(target),
        _ => None,
    }
}

fn route(target: &str, state: &str) -> Callback {
    let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Ignored(400);
    };
    if url.path() != CALLBACK_PATH {
        return Callback::Ignored(404);
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if param("state").as_deref() != Some(state) {
        return Callback::Ignored(400);
    }
    if let Some(error) = param("error") {
        let reason = param("error_description").unwrap_or(error);
        // The text arrives over a local socket anyone on the machine can write to, and
        // it is printed to the terminal.
        return Callback::Failed(
            reason
                .chars()
                .filter(|c| !c.is_control())
                .take(300)
                .collect(),
        );
    }
    match param("code") {
        Some(code) if !code.is_empty() => Callback::Code(code),
        _ => Callback::Ignored(400),
    }
}

async fn respond(stream: &mut TcpStream, status: u16, message: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>BugSplat CLI</title></head>\
         <body style=\"font-family:system-ui,sans-serif;max-width:32em;margin:4em auto;padding:0 1em\">\
         <h1>BugSplat CLI</h1><p>{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_target_of_a_get() {
        assert_eq!(
            request_target("GET /callback?code=1 HTTP/1.1\r\nHost: x\r\n\r\n"),
            Some("/callback?code=1")
        );
        assert_eq!(request_target("POST /callback HTTP/1.1\r\n\r\n"), None);
        assert_eq!(request_target("garbage"), None);
    }

    #[test]
    fn a_callback_with_the_state_yields_the_code() {
        assert_eq!(
            route("/callback?code=abc&state=s1", "s1"),
            Callback::Code("abc".into())
        );
    }

    #[test]
    fn another_state_or_path_is_ignored() {
        assert_eq!(
            route("/callback?code=abc&state=other", "s1"),
            Callback::Ignored(400)
        );
        assert_eq!(route("/callback?code=abc", "s1"), Callback::Ignored(400));
        assert_eq!(route("/callback?state=s1", "s1"), Callback::Ignored(400));
        assert_eq!(route("/favicon.ico", "s1"), Callback::Ignored(404));
    }

    #[test]
    fn an_error_callback_fails_with_the_description_and_no_control_characters() {
        assert_eq!(
            route(
                "/callback?error=access_denied&error_description=The%20user%20declined%1b%5b2J.&state=s1",
                "s1"
            ),
            Callback::Failed("The user declined[2J.".into())
        );
        assert_eq!(
            route("/callback?error=access_denied&state=s1", "s1"),
            Callback::Failed("access_denied".into())
        );
    }

    #[tokio::test]
    async fn serves_the_browser_and_returns_the_code() {
        let loopback = Loopback::bind().await.unwrap();
        let redirect_uri = loopback.redirect_uri();
        let waiting = tokio::spawn(async move { loopback.wait_for_code("s1").await });

        let http = reqwest::Client::new();
        let stray = http
            .get(format!("{redirect_uri}?code=stolen&state=other"))
            .send()
            .await
            .unwrap();
        assert_eq!(stray.status(), 400);
        let done = http
            .get(format!("{redirect_uri}?code=abc&state=s1"))
            .send()
            .await
            .unwrap();
        assert_eq!(done.status(), 200);
        assert!(done.text().await.unwrap().contains("signed in"));

        assert_eq!(waiting.await.unwrap().unwrap(), "abc");
    }
}
