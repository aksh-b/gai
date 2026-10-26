//! Dashboards: panels that are a query and a chart, refreshed on a schedule.
//! A panel can watch a number (a KPI): when it goes below or above a limit,
//! the dashboard's `notify` hears about it once, and again only after it
//! has come back.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use gai_store::DashboardRecord;
use gai_tools::notify::{Delivery, Notice, Notifier};
use gai_tools::{SharedTool, ToolError, ToolOutput, typed_tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Column, Data, DataError, Result, invalid, valid_name};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Chart {
    Line,
    Bar,
    Area,
    Pie,
    /// One big number: the first column of the last row.
    Number,
    #[default]
    Table,
}

/// Watch a panel's number: the last row's `column` (default the first `y`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Alert {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Alert when it is below this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
    /// Alert when it is above this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    /// What to say (default: the panel, the value and the limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    pub title: String,
    pub connection: String,
    /// One SELECT; for charts, one row per point.
    pub sql: String,
    #[serde(default)]
    pub chart: Chart,
    /// The column along the bottom (or the slice names, for pie).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    /// The columns plotted (default: every numeric one but `x`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub y: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alert: Option<Alert>,
}

/// A dashboard.
///
/// ```yaml
/// name: growth
/// title: Growth
/// refresh_minutes: 60
/// notify: { email: [ana@example.com] }
/// panels:
///   - title: Signups per day
///     connection: app
///     sql: SELECT day, COUNT(*) AS signups FROM signups GROUP BY day ORDER BY day
///     chart: line
///     x: day
///     y: [signups]
///     alert: { below: 50 }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DashboardSpec {
    pub name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Refresh this often while `gai serve` runs (none: by hand).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_minutes: Option<u32>,
    /// Who hears about alerts.
    #[serde(default, skip_serializing_if = "Delivery::is_empty")]
    pub notify: Delivery,
    pub panels: Vec<Panel>,
}

/// What one panel's query returned at the last refresh.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PanelResult {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The watched number, for panels with an alert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default)]
    pub alerting: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DashboardResults {
    pub panels: Vec<PanelResult>,
    pub refreshed_at: Option<DateTime<Utc>>,
    /// Alerts sent at this refresh.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<String>,
}

/// A dashboard with its last results.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct DashboardView {
    pub spec: DashboardSpec,
    pub results: Option<DashboardResults>,
}

/// The dashboards of one gai instance.
#[derive(Clone)]
pub struct Dashboards {
    data: Data,
    notifier: Option<Notifier>,
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

impl Dashboards {
    pub fn new(data: Data, notifier: Option<Notifier>) -> Self {
        Self { data, notifier }
    }

    pub fn parse(value: Value) -> Result<DashboardSpec> {
        let spec: DashboardSpec =
            serde_json::from_value(value).map_err(|e| invalid(format!("dashboard: {e}")))?;
        if !valid_name(&spec.name) {
            return Err(invalid("dashboard names use letters, digits, '-' and '_'"));
        }
        if spec.panels.is_empty() || spec.panels.len() > 24 {
            return Err(invalid("a dashboard has 1 to 24 panels"));
        }
        if spec.refresh_minutes == Some(0) {
            return Err(invalid("refresh_minutes must be at least 1"));
        }
        spec.notify.validate().map_err(invalid)?;
        for p in &spec.panels {
            if let Some(a) = &p.alert
                && a.below.is_none()
                && a.above.is_none()
            {
                return Err(invalid(format!(
                    "{}: an alert needs below or above",
                    p.title
                )));
            }
        }
        Ok(spec)
    }

    /// Save a dashboard: every panel's connection must exist and its SQL be
    /// a query.
    pub async fn save(&self, spec: &DashboardSpec) -> Result<()> {
        for p in &spec.panels {
            let c = self.data.spec(&p.connection).await?;
            crate::sql::check_query(&p.sql, c.kind.name())
                .map_err(|e| invalid(format!("{}: {e}", p.title)))?;
        }
        self.data
            .store()
            .put_dashboard(&spec.name, &serde_json::to_value(spec).expect("spec"))
            .await?;
        Ok(())
    }

    fn view(r: DashboardRecord) -> Result<DashboardView> {
        Ok(DashboardView {
            spec: Self::parse(r.spec)?,
            results: r.results.and_then(|v| serde_json::from_value(v).ok()),
        })
    }

    pub async fn get(&self, name: &str) -> Result<DashboardView> {
        let r = self
            .data
            .store()
            .get_dashboard(name)
            .await?
            .ok_or_else(|| DataError::NotFound(format!("no dashboard {name}")))?;
        Self::view(r)
    }

    pub async fn list(&self) -> Result<Vec<DashboardView>> {
        let rows = self.data.store().list_dashboards().await?;
        Ok(rows
            .into_iter()
            .filter_map(|r| Self::view(r).ok())
            .collect())
    }

    pub async fn delete(&self, name: &str) -> Result<bool> {
        Ok(self.data.store().delete_dashboard(name).await?)
    }

