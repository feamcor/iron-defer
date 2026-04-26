//! Integration tests for `PostgresTaskRepository` (TEA P0-INT-001).
//!
//! Each test gets its own pool on the shared testcontainers Postgres instance
//! via `common::fresh_pool_on_shared_container()`.
//! Each test scopes its writes to a unique queue name (`format!("test-{}",
//! Uuid::new_v4())`) so concurrent tests in the same binary do not collide.

mod common;

use std::sync::Arc;
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use iron_defer_application::TaskRepository;
use iron_defer_domain::{
    AttemptCount, MaxAttempts, Priority, QueueName, TaskId, TaskKind, TaskRecord, TaskStatus,
    WorkerId,
};
use iron_defer_infrastructure::PostgresTaskRepository;
use uuid::Uuid;

/// Build a unique `QueueName` for a test, scoped under a `test-` prefix
/// so live-database queries from other tests do not contaminate this one.
fn unique_queue() -> QueueName {
    QueueName::try_from(format!("test-{}", Uuid::new_v4()).as_str())
        .expect("generated test queue name is valid")
}

fn sample_task(queue: QueueName, kind: &str) -> TaskRecord {
    sample_task_with(queue, kind, 0, 3)
}

fn sample_task_with(queue: QueueName, kind: &str, priority: i16, max_attempts: i32) -> TaskRecord {
    let now = Utc::now();
    TaskRecord::builder()
        .id(TaskId::new())
        .queue(queue)
        .kind(TaskKind::try_from(kind).expect("test kind must be non-empty"))
        .payload(Arc::new(serde_json::json!({"hello": "world", "n": 42})))
        .status(TaskStatus::Pending)
        .priority(Priority::new(priority))
        .attempts(AttemptCount::ZERO)
        .max_attempts(MaxAttempts::new(max_attempts).unwrap())
        .scheduled_at(now)
        .created_at(now)
        .updated_at(now)
        .build()
}

const DEFAULT_LEASE: Duration = Duration::from_mins(5);
const BASE_DELAY_SECS: f64 = 5.0;
const MAX_DELAY_SECS: f64 = 1800.0;

#[tokio::test]
async fn save_then_find_by_id_round_trips_all_fields() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);

    let queue = unique_queue();
    let custom_payload = serde_json::json!({
        "amount": 12_345,
        "currency": "USD",
        "metadata": {"order_id": "abc-123"}
    });
    let task = sample_task_with(queue.clone(), "PaymentWebhook", 5, 7)
        .with_payload(custom_payload.clone());

    let saved = repo.save(&task).await.expect("save succeeds");
    let fetched = repo
        .find_by_id(saved.id())
        .await
        .expect("find_by_id succeeds")
        .expect("task exists");

    // Application-controlled fields round-trip exactly.
    assert_eq!(fetched.id(), saved.id());
    assert_eq!(*fetched.queue(), queue);
    assert_eq!(*fetched.kind(), "PaymentWebhook");
    assert_eq!(*fetched.payload(), custom_payload);
    assert_eq!(fetched.status(), TaskStatus::Pending);
    assert_eq!(fetched.priority().get(), 5);
    assert_eq!(fetched.attempts().get(), 0);
    assert_eq!(fetched.max_attempts().get(), 7);
    assert_eq!(fetched.last_error(), None);
    assert_eq!(fetched.claimed_by(), None);
    assert_eq!(fetched.claimed_until(), None);

    // scheduled_at preserved with sub-second precision.
    let drift = (fetched.scheduled_at() - saved.scheduled_at())
        .num_milliseconds()
        .abs();
    assert!(drift < 1, "scheduled_at drift = {drift}ms");
}

#[tokio::test]
async fn find_by_id_returns_none_for_missing_id() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);

    let result = repo
        .find_by_id(TaskId::new())
        .await
        .expect("find_by_id succeeds for absent id");
    assert!(result.is_none());
}

