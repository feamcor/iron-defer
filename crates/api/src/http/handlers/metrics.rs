//! Prometheus `/metrics` scrape endpoint (FR18, NFR-I2).

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::IronDefer;
use crate::http::errors::{ErrorDetail, ErrorResponse};

/// `GET /metrics` — Prometheus text exposition format >= 0.0.4.
///
/// Encodes all `OTel` metrics registered with the `prometheus::Registry`
/// into the standard Prometheus text format. Returns 404 when the engine
/// was built without a Prometheus registry (embedded mode without metrics).
#[utoipa::path(
    get, path = "/metrics",
    responses(
        (status = 200, description = "Prometheus text exposition format"),
        (status = 404, description = "Metrics not configured"),
    )
)]
pub async fn prometheus_metrics(State(engine): State<Arc<IronDefer>>) -> Response {
    let Some(ref registry) = engine.prometheus_registry else {
        return error_response(
            StatusCode::NOT_FOUND,
            "METRICS_UNAVAILABLE",
            "metrics not configured",
        );
    };

    let encoder = prometheus::TextEncoder::new();
    let metric_families = registry.gather();

    match encoder.encode_to_string(&metric_families) {
        Ok(body) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            body,
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "failed to encode Prometheus metrics");
            // Use the canonical ErrorResponse serializer so any control
            // characters in the error message are JSON-escaped by serde_json.
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "METRICS_ENCODE_ERROR",
                "failed to encode metrics",
            )
        }
    }
}

fn error_response(status: StatusCode, code: &str, message: &str) -> Response {
    let body = ErrorResponse {
        error: ErrorDetail {
            code: code.to_string(),
            message: message.to_string(),
        },
    };
    (status, Json(body)).into_response()
}
