//! What a trigger is: when it fires and what it starts.

use std::str::FromStr;

use chrono::{DateTime, Utc};
use croner::Cron;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// No deny_unknown_fields: serde cannot combine it with the flattened source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TriggerSpec {
    /// Unique; letters, digits, `-` and `_`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub source: Source,
    pub target: Target,
    /// For agents: the task, a minijinja template. Variables: `trigger`,
    /// `reason`, `now`, `date`, and per kind `payload` (webhooks), `path`,
    /// `event` and `content` (files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// For workflows: inputs. Strings are templates; a string that is one
    /// `{{ expression }}` keeps the expression's type (`{{ payload }}`).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub inputs: Map<String, Value>,
    /// Run `approve` tools without asking. Otherwise those calls wait in
    /// the approvals inbox.
    #[serde(default)]
    pub auto_approve: bool,
    /// Do not start a run while this trigger's previous run is unfinished
    /// (default true).
    #[serde(default = "yes")]
    pub skip_if_running: bool,
    /// Send the run's answer and the files it made (by email or webhook)
    /// when it finishes: a scheduled report.
    #[serde(default, skip_serializing_if = "gai_tools::notify::Delivery::is_empty")]
    pub deliver: gai_tools::notify::Delivery,
    #[serde(default = "yes", skip_serializing)]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// When a trigger fires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Source {
    Cron {
        /// Five fields (minute hour day month weekday), six with seconds,
        /// or `@hourly`, `@daily`...
        schedule: String,
        /// IANA zone the schedule is read in (default UTC).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
        /// What to do about a time missed while gai was not running.
        #[serde(default)]
        catch_up: CatchUp,
    },
    Webhook {},
    File {
        /// A folder (watched recursively) or a file.
        path: String,
        /// Only file names matching this, with `*` wildcards (`*.pdf`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        /// Wait this long after the last change before firing (default 1000).
        #[serde(default = "debounce")]
        debounce_ms: u64,
    },
}

fn debounce() -> u64 {
    1_000
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Cron { .. } => "cron",
            Self::Webhook {} => "webhook",
            Self::File { .. } => "file",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CatchUp {
    /// Fire once for everything missed (default).
    #[default]
    Once,
    /// Wait for the next scheduled time.
    Skip,
}

/// What a trigger starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// An agent reference: `name`, `name@v3`, `name@prod` or a file.
    Agent(String),
    /// A workflow reference: `name`, `name@v3`, `name@prod` or a file.
    Workflow(String),
}

impl TriggerSpec {
    pub fn parse(text: &str) -> Result<Self, String> {
        // Through JSON: serde_yaml reads enums as YAML tags (!agent), while
        // the API and files use plain maps ({agent: name}).
        let v: Value = serde_yaml::from_str(text).map_err(|e| format!("trigger: {e}"))?;
        serde_json::from_value(v).map_err(|e| format!("trigger: {e}"))
    }

    /// Check what can be checked without the runtime.
    pub fn validate(&self) -> Result<(), String> {
        self.deliver.validate()?;
        if self.name.is_empty()
            || self.name.len() > 64
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("name: 1-64 letters, digits, - or _".into());
        }
        if let Source::Cron {
            schedule, timezone, ..
        } = &self.source
        {
            cron(schedule)?;
            zone(timezone.as_deref())?;
        }
        if let Source::File { path, .. } = &self.source
            && path.trim().is_empty()
        {
            return Err("path is empty".into());
        }
        let env = minijinja::Environment::new();
        if let Some(t) = &self.input {
            env.template_from_str(t)
                .map_err(|e| format!("input: {e}"))?;
        }
        Ok(())
    }

    /// For schedules: the first time strictly after `after`.
    pub fn next_after(&self, after: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, String> {
        let Source::Cron {
            schedule, timezone, ..
        } = &self.source
        else {
            return Ok(None);
        };
        let c = cron(schedule)?;
        let tz = zone(timezone.as_deref())?;
        let next = c
            .find_next_occurrence(&after.with_timezone(&tz), false)
            .map_err(|e| format!("schedule: {e}"))?;
        Ok(Some(next.with_timezone(&Utc)))
    }

    /// A schedule in words ("At 08:00 on Monday through Friday").
    pub fn describe(&self) -> Option<String> {
        match &self.source {
            Source::Cron {
                schedule, timezone, ..
            } => cron(schedule).ok().map(|c| {
                format!(
                    "{}{}",
                    c.describe(),
                    timezone
                        .as_deref()
                        .map(|z| format!(" ({z})"))
                        .unwrap_or_else(|| " (UTC)".into())
                )
            }),
            _ => None,
        }
    }
}

fn cron(schedule: &str) -> Result<Cron, String> {
    Cron::from_str(schedule.trim()).map_err(|e| format!("schedule {schedule:?}: {e}"))
}

fn zone(tz: Option<&str>) -> Result<chrono_tz::Tz, String> {
    match tz {
        None => Ok(chrono_tz::UTC),
        Some(z) => z
            .parse()
            .map_err(|_| format!("timezone {z:?} is not an IANA zone like Europe/Berlin")),
    }
}

/// `*` wildcards against a file name.
pub(crate) fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], n: &[u8]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some(b'*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_validates_and_schedules() {
        let t = TriggerSpec::parse(
            "name: digest\nkind: cron\nschedule: \"0 8 * * 1-5\"\ntimezone: Europe/Berlin\ntarget: {workflow: digest}\ninputs: {day: \"{{ date }}\"}\n",
        )
        .unwrap();
        t.validate().unwrap();
        assert!(t.enabled && t.skip_if_running);
        // Friday 2026-10-16 10:00 UTC (12:00 in Berlin) → Monday 08:00 Berlin.
        let after = "2026-10-16T10:00:00Z".parse().unwrap();
        let next = t.next_after(after).unwrap().unwrap();
        assert_eq!(next.to_rfc3339(), "2026-10-19T06:00:00+00:00");
        assert!(t.describe().unwrap().contains("Europe/Berlin"));

        let bad = TriggerSpec::parse(
            "name: x\nkind: cron\nschedule: \"61 * * * *\"\ntarget: {agent: a}\n",
        )
        .unwrap();
        assert!(bad.validate().is_err());
        let bad_tz = TriggerSpec::parse(
            "name: x\nkind: cron\nschedule: \"@daily\"\ntimezone: Mars/Base\ntarget: {agent: a}\n",
        )
        .unwrap();
        assert!(bad_tz.validate().unwrap_err().contains("IANA"));
        let hook =
            TriggerSpec::parse("name: gh\nkind: webhook\ntarget: {agent: triage}\n").unwrap();
        assert_eq!(hook.source.kind(), "webhook");
        assert!(TriggerSpec::parse("name: x\nkind: smoke\ntarget: {agent: a}\n").is_err());
    }

    #[test]
    fn globs_match_file_names() {
        assert!(glob("*.pdf", "Report.PDF"));
        assert!(glob("inv-*-2026.csv", "inv-03-2026.csv"));
        assert!(!glob("*.pdf", "notes.txt"));
        assert!(glob("*", "anything"));
    }
}
