//! The day's work from the command line: a thin layer over the same provider
//! code the TUI uses, so nothing here decides anything the TUI would decide
//! differently. `--json` prints what the provider returned; read-only accounts
//! refuse writes in the same place as ever.

use std::{io::Read, path::Path, process::ExitCode, sync::Arc, time::Duration};

use color_eyre::eyre::{Result, WrapErr, eyre};
use hit_config::{Config, LogLevel, Paths};
use hit_core::{
    Forge, Forges, ItemActions, ItemRef, RepoRef,
    focus::FocusSubject,
    model::{CloseReason, MergeMethod, ReviewVerdict},
    query::{IssueQuery, NotificationQuery, Page, PageRequest, PrQuery, Scope, StateFilter},
};
use hit_forge_mock::{MockForge, MockOptions};
use serde::Serialize;

use crate::{
    accounts,
    cli::{DebugCommand, InboxCommand, IssueCommand, MethodArg, PrCommand, WorkCommand},
    commands::tui,
    logging, style,
};

pub struct Options<'a> {
    pub config_file: &'a Path,
    pub log_level: Option<LogLevel>,
    pub demo: bool,
    pub repo: Option<&'a str>,
    pub account: Option<&'a str>,
    pub json: bool,
}

/// How long a screen must stay quiet to count as settled.
const SETTLE: Duration = Duration::from_millis(2500);
const PER_PAGE: u16 = 50;

struct Session {
    forge: Arc<dyn Forge>,
    repo: Option<RepoRef>,
    json: bool,
}

impl Session {
    fn scope(&self) -> Scope {
        self.repo.clone().map_or(Scope::All, Scope::Repo)
    }

    fn repo(&self) -> Result<&RepoRef> {
        self.repo
            .as_ref()
            .ok_or_else(|| eyre!("which repository? Run this inside one or pass --repo OWNER/NAME"))
    }

    fn pull(&self, number: u64) -> Result<ItemRef> {
        Ok(ItemRef::pull_request(self.repo()?.clone(), number))
    }

    fn issue(&self, number: u64) -> Result<ItemRef> {
        Ok(ItemRef::issue(self.repo()?.clone(), number))
    }

    fn actions(&self) -> Result<&dyn ItemActions> {
        self.forge
            .actions()
            .ok_or_else(|| eyre!("this forge cannot change anything"))
    }

    /// `--json` as is, or the human form.
    fn show<T: Serialize>(&self, value: &T, human: impl FnOnce(&T)) -> Result<()> {
        if self.json {
            println!("{}", serde_json::to_string_pretty(value)?);
        } else {
            human(value);
        }
        Ok(())
    }

    fn done(&self, message: &str) -> Result<()> {
        self.show(
            &serde_json::json!({ "ok": true, "message": message }),
            |_| println!("{} {message}", style::ok("✓")),
        )
    }
}

/// A body from `--body`, or from stdin with `--body -`.
fn body(text: Option<&str>) -> Result<String> {
    match text {
        Some("-") => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .wrap_err("could not read the text from stdin")?;
            Ok(buffer)
        }
        Some(text) => Ok(text.to_owned()),
        None => Ok(String::new()),
    }
}

fn required_body(text: Option<&str>) -> Result<String> {
    let body = body(text)?;
    if body.trim().is_empty() {
        return Err(eyre!("say something: --body TEXT, or --body - for stdin"));
    }
    Ok(body)
}

pub fn run(command: &WorkCommand, paths: &Paths, options: &Options<'_>) -> Result<ExitCode> {
    paths
        .ensure()
        .wrap_err("could not create hit's directories")?;
    let loaded = hit_config::load(options.config_file);
    let _log_guard = logging::init(&paths.logs, &loaded.config.log, options.log_level)?;
    let forges = if options.demo {
        let mut forges = Forges::new();
        forges.add(Arc::new(MockForge::demo(MockOptions::instant())));
        forges
    } else {
        let (forges, warnings) = accounts::forges(&loaded.config, paths);
        for warning in warnings {
            eprintln!("{} {warning}", style::warn("warning:"));
        }
        forges
    };
    let repo = tui::repo_to_open(
        &tui::Options {
            config_file: options.config_file,
            log_level: options.log_level,
            demo: options.demo,
            repo: options.repo,
        },
        &loaded.config,
        &forges,
    )
    .map_err(|e| eyre!(e))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .wrap_err("could not start the async runtime")?;

    if let WorkCommand::Debug { command } = command {
        return runtime.block_on(debug(command, &loaded.config, paths, forges, repo, options));
    }

    let forge = choose_forge(&forges, repo.as_ref(), options.account)?;
    let session = Session {
        forge,
        repo,
        json: options.json,
    };
    runtime.block_on(work(command, &session))?;
    Ok(ExitCode::SUCCESS)
}

