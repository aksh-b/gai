//! Multi-agent orchestration for gai.
//!
//! Two ways for agents to work together:
//!
//! - **Delegation** (in `gai-agent`): an agent lists `delegates`, and each
//!   becomes an `ask_<name>` tool that runs the other agent as a child run.
//!   A supervisor is simply an agent whose delegates are workers.
//! - **Workflows** (this crate): a YAML graph of nodes ([`WorkflowSpec`])
//!   that run agents, call tools, transform data and wait for people, with
//!   independent nodes in parallel and every finished node checkpointed, so
//!   a run resumes after a crash without redoing work ([`Workflows`]).

mod engine;
pub mod spec;
pub mod tmpl;

pub use engine::{KIND, WorkflowError, WorkflowHandle, WorkflowRequest, Workflows};
pub use spec::{AgentRef, GraphNode, InputSpec, NodeKind, NodeSpec, WorkflowLimits, WorkflowSpec};