    /// Run every panel's query, check alerts, and keep the results.
    pub async fn refresh(&self, name: &str) -> Result<DashboardResults> {
        let view = self.get(name).await?;
        let before = view.results.unwrap_or_default();
        let mut out = DashboardResults {
            refreshed_at: Some(Utc::now()),
            ..Default::default()
        };
        let mut fired = Vec::new();
        for (i, p) in view.spec.panels.iter().enumerate() {
            let mut r = match self.data.query(&p.connection, &p.sql, None).await {
                Ok(q) => PanelResult {
                    columns: q.columns,
                    rows: q.rows,
                    ..Default::default()
                },
                Err(e) => PanelResult {
                    error: Some(e.to_string()),
                    ..Default::default()
                },
            };
            if let Some(a) = &p.alert {
                let col = a
                    .column
                    .clone()
                    .or_else(|| p.y.first().cloned())
                    .or_else(|| r.columns.iter().rev().map(|c| c.name.clone()).next());
                let idx = col.and_then(|c| r.columns.iter().position(|x| x.name == c));
                r.value = idx.and_then(|i| r.rows.last().and_then(|row| number(&row[i])));
                if let Some(v) = r.value {
                    let low = a.below.filter(|b| v < *b);
                    let high = a.above.filter(|b| v > *b);
                    r.alerting = low.is_some() || high.is_some();
                    let was = before.panels.get(i).is_some_and(|b| b.alerting);
                    if r.alerting && !was {
                        let why = match (low, high) {
                            (Some(b), _) => format!("{v} is below {b}"),
                            (_, Some(b)) => format!("{v} is above {b}"),
                            _ => unreachable!(),
                        };
                        fired.push(
                            a.message
                                .clone()
                                .unwrap_or_else(|| format!("{}: {why}", p.title)),
                        );
                    }
                }
            }
            out.panels.push(r);
        }
        if !fired.is_empty()
            && !view.spec.notify.is_empty()
            && let Some(n) = &self.notifier
        {
            let notice = Notice {
                subject: format!("Alert: {}", view.spec.title),
                text: format!("{}\n\nDashboard: {}", fired.join("\n"), view.spec.title),
                attachments: Vec::new(),
                meta: json!({"kind": "alert", "dashboard": view.spec.name, "alerts": fired}),
            };
            match n.send(&view.spec.notify, &notice).await {
                Ok(_) => {}
                Err(e) => tracing::warn!(dashboard = %name, error = %e, "could not send an alert"),
            }
        }
        out.alerts = fired;
        self.data
            .store()
            .save_dashboard_results(name, &serde_json::to_value(&out).expect("results"))
            .await?;
        Ok(out)
    }

    /// Dashboards whose `refresh_minutes` have passed.
    pub async fn due(&self) -> Result<Vec<String>> {
        let now = Utc::now();
        Ok(self
            .data
            .store()
            .list_dashboards()
            .await?
            .into_iter()
            .filter(|r| {
                let every = r.spec["refresh_minutes"].as_u64();
                every.is_some_and(|m| {
                    r.refreshed_at
                        .is_none_or(|t| now - t >= chrono::Duration::minutes(m as i64))
                })
            })
            .map(|r| r.name)
            .collect())
    }

    /// `save_dashboard`: agents build dashboards from queries they checked.
    pub fn tool(&self) -> SharedTool {
        let d = self.clone();
        Arc::new(typed_tool(
            "save_dashboard",
            "Save a dashboard of charts, each a SQL query on a connection, and refresh it now. Check each query with sql_query first. Give refresh_minutes to keep it up to date, and an alert (below/above) on a panel to watch a KPI; notify says who hears about alerts. Saving a dashboard with the same name replaces it.",
            move |_ctx, spec: DashboardSpec| {
                let d = d.clone();
                async move {
                    let spec = Dashboards::parse(serde_json::to_value(&spec).expect("spec"))
                        .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
                    d.save(&spec)
                        .await
                        .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
                    let r = d
                        .refresh(&spec.name)
                        .await
                        .map_err(|e| ToolError::Failed(e.to_string()))?;
                    let lines: Vec<String> = spec
                        .panels
                        .iter()
                        .zip(&r.panels)
                        .map(|(p, r)| match &r.error {
                            Some(e) => format!("- {}: failed: {e}", p.title),
                            None => format!(
                                "- {}: {} rows{}",
                                p.title,
                                r.rows.len(),
                                r.value
                                    .map(|v| format!(
                                        ", value {v}{}",
                                        if r.alerting { " (alerting)" } else { "" }
                                    ))
                                    .unwrap_or_default()
                            ),
                        })
                        .collect();
                    Ok(ToolOutput::text(format!(
                        "Saved the dashboard {:?} (open it at #/dashboards/{}){}:\n{}",
                        spec.title,
                        spec.name,
                        spec.refresh_minutes
                            .map(|m| format!(", refreshed every {m} minutes"))
                            .unwrap_or_default(),
                        lines.join("\n")
                    )))
                }
            },
        ))
    }
}
