//! Provider tests against a mock GitHub. All data is fictional (audit
//! rules.md): no real accounts, tokens or repositories.

use hit_auth::Secret;
use hit_core::{AccessHint, Host, model::CheckState};
use serde_json::{Value, json};
use url::Url;
use wiremock::{
    Mock, MockServer, Request, ResponseTemplate,
    matchers::{body_string_contains, header, method, path, query_param},
};

use super::*;
use crate::{ApiOptions, GithubHost};

const ME: &str = "octo";

fn forge(server: &MockServer, options: ApiOptions, focus: FocusOptions) -> GithubForge {
    let base = Url::parse(&server.uri()).unwrap();
    let api = Api::new(
        hit_auth::client().unwrap(),
        GithubHost::for_tests("github.com", &base),
        Secret::new("gho_fictional").unwrap(),
        options,
    );
    GithubForge::new(AccountId::new(Host::new("github.com"), ME), api, focus)
}

fn default_forge(server: &MockServer) -> GithubForge {
    forge(server, ApiOptions::default(), FocusOptions::default())
}

fn rocket() -> RepoRef {
    RepoRef::new(Host::new("github.com"), "acme", "rocket")
}

fn pr_json(repo: &str, number: u64, author: &str, extra: Value) -> Value {
    let (owner, name) = repo.split_once('/').unwrap();
    let mut pr = json!({
        "__typename": "PullRequest",
        "number": number,
        "title": format!("PR {number}"),
        "url": format!("https://github.com/{repo}/pull/{number}"),
        "state": "OPEN",
        "isDraft": false,
        "createdAt": "2026-10-01T10:00:00Z",
        "updatedAt": format!("2026-10-2{}T10:00:00Z", number % 10),
        "closedAt": null,
        "mergedAt": null,
        "author": { "__typename": "User", "login": author },
        "repository": { "name": name, "owner": { "login": owner } },
        "labels": { "nodes": [{ "name": "bug", "color": "ff4d6d", "description": "" }] },
        "assignees": { "nodes": [] },
        "reviewRequests": { "nodes": [
            { "requestedReviewer": { "__typename": "Team", "combinedSlug": "acme/core" } },
            { "requestedReviewer": null }
        ] },
        "reviewDecision": "REVIEW_REQUIRED",
        "mergeable": "MERGEABLE",
        "additions": 10,
        "deletions": 2,
        "changedFiles": 1,
        "comments": { "totalCount": 3 },
        "headRefName": "feature",
        "baseRefName": "main",
        "commits": { "nodes": [{ "commit": { "statusCheckRollup": {
            "state": "SUCCESS",
            "contexts": {
                "checkRunCountsByState": [{ "state": "SUCCESS", "count": 3 }],
                "statusContextCountsByState": []
            }
        } } }] }
    });
    if let (Some(pr), Some(extra)) = (pr.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            pr.insert(k.clone(), v.clone());
        }
    }
    pr
}

fn issue_json(repo: &str, number: u64, author: &str) -> Value {
    let (owner, name) = repo.split_once('/').unwrap();
    json!({
        "__typename": "Issue",
        "number": number,
        "title": format!("Issue {number}"),
        "url": format!("https://github.com/{repo}/issues/{number}"),
        "state": "OPEN",
        "stateReason": null,
        "createdAt": "2026-10-01T10:00:00Z",
        "updatedAt": format!("2026-10-2{}T10:00:00Z", number % 10),
        "closedAt": null,
        "author": { "__typename": "User", "login": author },
        "repository": { "name": name, "owner": { "login": owner } },
        "labels": { "nodes": [] },
        "assignees": { "nodes": [{ "login": ME }] },
        "comments": { "totalCount": 0 }
    })
}

async fn graphql_reply(server: &MockServer, data: Value) {
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "data": data })))
        .mount(server)
        .await;
}

fn body(request: &Request) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

