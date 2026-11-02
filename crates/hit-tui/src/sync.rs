//! Stale-while-revalidate loading and refresh loops.
//!
//! Each loop first sends what the cache has (marked not fresh), then what the
//! forge returns. On failure the view keeps showing the last data and says
//! how old it is. Loops end when their task is aborted (the view went away
//! or the user asked to refresh).

use std::{collections::HashSet, future::Future, path::PathBuf, sync::Arc, time::Duration};

use futures::future::try_join_all;
use hit_cache::{Cache, Vault};
use hit_config::ConfigEdit;
use hit_core::{
    Forge, ForgeError, ItemRef, Notifications, RepoRef, Timestamp,
    model::{
        Issue, IssueDetail, Notification, NotificationId, PullRequest, PullRequestDetail, Repo,
        RepoCounts,
    },
    query::{Cursor, IssueQuery, NotificationQuery, Page, PageRequest, PrQuery, Scope},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{Action, DataEvent, Theme, Toast, highlight::highlight_diff};

/// A value and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded<T> {
    pub value: T,
    pub fetched_at: Timestamp,
    /// `false` when it came from the cache and has not been revalidated yet.
    pub fresh: bool,
}

/// A list as far as it has been loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing<T> {
    pub items: Vec<T>,
    /// Where the rest starts; `None` when this is all of it.
    pub next: Option<Cursor>,
    /// How many there are in all, when the forge says.
    pub total: Option<u32>,
}

impl<T> std::ops::Deref for Listing<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.items
    }
}

impl<T> Listing<T> {
    /// Adds the next page, leaving out what is already here.
    pub fn extend(&mut self, page: Page<T>, id: impl Fn(&T) -> &ItemRef) {
        let known: HashSet<ItemRef> = self.items.iter().map(|item| id(item).clone()).collect();
        self.items.extend(
            page.items
                .into_iter()
                .filter(|item| !known.contains(id(item))),
        );
        self.next = page.next;
        self.total = page.total.or(self.total);
    }
}

/// Fallback when the forge does not say how often to poll.
const DEFAULT_POLL: Duration = Duration::from_secs(60);
/// Upper bound on notification pages fetched just to count unread ones.
const MAX_UNREAD_PAGES: usize = 10;

const FOCUS_KEY: &str = "focus:all";
const UNREAD_KEY: &str = "unread:all";

/// A cache key that keeps a repository's data apart from everything else.
fn scoped_key(base: &str, scope: &Scope) -> String {
    match scope {
        Scope::All => base.to_owned(),
        Scope::Repo(repo) => format!("{base}@{repo}"),
    }
}

/// Pages of overlapping queries as one list without repeats; an item keeps
/// its copy from the first query that found it.
fn merged<T>(pages: Vec<Page<T>>, id: impl Fn(&T) -> &ItemRef) -> Vec<T> {
    let mut seen = HashSet::new();
    let mut items: Vec<T> = pages.into_iter().flat_map(|page| page.items).collect();
    items.retain(|item| seen.insert(id(item).clone()));
    items
}

/// The account's encryption context, or `None` (no caching) when the forge
/// offers no local key.
fn vault(forge: &dyn Forge) -> Option<Arc<Vault>> {
    let key = forge.local_key()?;
    Some(Arc::new(Vault::new(&key, &forge.account().to_string())))
}

/// Cache and vault together, or nothing.
type Store = Option<(Arc<Cache>, Arc<Vault>)>;

fn store_for(cache: Option<Arc<Cache>>, forge: &dyn Forge) -> Store {
    Some((cache?, vault(forge)?))
}

async fn cached<T>(store: &Store, key: &str) -> Option<Loaded<T>>
where
    T: DeserializeOwned + Send + 'static,
{
    let (cache, vault) = store.clone()?;
    let key = key.to_owned();
    tokio::task::spawn_blocking(move || cache.get::<T>(&vault, &key))
        .await
        .ok()
        .flatten()
        .map(|entry| Loaded {
            value: entry.value,
            fetched_at: entry.fetched_at,
            fresh: false,
        })
}

async fn save<T>(store: &Store, key: &str, value: T)
where
    T: Serialize + Send + 'static,
{
    let Some((cache, vault)) = store.clone() else {
        return;
    };
    let owned = key.to_owned();
    let result = tokio::task::spawn_blocking(move || cache.put(&vault, &owned, &value)).await;
    if let Ok(Err(e)) = result {
        tracing::warn!(error = %e, key, "could not update the cache");
    }
}