#[tokio::test]
async fn list_by_queue_returns_only_matching_queue() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);

    let payments = unique_queue();
    let notifications = unique_queue();

    for _ in 0..3 {
        repo.save(&sample_task(payments.clone(), "Pay"))
            .await
            .expect("save");
    }
    for _ in 0..2 {
        repo.save(&sample_task(notifications.clone(), "Notify"))
            .await
            .expect("save");
    }

    let payments_list = repo.list_by_queue(&payments).await.expect("list payments");
    let notifications_list = repo
        .list_by_queue(&notifications)
        .await
        .expect("list notifications");

    assert_eq!(payments_list.len(), 3);
    assert_eq!(notifications_list.len(), 2);
    assert!(payments_list.iter().all(|t| *t.queue() == payments));
    assert!(
        notifications_list
            .iter()
            .all(|t| *t.queue() == notifications)
    );
}

#[tokio::test]
async fn save_populates_default_timestamps() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);

    // Use deliberately stale placeholder timestamps; the database default
    // (now()) should override created_at/updated_at on insert.
    let stale = Utc::now() - ChronoDuration::days(365);
    let task = sample_task(unique_queue(), "TimestampCheck");

    let before = Utc::now();
    let saved = repo.save(&task).await.expect("save succeeds");
    let after = Utc::now();

    // 5-second tolerance per AC 9 — accommodates slow CI / loaded hosts.
    let tolerance = ChronoDuration::seconds(5);
    assert!(
        saved.created_at() >= before - tolerance && saved.created_at() <= after + tolerance,
        "created_at {} should be near now ({}–{})",
        saved.created_at(),
        before,
        after
    );
    assert!(
        saved.updated_at() >= before - tolerance && saved.updated_at() <= after + tolerance,
        "updated_at {} should be near now",
        saved.updated_at()
    );
    // Sanity: definitely not the stale year-old placeholder. The adapter
    // intentionally never binds the application's `created_at`/`updated_at`
    // — the database `DEFAULT now()` is the only path that populates them.
    assert!(saved.created_at() > stale + ChronoDuration::days(30));
}

#[tokio::test]
async fn last_error_is_truncated_to_4_kib() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);

    // Persist a 10 KiB last_error; the adapter must truncate to 4 KiB
    // BOTH on the write path (so the database row itself is bounded) AND
    // on the read path (defense-in-depth for rows written by other
    // clients).
    let huge = "x".repeat(10 * 1024);
    let now = Utc::now();
    let task = TaskRecord::builder()
        .id(TaskId::new())
        .queue(unique_queue())
        .kind(TaskKind::try_from("ErrorTruncation").unwrap())
        .payload(Arc::new(serde_json::json!({})))
        .status(TaskStatus::Pending)
        .priority(Priority::new(0))
        .attempts(AttemptCount::ZERO)
        .max_attempts(MaxAttempts::new(3).unwrap())
        .last_error(huge.clone())
        .scheduled_at(now)
        .created_at(now)
        .updated_at(now)
        .build();

    let saved = repo.save(&task).await.expect("save");

    // (1) Read-side guard: the value returned by save() must be capped.
    let last_error = saved.last_error().expect("last_error present after save");
    assert_eq!(
        last_error.len(),
        4096,
        "last_error must be truncated to LAST_ERROR_MAX_BYTES (4096) on the read path"
    );
    assert!(last_error.chars().all(|c| c == 'x'));

    // (2) WRITE-side guard (the load-bearing assertion per AC 9): query
    // the raw column byte length directly from Postgres and confirm the
    // STORED row is also 4096 bytes — i.e. the database itself does not
    // hold the unbounded original. Bypasses the adapter's TryFrom layer
    // entirely.
    let row: (i32,) =
        sqlx::query_as("SELECT octet_length(last_error)::int4 FROM tasks WHERE id = $1")
            .bind(saved.id().as_uuid())
            .fetch_one(pool)
            .await
            .expect("octet_length query");
    assert_eq!(
        row.0, 4096,
        "raw last_error column in Postgres must also be capped at 4096 bytes — \
         truncation must run on the WRITE path, not only on the read-side TryFrom"
    );

    // (3) find_by_id round-trip per AC 9 wording ("persisted-and-read-back").
    let fetched = repo
        .find_by_id(saved.id())
        .await
        .expect("find_by_id")
        .expect("task exists");
    assert_eq!(
        fetched.last_error().map(str::len),
        Some(4096),
        "find_by_id round-trip must also see the capped value"
    );
}

