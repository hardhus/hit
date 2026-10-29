//! The demo data set. Deterministic apart from timestamps, which are relative
//! to "now" so the demo always looks fresh.

use std::collections::HashMap;

use hit_core::{
    Host, ItemRef, RepoRef, Timestamp,
    diff::Diff,
    model::{
        CheckRun, CheckState, CheckSummary, CloseReason, Comment, Issue, IssueDetail, IssueState,
        Label, Mergeable, Notification, NotificationId, NotificationReason, PrState, PullRequest,
        PullRequestDetail, Repo, Review, ReviewDecision, ReviewState, Rgb, Subject, SubjectKind,
        User,
    },
};
use jiff::SignedDuration;

use crate::DEMO_USER;

pub(crate) struct Data {
    pub(crate) repos: Vec<Repo>,
    pub(crate) pulls: Vec<PullRequestDetail>,
    pub(crate) issues: Vec<IssueDetail>,
    pub(crate) notifications: Vec<Notification>,
    pub(crate) diffs: HashMap<ItemRef, String>,
}

/// Minutes before `now`.
fn ago(now: Timestamp, minutes: i64) -> Timestamp {
    now.checked_sub(SignedDuration::from_mins(minutes))
        .unwrap_or(now)
}

const HOUR: i64 = 60;
const DAY: i64 = 24 * HOUR;

fn user(login: &str) -> User {
    let mut user = User::new(login);
    user.is_bot = login.ends_with("[bot]");
    user
}

fn label(name: &str) -> Label {
    let hex = match name {
        "bug" => "ff4d6d",
        "enhancement" => "00b4ff",
        "docs" => "2de2e6",
        "good first issue" => "7cff4f",
        "ci" => "ffe14d",
        "breaking" => "ff2e97",
        "question" => "9d4edd",
        _ => "8a7fb0",
    };
    Label::new(name, Rgb::from_hex(hex))
}

struct PrBuilder {
    detail: PullRequestDetail,
    diff: Option<String>,
    now: Timestamp,
}

impl PrBuilder {
    fn new(now: Timestamp, repo: &RepoRef, number: u64, title: &str, author: &str) -> Self {
        let reference = ItemRef::pull_request(repo.clone(), number);
        let url = format!("https://{}/{repo}/pull/{number}", repo.host);
        let pr = PullRequest {
            reference,
            title: title.to_owned(),
            author: user(author),
            state: PrState::Open,
            draft: Some(false),
            labels: Vec::new(),
            assignees: Vec::new(),
            requested_reviewers: Vec::new(),
            review_decision: Some(ReviewDecision::ReviewRequired),
            checks: None,
            mergeable: Some(Mergeable::Mergeable),
            additions: None,
            deletions: None,
            changed_files: None,
            comments: 0,
            head_ref: format!("{author}/{}", slug(title)),
            base_ref: "main".to_owned(),
            url,
            created_at: now,
            updated_at: now,
            closed_at: None,
            merged_at: None,
        };
        Self {
            detail: PullRequestDetail {
                pr,
                body: String::new(),
                comments: Vec::new(),
                reviews: Vec::new(),
                checks: Vec::new(),
            },
            diff: None,
            now,
        }
    }

    /// Created `created` minutes ago, last updated `updated` minutes ago.
    fn times(mut self, created: i64, updated: i64) -> Self {
        self.detail.pr.created_at = ago(self.now, created);
        self.detail.pr.updated_at = ago(self.now, updated);
        self
    }

    fn draft(mut self) -> Self {
        self.detail.pr.draft = Some(true);
        self
    }

    fn merged(mut self, minutes_ago: i64) -> Self {
        let at = ago(self.now, minutes_ago);
        self.detail.pr.state = PrState::Merged;
        self.detail.pr.merged_at = Some(at);
        self.detail.pr.closed_at = Some(at);
        self.detail.pr.review_decision = Some(ReviewDecision::Approved);
        self
    }

    fn closed(mut self, minutes_ago: i64) -> Self {
        self.detail.pr.state = PrState::Closed;
        self.detail.pr.closed_at = Some(ago(self.now, minutes_ago));
        self
    }

