//! GraphQL response shapes and their conversion into core models.
//!
//! GitHub returns `null` for deleted users, unreadable nodes and missing
//! connections; everything here tolerates that instead of failing a page.

use hit_core::{
    Host, ItemRef, RepoRef, Timestamp,
    model::{
        CheckRun, CheckState, CheckSummary, CloseReason, Comment, Issue, IssueState, Label,
        MergeMethod, MergeOptions, Mergeable, PrState, PullRequest, RepoCounts, Review,
        ReviewDecision, ReviewState, ReviewThread, Rgb, User,
    },
};
use serde::Deserialize;

pub(crate) mod queries {
    // GitHub refuses a query with a fragment it does not use, so each one
    // brings only its own.
    pub const FOCUS: &str = concat!(
        include_str!("graphql/focus.graphql"),
        include_str!("graphql/fragment_pr.graphql"),
        include_str!("graphql/fragment_issue.graphql")
    );
    pub const SEARCH: &str = concat!(
        include_str!("graphql/search.graphql"),
        include_str!("graphql/fragment_pr.graphql"),
        include_str!("graphql/fragment_issue.graphql")
    );
    pub const PULL: &str = concat!(
        include_str!("graphql/pull.graphql"),
        include_str!("graphql/fragment_pr.graphql")
    );
    pub const REPO_COUNTS: &str = include_str!("graphql/repo_counts.graphql");
    pub const MERGE_OPTIONS: &str = include_str!("graphql/merge_options.graphql");
    pub const ISSUE: &str = concat!(
        include_str!("graphql/issue.graphql"),
        include_str!("graphql/fragment_issue.graphql")
    );

    #[cfg(test)]
    pub const ALL: [&str; 6] = [FOCUS, SEARCH, PULL, REPO_COUNTS, MERGE_OPTIONS, ISSUE];
}

#[cfg(test)]
mod query_tests {
    use super::queries;