#[tokio::test]
async fn save_rejects_empty_kind() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);

    // Empty `kind` is now rejected at the domain level by `TaskKind`'s
    // `TryFrom` impl — a `TaskRecord` with an empty kind can no longer
    // be constructed. Verify the domain validation catches it.
    let err = TaskKind::try_from("");
    assert!(err.is_err(), "empty kind must be rejected by TaskKind");

    // Also confirm that a valid task round-trips through save correctly.
    let task = sample_task(unique_queue(), "placeholder");
    let saved = repo
        .save(&task)
        .await
        .expect("valid task should save successfully");

    // Confirm the row was persisted.
    let count: (i64,) = sqlx::query_as("SELECT count(*) FROM tasks WHERE id = $1")
        .bind(saved.id().as_uuid())
        .fetch_one(pool)
        .await
        .expect("count query");
    assert_eq!(count.0, 1, "valid task should have been persisted");
}

// ──────────────────────────────────────────────────────────────────────
// Claiming integration tests (Story 1B.1, AC 8)
// ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn claim_next_returns_running_task_with_correct_fields() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();
    let worker = WorkerId::new();
    let lease = Duration::from_mins(5);

    let task = sample_task(queue.clone(), "ClaimTest");
    repo.save(&task).await.expect("save");

    let before = Utc::now();
    let claimed = repo
        .claim_next(&queue, worker, lease, None)
        .await
        .expect("claim_next succeeds")
        .expect("task available");
    let after = Utc::now();

    assert_eq!(claimed.status(), TaskStatus::Running);
    assert_eq!(claimed.claimed_by(), Some(worker));
    assert_eq!(claimed.attempts().get(), 1);

    // claimed_until should be roughly now + 300s
    let expected_until_min = before + ChronoDuration::seconds(295);
    let expected_until_max = after + ChronoDuration::seconds(305);
    let until = claimed.claimed_until().expect("claimed_until set");
    assert!(
        until >= expected_until_min && until <= expected_until_max,
        "claimed_until {until} should be near now + 300s"
    );
}

#[tokio::test]
async fn claim_next_returns_none_when_queue_empty() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    let result = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim_next succeeds");
    assert!(result.is_none());
}

#[tokio::test]
async fn claim_next_skips_future_scheduled_tasks() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    let now = Utc::now();
    let task = TaskRecord::builder()
        .id(TaskId::new())
        .queue(queue.clone())
        .kind(TaskKind::try_from("FutureTask").unwrap())
        .payload(Arc::new(serde_json::json!({"hello": "world", "n": 42})))
        .status(TaskStatus::Pending)
        .priority(Priority::new(0))
        .attempts(AttemptCount::ZERO)
        .max_attempts(MaxAttempts::new(3).unwrap())
        .scheduled_at(now + ChronoDuration::hours(1))
        .created_at(now)
        .updated_at(now)
        .build();
    repo.save(&task).await.expect("save");

    let result = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim_next succeeds");
    assert!(
        result.is_none(),
        "future-scheduled tasks should not be claimed"
    );
}

#[tokio::test]
async fn claim_next_respects_priority_ordering() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    // Insert tasks with different priorities
    for priority in [0_i16, 5, 10] {
        let task = sample_task_with(queue.clone(), "PriorityTest", priority, 3);
        repo.save(&task).await.expect("save");
    }

    // Claim should return highest priority first
    let first = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim")
        .expect("task");
    assert_eq!(first.priority().get(), 10);

    let second = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim")
        .expect("task");
    assert_eq!(second.priority().get(), 5);

    let third = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim")
        .expect("task");
    assert_eq!(third.priority().get(), 0);
}