    fn labels(mut self, names: &[&str]) -> Self {
        self.detail.pr.labels = names.iter().map(|n| label(n)).collect();
        self
    }

    fn reviewers(mut self, logins: &[&str]) -> Self {
        self.detail.pr.requested_reviewers = logins.iter().map(|l| user(l)).collect();
        self
    }

    fn assignees(mut self, logins: &[&str]) -> Self {
        self.detail.pr.assignees = logins.iter().map(|l| user(l)).collect();
        self
    }

    fn decision(mut self, decision: ReviewDecision) -> Self {
        self.detail.pr.review_decision = Some(decision);
        self
    }

    fn conflicting(mut self) -> Self {
        self.detail.pr.mergeable = Some(Mergeable::Conflicting);
        self
    }

    fn checks(mut self, runs: &[(&str, CheckState)]) -> Self {
        let repo = &self.detail.pr.reference.repo;
        self.detail.checks = runs
            .iter()
            .enumerate()
            .map(|(i, (name, state))| CheckRun {
                name: (*name).to_owned(),
                state: *state,
                url: Some(format!(
                    "https://{}/{repo}/actions/runs/{}",
                    repo.host,
                    9000 + i
                )),
            })
            .collect();
        self.detail.pr.checks = CheckSummary::from_runs(&self.detail.checks);
        self
    }

    fn body(mut self, body: &str) -> Self {
        self.detail.body = body.to_owned();
        self
    }

    fn comment(mut self, author: &str, minutes_ago: i64, body: &str) -> Self {
        let at = ago(self.now, minutes_ago);
        let id = format!(
            "c{}-{}",
            self.detail.pr.reference.number,
            self.detail.comments.len()
        );
        self.detail.comments.push(Comment {
            id,
            author: user(author),
            body: body.to_owned(),
            created_at: at,
            updated_at: at,
        });
        self.detail.pr.comments = u32::try_from(self.detail.comments.len()).unwrap_or(u32::MAX);
        self
    }

    fn review(mut self, author: &str, state: ReviewState, minutes_ago: i64, body: &str) -> Self {
        self.detail.reviews.push(Review {
            author: user(author),
            state,
            body: body.to_owned(),
            submitted_at: Some(ago(self.now, minutes_ago)),
        });
        self
    }

    fn diff(mut self, diff: &str) -> Self {
        self.diff = Some(diff.to_owned());
        self
    }

    fn build(self) -> (PullRequestDetail, String) {
        let mut detail = self.detail;
        let diff = self.diff.unwrap_or_else(|| changelog_diff(&detail.pr));
        // Keep the summary numbers consistent with the diff itself.
        if let Ok(parsed) = Diff::parse_unified(&diff) {
            detail.pr.additions = Some(parsed.additions());
            detail.pr.deletions = Some(parsed.deletions());
            detail.pr.changed_files = u32::try_from(parsed.files.len()).ok();
        }
        (detail, diff)
    }
}

struct IssueBuilder {
    detail: IssueDetail,
    now: Timestamp,
}

impl IssueBuilder {
    fn new(now: Timestamp, repo: &RepoRef, number: u64, title: &str, author: &str) -> Self {
        let url = format!("https://{}/{repo}/issues/{number}", repo.host);
        Self {
            detail: IssueDetail {
                issue: Issue {
                    reference: ItemRef::issue(repo.clone(), number),
                    title: title.to_owned(),
                    author: user(author),
                    state: IssueState::Open,
                    labels: Vec::new(),
                    assignees: Vec::new(),
                    comments: 0,
                    url,
                    created_at: now,
                    updated_at: now,
                    closed_at: None,
                },
                body: String::new(),
                comments: Vec::new(),
            },
            now,
        }
    }

    fn times(mut self, created: i64, updated: i64) -> Self {
        self.detail.issue.created_at = ago(self.now, created);
        self.detail.issue.updated_at = ago(self.now, updated);
        self
    }

