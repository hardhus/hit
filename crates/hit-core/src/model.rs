//! Domain models shared by every forge.
//!
//! Forge-specific details are typed `Option` fields: a forge that does not
//! know a value leaves it `None`. All models are serializable so the cache can
//! store them as-is.

mod check;
mod comment;
mod issue;
mod label;
mod notification;
mod pull;
mod repo;
mod review;
mod user;

pub use check::{CheckRun, CheckState, CheckSummary};
pub use comment::Comment;
pub use issue::{CloseReason, Issue, IssueDetail, IssueState};
pub use label::{Label, Rgb};
pub use notification::{Notification, NotificationId, NotificationReason, Subject, SubjectKind};
pub use pull::{Mergeable, PrState, PullRequest, PullRequestDetail, ReviewDecision};
pub use repo::Repo;
pub use review::{Review, ReviewState};
pub use user::User;
