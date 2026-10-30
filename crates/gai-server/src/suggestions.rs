//! The suggestions inbox: what gai and proactive agents suggest doing.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use gai::store::Scope;
use gai::suggestions::{SuggestedAction, Suggestion, SuggestionStatus};
use schemars::JsonSchema;
use serde::Serialize;

use crate::AppState;
use crate::auth::Principal;
use crate::error::{ApiError, ApiResult};
use crate::orgs::Tenant;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/suggestions", get(list))
        .route("/v1/suggestions/{id}/accept", post(accept))
        .route("/v1/suggestions/{id}/dismiss", post(dismiss))
}

async fn list(Tenant(s): Tenant) -> ApiResult<Json<Vec<Suggestion>>> {
    Ok(Json(s.gai.suggestions().await?.open(&s.rt).await?))
}

#[derive(Serialize, JsonSchema)]
pub struct Accepted {
    pub suggestion: Suggestion,
    /// The run it started (`run_agent`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// What it installed (`install_template`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<String>,
    /// Where to go next.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// Do what the suggestion says and mark it accepted.
async fn accept(
    Tenant(s): Tenant,
    Extension(p): Extension<Principal>,
    Path(id): Path<String>,
) -> ApiResult<Json<Accepted>> {
    let sug = s.gai.suggestions().await?;
    let found = sug.get(&id).await?;
    if found.status != SuggestionStatus::Open {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!("the suggestion was already {:?}", found.status).to_lowercase(),
        ));
    }
    let mut out = Accepted {
        suggestion: found.clone(),
        run_id: None,
        installed: None,
        link: None,
    };
    match &found.action {
        Some(a @ SuggestedAction::RunAgent { .. }) => {
            out.run_id = sug.start(&s.rt, a).await?;
            out.link = out.run_id.as_ref().map(|r| format!("#/runs/{r}"));
        }
        Some(SuggestedAction::InstallTemplate { template }) => {
            if !Scope::allows(&p.scopes, Scope::Build) {
                return Err(ApiError::new(
                    StatusCode::FORBIDDEN,
                    "installing a template needs the build scope",
                ));
            }
            let i = crate::setup::install(&s, template).await?;
            out.link = Some(match i.kind.as_str() {
                "agent" => format!("#/assistants/{}", i.name),
                "workflow" => format!("#/workflows/{}", i.name),
                "mcp" => "#/tools".into(),
                _ => "#/assistants".into(),
            });
            out.installed = Some(i.name);
        }
        Some(SuggestedAction::Open { link }) => out.link = Some(link.clone()),
        None => {}
    }
    out.suggestion = sug.decide(&id, SuggestionStatus::Accepted).await?;
    Ok(Json(out))
}

async fn dismiss(Tenant(s): Tenant, Path(id): Path<String>) -> ApiResult<Json<Suggestion>> {
    Ok(Json(
        s.gai
            .suggestions()
            .await?
            .decide(&id, SuggestionStatus::Dismissed)
            .await?,
    ))
}