/// Saves settings off the UI thread, in order, keeping only the latest value
/// when several are waiting (holding a key down). Ends, after the last write,
/// when the sender goes away with the settings screen.
pub fn spawn_config_writer(
    path: PathBuf,
    tx: mpsc::UnboundedSender<Action>,
) -> mpsc::UnboundedSender<ConfigEdit> {
    let (edits, mut pending) = mpsc::unbounded_channel::<ConfigEdit>();
    tokio::task::spawn_blocking(move || {
        while let Some(first) = pending.blocking_recv() {
            let mut batch = vec![first];
            while let Ok(edit) = pending.try_recv() {
                batch.retain(|e| (e.section, e.key) != (edit.section, edit.key));
                batch.push(edit);
            }
            if let Err(e) = hit_config::apply_edits(&path, &batch) {
                tracing::warn!(error = %e, "could not save settings");
                let _ = tx.send(Action::Toast(Toast::error(format!(
                    "couldn't save: {e} · the change lasts until you quit"
                ))));
            }
        }
    });
    edits
}

/// One stale-while-revalidate round.
async fn load<T, F, Fut>(
    store: &Store,
    key: &str,
    use_cache: bool,
    fetch: F,
    send: &(dyn Fn(Result<Loaded<T>, ForgeError>) + Send + Sync),
) where
    T: Serialize + DeserializeOwned + Clone + Send + 'static,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, ForgeError>>,
{
    if use_cache && let Some(hit) = cached::<T>(store, key).await {
        send(Ok(hit));
    }
    match fetch().await {
        Ok(value) => {
            save(store, key, value.clone()).await;
            send(Ok(Loaded {
                value,
                fetched_at: Timestamp::now(),
                fresh: true,
            }));
        }
        Err(e) => {
            tracing::warn!(error = %e, key, "could not refresh");
            send(Err(e));
        }
    }
}

/// Action Required, every `every` (once if `None`).
pub fn spawn_focus(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    scope: Scope,
    every: Option<Duration>,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let key = scoped_key(FOCUS_KEY, &scope);
        let send = |result: Result<Loaded<Vec<_>>, ForgeError>| {
            let result = result.map(|l| Loaded {
                value: Arc::new(l.value),
                fetched_at: l.fetched_at,
                fresh: l.fresh,
            });
            let _ = tx.send(Action::Data(DataEvent::Focus {
                account: account.clone(),
                scope: scope.clone(),
                result,
            }));
        };
        let mut first = true;
        loop {
            let _ = tx.send(Action::Data(DataEvent::Refreshing(account.clone())));
            let fetch = || async {
                match forge.focus() {
                    Some(focus) => focus.items(&scope).await,
                    None => Err(ForgeError::Unsupported("Action Required")),
                }
            };
            load(&store, &key, first, fetch, &send).await;
            first = false;
            match every {
                Some(every) => tokio::time::sleep(every).await,
                None => break,
            }
        }
    })
}

/// Unread notifications, at the forge's poll interval (once if `poll` is off).
pub fn spawn_unread(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    scope: Scope,
    poll: bool,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Some(_) = forge.notifications() else {
            return;
        };
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let key = scoped_key(UNREAD_KEY, &scope);
        let send = |result| {
            let _ = tx.send(Action::Data(DataEvent::UnreadCount {
                account: account.clone(),
                scope: scope.clone(),
                result,
            }));
        };
        let mut first = true;
        loop {
            let fetch = || async {
                match forge.notifications() {
                    Some(notifications) => count_unread(notifications, &scope).await,
                    None => Ok(0),
                }
            };
            load(&store, &key, first, fetch, &send).await;
            first = false;
            if !poll {
                break;
            }
            let interval = forge
                .notifications()
                .and_then(Notifications::poll_interval)
                .unwrap_or(DEFAULT_POLL);
            tokio::time::sleep(interval).await;
        }
    })
}

async fn count_unread(
    notifications: &dyn Notifications,
    scope: &Scope,
) -> Result<usize, ForgeError> {
    let query = NotificationQuery {
        scope: scope.clone(),
        unread_only: true,
        ..NotificationQuery::default()
    };
    let mut page = PageRequest::first(50);
    let mut total = 0;
    for _ in 0..MAX_UNREAD_PAGES {
        let result = notifications.list(&query, &page).await?;
        total += result.items.len();
        match result.next {
            Some(cursor) => page = PageRequest::after(cursor, 50),
            None => break,
        }
    }
    Ok(total)
}