#[tokio::test]
async fn focus_merges_categories() {
    let server = MockServer::start().await;
    let review = pr_json("acme/rocket", 1, "mira", json!({}));
    let mine = pr_json(
        "acme/rocket",
        2,
        ME,
        json!({
            "reviewDecision": "CHANGES_REQUESTED",
            "commits": { "nodes": [{ "commit": { "statusCheckRollup": {
                "state": "FAILURE",
                "contexts": {
                    "checkRunCountsByState": [
                        { "state": "SUCCESS", "count": 2 },
                        { "state": "FAILURE", "count": 1 }
                    ],
                    "statusContextCountsByState": [{ "state": "PENDING", "count": 1 }]
                }
            } } }] }
        }),
    );
    let mentioned = issue_json("acme/infra", 3, "sora");
    let own_issue = issue_json("acme/infra", 4, ME);
    graphql_reply(
        &server,
        json!({
            "reviews": { "nodes": [review.clone()] },
            "mine": { "nodes": [mine, null] },
            "mentions": { "nodes": [mentioned.clone(), review, own_issue] },
            "assigned": { "nodes": [mentioned, { "__typename": "Discussion" }] }
        }),
    )
    .await;

    let items = default_forge(&server).items(&Scope::All).await.unwrap();
    let find = |n: u64| items.iter().find(|i| i.subject.reference().number == n);

    assert_eq!(
        find(1).unwrap().reasons,
        [FocusReason::ReviewRequested, FocusReason::Mentioned]
    );
    let two = find(2).unwrap();
    assert_eq!(
        two.reasons,
        [FocusReason::ChangesRequested, FocusReason::ChecksFailing]
    );
    let FocusSubject::PullRequest(pr) = &two.subject else {
        panic!("expected a pull request");
    };
    let checks = pr.checks.unwrap();
    assert_eq!(
        (checks.state, checks.passed, checks.failed, checks.pending),
        (CheckState::Failure, 2, 1, 1)
    );
    assert_eq!(
        find(3).unwrap().reasons,
        [FocusReason::Mentioned, FocusReason::Assigned]
    );
    // Mentions in your own items are not "needs you".
    assert!(find(4).is_none());
    assert!(items.windows(2).all(|w| w[0].updated_at >= w[1].updated_at));

    let request = &server.received_requests().await.unwrap()[0];
    let vars = &body(request)["variables"];
    assert_eq!(
        vars["reviews"],
        "is:open archived:false is:pr review-requested:octo"
    );
    assert_eq!(vars["mine"], "is:open archived:false is:pr author:octo");
    assert!(
        vars["mentions"]
            .as_str()
            .unwrap()
            .contains("mentions:octo updated:>=")
    );
}

#[tokio::test]
async fn focus_categories_can_be_disabled_and_scoped() {
    let server = MockServer::start().await;
    graphql_reply(&server, json!({ "reviews": { "nodes": [] } })).await;
    let focus = FocusOptions {
        mentions: false,
        assigned: false,
        my_pull_requests: false,
        ..FocusOptions::default()
    };
    let forge = forge(&server, ApiOptions::default(), focus);
    let items = forge.items(&Scope::Repo(rocket())).await.unwrap();
    assert!(items.is_empty());

    let request = &server.received_requests().await.unwrap()[0];
    let vars = &body(request)["variables"];
    assert_eq!(vars["withReviews"], true);
    assert_eq!(vars["withMentions"], false);
    assert_eq!(vars["withAssigned"], false);
    assert_eq!(vars["withMine"], false);
    assert!(
        vars["reviews"]
            .as_str()
            .unwrap()
            .ends_with("repo:acme/rocket")
    );
}

#[tokio::test]
async fn pull_request_list_builds_the_search() {
    let server = MockServer::start().await;
    graphql_reply(
        &server,
        json!({ "search": {
            "pageInfo": { "hasNextPage": true, "endCursor": "Y3Vyc29y" },
            "nodes": [pr_json("acme/rocket", 7, "mira", json!({})), issue_json("acme/rocket", 8, "mira")]
        } }),
    )
    .await;

    let query = PrQuery {
        scope: Scope::Repo(rocket()),
        author: Some("mira".into()),
        search: Some("dark \"mode\"\n".into()),
        ..PrQuery::default()
    };
    let page = PullRequests::list(&default_forge(&server), &query, &PageRequest::first(20))
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1, "issues are filtered out");
    let pr = &page.items[0];
    assert_eq!(pr.reference, ItemRef::pull_request(rocket(), 7));
    assert_eq!(pr.requested_reviewers[0].login, "acme/core");
    assert_eq!(pr.labels[0].name, "bug");
    assert_eq!(pr.comments, 3);
    assert_eq!(page.next, Some(Cursor("Y3Vyc29y".into())));

    let request = &server.received_requests().await.unwrap()[0];
    let vars = &body(request)["variables"];
    assert_eq!(
        vars["q"],
        "is:pr archived:false is:open repo:acme/rocket author:mira sort:updated-desc dark  mode"
    );
    assert_eq!(vars["first"], 20);
}

