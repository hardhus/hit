//! Every setting the settings screen can change. Most used first: the screen
//! is opened to change how hit behaves day to day, not to read a manual.

use hit_config::{
    CacheConfig, Config, FocusConfig, LogConfig, LogLevel, MarkRead, RefreshConfig, SettingValue,
    SortOrder, ThemeName, UiConfig,
};

/// When a change takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applies {
    Now,
    NextStart,
    /// A protection: on right away, off only after a restart.
    OnNowOffNextStart,
}

#[derive(Clone, Copy)]
pub enum Kind {
    Toggle {
        get: fn(&Config) -> bool,
        set: fn(&mut Config, bool),
    },
    Number {
        get: fn(&Config) -> i64,
        set: fn(&mut Config, i64),
        min: i64,
        max: i64,
        step: i64,
    },
    Choice {
        options: &'static [&'static str],
        get: fn(&Config) -> &'static str,
        set: fn(&mut Config, &str),
    },
}

/// Changing a toggle to `to` weakens a protection, so it asks first.
#[derive(Clone, Copy)]
pub struct Confirm {
    pub to: bool,
    pub question: &'static str,
}

#[derive(Clone, Copy)]
pub struct Setting {
    pub group: &'static str,
    pub section: &'static str,
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub applies: Applies,
    pub kind: Kind,
    pub confirm: Option<Confirm>,
}

impl Setting {
    /// The value as it is written to the config file.
    pub fn value(&self, config: &Config) -> SettingValue {
        match self.kind {
            Kind::Toggle { get, .. } => SettingValue::Bool(get(config)),
            Kind::Number { get, .. } => SettingValue::Int(get(config)),
            Kind::Choice { get, .. } => SettingValue::Str(get(config).to_owned()),
        }
    }

    /// The value as shown on screen.
    pub fn shown(&self, config: &Config) -> String {
        match self.value(config) {
            SettingValue::Bool(true) => "on".to_owned(),
            SettingValue::Bool(false) => "off".to_owned(),
            SettingValue::Int(n) => n.to_string(),
            SettingValue::Str(s) => s,
        }
    }
}

fn to_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

fn to_i64<T: Into<i64>>(value: T) -> i64 {
    value.into()
}

fn sort_name(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::Newest => "newest",
        SortOrder::Oldest => "oldest",
    }
}

fn mark_read_name(mark_read: MarkRead) -> &'static str {
    match mark_read {
        MarkRead::Manual => "manual",
        MarkRead::Open => "open",
        MarkRead::Leave => "leave",
    }
}

fn theme_name(theme: ThemeName) -> &'static str {
    match theme {
        ThemeName::Jinx => "jinx",
    }
}

macro_rules! toggle {
    ($section:ident . $key:ident) => {
        Kind::Toggle {
            get: |c| c.$section.$key,
            set: |c, v| c.$section.$key = v,
        }
    };
}

