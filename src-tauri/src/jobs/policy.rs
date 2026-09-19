//! What a job's state *means*, decided once here rather than in the frontend.
//!
//! Every rule in this module used to exist twice: once in Rust, where it
//! guarded the action, and once in JavaScript, where it decided what the user
//! saw. Nothing kept the two in step, so the UI could offer a button for a
//! change the backend would refuse.
//!
//! Everything here is pure — no files, no commands — so each rule is tested
//! directly rather than inferred from a running machine.

use serde::Serialize;

use super::types::{Action, Scope};

/// What a launchd job is doing right now.
///
/// Serialised with an internal tag, so the frontend reads
/// `job.status.state === "running"` and takes `job.status.pid` from the same
/// object. Each variant carries only the data that variant can have, which is
/// why "running with no pid" is no longer expressible.
#[derive(Serialize, Clone, PartialEq, Eq, Debug)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    /// Switched off in launchd's disabled database. Beats every other state:
    /// a disabled job may still show a pid until it next exits.
    Disabled,
    /// Currently running.
    Running { pid: i64 },
    /// Loaded and waiting for its trigger.
    Idle { last_exit: Option<i64> },
    /// Known to launchd's configuration but not loaded into the session.
    NotLoaded { last_exit: Option<i64> },
    /// A live crontab line. cron reports no run state at all, so this says
    /// only what is actually known: the line exists and will fire.
    Scheduled,
}

/// The raw launchd facts a [`Status`] is derived from.
///
/// Grouped into one value so the derivation has a single input and the call
/// site reads as a list of named facts rather than five positional arguments.
pub struct LaunchdState {
    /// The `Disabled` key written inside the plist file.
    pub disabled_key: bool,
    /// launchd's disabled database, if it has an entry for this label.
    pub disabled_override: Option<bool>,
    /// Whether the label appears in `launchctl list`.
    pub loaded: bool,
    /// Process id, if it is running.
    pub pid: Option<i64>,
    /// Exit status of the most recent run, if launchd reported one.
    pub last_exit: Option<i64>,
}

impl LaunchdState {
    /// Collapse the raw facts into the one state the UI should show.
    ///
    /// Order matters: disabled wins over everything, because a job disabled
    /// while running keeps its pid until it exits and showing "running" would
    /// invite the user to disable something already disabled.
    pub fn status(&self) -> Status {
        if self.disabled_override.unwrap_or(self.disabled_key) {
            return Status::Disabled;
        }
        match (self.pid, self.loaded) {
            (Some(pid), _) => Status::Running { pid },
            (None, true) => Status::Idle { last_exit: self.last_exit },
            (None, false) => Status::NotLoaded { last_exit: self.last_exit },
        }
    }
}

/// Why a change was refused outright.
///
/// A code, not a sentence: the frontend writes the English so wording can be
/// changed without rebuilding the app. [`Refusal::message`] supplies the text
/// used when a command refuses, which the user sees as an error.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// An Apple-owned job. Disabling these can leave macOS unable to boot.
    Apple,
    /// On the sealed, read-only `/System` volume.
    SystemVolume,
    /// cron: this app reads crontabs but never writes them.
    Unsupported,
}

impl Refusal {
    /// The message a refused command returns to the user.
    pub fn message(&self) -> &'static str {
        match self {
            Refusal::Apple => {
                "Apple system job — refused. Changing com.apple.* jobs can destabilise macOS."
            }
            Refusal::SystemVolume => {
                "Job lives in /System — refused. That volume is sealed and read-only."
            }
            Refusal::Unsupported => "cron jobs cannot be changed from this app.",
        }
    }
}

/// Whether one action is available on one job, and at what cost.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Permission {
    /// Goes ahead with no prompt.
    Editable,
    /// Shows the native administrator authentication dialog first.
    NeedsAdmin,
    /// Will not be attempted.
    Refused { reason: Refusal },
}

/// What the user may do to a job — one entry per action the UI offers.
///
/// Both actions are answered for every job, so the frontend never has to work
/// out which rule applies to which button.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Permissions {
    pub toggle: Permission,
    pub delete: Permission,
}

/// Whether this job is protected from every change, and why.
///
/// Apple is checked before the volume so an Apple job under `/System` reports
/// the more specific reason. Both the label and the scope are consulted: an
/// Apple label can sit in a user directory, and an Apple-owned job need not
/// carry the label.
pub fn refusal(label: &str, path: &str, scope: Scope) -> Option<Refusal> {
    if label.starts_with("com.apple.") || scope == Scope::Apple {
        Some(Refusal::Apple)
    } else if path.starts_with("/System/") {
        Some(Refusal::SystemVolume)
    } else {
        None
    }
}

/// What the user may do to a launchd job.
pub fn launchd_permissions(label: &str, path: &str, scope: Scope) -> Permissions {
    let permission = |action: Action| match refusal(label, path, scope) {
        Some(reason) => Permission::Refused { reason },
        None if scope.needs_admin(action) => Permission::NeedsAdmin,
        None => Permission::Editable,
    };
    Permissions {
        toggle: permission(Action::Toggle),
        delete: permission(Action::Delete),
    }
}

