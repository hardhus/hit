//! Stale-while-revalidate loading and refresh loops.
//!
//! Each loop first sends what the cache has (marked not fresh), then what the
//! forge returns. On failure the view keeps showing the last data and says
//! how old it is. Loops end when their task is aborted (the view went away
//! or the user asked to refresh).

use std::{future::Future, sync::Arc, time::Duration};

use hit_cache::{Cache, Vault};
use hit_core::{
    Forge, ForgeError, ItemRef, Notifications, Timestamp,
    model::{Notification, NotificationId, PullRequest, PullRequestDetail},
    query::{NotificationQuery, PageRequest, PrQuery, Scope},
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{Action, DataEvent, Theme, highlight::highlight_diff};

/// A value and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded<T> {
    pub value: T,
    pub fetched_at: Timestamp,
    /// `false` when it came from the cache and has not been revalidated yet.
    pub fresh: bool,
}

/// Fallback when the forge does not say how often to poll.
const DEFAULT_POLL: Duration = Duration::from_secs(60);
/// Upper bound on notification pages fetched just to count unread ones.
const MAX_UNREAD_PAGES: usize = 10;

const FOCUS_KEY: &str = "focus:all";
const UNREAD_KEY: &str = "unread:all";

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
        Err(e) => send(Err(e)),
    }
}

/// Action Required, every `every` (once if `None`).
pub fn spawn_focus(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    every: Option<Duration>,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let send = |result: Result<Loaded<Vec<_>>, ForgeError>| {
            let result = result.map(|l| Loaded {
                value: Arc::new(l.value),
                fetched_at: l.fetched_at,
                fresh: l.fresh,
            });
            let _ = tx.send(Action::Data(DataEvent::Focus {
                account: account.clone(),
                result,
            }));
        };
        let mut first = true;
        loop {
            let _ = tx.send(Action::Data(DataEvent::Refreshing(account.clone())));
            let fetch = || async {
                match forge.focus() {
                    Some(focus) => focus.items(&Scope::All).await,
                    None => Err(ForgeError::Unsupported("Action Required")),
                }
            };
            load(&store, FOCUS_KEY, first, fetch, &send).await;
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
    poll: bool,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let Some(_) = forge.notifications() else {
            return;
        };
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let send = |result| {
            let _ = tx.send(Action::Data(DataEvent::UnreadCount {
                account: account.clone(),
                result,
            }));
        };
        let mut first = true;
        loop {
            let fetch = || async {
                match forge.notifications() {
                    Some(notifications) => count_unread(notifications).await,
                    None => Ok(0),
                }
            };
            load(&store, UNREAD_KEY, first, fetch, &send).await;
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

async fn count_unread(notifications: &dyn Notifications) -> Result<usize, ForgeError> {
    let query = NotificationQuery {
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

/// The inbox, at the forge's poll interval (once if `poll` is off). Unread
/// and all notifications are cached separately.
pub fn spawn_inbox(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
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
        let key = if unread_only {
            "inbox:unread"
        } else {
            "inbox:all"
        };
        let send = |result: Result<Loaded<Vec<Notification>>, ForgeError>| {
            let result = result.map(|l| Loaded {
                value: Arc::new(l.value),
                fetched_at: l.fetched_at,
                fresh: l.fresh,
            });
            let _ = tx.send(Action::Data(DataEvent::Notifications {
                account: account.clone(),
                unread_only,
                result,
            }));
        };
        let mut first = true;
        loop {
            let fetch = || async {
                match forge.notifications() {
                    Some(notifications) => fetch_inbox(notifications, unread_only).await,
                    None => Ok(Vec::new()),
                }
            };
            load(&store, key, first, fetch, &send).await;
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
    unread_only: bool,
) -> Result<Vec<Notification>, ForgeError> {
    let query = NotificationQuery {
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

/// One page of pull requests for a list view (cached per view).
pub fn spawn_pulls(
    forge: Arc<dyn Forge>,
    cache: Option<Arc<Cache>>,
    view: &'static str,
    query: PrQuery,
    tx: mpsc::UnboundedSender<Action>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let store = store_for(cache, forge.as_ref());
        let account = forge.account().clone();
        let send = |result: Result<Loaded<Vec<PullRequest>>, ForgeError>| {
            let _ = tx.send(Action::Data(DataEvent::Pulls {
                account: account.clone(),
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
                Some(pulls) => pulls
                    .list(&query, &PageRequest::first(50))
                    .await
                    .map(|page| page.items),
                None => Err(ForgeError::Unsupported("pull requests")),
            }
        };
        load(&store, &format!("pulls:{view}"), true, fetch, &send).await;
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
        spawn_focus(Arc::clone(&forge), Some(Arc::clone(&cache)), None, tx)
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
        spawn_unread(demo(), Some(Arc::clone(&cache)), false, tx.clone())
            .await
            .unwrap();
        let _ = focus_events(&mut rx, 1).await;

        let failing: Arc<dyn Forge> = Arc::new(MockForge::demo(MockOptions {
            failure_rate: 1.0,
            ..MockOptions::instant()
        }));
        spawn_unread(failing, Some(cache), false, tx).await.unwrap();
        let events = focus_events(&mut rx, 2).await;
        assert!(matches!(&events[0], DataEvent::UnreadCount { result: Ok(l), .. } if !l.fresh));
        assert!(matches!(
            &events[1],
            DataEvent::UnreadCount { result: Err(_), .. }
        ));
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
