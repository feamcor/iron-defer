//! Axum router factory.
//!
//! Architecture references:
//! - §D4.2: 1 MiB default body limit
//! - §Naming Patterns (REST API): no `/v1/` prefix in MVP

#![allow(clippy::needless_for_each)]

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use utoipa::OpenApi;

use super::handlers::{health, metrics, queues, tasks};
use crate::IronDefer;
use crate::http::errors::{ErrorDetail, ErrorResponse};

/// Maximum request body size — 1 MiB per Architecture D4.2.
const MAX_BODY_SIZE: usize = 1_048_576;

/// Build the axum `Router` for the iron-defer REST API.
pub fn build(engine: Arc<IronDefer>) -> Router {
    Router::new()
        .route(
            "/tasks",
            axum::routing::post(tasks::create_task).get(tasks::list_tasks),
        )
        .route(
            "/tasks/{id}",
            get(tasks::get_task).delete(tasks::delete_task),
        )
        .route("/tasks/{id}/audit", get(tasks::get_audit_log))
        .route("/tasks/{id}/signal", axum::routing::post(tasks::signal_task))
        .route("/queues", get(queues::list_queues))
        .route("/health", get(health::liveness))
        .route("/health/ready", get(health::readiness))
        .route("/metrics", get(metrics::prometheus_metrics))
        .route("/openapi.json", get(openapi_spec))
        .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
        .with_state(engine)
}

#[allow(clippy::needless_for_each, clippy::doc_markdown)]
#[derive(OpenApi)]
#[openapi(
    info(
        title = "iron-defer REST API",
        version = "0.1.0",
        description = "Durable background task execution engine for Rust"
    ),
    paths(
        tasks::create_task,
        tasks::get_task,
        tasks::get_audit_log,
        tasks::delete_task,
        tasks::signal_task,
        tasks::list_tasks,
        queues::list_queues,
        health::liveness,
        health::readiness,
        metrics::prometheus_metrics,
        openapi_spec,
    ),
    components(schemas(
        tasks::CreateTaskRequest,
        tasks::SignalTaskRequest,
        tasks::TaskResponse,
        tasks::AuditLogResponse,
        tasks::ListTasksResponse,
        queues::QueueStatsResponse,
        ErrorResponse,
        ErrorDetail,
    ))
)]
struct ApiDoc;

/// `GET /openapi.json` — return the `OpenAPI` specification.
#[utoipa::path(
    get, path = "/openapi.json",
    responses(
        (status = 200, description = "OpenAPI 3.1 JSON document"),
    )
)]
#[tracing::instrument(skip_all, fields(method = "GET", path = "/openapi.json"))]
async fn openapi_spec() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
