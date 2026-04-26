//! Custom axum extractors that convert rejection errors into the
//! architecture-mandated JSON error envelope.
//!
//! Axum's built-in `Json` and `Path` extractors return plain-text
//! rejections on parse failure. These wrappers intercept the rejection
//! and produce `{"error":{"code":"...","message":"..."}}` responses.

use axum::extract::FromRequestParts;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use serde::de::DeserializeOwned;

use super::errors::{ErrorDetail, ErrorResponse};

/// Drop-in replacement for `axum::Json` that returns structured JSON
/// errors on deserialization failure.
pub struct JsonBody<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for JsonBody<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = JsonBodyRejection;

    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(JsonBody(value)),
            Err(rejection) => Err(JsonBodyRejection(rejection)),
        }
    }
}

pub struct JsonBodyRejection(JsonRejection);

impl IntoResponse for JsonBodyRejection {
    fn into_response(self) -> Response {
        let status = self.0.status();
        let code = match status {
            StatusCode::UNPROCESSABLE_ENTITY => "INVALID_PAYLOAD",
            StatusCode::PAYLOAD_TOO_LARGE => "PAYLOAD_TOO_LARGE",
            _ => "BAD_REQUEST",
        };
        let body = ErrorResponse {
            error: ErrorDetail {
                code: code.to_string(),
                message: self.0.body_text(),
            },
        };
        (status, axum::Json(body)).into_response()
    }
}

/// Drop-in replacement for `axum::extract::Path` that returns structured
/// JSON errors on path parameter parse failure.
pub struct PathParam<T>(pub T);

impl<S, T> FromRequestParts<S> for PathParam<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = PathParamRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(PathParam(value)),
            Err(rejection) => Err(PathParamRejection(rejection)),
        }
    }
}

pub struct PathParamRejection(PathRejection);

impl IntoResponse for PathParamRejection {
    fn into_response(self) -> Response {
        let body = ErrorResponse {
            error: ErrorDetail {
                code: "INVALID_PATH_PARAMETER".to_string(),
                message: self.0.body_text(),
            },
        };
        (StatusCode::BAD_REQUEST, axum::Json(body)).into_response()
    }
}