    fn closed(mut self, reason: CloseReason, minutes_ago: i64) -> Self {
        self.detail.issue.state = IssueState::Closed(Some(reason));
        self.detail.issue.closed_at = Some(ago(self.now, minutes_ago));
        self
    }

    fn labels(mut self, names: &[&str]) -> Self {
        self.detail.issue.labels = names.iter().map(|n| label(n)).collect();
        self
    }

    fn assignees(mut self, logins: &[&str]) -> Self {
        self.detail.issue.assignees = logins.iter().map(|l| user(l)).collect();
        self
    }

    fn body(mut self, body: &str) -> Self {
        self.detail.body = body.to_owned();
        self
    }

    fn comment(mut self, author: &str, minutes_ago: i64, body: &str) -> Self {
        let at = ago(self.now, minutes_ago);
        let id = format!(
            "i{}-{}",
            self.detail.issue.reference.number,
            self.detail.comments.len()
        );
        self.detail.comments.push(Comment {
            id,
            author: user(author),
            body: body.to_owned(),
            created_at: at,
            updated_at: at,
        });
        self.detail.issue.comments = u32::try_from(self.detail.comments.len()).unwrap_or(u32::MAX);
        self
    }

    fn build(self) -> IssueDetail {
        self.detail
    }
}

fn slug(title: &str) -> String {
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.split('-')
        .filter(|s| !s.is_empty())
        .take(4)
        .collect::<Vec<_>>()
        .join("-")
}

/// A small generated diff for pull requests without a handwritten one.
fn changelog_diff(pr: &PullRequest) -> String {
    format!(
        "diff --git a/CHANGELOG.md b/CHANGELOG.md\n\
         --- a/CHANGELOG.md\n\
         +++ b/CHANGELOG.md\n\
         @@ -1,3 +1,4 @@\n \
         # Changelog\n \
         \n \
         ## Unreleased\n\
         +- {} (#{})\n",
        pr.title, pr.reference.number
    )
}

