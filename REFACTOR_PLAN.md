# Refactored jobs.rs - How It Should Look

The current `jobs.rs` (702 lines, score 42/100) should be split into **4 focused modules**:

## 1. `jobs/types.rs` (≈100 lines)
**Purpose:** Data structures for Job and Domain

```rust
/// A scheduled job from launchd or cron.
/// 
/// This struct represents a single scheduled job with all its metadata.
/// Used for displaying in the UI and for enable/disable operations.
#[derive(Serialize, Clone)]
pub struct Job {
    pub label: String,           // Unique identifier (e.g., "com.apple.some.service")
    pub kind: String,            // "launchd" or "cron"
    pub scope: String,           // "user", "global", "system", or "apple"
    pub source_path: String,     // Full path to the plist or crontab file
    pub source_group: String,    // Human-readable group name (e.g., "User Agents")
    pub program: String,         // Executable path
    pub args: Vec<String>,       // Command-line arguments
    pub schedule_human: String,  // Human-readable schedule description
    pub stdout_path: Option<String>,  // Where stdout is logged
    pub stderr_path: Option<String>,  // Where stderr is logged
    pub disabled_key: bool,      // Disabled=true in the plist itself
    pub disabled_override: Option<bool>, // System override from launchctl print-disabled
    pub loaded: bool,            // Currently loaded in launchd
    pub pid: Option<i64>,        // Process ID if running
    pub last_exit: Option<i64>,  // Last exit code
    pub apple: bool,             // true if com.apple.* (protected from modification)
}

/// A domain where launchd jobs live (e.g., ~/Library/LaunchAgents)
struct Domain {
    dir: &'static str,   // Path (with ~ for user domains)
    scope: &'static str, // user/global/system/apple
    group: &'static str, // Human-readable label
    home: bool,          // true if path needs ~ expanded to home dir
    daemon: bool,        // true if in system domain (vs gui/<uid>)
}
```

**Why this matters:** Centralized data types with clear documentation. Other modules import from here.

---

## 2. `jobs/launchd.rs` (≈250 lines)
**Purpose:** All launchd-specific logic - listing, parsing, schedule decoding

```rust
/// List all launchd jobs from all standard domains.
///
/// Scans the following domains:
/// - User agents (~/Library/LaunchAgents)
/// - Global agents (/Library/LaunchAgents)
/// - System daemons (/Library/LaunchDaemons)
/// - Apple agents/daemons (/System/Library/...)
///
/// For each job, enriches with runtime status from `launchctl list`
/// and disabled state from `launchctl print-disabled`.
///
/// # Returns
/// Vec<Job> - may be empty if no jobs found or all domains unreadable
pub fn list_launchd_jobs() -> Vec<Job> {
    // Implementation...
}

/// Parse a single launchd plist file into a Job struct.
///
/// # Arguments
/// * `path` - Path to the .plist file
/// * `domain` - The domain this file belongs to (determines scope/group)
/// * `runtime` - Pre-fetched runtime status from launchctl list
/// * `disabled_db` - Pre-fetched disabled state from launchctl print-disabled
///
/// # Returns
/// Some(Job) if parsing succeeds, None if file is unreadable or invalid
fn parse_launchd_plist(
    path: &Path,
    domain: &Domain,
    runtime: &HashMap<String, (Option<i64>, Option<i64>)>,
    disabled_db: &HashMap<String, bool>,
) -> Option<Job> {
    // Implementation...
}

/// Execute `launchctl list` and parse the output.
///
/// # Returns
/// HashMap<label, (pid, last_exit_status)>
/// - pid: Some if process is running, None otherwise
/// - last_exit_status: Last exit code if available
fn launchctl_list() -> HashMap<String, (Option<i64>, Option<i64>)> {
    // Implementation...
}

/// Execute `launchctl print-disabled <domain>` and parse disabled state.
///
/// Output format varies by macOS version:
/// - Newer: "com.example.service" => disabled
/// - Older: "com.example.service" => true/false
///
/// # Arguments
/// * `domain` - Domain to query (e.g., "gui/501", "system")
///
/// # Returns
/// HashMap<label, disabled> where disabled=true means the job is disabled
fn print_disabled(domain: &str) -> HashMap<String, bool> {
    // Implementation...
}

/// Decode launchd schedule into human-readable string.
///
/// Handles:
/// - RunAtLoad: "At login/boot"
/// - StartInterval: "Every N hours/minutes/days"
/// - StartCalendarInterval: "Daily at 14:30", "Every Monday at 09:00"
/// - WatchPaths: "When path changes: /tmp, /var/log"
/// - StartOnMount: "On volume mount"
/// - KeepAlive: "Kept running (restarted if it exits)"
///
/// # Arguments
/// * `dict` - Parsed plist dictionary containing schedule keys
///
/// # Returns
/// Human-readable schedule string, or "On demand / manual" if no schedule
fn decode_launchd_schedule(dict: &plist::Dictionary) -> String {
    // Implementation...
}

/// Decode a StartCalendarInterval dictionary into text.
///
/// # Arguments
/// * `d` - Dictionary with keys: Minute, Hour, Day, Weekday, Month
///
/// # Examples
/// - {Hour: 9, Minute: 30} -> "09:30"
/// - {Weekday: 1} -> "Every Monday at 00:00"
/// - {Month: 1, Day: 15} -> "On January 15 at 00:00"
fn decode_calendar(d: &plist::Dictionary) -> String {
    // Implementation...
}

// Helper functions (all documented):
// - human_duration(secs) -> "Every 2 hours", "Every 30 minutes"
// - weekday_name(w) -> "Monday", "Tuesday", etc.
// - month_name(m) -> "January", "February", etc.
// - plural(n) -> "" if n==1, "s" otherwise
```

