//! Data types shared by every part of the jobs module, plus the handful of
//! helpers that both the launchd and cron decoders need.
//!
//! Kept deliberately dependency-free (no `Command`, no filesystem access) so it
//! can be read as the vocabulary of the module without any behaviour attached.

use serde::Serialize;
use std::process::Command;

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
    /// Ownership/trust bucket: `"user"`, `"global"`, `"system"` or `"apple"`.
    /// Drives both the UI grouping and whether a change needs admin rights.
    pub scope: String,
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
    /// The `Disabled` key written inside the plist file itself.
    pub disabled_key: bool,
    /// launchd's own disabled database (`launchctl print-disabled`).
    ///
    /// This — not [`Job::disabled_key`] — is what `launchctl enable/disable`
    /// writes, and it overrides the plist key. `None` means the database has no
    /// entry for this label, so the plist key wins. The frontend applies that
    /// precedence; using `disabled_key` alone makes toggles look like no-ops.
    pub disabled_override: Option<bool>,
    /// Whether the label is present in `launchctl list`.
    pub loaded: bool,
    /// Process ID, if the job is running right now.
    pub pid: Option<i64>,
    /// Exit status of the most recent run, if launchd reported one.
    pub last_exit: Option<i64>,
    /// `com.apple.*` — protected; every mutating operation refuses these.
    pub apple: bool,
}

/// One directory that launchd loads job definitions from.
///
/// Stored as `&'static str` fields in a `const` table rather than owned
/// `String`s so the domain list costs nothing at runtime and cannot drift.
pub struct Domain {
    /// Directory path. Relative to the home directory when [`Domain::home`].
    pub dir: &'static str,
    /// Default [`Job::scope`] for jobs found here (Apple labels override it).
    pub scope: &'static str,
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
