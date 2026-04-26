//! Port traits — abstract boundaries between the application layer and the
//! infrastructure adapters that satisfy them.

pub mod task_executor;
pub mod task_repository;

pub use task_executor::TaskExecutor;
pub use task_repository::{RecoveryOutcome, TaskRepository, TransactionalTaskRepository};
