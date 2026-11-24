//! `login`, `logout`, `whoami`. https://github.com/BugSplat-Git/bugsplat-cli/issues/3

use bugsplat_auth::{Identity, LoginOptions, TokenSource};
use clap::Args;
use serde_json::json;

use crate::context::Context;
use crate::output::print_json;

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Print the sign-in URL instead of opening a browser
    #[arg(long)]
    pub no_browser: bool,
}

#[derive(Debug, Args)]
pub struct LogoutArgs {}

#[derive(Debug, Args)]
pub struct WhoamiArgs {}

pub async fn login(args: LoginArgs, ctx: &Context) -> anyhow::Result<()> {
    let options = LoginOptions {
        no_browser: args.no_browser,
    };
    let identity = bugsplat_auth::login(&ctx.config, &options).await?;
    if ctx.config.is_json() {
        return print_json(&identity);
    }
    match &identity.email {
        Some(email) => println!("Signed in to {} as {email}", identity.api_url),
        None => println!("Signed in to {}", identity.api_url),
    }
    Ok(())
}

pub async fn logout(_args: LogoutArgs, ctx: &Context) -> anyhow::Result<()> {
    bugsplat_auth::logout(&ctx.config).await?;
    let api_url = ctx.config.api_url.as_str().trim_end_matches('/');
    if ctx.config.is_json() {
        return print_json(&json!({"apiUrl": api_url, "signedOut": true}));
    }
    println!("Signed out of {api_url}");
    Ok(())
}

pub async fn whoami(_args: WhoamiArgs, ctx: &Context) -> anyhow::Result<()> {
    let identity = bugsplat_auth::whoami(&ctx.config).await?;
    if ctx.config.is_json() {
        return print_json(&identity);
    }
    print!("{}", describe(&identity));
    Ok(())
}

fn describe(identity: &Identity) -> String {
    let who = match (&identity.email, &identity.client_id) {
        (Some(email), _) => format!("Email:      {email}"),
        (None, Some(client_id)) => format!("Client ID:  {client_id}"),
        (None, None) => "Email:      not available (the access token doesn't name one)".to_string(),
    };
    let signed_in_with = match identity.source {
        TokenSource::Session => "browser sign-in (bugsplat login)",
        TokenSource::ClientCredentials => "Client ID and Client Secret",
        TokenSource::Environment => "the BUGSPLAT_ACCESS_TOKEN environment variable",
    };
    format!(
        "{who}\nAPI URL:    {}\nSigned in:  {signed_in_with}\n",
        identity.api_url
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(source: TokenSource) -> Identity {
        Identity {
            email: None,
            api_url: "https://api.bugsplat.com".into(),
            source,
            client_id: None,
        }
    }

    #[test]
    fn describes_a_browser_session() {
        let mut identity = identity(TokenSource::Session);
        identity.email = Some("fred@example.com".into());
        assert_eq!(
            describe(&identity),
            "Email:      fred@example.com\n\
             API URL:    https://api.bugsplat.com\n\
             Signed in:  browser sign-in (bugsplat login)\n"
        );
    }

    #[test]
    fn describes_client_credentials_by_their_client_id() {
        let mut identity = identity(TokenSource::ClientCredentials);
        identity.client_id = Some("ci-client".into());
        let text = describe(&identity);
        assert!(text.starts_with("Client ID:  ci-client\n"), "{text}");
        assert!(text.contains("Client ID and Client Secret"), "{text}");
    }

    #[test]
    fn says_so_when_the_token_names_no_email() {
        let text = describe(&identity(TokenSource::Environment));
        assert!(text.starts_with("Email:      not available"), "{text}");
        assert!(text.contains("BUGSPLAT_ACCESS_TOKEN"), "{text}");
    }
}
