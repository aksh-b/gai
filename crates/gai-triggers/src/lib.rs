//! Triggers: things that start gai runs on their own.
//!
//! - **cron**: on a schedule, in a time zone. The next time is stored, so a
//!   restart neither loses a firing nor fires twice; a time missed while
//!   the server was down fires once on start (or is skipped).
//! - **webhook**: `POST /v1/webhooks/{id}`, signed with the trigger's
//!   secret (HMAC-SHA256 of the body, GitHub style) or carrying it as a
//!   token.
//! - **file**: when files under a folder are created or changed.
//!
//! Each trigger targets an agent or a workflow. Its input is a minijinja
//! template over what fired it (`payload`, `path`, `now`...). Runs go
//! through the persisted background queue, and each firing is recorded.
//!
//! ```yaml
//! name: weekday-digest
//! kind: cron
//! schedule: "0 8 * * 1-5"
//! timezone: Europe/Berlin
//! target: { workflow: digest }
//! inputs: { day: "{{ date }}" }
//! ```

mod engine;
mod spec;

pub use engine::{Fired, TriggerError, Triggers, verify_signature};
pub use spec::{CatchUp, Source, Target, TriggerSpec};