impl Data {
    pub(crate) fn demo(host: &Host) -> Self {
        let now = Timestamp::now();
        let me = DEMO_USER;
        let repo = |owner: &str, name: &str| RepoRef::new(host.clone(), owner, name);
        let rocket = repo("acme", "rocket");
        let website = repo("acme", "website");
        let infra = repo("acme", "infra");
        let dotfiles = repo(me, "dotfiles");
        let termpaint = repo("oss", "termpaint");

        let repos = [
            (&rocket, "Blazing fast deploy CLI", false, 30),
            (&website, "Docs and marketing site", false, 3 * HOUR),
            (&infra, "Terraform for everything", true, 2 * DAY),
            (
                &dotfiles,
                "My shell, editor and terminal setup",
                false,
                20 * HOUR,
            ),
            (&termpaint, "Pixel art in your terminal", false, 4 * DAY),
        ]
        .into_iter()
        .map(|(reference, description, private, pushed)| Repo {
            reference: reference.clone(),
            description: Some(description.to_owned()),
            private,
            fork: false,
            archived: false,
            default_branch: Some("main".to_owned()),
            url: format!("https://{host}/{reference}"),
            pushed_at: Some(ago(now, pushed)),
        })
        .collect();

        use CheckState::{Failure, Pending, Success};
        let ci = |unit: CheckState, lint: CheckState| {
            [("build", Success), ("test", unit), ("clippy", lint)]
        };

        let pulls: Vec<(PullRequestDetail, String)> = vec![
            // Mine, CI failing.
            PrBuilder::new(now, &rocket, 142, "Retry flaky uploads with exponential backoff", me)
                .times(2 * DAY, 25)
                .labels(&["enhancement"])
                .reviewers(&["mira"])
                .checks(&ci(Failure, Success))
                .body("Uploads to the artifact store fail ~2% of the time with transient 503s.\n\nThis retries transient errors with exponential backoff (200ms, 400ms, 800ms…).\n\nCloses #57")
                .comment("mira", 3 * HOUR, "Nice! Can we cap the total delay so CI doesn't hang?")
                .comment(me, 2 * HOUR, "Good call, 5 attempts tops out at ~6s. Adding a test.")
                .diff(UPLOAD_DIFF)
                .build(),
            // Review requested from me.
            PrBuilder::new(now, &rocket, 145, "Add --json output to status command", "mira")
                .times(5 * HOUR, 40)
                .labels(&["enhancement"])
                .reviewers(&[me, "kaan"])
                .checks(&ci(Success, Success))
                .body("Scripts keep scraping the table output. This adds `--json`.\n\n@you this touches the CLI layout you designed, mind taking a look?")
                .diff(STATUS_DIFF)
                .build(),
            // Mine, changes requested.
            PrBuilder::new(now, &website, 88, "Dark mode for the docs site", me)
                .times(3 * DAY, 3 * HOUR)
                .labels(&["docs", "enhancement"])
                .decision(ReviewDecision::ChangesRequested)
                .checks(&[("deploy-preview", Success), ("lighthouse", Success)])
                .body("Follows `prefers-color-scheme`. Colors from the brand palette.")
                .review("sora", ReviewState::ChangesRequested, 4 * HOUR, "The pink accent fails contrast on code blocks, can we darken it a bit?")
                .diff(THEME_DIFF)
                .build(),
            // Mine, approved and green.
            PrBuilder::new(now, &dotfiles, 12, "Switch prompt to starship", me)
                .times(DAY, 2 * HOUR)
                .decision(ReviewDecision::Approved)
                .checks(&[("shellcheck", Success)])
                .review("kaan", ReviewState::Approved, 2 * HOUR, "Finally 🚀")
                .diff(PROMPT_DIFF)
                .build(),
            // Review requested from me, open source.
            PrBuilder::new(now, &termpaint, 311, "Support sixel graphics", "elif")
                .times(6 * DAY, 9 * HOUR)
                .labels(&["enhancement", "breaking"])
                .reviewers(&[me])
                .checks(&ci(Pending, Success))
                .comment("noah", 10 * HOUR, "Tested on foot and wezterm, works great.")
                .build(),
            // Assigned to me.
            PrBuilder::new(now, &infra, 77, "Rotate database credentials monthly", "kaan")
                .times(DAY, 5 * HOUR)
                .labels(&["ci"])
                .assignees(&[me])
                .checks(&[("plan", Success), ("tfsec", Success)])
                .build(),
            // Mention in a comment.
            PrBuilder::new(now, &website, 91, "Fix broken links in the changelog", "noah")
                .times(7 * HOUR, 6 * HOUR)
                .labels(&["docs"])
                .checks(&[("deploy-preview", Success), ("linkcheck", Success)])
                .comment("noah", 6 * HOUR, "@you is the old /v1 docs path still served? Two links point there.")
                .build(),
            // Others' open pull requests.
            PrBuilder::new(now, &rocket, 146, "Bump tokio from 1.43 to 1.44", "deps[bot]")
                .times(11 * HOUR, 11 * HOUR)
                .labels(&["dependencies"])
                .checks(&ci(Success, Success))
                .build(),
            PrBuilder::new(now, &rocket, 139, "WIP: plugin system", "kaan")
                .times(9 * DAY, DAY)
                .draft()
                .conflicting()
                .checks(&ci(Failure, Failure))
                .build(),
            PrBuilder::new(now, &termpaint, 309, "Faster dithering with lookup tables", "noah")
                .times(4 * DAY, 2 * DAY)
                .decision(ReviewDecision::Approved)
                .checks(&ci(Success, Success))
                .build(),
            PrBuilder::new(now, &infra, 79, "Add staging environment", "sora")
                .times(5 * HOUR, 4 * HOUR)
                .reviewers(&["kaan"])
                .checks(&[("plan", Pending)])
                .build(),
            // Closed and merged.
            PrBuilder::new(now, &rocket, 140, "Parallel artifact uploads", me)
                .times(5 * DAY, 3 * DAY)
                .merged(3 * DAY)
                .checks(&ci(Success, Success))
                .build(),
            PrBuilder::new(now, &website, 85, "New landing page hero", "sora")
                .times(8 * DAY, 6 * DAY)
                .merged(6 * DAY)
                .build(),
            PrBuilder::new(now, &termpaint, 300, "Rewrite in Zig", "anon")
                .times(20 * DAY, 19 * DAY)
                .closed(19 * DAY)
                .build(),
            PrBuilder::new(now, &dotfiles, 11, "Add helix config", me)
                .times(10 * DAY, 9 * DAY)
                .merged(9 * DAY)
                .build(),
        ];

        let issues = vec![
            IssueBuilder::new(
                now,
                &rocket,
                57,
                "Uploads fail intermittently with 503",
                "kaan",
            )
            .times(6 * DAY, 25)
            .labels(&["bug"])
            .assignees(&[me])
            .body("Seen in ~2% of CI runs since the storage migration.")
            .comment(
                me,
                2 * DAY,
                "On it, looks like we never retry transient errors.",
            )
            .build(),
            IssueBuilder::new(now, &infra, 31, "Who owns the staging budget?", "sora")
                .times(2 * DAY, 90)
                .labels(&["question"])
                .comment(
                    "sora",
                    90,
                    "@you you set up the billing alerts, any idea who should get them now?",
                )
                .build(),
            IssueBuilder::new(
                now,
                &termpaint,
                280,
                "Colors look washed out in tmux",
                "pixelfan",
            )
            .times(12 * DAY, 3 * DAY)
            .labels(&["bug", "good first issue"])
            .assignees(&[me])
            .body("Probably missing `Tc` in terminal-overrides.")
            .build(),
            IssueBuilder::new(now, &rocket, 60, "Config file in TOML", "noah")
                .times(4 * DAY, 2 * DAY)
                .labels(&["enhancement"])
                .build(),
            IssueBuilder::new(now, &website, 40, "Search doesn't find CLI flags", "elif")
                .times(3 * DAY, DAY)
                .labels(&["bug", "docs"])
                .build(),
            IssueBuilder::new(
                now,
                &rocket,
                61,
                "Windows: colors broken in cmd.exe",
                "winuser",
            )
            .times(DAY, 20 * HOUR)
            .labels(&["bug"])
            .build(),
            IssueBuilder::new(now, &termpaint, 285, "Add a gallery to the README", "mira")
                .times(5 * DAY, 5 * DAY)
                .labels(&["docs", "good first issue"])
                .build(),
            IssueBuilder::new(now, &dotfiles, 3, "Try out a new font", me)
                .times(15 * DAY, 15 * DAY)
                .build(),
            IssueBuilder::new(now, &infra, 28, "Migrate DNS to the new provider", "kaan")
                .times(10 * DAY, 4 * DAY)
                .closed(CloseReason::Completed, 4 * DAY)
                .build(),
            IssueBuilder::new(now, &rocket, 50, "Crash on empty config", "pixelfan")
                .times(14 * DAY, 12 * DAY)
                .labels(&["bug"])
                .closed(CloseReason::Completed, 12 * DAY)
                .build(),
            IssueBuilder::new(now, &termpaint, 270, "Support Windows 7", "retro")
                .times(30 * DAY, 29 * DAY)
                .closed(CloseReason::NotPlanned, 29 * DAY)
                .build(),
            IssueBuilder::new(now, &website, 38, "Typo on pricing page", "elif")
                .times(9 * DAY, 8 * DAY)
                .closed(CloseReason::Duplicate, 8 * DAY)
                .build(),
        ];

        let notifications = notifications(now, host, &pulls, &issues);
        let diffs = pulls
            .iter()
            .map(|(d, diff)| (d.pr.reference.clone(), diff.clone()))
            .collect();

        Self {
            repos,
            pulls: pulls.into_iter().map(|(d, _)| d).collect(),
            issues,
            notifications,
            diffs,
        }
    }
}

