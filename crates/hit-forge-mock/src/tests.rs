use std::collections::HashSet;

use hit_core::{
    Forge, ForgeError, ItemKind,
    focus::{FocusReason, FocusSubject},
    query::{IssueQuery, NotificationQuery, PageRequest, PrQuery, Scope, StateFilter},
};

use super::*;

fn forge() -> MockForge {
    MockForge::demo(MockOptions::instant())
}

fn caps(
    forge: &MockForge,
) -> (
    &dyn PullRequests,
    &dyn Issues,
    &dyn Notifications,
    &dyn Focus,
) {
    (
        forge.pull_requests().unwrap(),
        forge.issues().unwrap(),
        forge.notifications().unwrap(),
        forge.focus().unwrap(),
    )
}

#[test]
fn every_diff_parses_and_matches_pr_stats() {
    let forge = forge();
    let data = forge.data();
    for detail in &data.pulls {
        let text = &data.diffs[&detail.pr.reference];
        let diff =
            Diff::parse_unified(text).unwrap_or_else(|e| panic!("{}: {e}", detail.pr.reference));
        assert!(!diff.files.is_empty(), "{}", detail.pr.reference);
        assert_eq!(detail.pr.additions, Some(diff.additions()));
        assert_eq!(detail.pr.deletions, Some(diff.deletions()));
    }
}

#[test]
fn notifications_point_at_existing_items() {
    let forge = forge();
    let data = forge.data();
    let items: HashSet<_> = data
        .pulls
        .iter()
        .map(|d| d.pr.reference.clone())
        .chain(data.issues.iter().map(|d| d.issue.reference.clone()))
        .collect();
    assert_eq!(
        data.notifications.len(),
        20,
        "a notification subject was not found"
    );
    for n in &data.notifications {
        if let Some(item) = &n.subject.item {
            assert!(items.contains(item), "{item}");
            assert_eq!(&item.repo, &n.repo);
        }
    }
    let ids: HashSet<_> = data.notifications.iter().map(|n| &n.id).collect();
    assert_eq!(ids.len(), data.notifications.len(), "ids must be unique");
}

#[test]
fn models_roundtrip_through_serde() {
    let forge = forge();
    let data = forge.data();
    for detail in &data.pulls {
        let json = serde_json::to_string(detail).unwrap();
        assert_eq!(
            &serde_json::from_str::<PullRequestDetail>(&json).unwrap(),
            detail
        );
    }
    for detail in &data.issues {
        let json = serde_json::to_string(detail).unwrap();
        assert_eq!(&serde_json::from_str::<IssueDetail>(&json).unwrap(), detail);
    }
    for n in &data.notifications {
        let json = serde_json::to_string(n).unwrap();
        assert_eq!(&serde_json::from_str::<Notification>(&json).unwrap(), n);
    }
}

#[tokio::test]
async fn focus_covers_every_reason() {
    let forge = forge();
    let (.., focus) = caps(&forge);
    let items = focus.items(&Scope::All).await.unwrap();
    let reasons: HashSet<_> = items
        .iter()
        .flat_map(|i| i.reasons.iter().copied())
        .collect();
    for reason in [
        FocusReason::ReviewRequested,
        FocusReason::ChangesRequested,
        FocusReason::ChecksFailing,
        FocusReason::ReadyToMerge,
        FocusReason::Mentioned,
        FocusReason::Assigned,
    ] {
        assert!(reasons.contains(&reason), "{reason:?} missing");
    }
    assert!(items.windows(2).all(|w| w[0].updated_at >= w[1].updated_at));
    // Closed and merged items never need attention.
    assert!(items.iter().all(|i| match &i.subject {
        FocusSubject::PullRequest(pr) => pr.is_open(),
        FocusSubject::Issue(issue) => issue.is_open(),
    }));
}

#[tokio::test]
async fn scope_limits_results_to_one_repo() {
    let forge = forge();
    let (pulls, _, notifications, focus) = caps(&forge);
    let repo = RepoRef::new(Host::new(DEMO_HOST), "acme", "rocket");
    let scope = Scope::Repo(repo.clone());

    let query = PrQuery {
        scope: scope.clone(),
        state: StateFilter::All,
        ..PrQuery::default()
    };
    let page = pulls.list(&query, &PageRequest::first(100)).await.unwrap();
    assert!(!page.items.is_empty());
    assert!(page.items.iter().all(|pr| pr.reference.repo == repo));

    let query = NotificationQuery {
        scope: scope.clone(),
        ..NotificationQuery::default()
    };
    let page = notifications
        .list(&query, &PageRequest::first(100))
        .await
        .unwrap();
    assert!(page.items.iter().all(|n| n.repo == repo));

    let items = focus.items(&scope).await.unwrap();
    assert!(items.iter().all(|i| i.subject.reference().repo == repo));
}

