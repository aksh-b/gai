//! Suggestions: things worth doing, shown in the dashboard's inbox.
//!
//! They come from two places. gai looks at how it is used and suggests
//! assistants, tools and templates (an assistant that keeps failing, a
//! database without a dashboard, GitHub connected but nothing using it).
//! And agents suggest actions with the `suggest` tool: an agent run on a
//! schedule or by a webhook can watch data and propose what to do, which
//! a person accepts or dismisses.
//!
//! Suggestions are settings (`suggest.<id>`) of each organization.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use gai_agent::{AgentSpec, RunRequest, Runtime};
use gai_store::{SharedStore, StatsFilter};
use gai_tools::{SharedTool, ToolCtx, ToolError, ToolOutput, typed_tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Error;
use crate::notifications::{Event, Notifications, NotifyEvent};

const PREFIX: &str = "suggest.";
/// Open suggestions kept at most; the oldest agent ones go first.
const MAX_OPEN: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    #[default]
    Open,
    Accepted,
    Dismissed,
}

/// What accepting a suggestion does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SuggestedAction {
    /// Start a run of an assistant with this message.
    RunAgent { agent: String, input: String },
    /// Install a template (an assistant, workflow, skill or connection).
    InstallTemplate { template: String },
    /// Open a page of the dashboard (`#/assistants/triage`).
    Open { link: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Suggestion {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub details: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<SuggestedAction>,
    /// `gai` for suggestions from usage, else the agent that made it.
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default)]
    pub status: SuggestionStatus,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
}

/// What the `suggest` tool takes.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SuggestInput {
    /// A short title: what to do, as an instruction ("Restock SKU 1182").
    pub title: String,
    /// Why, with the numbers or facts behind it. Markdown.
    #[serde(default)]
    pub details: String,
    /// What accepting it does, if anything: run an assistant with a
    /// message, install a template, or open a page.
    #[serde(default)]
    pub action: Option<SuggestedAction>,
}

#[derive(Clone)]
pub struct Suggestions {
    store: SharedStore,
    notifications: Notifications,
}

impl Suggestions {
    pub fn new(store: SharedStore, notifications: Notifications) -> Self {
        Self {
            store,
            notifications,
        }
    }

    async fn all(&self) -> Result<Vec<Suggestion>, Error> {
        let mut out: Vec<Suggestion> = self
            .store
            .list_settings(PREFIX)
            .await?
            .into_iter()
            .filter_map(|(_, v)| serde_json::from_str(&v).ok())
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        Ok(out)
    }

    /// Open suggestions, newest first (after looking at usage again).
    pub async fn open(&self, rt: &Runtime) -> Result<Vec<Suggestion>, Error> {
        self.add_from_usage(rt).await?;
        Ok(self
            .all()
            .await?
            .into_iter()
            .filter(|s| s.status == SuggestionStatus::Open)
            .collect())
    }

    pub async fn get(&self, id: &str) -> Result<Suggestion, Error> {
        let v = self
            .store
            .get_setting(&format!("{PREFIX}{id}"))
            .await?
            .ok_or_else(|| Error::NotFound(format!("no suggestion {id}")))?;
        serde_json::from_str(&v).map_err(|e| Error::Invalid(e.to_string()))
    }

    async fn put(&self, s: &Suggestion) -> Result<(), Error> {
        self.store
            .put_setting(
                &format!("{PREFIX}{}", s.id),
                &serde_json::to_string(s).expect("suggestion"),
            )
            .await?;
        Ok(())
    }

    /// Add a suggestion and tell the people whose rules want to hear.
    pub async fn add(&self, mut s: Suggestion) -> Result<Suggestion, Error> {
        if s.title.trim().is_empty() {
            return Err(Error::Invalid("a suggestion needs a title".into()));
        }
        s.title = s.title.trim().chars().take(200).collect();
        s.details = s.details.chars().take(4000).collect();
        self.put(&s).await?;
        self.trim().await?;
        let e = Event {
            kind: NotifyEvent::Suggestion,
            source: Some(s.source.clone()),
            subject: format!("Suggestion: {}", s.title),
            text: s.details.clone(),
            link: Some("#/suggestions".into()),
            meta: json!({"suggestion_id": s.id, "run_id": s.run_id}),
        };
        let n = self.notifications.clone();
        tokio::spawn(async move { n.dispatch(&e).await });
        Ok(s)
    }

    /// Mark a suggestion accepted or dismissed. Dismissed suggestions from
    /// usage are not made again.
    pub async fn decide(&self, id: &str, status: SuggestionStatus) -> Result<Suggestion, Error> {
        let mut s = self.get(id).await?;
        s.status = status;
        s.decided_at = Some(Utc::now());
        self.put(&s).await?;
        Ok(s)
    }