fn choose_forge(
    forges: &Forges,
    repo: Option<&RepoRef>,
    account: Option<&str>,
) -> Result<Arc<dyn Forge>> {
    if forges.is_empty() {
        return Err(eyre!("not signed in; run `hit auth login`, or try --demo"));
    }
    let on_host = |forge: &&Arc<dyn Forge>| repo.is_none_or(|r| forge.account().host == r.host);
    let found = match account {
        Some(login) => forges
            .iter()
            .filter(on_host)
            .find(|f| f.account().login.eq_ignore_ascii_case(login)),
        None => forges.iter().find(on_host),
    };
    found
        .cloned()
        .ok_or_else(|| eyre!("no matching account is signed in"))
}

async fn debug(
    command: &DebugCommand,
    config: &Config,
    paths: &Paths,
    forges: Forges,
    repo: Option<RepoRef>,
    options: &Options<'_>,
) -> Result<ExitCode> {
    let DebugCommand::Screen {
        keys,
        width,
        height,
        every_step,
    } = command;
    let services = hit_tui::Services {
        forges,
        repo,
        // Real settings file off: a script must not change what a person set.
        cache: (!options.demo)
            .then(|| accounts::open_cache(config, paths))
            .flatten(),
        ..hit_tui::Services::default()
    };
    let screens = hit_tui::screens(config, services, keys, *width, *height, SETTLE)
        .await
        .map_err(|e| eyre!(e))?;
    let shown = if *every_step {
        &screens[..]
    } else {
        &screens[screens.len().saturating_sub(1)..]
    };
    if options.json {
        println!("{}", serde_json::to_string_pretty(&shown)?);
    } else {
        for screen in shown {
            println!("── {} ──\n{}", screen.step, screen.text);
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn work(command: &WorkCommand, s: &Session) -> Result<()> {
    match command {
        WorkCommand::Focus => focus(s).await,
        WorkCommand::Inbox {
            all,
            participating,
            command,
        } => inbox(s, *all, *participating, command.as_ref()).await,
        WorkCommand::Pr { command } => pr(s, command).await,
        WorkCommand::Issue { command } => issue(s, command).await,
        WorkCommand::Repos { counts } => repos(s, *counts).await,
        WorkCommand::Debug { .. } => Ok(()),
    }
}

async fn focus(s: &Session) -> Result<()> {
    let focus = s
        .forge
        .focus()
        .ok_or_else(|| eyre!("this forge has no Action Required"))?;
    let items = focus.items(&s.scope()).await?;
    s.show(&items, |items| {
        if items.is_empty() {
            println!("nothing needs you");
        }
        for item in items {
            let kind = match item.subject {
                FocusSubject::PullRequest(_) => "pr",
                FocusSubject::Issue(_) => "issue",
            };
            println!(
                "{kind} {} {:?}  {}",
                item.subject.reference(),
                item.reasons,
                item.subject.title()
            );
        }
    })
}

async fn inbox(
    s: &Session,
    all: bool,
    participating: bool,
    command: Option<&InboxCommand>,
) -> Result<()> {
    let notifications = s
        .forge
        .notifications()
        .ok_or_else(|| eyre!("this forge has no notifications"))?;
    if let Some(InboxCommand::Read { id }) = command {
        notifications
            .mark_read(&hit_core::model::NotificationId(id.clone()))
            .await?;
        return s.done(&format!("marked {id} read"));
    }
    let query = NotificationQuery {
        scope: s.scope(),
        unread_only: !all,
        participating_only: participating,
        since: None,
    };
    let page = notifications
        .list(&query, &PageRequest::first(PER_PAGE))
        .await?;
    s.show(&page.items, |items| {
        if items.is_empty() {
            println!("no notifications");
        }
        for n in items {
            println!(
                "{} {} {:?} {:?} {}  {}",
                n.id.0,
                if n.unread { "unread" } else { "read" },
                n.subject.kind,
                n.reason,
                n.repo,
                n.subject.title
            );
            if let Some(url) = &n.subject.url {
                println!("    {url}");
            }
        }
    })
}

fn pr_line(pr: &hit_core::model::PullRequest) -> String {
    format!(
        "#{} {:?} @{} checks={:?} review={:?} mergeable={:?}  {}",
        pr.reference.number,
        pr.state,
        pr.author.login,
        pr.checks.map(|c| c.state),
        pr.review_decision,
        pr.mergeable,
        pr.title
    )
}

fn print_total<T>(page: &Page<T>) {
    if let Some(total) = page.total
        && usize::try_from(total).is_ok_and(|total| total > page.items.len())
    {
        println!("({} of {total})", page.items.len());
    }
}

async fn pr(s: &Session, command: &PrCommand) -> Result<()> {
    let pulls = s
        .forge
        .pull_requests()
        .ok_or_else(|| eyre!("this forge has no pull requests"))?;
    match command {
        PrCommand::List {
            mine,
            review,
            closed,
        } => {
            let me = &s.forge.account().login;
            let query = PrQuery {
                scope: s.scope(),
                state: if *closed {
                    StateFilter::Closed
                } else {
                    StateFilter::Open
                },
                author: mine.then(|| me.clone()),
                reviewer: review.then(|| me.clone()),
                ..PrQuery::default()
            };
            let page = pulls.list(&query, &PageRequest::first(PER_PAGE)).await?;
            s.show(&page, |page| {
                for pr in &page.items {
                    println!("{}", pr_line(pr));
                }
                print_total(page);
            })
        }
        PrCommand::View { number } => {
            let detail = pulls.get(&s.pull(*number)?).await?;
            s.show(&detail, |d| {
                println!("{}", pr_line(&d.pr));
                println!("{} ← {}  {}", d.pr.base_ref, d.pr.head_ref, d.pr.url);
                println!("{}", d.body);
                if d.earlier_comments > 0 {
                    println!("({} earlier comments)", d.earlier_comments);
                }
                for c in &d.comments {
                    println!("comment @{}: {}", c.author.login, c.body);
                }
                for r in &d.reviews {
                    println!("review @{} {:?}: {}", r.author.login, r.state, r.body);
                }
                for t in &d.review_threads {
                    let done = if t.resolved { "resolved" } else { "open" };
                    println!("thread {}:{:?} {done}", t.path, t.line);
                    for c in &t.comments {
                        println!("  @{}: {}", c.author.login, c.body);
                    }
                }
                for c in &d.checks {
                    println!("check {} {:?} {:?}", c.name, c.state, c.url);
                }
            })
        }
        PrCommand::Diff { number } => {
            let diff = pulls.diff(&s.pull(*number)?).await?;
            s.show(&diff, |d| {
                for f in &d.files {
                    println!(
                        "{:?} {} +{} -{}",
                        f.status,
                        f.new_path
                            .as_deref()
                            .or(f.old_path.as_deref())
                            .unwrap_or("?"),
                        f.additions,
                        f.deletions
                    );
                }
            })
        }
        PrCommand::Comment { number, body: text } => {
            let item = s.pull(*number)?;
            s.actions()?
                .comment(&item, &required_body(text.as_deref())?)
                .await?;
            s.done(&format!("commented on {item}"))
        }
        PrCommand::Approve { number, body: text } => {
            let item = s.pull(*number)?;
            s.actions()?
                .review(&item, ReviewVerdict::Approve, &body(text.as_deref())?)
                .await?;
            s.done(&format!("approved {item}"))
        }
        PrCommand::RequestChanges { number, body: text } => {
            let item = s.pull(*number)?;
            s.actions()?
                .review(
                    &item,
                    ReviewVerdict::RequestChanges,
                    &required_body(text.as_deref())?,
                )
                .await?;
            s.done(&format!("asked for changes on {item}"))
        }
        PrCommand::Merge {
            number,
            method,
            delete_branch,
            yes,
        } => merge(s, *number, *method, *delete_branch, *yes).await,
        PrCommand::Close { number } => {
            let item = s.pull(*number)?;
            s.actions()?.set_open(&item, false, None).await?;
            s.done(&format!("closed {item}"))
        }
        PrCommand::Reopen { number } => {
            let item = s.pull(*number)?;
            s.actions()?.set_open(&item, true, None).await?;
            s.done(&format!("reopened {item}"))
        }
        PrCommand::Rerun { number } => {
            let item = s.pull(*number)?;
            let runs = s.actions()?.rerun_failed(&item).await?;
            s.done(&format!("running {runs} failed workflows again on {item}"))
        }
    }
}

/// Without `--yes` it only says what would happen: merging cannot be undone.
async fn merge(
    s: &Session,
    number: u64,
    method: Option<MethodArg>,
    delete_branch: bool,
    yes: bool,
) -> Result<()> {
    let item = s.pull(number)?;
    let actions = s.actions()?;
    let options = actions.merge_options(&item).await?;
    let method = method.map_or(options.default, |m| match m {
        MethodArg::Merge => MergeMethod::Merge,
        MethodArg::Squash => MergeMethod::Squash,
        MethodArg::Rebase => MergeMethod::Rebase,
    });
    if !options.allowed.contains(&method) {
        return Err(eyre!(
            "{item}: this repository does not allow {}",
            method.label()
        ));
    }
    if !yes {
        return s.show(&options, |o| {
            println!("{} → {}", o.head_ref, o.base_ref);
            println!("would use: {}", method.label());
            println!("allowed: {:?}", o.allowed);
            if let Some(warning) = &o.warning {
                println!("{} {warning}", style::warn("!"));
            }
            match &o.blocked {
                Some(blocked) => println!("{} can't merge: {blocked}", style::error("✗")),
                None => println!("nothing merged: pass --yes to merge"),
            }
        });
    }
    if let Some(blocked) = &options.blocked {
        return Err(eyre!("{item}: can't merge: {blocked}"));
    }
    actions.merge(&item, method).await?;
    if delete_branch {
        if options.can_delete_branch {
            if let Err(e) = actions.delete_branch(&item, &options.head_ref).await {
                eprintln!(
                    "{} couldn't delete {}: {e}",
                    style::warn("warning:"),
                    options.head_ref
                );
            }
        } else {
            eprintln!(
                "{} the branch is in a fork; left alone",
                style::warn("warning:")
            );
        }
    }
    s.done(&format!("merged {item}"))
}

async fn issue(s: &Session, command: &IssueCommand) -> Result<()> {
    let issues = s
        .forge
        .issues()
        .ok_or_else(|| eyre!("this forge has no issues"))?;
    match command {
        IssueCommand::List {
            assigned,
            created,
            mentioned,
            closed,
        } => {
            let me = &s.forge.account().login;
            let query = IssueQuery {
                scope: s.scope(),
                state: if *closed {
                    StateFilter::Closed
                } else {
                    StateFilter::Open
                },
                author: created.then(|| me.clone()),
                assignee: assigned.then(|| me.clone()),
                mentioned: mentioned.then(|| me.clone()),
                ..IssueQuery::default()
            };
            let page = issues.list(&query, &PageRequest::first(PER_PAGE)).await?;
            s.show(&page, |page| {
                for i in &page.items {
                    println!(
                        "#{} {:?} @{}  {}",
                        i.reference.number, i.state, i.author.login, i.title
                    );
                }
                print_total(page);
            })
        }
        IssueCommand::View { number } => {
            let detail = issues.get(&s.issue(*number)?).await?;
            s.show(&detail, |d| {
                println!(
                    "#{} {:?} @{}  {}",
                    d.issue.reference.number, d.issue.state, d.issue.author.login, d.issue.title
                );
                println!("{}", d.body);
                if d.earlier_comments > 0 {
                    println!("({} earlier comments)", d.earlier_comments);
                }
                for c in &d.comments {
                    println!("comment @{}: {}", c.author.login, c.body);
                }
            })
        }
        IssueCommand::Comment { number, body: text } => {
            let item = s.issue(*number)?;
            s.actions()?
                .comment(&item, &required_body(text.as_deref())?)
                .await?;
            s.done(&format!("commented on {item}"))
        }
        IssueCommand::Close {
            number,
            not_planned,
        } => {
            let item = s.issue(*number)?;
            let reason = if *not_planned {
                CloseReason::NotPlanned
            } else {
                CloseReason::Completed
            };
            s.actions()?.set_open(&item, false, Some(reason)).await?;
            s.done(&format!("closed {item}"))
        }
        IssueCommand::Reopen { number } => {
            let item = s.issue(*number)?;
            s.actions()?.set_open(&item, true, None).await?;
            s.done(&format!("reopened {item}"))
        }
    }
}

async fn repos(s: &Session, counts: bool) -> Result<()> {
    let repositories = s
        .forge
        .repositories()
        .ok_or_else(|| eyre!("this forge has no repository list"))?;
    let mut page = repositories.list_mine(&PageRequest::first(100)).await?;
    if counts {
        let known = repositories.open_counts(&PageRequest::first(25)).await?;
        for count in known.items {
            if let Some(repo) = page
                .items
                .iter_mut()
                .find(|r| r.reference == count.reference)
            {
                repo.open_prs = Some(count.open_prs);
                repo.open_issues = Some(count.open_issues);
            }
        }
    }
    s.show(&page, |page| {
        for r in &page.items {
            println!(
                "{} prs={:?} issues={:?} private={} {}",
                r.reference,
                r.open_prs,
                r.open_issues,
                r.private,
                r.description.as_deref().unwrap_or("")
            );
        }
    })
}