/// Pages fetched for the inbox (newest first, 50 per page).
const INBOX_PAGES: usize = 4;
/// The inbox shows this many, and says so when it is full.
pub const INBOX_LIMIT: usize = INBOX_PAGES * 50;

/// The inbox, at the forge's poll interval (once if `poll` is off). Unread
/// and all notifications are cached separately.
pub fn spawn_inbox(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    scope: Scope,
    unread_only: bool,
    poll: bool,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if forge.notifications().is_none() {
            return;
        }
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let key = scoped_key(
            if unread_only {
                "inbox:unread"
            } else {
                "inbox:all"
            },
            &scope,
        );
        let send = |result: Result<Loaded<Vec<Notification>>, ForgeError>| {
            let result = result.map(|l| Loaded {
                value: Arc::new(l.value),
                fetched_at: l.fetched_at,
                fresh: l.fresh,
            });
            let _ = tx.send(Action::Data(DataEvent::Notifications {
                account: account.clone(),
                scope: scope.clone(),
                unread_only,
                result,
            }));
        };
        let mut first = true;
        loop {
            let fetch = || async {
                match forge.notifications() {
                    Some(notifications) => fetch_inbox(notifications, &scope, unread_only).await,
                    None => Ok(Vec::new()),
                }
            };
            load(&store, &key, first, fetch, &send).await;
            first = false;
            if !poll {
                break;
            }
            let interval = forge
                .notifications()
                .and_then(Notifications::poll_interval)
                .unwrap_or(DEFAULT_POLL);
            tokio::time::sleep(interval).await;
        }
    })
}

async fn fetch_inbox(
    notifications: &dyn Notifications,
    scope: &Scope,
    unread_only: bool,
) -> Result<Vec<Notification>, ForgeError> {
    let query = NotificationQuery {
        scope: scope.clone(),
        unread_only,
        ..NotificationQuery::default()
    };
    let mut page = PageRequest::first(50);
    let mut items = Vec::new();
    for _ in 0..INBOX_PAGES {
        let result = notifications.list(&query, &page).await?;
        items.extend(result.items);
        match result.next {
            Some(cursor) => page = PageRequest::after(cursor, 50),
            None => break,
        }
    }
    Ok(items)
}

/// Marks exactly `ids` read on the forge (never "everything": that would
/// also hit notifications the user has not seen), reporting the outcome so
/// the view can confirm or undo its optimistic update.
pub fn spawn_mark_read(
    forge: Arc<dyn Forge>,
    ids: Vec<NotificationId>,
    tx: mpsc::UnboundedSender<Action>,
) {
    tokio::spawn(async move {
        let result = match forge.notifications() {
            None => Err(ForgeError::Unsupported("notifications")),
            Some(notifications) => {
                let mut result = Ok(());
                for id in &ids {
                    if let Err(e) = notifications.mark_read(id).await {
                        result = Err(e);
                        break;
                    }
                }
                result
            }
        };
        let _ = tx.send(Action::Data(DataEvent::MarkedRead {
            account: forge.account().clone(),
            ids,
            result,
        }));
    });
}

/// Pull requests and issues per page.
const LIST_PAGE: u16 = 50;

/// Only the search knows about requests to a team.
fn mark_review_requests(query: &PrQuery, page: &mut Page<PullRequest>, login: &str) {
    if query.reviewer.as_deref() == Some(login) {
        for pr in &mut page.items {
            pr.review_requested_of_viewer = true;
        }
    }
}

/// The next page of one query, for a list the user has scrolled down.
pub fn spawn_more_pulls(
    forge: Arc<dyn Forge>,
    view: &'static str,
    query: PrQuery,
    cursor: Cursor,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let account = forge.account().clone();
        let page = PageRequest::after(cursor.clone(), LIST_PAGE);
        let result = match forge.pull_requests() {
            Some(pulls) => pulls.list(&query, &page).await.map(|mut page| {
                mark_review_requests(&query, &mut page, &account.login);
                page
            }),
            None => Err(ForgeError::Unsupported("pull requests")),
        };
        let _ = tx.send(Action::Data(DataEvent::PullsMore {
            account,
            scope: query.scope,
            view,
            cursor,
            result,
        }));
    })
}

