//! Job enumeration and management for launchd and cron.
//!
//! The data layer behind every Tauri command in `lib.rs`:
//! - list all scheduled jobs from launchd and cron ([`list_launchd_jobs`],
//!   [`list_cron_jobs`]);
//! - decode their schedules into plain English for the UI;
//! - enable, disable and delete launchd jobs ([`set_job_enabled`],
//!   [`delete_job`]);
//! - reveal a job's file in Finder or open its log ([`reveal_in_finder`],
//!   [`open_path`]).
//!
//! # Layout
//! Reading and writing are kept apart on purpose: the `launchd` and `cron`
//! modules only ever read, so they can be careless about failure, while
//! `actions` is the single place where anything on the machine changes.
//!
//! # Safety
//! - Every read path is best-effort — an unreadable file or directory is
//!   skipped, never fatal, so one bad plist cannot hide the rest.
//! - `com.apple.*` jobs and anything under `/System` are refused by every
//!   mutating operation.
//! - Changes outside the user's own domain go through one native administrator
//!   authentication dialog.

mod actions;
mod cron;
mod launchd;
mod types;

pub use actions::{delete_job, open_path, reveal_in_finder, set_job_enabled};
pub use cron::list_cron_jobs;
pub use launchd::list_launchd_jobs;
pub use types::Job;
