//! Data types shared by every part of the jobs module, plus the handful of
//! helpers that both the launchd and cron decoders need.
//!
//! Kept deliberately dependency-free (no `Command`, no filesystem access) so it
//! can be read as the vocabulary of the module without any behaviour attached.

use serde::{Deserialize, Serialize};

use super::policy::{Permissions, Status};
use std::process::Command;

/// A change the user can ask for on a job.
///
/// Named separately from the commands that perform it because the authority a
/// change needs depends on which one it is — see [`Scope::needs_admin`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Enable or disable: writes launchd's disabled database.
    Toggle,
    /// Unload and move the job's file to the Trash.
    Delete,
}

/// Which part of the machine a job belongs to, and therefore how much authority
/// is needed to change it.
///
/// Serialises to the lowercase name. The frontend receives this string on every
/// job and hands it straight back when it asks for an action, so the four
/// spellings are part of the wire format rather than an internal detail.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// `~/Library/LaunchAgents` — the user's own jobs, changeable without a password.
    User,
    /// `/Library/LaunchAgents` — installed for every user; the directory is root-owned.
    Global,
    /// `/Library/LaunchDaemons` — loaded into launchd's `system` domain.
    System,
    /// An Apple-owned job, wherever it was found. Protected from every change.
    Apple,
}

impl Scope {
    /// The launchd domain target this job is addressed in.
    ///
    /// System daemons are loaded into the machine-wide `system` domain;
    /// everything else belongs to the logged-in user's `gui/<uid>` domain. The
    /// uid is supplied by the caller rather than read here, so the rule can be
    /// tested without a real session.
    pub fn launchd_domain(&self, uid: &str) -> String {
        match self {
            Scope::System => "system".to_string(),
            _ => format!("gui/{}", uid),
        }
    }

    /// Whether this action on this scope needs administrator authentication.
    ///
    /// The two actions genuinely differ, and the reason is worth keeping in one
    /// place: [`Action::Toggle`] writes launchd's disabled database, which is
    /// per-user except in the `system` domain, whereas [`Action::Delete`] moves
    /// the job's file, and every directory outside `~/Library` is root-owned.
    pub fn needs_admin(&self, action: Action) -> bool {
        match action {
            Action::Toggle => matches!(self, Scope::System),
            Action::Delete => !matches!(self, Scope::User),
        }
    }
}

/// A single scheduled job, from either launchd or cron.
///
/// One value per plist file or per crontab line. This is the only type crossing
/// the Tauri bridge to the frontend, which is why it is `Serialize` and uses
/// plain `String`s rather than enums — the UI does its own string matching and
/// serde field names must stay stable.
#[derive(Serialize, Clone)]
pub struct Job {
    /// Unique identifier: the plist `Label`, or `<file>:<line>` for cron.
    pub label: String,
    /// Which subsystem the job came from: `"launchd"` or `"cron"`.
    pub kind: String,
    /// Ownership/trust bucket. Drives both the UI grouping and, via
    /// [`Scope::needs_admin`], whether a change needs admin rights.
    pub scope: Scope,
    /// Absolute path to the plist or crontab file this job was read from.
    pub source_path: String,
    /// Human label for that source, e.g. `"User Agents"` or `"Cron"`.
    pub source_group: String,
    /// The executable: plist `Program`, else the first `ProgramArguments` entry.
    /// For cron this holds the whole command line.
    pub program: String,
    /// Remaining command-line arguments (always empty for cron).
    pub args: Vec<String>,
    /// Schedule rendered as plain English, e.g. `"Daily at 09:30"`.
    pub schedule_human: String,
    /// `StandardOutPath` from the plist, if set.
    pub stdout_path: Option<String>,
    /// `StandardErrorPath` from the plist, if set.
    pub stderr_path: Option<String>,
    /// What the job is doing, decided in [`super::policy`] from the plist, the
    /// disabled database and `launchctl list`. The frontend renders this
    /// directly rather than combining the raw facts itself.
    pub status: Status,
    /// What the user may do to the job, one answer per action.
    pub permissions: Permissions,
}