    /// Accept a `run_agent` suggestion by starting the run in the
    /// background; returns its run id. Other actions are the caller's.
    pub async fn start(
        &self,
        rt: &Runtime,
        action: &SuggestedAction,
    ) -> Result<Option<String>, Error> {
        let SuggestedAction::RunAgent { agent, input } = action else {
            return Ok(None);
        };
        let (spec, version) = rt.resolve_agent(agent).await?;
        let mut req = RunRequest::new(spec, input.clone());
        req.agent_version = version;
        req.metadata.insert("source".into(), json!("suggestion"));
        let (run_id, _) = rt.enqueue(req).await?;
        Ok(Some(run_id))
    }

    /// Keep at most [`MAX_OPEN`] open ones, and decided ones for 30 days.
    async fn trim(&self) -> Result<(), Error> {
        let all = self.all().await?;
        let old = Utc::now() - Duration::days(30);
        let mut open = 0;
        for s in all {
            let drop = match s.status {
                SuggestionStatus::Open => {
                    open += 1;
                    open > MAX_OPEN
                }
                _ => s.decided_at.is_some_and(|d| d < old) && s.source != "gai",
            };
            if drop {
                self.store
                    .delete_setting(&format!("{PREFIX}{}", s.id))
                    .await?;
            }
        }
        Ok(())
    }

    /// Look at how gai is used and add what is worth suggesting. Each has a
    /// fixed id, so it is made once, and never again once dismissed.
    async fn add_from_usage(&self, rt: &Runtime) -> Result<(), Error> {
        let have: BTreeSet<String> = self.all().await?.into_iter().map(|s| s.id).collect();
        for s in usage(&self.store, rt).await? {
            if !have.contains(&s.id) {
                self.put(&s).await?;
            }
        }
        Ok(())
    }

    /// The `suggest` tool.
    pub fn tool(&self) -> SharedTool {
        let me = self.clone();
        Arc::new(typed_tool(
            "suggest",
            "Suggest an action to the people using gai: it shows in their Suggestions inbox (and reaches them through their notification rules), where they accept or dismiss it. Use it when you find something worth doing that you should not do yourself, for example from data you watch on a schedule. Give a short title, the facts behind it, and optionally what accepting does: run an assistant with a message, install a template, or open a page. Don't suggest the same thing twice.",
            move |ctx: ToolCtx, input: SuggestInput| {
                let me = me.clone();
                async move {
                    if let Some(SuggestedAction::Open { link }) = &input.action
                        && !link.starts_with('#')
                    {
                        return Err(ToolError::InvalidInput(
                            "open links are dashboard pages starting with #/".into(),
                        ));
                    }
                    let open = me
                        .all()
                        .await
                        .map_err(|e| ToolError::Failed(e.to_string()))?;
                    if open.iter().any(|s| {
                        s.status == SuggestionStatus::Open
                            && s.source == ctx.agent
                            && s.title.eq_ignore_ascii_case(input.title.trim())
                    }) {
                        return Ok(ToolOutput::text(
                            "That is already suggested and still open.",
                        ));
                    }
                    let s = me
                        .add(Suggestion {
                            id: gai_core::new_id("sug"),
                            title: input.title,
                            details: input.details,
                            action: input.action,
                            source: ctx.agent.clone(),
                            run_id: (!ctx.run_id.is_empty()).then(|| ctx.run_id.clone()),
                            status: SuggestionStatus::Open,
                            created_at: Utc::now(),
                            decided_at: None,
                        })
                        .await
                        .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
                    Ok(ToolOutput::text(format!(
                        "Suggested {:?}; people will see it in their inbox.",
                        s.title
                    )))
                }
            },
        ))
    }
}

fn system(id: &str, title: String, details: String, action: SuggestedAction) -> Suggestion {
    Suggestion {
        id: format!("gai-{id}"),
        title,
        details,
        action: Some(action),
        source: "gai".into(),
        run_id: None,
        status: SuggestionStatus::Open,
        created_at: Utc::now(),
        decided_at: None,
    }
}