#[tokio::test]
async fn pagination_visits_every_item_once() {
    let forge = forge();
    let (_, issues, ..) = caps(&forge);
    let query = IssueQuery {
        state: StateFilter::All,
        ..IssueQuery::default()
    };
    let mut seen = Vec::new();
    let mut page = PageRequest::first(5);
    loop {
        let result = issues.list(&query, &page).await.unwrap();
        assert!(result.items.len() <= 5);
        seen.extend(result.items.into_iter().map(|i| i.reference));
        match result.next {
            Some(cursor) => page = PageRequest::after(cursor, 5),
            None => break,
        }
    }
    let unique: HashSet<_> = seen.iter().collect();
    assert_eq!(seen.len(), forge.data().issues.len());
    assert_eq!(unique.len(), seen.len());
}

#[tokio::test]
async fn filters_by_state_author_and_search() {
    let forge = forge();
    let (pulls, issues, ..) = caps(&forge);

    let mine = PrQuery {
        author: Some(DEMO_USER.to_owned()),
        ..PrQuery::default()
    };
    let page = pulls.list(&mine, &PageRequest::first(100)).await.unwrap();
    assert!(!page.items.is_empty());
    assert!(
        page.items
            .iter()
            .all(|pr| pr.author.login == DEMO_USER && pr.is_open())
    );

    let closed = IssueQuery {
        state: StateFilter::Closed,
        ..IssueQuery::default()
    };
    let page = issues
        .list(&closed, &PageRequest::first(100))
        .await
        .unwrap();
    assert!(!page.items.is_empty());
    assert!(page.items.iter().all(|i| !i.is_open()));

    let search = PrQuery {
        search: Some("DARK MODE".to_owned()),
        ..PrQuery::default()
    };
    let page = pulls.list(&search, &PageRequest::first(100)).await.unwrap();
    assert_eq!(page.items.len(), 1);
}

#[tokio::test]
async fn mark_read_changes_state() {
    let forge = forge();
    let (.., notifications, _) = caps(&forge);
    let unread = NotificationQuery {
        unread_only: true,
        ..NotificationQuery::default()
    };
    let before = notifications
        .list(&unread, &PageRequest::first(100))
        .await
        .unwrap();
    let first = before.items[0].id.clone();

    notifications.mark_read(&first).await.unwrap();
    let after = notifications
        .list(&unread, &PageRequest::first(100))
        .await
        .unwrap();
    assert_eq!(after.items.len(), before.items.len() - 1);

    notifications.mark_all_read(&Scope::All).await.unwrap();
    let after = notifications
        .list(&unread, &PageRequest::first(100))
        .await
        .unwrap();
    assert!(after.items.is_empty());

    let missing = NotificationId("nope".into());
    assert_eq!(
        notifications.mark_read(&missing).await,
        Err(ForgeError::NotFound)
    );
}

#[tokio::test]
async fn get_and_diff_lookups() {
    let forge = forge();
    let (pulls, issues, ..) = caps(&forge);
    let repo = RepoRef::new(Host::new(DEMO_HOST), "acme", "rocket");

    let detail = pulls
        .get(&ItemRef::pull_request(repo.clone(), 142))
        .await
        .unwrap();
    assert_eq!(detail.pr.reference.kind, ItemKind::PullRequest);
    assert!(!detail.comments.is_empty());

    let diff = pulls
        .diff(&ItemRef::pull_request(repo.clone(), 142))
        .await
        .unwrap();
    assert_eq!(diff.files.len(), 2);

    assert_eq!(
        pulls.get(&ItemRef::pull_request(repo.clone(), 9999)).await,
        Err(ForgeError::NotFound)
    );
    assert!(issues.get(&ItemRef::issue(repo, 57)).await.is_ok());
}

#[tokio::test]
async fn failure_rate_one_always_fails() {
    let forge = MockForge::demo(MockOptions {
        failure_rate: 1.0,
        ..MockOptions::instant()
    });
    let (.., focus) = caps(&forge);
    assert!(matches!(
        focus.items(&Scope::All).await,
        Err(ForgeError::Network(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn latency_is_simulated() {
    let forge = MockForge::demo(MockOptions::default());
    let (.., focus) = caps(&forge);
    let start = tokio::time::Instant::now();
    focus.items(&Scope::All).await.unwrap();
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(150) && elapsed < Duration::from_millis(600));
}
