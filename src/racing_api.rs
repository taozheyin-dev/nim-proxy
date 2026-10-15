//! Admin-only racing policy and performance endpoints. Policy writes use the
//! same serialized validate → persist → publish transaction as other settings.

use std::sync::Arc;

use axum::extract::{FromRequest, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::api::{
    ApiError, ApiJson, ModelStatsEntry, OkResponse, PerformanceStatsResponse, RacingConfigResponse,
};
use crate::auth::Identity;
use crate::config::StoredConfig;
use crate::racing_config::RaceStrategy;
use crate::AppState;

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateRacingConfigRequest {
    pub enabled: bool,
    pub strategy: Option<RaceStrategy>,
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(ApiError::new(code, message))).into_response()
}

fn admin_rejection(store: &StoredConfig, username: &str) -> Option<Response> {
    match store.user(username) {
        Some(user) if user.role.is_admin() => None,
        Some(_) => Some(error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "racing requires an admin",
        )),
        None => Some(error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "your user no longer exists",
        )),
    }
}

/// Get the current racing policy (admin).
#[utoipa::path(
    get,
    path = "/api/racing/config",
    tag = "racing",
    responses(
        (status = 200, description = "Current persisted policy", body = RacingConfigResponse),
        (status = 401, description = "Operator authentication required", body = ApiError),
        (status = 403, description = "Racing requires an admin", body = ApiError),
        (status = 503, description = "Initial setup is required", body = ApiError),
    ),
)]
pub async fn get_racing_config(
    State(state): State<Arc<AppState>>,
    Extension(Identity(username)): Extension<Identity>,
) -> Response {
    if let Some(response) = admin_rejection(&state.store.lock().unwrap(), &username) {
        return response;
    }
    let strategy = state.cfg().racing.clone();
    Json(RacingConfigResponse {
        enabled: strategy.is_some(),
        strategy,
    })
    .into_response()
}

/// Persist and apply a bounded racing policy (admin).
#[utoipa::path(
    post,
    path = "/api/racing/config",
    tag = "racing",
    request_body = UpdateRacingConfigRequest,
    responses(
        (status = 200, description = "Validated, persisted, and applied atomically", body = OkResponse),
        (status = 400, description = "Invalid JSON or invalid_config: invalid racing policy, strict passthrough conflict, or failed save", body = ApiError),
        (status = 401, description = "Operator authentication required", body = ApiError),
        (status = 403, description = "Racing requires an admin", body = ApiError),
        (status = 413, description = "Request body exceeds the configured limit", body = ApiError),
        (status = 415, description = "Content-Type must be application/json", body = ApiError),
        (status = 422, description = "Invalid JSON request body", body = ApiError),
        (status = 503, description = "Initial setup is required", body = ApiError),
    ),
)]
pub async fn update_racing_config(
    State(state): State<Arc<AppState>>,
    Extension(Identity(username)): Extension<Identity>,
    request: Request,
) -> Response {
    // As with server-locale settings, authorization wins before body parsing.
    if let Some(response) = admin_rejection(&state.store.lock().unwrap(), &username) {
        return response;
    }
    let ApiJson(request) =
        match ApiJson::<UpdateRacingConfigRequest>::from_request(request, &state).await {
            Ok(request) => request,
            Err(response) => return response,
        };
    let result = {
        let mut store = state.store.lock().unwrap();
        if let Some(response) = admin_rejection(&store, &username) {
            return response;
        }
        let mut candidate = store.clone();
        candidate.racing = if request.enabled {
            let Some(strategy) = request.strategy else {
                return error(
                    StatusCode::BAD_REQUEST,
                    "invalid_config",
                    "enabled racing requires a strategy",
                );
            };
            Some(strategy)
        } else {
            None
        };
        crate::settings::commit(&state, &mut store, candidate)
    };
    match result {
        Ok(()) => Json(OkResponse::new()).into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, "invalid_config", message),
    }
}

fn model_entry((name, stats): (String, crate::performance::ModelStats)) -> ModelStatsEntry {
    ModelStatsEntry {
        avg_tokens_per_sec: stats.avg_tokens_per_sec,
        avg_ttft_ms: stats.avg_ttft_ms,
        name,
        success_rate: stats.success_rate,
        successful_requests: stats.successful_requests,
        total_requests: stats.total_requests,
    }
}

/// Get model performance observations since process startup (admin).
#[utoipa::path(
    get,
    path = "/api/racing/stats",
    tag = "racing",
    responses(
        (status = 200, description = "Model observations in model-id order", body = PerformanceStatsResponse),
        (status = 401, description = "Operator authentication required", body = ApiError),
        (status = 403, description = "Racing requires an admin", body = ApiError),
        (status = 503, description = "Initial setup is required", body = ApiError),
    ),
)]
pub async fn get_performance_stats(
    State(state): State<Arc<AppState>>,
    Extension(Identity(username)): Extension<Identity>,
) -> Response {
    if let Some(response) = admin_rejection(&state.store.lock().unwrap(), &username) {
        return response;
    }
    let mut models: Vec<_> = state
        .perf_tracker
        .get_all_stats()
        .into_iter()
        .map(model_entry)
        .collect();
    models.sort_by(|a, b| a.name.cmp(&b.name));
    Json(PerformanceStatsResponse { models }).into_response()
}

/// Get models ranked by measured performance since process startup (admin).
#[utoipa::path(
    get,
    path = "/api/racing/ranked",
    tag = "racing",
    responses(
        (status = 200, description = "Ranked models with actual latency observations", body = PerformanceStatsResponse),
        (status = 401, description = "Operator authentication required", body = ApiError),
        (status = 403, description = "Racing requires an admin", body = ApiError),
        (status = 503, description = "Initial setup is required", body = ApiError),
    ),
)]
pub async fn get_ranked_models(
    State(state): State<Arc<AppState>>,
    Extension(Identity(username)): Extension<Identity>,
) -> Response {
    if let Some(response) = admin_rejection(&state.store.lock().unwrap(), &username) {
        return response;
    }
    let models = state
        .perf_tracker
        .get_ranked_models()
        .into_iter()
        .map(model_entry)
        .collect();
    Json(PerformanceStatsResponse { models }).into_response()
}
