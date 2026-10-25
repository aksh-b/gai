//! The agent runtime.
//!
//! An [`AgentSpec`] says which model, prompt, tools and limits an agent
//! has. The [`Runtime`] runs it: it streams the model, executes tool calls
//! (in parallel, with policies and approvals), keeps the conversation inside
//! the context window, enforces budgets, journals every step to the store
//! and publishes [`RunEvent`]s as it goes.

mod approval;
mod context;
mod event;
pub mod hooks;
mod media;
mod multi;
mod persona;
mod replay;
mod runtime;
pub mod safety;
mod spec;

pub use approval::{ApprovalHub, Decision, PendingApproval};
pub use event::{EventEnvelope, RunEvent};
pub use hooks::{Blocked, HookCtx, RunHook, ToolCall};
pub use media::MediaSettings;
pub use multi::{
    DelegateRef, DelegateSpec, ExternalRun, MAX_DEPTH, RemoteDelegate, RunLauncher,
    delegate_tool_name, outcome_of, run_kind,
};
pub use persona::{
    Formality, NO_PERSONA, PersonaDefaults, PersonaExample, PersonaRef, PersonaSpec, Placement,
    ResolvedPersona, Verbosity,
};
pub use replay::{ForkInfo, ReplayOverrides, ReplayRequest, StepEdit};
pub use runtime::{
    AgentError, FinishHook, RunHandle, RunOutcome, RunRequest, Runtime, RuntimeConfig,
};
pub use spec::{AgentSpec, ContextConfig, Limits};
