//! The GitHub provider: core capabilities on top of [`Api`].

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use hit_core::{
    AccountId, Focus, Forge, ForgeError, ForgeKind, Issues, ItemRef, Notifications, PullRequests,
    RepoRef, Repositories, Result, Timestamp,
    diff::Diff,
    focus::{self, FocusItem, FocusReason, FocusSubject},
    model::{
        Issue, IssueDetail, Notification, NotificationId, PullRequest, PullRequestDetail, Repo,
    },
    query::{
        Cursor, IssueQuery, NotificationQuery, Page, PageRequest, PrQuery, Scope, Sort, StateFilter,
    },
};
use reqwest::Method;
use serde_json::json;

use crate::{
    Api,
    api::Op,
    nodes::{
        self, FocusData, IssueData, PullData, SearchConnection, SearchData, SearchNode, queries,
    },
    rest::{NotificationJson, RepoJson, repo_path, thread_id},
};

/// Diffs larger than this are not loaded.
const MAX_DIFF_BYTES: usize = 10 * 1024 * 1024;
/// Items per Action Required category.
const FOCUS_PER_CATEGORY: u32 = 50;

/// Which Action Required categories to fetch (`[focus]` in the config).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusOptions {
    pub review_requests: bool,
    pub my_pull_requests: bool,
    pub mentions: bool,
    /// Only mentions updated in the last this many days.
    pub mention_days: u32,
    pub assigned: bool,
}

impl Default for FocusOptions {
    fn default() -> Self {
        Self {
            review_requests: true,
            my_pull_requests: true,
            mentions: true,
            mention_days: 14,
            assigned: true,
        }
    }
}

/// A signed-in GitHub account.
#[derive(Debug, Clone)]
pub struct GithubForge {
    account: AccountId,
    api: Arc<Api>,
    focus: FocusOptions,
    /// Last `X-Poll-Interval` from the notifications API, in seconds (0 = unknown).
    poll_secs: Arc<AtomicU64>,
}

impl GithubForge {
    pub fn new(account: AccountId, api: Api, focus: FocusOptions) -> Self {
        Self {
            account,
            api: Arc::new(api),
            focus,
            poll_secs: Arc::default(),
        }
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    fn host(&self) -> &hit_core::Host {
        self.api.host().host()
    }

    fn me(&self) -> &str {
        &self.account.login
    }

    async fn search(&self, query: String, page: &PageRequest) -> Result<SearchConnection> {
        let data: SearchData = self
            .api
            .graphql(
                queries::SEARCH,
                json!({
                    "q": query,
                    "first": page.per_page.clamp(1, 100),
                    "after": page.cursor.as_ref().map(|c| c.0.clone()),
                }),
            )
            .await?;
        Ok(data.search)
    }
}

impl Forge for GithubForge {
    fn kind(&self) -> ForgeKind {
        ForgeKind::GitHub
    }

    fn account(&self) -> &AccountId {
        &self.account
    }

    fn pull_requests(&self) -> Option<&dyn PullRequests> {
        Some(self)
    }

    fn issues(&self) -> Option<&dyn Issues> {
        Some(self)
    }

    fn notifications(&self) -> Option<&dyn Notifications> {
        Some(self)
    }

    fn focus(&self) -> Option<&dyn Focus> {
        Some(self)
    }

    fn repositories(&self) -> Option<&dyn Repositories> {
        Some(self)
    }

    fn local_key(&self) -> Option<hit_core::Zeroizing<[u8; 32]>> {
        Some(self.api.local_key())
    }
}

/// Free text for a search query: one line, no quotes that could end a
/// qualifier early.
fn search_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() || c == '"' { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A login used inside a search qualifier.
fn qualifier_login(login: &str) -> Result<&str> {
    let base = login.strip_suffix("[bot]").unwrap_or(login);
    let valid = !base.is_empty() && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid {
        Ok(login)
    } else {
        Err(ForgeError::InvalidResponse(format!(
            "invalid login {login:?}"
        )))
    }
}

fn scope_qualifier(scope: &Scope, me: &str) -> Result<String> {
    Ok(match scope {
        Scope::All => format!("involves:{}", qualifier_login(me)?),
        Scope::Repo(repo) => format!("repo:{}", repo_path(repo)?),
    })
}

fn state_qualifier(state: StateFilter) -> &'static str {
    match state {
        StateFilter::Open => "is:open",
        StateFilter::Closed => "is:closed",
        StateFilter::All => "",
    }
}

fn sort_qualifier(sort: Sort) -> &'static str {
    match sort {
        Sort::Updated => "sort:updated-desc",
        Sort::Created => "sort:created-desc",
    }
}

