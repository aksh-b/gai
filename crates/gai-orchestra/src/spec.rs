//! Workflow definitions and their validation.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use gai_agent::AgentSpec;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A workflow: a graph of nodes that run when the nodes they depend on are
/// done, independent ones in parallel.
///
/// ```yaml
/// name: brief
/// inputs:
///   topic: { description: What to research }
/// nodes:
///   - id: research
///     agent: researcher
///     input: "Find the key facts about {{ inputs.topic }}."
///   - id: outline
///     agent: writer
///     input: "Outline a brief from these notes:\n{{ nodes.research.output }}"
///   - id: review
///     human: "Approve this outline?\n{{ nodes.outline.output }}"
///   - id: sections
///     map: "nodes.outline.output | lines"
///     agent: writer
///     input: "Write the section: {{ item }}"
///   - id: brief
///     template: "{{ nodes.sections.output | join('\n\n') }}"
/// output: "{{ nodes.brief.output }}"
/// ```
///
/// Templates are minijinja. They see `inputs`, `nodes.<id>.output` and
/// `nodes.<id>.status`, and inside a `map` node `item` and `index`. A node
/// depends on every node it names in a template, plus those in `needs`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputSpec>,
    pub nodes: Vec<NodeSpec>,
    /// The result: a template, or JSON with templates in its strings.
    /// Defaults to the output of the last node that ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    pub limits: WorkflowLimits,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct InputSpec {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Used when the input is not given. An input without a default is
    /// required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct WorkflowLimits {
    /// Nodes running at once.
    pub max_concurrency: usize,
    /// Wall-clock limit for the whole workflow, including waits for people.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

impl Default for WorkflowLimits {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            timeout_secs: None,
        }
    }
}

/// An agent for a node: a reference (`name`, `name@v2`, `name@prod`, a
/// file) or an inline definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AgentRef {
    Name(String),
    Spec(Box<AgentSpec>),
}

/// One step. Give exactly one of `agent`, `tool`, `template`, `value` or
/// `human`; `map` repeats it for every item of a list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct NodeSpec {
    /// Letters, digits and `_`, starting with a letter.
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Nodes to wait for besides those named in templates.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// Run only when this expression is true. Without `if`, a node is
    /// skipped when all of the nodes it depends on were skipped.
    #[serde(rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,

    /// Run an agent; its final answer is the output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentRef>,
    /// The agent's task (a template).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// Call a tool from the registry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The tool's arguments; strings are templates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    /// Render text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Build JSON; strings are templates, and a string that is a single
    /// `{{ expression }}` keeps the expression's type (list, object…).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Ask a person: the prompt (a template). Waits in the approvals inbox;
    /// the output is `{approved, response, reason}`, and declining fails
    /// the node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub human: Option<String>,

    /// An expression giving a list; the node runs once per item, with
    /// `item` and `index` in its templates, and outputs the list of results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    /// Items processed at once by a `map` node (default 4).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    /// Extra attempts after a failure.
    #[serde(skip_serializing_if = "is_zero")]
    pub retries: u32,
    /// Keep going when this node fails; its status is `failed` and its
    /// output null.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub continue_on_error: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// What a node does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Agent,
    Tool,
    Template,
    Value,
    Human,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Tool => "tool",
            Self::Template => "template",
            Self::Value => "value",
            Self::Human => "human",
        }
    }
}

impl NodeSpec {
    pub fn kind(&self) -> Result<NodeKind, String> {
        let kinds: Vec<NodeKind> = [
            (self.agent.is_some(), NodeKind::Agent),
            (self.tool.is_some(), NodeKind::Tool),
            (self.template.is_some(), NodeKind::Template),
            (self.value.is_some(), NodeKind::Value),
            (self.human.is_some(), NodeKind::Human),
        ]
        .into_iter()
        .filter_map(|(set, k)| set.then_some(k))
        .collect();
        match kinds.as_slice() {
            [k] => Ok(*k),
            [] => Err(format!(
                "node {}: give one of agent, tool, template, value or human",
                self.id
            )),
            _ => Err(format!(
                "node {}: give only one of agent, tool, template, value or human",
                self.id
            )),
        }
    }

