//! An in-memory demo forge.
//!
//! Powers `hit --demo`: a realistic, deterministic data set that behaves like
//! a real forge (latency, filtering, pagination, optional failures) so the UI
//! can be developed and tried without an account. Changes (marking
//! notifications read, …) live only in memory; nothing ever leaves the process.

mod data;
mod rng;

use std::{
    cmp::Reverse,
    ops::Range,
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use async_trait::async_trait;
use hit_core::{
    AccountId, Focus, Forge, ForgeError, ForgeKind, Host, Issues, ItemRef, Notifications,
    PullRequests, RepoRef, Repositories, Result,
    diff::Diff,
    focus::{self, FocusItem, FocusSubject},
    model::{
        Issue, IssueDetail, IssueState, Notification, NotificationId, PrState, PullRequest,
        PullRequestDetail, Repo,
    },
    query::{
        Cursor, IssueQuery, NotificationQuery, Page, PageRequest, PrQuery, Scope, Sort, StateFilter,
    },
};

use crate::{data::Data, rng::Lcg};

pub const DEMO_HOST: &str = "demo.hit";
pub const DEMO_USER: &str = "you";

#[derive(Debug, Clone, PartialEq)]
pub struct MockOptions {
    /// Each call waits a pseudo-random time in this range.
    pub latency: Range<Duration>,
    /// Probability (0..=1) that a call fails with a network error.
    pub failure_rate: f32,
    pub seed: u64,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self {
            latency: Duration::from_millis(150)..Duration::from_millis(600),
            failure_rate: 0.0,
            seed: 0x5EED_F417,
        }
    }
}

impl MockOptions {
    /// No latency, no failures. For tests.
    pub fn instant() -> Self {
        Self {
            latency: Duration::ZERO..Duration::ZERO,
            ..Self::default()
        }
    }
}

pub struct MockForge {
    account: AccountId,
    options: MockOptions,
    rng: Mutex<Lcg>,
    data: Mutex<Data>,
}

impl MockForge {
    pub fn demo(options: MockOptions) -> Self {
        let host = Host::new(DEMO_HOST);
        Self {
            account: AccountId::new(host.clone(), DEMO_USER),
            rng: Mutex::new(Lcg::new(options.seed)),
            options,
            data: Mutex::new(Data::demo(&host)),
        }
    }

    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Simulates the network: waits, then maybe fails.
    async fn network(&self) -> Result<()> {
        let (delay, fail) = {
            let mut rng = self.rng.lock().unwrap_or_else(PoisonError::into_inner);
            let Range { start, end } = self.options.latency;
            let span = end.saturating_sub(start);
            let delay = start + span.mul_f32(rng.next_f32());
            (delay, rng.next_f32() < self.options.failure_rate)
        };
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if fail {
            return Err(ForgeError::Network(
                "demo: simulated network failure".to_owned(),
            ));
        }
        Ok(())
    }

    fn me(&self) -> &str {
        &self.account.login
    }
}

impl Forge for MockForge {
    fn kind(&self) -> ForgeKind {
        ForgeKind::Demo
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

    /// The demo has nothing secret; a fixed key lets tests exercise the
    /// encrypted cache. (`hit --demo` itself runs without a cache.)
    fn local_key(&self) -> Option<hit_core::Zeroizing<[u8; 32]>> {
        Some(hit_core::Zeroizing::new([0x5e; 32]))
    }
}

#[async_trait]
impl PullRequests for MockForge {
    async fn list(&self, query: &PrQuery, page: &PageRequest) -> Result<Page<PullRequest>> {
        self.network().await?;
        let mut items: Vec<PullRequest> = self
            .data()
            .pulls
            .iter()
            .map(|d| &d.pr)
            .filter(|pr| query.scope.includes(&pr.reference.repo))
            .filter(|pr| match query.state {
                StateFilter::Open => pr.state == PrState::Open,
                StateFilter::Closed => pr.state != PrState::Open,
                StateFilter::All => true,
            })
            .filter(|pr| query.author.as_ref().is_none_or(|a| &pr.author.login == a))
            .filter(|pr| matches_search(&pr.title, query.search.as_deref()))
            .cloned()
            .collect();
        sort(&mut items, query.sort, |pr| (pr.updated_at, pr.created_at));
        paginate(items, page)
    }

    async fn get(&self, pr: &ItemRef) -> Result<PullRequestDetail> {
        self.network().await?;
        self.data()
            .pulls
            .iter()
            .find(|d| &d.pr.reference == pr)
            .cloned()
            .ok_or(ForgeError::NotFound)
    }

    async fn diff(&self, pr: &ItemRef) -> Result<Diff> {
        self.network().await?;
        let text = {
            let data = self.data();
            if !data.pulls.iter().any(|d| &d.pr.reference == pr) {
                return Err(ForgeError::NotFound);
            }
            data.diffs.get(pr).cloned()
        };
        match text {
            Some(text) => {
                Diff::parse_unified(&text).map_err(|e| ForgeError::InvalidResponse(e.to_string()))
            }
            None => Ok(Diff::default()),
        }
    }
}

#[async_trait]
impl Issues for MockForge {
    async fn list(&self, query: &IssueQuery, page: &PageRequest) -> Result<Page<Issue>> {
        self.network().await?;
        let mut items: Vec<Issue> = self
            .data()
            .issues
            .iter()
            .map(|d| &d.issue)
            .filter(|issue| query.scope.includes(&issue.reference.repo))
            .filter(|issue| match query.state {
                StateFilter::Open => issue.state == IssueState::Open,
                StateFilter::Closed => issue.state != IssueState::Open,
                StateFilter::All => true,
            })
            .filter(|issue| {
                query
                    .author
                    .as_ref()
                    .is_none_or(|a| &issue.author.login == a)
            })
            .filter(|issue| {
                query
                    .assignee
                    .as_ref()
                    .is_none_or(|a| issue.assignees.iter().any(|u| &u.login == a))
            })
            .filter(|issue| matches_search(&issue.title, query.search.as_deref()))
            .cloned()
            .collect();
        sort(&mut items, query.sort, |i| (i.updated_at, i.created_at));
        paginate(items, page)
    }

