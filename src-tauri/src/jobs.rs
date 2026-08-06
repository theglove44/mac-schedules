// Data layer: enumerate + decode launchd and cron scheduled jobs, and
// enable/disable launchd jobs. All read paths are best-effort — an unreadable
// file is skipped, never fatal.

use plist::Value;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Serialize, Clone)]
pub struct Job {
    pub label: String,
    pub kind: String,   // "launchd" | "cron"
    pub scope: String,  // "user" | "global" | "system" | "apple"
    pub source_path: String,
    pub source_group: String, // human label for the source directory / file
    pub program: String,
    pub args: Vec<String>,
    pub schedule_human: String,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub disabled_key: bool, // Disabled=true inside the plist
    // launchd's own disabled database (`launchctl print-disabled`). This is what
    // `launchctl enable/disable` actually writes, and it overrides the plist key.
    // None = no entry for this label.
    pub disabled_override: Option<bool>,
    pub loaded: bool,       // present in `launchctl list`
    pub pid: Option<i64>,
    pub last_exit: Option<i64>,
    pub apple: bool,        // com.apple.* — protected, never toggled
}

// ---------------------------------------------------------------------------
// launchd
// ---------------------------------------------------------------------------

struct Domain {
    dir: &'static str, // path with ~ expanded later for the user one
    scope: &'static str,
    group: &'static str,
    home: bool,   // prefix with home dir
    daemon: bool, // lives in the `system` launchd domain rather than `gui/<uid>`
}

const DOMAINS: &[Domain] = &[
    Domain { dir: "Library/LaunchAgents", scope: "user", group: "User Agents", home: true, daemon: false },
    Domain { dir: "/Library/LaunchAgents", scope: "global", group: "Global Agents", home: false, daemon: false },
    Domain { dir: "/Library/LaunchDaemons", scope: "system", group: "System Daemons", home: false, daemon: true },
    Domain { dir: "/System/Library/LaunchAgents", scope: "apple", group: "Apple Agents", home: false, daemon: false },
    Domain { dir: "/System/Library/LaunchDaemons", scope: "apple", group: "Apple Daemons", home: false, daemon: true },
];

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

fn parse_launchd_plist(
    path: &Path,
    d: &Domain,
    runtime: &HashMap<String, (Option<i64>, Option<i64>)>,
    disabled_db: &HashMap<String, bool>,
) -> Option<Job> {
    let value = Value::from_file(path).ok()?;
    let dict = value.as_dictionary()?;

    let label = dict
        .get("Label")
        .and_then(|v| v.as_string())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string());

    let apple = label.starts_with("com.apple.");
    let scope = if apple { "apple" } else { d.scope };

    // Program / ProgramArguments
    let mut args: Vec<String> = Vec::new();
    if let Some(pa) = dict.get("ProgramArguments").and_then(|v| v.as_array()) {
        for a in pa {
            if let Some(s) = a.as_string() {
                args.push(s.to_string());
            }
        }
    }
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

