//! Application services — orchestration logic over port traits.
//!
//! - `SchedulerService`: typed enqueue / find / list facade.
//! - `WorkerService`: async worker pool with bounded concurrency.
//! - `SweeperService`: zombie task recovery.

pub mod scheduler;
pub mod sweeper;
pub mod worker;

pub use scheduler::SchedulerService;
pub use sweeper::SweeperService;
pub use worker::{WorkerService, drain_join_set};