fn join(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

fn next_cursor(connection: &SearchConnection) -> Option<Cursor> {
    connection
        .page_info
        .as_ref()
        .filter(|p| p.has_next_page)
        .and_then(|p| p.end_cursor.clone())
        .map(Cursor)
}

#[async_trait]
impl PullRequests for GithubForge {
    async fn list(&self, query: &PrQuery, page: &PageRequest) -> Result<Page<PullRequest>> {
        let author = query
            .author
            .as_deref()
            .map(|a| qualifier_login(a).map(|a| format!("author:{a}")))
            .transpose()?
            .unwrap_or_default();
        let text = query.search.as_deref().map(search_text).unwrap_or_default();
        let q = join(&[
            "is:pr",
            "archived:false",
            state_qualifier(query.state),
            &scope_qualifier(&query.scope, self.me())?,
            &author,
            sort_qualifier(query.sort),
            &text,
        ]);
        let connection = self.search(q, page).await?;
        let next = next_cursor(&connection);
        let items = connection
            .nodes
            .into_iter()
            .flatten()
            .filter_map(|node| match node {
                SearchNode::PullRequest(pr) => Some(pr.into_model(self.host())),
                _ => None,
            })
            .collect();
        Ok(Page { items, next })
    }

    async fn get(&self, pr: &ItemRef) -> Result<PullRequestDetail> {
        repo_path(&pr.repo)?;
        let data: PullData = self
            .api
            .graphql(
                queries::PULL,
                json!({ "owner": pr.repo.owner, "name": pr.repo.name, "number": pr.number }),
            )
            .await?;
        let node = data
            .repository
            .and_then(|r| r.pull_request)
            .ok_or(ForgeError::NotFound)?;
        Ok(PullRequestDetail {
            pr: node.pr.into_model(self.host()),
            body: node.body,
            comments: nodes::comments(node.thread),
            reviews: nodes::reviews(node.reviews),
            checks: nodes::check_runs(node.checks),
        })
    }

    async fn diff(&self, pr: &ItemRef) -> Result<Diff> {
        let path = format!("repos/{}/pulls/{}", repo_path(&pr.repo)?, pr.number);
        let text = self
            .api
            .get_text(&path, "application/vnd.github.diff", MAX_DIFF_BYTES)
            .await?;
        Diff::parse_unified(&text).map_err(|e| ForgeError::InvalidResponse(e.to_string()))
    }
}

#[async_trait]
impl Issues for GithubForge {
    async fn list(&self, query: &IssueQuery, page: &PageRequest) -> Result<Page<Issue>> {
        let qualifier = |key: &str, value: Option<&str>| -> Result<String> {
            value
                .map(|v| qualifier_login(v).map(|v| format!("{key}:{v}")))
                .transpose()
                .map(Option::unwrap_or_default)
        };
        let author = qualifier("author", query.author.as_deref())?;
        let assignee = qualifier("assignee", query.assignee.as_deref())?;
        let text = query.search.as_deref().map(search_text).unwrap_or_default();
        let q = join(&[
            "is:issue",
            "archived:false",
            state_qualifier(query.state),
            &scope_qualifier(&query.scope, self.me())?,
            &author,
            &assignee,
            sort_qualifier(query.sort),
            &text,
        ]);
        let connection = self.search(q, page).await?;
        let next = next_cursor(&connection);
        let items = connection
            .nodes
            .into_iter()
            .flatten()
            .filter_map(|node| match node {
                SearchNode::Issue(issue) => Some(issue.into_model(self.host())),
                _ => None,
            })
            .collect();
        Ok(Page { items, next })
    }

    async fn get(&self, issue: &ItemRef) -> Result<IssueDetail> {
        repo_path(&issue.repo)?;
        let data: IssueData = self
            .api
            .graphql(
                queries::ISSUE,
                json!({ "owner": issue.repo.owner, "name": issue.repo.name, "number": issue.number }),
            )
            .await?;
        let node = data
            .repository
            .and_then(|r| r.issue)
            .ok_or(ForgeError::NotFound)?;
        Ok(IssueDetail {
            issue: node.issue.into_model(self.host()),
            body: node.body,
            comments: nodes::comments(node.thread),
        })
    }
}

#[async_trait]
impl Notifications for GithubForge {
    async fn list(
        &self,
        query: &NotificationQuery,
        page: &PageRequest,
    ) -> Result<Page<Notification>> {
        let base = match &query.scope {
            Scope::All => "notifications".to_owned(),
            Scope::Repo(repo) => format!("repos/{}/notifications", repo_path(repo)?),
        };
        let number = match &page.cursor {
            None => 1,
            Some(Cursor(n)) => n
                .parse::<u32>()
                .map_err(|_| ForgeError::InvalidResponse(format!("invalid cursor {n:?}")))?,
        };
        let mut path = format!(
            "{base}?all={}&participating={}&per_page={}&page={number}",
            !query.unread_only,
            query.participating_only,
            page.per_page.clamp(1, 50),
        );
        if let Some(since) = query.since {
            path.push_str(&format!("&since={}", since.strftime("%Y-%m-%dT%H:%M:%SZ")));
        }
        // Polled every minute: conditional, so unchanged pages cost nothing.
        let fetched = self.api.get_conditional(&path).await?;
        if let Some(interval) = fetched.poll_interval {
            self.poll_secs.store(interval.as_secs(), Ordering::Relaxed);
        }
        let next = fetched.next_page.map(|n| Cursor(n.to_string()));
        let raw: Vec<NotificationJson> = fetched.json("notifications")?;
        let items = raw
            .into_iter()
            .map(|n| n.into_model(self.api.host()))
            .collect();
        Ok(Page { items, next })
    }

    fn poll_interval(&self) -> Option<std::time::Duration> {
        match self.poll_secs.load(Ordering::Relaxed) {
            0 => None,
            secs => Some(std::time::Duration::from_secs(secs)),
        }
    }

    async fn mark_read(&self, id: &NotificationId) -> Result<()> {
        let path = format!("notifications/threads/{}", thread_id(id)?);
        self.api.send(Method::PATCH, &path, Op::MarkRead).await?;
        Ok(())
    }

    async fn mark_all_read(&self, scope: &Scope) -> Result<()> {
        let path = match scope {
            Scope::All => "notifications".to_owned(),
            Scope::Repo(repo) => format!("repos/{}/notifications", repo_path(repo)?),
        };
        self.api
            .send_json(Method::PUT, &path, Op::MarkRead, &json!({ "read": true }))
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Focus for GithubForge {
    async fn items(&self, scope: &Scope) -> Result<Vec<FocusItem>> {
        let me = qualifier_login(self.me())?;
        let scope = match scope {
            Scope::All => String::new(),
            Scope::Repo(repo) => format!("repo:{}", repo_path(repo)?),
        };
        let since = Timestamp::now()
            .checked_sub(jiff::SignedDuration::from_hours(
                24 * i64::from(self.focus.mention_days.max(1)),
            ))
            .unwrap_or_else(|_| Timestamp::now())
            .strftime("%Y-%m-%d")
            .to_string();
        let base = "is:open archived:false";
        let options = self.focus;
        let data: FocusData = self
            .api
            .graphql(
                queries::FOCUS,
                json!({
                    "reviews": join(&[base, "is:pr", &format!("review-requested:{me}"), &scope]),
                    "withReviews": options.review_requests,
                    "mine": join(&[base, "is:pr", &format!("author:{me}"), &scope]),
                    "withMine": options.my_pull_requests,
                    "mentions": join(&[base, &format!("mentions:{me}"), &format!("updated:>={since}"), &scope]),
                    "withMentions": options.mentions,
                    "assigned": join(&[base, &format!("assignee:{me}"), &scope]),
                    "withAssigned": options.assigned,
                    "first": FOCUS_PER_CATEGORY,
                }),
            )
            .await?;
        Ok(collect_focus(data, self.host(), me))
    }
}

/// Merges the per-category searches into one list; an item found by several
/// searches gets all their reasons.
fn collect_focus(data: FocusData, host: &hit_core::Host, me: &str) -> Vec<FocusItem> {
    let mut merged: HashMap<ItemRef, FocusItem> = HashMap::new();
    let mut add = |subject: FocusSubject, reasons: Vec<FocusReason>| {
        if reasons.is_empty() {
            return;
        }
        let updated_at = match &subject {
            FocusSubject::PullRequest(pr) => pr.updated_at,
            FocusSubject::Issue(issue) => issue.updated_at,
        };
        let entry = merged
            .entry(subject.reference().clone())
            .or_insert_with(|| FocusItem {
                subject,
                reasons: Vec::new(),
                updated_at,
            });
        entry.reasons.extend(reasons);
    };

    let nodes = |connection: Option<SearchConnection>| {
        connection
            .map(|c| c.nodes)
            .unwrap_or_default()
            .into_iter()
            .flatten()
    };
    for node in nodes(data.reviews) {
        if let SearchNode::PullRequest(pr) = node {
            add(
                FocusSubject::PullRequest(pr.into_model(host)),
                vec![FocusReason::ReviewRequested],
            );
        }
    }
    for node in nodes(data.mine) {
        if let SearchNode::PullRequest(pr) = node {
            let pr = pr.into_model(host);
            let reasons = focus::pr_reasons(&pr, me, false);
            add(FocusSubject::PullRequest(pr), reasons);
        }
    }
    for node in nodes(data.mentions) {
        match node {
            SearchNode::PullRequest(pr) => {
                let pr = pr.into_model(host);
                if pr.author.login != me {
                    add(FocusSubject::PullRequest(pr), vec![FocusReason::Mentioned]);
                }
            }
            SearchNode::Issue(issue) => {
                let issue = issue.into_model(host);
                if issue.author.login != me {
                    add(FocusSubject::Issue(issue), vec![FocusReason::Mentioned]);
                }
            }
            SearchNode::Other => {}
        }
    }
    for node in nodes(data.assigned) {
        match node {
            SearchNode::PullRequest(pr) => add(
                FocusSubject::PullRequest(pr.into_model(host)),
                vec![FocusReason::Assigned],
            ),
            SearchNode::Issue(issue) => add(
                FocusSubject::Issue(issue.into_model(host)),
                vec![FocusReason::Assigned],
            ),
            SearchNode::Other => {}
        }
    }

    let mut items: Vec<FocusItem> = merged
        .into_values()
        .map(|mut item| {
            item.reasons.sort();
            item.reasons.dedup();
            item
        })
        .collect();
    items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
    items
}

#[async_trait]
impl Repositories for GithubForge {
    async fn get(&self, repo: &RepoRef) -> Result<Repo> {
        let raw: RepoJson = self
            .api
            .get_json(&format!("repos/{}", repo_path(repo)?))
            .await?;
        Ok(raw.into_model(self.host()))
    }

    async fn list_mine(&self, page: &PageRequest) -> Result<Page<Repo>> {
        let number = match &page.cursor {
            None => 1,
            Some(Cursor(n)) => n
                .parse::<u32>()
                .map_err(|_| ForgeError::InvalidResponse(format!("invalid cursor {n:?}")))?,
        };
        let path = format!(
            "user/repos?sort=pushed&per_page={}&page={number}",
            page.per_page.clamp(1, 100)
        );
        let fetched = self.api.get_conditional(&path).await?;
        let next = fetched.next_page.map(|n| Cursor(n.to_string()));
        let raw: Vec<RepoJson> = fetched.json("repos")?;
        Ok(Page {
            items: raw.into_iter().map(|r| r.into_model(self.host())).collect(),
            next,
        })
    }
}

#[cfg(test)]
mod tests;