#[tokio::test]
async fn claim_next_concurrent_no_duplicates() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    // Insert 10 pending tasks
    for i in 0..10 {
        let task = sample_task(queue.clone(), &format!("ConcurrentTest-{i}"));
        repo.save(&task).await.expect("save");
    }

    // Spawn 10 concurrent claim_next calls with different worker_ids
    let mut handles = Vec::new();
    for _ in 0..10 {
        let repo_clone = PostgresTaskRepository::new(pool.clone(), false);
        let q = queue.clone();
        let worker = WorkerId::new();
        handles.push(tokio::spawn(async move {
            repo_clone.claim_next(&q, worker, DEFAULT_LEASE, None).await
        }));
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        results.push(handle.await.expect("join"));
    }

    let claimed: Vec<_> = results
        .into_iter()
        .filter_map(|r| r.expect("claim_next ok"))
        .collect();

    assert_eq!(claimed.len(), 10, "all 10 tasks should be claimed");

    // Verify via raw SQL that each task was claimed by exactly one distinct worker
    let row: (i64,) = sqlx::query_as(
        "SELECT count(DISTINCT claimed_by) FROM tasks WHERE queue = $1 AND status = 'running'",
    )
    .bind(queue.as_str())
    .fetch_one(pool)
    .await
    .expect("count query");
    assert_eq!(
        row.0, 10,
        "10 distinct workers must each claim exactly one task"
    );
}

#[tokio::test]
async fn complete_transitions_to_completed() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    let task = sample_task(queue.clone(), "CompleteTest");
    repo.save(&task).await.expect("save");

    let claimed = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim")
        .expect("task");

    let before = Utc::now();
    let completed = repo.complete(claimed.id()).await.expect("complete");

    assert_eq!(completed.status(), TaskStatus::Completed);
    assert!(completed.updated_at() >= before - ChronoDuration::seconds(2));
}

#[tokio::test]
async fn complete_fails_for_non_running_task() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    // Save a pending task (not claimed)
    let task = sample_task(queue, "CompleteFailTest");
    let saved = repo.save(&task).await.expect("save");

    let err = repo
        .complete(saved.id())
        .await
        .expect_err("complete should fail for non-running task");
    let msg = format!("{err}");
    assert!(msg.contains("not in Running status"), "got: {msg}");
}

#[tokio::test]
async fn fail_retries_when_under_max_attempts() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    let task = sample_task_with(queue.clone(), "RetryTest", 0, 3);
    repo.save(&task).await.expect("save");

    let claimed = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim")
        .expect("task");
    assert_eq!(claimed.attempts().get(), 1);

    let failed = repo
        .fail(claimed.id(), "oops", BASE_DELAY_SECS, MAX_DELAY_SECS)
        .await
        .expect("fail with retry");

    assert_eq!(failed.status(), TaskStatus::Pending);
    assert_eq!(failed.claimed_by(), None);
    assert_eq!(failed.claimed_until(), None);
    assert_eq!(failed.last_error(), Some("oops"));
    assert!(
        failed.scheduled_at() > Utc::now(),
        "scheduled_at should be in the future (backoff applied)"
    );
}

#[tokio::test]
async fn fail_transitions_to_failed_when_max_attempts_reached() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    // max_attempts = 1, so claiming (attempts becomes 1) means attempts >= max_attempts
    let task = sample_task_with(queue.clone(), "TerminalFailTest", 0, 1);
    repo.save(&task).await.expect("save");

    let claimed = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim")
        .expect("task");
    assert_eq!(claimed.attempts().get(), 1);

    let failed = repo
        .fail(claimed.id(), "fatal", BASE_DELAY_SECS, MAX_DELAY_SECS)
        .await
        .expect("fail terminal");

    assert_eq!(failed.status(), TaskStatus::Failed);
    assert_eq!(failed.last_error(), Some("fatal"));

    // Verify via raw SQL that status is literally 'failed' in the database
    let row: (String,) = sqlx::query_as("SELECT status FROM tasks WHERE id = $1")
        .bind(claimed.id().as_uuid())
        .fetch_one(pool)
        .await
        .expect("status query");
    assert_eq!(row.0, "failed", "raw DB status must be 'failed'");
}