/// See [`spawn_more_pulls`].
pub fn spawn_more_issues(
    forge: Arc<dyn Forge>,
    view: &'static str,
    query: IssueQuery,
    cursor: Cursor,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let page = PageRequest::after(cursor.clone(), LIST_PAGE);
        let result = match forge.issues() {
            Some(issues) => issues.list(&query, &page).await,
            None => Err(ForgeError::Unsupported("issues")),
        };
        let _ = tx.send(Action::Data(DataEvent::IssuesMore {
            account: forge.account().clone(),
            scope: query.scope,
            view,
            cursor,
            result,
        }));
    })
}

/// A listing from overlapping queries; the last one is the main one, whose
/// pages continue when the user scrolls.
fn listing<T>(pages: Vec<Page<T>>, id: impl Fn(&T) -> &ItemRef) -> Listing<T> {
    let next = pages.last().and_then(|page| page.next.clone());
    let total = pages.last().and_then(|page| page.total);
    Listing {
        items: merged(pages, id),
        next,
        total,
    }
}

/// One page of pull requests for a list view (cached per view).
pub fn spawn_pulls(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    view: &'static str,
    queries: Vec<PrQuery>,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let scope = queries.first().map(|q| q.scope.clone()).unwrap_or_default();
        let send = |result: Result<Loaded<Listing<PullRequest>>, ForgeError>| {
            let _ = tx.send(Action::Data(DataEvent::Pulls {
                account: account.clone(),
                scope: scope.clone(),
                view,
                result: result.map(|l| Loaded {
                    value: Arc::new(l.value),
                    fetched_at: l.fetched_at,
                    fresh: l.fresh,
                }),
            }));
        };
        let fetch = || async {
            match forge.pull_requests() {
                Some(pulls) => {
                    let page = PageRequest::first(LIST_PAGE);
                    try_join_all(queries.iter().map(|query| pulls.list(query, &page)))
                        .await
                        .map(|mut pages| {
                            for (query, page) in queries.iter().zip(&mut pages) {
                                mark_review_requests(query, page, &account.login);
                            }
                            listing(pages, |pr| &pr.reference)
                        })
                }
                None => Err(ForgeError::Unsupported("pull requests")),
            }
        };
        let key = scoped_key(&format!("pulls:{view}"), &scope);
        load(&store, &key, true, fetch, &send).await;
    })
}

/// One ready-made issue view of one account, cached per view.
pub fn spawn_issues(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    view: &'static str,
    queries: Vec<IssueQuery>,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let scope = queries.first().map(|q| q.scope.clone()).unwrap_or_default();
        let send = |result: Result<Loaded<Listing<Issue>>, ForgeError>| {
            let _ = tx.send(Action::Data(DataEvent::Issues {
                account: account.clone(),
                scope: scope.clone(),
                view,
                result: result.map(|l| Loaded {
                    value: Arc::new(l.value),
                    fetched_at: l.fetched_at,
                    fresh: l.fresh,
                }),
            }));
        };
        let fetch = || async {
            match forge.issues() {
                Some(issues) => {
                    let page = PageRequest::first(LIST_PAGE);
                    try_join_all(queries.iter().map(|query| issues.list(query, &page)))
                        .await
                        .map(|pages| listing(pages, |issue| &issue.reference))
                }
                None => Err(ForgeError::Unsupported("issues")),
            }
        };
        let key = scoped_key(&format!("issues:{view}"), &scope);
        load(&store, &key, true, fetch, &send).await;
    })
}

const REPOS_KEY: &str = "repos:mine";
/// The Repos tab shows this many, and says so when there are more.
pub const REPOS_SHOWN: u16 = 100;
/// Open counts are slow: short pages, so the top of the list fills in first.
const REPO_COUNTS_PER_PAGE: u16 = 25;
const REPO_COUNTS_PAGES: usize = 4;

/// Counts from the last run stand in until the new ones arrive.
fn keep_known_counts(repos: &mut [Repo], known: &[Repo]) {
    for repo in repos.iter_mut().filter(|r| r.open_prs.is_none()) {
        if let Some(old) = known.iter().find(|old| old.reference == repo.reference) {
            repo.open_prs = old.open_prs;
            repo.open_issues = old.open_issues;
        }
    }
}

fn apply_counts(repos: &mut [Repo], counts: &[RepoCounts]) {
    for count in counts {
        if let Some(repo) = repos.iter_mut().find(|r| r.reference == count.reference) {
            repo.open_prs = Some(count.open_prs);
            repo.open_issues = Some(count.open_issues);
        }
    }
}

