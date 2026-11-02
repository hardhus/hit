use std::{path::PathBuf, sync::Arc, time::Instant};

use hit_cache::Cache;
use hit_config::{Config, InboxConfig, RefreshConfig, SortOrder};
use hit_core::{AccountId, Forge, Forges, RepoRef, auth::LoginFlow, query::Scope};
use ratatui::{Frame, layout::Rect, style::Color};
use tokio::sync::mpsc;

use crate::{Action, Event, Theme, keymap::Keymap};

/// What the TUI is connected to, built by the binary.
#[derive(Default)]
pub struct Services {
    /// Connected forge accounts (the demo forge with `--demo`).
    pub forges: Forges,
    /// Sign-in from inside the TUI, when available.
    pub login: Option<Arc<dyn LoginFlow>>,
    /// Local cache for instant start; `None` when disabled (and in the demo).
    pub cache: Option<Arc<Cache>>,
    /// The repository to open on (detected from the current folder or asked
    /// for with `--repo`).
    pub repo: Option<RepoRef>,
    /// Where settings are saved; `None` keeps them in memory (the demo).
    pub config_file: Option<PathBuf>,
}

/// Preferences views read; from the config, some adjustable at runtime.
#[derive(Debug, Clone)]
pub struct Prefs {
    pub refresh: RefreshConfig,
    pub focus_sort: SortOrder,
    pub focus_group: bool,
    pub inbox: InboxConfig,
}

impl Prefs {
    pub fn from_config(config: &Config) -> Self {
        Self {
            refresh: config.refresh.clone(),
            focus_sort: config.focus.sort,
            focus_group: config.focus.group,
            inbox: config.inbox.clone(),
        }
    }
}

/// Shared context handed to every view. Only the app mutates it.
pub struct Ctx {
    pub theme: Theme,
    pub animations: bool,
    pub forges: Forges,
    pub login: Option<Arc<dyn LoginFlow>>,
    pub cache: Option<Arc<Cache>>,
    pub prefs: Prefs,
    pub keymap: Keymap,
    /// The settings as they are now.
    pub config: Config,
    /// The settings hit started with: what is not applied live still works
    /// this way.
    pub started_config: Config,
    pub config_file: Option<PathBuf>,
    /// Only this account is shown; `None` shows all (audit D-039). Future
    /// profiles will set this instead of views knowing about accounts.
    pub account_filter: Option<AccountId>,
    /// What every tab shows: all repositories or one (audit D-045).
    pub scope: Scope,
    /// The repository hit was opened on; pinned first in the Repos tab.
    pub home_repo: Option<RepoRef>,
    started: Instant,
    tx: mpsc::UnboundedSender<Action>,
}

impl Ctx {
    pub fn new(
        config: &Config,
        services: Services,
        keymap: Keymap,
        tx: mpsc::UnboundedSender<Action>,
    ) -> Self {
        let scope = services.repo.clone().map_or(Scope::All, Scope::Repo);
        Self {
            theme: Theme::from_config(&config.ui),
            animations: config.ui.animations,
            forges: services.forges,
            login: services.login,
            cache: services.cache,
            prefs: Prefs::from_config(config),
            keymap,
            config: config.clone(),
            started_config: config.clone(),
            config_file: services.config_file,
            account_filter: None,
            scope,
            home_repo: services.repo,
            started: Instant::now(),
            tx,
        }
    }

    /// A context for view tests: no animations, default settings.
    #[cfg(test)]
    pub(crate) fn for_tests(services: Services) -> (Self, mpsc::UnboundedReceiver<Action>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let config = Config::default();
        let mut ctx = Self::new(&config, services, Keymap::default(), tx);
        ctx.theme = Theme::jinx(true, true);
        ctx.animations = false;
        (ctx, rx)
    }

    /// Puts changed settings to work. What needs a restart only takes effect
    /// through `started_config` staying as it was.
    pub(crate) fn apply_config(&mut self, config: &Config) {
        self.theme = Theme::from_config(&config.ui);
        self.animations = config.ui.animations;
        self.prefs = Prefs::from_config(config);
        if config.auth.read_only {
            for forge in self.forges.iter() {
                forge.lock_read_only();
            }
        }
        self.config = config.clone();
    }

    /// Seconds since the app started; drives animations.
    pub fn time(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// Channel for actions produced outside input handling (background tasks).
    pub fn sender(&self) -> mpsc::UnboundedSender<Action> {
        self.tx.clone()
    }

    /// Whether `account` passes the account filter.
    pub fn shows(&self, account: &AccountId) -> bool {
        self.account_filter.as_ref().is_none_or(|a| a == account)
    }

    /// The forges currently shown.
    pub fn active_forges(&self) -> impl Iterator<Item = &Arc<dyn Forge>> {
        self.forges.iter().filter(|f| self.shows(f.account()))
    }

    /// The forges that can answer for the current scope: all of them, or
    /// the ones on the repository's host.
    pub fn scoped_forges(&self) -> impl Iterator<Item = &Arc<dyn Forge>> {
        self.forges.iter().filter(|f| match &self.scope {
            Scope::All => true,
            Scope::Repo(repo) => f.account().host == repo.host,
        })
    }

    pub fn multi_account(&self) -> bool {
        self.forges.len() > 1
    }

    /// A stable color per account for badges.
    pub fn account_color(&self, account: &AccountId) -> Color {
        let t = &self.theme;
        let palette = [
            t.accent,
            t.accent_alt,
            t.success,
            t.warning,
            t.info,
            t.highlight,
        ];
        let index = self
            .forges
            .iter()
            .position(|f| f.account() == account)
            .unwrap_or(0);
        palette
            .get(index % palette.len())
            .copied()
            .unwrap_or(t.accent)
    }
}

/// A screen on the view stack. Kept deliberately small; extend as the UX grows.
pub trait View {
    /// Called once when the view is pushed; a good place to start fetches.
    fn mount(&mut self, ctx: &Ctx) {
        let _ = ctx;
    }

    /// Handles input. Unhandled input returns `None` and falls through to the
    /// app's global keys.
    fn handle_event(&mut self, event: &Event, ctx: &Ctx) -> Option<Action> {
        let _ = (event, ctx);
        None
    }

    /// Reacts to an action the app did not consume itself.
    fn update(&mut self, action: &Action, ctx: &Ctx) -> Option<Action> {
        let _ = (action, ctx);
        None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, ctx: &Ctx);

    /// While any view animates, the app redraws at the configured frame cap.
    fn is_animating(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use hit_core::{ForgeKind, Host};

    use super::*;

    struct Spy {
        account: AccountId,
        locked: AtomicBool,
    }

    impl Forge for Spy {
        fn kind(&self) -> ForgeKind {
            ForgeKind::GitHub
        }

        fn account(&self) -> &AccountId {
            &self.account
        }

        fn lock_read_only(&self) {
            self.locked.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn turning_read_only_on_locks_every_account_at_once() {
        let spy = Arc::new(Spy {
            account: AccountId::new(Host::new("example.test"), "someone"),
            locked: AtomicBool::new(false),
        });
        let mut forges = Forges::new();
        forges.add(Arc::clone(&spy) as Arc<dyn Forge>);
        let (mut ctx, _rx) = Ctx::for_tests(Services {
            forges,
            ..Services::default()
        });

        let mut config = Config::default();
        config.ui.animations = false;
        ctx.apply_config(&config);
        assert!(!spy.locked.load(Ordering::Relaxed));

        config.auth.read_only = true;
        ctx.apply_config(&config);
        assert!(spy.locked.load(Ordering::Relaxed));
    }
}
