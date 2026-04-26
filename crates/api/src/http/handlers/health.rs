//! Health probe HTTP handlers — `GET /health` and `GET /health/ready`.
//!
//! Architecture references:
//! - FR29 (PRD): liveness and readiness probes
//! - Architecture §Gap Analysis Results — Important gaps item #1

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tracing::instrument;
use utoipa::ToSchema;

use crate::IronDefer;

#[derive(Serialize, ToSchema)]
struct ReadinessResponse {
    status: &'static str,
    db: &'static str,
}

/// `GET /health` — liveness probe. Always returns 200 while the process is alive.
#[utoipa::path(
    get, path = "/health",
    responses(
        (status = 200, description = "Process is alive"),
    )
)]
#[instrument(skip_all, fields(method = "GET", path = "/health"))]
pub async fn liveness() -> Json<serde_json::Value> {
    Json(serde_json::json!({}))
}

/// `GET /health/ready` — readiness probe. Checks Postgres connectivity via `SELECT 1`.
#[utoipa::path(
    get, path = "/health/ready",
    responses(
        (status = 200, description = "Engine is ready", body = ReadinessResponse),
        (status = 503, description = "Database unavailable", body = ReadinessResponse),
    )
)]
#[instrument(skip_all, fields(method = "GET", path = "/health/ready"))]
pub async fn readiness(State(engine): State<Arc<IronDefer>>) -> Response {
    let timeout = engine.readiness_timeout();
    let timeout = if timeout.is_zero() {
        std::time::Duration::from_millis(100)
    } else {
        timeout
    };
    let query_fut = sqlx::query("SELECT 1").execute(engine.pool());

    match tokio::time::timeout(timeout, query_fut).await {
        Ok(Ok(_)) => (
            StatusCode::OK,
            Json(ReadinessResponse {
                status: "ready",
                db: "ok",
            }),
        )
            .into_response(),
        Ok(Err(_)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadinessResponse {
                status: "degraded",
                db: "unavailable",
            }),
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ReadinessResponse {
                status: "degraded",
                db: "timeout",
            }),
        )
            .into_response(),
    }
}
