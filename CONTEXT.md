# CONTEXT

The vocabulary this codebase uses. Terms here are the ones to reach for in code,
commits and issues; reaching for a synonym is how two names for one idea start.

## Job

One scheduled thing on this Mac: a launchd plist, or a single line of a crontab.
The only type that crosses the bridge to the frontend.

## Domain

A directory launchd loads job definitions from. There are five, fixed by macOS,
listed in `DOMAINS` (`jobs/launchd.rs`). Not to be confused with a *launchd
domain target* (`gui/501`, `system`), which is the address a job is acted on at —
see `Scope::launchd_domain`.

## Scope

Which part of the machine a job belongs to: `user`, `global`, `system` or
`apple`. Decides the launchd domain target, and how much authority a change
needs. `apple` is a protection marker rather than a location — an Apple-labelled
job in `/Library` is still Apple's.

## Action

A change the user can ask for: **toggle** (enable/disable) or **delete**. Named
separately from the commands that perform it because the authority a change
needs depends on which action it is.

## Status

What a job is doing, as one value: `running`, `idle`, `not_loaded`, `disabled`,
or `scheduled` for cron. Decided in `jobs/policy.rs`. Each variant carries only
the facts that variant can have, so "running with no pid" cannot be expressed.

## Permission

Whether one action on one job is `editable`, `needs_admin`, or `refused` with a
reason. Every job answers for every action, so the frontend never works out
which rule applies to which button.

## Disabled database

launchd's own record of which jobs are switched off, read with `launchctl
print-disabled` and written by `launchctl enable`/`disable`. It is **not** the
plist's `Disabled` key, and it wins wherever it has an entry.