    /// What GitHub checks and a fake server does not: `scripts/graphql.sh`
    /// checks the rest against the real schema.
    #[test]
    fn every_fragment_sent_is_used_and_every_one_used_is_sent() {
        for query in queries::ALL {
            let name = |word: &str| word.trim_matches(|c: char| !c.is_alphanumeric()).to_owned();
            let defined: Vec<String> = query
                .lines()
                .filter_map(|line| line.strip_prefix("fragment "))
                .filter_map(|rest| rest.split_whitespace().next())
                .map(name)
                .collect();
            for fragment in &defined {
                assert!(
                    query.contains(&format!("...{fragment}")),
                    "{fragment} is sent but not used in:\n{query}"
                );
            }
            for used in query.split("...").skip(1) {
                let used = used.trim_start();
                if used.starts_with("on ") {
                    continue;
                }
                let used = name(used.split_whitespace().next().unwrap_or_default());
                assert!(defined.contains(&used), "{used} is used but not sent");
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Nodes<T> {
    #[serde(default = "Vec::new")]
    pub nodes: Vec<Option<T>>,
}

impl<T> Nodes<T> {
    pub fn into_iter_some(self) -> impl Iterator<Item = T> {
        self.nodes.into_iter().flatten()
    }
}

fn nodes<T>(connection: Option<Nodes<T>>) -> impl Iterator<Item = T> {
    connection.into_iter().flat_map(Nodes::into_iter_some)
}

#[derive(Debug, Deserialize)]
pub(crate) struct Actor {
    #[serde(rename = "__typename", default)]
    typename: String,
    login: String,
}

fn user(actor: Option<Actor>) -> User {
    match actor {
        Some(actor) => {
            let mut user = User::new(actor.login);
            user.is_bot = actor.typename == "Bot";
            user
        }
        // Deleted accounts show up as `null`; GitHub calls them "ghost".
        None => User::new("ghost"),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Login {
    login: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RepoNode {
    name: String,
    owner: Login,
}

impl RepoNode {
    fn reference(&self, host: &Host) -> RepoRef {
        RepoRef::new(host.clone(), self.owner.login.clone(), self.name.clone())
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct LabelNode {
    name: String,
    color: Option<String>,
    description: Option<String>,
}

fn labels(connection: Option<Nodes<LabelNode>>) -> Vec<Label> {
    nodes(connection)
        .map(|l| Label {
            color: l.color.as_deref().and_then(Rgb::from_hex),
            name: l.name,
            description: l.description.filter(|d| !d.is_empty()),
        })
        .collect()
}

fn logins(connection: Option<Nodes<Login>>) -> Vec<User> {
    nodes(connection).map(|l| User::new(l.login)).collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Count {
    total_count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Reviewer {
    login: Option<String>,
    combined_slug: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewRequest {
    requested_reviewer: Option<Reviewer>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct StateCount {
    state: String,
    count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RollupCounts {
    check_run_counts_by_state: Option<Vec<StateCount>>,
    status_context_counts_by_state: Option<Vec<StateCount>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Rollup {
    state: Option<String>,
    contexts: Option<RollupCounts>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitNode {
    status_check_rollup: Option<Rollup>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommitWrap {
    commit: CommitNode,
}

/// Classifies a check run or status context state.
fn check_state(state: &str) -> CheckState {
    match state {
        "SUCCESS" => CheckState::Success,
        "FAILURE" | "ERROR" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED" => {
            CheckState::Failure
        }
        "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" | "REQUESTED" | "EXPECTED" => {
            CheckState::Pending
        }
        _ => CheckState::Neutral,
    }
}

fn summary(rollup: Rollup) -> Option<CheckSummary> {
    let state = check_state(rollup.state.as_deref()?);
    let (mut passed, mut failed, mut pending) = (0, 0, 0);
    if let Some(contexts) = rollup.contexts {
        let counts = contexts
            .check_run_counts_by_state
            .into_iter()
            .flatten()
            .chain(
                contexts
                    .status_context_counts_by_state
                    .into_iter()
                    .flatten(),
            );
        for StateCount { state, count } in counts {
            match check_state(&state) {
                CheckState::Success => passed += count,
                CheckState::Failure => failed += count,
                CheckState::Pending => pending += count,
                CheckState::Neutral => {}
            }
        }
    }
    Some(CheckSummary {
        state,
        passed,
        failed,
        pending,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PrNode {
    number: u64,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
    created_at: Timestamp,
    updated_at: Timestamp,
    closed_at: Option<Timestamp>,
    merged_at: Option<Timestamp>,
    author: Option<Actor>,
    repository: RepoNode,
    labels: Option<Nodes<LabelNode>>,
    assignees: Option<Nodes<Login>>,
    review_requests: Option<Nodes<ReviewRequest>>,
    review_decision: Option<String>,
    mergeable: Option<String>,
    additions: Option<u32>,
    deletions: Option<u32>,
    changed_files: Option<u32>,
    comments: Option<Count>,
    head_ref_name: String,
    base_ref_name: String,
    commits: Option<Nodes<CommitWrap>>,
}

impl PrNode {
    pub fn into_model(self, host: &Host) -> PullRequest {
        let checks = nodes(self.commits)
            .next()
            .and_then(|c| c.commit.status_check_rollup)
            .and_then(summary);
        PullRequest {
            reference: ItemRef::pull_request(self.repository.reference(host), self.number),
            title: self.title,
            author: user(self.author),
            state: match self.state.as_str() {
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _ => PrState::Open,
            },
            draft: Some(self.is_draft),
            labels: labels(self.labels),
            assignees: logins(self.assignees),
            requested_reviewers: nodes(self.review_requests)
                .filter_map(|r| r.requested_reviewer)
                .filter_map(|r| r.login.or(r.combined_slug))
                .map(User::new)
                .collect(),
            review_requested_of_viewer: false,
            review_decision: match self.review_decision.as_deref() {
                Some("APPROVED") => Some(ReviewDecision::Approved),
                Some("CHANGES_REQUESTED") => Some(ReviewDecision::ChangesRequested),
                Some("REVIEW_REQUIRED") => Some(ReviewDecision::ReviewRequired),
                _ => None,
            },
            checks,
            mergeable: match self.mergeable.as_deref() {
                Some("MERGEABLE") => Some(Mergeable::Mergeable),
                Some("CONFLICTING") => Some(Mergeable::Conflicting),
                Some(_) => Some(Mergeable::Unknown),
                None => None,
            },
            additions: self.additions,
            deletions: self.deletions,
            changed_files: self.changed_files,
            comments: self.comments.map_or(0, |c| c.total_count),
            head_ref: self.head_ref_name,
            base_ref: self.base_ref_name,
            url: self.url,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
            merged_at: self.merged_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IssueNode {
    number: u64,
    title: String,
    url: String,
    #[serde(rename = "issueState")]
    state: String,
    state_reason: Option<String>,
    created_at: Timestamp,
    updated_at: Timestamp,
    closed_at: Option<Timestamp>,
    author: Option<Actor>,
    repository: RepoNode,
    labels: Option<Nodes<LabelNode>>,
    assignees: Option<Nodes<Login>>,
    comments: Option<Count>,
}

impl IssueNode {
    pub fn into_model(self, host: &Host) -> Issue {
        let state = if self.state == "CLOSED" {
            IssueState::Closed(match self.state_reason.as_deref() {
                Some("COMPLETED") => Some(CloseReason::Completed),
                Some("NOT_PLANNED") => Some(CloseReason::NotPlanned),
                Some("DUPLICATE") => Some(CloseReason::Duplicate),
                _ => None,
            })
        } else {
            IssueState::Open
        };
        Issue {
            reference: ItemRef::issue(self.repository.reference(host), self.number),
            title: self.title,
            author: user(self.author),
            state,
            labels: labels(self.labels),
            assignees: logins(self.assignees),
            comments: self.comments.map_or(0, |c| c.total_count),
            url: self.url,
            created_at: self.created_at,
            updated_at: self.updated_at,
            closed_at: self.closed_at,
        }
    }
}

/// A `search` result node.
#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
pub(crate) enum SearchNode {
    PullRequest(Box<PrNode>),
    Issue(Box<IssueNode>),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchConnection {
    pub issue_count: Option<u32>,
    pub page_info: Option<PageInfo>,
    #[serde(default)]
    pub nodes: Vec<Option<SearchNode>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TotalCount {
    total_count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepoCountsNode {
    name: String,
    owner: Login,
    pull_requests: TotalCount,
    issues: TotalCount,
}

impl RepoCountsNode {
    pub fn into_model(self, host: &Host) -> RepoCounts {
        RepoCounts {
            reference: RepoRef::new(host.clone(), self.owner.login, self.name),
            open_prs: self.pull_requests.total_count,
            open_issues: self.issues.total_count,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReposConnection {
    pub page_info: Option<PageInfo>,
    #[serde(default)]
    pub nodes: Vec<Option<RepoCountsNode>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReposViewer {
    pub repositories: ReposConnection,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReposData {
    pub viewer: ReposViewer,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SearchData {
    pub search: SearchConnection,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FocusData {
    pub reviews: Option<SearchConnection>,
    pub mine: Option<SearchConnection>,
    pub mentions: Option<SearchConnection>,
    pub assigned: Option<SearchConnection>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommentNode {
    id: String,
    body: String,
    created_at: Timestamp,
    updated_at: Timestamp,
    author: Option<Actor>,
}

pub(crate) fn comments(connection: Option<Nodes<CommentNode>>) -> Vec<Comment> {
    nodes(connection)
        .map(|c| Comment {
            id: c.id,
            author: user(c.author),
            body: c.body,
            created_at: c.created_at,
            updated_at: c.updated_at,
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewNode {
    state: String,
    body: String,
    submitted_at: Option<Timestamp>,
    author: Option<Actor>,
}

pub(crate) fn reviews(connection: Option<Nodes<ReviewNode>>) -> Vec<Review> {
    nodes(connection)
        .map(|r| Review {
            state: match r.state.as_str() {
                "APPROVED" => ReviewState::Approved,
                "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
                "DISMISSED" => ReviewState::Dismissed,
                "PENDING" => ReviewState::Pending,
                _ => ReviewState::Commented,
            },
            author: user(r.author),
            body: r.body,
            submitted_at: r.submitted_at,
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(tag = "__typename")]
pub(crate) enum ContextNode {
    #[serde(rename_all = "camelCase")]
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        details_url: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    StatusContext {
        context: String,
        state: String,
        target_url: Option<String>,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContextsRollup {
    contexts: Option<Nodes<ContextNode>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContextsCommit {
    status_check_rollup: Option<ContextsRollup>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ContextsWrap {
    commit: ContextsCommit,
}

pub(crate) fn check_runs(connection: Option<Nodes<ContextsWrap>>) -> Vec<CheckRun> {
    nodes(connection)
        .next()
        .and_then(|c| c.commit.status_check_rollup)
        .map(|r| nodes(r.contexts).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|context| match context {
            ContextNode::CheckRun {
                name,
                status,
                conclusion,
                details_url,
            } => Some(CheckRun {
                name,
                state: check_state(conclusion.as_deref().unwrap_or(&status)),
                url: details_url,
            }),
            ContextNode::StatusContext {
                context,
                state,
                target_url,
            } => Some(CheckRun {
                name: context,
                state: check_state(&state),
                url: target_url,
            }),
            ContextNode::Other => None,
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MergePull {
    state: String,
    is_draft: bool,
    is_cross_repository: bool,
    merge_state_status: String,
    head_ref_name: String,
    base_ref_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MergeRepo {
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    viewer_default_merge_method: Option<String>,
    pull_request: Option<MergePull>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MergeData {
    pub repository: Option<MergeRepo>,
}

impl MergeRepo {
    pub fn into_model(self) -> Option<MergeOptions> {
        let pull = self.pull_request?;
        let mut allowed = Vec::new();
        for (on, method) in [
            (self.merge_commit_allowed, MergeMethod::Merge),
            (self.squash_merge_allowed, MergeMethod::Squash),
            (self.rebase_merge_allowed, MergeMethod::Rebase),
        ] {
            if on {
                allowed.push(method);
            }
        }
        let preferred = match self.viewer_default_merge_method.as_deref() {
            Some("SQUASH") => Some(MergeMethod::Squash),
            Some("REBASE") => Some(MergeMethod::Rebase),
            Some("MERGE") => Some(MergeMethod::Merge),
            _ => None,
        };
        let default = preferred
            .filter(|method| allowed.contains(method))
            .or_else(|| allowed.first().copied())?;

        let blocked = if pull.state != "OPEN" {
            Some("it is no longer open")
        } else if pull.is_draft {
            Some("it is still a draft")
        } else {
            match pull.merge_state_status.as_str() {
                "DIRTY" => Some("it has conflicts with the base branch"),
                "BLOCKED" => Some("required reviews or checks are missing"),
                "DRAFT" => Some("it is still a draft"),
                _ => None,
            }
        };
        // Worth knowing before a merge; there will not be one if it is blocked.
        let warning = match pull.merge_state_status.as_str() {
            _ if blocked.is_some() => None,
            "UNSTABLE" => Some("some checks are failing"),
            "BEHIND" => Some("the branch is behind its base"),
            "UNKNOWN" => Some("GitHub is still working out whether it can merge"),
            _ => None,
        };
        Some(MergeOptions {
            allowed,
            default,
            blocked: blocked.map(str::to_owned),
            warning: warning.map(str::to_owned),
            head_ref: pull.head_ref_name,
            base_ref: pull.base_ref_name,
            can_delete_branch: !pull.is_cross_repository,
        })
    }
}

/// The latest comments of a conversation and how many there are in all.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Thread {
    #[serde(default)]
    total_count: u32,
    #[serde(flatten)]
    comments: Nodes<CommentNode>,
}

/// The comments we have, and how many came before them.
pub(crate) fn thread(thread: Option<Thread>) -> (Vec<Comment>, u32) {
    let Some(thread) = thread else {
        return (Vec::new(), 0);
    };
    let total = thread.total_count;
    let shown = comments(Some(thread.comments));
    let earlier = total.saturating_sub(u32::try_from(shown.len()).unwrap_or(u32::MAX));
    (shown, earlier)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewThreadNode {
    path: String,
    line: Option<u32>,
    #[serde(default)]
    is_resolved: bool,
    #[serde(default)]
    is_outdated: bool,
    comments: Option<Nodes<CommentNode>>,
}

pub(crate) fn review_threads(threads: Option<Nodes<ReviewThreadNode>>) -> Vec<ReviewThread> {
    nodes(threads)
        .map(|node| ReviewThread {
            path: node.path,
            line: node.line,
            resolved: node.is_resolved,
            outdated: node.is_outdated,
            comments: comments(node.comments),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub(crate) struct PullDetailNode {
    #[serde(flatten)]
    pub pr: PrNode,
    pub body: String,
    pub thread: Option<Thread>,
    pub reviews: Option<Nodes<ReviewNode>>,
    #[serde(rename = "reviewThreads")]
    pub review_threads: Option<Nodes<ReviewThreadNode>>,
    pub checks: Option<Nodes<ContextsWrap>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PullRepo {
    pub pull_request: Option<PullDetailNode>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PullData {
    pub repository: Option<PullRepo>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueDetailNode {
    #[serde(flatten)]
    pub issue: IssueNode,
    pub body: String,
    pub thread: Option<Thread>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueRepo {
    pub issue: Option<IssueDetailNode>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct IssueData {
    pub repository: Option<IssueRepo>,
}