    async fn get(&self, issue: &ItemRef) -> Result<IssueDetail> {
        self.network().await?;
        self.data()
            .issues
            .iter()
            .find(|d| &d.issue.reference == issue)
            .cloned()
            .ok_or(ForgeError::NotFound)
    }
}

#[async_trait]
impl Notifications for MockForge {
    async fn list(
        &self,
        query: &NotificationQuery,
        page: &PageRequest,
    ) -> Result<Page<Notification>> {
        self.network().await?;
        let mut items: Vec<Notification> = self
            .data()
            .notifications
            .iter()
            .filter(|n| query.scope.includes(&n.repo))
            .filter(|n| !query.unread_only || n.unread)
            .filter(|n| !query.participating_only || n.reason.is_participating())
            .filter(|n| query.since.is_none_or(|since| n.updated_at >= since))
            .cloned()
            .collect();
        items.sort_by_key(|n| Reverse(n.updated_at));
        paginate(items, page)
    }

    async fn mark_read(&self, id: &NotificationId) -> Result<()> {
        self.network().await?;
        let mut data = self.data();
        let notification = data
            .notifications
            .iter_mut()
            .find(|n| &n.id == id)
            .ok_or(ForgeError::NotFound)?;
        notification.unread = false;
        Ok(())
    }

    async fn mark_all_read(&self, scope: &Scope) -> Result<()> {
        self.network().await?;
        for n in self.data().notifications.iter_mut() {
            if scope.includes(&n.repo) {
                n.unread = false;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Focus for MockForge {
    async fn items(&self, scope: &Scope) -> Result<Vec<FocusItem>> {
        self.network().await?;
        let me = self.me();
        let data = self.data();
        let pulls = data.pulls.iter().filter_map(|d| {
            let mentioned = mentions(me, &d.body, d.comments.iter().map(|c| c.body.as_str()));
            let reasons = focus::pr_reasons(&d.pr, me, mentioned);
            (!reasons.is_empty()).then(|| FocusItem {
                updated_at: d.pr.updated_at,
                subject: FocusSubject::PullRequest(d.pr.clone()),
                reasons,
            })
        });
        let issues = data.issues.iter().filter_map(|d| {
            let mentioned = mentions(me, &d.body, d.comments.iter().map(|c| c.body.as_str()));
            let reasons = focus::issue_reasons(&d.issue, me, mentioned);
            (!reasons.is_empty()).then(|| FocusItem {
                updated_at: d.issue.updated_at,
                subject: FocusSubject::Issue(d.issue.clone()),
                reasons,
            })
        });
        let mut items: Vec<FocusItem> = pulls
            .chain(issues)
            .filter(|item| scope.includes(&item.subject.reference().repo))
            .collect();
        items.sort_by_key(|n| Reverse(n.updated_at));
        Ok(items)
    }
}

#[async_trait]
impl Repositories for MockForge {
    async fn get(&self, repo: &RepoRef) -> Result<Repo> {
        self.network().await?;
        self.data()
            .repos
            .iter()
            .find(|r| &r.reference == repo)
            .cloned()
            .ok_or(ForgeError::NotFound)
    }

    async fn list_mine(&self, page: &PageRequest) -> Result<Page<Repo>> {
        self.network().await?;
        let mut repos = self.data().repos.clone();
        repos.sort_by_key(|r| Reverse(r.pushed_at));
        paginate(repos, page)
    }
}

fn mentions<'a>(me: &str, body: &'a str, comments: impl Iterator<Item = &'a str>) -> bool {
    let handle = format!("@{me}");
    std::iter::once(body)
        .chain(comments)
        .any(|text| text.contains(&handle))
}

fn matches_search(title: &str, search: Option<&str>) -> bool {
    search.is_none_or(|s| title.to_lowercase().contains(&s.to_lowercase()))
}

fn sort<T, K: Ord>(items: &mut [T], sort: Sort, key: impl Fn(&T) -> (K, K)) {
    match sort {
        Sort::Updated => items.sort_by(|a, b| key(b).0.cmp(&key(a).0)),
        Sort::Created => items.sort_by(|a, b| key(b).1.cmp(&key(a).1)),
    }
}

/// Offset-based pages; the cursor is the offset of the next item.
fn paginate<T>(items: Vec<T>, page: &PageRequest) -> Result<Page<T>> {
    let start = match &page.cursor {
        None => 0,
        Some(Cursor(offset)) => offset
            .parse::<usize>()
            .map_err(|_| ForgeError::InvalidResponse(format!("invalid cursor {offset:?}")))?,
    };
    let per_page = usize::from(page.per_page.max(1));
    let total = items.len();
    let end = start.saturating_add(per_page).min(total);
    let next = (end < total).then(|| Cursor(end.to_string()));
    let items = items.into_iter().skip(start).take(per_page).collect();
    Ok(Page { items, next })
}

#[cfg(test)]
mod tests;