pub static SETTINGS: &[Setting] = &[
    Setting {
        group: "Action Required",
        section: "focus",
        key: "review_requests",
        label: "Reviews asked of you",
        help: "Pull requests waiting for your review, directly or through a team.",
        applies: Applies::NextStart,
        kind: toggle!(focus.review_requests),
        confirm: None,
    },
    Setting {
        group: "Action Required",
        section: "focus",
        key: "my_pull_requests",
        label: "Your pull requests",
        help: "Yours with requested changes, failing checks, or ready to merge.",
        applies: Applies::NextStart,
        kind: toggle!(focus.my_pull_requests),
        confirm: None,
    },
    Setting {
        group: "Action Required",
        section: "focus",
        key: "mentions",
        label: "Mentions",
        help: "Open issues and pull requests that mention you.",
        applies: Applies::NextStart,
        kind: toggle!(focus.mentions),
        confirm: None,
    },
    Setting {
        group: "Action Required",
        section: "focus",
        key: "mention_days",
        label: "Mentions from the last (days)",
        help: "Older mentions are left out, so the list does not fill up with old threads.",
        applies: Applies::NextStart,
        kind: Kind::Number {
            get: |c| to_i64(c.focus.mention_days),
            set: |c, v| c.focus.mention_days = to_u32(v),
            min: FocusConfig::MENTION_DAYS_RANGE.0 as i64,
            max: FocusConfig::MENTION_DAYS_RANGE.1 as i64,
            step: 1,
        },
        confirm: None,
    },
    Setting {
        group: "Action Required",
        section: "focus",
        key: "assigned",
        label: "Assigned to you",
        help: "Open issues and pull requests assigned to you.",
        applies: Applies::NextStart,
        kind: toggle!(focus.assigned),
        confirm: None,
    },
    Setting {
        group: "Action Required",
        section: "focus",
        key: "sort",
        label: "Order",
        help: "Which comes first inside each group. `s` switches it for the moment.",
        applies: Applies::Now,
        kind: Kind::Choice {
            options: &["newest", "oldest"],
            get: |c| sort_name(c.focus.sort),
            set: |c, v| {
                c.focus.sort = if v == "oldest" {
                    SortOrder::Oldest
                } else {
                    SortOrder::Newest
                };
            },
        },
        confirm: None,
    },
    Setting {
        group: "Action Required",
        section: "focus",
        key: "group",
        label: "Group by what is asked of you",
        help: "Off shows one list. `v` switches it for the moment.",
        applies: Applies::Now,
        kind: toggle!(focus.group),
        confirm: None,
    },
    Setting {
        group: "Inbox",
        section: "inbox",
        key: "unread_only",
        label: "Start with unread only",
        help: "`u` switches between unread and everything for the moment.",
        applies: Applies::Now,
        kind: toggle!(inbox.unread_only),
        confirm: None,
    },
    Setting {
        group: "Inbox",
        section: "inbox",
        key: "mark_read",
        label: "Mark read on GitHub",
        help: "manual: only with `m` and `M`. open: also when you open one. leave: the ones you opened, when you leave the Inbox.",
        applies: Applies::Now,
        kind: Kind::Choice {
            options: &["manual", "open", "leave"],
            get: |c| mark_read_name(c.inbox.mark_read),
            set: |c, v| {
                c.inbox.mark_read = match v {
                    "open" => MarkRead::Open,
                    "leave" => MarkRead::Leave,
                    _ => MarkRead::Manual,
                };
            },
        },
        confirm: None,
    },
    Setting {
        group: "Repositories",
        section: "general",
        key: "repo_context",
        label: "Open on the current repository",
        help: "Inside a git repository, hit opens on it. `.` shows everything.",
        applies: Applies::NextStart,
        kind: toggle!(general.repo_context),
        confirm: None,
    },
    Setting {
        group: "Refresh",
        section: "refresh",
        key: "focus_minutes",
        label: "Action Required every (minutes)",
        help: "0 refreshes only on start and when you press `r`.",
        applies: Applies::Now,
        kind: Kind::Number {
            get: |c| to_i64(c.refresh.focus_minutes),
            set: |c, v| c.refresh.focus_minutes = to_u32(v),
            min: RefreshConfig::FOCUS_MINUTES_RANGE.0 as i64,
            max: RefreshConfig::FOCUS_MINUTES_RANGE.1 as i64,
            step: 1,
        },
        confirm: None,
    },
    Setting {
        group: "Refresh",
        section: "refresh",
        key: "notifications",
        label: "Poll notifications",
        help: "As often as GitHub asks, usually every 60 seconds. Unchanged results do not count against the rate limit.",
        applies: Applies::Now,
        kind: toggle!(refresh.notifications),
        confirm: None,
    },
    Setting {
        group: "Appearance",
        section: "ui",
        key: "animations",
        label: "Animations",
        help: "Logo shimmer, spinners and transitions.",
        applies: Applies::Now,
        kind: toggle!(ui.animations),
        confirm: None,
    },
    Setting {
        group: "Appearance",
        section: "ui",
        key: "transparent",
        label: "Transparent background",
        help: "Keep the terminal's own background visible behind hit.",
        applies: Applies::Now,
        kind: toggle!(ui.transparent),
        confirm: None,
    },
    Setting {
        group: "Appearance",
        section: "ui",
        key: "theme",
        label: "Theme",
        help: "Jinx is the only theme for now.",
        applies: Applies::Now,
        kind: Kind::Choice {
            options: &["jinx"],
            get: |c| theme_name(c.ui.theme),
            set: |c, _| c.ui.theme = ThemeName::Jinx,
        },
        confirm: None,
    },
    Setting {
        group: "Appearance",
        section: "ui",
        key: "max_fps",
        label: "Frame cap",
        help: "Only while something is animating. Idle screens are not redrawn.",
        applies: Applies::Now,
        kind: Kind::Number {
            get: |c| to_i64(c.ui.max_fps),
            set: |c, v| c.ui.max_fps = u16::try_from(v).unwrap_or(UiConfig::FPS_RANGE.1),
            min: UiConfig::FPS_RANGE.0 as i64,
            max: UiConfig::FPS_RANGE.1 as i64,
            step: 10,
        },
        confirm: None,
    },
    Setting {
        group: "Safety",
        section: "auth",
        key: "read_only",
        label: "Read-only",
        help: "Never write with any account, whatever the token allows. A running hit can only get stricter.",
        applies: Applies::OnNowOffNextStart,
        kind: toggle!(auth.read_only),
        confirm: Some(Confirm {
            to: false,
            question: "Turn off read-only? After the next start hit can write with your accounts.",
        }),
    },
    Setting {
        group: "Safety",
        section: "auth",
        key: "allow_mark_read",
        label: "Read-only may still mark read",
        help: "Only your own inbox state changes. Off means read-only changes nothing at all.",
        applies: Applies::NextStart,
        kind: toggle!(auth.allow_mark_read),
        confirm: Some(Confirm {
            to: true,
            question: "Let read-only mode mark notifications read on GitHub?",
        }),
    },
    Setting {
        group: "Cache",
        section: "cache",
        key: "enabled",
        label: "Keep data on this computer",
        help: "So hit opens instantly and works offline. Encrypted with a key from each account's token. `hit cache clear` deletes it all.",
        applies: Applies::NextStart,
        kind: toggle!(cache.enabled),
        confirm: None,
    },
    Setting {
        group: "Cache",
        section: "cache",
        key: "max_age_days",
        label: "Keep for (days)",
        help: "Cached data older than this is dropped on start.",
        applies: Applies::NextStart,
        kind: Kind::Number {
            get: |c| to_i64(c.cache.max_age_days),
            set: |c, v| c.cache.max_age_days = to_u32(v),
            min: CacheConfig::MAX_AGE_RANGE.0 as i64,
            max: CacheConfig::MAX_AGE_RANGE.1 as i64,
            step: 1,
        },
        confirm: None,
    },
    Setting {
        group: "Logs",
        section: "log",
        key: "level",
        label: "Level",
        help: "How much hit writes to its log files. HIT_LOG overrides this.",
        applies: Applies::NextStart,
        kind: Kind::Choice {
            options: &["off", "error", "warn", "info", "debug", "trace"],
            get: |c| c.log.level.as_str(),
            set: |c, v| {
                c.log.level = match v {
                    "off" => LogLevel::Off,
                    "error" => LogLevel::Error,
                    "warn" => LogLevel::Warn,
                    "debug" => LogLevel::Debug,
                    "trace" => LogLevel::Trace,
                    _ => LogLevel::Info,
                };
            },
        },
        confirm: None,
    },
    Setting {
        group: "Logs",
        section: "log",
        key: "retention_days",
        label: "Keep for (days)",
        help: "Older log files are deleted on start.",
        applies: Applies::NextStart,
        kind: Kind::Number {
            get: |c| to_i64(c.log.retention_days),
            set: |c, v| c.log.retention_days = to_u32(v),
            min: LogConfig::RETENTION_RANGE.0 as i64,
            max: LogConfig::RETENTION_RANGE.1 as i64,
            step: 1,
        },
        confirm: None,
    },
    Setting {
        group: "Logs",
        section: "log",
        key: "max_total_mb",
        label: "Total size (MB)",
        help: "The oldest log files are deleted on start until the total fits.",
        applies: Applies::NextStart,
        kind: Kind::Number {
            get: |c| i64::try_from(c.log.max_total_mb).unwrap_or(i64::MAX),
            set: |c, v| c.log.max_total_mb = u64::try_from(v).unwrap_or(1),
            min: LogConfig::TOTAL_MB_RANGE.0 as i64,
            max: LogConfig::TOTAL_MB_RANGE.1 as i64,
            step: 10,
        },
        confirm: None,
    },
];
