//! Reading launchd: enumerate the agent/daemon directories, parse each plist,
//! merge in live state from `launchctl`, and turn schedule keys into English.
//!
//! Everything here is read-only and best-effort — an unreadable directory or a
//! malformed plist is skipped rather than failing the whole listing, because a
//! machine with one bad file should still show all its other jobs.

use plist::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::types::{uid, weekday_name, Domain, Job};

/// Every directory launchd loads job definitions from, in display order.
///
/// A `const` table rather than runtime discovery: these five paths are fixed by
/// macOS, and hard-coding them keeps the listing deterministic.
const DOMAINS: &[Domain] = &[
    Domain { dir: "Library/LaunchAgents", scope: "user", group: "User Agents", home: true, daemon: false },
    Domain { dir: "/Library/LaunchAgents", scope: "global", group: "Global Agents", home: false, daemon: false },
    Domain { dir: "/Library/LaunchDaemons", scope: "system", group: "System Daemons", home: false, daemon: true },
    Domain { dir: "/System/Library/LaunchAgents", scope: "apple", group: "Apple Agents", home: false, daemon: false },
    Domain { dir: "/System/Library/LaunchDaemons", scope: "apple", group: "Apple Daemons", home: false, daemon: true },
];

/// List every launchd job across all five domains in [`DOMAINS`].
///
/// The two `launchctl` queries (running state, disabled database) are run
/// **once up front** and passed down into the per-file parser, rather than once
/// per job: there are typically several hundred plists, and shelling out for
/// each would take seconds.
///
/// # Returns
/// Every job that parsed successfully. Empty if no domain is readable — never
/// an error, since a partial listing is more useful than none.
pub fn list_launchd_jobs() -> Vec<Job> {
    let runtime = launchctl_list();
    let disabled_gui = print_disabled(&format!("gui/{}", uid()));
    let disabled_sys = print_disabled("system");
    let mut jobs = Vec::new();
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

    for d in DOMAINS {
        let disabled_db = if d.daemon { &disabled_sys } else { &disabled_gui };
        let dir: PathBuf = if d.home { home.join(d.dir) } else { PathBuf::from(d.dir) };
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("plist") {
                continue;
            }
            if let Some(job) = parse_launchd_plist(&path, d, &runtime, disabled_db) {
                jobs.push(job);
            }
        }
    }
    jobs
}

/// Parse one `.plist` file into a [`Job`].
///
/// # Arguments
/// * `path` — the plist file to read.
/// * `d` — the domain it was found in; supplies the default scope and group.
/// * `runtime` — output of [`launchctl_list`], keyed by label.
/// * `disabled_db` — output of [`print_disabled`] for this domain.
///
/// # Returns
/// `Some(Job)` when the file is a readable plist dictionary; `None` when it is
/// unreadable, binary junk, or not a dictionary at the top level.
fn parse_launchd_plist(
    path: &Path,
    d: &Domain,
    runtime: &HashMap<String, (Option<i64>, Option<i64>)>,
    disabled_db: &HashMap<String, bool>,
) -> Option<Job> {
    let value = Value::from_file(path).ok()?;
    let dict = value.as_dictionary()?;

    // The Label key is meant to be mandatory, but plenty of third-party plists
    // omit it; launchd falls back to the file name and so do we.
    let label = dict
        .get("Label")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string());

    // Label wins over location: an Apple-owned label installed into /Library is
    // still Apple's, and must still be protected from toggling.
    let apple = label.starts_with("com.apple.");
    let scope = if apple { "apple" } else { d.scope };

    let mut args: Vec<String> = Vec::new();
    if let Some(pa) = dict.get("ProgramArguments").and_then(|v| v.as_array()) {
        for a in pa {
            if let Some(s) = a.as_string() {
                args.push(s.to_string());
            }
        }
    }
    // A plist gives the executable as either `Program` or argv[0] of
    // `ProgramArguments`; `Program` takes precedence, matching launchd.
    let program = if let Some(p) = dict.get("Program").and_then(|v| v.as_string()) {
        p.to_string()
    } else {
        args.first().cloned().unwrap_or_default()
    };

    let disabled_key = dict.get("Disabled").and_then(|v| v.as_boolean()).unwrap_or(false);
    let stdout_path = dict.get("StandardOutPath").and_then(|v| v.as_string()).map(|s| s.to_string());
    let stderr_path = dict.get("StandardErrorPath").and_then(|v| v.as_string()).map(|s| s.to_string());

    let (pid, last_exit) = runtime.get(&label).cloned().unwrap_or((None, None));
    let loaded = runtime.contains_key(&label);
    let label_for_db = label.clone();

    Some(Job {
        label,
        kind: "launchd".into(),
        scope: scope.into(),
        source_path: path.to_string_lossy().into_owned(),
        source_group: d.group.into(),
        program,
        args,
        schedule_human: decode_launchd_schedule(dict),
        stdout_path,
        stderr_path,
        disabled_key,
        disabled_override: disabled_db.get(&label_for_db).copied(),
        loaded,
        pid,
        last_exit,
        apple,
    })
}