/// The account's repositories: the list as soon as it is here, then open
/// counts as they come, so slow counting never holds the list back. `pinned` (the repository hit was opened on)
/// is added when the list does not have it, e.g. someone else's project.
pub fn spawn_repos(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    pinned: Option<RepoRef>,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let send = |result: Result<Loaded<Vec<Repo>>, ForgeError>| {
            let _ = tx.send(Action::Data(DataEvent::Repos {
                account: account.clone(),
                result: result.map(|l| Loaded {
                    value: Arc::new(l.value),
                    fetched_at: l.fetched_at,
                    fresh: l.fresh,
                }),
            }));
        };
        let Some(repositories) = forge.repositories() else {
            send(Err(ForgeError::Unsupported("repositories")));
            return;
        };
        let known = cached::<Vec<Repo>>(&store, REPOS_KEY).await;
        if let Some(hit) = &known {
            send(Ok(hit.clone()));
        }

        let mut repos = match repositories
            .list_mine(&PageRequest::first(REPOS_SHOWN))
            .await
        {
            Ok(page) => page.items,
            Err(e) => {
                tracing::warn!(error = %e, key = REPOS_KEY, "could not refresh");
                send(Err(e));
                return;
            }
        };
        if let Some(pinned) = pinned.as_ref().filter(|r| r.host == account.host)
            && !repos.iter().any(|r| &r.reference == pinned)
            && let Ok(repo) = repositories.get(pinned).await
        {
            repos.push(repo);
        }
        if let Some(known) = &known {
            keep_known_counts(&mut repos, &known.value);
        }
        let fresh = |repos: &[Repo]| {
            Ok(Loaded {
                value: repos.to_vec(),
                fetched_at: Timestamp::now(),
                fresh: true,
            })
        };
        send(fresh(&repos));
        save(&store, REPOS_KEY, repos.clone()).await;

        let mut page = PageRequest::first(REPO_COUNTS_PER_PAGE);
        for _ in 0..REPO_COUNTS_PAGES {
            match repositories.open_counts(&page).await {
                Ok(counts) => {
                    if !counts.items.is_empty() {
                        apply_counts(&mut repos, &counts.items);
                        send(fresh(&repos));
                    }
                    match counts.next {
                        Some(cursor) => page = PageRequest::after(cursor, REPO_COUNTS_PER_PAGE),
                        None => break,
                    }
                }
                // The list is what matters; it stays, with the counts it has.
                Err(e) => {
                    tracing::warn!(error = %e, "could not load open counts");
                    break;
                }
            }
        }
        save(&store, REPOS_KEY, repos).await;
    })
}

/// An issue's detail, cached so reopening is instant.
pub fn spawn_issue(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    item: ItemRef,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let store = store_for(cache, forge.as_ref());
        let send = |result: Result<Loaded<IssueDetail>, ForgeError>| {
            let _ = tx.send(Action::Data(DataEvent::IssueDetail {
                item: item.clone(),
                result: result.map(|l| Loaded {
                    value: Arc::new(l.value),
                    fetched_at: l.fetched_at,
                    fresh: l.fresh,
                }),
            }));
        };
        let fetch = || async {
            match forge.issues() {
                Some(issues) => issues.get(&item).await,
                None => Err(ForgeError::Unsupported("issues")),
            }
        };
        load(&store, &format!("issue:{item}"), true, fetch, &send).await;
    })
}

/// A pull request's detail (cached, so reopening is instant) and its diff
/// (highlighted off the UI thread).
pub fn spawn_pull(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    item: ItemRef,
    theme: Theme,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Some(pulls) = forge.pull_requests() else {
            let _ = tx.send(Action::Data(DataEvent::PullDetail {
                item,
                result: Err(ForgeError::Unsupported("pull requests")),
            }));
            return;
        };
        let store = store_for(cache, forge.as_ref());
        let send = |result: Result<Loaded<PullRequestDetail>, ForgeError>| {
            let _ = tx.send(Action::Data(DataEvent::PullDetail {
                item: item.clone(),
                result: result.map(|l| Loaded {
                    value: Arc::new(l.value),
                    fetched_at: l.fetched_at,
                    fresh: l.fresh,
                }),
            }));
        };
        let detail = || pulls.get(&item);
        let key = format!("pull:{item}");
        let (_, diff) = tokio::join!(load(&store, &key, true, detail, &send), pulls.diff(&item));
        let result = match diff {
            Ok(diff) => tokio::task::spawn_blocking(move || highlight_diff(diff, &theme))
                .await
                .map(Arc::new)
                .map_err(|e| ForgeError::InvalidResponse(format!("highlighting failed: {e}"))),
            Err(e) => Err(e),
        };
        let _ = tx.send(Action::Data(DataEvent::PullDiff {
            item: item.clone(),
            result,
        }));
    })
}