#[tokio::test]
async fn invalid_inputs_never_reach_the_network() {
    let server = MockServer::start().await;
    let forge = default_forge(&server);
    let evil = RepoRef::new(Host::new("github.com"), "acme", "../../user");
    assert!(
        PullRequests::get(&forge, &ItemRef::pull_request(evil.clone(), 1))
            .await
            .is_err()
    );
    assert!(forge.diff(&ItemRef::pull_request(evil, 1)).await.is_err());
    let query = PrQuery {
        author: Some("x author:someone".into()),
        ..PrQuery::default()
    };
    assert!(
        PullRequests::list(&forge, &query, &PageRequest::default())
            .await
            .is_err()
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn pull_request_detail() {
    let server = MockServer::start().await;
    let mut pr = pr_json("acme/rocket", 7, "mira", json!({}));
    let extra = json!({
        "body": "Adds **dark mode**.",
        "thread": { "nodes": [{
            "id": "IC_1", "body": "LGTM", "createdAt": "2026-10-02T10:00:00Z",
            "updatedAt": "2026-10-02T10:00:00Z", "author": null
        }] },
        "reviews": { "nodes": [{
            "state": "APPROVED", "body": "", "submittedAt": "2026-10-03T10:00:00Z",
            "author": { "__typename": "Bot", "login": "ci-bot" }
        }] },
        "checks": { "nodes": [{ "commit": { "statusCheckRollup": { "contexts": { "nodes": [
            { "__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE", "detailsUrl": "https://example.invalid/1" },
            { "__typename": "CheckRun", "name": "lint", "status": "IN_PROGRESS", "conclusion": null, "detailsUrl": null },
            { "__typename": "StatusContext", "context": "deploy", "state": "SUCCESS", "targetUrl": null }
        ] } } } }] }
    });
    for (k, v) in extra.as_object().unwrap() {
        pr.as_object_mut().unwrap().insert(k.clone(), v.clone());
    }
    graphql_reply(&server, json!({ "repository": { "pullRequest": pr } })).await;

    let detail = PullRequests::get(&default_forge(&server), &ItemRef::pull_request(rocket(), 7))
        .await
        .unwrap();
    assert_eq!(detail.body, "Adds **dark mode**.");
    assert_eq!(detail.comments[0].author.login, "ghost");
    assert!(detail.reviews[0].author.is_bot);
    let states: Vec<_> = detail.checks.iter().map(|c| c.state).collect();
    assert_eq!(
        states,
        [
            CheckState::Failure,
            CheckState::Pending,
            CheckState::Success
        ]
    );
}

#[tokio::test]
async fn missing_items_and_graphql_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_string_contains("query Issue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "type": "NOT_FOUND", "message": "Could not resolve to a Repository" }]
        })))
        .mount(&server)
        .await;
    graphql_reply(&server, json!({ "repository": { "pullRequest": null } })).await;
    let forge = default_forge(&server);
    assert_eq!(
        Issues::get(&forge, &ItemRef::issue(rocket(), 1)).await,
        Err(ForgeError::NotFound)
    );
    assert_eq!(
        PullRequests::get(&forge, &ItemRef::pull_request(rocket(), 1)).await,
        Err(ForgeError::NotFound)
    );
}

#[tokio::test]
async fn diff_uses_the_diff_media_type() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/repos/acme/rocket/pulls/7"))
        .and(header("accept", "application/vnd.github.diff"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-x\n+y\n",
        ))
        .mount(&server)
        .await;
    let diff = default_forge(&server)
        .diff(&ItemRef::pull_request(rocket(), 7))
        .await
        .unwrap();
    assert_eq!((diff.additions(), diff.deletions()), (1, 1));
}

fn notification_json(id: &str, kind: &str, number: Option<u64>, reason: &str) -> Value {
    json!({
        "id": id,
        "unread": true,
        "reason": reason,
        "updated_at": "2026-10-28T10:00:00Z",
        "subject": {
            "title": format!("subject {id}"),
            "url": number.map(|n| format!("https://api.github.com/repos/acme/rocket/{}/{n}",
                if kind == "PullRequest" { "pulls" } else { "issues" })),
            "type": kind
        },
        "repository": {
            "name": "rocket", "owner": { "login": "acme" },
            "html_url": "https://github.com/acme/rocket"
        }
    })
}

