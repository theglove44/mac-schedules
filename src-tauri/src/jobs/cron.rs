//! Reading cron: the current user's crontab plus the system cron files, turned
//! into the same [`Job`] shape launchd produces.
//!
//! cron is effectively dead on modern macOS — most machines have nothing here —
//! so every source is optional and a missing one is silently skipped.

use std::path::Path;
use std::process::Command;

use super::types::{weekday_name, Job};

/// System-wide crontab files, which carry a user-name field before the command.
const SYSTEM_CRON_FILES: &[&str] = &["/etc/crontab"];

/// Directories of drop-in system crontabs, same format as [`SYSTEM_CRON_FILES`].
const SYSTEM_CRON_DIRS: &[&str] = &["/etc/cron.d"];

/// List every cron job visible to this user.
///
/// Sources, in order: `crontab -l` for the current user, then the system files.
/// The user's crontab is read through the `crontab` command rather than from
/// `/var/at/tabs`, because that directory is not readable without root.
///
/// # Returns
/// Every parsed line. Usually empty on macOS, which the UI shows as an empty
/// state rather than an error.
pub fn list_cron_jobs() -> Vec<Job> {
    let mut jobs = Vec::new();

    // A non-zero exit just means "no crontab for this user" — not a failure.
    if let Ok(out) = Command::new("crontab").arg("-l").output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_cron_text(&text, "crontab -l (this user)", false, &mut jobs);
        }
    }

    for f in SYSTEM_CRON_FILES {
        if let Ok(text) = std::fs::read_to_string(f) {
            parse_cron_text(&text, f, true, &mut jobs);
        }
    }
    for dir in SYSTEM_CRON_DIRS {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    parse_cron_text(&text, &entry.path().to_string_lossy(), true, &mut jobs);
                }
            }
        }
    }

    jobs
}

/// Parse crontab text, appending one [`Job`] per schedule line.
///
/// # Arguments
/// * `text` — raw crontab contents.
/// * `source` — where it came from; becomes [`Job::source_path`] and, shortened,
///   part of the label.
/// * `has_user_field` — true for system crontabs, which put a user name between
///   the schedule and the command. Getting this wrong would swallow the first
///   word of the command, so it is passed in per source rather than guessed.
/// * `jobs` — accumulator, appended to. Takes a `&mut Vec` so several sources
///   can build one list without intermediate allocations.
fn parse_cron_text(text: &str, source: &str, has_user_field: bool, jobs: &mut Vec<Job>) {
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Environment assignments (FOO=bar) are settings, not jobs. The `@`
        // check comes first because `@reboot FOO=bar cmd` is a real job line.
        if !line.starts_with('@') && line.split_whitespace().next().map_or(false, |w| w.contains('=')) {
            continue;
        }

        let (schedule_human, command) = if line.starts_with('@') {
            let mut it = line.splitn(2, char::is_whitespace);
            let kw = it.next().unwrap_or("");
            let rest = it.next().unwrap_or("").trim().to_string();
            (decode_cron_keyword(kw), rest)
        } else {
            // Five schedule fields, then everything else. splitn(6) keeps the
            // command intact including its own whitespace.
            let fields: Vec<&str> = line.splitn(6, char::is_whitespace).collect();
            if fields.len() < 6 {
                continue;
            }
            let (m, h, dom, mon, dow) = (fields[0], fields[1], fields[2], fields[3], fields[4]);
            let remainder = fields[5].trim();
            let command = if has_user_field {
                let mut it = remainder.splitn(2, char::is_whitespace);
                let user = it.next().unwrap_or("");
                let cmd = it.next().unwrap_or("").trim();
                format!("[{}] {}", user, cmd)
            } else {
                remainder.to_string()
            };
            (decode_cron(m, h, dom, mon, dow), command)
        };

        // cron has no equivalent of a launchd label, so file name plus line
        // number gives each row something stable and unique to key on.
        jobs.push(Job {
            label: format!("{}:{}", short_source(source), i + 1),
            kind: "cron".into(),
            scope: if has_user_field { "system".into() } else { "user".into() },
            source_path: source.to_string(),
            source_group: "Cron".into(),
            program: command.clone(),
            args: vec![],
            schedule_human,
            stdout_path: None,
            stderr_path: None,
            // cron has no disabled flag and no runtime state to report; a line
            // that exists is live, which is why `loaded` is unconditionally true.
            disabled_key: false,
            disabled_override: None,
            loaded: true,
            pid: None,
            last_exit: None,
            apple: false,
        });
    }
}