**Why this matters:** All launchd logic in one place. Clear separation from cron logic.

---

## 3. `jobs/cron.rs` (≈150 lines)
**Purpose:** All cron-specific logic - listing, parsing, schedule decoding

```rust
/// List all cron jobs for the current user and system.
///
/// Sources:
/// - User crontab (`crontab -l`)
/// - System crontab (/etc/crontab)
///
/// # Returns
/// Vec<Job> - may be empty if no cron jobs defined
pub fn list_cron_jobs() -> Vec<Job> {
    // Implementation...
}

/// Parse crontab text into Job entries.
///
/// # Arguments
/// * `text` - Raw crontab content
/// * `source` - Source label (e.g., "crontab -l (this user)")
/// * `has_user_field` - true if crontab has username field (system crontabs do)
/// * `jobs` - Vector to append parsed jobs to
fn parse_cron_text(text: &str, source: &str, has_user_field: bool, jobs: &mut Vec<Job>) {
    // Implementation...
}

/// Convert cron field value to human-readable text.
///
/// # Arguments
/// * `kw` - Cron keyword or value (e.g., "*", "1-5", "0-23/2")
///
/// # Returns
/// Human-readable description (e.g., "every hour", "minutes 0-30")
fn decode_cron_keyword(kw: &str) -> String {
    // Implementation...
}

/// Build full schedule description from cron fields.
///
/// # Arguments
/// * `m` - Minute field (0-59 or *)
/// * `h` - Hour field (0-23 or *)
/// * `dom` - Day of month (1-31 or *)
/// * `mon` - Month (1-12 or *)
/// * `dow` - Day of week (0-7, where 0 and 7 are Sunday)
///
/// # Returns
/// Human-readable schedule (e.g., "Daily at 09:30", "Every 15 minutes")
fn decode_cron(m: &str, h: &str, dom: &str, mon: &str, dow: &str) -> String {
    // Implementation...
}

// Helper: short_source(s) -> shorten long paths for display
```

**Why this matters:** Cron logic isolated. Easy to test and modify independently.

---

## 4. `jobs/actions.rs` (≈200 lines)
**Purpose:** Job modification operations - enable, disable, delete, reveal