// Parse `launchctl list` -> label => (pid, last_exit_status)
fn launchctl_list() -> HashMap<String, (Option<i64>, Option<i64>)> {
    let mut map = HashMap::new();
    let out = match Command::new("launchctl").arg("list").output() {
        Ok(o) => o,
        Err(_) => return map,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().skip(1) {
        // columns: PID \t Status \t Label
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

// Parse `launchctl print-disabled <domain>` -> label => disabled?
//
// Output looks like:
//     disabled services = {
//         "com.example.thing" => enabled
//         "com.example.other" => disabled
//     }
// Older macOS releases print `=> true` / `=> false` instead, where true means
// disabled. Both spellings are accepted.
fn print_disabled(domain: &str) -> HashMap<String, bool> {
    let out = match Command::new("launchctl").args(["print-disabled", domain]).output() {
        Ok(o) => o,
        Err(_) => return HashMap::new(),
    };
    parse_disabled_text(&String::from_utf8_lossy(&out.stdout))
}

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

    let keep_alive = match dict.get("KeepAlive") {
        Some(Value::Boolean(true)) => true,
        Some(Value::Dictionary(_)) => true,
        _ => false,
    };
    if keep_alive {
        parts.push("Kept running (restarted if it exits)".into());
    }

    if parts.is_empty() {
        "On demand / manual".into()
    } else {
        parts.join(" · ")
    }
}

fn decode_calendar(d: &plist::Dictionary) -> String {
    let get = |k: &str| d.get(k).and_then(|v| v.as_signed_integer());
    let minute = get("Minute");
    let hour = get("Hour");
    let day = get("Day");
    let weekday = get("Weekday");
    let month = get("Month");

    let time = match (hour, minute) {
        (Some(h), Some(m)) => format!("{:02}:{:02}", h, m),
        (Some(h), None) => format!("{:02}:00", h),
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
        // "every hour at :MM"
        let cap = format!("Every hour{}", &time["every hour".len()..]);
        return cap;
    }
    if !time.is_empty() {
        return format!("Daily at {}", time);
    }
    "On a calendar schedule".into()
}

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

fn plural(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn weekday_name(w: i64) -> String {
    // launchd: 0 or 7 = Sunday
    let names = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let idx = ((w % 7 + 7) % 7) as usize;
    names.get(idx).unwrap_or(&"?").to_string()
}

fn month_name(m: i64) -> String {
    let names = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    if (1..=12).contains(&m) {
        names[(m - 1) as usize].to_string()
    } else {
        format!("month {}", m)
    }
}

// ---------------------------------------------------------------------------
// cron
// ---------------------------------------------------------------------------

pub fn list_cron_jobs() -> Vec<Job> {
    let mut jobs = Vec::new();

    // Current user's crontab
    if let Ok(out) = Command::new("crontab").arg("-l").output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_cron_text(&text, "crontab -l (this user)", false, &mut jobs);
        }
    }

    // System cron files
    let files = ["/etc/crontab"];
    for f in files {
        if let Ok(text) = std::fs::read_to_string(f) {
            parse_cron_text(&text, f, true, &mut jobs);
        }
    }
    for dir in ["/etc/cron.d"] {
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

fn parse_cron_text(text: &str, source: &str, has_user_field: bool, jobs: &mut Vec<Job>) {
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Skip environment assignments (FOO=bar)
        if !line.starts_with('@') && line.split_whitespace().next().map_or(false, |w| w.contains('=')) {
            continue;
        }

        let (schedule_human, command) = if line.starts_with('@') {
            let mut it = line.splitn(2, char::is_whitespace);
            let kw = it.next().unwrap_or("");
            let rest = it.next().unwrap_or("").trim().to_string();
            (decode_cron_keyword(kw), rest)
        } else {
            // 5 schedule fields, then the remainder (which for system files
            // begins with a user name before the command).
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
            disabled_key: false,
            disabled_override: None,
            loaded: true,
            pid: None,
            last_exit: None,
            apple: false,
        });
    }
}

fn short_source(s: &str) -> String {
    Path::new(s).file_name().and_then(|f| f.to_str()).unwrap_or(s).to_string()
}

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

fn decode_cron(m: &str, h: &str, dom: &str, mon: &str, dow: &str) -> String {
    let simple = |f: &str| f.parse::<i64>().ok();
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
    if dom == "*" && mon == "*" && dow != "*" {
        if let (Some(mm), Some(hh), Some(d)) = (simple(m), simple(h), simple(dow)) {
            return format!("Every {} at {:02}:{:02}", weekday_name(d), hh, mm);
        }
    }
    format!("cron: {} {} {} {} {}", m, h, dom, mon, dow)
}

// ---------------------------------------------------------------------------
// enable / disable
// ---------------------------------------------------------------------------

fn uid() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".into())
}

// One launchctl invocation. `critical` steps decide success/failure; the others
// are best-effort (bootstrap/bootout routinely fail when a job is already in the
// requested state, which is not an error for our purposes).
struct Step {
    args: Vec<String>,
    critical: bool,
}

fn step(args: &[&str], critical: bool) -> Step {
    Step { args: args.iter().map(|s| s.to_string()).collect(), critical }
}

// Reject anything Apple-owned or living inside the sealed system volume.
fn guard_protected(label: &str, path: &str, scope: &str) -> Result<(), String> {
    if label.starts_with("com.apple.") || scope == "apple" {
        return Err("Apple system job — refused. Changing com.apple.* jobs can destabilise macOS.".into());
    }
    if path.starts_with("/System/") {
        return Err("Job lives in /System — refused. That volume is sealed and read-only.".into());
    }
    Ok(())
}

// Returns Ok(message) or Err(message).
pub fn set_job_enabled(label: &str, path: &str, scope: &str, enable: bool) -> Result<String, String> {
    guard_protected(label, path, scope)?;

    let is_daemon = scope == "system";
    let domain = if is_daemon { "system".to_string() } else { format!("gui/{}", uid()) };
    let target = format!("{}/{}", domain, label);

    // The enable/disable half is what persists (it writes launchd's disabled
    // database); bootstrap/bootout only make the change take effect now.
    let steps = if enable {
        vec![
            step(&["enable", &target], true),
            step(&["bootstrap", &domain, path], false),
        ]
    } else {
        vec![
            step(&["bootout", &target], false),
            step(&["disable", &target], true),
        ]
    };

    run_steps(&steps, is_daemon)
        .map(|_| format!("{} {}", label, if enable { "enabled" } else { "disabled" }))
}