/// Reduce a source path to its file name for use in a label.
///
/// Falls back to the whole string when there is no file name — the user
/// crontab's source is the phrase `"crontab -l (this user)"`, not a path.
fn short_source(s: &str) -> String {
    Path::new(s).file_name().and_then(|f| f.to_str()).unwrap_or(s).to_string()
}

/// Expand a cron `@` shorthand into English, e.g. `"@daily"` -> `"Daily at 00:00"`.
///
/// The exact times are cron's own definitions, spelled out so the UI never
/// requires the reader to know what `@weekly` means. Unknown keywords are
/// returned verbatim rather than dropped.
fn decode_cron_keyword(kw: &str) -> String {
    match kw {
        "@reboot" => "At startup",
        "@yearly" | "@annually" => "Yearly (Jan 1, 00:00)",
        "@monthly" => "Monthly (day 1, 00:00)",
        "@weekly" => "Weekly (Sunday 00:00)",
        "@daily" | "@midnight" => "Daily at 00:00",
        "@hourly" => "Hourly (at :00)",
        other => other,
    }
    .to_string()
}

/// Describe the five cron schedule fields in English.
///
/// Only the common shapes are translated — a plain daily/hourly/weekly time.
/// Ranges, steps and lists (`1-5`, `*/15`, `0,30`) fall through to the raw
/// expression, which is more honest than a half-right paraphrase.
///
/// # Arguments
/// * `m` — minute, `h` — hour, `dom` — day of month, `mon` — month,
///   `dow` — day of week (0 and 7 both Sunday).
///
/// # Returns
/// Something like `"Daily at 09:30"`, or `"cron: <the five fields>"` when the
/// expression is more complex than the cases above.
fn decode_cron(m: &str, h: &str, dom: &str, mon: &str, dow: &str) -> String {
    let simple = |f: &str| f.parse::<i64>().ok();

    // Every day: distinguish a fixed time, every minute, and hourly.
    if dom == "*" && mon == "*" && dow == "*" {
        if let (Some(mm), Some(hh)) = (simple(m), simple(h)) {
            return format!("Daily at {:02}:{:02}", hh, mm);
        }
        if h == "*" && m == "*" {
            return "Every minute".into();
        }
        if h == "*" {
            if let Some(mm) = simple(m) {
                return format!("Every hour at :{:02}", mm);
            }
        }
    }
    // A single named weekday at a fixed time.
    if dom == "*" && mon == "*" && dow != "*" {
        if let (Some(mm), Some(hh), Some(d)) = (simple(m), simple(h), simple(dow)) {
            return format!("Every {} at {:02}:{:02}", weekday_name(d), hh, mm);
        }
    }
    format!("cron: {} {} {} {} {}", m, h, dom, mon, dow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_comments_and_environment_lines() {
        let mut jobs = Vec::new();
        parse_cron_text("# a comment\nPATH=/usr/bin\n\n30 9 * * * /bin/echo hi\n", "crontab", false, &mut jobs);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].schedule_human, "Daily at 09:30");
        assert_eq!(jobs[0].program, "/bin/echo hi");
    }

    #[test]
    fn system_crontabs_split_off_the_user_field() {
        let mut jobs = Vec::new();
        parse_cron_text("0 * * * * root /usr/sbin/thing --flag\n", "/etc/crontab", true, &mut jobs);
        assert_eq!(jobs[0].program, "[root] /usr/sbin/thing --flag");
        assert_eq!(jobs[0].schedule_human, "Every hour at :00");
        assert_eq!(jobs[0].label, "crontab:1");
    }

    #[test]
    fn complex_expressions_fall_through_to_the_raw_fields() {
        assert_eq!(decode_cron("*/15", "*", "*", "*", "*"), "cron: */15 * * * *");
    }
}