```rust
/// Enable or disable a launchd job.
///
/// # Arguments
/// * `label` - Job label (e.g., "com.example.service")
/// * `path` - Path to the plist file
/// * `scope` - Job scope ("user", "global", "system", "apple")
/// * `enable` - true to enable, false to disable
///
/// # Returns
/// Ok(String) - Success message
/// Err(String) - Error message if operation failed
///
/// # Safety
/// - Apple jobs (com.apple.*) are protected and cannot be modified
/// - System jobs require sudo privileges
/// - User jobs can be modified directly
pub fn set_job_enabled(label: &str, path: &str, scope: &str, enable: bool) -> Result<String, String> {
    // Implementation...
}

/// Delete a job by moving its plist to Trash.
///
/// # Arguments
/// * `label` - Job label
/// * `path` - Path to the plist file
/// * `scope` - Job scope
///
/// # Returns
/// Ok(String) - Success message with Trash location
/// Err(String) - Error message if deletion failed
///
/// # Safety
/// - Apple jobs are protected
/// - System jobs require sudo
/// - Uses macOS Trash API for safe deletion
pub fn delete_job(label: &str, path: &str, scope: &str) -> Result<String, String> {
    // Implementation...
}

/// Reveal a file in Finder.
///
/// # Arguments
/// * `path` - Absolute path to reveal
///
/// # Returns
/// Ok(()) if successful, Err(String) if Finder couldn't be invoked
pub fn reveal_in_finder(path: &str) -> Result<(), String> {
    // Implementation...
}

/// Open a file with its default application.
///
/// # Arguments
/// * `path` - Absolute path to open
///
/// # Returns
/// Ok(()) if successful, Err(String) if open command failed
pub fn open_path(path: &str) -> Result<(), String> {
    // Implementation...
}

// Internal helpers (all documented):
// - guard_protected(label, path, scope) -> Err if job is protected
// - trash_destination(src) -> Path in Trash
// - run_steps(steps, privileged) -> Execute shell steps
// - run_direct(steps) -> Run without sudo
// - run_privileged(script) -> Run with sudo via AppleScript
// - script_for(steps) -> Generate shell script from steps
// - shell_quote(s) -> Escape string for shell
```

**Why this matters:** All mutation operations in one place. Clear safety guarantees documented.

---

## Module Structure

In `jobs/mod.rs`:

```rust
//! Job enumeration and management for launchd and cron.
//!
//! This module provides functionality to:
//! - List all scheduled jobs from launchd and cron
//! - Parse job configurations from plists and crontabs
//! - Enable/disable and delete jobs (with safety checks)
//! - Reveal job files in Finder or open them
//!
//! # Safety
//! - Apple jobs (com.apple.*) are read-only
//! - System jobs require elevated privileges
//! - All operations are best-effort - unreadable files are skipped

mod types;      // Job and Domain structs
mod launchd;    // launchd-specific logic
mod cron;       // cron-specific logic
mod actions;    // enable/disable/delete operations

pub use types::Job;
pub use launchd::list_launchd_jobs;
pub use cron::list_cron_jobs;
pub use actions::{set_job_enabled, delete_job, reveal_in_finder, open_path};
```

---

## What This Fixes

| Issue | Before | After |
|-------|--------|-------|
| **File size** | 702 lines in one file | 4 files, 100-250 lines each |
| **Documentation** | 17/26 functions undocumented | ALL functions documented |
| **Duplication** | 11 duplicated lines | Shared helpers in `types.rs` |
| **Maintainability** | Hard to navigate | Clear module boundaries |
| **Testability** | Hard to test in isolation | Each module independently testable |

---

## Your Move

Tell the AI:

```
Refactor jobs.rs (702 lines) into 4 focused modules:

1. types.rs (~100 lines) - Job and Domain structs with full docstrings
2. launchd.rs (~250 lines) - All launchd logic, every function documented
3. cron.rs (~150 lines) - All cron logic, every function documented  
4. actions.rs (~200 lines) - Enable/disable/delete operations with safety docs

Each function must have:
- A docstring explaining WHAT it does
- A comment explaining WHY this approach was chosen
- Clear argument and return value documentation

Use the module pattern in jobs/mod.rs to export the public API.

After refactoring, run the code quality checker again - I expect all files to score 80+.
```

This is how experienced developers structure code. AI won't do this automatically - you have to demand it. 🎯
