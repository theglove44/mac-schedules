//! Everything that *changes* something: enable, disable, delete, and the two
//! Finder helpers.
//!
//! # Safety
//! Two rules apply to every mutating call here, enforced by [`guard_protected`]
//! before anything runs:
//! - `com.apple.*` labels and the `apple` scope are refused outright, because
//!   disabling Apple's own jobs can leave macOS unable to boot cleanly.
//! - Anything under `/System` is refused, because that volume is sealed.
//!
//! Beyond that, user agents run `launchctl` directly (no password), while
//! system daemons and root-owned directories go through one `osascript … with
//! administrator privileges` call, which shows the native authentication dialog.

use std::path::Path;
use std::process::Command;

use super::types::uid;

/// One `launchctl` invocation within a larger operation.
///
/// Only `critical` steps decide overall success. `bootstrap`/`bootout` routinely
/// fail when a job is already in the requested state, which is not an error for
/// our purposes — but `enable`/`disable`, which write the persistent state, are.
struct Step {
    /// Arguments after the `launchctl` binary itself.
    args: Vec<String>,
    /// Whether a non-zero exit should abort the operation.
    critical: bool,
}

/// Build a [`Step`], copying the borrowed arguments into owned strings.
fn step(args: &[&str], critical: bool) -> Step {
    Step { args: args.iter().map(|s| s.to_string()).collect(), critical }
}

/// Refuse to touch Apple-owned jobs or the sealed system volume.
///
/// Both the label and the scope are checked, since either alone can be
/// misleading: an Apple label can sit in a user directory, and a third-party
/// label can sit under `/System`.
///
/// # Returns
/// `Ok(())` if the job may be modified, otherwise `Err` with a message written
/// for the user, not for a log.
fn guard_protected(label: &str, path: &str, scope: &str) -> Result<(), String> {
    if label.starts_with("com.apple.") || scope == "apple" {
        return Err("Apple system job — refused. Changing com.apple.* jobs can destabilise macOS.".into());
    }
    if path.starts_with("/System/") {
        return Err("Job lives in /System — refused. That volume is sealed and read-only.".into());
    }
    Ok(())
}

/// Enable or disable a launchd job.
///
/// The `enable`/`disable` half is what persists — it writes launchd's disabled
/// database, never the plist file — while `bootstrap`/`bootout` only make the
/// change take effect in the current session. That is why the persistent step is
/// the critical one and the session step is best-effort.
///
/// # Arguments
/// * `label` — the job's launchd label.
/// * `path` — its plist, needed by `bootstrap` when re-enabling.
/// * `scope` — `"user"`, `"global"`, `"system"` or `"apple"`; `"system"` selects
///   the `system` launchd domain and triggers the admin prompt.
/// * `enable` — `true` to enable, `false` to disable.
///
/// # Returns
/// `Ok` with a short confirmation, or `Err` with launchctl's message (or
/// `"Cancelled"` if the user dismissed the authentication dialog).
///
/// # Safety
/// Apple jobs and `/System` are refused — see [`guard_protected`].
pub fn set_job_enabled(label: &str, path: &str, scope: &str, enable: bool) -> Result<String, String> {
    guard_protected(label, path, scope)?;

    let is_daemon = scope == "system";
    let domain = if is_daemon { "system".to_string() } else { format!("gui/{}", uid()) };
    let target = format!("{}/{}", domain, label);

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

/// Unload a job and move its plist to the Trash.
///
/// Deliberately reversible: the file is *moved*, never unlinked, so the user can
/// drag it back out of the Trash. The disabled-database entry is cleared on the
/// way out so a future job reusing the label doesn't inherit a stale "disabled".
///
/// # Arguments
/// * `label`, `path`, `scope` — as for [`set_job_enabled`].
///
/// # Returns
/// `Ok` with a confirmation, or `Err` if the file is missing, the Trash is
/// unwritable, or the privileged move was declined.
///
/// # Safety
/// Apple jobs and `/System` are refused. Unlike [`set_job_enabled`], *global*
/// agents also need administrator rights here, because moving the file requires
/// write access to root-owned `/Library/LaunchAgents`.
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

    // Both steps are best-effort: a job that was never loaded, or has no
    // disabled-db entry, is still perfectly deletable.
    let steps = vec![
        step(&["bootout", &target], false),
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

/// Pick a destination inside `~/.Trash` for a file being deleted.
///
/// If the name is already taken, a Unix timestamp is appended rather than
/// overwriting — the whole point of moving to the Trash is that nothing is lost.
///
/// # Returns
/// The absolute destination path, or `Err` if the home directory or Trash
/// cannot be reached.
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

/// Run a sequence of steps, choosing the privileged path when required.
///
/// # Arguments
/// * `steps` — executed in order; a failing critical step aborts the rest.
/// * `privileged` — `true` to run the whole chain under one admin prompt.
fn run_steps(steps: &[Step], privileged: bool) -> Result<(), String> {
    if privileged {
        run_privileged(&script_for(steps))
    } else {
        run_direct(steps)
    }
}

/// Run the steps as the current user, one `launchctl` process each.
///
/// No shell is involved, so arguments are passed verbatim. This matters:
/// shell-quoting a launchd target makes launchctl see literal quote marks and
/// answer `Unrecognized target specifier.`
///
/// # Returns
/// `Err` with launchctl's stderr (or a synthesised message when it printed
/// nothing) the first time a critical step fails; non-critical failures are
/// ignored.
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

/// Render the steps as a single shell script for the privileged path.
///
/// The whole chain goes into one script so the user authenticates **once**
/// rather than per step. Steps are joined with `&&` so a critical failure stops
/// the rest; best-effort steps are wrapped in `{ … ; } || true` so they cannot
/// break that chain. Arguments are shell-quoted here — unlike [`run_direct`] —
/// because this genuinely does pass through a shell.
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

/// Execute a shell script as an administrator via `osascript`.
///
/// AppleScript is used rather than `sudo` because it produces the native macOS
/// authentication dialog — no terminal, no password handling in this process.
///
/// # Returns
/// `Ok(())` on success, `Err("Cancelled")` if the user dismissed the dialog
/// (AppleScript reports that as error -128), otherwise `Err` with stderr.
fn run_privileged(script: &str) -> Result<(), String> {
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        // The script is being embedded in an AppleScript string literal, so
        // backslashes and quotes need escaping a second time.
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

/// Wrap a string in single quotes for safe use in a shell command.
///
/// Single quotes suppress every kind of expansion; the only character needing
/// care is the quote itself, closed and re-opened as `'\''`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Show a file in Finder, selected in its containing folder.
///
/// # Returns
/// `Ok(())` if `open` ran; `Err` with the OS error if it could not be launched.
pub fn reveal_in_finder(path: &str) -> Result<(), String> {
    Command::new("open").arg("-R").arg(path).status().map_err(|e| e.to_string())?;
    Ok(())
}

/// Open a file with whichever application macOS associates with it.
///
/// Used for job log files, so the user lands in Console or their editor.
///
/// # Returns
/// `Ok(())` if `open` ran; `Err` with the OS error if it could not be launched.
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
    fn guard_refuses_apple_and_system_volume() {
        assert!(guard_protected("com.apple.thing", "/Library/LaunchAgents/x.plist", "user").is_err());
        assert!(guard_protected("com.me.thing", "/System/Library/LaunchAgents/x.plist", "user").is_err());
        assert!(guard_protected("com.me.thing", "/Library/LaunchAgents/x.plist", "user").is_ok());
    }
}
