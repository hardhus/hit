//! `hit auth login | logout | status`.

use std::{
    ffi::OsStr,
    io::{self, IsTerminal, Read},
    process::ExitCode,
};

use color_eyre::eyre::{Result, WrapErr, eyre};
use hit_auth::{Account, Accounts, KeyringStore, Secret, TokenKind, TokenSource, env_token};
use hit_config::{Config, Paths};
use hit_core::{AccessHint, ForgeError, Host};
use hit_github::{
    Api, GithubHost, GithubLogin, LoginSettings, LoginStorage, notifications_access, oauth, orgs,
    verify,
};

use crate::{
    accounts::{self, api_options},
    cli::{AuthCommand, LoginArgs},
    style,
};

pub fn run(command: &AuthCommand, config: &Config, paths: &Paths) -> Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .wrap_err("could not start the async runtime")?;
    match command {
        AuthCommand::Login(args) => runtime.block_on(login(args, config, paths)),
        AuthCommand::Logout { hostname, user } => {
            logout(hostname.as_deref(), user.as_deref(), config, paths)
        }
        AuthCommand::Status => runtime.block_on(status(config, paths)),
    }
}

async fn login(args: &LoginArgs, config: &Config, paths: &Paths) -> Result<ExitCode> {
    let host = GithubHost::parse(&args.hostname)?;
    let storage = if args.insecure_storage {
        LoginStorage::InsecureFile
    } else {
        LoginStorage::Keyring
    };
    let scopes = if args.minimal_scopes {
        &oauth::MINIMAL_SCOPES
    } else {
        &oauth::SCOPES
    };
    let settings = LoginSettings {
        scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        read_only: args.read_only || config.auth.read_only,
        allow_mark_read: config.auth.allow_mark_read,
        storage,
        focus: accounts::focus_options(config),
    };

    if storage == LoginStorage::Keyring {
        // Fail before the user approves anything in the browser.
        KeyringStore::default().probe().map_err(|e| {
            eyre!(
                "{e}\n\nTo keep the token in a private file instead (readable by anyone \
                 with access to your user account), run:\n  hit auth login --insecure-storage"
            )
        })?;
    } else {
        eprintln!(
            "{} the token will be stored in a plain file under {}",
            style::warn("warning:"),
            paths.state.join("tokens").display()
        );
    }

    let login = accounts::github_login(host.clone(), config, paths, settings)?;
    let (account, _) = if args.with_token {
        let token = read_token_from_stdin()?;
        login.with_token(token, None).await?
    } else if args.from_gh {
        let token = hit_auth::gh::token_from_gh(OsStr::new("gh"), host.host())?;
        login.with_token(token, Some(TokenKind::GhCli)).await?
    } else {
        device_login(&login).await?
    };

    println!(
        "{} signed in to {} as {}",
        style::ok("✓"),
        style::accent(&account.host),
        style::pink(format!("@{}", account.login))
    );
    print_scopes(&account.scopes, scopes, account.token_kind);
    if account.read_only {
        println!(
            "  {} read-only: hit will not change anything with this account",
            style::muted("•")
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Reads a token from stdin: hidden prompt on a terminal, otherwise the whole
/// piped input. Tokens are never accepted as arguments.
fn read_token_from_stdin() -> Result<Secret> {
    let raw = if io::stdin().is_terminal() {
        rpassword::prompt_password("Paste your token (input is hidden): ")
            .wrap_err("could not read the token")?
    } else {
        let mut buffer = String::new();
        io::stdin()
            .take(4096)
            .read_to_string(&mut buffer)
            .wrap_err("could not read the token from stdin")?;
        buffer
    };
    Ok(Secret::new(raw)?)
}

async fn device_login(login: &GithubLogin) -> Result<(Account, hit_github::GithubForge)> {
    let (authorization, code) = login.start_device().await?;

    // Keep the clipboard handle alive while waiting: on X11 the contents
    // disappear with their owner.
    let clipboard = arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(code.user_code.clone()).map(|()| c))
        .ok();
    // The URL was checked to be https on the account's own host.
    let opened = open::that_detached(&code.verification_uri).is_ok();

    println!();
    println!("  {}", style::muted("one-time code"));
    println!("  {}", style::pink(spaced(&code.user_code)));
    println!();
    println!(
        "  {} {}{}",
        style::muted("1."),
        style::accent(&code.verification_uri),
        if opened {
            style::muted("  (opened in your browser)")
        } else {
            String::new()
        }
    );
    println!(
        "  {} enter the code{}",
        style::muted("2."),
        if clipboard.is_some() {
            style::muted("  (copied to your clipboard)")
        } else {
            String::new()
        }
    );
    println!();
    println!(
        "  {}",
        style::muted("waiting for approval… (ctrl+c to cancel)")
    );

    let result = login.finish_device(&authorization).await;
    drop(clipboard);
    Ok(result?)
}

fn spaced(code: &str) -> String {
    code.chars().map(String::from).collect::<Vec<_>>().join(" ")
}

fn print_scopes(granted: &[String], wanted: &[&str], kind: TokenKind) {
    if granted.is_empty() {
        if matches!(kind, TokenKind::FineGrained | TokenKind::Unknown) {
            println!(
                "  {} this token has no classic scopes; access depends on its permissions",
                style::muted("•")
            );
        }
        return;
    }
    println!("  {} scopes: {}", style::muted("•"), granted.join(", "));
    let missing = oauth::missing_scopes(granted, wanted);
    if !missing.is_empty() {
        println!(
            "  {} missing scopes: {} (some features will not work)",
            style::warn("!"),
            missing.join(", ")
        );
    }
}

fn logout(
    hostname: Option<&str>,
    user: Option<&str>,
    config: &Config,
    paths: &Paths,
) -> Result<ExitCode> {
    let path = accounts::accounts_path(paths);
    let mut all = Accounts::load(&path)?;
    let host = hostname.map(GithubHost::parse).transpose()?;

    let candidates: Vec<&Account> = all
        .iter()
        .filter(|a| host.as_ref().is_none_or(|h| &a.host == h.host()))
        .filter(|a| user.is_none_or(|u| a.login.eq_ignore_ascii_case(u)))
        .collect();
    let account = match candidates.as_slice() {
        [] => return Err(eyre!("no matching account is signed in")),
        [one] => (*one).clone(),
        many => {
            let names: Vec<String> = many.iter().map(|a| a.key().to_string()).collect();
            return Err(eyre!(
                "several accounts match ({}); pick one with --hostname and --user",
                names.join(", ")
            ));
        }
    };

    let removed = accounts::store(account.storage, paths)
        .delete(&account.key())
        .wrap_err("could not remove the stored token")?;
    all.remove(&account.key());
    all.save(&path)?;

    println!(
        "{} signed out {}{}",
        style::ok("✓"),
        style::pink(account.key()),
        if removed {
            ""
        } else {
            " (no stored token was found)"
        }
    );

    let gh_host = GithubHost::parse(account.host.as_str())?;
    match account.token_kind {
        TokenKind::OAuth => {
            let page = match accounts::client_id(&gh_host, config) {
                Some(id) => gh_host.oauth_app_settings(&id).to_string(),
                None => format!("{}settings/applications", gh_host.web()),
            };
            println!(
                "  {} the token is still valid on GitHub until revoked: {}",
                style::muted("•"),
                style::accent(page)
            );
        }
        TokenKind::GhCli => println!(
            "  {} the token belongs to the GitHub CLI; `gh auth logout` revokes it there",
            style::muted("•")
        ),
        TokenKind::Classic | TokenKind::FineGrained | TokenKind::Unknown => println!(
            "  {} delete the token on GitHub if you no longer need it: {}",
            style::muted("•"),
            style::accent(gh_host.token_settings())
        ),
    }
    if env_token(&account.host, &accounts::env).is_some() {
        println!(
            "  {} a token for {} is still set in the environment",
            style::warn("!"),
            account.host
        );
    }
    Ok(ExitCode::SUCCESS)
}

async fn status(config: &Config, paths: &Paths) -> Result<ExitCode> {
    let all = Accounts::load(&accounts::accounts_path(paths))?;
    let http = hit_auth::client()?;
    let mut problems = false;

    if all.is_empty() {
        println!("not signed in; run {}", style::accent("hit auth login"));
        let host = Host::new("github.com");
        if env_token(&host, &accounts::env).is_some() {
            println!(
                "  {} GH_TOKEN/GITHUB_TOKEN is set; `hit auth login --with-token` signs in with it",
                style::muted("•")
            );
        }
        return Ok(ExitCode::FAILURE);
    }

    for account in all.iter() {
        println!(
            "{} {}",
            style::accent(&account.host),
            style::pink(format!("@{}", account.login))
        );
        let host = GithubHost::parse(account.host.as_str())?;
        let (token, source) = match accounts::token(account, paths) {
            Ok(Some(found)) => found,
            Ok(None) => {
                problems = true;
                println!(
                    "  {} no stored token; run `hit auth login`",
                    style::error("✗")
                );
                continue;
            }
            Err(e) => {
                problems = true;
                println!("  {} {e}", style::error("✗"));
                continue;
            }
        };
        let source_line = match source {
            TokenSource::File => style::warn(source),
            other => other.to_string(),
        };
        println!("  {} token: {source_line}", style::muted("•"));
        if account.read_only || config.auth.read_only {
            println!("  {} read-only", style::muted("•"));
        }

        let api = Api::new(
            http.clone(),
            host.clone(),
            token,
            api_options(account, config),
        );
        match verify(&api).await {
            Ok(viewer) => {
                if !viewer.login.eq_ignore_ascii_case(&account.login) {
                    problems = true;
                    println!(
                        "  {} the token belongs to @{}, not @{}",
                        style::warn("!"),
                        viewer.login,
                        account.login
                    );
                }
                match &viewer.scopes {
                    Some(scopes) => print_scopes(scopes, &oauth::SCOPES, account.token_kind),
                    None => print_scopes(&[], &oauth::SCOPES, TokenKind::FineGrained),
                }
            }
            Err(e) => {
                problems = true;
                println!("  {} {}", style::error("✗"), describe(&e));
                continue;
            }
        }

        match notifications_access(&api).await {
            Ok(()) => println!("  {} notifications: available", style::muted("•")),
            Err(e) => {
                problems = true;
                println!(
                    "  {} notifications unavailable: {} (classic or OAuth tokens are required)",
                    style::warn("!"),
                    describe(&e)
                );
            }
        }

        match orgs(&api).await {
            Ok((orgs, hint)) => {
                if !orgs.is_empty() {
                    let names: Vec<&str> = orgs.iter().map(|o| o.login.as_str()).collect();
                    println!(
                        "  {} organizations: {}",
                        style::muted("•"),
                        names.join(", ")
                    );
                }
                if let Some(hint) = hint {
                    problems = true;
                    println!("  {} {}", style::warn("!"), describe_hint(&hint));
                }
                if account.token_kind == TokenKind::OAuth {
                    let page = accounts::client_id(&host, config)
                        .map(|id| host.oauth_app_settings(&id).to_string())
                        .unwrap_or_else(|| format!("{}settings/applications", host.web()));
                    println!(
                        "  {} missing an organization's private repos? request access for hit at {}",
                        style::muted("•"),
                        style::accent(page)
                    );
                }
            }
            Err(e) => println!("  {} organizations: {}", style::warn("!"), describe(&e)),
        }
    }
    Ok(if problems {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn describe(error: &ForgeError) -> String {
    match error {
        ForgeError::Unauthorized => {
            "the token is invalid or revoked; sign in again with `hit auth login`".to_owned()
        }
        ForgeError::Forbidden {
            hint: Some(hint), ..
        } => describe_hint(hint),
        other => other.to_string(),
    }
}

fn describe_hint(hint: &AccessHint) -> String {
    match hint {
        AccessHint::SsoRequired { url } => {
            format!("SAML SSO authorization required; authorize the token at {url}")
        }
        AccessHint::SsoPartial { org_ids } => format!(
            "{} organization(s) need SAML SSO authorization for this token",
            org_ids.len()
        ),
        AccessHint::OrgRestricted { org, url } => {
            format!("{org} restricts third-party apps; request access at {url}")
        }
        AccessHint::TokenScope { needed } => format!("the token needs scope(s): {needed}"),
    }
}
