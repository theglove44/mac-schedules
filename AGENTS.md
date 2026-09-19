# Classic Schedules

- Mac app for viewing launchd and cron jobs in a classic Mac OS style.
- Apple jobs may be viewed but must remain protected.
- Keep job changes reversible and clearly confirmed.
- Rebuild and relaunch the app after interface changes.
- Use `npm run tauri build -- --bundles app` when an app bundle is needed without a DMG.
- Do not hand-edit generated app bundles.

## Agent skills

### Issue tracker

Issues live as GitHub issues in `theglove44/mac-schedules`, managed with the
`gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical triage labels, unchanged: `needs-triage`, `needs-info`,
`ready-for-agent`, `ready-for-human`, `wontfix`. See
`docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root, created lazily.
See `docs/agents/domain.md`.