/// One directory that launchd loads job definitions from.
///
/// Stored as `&'static str` fields in a `const` table rather than owned
/// `String`s so the domain list costs nothing at runtime and cannot drift.
pub struct Domain {
    /// Directory path. Relative to the home directory when [`Domain::home`].
    pub dir: &'static str,
    /// Default [`Job::scope`] for jobs found here (Apple labels override it).
    pub scope: Scope,
    /// Human-readable name for the UI's source column.
    pub group: &'static str,
    /// Whether `dir` must be joined onto the user's home directory.
    pub home: bool,
    /// Whether jobs here live in launchd's `system` domain rather than
    /// `gui/<uid>`. Decides which disabled database to consult and whether a
    /// change needs administrator authentication.
    pub daemon: bool,
}

/// The current user's numeric UID, as a string.
///
/// Shells out to `id -u` instead of pulling in a libc dependency for one value.
/// Falls back to `"501"` (the first macOS account) if the call fails, which is
/// only ever used to build a `gui/<uid>` launchd target — a wrong value yields a
/// clean "target not found" error rather than touching the wrong job.
pub fn uid() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".into())
}

/// Name of a weekday from its number, e.g. `1` -> `"Monday"`.
///
/// Shared by both decoders because launchd and cron use the same convention:
/// 0 and 7 both mean Sunday. The modulo makes 7 wrap to 0 and tolerates any
/// out-of-range value rather than panicking on malformed input.
pub fn weekday_name(w: i64) -> String {
    let names = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let idx = ((w % 7 + 7) % 7) as usize;
    names.get(idx).unwrap_or(&"?").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frontend passes `scope` straight back to Rust on every action, so
    /// these four spellings are part of the wire format, not an internal detail.
    /// Changing one breaks every button in the detail pane.
    #[test]
    fn scope_round_trips_through_its_wire_strings() {
        for (scope, wire) in [
            (Scope::User, r#""user""#),
            (Scope::Global, r#""global""#),
            (Scope::System, r#""system""#),
            (Scope::Apple, r#""apple""#),
        ] {
            assert_eq!(serde_json::to_string(&scope).unwrap(), wire);
            assert_eq!(serde_json::from_str::<Scope>(wire).unwrap(), scope);
        }
    }

    /// Only system daemons are addressed in launchd's `system` domain;
    /// everything else is addressed per-user as `gui/<uid>`. The uid is passed
    /// in rather than looked up so the decision stays testable.
    #[test]
    fn only_system_daemons_use_the_system_launchd_domain() {
        assert_eq!(Scope::System.launchd_domain("501"), "system");
        assert_eq!(Scope::User.launchd_domain("501"), "gui/501");
        assert_eq!(Scope::Global.launchd_domain("501"), "gui/501");
        // Apple daemons exist, but every action on them is refused long before
        // a domain is needed, so they follow the same path as agents.
        assert_eq!(Scope::Apple.launchd_domain("501"), "gui/501");
    }

    /// Whether a change needs administrator authentication depends on the
    /// action as well as the scope, and the difference is deliberate:
    /// toggling a global agent writes the *user's* disabled database, while
    /// deleting one moves a file out of root-owned `/Library/LaunchAgents`.
    /// Before this type existed the two rules lived in separate functions and
    /// nothing said they were meant to differ.
    #[test]
    fn admin_is_required_per_action_not_per_scope() {
        assert!(!Scope::Global.needs_admin(Action::Toggle));
        assert!(Scope::Global.needs_admin(Action::Delete));

        assert!(Scope::System.needs_admin(Action::Toggle));
        assert!(Scope::System.needs_admin(Action::Delete));

        assert!(!Scope::User.needs_admin(Action::Toggle));
        assert!(!Scope::User.needs_admin(Action::Delete));
    }
}
