use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// An error as the API reports it: a status and `{"error": {"message"}}`.
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn not_found(what: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, what)
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({"error": {"message": self.message}})),
        )
            .into_response()
    }
}

impl From<gai::store::StoreError> for ApiError {
    fn from(e: gai::store::StoreError) -> Self {
        match e {
            gai::store::StoreError::NotFound(m) => Self::not_found(m),
            gai::store::StoreError::Conflict(m) => Self::new(StatusCode::CONFLICT, m),
            other => Self::new(StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
        }
    }
}

impl From<gai::agent::AgentError> for ApiError {
    fn from(e: gai::agent::AgentError) -> Self {
        use gai::agent::AgentError as E;
        match e {
            E::NotFound(m) => Self::not_found(m),
            E::Invalid(m) | E::Template(m) => Self::bad_request(m),
            E::Model(m) => Self::bad_request(m.to_string()),
            E::Store(s) => s.into(),
            E::Memory(m) => m.into(),
            E::Skill(s) => s.into(),
        }
    }
}

impl From<gai::memory::MemoryError> for ApiError {
    fn from(e: gai::memory::MemoryError) -> Self {
        use gai::memory::MemoryError as E;
        match e {
            E::Store(s) => s.into(),
            E::NotFound(m) => Self::not_found(m),
            E::Invalid(m) => Self::bad_request(m),
            E::Llm(m) => Self::new(StatusCode::BAD_GATEWAY, m.to_string()),
        }
    }
}

impl From<gai::skills::SkillError> for ApiError {
    fn from(e: gai::skills::SkillError) -> Self {
        use gai::skills::SkillError as E;
        match e {
            E::Store(s) => s.into(),
            E::Invalid(m) => Self::bad_request(m),
            E::Io(m) => Self::new(StatusCode::INTERNAL_SERVER_ERROR, m),
        }
    }
}

impl From<gai::Error> for ApiError {
    fn from(e: gai::Error) -> Self {
        match e {
            gai::Error::NotFound(m) => Self::not_found(m),
            gai::Error::Agent(a) => a.into(),
            gai::Error::Store(s) => s.into(),
            gai::Error::Memory(m) => m.into(),
            gai::Error::Skill(s) => s.into(),
            gai::Error::Workflow(w) => w.into(),
            gai::Error::Eval(e) => e.into(),
            other => Self::bad_request(other.to_string()),
        }
    }
}

impl From<gai::eval::EvalError> for ApiError {
    fn from(e: gai::eval::EvalError) -> Self {
        use gai::eval::EvalError as E;
        match e {
            E::NotFound(m) => Self::not_found(m),
            E::Invalid(m) => Self::bad_request(m),
            E::Store(s) => s.into(),
            E::Agent(a) => a.into(),
            E::Llm(l) => l.into(),
        }
    }
}

impl From<gai::llm::LlmError> for ApiError {
    fn from(e: gai::llm::LlmError) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, e.to_string())
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

impl From<gai::triggers::TriggerError> for ApiError {
    fn from(e: gai::triggers::TriggerError) -> Self {
        use gai::triggers::TriggerError as E;
        match e {
            E::Invalid(m) => Self::bad_request(m),
            E::NotFound(m) => Self::not_found(m),
            E::Store(e) => e.into(),
            E::Agent(e) => e.into(),
            E::Workflow(e) => e.into(),
        }
    }
}

impl From<gai::orchestra::WorkflowError> for ApiError {
    fn from(e: gai::orchestra::WorkflowError) -> Self {
        use gai::orchestra::WorkflowError as E;
        match e {
            E::Invalid(m) => Self::bad_request(m),
            E::NotFound(m) => Self::not_found(m),
            E::Agent(a) => a.into(),
            E::Store(s) => s.into(),
        }
    }
}