fn notifications(
    now: Timestamp,
    host: &Host,
    pulls: &[(PullRequestDetail, String)],
    issues: &[IssueDetail],
) -> Vec<Notification> {
    use NotificationReason as R;

    let pr = |repo: &str, number: u64| {
        pulls
            .iter()
            .map(|(d, _)| &d.pr)
            .find(|pr| pr.reference.repo.full_name() == repo && pr.reference.number == number)
            .map(|pr| {
                (
                    pr.reference.clone(),
                    pr.title.clone(),
                    pr.url.clone(),
                    SubjectKind::PullRequest,
                )
            })
    };
    let issue = |repo: &str, number: u64| {
        issues
            .iter()
            .map(|d| &d.issue)
            .find(|i| i.reference.repo.full_name() == repo && i.reference.number == number)
            .map(|i| {
                (
                    i.reference.clone(),
                    i.title.clone(),
                    i.url.clone(),
                    SubjectKind::Issue,
                )
            })
    };

    // (subject, reason, unread, minutes ago)
    let items = [
        (pr("acme/rocket", 145), R::ReviewRequested, true, 40),
        (pr("oss/termpaint", 311), R::ReviewRequested, true, 9 * HOUR),
        (pr("acme/rocket", 142), R::CiActivity, true, 25),
        (pr("acme/rocket", 142), R::Comment, true, 3 * HOUR),
        (pr("acme/website", 88), R::Author, true, 3 * HOUR),
        (pr("you/dotfiles", 12), R::Author, false, 2 * HOUR),
        (pr("acme/website", 91), R::Mention, true, 6 * HOUR),
        (pr("acme/infra", 77), R::Assign, true, 5 * HOUR),
        (pr("acme/rocket", 146), R::Subscribed, false, 11 * HOUR),
        (pr("oss/termpaint", 309), R::Subscribed, false, 2 * DAY),
        (pr("acme/infra", 79), R::TeamMention, true, 4 * HOUR),
        (pr("acme/rocket", 140), R::StateChange, false, 3 * DAY),
        (issue("acme/rocket", 57), R::Assign, false, 2 * DAY),
        (issue("acme/infra", 31), R::Mention, true, 90),
        (issue("oss/termpaint", 280), R::Assign, true, 3 * DAY),
        (issue("acme/rocket", 61), R::Subscribed, true, 20 * HOUR),
        (issue("acme/website", 40), R::Subscribed, false, DAY),
        (issue("acme/infra", 28), R::StateChange, false, 4 * DAY),
    ];

    let mut out: Vec<Notification> = items
        .into_iter()
        .filter_map(|(subject, reason, unread, minutes)| {
            let (item, title, url, kind) = subject?;
            Some(Notification {
                id: NotificationId(String::new()),
                repo: item.repo.clone(),
                subject: Subject {
                    title,
                    kind,
                    item: Some(item),
                    url: Some(url),
                },
                reason,
                unread,
                updated_at: ago(now, minutes),
            })
        })
        .collect();

    // Subjects that are not pull requests or issues.
    let termpaint = RepoRef::new(host.clone(), "oss", "termpaint");
    let rocket = RepoRef::new(host.clone(), "acme", "rocket");
    out.push(Notification {
        id: NotificationId(String::new()),
        repo: termpaint.clone(),
        subject: Subject {
            title: "v2.3.0".to_owned(),
            kind: SubjectKind::Release,
            item: None,
            url: Some(format!("https://{host}/{termpaint}/releases/tag/v2.3.0")),
        },
        reason: R::Subscribed,
        unread: true,
        updated_at: ago(now, 12 * HOUR),
    });
    out.push(Notification {
        id: NotificationId(String::new()),
        repo: rocket.clone(),
        subject: Subject {
            title: "Potential secret exposed in config/example.env".to_owned(),
            kind: SubjectKind::Other("SecretScanningAlert".to_owned()),
            item: None,
            url: Some(format!("https://{host}/{rocket}/security")),
        },
        reason: R::SecurityAlert,
        unread: true,
        updated_at: ago(now, 50),
    });

    for (index, n) in out.iter_mut().enumerate() {
        n.id = NotificationId(format!("n{}", index + 1));
    }
    out
}