/// "just now", "5m ago", "3h ago", "2d ago".
pub fn ago(at: Timestamp) -> String {
    let secs = Timestamp::now().duration_since(at).as_secs().max(0);
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{}m ago", secs / 60),
        3600..86_400 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use hit_forge_mock::{MockForge, MockOptions};

    use super::*;

    fn demo() -> Arc<dyn Forge> {
        Arc::new(MockForge::demo(MockOptions::instant()))
    }

    async fn focus_events(rx: &mut mpsc::UnboundedReceiver<Action>, n: usize) -> Vec<DataEvent> {
        let mut events = Vec::new();
        while events.len() < n {
            if let Some(Action::Data(event)) = rx.recv().await
                && !matches!(event, DataEvent::Refreshing(_))
            {
                events.push(event);
            }
        }
        events
    }

    #[tokio::test]
    async fn cached_value_comes_first_then_fresh() {
        let cache = Arc::new(Cache::in_memory().unwrap());
        let forge = demo();
        let (tx, mut rx) = mpsc::unbounded_channel();

        // No cache yet: only the fresh value arrives, and gets stored.
        spawn_focus(
            Arc::clone(&forge),
            Some(Arc::clone(&cache)),
            Scope::All,
            None,
            tx.clone(),
        )
        .await
        .unwrap();
        let events = focus_events(&mut rx, 1).await;
        let DataEvent::Focus {
            result: Ok(first), ..
        } = &events[0]
        else {
            panic!("expected focus data");
        };
        assert!(first.fresh);

        // Second run: cached copy first, then fresh.
        spawn_focus(
            Arc::clone(&forge),
            Some(Arc::clone(&cache)),
            Scope::All,
            None,
            tx,
        )
        .await
        .unwrap();
        let events = focus_events(&mut rx, 2).await;
        let (
            DataEvent::Focus {
                result: Ok(cached), ..
            },
            DataEvent::Focus {
                result: Ok(fresh), ..
            },
        ) = (&events[0], &events[1])
        else {
            panic!("expected cached then fresh: {events:?}");
        };
        assert!(!cached.fresh && fresh.fresh);
        assert_eq!(cached.value, fresh.value);
    }

    #[tokio::test]
    async fn failures_keep_the_cached_value_visible() {
        let cache = Arc::new(Cache::in_memory().unwrap());
        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_unread(
            demo(),
            Some(Arc::clone(&cache)),
            Scope::All,
            false,
            tx.clone(),
        )
        .await
        .unwrap();
        let _ = focus_events(&mut rx, 1).await;

        let failing: Arc<dyn Forge> = Arc::new(MockForge::demo(MockOptions {
            failure_rate: 1.0,
            ..MockOptions::instant()
        }));
        spawn_unread(failing, Some(cache), Scope::All, false, tx)
            .await
            .unwrap();
        let events = focus_events(&mut rx, 2).await;
        assert!(matches!(&events[0], DataEvent::UnreadCount { result: Ok(l), .. } if !l.fresh));
        assert!(matches!(
            &events[1],
            DataEvent::UnreadCount { result: Err(_), .. }
        ));
    }

    #[tokio::test]
    async fn pulls_found_by_the_review_search_are_marked() {
        let query = |reviewer: Option<&str>| PrQuery {
            reviewer: reviewer.map(str::to_owned),
            ..PrQuery::default()
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_pulls(
            demo(),
            None,
            "test",
            vec![query(Some("you")), query(None)],
            tx,
        )
        .await
        .unwrap();
        let events = focus_events(&mut rx, 1).await;
        let DataEvent::Pulls {
            result: Ok(loaded), ..
        } = &events[0]
        else {
            panic!("expected pull requests: {events:?}");
        };
        assert!(
            loaded
                .value
                .iter()
                .any(|pr| pr.requested_reviewers.is_empty())
        );
        for pr in loaded.value.iter() {
            let asked = pr.requested_reviewers.iter().any(|u| u.login == "you");
            assert_eq!(pr.review_requested_of_viewer, asked, "{}", pr.reference);
        }
    }

    fn repo(name: &str, counts: Option<(u32, u32)>) -> Repo {
        Repo {
            reference: RepoRef::new(hit_core::Host::new("example.test"), "acme", name),
            description: None,
            private: false,
            fork: false,
            archived: false,
            default_branch: None,
            url: String::new(),
            pushed_at: None,
            open_prs: counts.map(|c| c.0),
            open_issues: counts.map(|c| c.1),
        }
    }

    #[test]
    fn counts_fill_in_without_touching_the_rest() {
        let mut repos = [repo("a", None), repo("b", Some((1, 1))), repo("c", None)];
        keep_known_counts(
            &mut repos,
            &[repo("a", Some((5, 6))), repo("b", Some((9, 9)))],
        );
        assert_eq!(repos[0].open_prs, Some(5), "known counts stand in");
        assert_eq!(repos[1].open_prs, Some(1), "newer counts stay");
        assert_eq!(repos[2].open_prs, None, "unknown stays unknown");

        let counts = RepoCounts {
            reference: repos[2].reference.clone(),
            open_prs: 2,
            open_issues: 3,
        };
        apply_counts(&mut repos, &[counts]);
        assert_eq!(
            (repos[2].open_prs, repos[2].open_issues),
            (Some(2), Some(3))
        );
        assert_eq!(repos[0].open_prs, Some(5));
    }

    #[tokio::test]
    async fn repositories_appear_without_waiting_for_counts() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        spawn_repos(demo(), None, None, tx).await.unwrap();
        let events = focus_events(&mut rx, 1).await;
        assert!(
            matches!(&events[0], DataEvent::Repos { result: Ok(l), .. } if l.fresh && !l.value.is_empty())
        );
    }

    #[tokio::test]
    async fn settings_reach_the_file_and_problems_come_back_as_toasts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let edit = |value| ConfigEdit {
            section: "ui",
            key: "animations",
            value: hit_config::SettingValue::Bool(value),
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        let writer = spawn_config_writer(path.clone(), tx);
        writer.send(edit(true)).unwrap();
        writer.send(edit(false)).unwrap();
        while !hit_config::load(&path).warnings.is_empty()
            || hit_config::load(&path).config.ui.animations
        {
            tokio::task::yield_now().await;
        }
        drop(writer);
        assert!(rx.recv().await.is_none(), "no problems reported");

        std::fs::write(&path, "[ui").unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let writer = spawn_config_writer(path.clone(), tx);
        writer.send(edit(true)).unwrap();
        let Some(Action::Toast(toast)) = rx.recv().await else {
            panic!("expected a toast");
        };
        assert!(toast.message.contains("syntax error"), "{}", toast.message);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[ui");
    }

    #[tokio::test]
    async fn a_list_continues_where_it_stopped() {
        let query = PrQuery::default;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let forge = demo();
        let first = forge
            .pull_requests()
            .unwrap()
            .list(&query(), &PageRequest::first(2))
            .await
            .unwrap();
        let cursor = first.next.clone().unwrap();
        let mut list = listing(vec![first], |pr: &PullRequest| &pr.reference);
        assert_eq!(list.len(), 2);
        assert!(list.total.unwrap() > 2);

        spawn_more_pulls(Arc::clone(&forge), "test", query(), cursor.clone(), tx)
            .await
            .unwrap();
        let events = focus_events(&mut rx, 1).await;
        let DataEvent::PullsMore {
            result: Ok(page),
            cursor: asked,
            ..
        } = &events[0]
        else {
            panic!("expected the next page: {events:?}");
        };
        assert_eq!(asked, &cursor);
        list.extend(page.clone(), |pr| &pr.reference);
        assert!(list.len() > 2);

        // Overlap, as when the list changed in between, repeats nothing.
        let before = list.len();
        list.extend(page.clone(), |pr| &pr.reference);
        assert_eq!(list.len(), before);
    }

    #[test]
    fn ago_formats() {
        let now = Timestamp::now();
        let minus = |s: i64| now.checked_sub(jiff::SignedDuration::from_secs(s)).unwrap();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(minus(5 * 60)), "5m ago");
        assert_eq!(ago(minus(3 * 3600)), "3h ago");
        assert_eq!(ago(minus(2 * 86_400)), "2d ago");
    }
}
