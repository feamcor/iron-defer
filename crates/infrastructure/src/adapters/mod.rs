//! Adapter implementations of application-layer port traits.
//!
//! `PostgresTaskRepository` satisfies the `TaskRepository` port from the
//! application crate.

pub mod postgres_task_repository;

pub use postgres_task_repository::{PostgresCheckpointWriter, PostgresTaskRepository};