const UPLOAD_DIFF: &str = r#"diff --git a/src/upload.rs b/src/upload.rs
index 3f2a1c4..9b7e0d2 100644
--- a/src/upload.rs
+++ b/src/upload.rs
@@ -1,14 +1,19 @@ use std::time::Duration;
 use std::time::Duration;

-const MAX_RETRIES: u32 = 3;
+const MAX_RETRIES: u32 = 5;
+const BASE_DELAY: Duration = Duration::from_millis(200);

 pub async fn upload(chunk: &[u8]) -> Result<(), UploadError> {
-    for _ in 0..MAX_RETRIES {
+    for attempt in 0..MAX_RETRIES {
         match send(chunk).await {
             Ok(()) => return Ok(()),
-            Err(e) if e.is_transient() => continue,
+            Err(e) if e.is_transient() => {
+                let delay = BASE_DELAY * 2u32.pow(attempt);
+                tracing::warn!(%e, ?delay, "upload failed, retrying");
+                tokio::time::sleep(delay).await;
+            }
             Err(e) => return Err(e),
         }
     }
     Err(UploadError::RetriesExhausted)
 }
@@ -40,4 +45,7 @@ impl UploadError {
     pub fn is_transient(&self) -> bool {
-        matches!(self, Self::Timeout | Self::ServerError(_))
+        matches!(self, Self::Timeout | Self::ServerError(_) | Self::ConnectionReset | Self::TooManyRequests { .. })
     }
+
+    /// How long the server asked us to wait before trying again (429 responses carry a Retry-After header).
+    pub fn retry_after(&self) -> Option<Duration> { match self { Self::TooManyRequests { retry_after } => *retry_after, _ => None } }
 }
diff --git a/tests/upload.rs b/tests/upload.rs
new file mode 100644
index 0000000..5e6f7a8
--- /dev/null
+++ b/tests/upload.rs
@@ -0,0 +1,6 @@
+#[tokio::test]
+async fn retries_transient_errors() {
+    let server = FlakyServer::failing(2);
+    upload_to(&server, b"hello").await.unwrap();
+    assert_eq!(server.attempts(), 3);
+}
"#;

const STATUS_DIFF: &str = r#"diff --git a/src/cli/status.rs b/src/cli/status.rs
index 7c8d9e0..1f2a3b4 100644
--- a/src/cli/status.rs
+++ b/src/cli/status.rs
@@ -8,9 +8,16 @@ use crate::service::Service;
 pub struct StatusArgs {
     /// Show all services, including stopped ones.
     #[arg(long)]
     pub all: bool,
+    /// Print machine-readable JSON instead of a table.
+    #[arg(long)]
+    pub json: bool,
 }

 pub fn run(args: &StatusArgs, services: &[Service]) {
-    print_table(services, args.all);
+    if args.json {
+        println!("{}", serde_json::to_string_pretty(services).unwrap_or_default());
+    } else {
+        print_table(services, args.all);
+    }
 }
"#;

const THEME_DIFF: &str = r#"diff --git a/styles/theme.css b/styles/theme.css
index 1a2b3c4..5d6e7f8 100644
--- a/styles/theme.css
+++ b/styles/theme.css
@@ -1,4 +1,13 @@
 :root {
   --bg: #ffffff;
   --fg: #1a1a2e;
 }
+
+@media (prefers-color-scheme: dark) {
+  :root {
+    --bg: #140f24;
+    --fg: #e6e1ff;
+    --accent: #ff2e97;
+  }
+}
+
"#;

const PROMPT_DIFF: &str = r#"diff --git a/shell/prompt.zsh b/shell/starship.zsh
similarity index 60%
rename from shell/prompt.zsh
rename to shell/starship.zsh
index 0a1b2c3..4d5e6f7 100644
--- a/shell/prompt.zsh
+++ b/shell/starship.zsh
@@ -1,3 +1,2 @@
-autoload -Uz vcs_info
-PROMPT='%F{magenta}%~%f $ '
+eval "$(starship init zsh)"
 export EDITOR=hx
"#;
