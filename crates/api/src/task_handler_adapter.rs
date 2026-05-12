//! Generic adapter that turns a concrete `T: Task` into a type-erased
//! `TaskHandler` for storage in the `TaskRegistry`.
//!
//! Architecture §C4 specifies this exact pattern. The adapter holds zero
//! state — it only carries the `T` type parameter so the `execute` method
//! can deserialize the payload into the right concrete type before calling
//! `T::execute(ctx)`.

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;

use iron_defer_application::TaskHandler;
use iron_defer_domain::{PayloadErrorKind, Task, TaskContext, TaskError};

pub(crate) struct TaskHandlerAdapter<T: Task>(PhantomData<T>);

impl<T: Task> TaskHandlerAdapter<T> {
    pub(crate) fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T: Task> TaskHandler for TaskHandlerAdapter<T> {
    fn kind(&self) -> &'static str {
        T::KIND
    }

    fn execute<'a>(
        &'a self,
        payload: &'a serde_json::Value,
        ctx: &'a TaskContext,
    ) -> Pin<Box<dyn Future<Output = Result<(), TaskError>> + Send + 'a>> {
        Box::pin(async move {
            // Deserialize via the by-reference `Deserializer` impl on
            // `&serde_json::Value` so we avoid cloning the entire JSON tree
            // on the per-task hot path. Architecture §C4 calls out
            // explicit allocation control as the reason this trait does
            // NOT use `#[async_trait]`; honoring that intent means avoiding
            // hidden payload clones too.
            let task: T = T::deserialize(payload).map_err(|e| TaskError::InvalidPayload {
                kind: PayloadErrorKind::Deserialization {
                    message: e.to_string(),
                },
            })?;
            task.execute(ctx).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iron_defer_domain::{TaskId, WorkerId};
    use serde::{Deserialize, Serialize};
    use std::sync::Arc;

    /// Test fixture: a minimal `Task` impl used to exercise
    /// `TaskHandlerAdapter` directly.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct UnitTestTask {
        n: i32,
    }

    impl Task for UnitTestTask {
        const KIND: &'static str = "unit_test_task";

        async fn execute(&self, _ctx: &TaskContext) -> Result<(), TaskError> {
            Ok(())
        }
    }

    fn sample_ctx() -> TaskContext {
        TaskContext::new(
            TaskId::new(),
            WorkerId::new(),
            iron_defer_domain::AttemptCount::new(1).unwrap(),
        )
    }

    #[tokio::test]
    async fn task_handler_adapter_kind_matches_task_kind() {
        let adapter = TaskHandlerAdapter::<UnitTestTask>::new();
        assert_eq!(adapter.kind(), UnitTestTask::KIND);
    }

    #[tokio::test]
    async fn task_handler_adapter_executes_valid_payload() {
        let adapter: Arc<dyn TaskHandler> = Arc::new(TaskHandlerAdapter::<UnitTestTask>::new());
        let payload = serde_json::to_value(UnitTestTask { n: 42 }).expect("serialize");
        let ctx = sample_ctx();

        let result = adapter.execute(&payload, &ctx).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
    }

    #[tokio::test]
    async fn task_handler_adapter_maps_serde_error_to_invalid_payload() {
        let adapter: Arc<dyn TaskHandler> = Arc::new(TaskHandlerAdapter::<UnitTestTask>::new());
        // Wrong shape: missing required field `n`.
        let bad_payload = serde_json::json!({"wrong": "shape"});
        let ctx = sample_ctx();

        let err = adapter
            .execute(&bad_payload, &ctx)
            .await
            .expect_err("malformed payload must error");
        match err {
            TaskError::InvalidPayload {
                kind: PayloadErrorKind::Deserialization { message },
            } => {
                assert!(
                    message.contains("missing field") || message.contains('n'),
                    "expected serde error mentioning the missing field, got: {message}"
                );
            }
            other => panic!("expected InvalidPayload::Deserialization, got {other:?}"),
        }
    }
}
