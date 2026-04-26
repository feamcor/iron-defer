//! Queue statistics handler — `GET /queues`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use iron_defer_domain::QueueStatistics;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use utoipa::{IntoParams, ToSchema};

use crate::IronDefer;
use crate::http::errors::AppError;

/// Query parameters for `GET /queues`.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct ListQueuesParams {
    /// If true, returns statistics grouped by (queue, region).
    /// If false (default), returns statistics aggregated by queue name only.
    #[serde(default)]
    pub by_region: bool,
}

/// Response DTO for a single queue's statistics.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueueStatsResponse {
    pub queue: String,
    pub region: Option<String>,
    pub pending: u64,
    pub running: u64,
    pub suspended: u64,
    pub active_workers: u64,
}

impl From<QueueStatistics> for QueueStatsResponse {
    fn from(s: QueueStatistics) -> Self {
        Self {
            queue: s.queue.to_string(),
            region: s.region,
            pending: s.pending,
            running: s.running,
            suspended: s.suspended,
            active_workers: s.active_workers,
        }
    }
}

/// `GET /queues` — return per-queue statistics.
#[utoipa::path(
    get, path = "/queues",
    params(ListQueuesParams),
    responses(
        (status = 200, description = "Queue statistics list", body = Vec<QueueStatsResponse>),
    )
)]
/// # Errors
///
/// Returns `AppError` if the storage layer fails.
#[instrument(skip_all, fields(method = "GET", path = "/queues"), err)]
pub async fn list_queues(
    State(engine): State<Arc<IronDefer>>,
    Query(params): Query<ListQueuesParams>,
) -> Result<Json<Vec<QueueStatsResponse>>, AppError> {
    let stats = engine.queue_statistics(params.by_region).await?;
    let response: Vec<QueueStatsResponse> =
        stats.into_iter().map(QueueStatsResponse::from).collect();
    Ok(Json(response))
}