#[tokio::test]
async fn notifications_list_and_paging() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/notifications"))
        .and(query_param("all", "false"))
        .and(query_param("page", "1"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "link",
                    "<https://api.github.com/notifications?all=false&page=2>; rel=\"next\", \
                     <https://api.github.com/notifications?all=false&page=5>; rel=\"last\"",
                )
                .set_body_json(json!([
                    notification_json("11", "PullRequest", Some(7), "review_requested"),
                    notification_json("12", "Issue", Some(8), "mention"),
                    notification_json("13", "Release", None, "subscribed"),
                    notification_json("14", "Issue", Some(9), "brand_new_reason"),
                ])),
        )
        .mount(&server)
        .await;

    let query = NotificationQuery {
        unread_only: true,
        ..NotificationQuery::default()
    };
    let page = Notifications::list(&default_forge(&server), &query, &PageRequest::first(50))
        .await
        .unwrap();
    assert_eq!(page.next, Some(Cursor("2".into())));
    let [pr, issue, release, other] = page.items.as_slice() else {
        panic!("expected four notifications");
    };
    assert_eq!(pr.subject.item, Some(ItemRef::pull_request(rocket(), 7)));
    assert_eq!(
        pr.subject.url.as_deref(),
        Some(format!("{}acme/rocket/pull/7", server.uri() + "/").as_str())
    );
    assert_eq!(issue.subject.item, Some(ItemRef::issue(rocket(), 8)));
    assert_eq!(release.subject.item, None);
    assert_eq!(
        other.reason,
        hit_core::model::NotificationReason::Other("brand_new_reason".into())
    );
}

#[tokio::test]
async fn mark_read_respects_read_only() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/notifications/threads/11"))
        .respond_with(ResponseTemplate::new(205))
        .mount(&server)
        .await;
    let id = NotificationId("11".into());

    let read_only = ApiOptions {
        read_only: true,
        allow_mark_read: false,
    };
    let forge_ro = forge(&server, read_only, FocusOptions::default());
    assert!(matches!(
        forge_ro.mark_read(&id).await,
        Err(ForgeError::ReadOnly(_))
    ));
    assert!(server.received_requests().await.unwrap().is_empty());

    let allowed = ApiOptions {
        read_only: true,
        allow_mark_read: true,
    };
    forge(&server, allowed, FocusOptions::default())
        .mark_read(&id)
        .await
        .unwrap();
    default_forge(&server).mark_read(&id).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert!(
        default_forge(&server)
            .mark_read(&NotificationId("../user".into()))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn sso_partial_results_keep_data() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "reviews": { "nodes": [pr_json("acme/rocket", 1, "mira", json!({}))] } },
            "errors": [{ "type": "FORBIDDEN", "message": "Resource protected by organization SAML enforcement." }]
        })))
        .mount(&server)
        .await;
    let focus = FocusOptions {
        my_pull_requests: false,
        mentions: false,
        assigned: false,
        ..FocusOptions::default()
    };
    let items = forge(&server, ApiOptions::default(), focus)
        .items(&Scope::All)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    let _ = AccessHint::SsoPartial { org_ids: vec![] };
}

#[tokio::test]
async fn notifications_revalidate_with_etag() {
    let server = MockServer::start().await;
    // First mounted mock wins: the 304 only matches revalidations.
    Mock::given(method("GET"))
        .and(path("/api/notifications"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(ResponseTemplate::new(304).insert_header("x-poll-interval", "120"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/notifications"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("etag", "\"v1\"")
                .insert_header("x-poll-interval", "60")
                .set_body_json(json!([notification_json(
                    "11",
                    "PullRequest",
                    Some(7),
                    "review_requested"
                )])),
        )
        .mount(&server)
        .await;

    let forge = default_forge(&server);
    assert_eq!(Notifications::poll_interval(&forge), None);
    let query = NotificationQuery::default();
    let first = Notifications::list(&forge, &query, &PageRequest::first(50))
        .await
        .unwrap();
    assert_eq!(
        Notifications::poll_interval(&forge),
        Some(std::time::Duration::from_secs(60))
    );
    let second = Notifications::list(&forge, &query, &PageRequest::first(50))
        .await
        .unwrap();
    assert_eq!(first.items, second.items, "304 reuses the stored body");
    assert_eq!(
        Notifications::poll_interval(&forge),
        Some(std::time::Duration::from_secs(120))
    );

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].headers.get("if-none-match").is_none());
    assert_eq!(requests[1].headers.get("if-none-match").unwrap(), "\"v1\"");
}