/// Run `launchctl list` and index its output by label.
///
/// # Returns
/// `label => (pid, last exit status)`. Both are `None` where launchctl printed
/// `-`: no pid means not currently running, no status means never run this
/// boot. Presence of a key at all is what tells us a job is *loaded*.
///
/// An empty map is returned if launchctl cannot be executed, so callers get
/// jobs with unknown runtime state rather than no jobs.
fn launchctl_list() -> HashMap<String, (Option<i64>, Option<i64>)> {
    let mut map = HashMap::new();
    let out = match Command::new("launchctl").arg("list").output() {
        Ok(o) => o,
        Err(_) => return map,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    // skip(1) drops the "PID Status Label" header row.
    for line in text.lines().skip(1) {
        // Tab-separated: PID \t Status \t Label. splitn(3) keeps labels
        // containing tabs (rare, but legal) intact in the last field.
        let cols: Vec<&str> = line.splitn(3, '\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let pid = cols[0].trim().parse::<i64>().ok();
        let status = cols[1].trim().parse::<i64>().ok();
        let label = cols[2].trim().to_string();
        if !label.is_empty() {
            map.insert(label, (pid, status));
        }
    }
    map
}

/// Query launchd's disabled database for one domain.
///
/// # Arguments
/// * `domain` — a launchd domain target, e.g. `"gui/501"` or `"system"`.
///
/// # Returns
/// `label => is disabled`. Empty if the command fails or the domain has no
/// entries; an absent label means "no override recorded".
fn print_disabled(domain: &str) -> HashMap<String, bool> {
    let out = match Command::new("launchctl").args(["print-disabled", domain]).output() {
        Ok(o) => o,
        Err(_) => return HashMap::new(),
    };
    parse_disabled_text(&String::from_utf8_lossy(&out.stdout))
}

/// Parse the body of `launchctl print-disabled`.
///
/// Split out from [`print_disabled`] purely so the format handling is testable
/// without invoking launchctl. The output looks like:
///
/// ```text
/// disabled services = {
///     "com.example.thing" => enabled
///     "com.example.other" => disabled
/// }
/// ```
///
/// Older macOS releases print `=> true` / `=> false` instead, where `true`
/// means disabled. Both spellings are accepted; any other right-hand side is
/// skipped rather than guessed at.
fn parse_disabled_text(text: &str) -> HashMap<String, bool> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let Some((lhs, rhs)) = line.split_once("=>") else { continue };
        let label = lhs.trim().trim_matches('"').to_string();
        if label.is_empty() {
            continue;
        }
        let disabled = match rhs.trim() {
            "disabled" | "true" => true,
            "enabled" | "false" => false,
            _ => continue,
        };
        map.insert(label, disabled);
    }
    map
}

/// Render a plist's schedule keys as one plain-English sentence.
///
/// A job can carry several triggers at once (run at load *and* every hour *and*
/// on a file change), so each recognised key contributes a clause and the
/// clauses are joined with `·` rather than one key winning.
///
/// Handles `RunAtLoad`, `StartInterval`, `StartCalendarInterval` (dictionary or
/// array of them), `WatchPaths`, `QueueDirectories`, `StartOnMount` and
/// `KeepAlive`.
///
/// # Returns
/// The joined description, or `"On demand / manual"` when no trigger key is
/// present — which for launchd genuinely means the job only runs when something
/// asks for it.
fn decode_launchd_schedule(dict: &plist::Dictionary) -> String {
    let mut parts: Vec<String> = Vec::new();

    if dict.get("RunAtLoad").and_then(|v| v.as_boolean()).unwrap_or(false) {
        parts.push("At login/boot".into());
    }

    if let Some(iv) = dict.get("StartInterval").and_then(|v| v.as_signed_integer()) {
        parts.push(format!("Every {}", human_duration(iv)));
    }

    if let Some(sci) = dict.get("StartCalendarInterval") {
        if let Some(d) = sci.as_dictionary() {
            parts.push(decode_calendar(d));
        } else if let Some(arr) = sci.as_array() {
            let each: Vec<String> = arr.iter().filter_map(|v| v.as_dictionary()).map(decode_calendar).collect();
            if !each.is_empty() {
                parts.push(each.join("; "));
            }
        }
    }

    if let Some(wp) = dict.get("WatchPaths").and_then(|v| v.as_array()) {
        let paths: Vec<String> = wp.iter().filter_map(|v| v.as_string()).map(|s| s.to_string()).collect();
        if !paths.is_empty() {
            parts.push(format!("When path changes: {}", paths.join(", ")));
        }
    }
    if let Some(qd) = dict.get("QueueDirectories").and_then(|v| v.as_array()) {
        let paths: Vec<String> = qd.iter().filter_map(|v| v.as_string()).map(|s| s.to_string()).collect();
        if !paths.is_empty() {
            parts.push(format!("When files queued in: {}", paths.join(", ")));
        }
    }
    if dict.get("StartOnMount").and_then(|v| v.as_boolean()).unwrap_or(false) {
        parts.push("On volume mount".into());
    }

    // KeepAlive is either `true` or a dictionary of conditions; both mean the
    // job is kept alive, and the conditions are too varied to summarise here.
    let keep_alive = matches!(dict.get("KeepAlive"), Some(Value::Boolean(true)) | Some(Value::Dictionary(_)));
    if keep_alive {
        parts.push("Kept running (restarted if it exits)".into());
    }

    if parts.is_empty() {
        "On demand / manual".into()
    } else {
        parts.join(" · ")
    }
}