/// Suggestions from how gai is used.
async fn usage(store: &SharedStore, rt: &Runtime) -> Result<Vec<Suggestion>, Error> {
    let mut out = Vec::new();
    let mut specs: BTreeMap<String, AgentSpec> = BTreeMap::new();
    for a in store.list_agents().await? {
        if let Ok(Some(v)) = store.get_agent(&a.name, None).await
            && let Ok(spec) = serde_json::from_value::<AgentSpec>(v.spec)
        {
            specs.insert(a.name, spec);
        }
    }
    let uses = |pred: &dyn Fn(&str) -> bool| {
        specs
            .values()
            .any(|s| s.tools.iter().any(|t| pred(t.trim_end_matches('?'))))
    };

    // Assistants that often fail or get thumbs down.
    let stats = store
        .agent_stats(StatsFilter {
            since: Some(Utc::now() - Duration::days(30)),
            ..Default::default()
        })
        .await?;
    let mut per_agent: BTreeMap<&str, (u64, u64, u64, u64)> = BTreeMap::new();
    for s in &stats {
        let e = per_agent.entry(&s.agent).or_default();
        e.0 += s.completed;
        e.1 += s.failed;
        e.2 += s.good;
        e.3 += s.bad;
    }
    for (agent, (ok, failed, good, bad)) in per_agent {
        if !specs.contains_key(agent) {
            continue;
        }
        let finished = ok + failed;
        if finished >= 5 && failed * 10 > finished * 3 {
            out.push(system(
                &format!("failing-{agent}"),
                format!("Improve {agent}: {failed} of its last {finished} runs failed"),
                "Open its runs to see why. Making a test set from them lets **Improve** rewrite its instructions until more pass.".into(),
                SuggestedAction::Open { link: format!("#/assistants/{agent}") },
            ));
        } else if good + bad >= 5 && bad * 10 > (good + bad) * 3 {
            out.push(system(
                &format!("disliked-{agent}"),
                format!("Improve {agent}: {bad} of {} ratings were thumbs down", good + bad),
                "Turn the rated replies into a test set and use **Improve** to rewrite its instructions.".into(),
                SuggestedAction::Open { link: format!("#/assistants/{agent}") },
            ));
        }
    }

    // Data, documents and connections nothing uses yet.
    let connections = store.list_connections().await?;
    if !connections.is_empty()
        && store.list_dashboards().await?.is_empty()
        && !uses(&|t| t == "sql_query")
    {
        out.push(system(
            "sql-analyst",
            "Ask questions of your databases".into(),
            format!(
                "You connected {} but no assistant uses it. The Database analyst answers questions in plain words with SQL and charts, and builds dashboards with alerts.",
                connections.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
            SuggestedAction::InstallTemplate { template: "sql-analyst".into() },
        ));
    }
    let collections = store.list_collections().await?;
    if !collections.is_empty() && !uses(&|t| t == "rag_search") {
        out.push(system(
            "docs-helper",
            "Answer questions from your documents".into(),
            "You have documents in the knowledge base but no assistant searches them. The documents helper answers from them, citing its sources.".into(),
            SuggestedAction::InstallTemplate { template: "docs-helper".into() },
        ));
    }
    let servers: BTreeSet<String> = store
        .list_mcp_servers()
        .await?
        .into_iter()
        .map(|(n, _)| n)
        .chain(rt.mcp().list().into_iter().map(|i| i.name))
        .collect();
    for (server, template, what) in [
        (
            "github",
            "code-assistant",
            "reviews pull requests and writes code",
        ),
        (
            "gmail",
            "email-drafter",
            "drafts emails and replies in Gmail",
        ),
        (
            "outlook",
            "email-drafter",
            "drafts emails and replies in Outlook",
        ),
    ] {
        let prefix = format!("mcp__{server}__");
        if servers.contains(server) && !uses(&|t: &str| t.starts_with(&prefix)) {
            out.push(system(
                &format!("{template}-{server}"),
                format!("Put {server} to work"),
                format!("{server} is connected but no assistant uses it. The {template} template {what}."),
                SuggestedAction::InstallTemplate { template: template.into() },
            ));
        }
    }

    // Questions asked again and again: a schedule could ask for them.
    let mut titles: BTreeMap<(String, String), u32> = BTreeMap::new();
    let month = Utc::now() - Duration::days(30);
    for s in store.list_sessions(None, 500).await? {
        if s.created_at < month || s.metadata["source"] == "trigger" {
            continue;
        }
        if let (Some(agent), Some(title)) = (s.agent, s.title) {
            let key = title.trim().to_lowercase();
            if key.len() >= 12 {
                *titles.entry((agent, key)).or_default() += 1;
            }
        }
    }
    for ((agent, title), n) in titles {
        if n >= 4 {
            let id: String = title
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(24)
                .collect();
            out.push(system(
                &format!("schedule-{agent}-{id}"),
                format!("Schedule \"{title}\""),
                format!("You asked {agent} this {n} times in the last month. A schedule can ask for you and send the answer by email."),
                SuggestedAction::Open { link: "#/schedules".into() },
            ));
        }
    }
    Ok(out)
}