// Unload the job, clear its disabled-database entry, then move the plist to the
// Trash. Deliberately reversible: nothing is unlinked, the file can be put back.
pub fn delete_job(label: &str, path: &str, scope: &str) -> Result<String, String> {
    guard_protected(label, path, scope)?;

    let src = Path::new(path);
    if !src.is_file() {
        return Err(format!("No such file: {}", path));
    }

    let is_daemon = scope == "system";
    let domain = if is_daemon { "system".to_string() } else { format!("gui/{}", uid()) };
    let target = format!("{}/{}", domain, label);
    let dest = trash_destination(src)?;

    // Direct filesystem access only works where the containing directory is
    // ours; /Library and /Library/LaunchDaemons are root-owned.
    let privileged = scope != "user";

    let steps = vec![
        step(&["bootout", &target], false),
        // Leave no stale disabled-db entry behind for a future job of this name.
        step(&["enable", &target], false),
    ];
    run_steps(&steps, privileged)?;

    if privileged {
        run_privileged(&format!("/bin/mv {} {}", shell_quote(path), shell_quote(&dest)))?;
    } else {
        std::fs::rename(src, &dest).map_err(|e| format!("Could not move to Trash: {}", e))?;
    }

    Ok(format!("{} moved to Trash", label))
}

// ~/.Trash/<name>, uniquified if something with that name is already there.
fn trash_destination(src: &Path) -> Result<String, String> {
    let home = dirs::home_dir().ok_or("Could not locate home directory")?;
    let trash = home.join(".Trash");
    std::fs::create_dir_all(&trash).map_err(|e| format!("Could not open Trash: {}", e))?;

    let name = src.file_name().and_then(|n| n.to_str()).ok_or("Bad file name")?;
    let mut dest = trash.join(name);
    if dest.exists() {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("job");
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        dest = trash.join(format!("{}-{}.plist", stem, secs));
    }
    Ok(dest.to_string_lossy().into_owned())
}

fn run_steps(steps: &[Step], privileged: bool) -> Result<(), String> {
    if privileged {
        run_privileged(&script_for(steps))
    } else {
        run_direct(steps)
    }
}

// For user agents: run launchctl directly, no shell, so arguments are passed
// verbatim (quoting them here would make launchctl see literal quote marks).
fn run_direct(steps: &[Step]) -> Result<(), String> {
    for s in steps {
        let out = Command::new("launchctl").args(&s.args).output();
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) if s.critical => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                return Err(if err.is_empty() {
                    format!("launchctl {} failed", s.args.join(" "))
                } else {
                    err
                });
            }
            Err(e) if s.critical => return Err(e.to_string()),
            _ => {}
        }
    }
    Ok(())
}

// For system daemons: one authenticated osascript prompt runs the whole chain.
// Best-effort steps are swallowed with `|| true` so the overall exit status
// reflects only the critical ones.
fn script_for(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|s| {
            let cmd = std::iter::once("/bin/launchctl".to_string())
                .chain(s.args.iter().map(|a| shell_quote(a)))
                .collect::<Vec<_>>()
                .join(" ");
            if s.critical { cmd } else { format!("{{ {} ; }} || true", cmd) }
        })
        .collect::<Vec<_>>()
        .join(" && ")
}

fn run_privileged(script: &str) -> Result<(), String> {
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        script.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let out = Command::new("osascript")
        .arg("-e")
        .arg(&apple_script)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if err.contains("User canceled") || err.contains("-128") {
            Err("Cancelled".into())
        } else {
            Err(if err.is_empty() { "Privileged command failed".into() } else { err })
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------------
// misc helpers exposed to the frontend
// ---------------------------------------------------------------------------

pub fn reveal_in_finder(path: &str) -> Result<(), String> {
    Command::new("open").arg("-R").arg(path).status().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn open_path(path: &str) -> Result<(), String> {
    Command::new("open").arg(path).status().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_quotes_targets_and_swallows_best_effort_steps() {
        let steps = vec![
            step(&["bootout", "gui/501/com.example.job"], false),
            step(&["disable", "gui/501/com.example.job"], true),
        ];
        let expected = "{ /bin/launchctl 'bootout' 'gui/501/com.example.job' ; } || true && /bin/launchctl 'disable' 'gui/501/com.example.job'";
        assert_eq!(script_for(&steps), expected);
    }

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

    #[test]
    fn guard_refuses_apple_and_system_volume() {
        assert!(guard_protected("com.apple.thing", "/Library/LaunchAgents/x.plist", "user").is_err());
        assert!(guard_protected("com.me.thing", "/System/Library/LaunchAgents/x.plist", "user").is_err());
        assert!(guard_protected("com.me.thing", "/Library/LaunchAgents/x.plist", "user").is_ok());
    }
}