    /// Every template string in the node, for finding references.
    fn templates(&self) -> Vec<&str> {
        let mut t: Vec<&str> = [
            &self.condition,
            &self.input,
            &self.template,
            &self.human,
            &self.map,
        ]
        .into_iter()
        .filter_map(|s| s.as_deref())
        .collect();
        fn walk<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
            match v {
                Value::String(s) => out.push(s),
                Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
                Value::Object(o) => o.values().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        for v in [&self.args, &self.value].into_iter().flatten() {
            walk(v, &mut t);
        }
        t
    }
}

/// A node as the graph sees it.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct GraphNode {
    pub id: String,
    pub kind: NodeKind,
    pub map: bool,
    /// The nodes it waits for.
    pub deps: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_alphabetic())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl WorkflowSpec {
    /// Parse a workflow file: `yaml`/`yml`, `toml` or `json`.
    pub fn parse(text: &str, format: &str) -> Result<Self, String> {
        let spec: Self = match format.to_ascii_lowercase().as_str() {
            "yaml" | "yml" => serde_yaml::from_str(text).map_err(|e| e.to_string())?,
            "toml" => toml::from_str(text).map_err(|e| e.to_string())?,
            "json" => serde_json::from_str(text).map_err(|e| e.to_string())?,
            other => return Err(format!("unknown workflow format {other:?}")),
        };
        spec.graph()?;
        Ok(spec)
    }

    /// Read a workflow file; the name defaults to the file stem.
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("yaml");
        let mut spec = Self::parse(&text, ext).map_err(|e| format!("{}: {e}", path.display()))?;
        if spec.name.is_empty()
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            spec.name = stem.to_owned();
        }
        Ok(spec)
    }

    /// Check the workflow and work out each node's dependencies, in node
    /// order. Fails on unknown references and cycles.
    pub fn graph(&self) -> Result<Vec<GraphNode>, String> {
        if !self.name.is_empty()
            && !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!(
                "workflow name {:?} may only use letters, digits, '-', '_' and '.'",
                self.name
            ));
        }
        if self.nodes.is_empty() {
            return Err("a workflow needs at least one node".into());
        }
        if self.limits.max_concurrency == 0 {
            return Err("limits.max_concurrency must be at least 1".into());
        }
        let ids: BTreeSet<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        if ids.len() != self.nodes.len() {
            return Err("node ids must be unique".into());
        }
        let refs = regex::Regex::new(
            r#"nodes\s*(?:\.\s*([A-Za-z_][A-Za-z0-9_]*)|\[\s*["']([^"']+)["']\s*\])"#,
        )
        .expect("valid regex");
        let input_refs =
            regex::Regex::new(r#"inputs\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)"#).expect("valid regex");
        let mut out = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            if !valid_id(&n.id) {
                return Err(format!(
                    "node id {:?}: use letters, digits and '_', starting with a letter",
                    n.id
                ));
            }
            let kind = n.kind()?;
            if n.input.is_some() && kind != NodeKind::Agent {
                return Err(format!("node {}: `input` goes with `agent`", n.id));
            }
            if kind == NodeKind::Agent && n.input.is_none() {
                return Err(format!("node {}: an agent node needs an `input`", n.id));
            }
            if n.args.is_some() && kind != NodeKind::Tool {
                return Err(format!("node {}: `args` goes with `tool`", n.id));
            }
            if n.map.is_some() && kind == NodeKind::Human {
                return Err(format!("node {}: a human node cannot `map`", n.id));
            }
            if n.concurrency == Some(0) {
                return Err(format!("node {}: concurrency must be at least 1", n.id));
            }
            if let Some(AgentRef::Spec(s)) = &n.agent {
                s.validate().map_err(|e| format!("node {}: {e}", n.id))?;
            }
            let mut deps: BTreeSet<String> = BTreeSet::new();
            for d in &n.needs {
                if !ids.contains(d.as_str()) {
                    return Err(format!("node {} needs unknown node {d}", n.id));
                }
                deps.insert(d.clone());
            }
            for t in n.templates() {
                for c in refs.captures_iter(t) {
                    let r = c.get(1).or_else(|| c.get(2)).expect("a group").as_str();
                    if !ids.contains(r) {
                        return Err(format!("node {} refers to unknown node {r}", n.id));
                    }
                    deps.insert(r.to_owned());
                }
                for c in input_refs.captures_iter(t) {
                    let r = &c[1];
                    if !self.inputs.contains_key(r) {
                        return Err(format!("node {} refers to undeclared input {r}", n.id));
                    }
                }
            }
            if deps.contains(&n.id) {
                return Err(format!("node {} depends on itself", n.id));
            }
            out.push(GraphNode {
                id: n.id.clone(),
                kind,
                map: n.map.is_some(),
                deps: deps.into_iter().collect(),
            });
        }
        // Kahn's algorithm: anything left over sits on a cycle.
        let mut indegree: HashMap<&str, usize> =
            out.iter().map(|g| (g.id.as_str(), g.deps.len())).collect();
        let mut ready: Vec<&str> = out
            .iter()
            .filter(|g| g.deps.is_empty())
            .map(|g| g.id.as_str())
            .collect();
        let mut seen = 0;
        while let Some(id) = ready.pop() {
            seen += 1;
            for g in out.iter().filter(|g| g.deps.iter().any(|d| d == id)) {
                let d = indegree.get_mut(g.id.as_str()).expect("known node");
                *d -= 1;
                if *d == 0 {
                    ready.push(&g.id);
                }
            }
        }
        if seen != out.len() {
            let stuck: Vec<&str> = indegree
                .iter()
                .filter(|(_, d)| **d > 0)
                .map(|(id, _)| *id)
                .collect();
            return Err(format!(
                "the workflow has a dependency cycle through {}",
                stuck.join(", ")
            ));
        }
        Ok(out)
    }

    /// The inputs a run gets: the given ones plus defaults. Errors on
    /// missing required inputs and on unknown ones.
    pub fn resolve_inputs(
        &self,
        given: &serde_json::Map<String, Value>,
    ) -> Result<serde_json::Map<String, Value>, String> {
        let mut out = serde_json::Map::new();
        for k in given.keys() {
            if !self.inputs.contains_key(k) {
                return Err(format!(
                    "unknown input {k:?}; this workflow takes: {}",
                    self.inputs.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
        }
        for (k, spec) in &self.inputs {
            match given.get(k).or(spec.default.as_ref()) {
                Some(v) => {
                    out.insert(k.clone(), v.clone());
                }
                None => return Err(format!("missing input {k:?}")),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRIEF: &str = r#"
name: brief
inputs:
  topic: {}
  style: { default: plain }
nodes:
  - id: research
    agent: researcher
    input: "Facts about {{ inputs.topic }}"
  - id: outline
    agent: { name: writer, model: mock/echo }
    input: "Outline from {{ nodes.research.output }}"
  - id: sections
    map: "nodes['outline'].output | lines"
    agent: writer
    input: "Write {{ item }}"
  - id: notify
    needs: [research]
    tool: current_time
  - id: brief
    if: "nodes.sections.status == 'completed'"
    template: "{{ nodes.sections.output | join('\n') }} ({{ inputs.style }})"
"#;

    #[test]
    fn parses_and_finds_dependencies() {
        let w = WorkflowSpec::parse(BRIEF, "yaml").unwrap();
        let g = w.graph().unwrap();
        let deps: Vec<(&str, Vec<&str>)> = g
            .iter()
            .map(|n| (n.id.as_str(), n.deps.iter().map(String::as_str).collect()))
            .collect();
        assert_eq!(
            deps,
            [
                ("research", vec![]),
                ("outline", vec!["research"]),
                ("sections", vec!["outline"]),
                ("notify", vec!["research"]),
                ("brief", vec!["sections"]),
            ]
        );
        assert!(g[2].map);
        assert_eq!(g[1].kind, NodeKind::Agent);
        let inputs = w
            .resolve_inputs(
                &serde_json::json!({"topic": "bees"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
        assert_eq!(inputs["style"], "plain");
        assert!(
            w.resolve_inputs(&Default::default())
                .unwrap_err()
                .contains("missing input")
        );
        assert!(
            w.resolve_inputs(
                &serde_json::json!({"topic": 1, "x": 2})
                    .as_object()
                    .unwrap()
                    .clone()
            )
            .unwrap_err()
            .contains("unknown input")
        );
    }

    #[test]
    fn rejects_bad_graphs() {
        let bad = |y: &str| WorkflowSpec::parse(y, "yaml").unwrap_err();
        assert!(bad("name: w\nnodes: []\n").contains("at least one node"));
        assert!(bad("nodes:\n  - id: a\n    template: x\n    tool: y\n").contains("only one"));
        assert!(bad("nodes:\n  - id: a\n").contains("give one of"));
        assert!(bad("nodes:\n  - id: a-b\n    template: x\n").contains("node id"));
        assert!(
            bad("nodes:\n  - id: a\n    template: '{{ nodes.b.output }}'\n")
                .contains("unknown node b")
        );
        assert!(
            bad("nodes:\n  - id: a\n    template: '{{ inputs.q }}'\n").contains("undeclared input")
        );
        assert!(bad("nodes:\n  - id: a\n    agent: x\n").contains("needs an `input`"));
        let cycle = bad(
            "nodes:\n  - id: a\n    template: '{{ nodes.b.output }}'\n  - id: b\n    template: '{{ nodes.a.output }}'\n",
        );
        assert!(cycle.contains("cycle"), "{cycle}");
        assert!(
            bad("nodes:\n  - id: a\n    template: x\n    retry: 2\n").contains("unknown field")
        );
    }
}