/// Render one `StartCalendarInterval` dictionary as English.
///
/// launchd treats every *absent* key as a wildcard, so the checks run
/// coarsest-first — weekday, then day-of-month, then time alone — and the first
/// match wins. That mirrors how the schedule actually fires.
///
/// # Examples
/// - `{Hour: 9, Minute: 30}` -> `"Daily at 09:30"`
/// - `{Weekday: 1}` -> `"Every Monday at 00:00"`
/// - `{Month: 1, Day: 15}` -> `"On January 15 at 00:00"`
/// - `{Minute: 5}` -> `"Every hour at :05"`
fn decode_calendar(d: &plist::Dictionary) -> String {
    let get = |k: &str| d.get(k).and_then(|v| v.as_signed_integer());
    let minute = get("Minute");
    let hour = get("Hour");
    let day = get("Day");
    let weekday = get("Weekday");
    let month = get("Month");

    let time = match (hour, minute) {
        (Some(h), Some(m)) => format!("{:02}:{:02}", h, m),
        // An hour with no minute means on the hour.
        (Some(h), None) => format!("{:02}:00", h),
        // A minute with no hour repeats every hour.
        (None, Some(m)) => format!("every hour at :{:02}", m),
        (None, None) => String::new(),
    };

    if let Some(w) = weekday {
        return format!("Every {} at {}", weekday_name(w), if time.is_empty() { "00:00".into() } else { time });
    }
    if let Some(day) = day {
        let when = if let Some(mo) = month {
            format!("{} {}", month_name(mo), day)
        } else {
            format!("day {} of the month", day)
        };
        return format!("On {} at {}", when, if time.is_empty() { "00:00".into() } else { time });
    }
    if time.starts_with("every hour") {
        // Capitalise the phrase built above now that it starts the sentence.
        return format!("Every hour{}", &time["every hour".len()..]);
    }
    if !time.is_empty() {
        return format!("Daily at {}", time);
    }
    "On a calendar schedule".into()
}

/// Turn a `StartInterval` value in seconds into the largest whole unit.
///
/// Only exact multiples are promoted (7200 -> `"2 hours"`, but 5400 stays
/// `"5400 seconds"`), because "1.5 hours" reads worse than the raw number and
/// rounding would misstate the schedule.
///
/// # Returns
/// A bare quantity like `"2 hours"`; callers prefix it with "Every".
fn human_duration(secs: i64) -> String {
    if secs <= 0 {
        return format!("{}s", secs);
    }
    if secs % 86400 == 0 {
        let n = secs / 86400;
        return format!("{} day{}", n, plural(n));
    }
    if secs % 3600 == 0 {
        let n = secs / 3600;
        return format!("{} hour{}", n, plural(n));
    }
    if secs % 60 == 0 {
        let n = secs / 60;
        return format!("{} minute{}", n, plural(n));
    }
    format!("{} second{}", secs, plural(secs))
}

/// The plural suffix for `n`: empty for 1, `"s"` otherwise.
fn plural(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Name of a month from its 1-based number, e.g. `1` -> `"January"`.
///
/// Falls back to `"month N"` for out-of-range values so a malformed plist shows
/// something honest instead of panicking.
fn month_name(m: i64) -> String {
    let names = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    if (1..=12).contains(&m) {
        names[(m - 1) as usize].to_string()
    } else {
        format!("month {}", m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_spellings_of_the_disabled_database() {
        let text = "\tdisabled services = {\n\
                    \t\t\"com.example.off\" => disabled\n\
                    \t\t\"com.example.on\" => enabled\n\
                    \t\t\"com.example.legacy\" => true\n\
                    \t}\n";
        let map = parse_disabled_text(text);
        assert_eq!(map.get("com.example.off"), Some(&true));
        assert_eq!(map.get("com.example.on"), Some(&false));
        assert_eq!(map.get("com.example.legacy"), Some(&true));
        assert_eq!(map.len(), 3);
    }
}
