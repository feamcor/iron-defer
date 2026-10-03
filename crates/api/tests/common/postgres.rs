//! Postgres image used by every test container in the workspace.
//!
//! Shared by `common/mod.rs`, `chaos_common.rs`, and the infrastructure
//! crate's `tests/common/mod.rs` (the latter two via `#[path]`) so the image
//! tag is pinned in one place.

use testcontainers::{ContainerRequest, ImageExt};
use testcontainers_modules::postgres::Postgres;

/// Pinned Postgres image tag. `testcontainers-modules` defaults to
/// `11-alpine`; keep tests aligned with the compose/CI version.
const POSTGRES_TAG: &str = "18-alpine";

/// Postgres container request pinned to [`POSTGRES_TAG`].
pub fn postgres_image() -> ContainerRequest<Postgres> {
    Postgres::default().with_tag(POSTGRES_TAG)
}
