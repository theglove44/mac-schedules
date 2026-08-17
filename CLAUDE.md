# classic-schedules

A Mac desktop app that shows every scheduled/background job on this machine —
**launchd** (LaunchAgents + LaunchDaemons) and **cron** — in a classic Mac OS
8/9 "Platinum" interface. Read-and-inspect plus enable/disable.

## Stack

- **Tauri v2** (Rust backend + system WebView), vanilla HTML/CSS/JS frontend.
- No JS framework, no bundler — `frontendDist` serves `src/` directly.
- Backend crates: `plist` (parse plists), `dirs` (home dir), `serde`.

## Layout

- `src/` — frontend. `index.html`, `main.js`, `styles/platinum.css` (hand-written
  Platinum theme; not System.css).
- `src-tauri/src/jobs/` — all data logic, split read-vs-write: `types.rs` (the
  `Job`/`Domain` structs and shared helpers), `launchd.rs` (enumerate domains,
  parse plists, merge `launchctl list`/`print-disabled` state, decode
  schedules), `cron.rs` (crontab parsing + decoding), `actions.rs`
  (enable/disable/delete, Finder helpers), `mod.rs` (public API).
- `src-tauri/src/lib.rs` — Tauri command wiring.

## Commands (Rust → JS)

- `get_launchd_jobs`, `get_cron_jobs` — enumerate + decode.
- `set_enabled(label, path, scope, enable)` — toggle. User agents run
  `launchctl` directly (no password); system daemons run via `osascript … with
  administrator privileges` (native auth dialog). **`com.apple.*` jobs and
  anything under `/System` are hard-refused** to avoid destabilising macOS.
- `delete_job(label, path, scope)` — bootout, clear the disabled-db entry, then
  move the plist to `~/.Trash` (never unlinked, so it stays reversible). Same
  guards and privilege split as `set_enabled`, except global agents also need
  admin because `/Library/LaunchAgents` is root-owned.
- `reveal(path)`, `open_file(path)` — Finder reveal / open logs.

## Enabled state has two sources

`launchctl enable/disable` writes launchd's own **disabled database**, not the
plist's `Disabled` key — the plist on disk never changes. `Job.disabled_override`
holds the database value (parsed from `launchctl print-disabled`, per domain) and
overrides `Job.disabled_key`. The frontend's `isDisabled()` applies that
precedence; using `disabled_key` alone makes toggles look like no-ops.

launchctl arguments must be passed as separate `Command` args, never shell-quoted
into one string — a quoted target yields `Unrecognized target specifier.` Only
the `osascript` path goes through a shell and needs `shell_quote`.

## Run / build

```
npm install
npm run tauri dev      # dev window with hot frontend
npm run tauri build    # produces src-tauri/target/release/bundle/macos/Schedules.app
```

## Notes

- Window uses `decorations: false` — the Platinum title bar is custom; dragging
  via `data-tauri-drag-region`, close/minimise via the JS window API (perms in
  `src-tauri/capabilities/default.json`).
- cron is typically empty on modern macOS; the Cron tab shows an empty state.
- Schedule decoding lives in `decode_launchd_schedule` (`jobs/launchd.rs`) and
  `decode_cron` (`jobs/cron.rs`) — extend there for new key types.