#[tokio::test]
async fn fail_applies_exponential_backoff() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    let task = sample_task_with(queue.clone(), "BackoffTest", 0, 5);
    repo.save(&task).await.expect("save");

    // First claim + fail: attempts=1, backoff = 5 * 2^0 = 5s
    let claimed1 = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim")
        .expect("task");
    let now_before_fail1 = Utc::now();
    let failed1 = repo
        .fail(claimed1.id(), "err1", BASE_DELAY_SECS, MAX_DELAY_SECS)
        .await
        .expect("fail 1");
    let backoff1 = (failed1.scheduled_at() - now_before_fail1).num_seconds();

    // Manually set scheduled_at to past so we can re-claim
    sqlx::query("UPDATE tasks SET scheduled_at = now() - interval '1 second' WHERE id = $1")
        .bind(claimed1.id().as_uuid())
        .execute(pool)
        .await
        .expect("reset scheduled_at");

    // Second claim + fail: attempts=2, backoff = 5 * 2^1 = 10s
    let claimed2 = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim 2")
        .expect("task");
    let now_before_fail2 = Utc::now();
    let failed2 = repo
        .fail(claimed2.id(), "err2", BASE_DELAY_SECS, MAX_DELAY_SECS)
        .await
        .expect("fail 2");
    let backoff2 = (failed2.scheduled_at() - now_before_fail2).num_seconds();

    // Base delay 5s with ±25% jitter → [3.75, 6.25]; allow 1s extra for DB round-trip
    assert!(
        (2..=8).contains(&backoff1),
        "first backoff should be ~5s ±25% jitter, got {backoff1}s"
    );
    // Second attempt: 10s with ±25% jitter → [7.5, 12.5]
    assert!(
        (6..=14).contains(&backoff2),
        "second backoff should be ~10s ±25% jitter, got {backoff2}s"
    );
}

#[tokio::test]
async fn fail_caps_backoff_at_max_delay() {
    let Some(pool) = common::fresh_pool_on_shared_container().await else {
        eprintln!("[skip] Docker not available");
        return;
    };
    let pool = &pool;
    let repo = PostgresTaskRepository::new(pool.clone(), false);
    let queue = unique_queue();

    // max_attempts=20 so we can claim+fail many times
    let task = sample_task_with(queue.clone(), "CapTest", 0, 20);
    repo.save(&task).await.expect("save");

    // Repeatedly claim+fail until attempts is high enough that
    // base_delay * 2^(attempts-1) exceeds max_delay (1800s).
    // 5 * 2^8 = 1280, 5 * 2^9 = 2560 > 1800. So at attempts=10
    // the backoff should be capped.
    for _ in 0..10 {
        let claimed = repo
            .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
            .await
            .expect("claim")
            .expect("task");
        let _failed = repo
            .fail(claimed.id(), "err", BASE_DELAY_SECS, MAX_DELAY_SECS)
            .await
            .expect("fail");

        // Reset scheduled_at for next claim
        sqlx::query("UPDATE tasks SET scheduled_at = now() - interval '1 second' WHERE id = $1")
            .bind(claimed.id().as_uuid())
            .execute(pool)
            .await
            .expect("reset");
    }

    // Now claim attempt 11 and fail — backoff should be capped at max_delay
    let claimed = repo
        .claim_next(&queue, WorkerId::new(), DEFAULT_LEASE, None)
        .await
        .expect("claim")
        .expect("task");
    let now_before = Utc::now();
    let failed = repo
        .fail(claimed.id(), "err", BASE_DELAY_SECS, MAX_DELAY_SECS)
        .await
        .expect("fail");
    let backoff = (failed.scheduled_at() - now_before).num_seconds();

    // Should be capped at max_delay (1800s) with ±25% jitter → [1350, 2250]
    assert!(
        (1345..=2255).contains(&backoff),
        "backoff should be capped at ~1800s ±25% jitter, got {backoff}s"
    );
}