/// What the user may do to a cron line: nothing, for now.
pub fn cron_permissions() -> Permissions {
    let refused = Permission::Refused { reason: Refusal::Unsupported };
    Permissions { toggle: refused, delete: refused }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `launchctl enable`/`disable` write launchd's own database and never touch
    /// the plist, so the database is the truth whenever it has an entry. Getting
    /// this backwards makes the toggle button look like it does nothing.
    #[test]
    fn the_disabled_database_overrides_the_plist_key() {
        let plist_says_off_database_says_on = LaunchdState {
            disabled_key: true,
            disabled_override: Some(false),
            loaded: true,
            pid: None,
            last_exit: None,
        };
        assert_eq!(
            plist_says_off_database_says_on.status(),
            Status::Idle { last_exit: None }
        );

        let database_says_off_while_running = LaunchdState {
            disabled_key: false,
            disabled_override: Some(true),
            loaded: true,
            pid: Some(431),
            last_exit: None,
        };
        assert_eq!(database_says_off_while_running.status(), Status::Disabled);
    }

    /// Each state carries only what it can know. A running job has no last
    /// exit, because it has not exited.
    #[test]
    fn each_state_carries_only_what_it_can_know() {
        let running = LaunchdState {
            disabled_key: false,
            disabled_override: None,
            loaded: true,
            pid: Some(431),
            last_exit: Some(0),
        };
        assert_eq!(running.status(), Status::Running { pid: 431 });

        let idle = LaunchdState {
            disabled_key: false,
            disabled_override: None,
            loaded: true,
            pid: None,
            last_exit: Some(1),
        };
        assert_eq!(idle.status(), Status::Idle { last_exit: Some(1) });

        let never_loaded = LaunchdState {
            disabled_key: false,
            disabled_override: None,
            loaded: false,
            pid: None,
            last_exit: None,
        };
        assert_eq!(never_loaded.status(), Status::NotLoaded { last_exit: None });
    }

    /// The frontend reads `job.status.state`, so the tag name and the
    /// snake_case spellings are part of the wire format.
    #[test]
    fn status_serialises_with_an_internal_tag() {
        let json = |s: &Status| serde_json::to_string(s).unwrap();
        assert_eq!(json(&Status::Running { pid: 431 }), r#"{"state":"running","pid":431}"#);
        assert_eq!(json(&Status::Disabled), r#"{"state":"disabled"}"#);
        assert_eq!(json(&Status::Scheduled), r#"{"state":"scheduled"}"#);
        assert_eq!(
            json(&Status::NotLoaded { last_exit: Some(2) }),
            r#"{"state":"not_loaded","last_exit":2}"#
        );
    }

    /// The same two refusals `actions.rs` enforces. Apple is checked before the
    /// sealed volume so an Apple job under /System reports the more specific
    /// reason, matching the order the guard has always used.
    #[test]
    fn apple_and_the_sealed_volume_are_refused_for_every_action() {
        let by_label = launchd_permissions("com.apple.thing", "/Library/LaunchAgents/x.plist", Scope::User);
        assert_eq!(by_label.toggle, Permission::Refused { reason: Refusal::Apple });
        assert_eq!(by_label.delete, Permission::Refused { reason: Refusal::Apple });

        // An Apple-owned job that does not carry the com.apple.* label.
        let by_scope = launchd_permissions("com.vendor.thing", "/Library/LaunchAgents/x.plist", Scope::Apple);
        assert_eq!(by_scope.toggle, Permission::Refused { reason: Refusal::Apple });

        let sealed = launchd_permissions("com.me.thing", "/System/Library/LaunchAgents/x.plist", Scope::User);
        assert_eq!(sealed.toggle, Permission::Refused { reason: Refusal::SystemVolume });
    }

    /// The per-action difference, now visible to the UI: a global agent needs
    /// no password to switch off, but does need one to delete.
    #[test]
    fn a_global_agent_toggles_freely_but_needs_admin_to_delete() {
        let global = launchd_permissions("com.me.thing", "/Library/LaunchAgents/x.plist", Scope::Global);
        assert_eq!(global.toggle, Permission::Editable);
        assert_eq!(global.delete, Permission::NeedsAdmin);

        let own = launchd_permissions("com.me.thing", "/Users/me/Library/LaunchAgents/x.plist", Scope::User);
        assert_eq!(own.toggle, Permission::Editable);
        assert_eq!(own.delete, Permission::Editable);
    }

    /// This app reads cron but never writes it, so both actions are refused
    /// rather than simply absent — the UI gets a reason it can explain.
    #[test]
    fn cron_lines_cannot_be_changed_from_this_app() {
        let cron = cron_permissions();
        assert_eq!(cron.toggle, Permission::Refused { reason: Refusal::Unsupported });
        assert_eq!(cron.delete, Permission::Refused { reason: Refusal::Unsupported });
    }
}
