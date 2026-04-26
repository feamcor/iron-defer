# Epic 1A Retrospective — Task Persistence & Domain Model

**Date:** 2026-04-06
**Epic:** 1A — Task Persistence & Domain Model
**Stories completed:** 3 (1A.1, 1A.2, 1A.3)
**Status:** ✅ All stories `done`, all reviews applied, committed as `b3fbebd`

---

## Epic Summary

A developer can define task types using the `Task` trait, persist tasks to named queues in PostgreSQL, and submit/retrieve them through the embedded library API — establishing the foundational domain model and persistence layer.

The end-to-end shape that now works:

```rust
let engine = IronDefer::builder()
    .pool(pool)
    .register::<MyTask>()
    .build()
    .await?;

let task = engine.enqueue("default", MyTask { ... }).await?;
let fetched = engine.find(task.id).await?;
let listing = engine.list("default").await?;
```

## Metrics

| Metric | Value |
|---|---|
| Stories shipped | 3 / 3 |
| Tests passing | 70 (19 domain unit + 11 application unit + 20 infra unit + 6 infra integration + 5 api unit + 9 api integration) |
| Test count growth | 0 → 19 (1A.1) → 44 (1A.2) → 67 (1A.3 dev) → 70 (1A.3 review patches) |
| Adversarial code reviews | 3 (one per story, 3 layers each: Blind Hunter, Edge Case Hunter, Acceptance Auditor) |
| Decisions resolved in review | 4 (1A.2: 3, 1A.3: 1) |
| Patches applied in review | 21 (1A.1: 9, 1A.2: 5, 1A.3: 11 — incl. 2 SQL CHECK constraints, 1 instrument decoration sweep, 1 deserialize-by-reference perf fix) |
| Deferred-work items resolved | 8 (across the three story reviews) |
| Deferred-work items still open | 17 (logged with target stories: Epic 1B, Story 1A.3 already-merged, Epic 5, Story 4.1, Story 3.1) |
| Files committed | 51 across `crates/`, `migrations/`, `.sqlx/`, `docs/artifacts/`, workspace config |
| Quality gates green | `cargo fmt --check`, `cargo clippy -- -D clippy::pedantic`, `cargo deny check bans`, `cargo test --workspace` |
| Production-graph TLS audit | `cargo tree -e normal \| grep -E "openssl\|native-tls"` empty |

---

## Successes & Strengths

### What worked well

1. **Adversarial review caught load-bearing defects** that the implementation pass missed.
   - 1A.2: `last_error` truncation only ran on the read path, not the write path. The integration test acknowledged this in its own comment but called itself complete. The Blind Hunter + Edge Case Hunter + Acceptance Auditor all flagged it independently. The patch added `truncate_last_error_borrow` and a load-bearing `SELECT octet_length(last_error)` assertion.
   - 1A.3: `IronDefer::enqueue/find/list` had **zero `#[instrument]` decoration** — the spec's Critical Conventions explicitly mandated `skip(self, payload)` on these methods, but the impl shipped without `#[instrument]` at all. The Acceptance Auditor caught it.
   - 1A.3: `TaskHandlerAdapter::execute` had **zero test coverage** — the central new code path of the entire story. Edge Case Hunter caught it; the patch added 3 unit tests including a malformed-payload `serde::Error → InvalidPayload` mapping test.

2. **The "validate at the edge" hexagonal principle held up under pressure.** When 1A.2 added `TryFrom<TaskRow>` validation in `PostgresTaskRepository`, the domain crate stayed free of policy. When 1A.3's adversarial review questioned cross-field invariants (`claimed_by`/`claimed_until`, `attempts > max_attempts`), the answer was clear: those belong to Epic 1B's claim flow, not 1A.2's storage layer. Hexagonal layering made the deferral defensible instead of arbitrary.

3. **Story-to-story context engineering compounded.** Each story's `Dev Notes` section explicitly cited "Previous Story Intelligence" with concrete signatures, error variants, conventions, and gotchas. Story 1A.3 built on `TaskRecord::new` (added in 1A.2 because of `#[non_exhaustive]`), `MIGRATOR` (1A.2), and the testcontainers `OnceCell` pattern (1A.2) — all documented in 1A.2's completion notes and copied into 1A.3's Dev Notes verbatim.

4. **Decisions resolved with explicit two-option tradeoffs** rather than implicit choices. Each story's review surfaced 1–3 spec-vs-reality decisions (`create_pool` return type, `deny.toml exclude-dev`, `query!` vs `query_as!`, enqueue registry check) and the user picked one with the reasoning recorded in the story file. Future story authors can read the decisions and understand WHY the spec was deviated from.

5. **Spec amendments were captured inline.** When the user chose option (b) on a decision, the AC text in the story file was edited with an "Amended 2026-04-06" note rather than left stale. The story file remains the source of truth.

6. **`cargo deny` enforcement became meaningful at 1A.2.** Story 1A.1's deny rule passed trivially because no production deps were resolved. Story 1A.2 added `sqlx` to the resolved graph, exposed false positives from `schannel`/`security-framework` (rustls cert-store helpers, not TLS implementations), and the 1A.2 review's option (b) decision narrowed the ban list to actual TLS impls. The architectural rule now has teeth.

7. **`mockall::automock` integrated cleanly.** Adding `#[cfg_attr(test, mockall::automock)]` to `TaskRepository` in 1A.3 let `SchedulerService` get 5 unit tests that exercise the happy path AND argument predicates without spinning up a database. The compile-time cost was modest (mockall_derive + tokio-macros + a few crates).

### Where we exceeded the spec

- **70 tests passing** vs the 1A.3 target of ~62.
- **`builder_runs_migrations_by_default` rewritten with `fresh_unmigrated_pool`** during 1A.3 review — the original test was tautological because it ran against the already-migrated shared pool. The patched version actually proves the migrator ran.
- **`payload.clone()` eliminated from `TaskHandlerAdapter::execute`** during 1A.3 review via `T::deserialize(payload)` (by-reference deserialization through `&serde_json::Value`'s `Deserializer` impl). Honors the architectural rationale for the manual `Pin<Box<...>>` shape ("explicit hot-path allocation control").

---

## Challenges & Growth Areas

### Recurring patterns across all 3 stories

1. **Spec wording vs implementation drift kept catching up to us.**
   - 1A.2: AC said `create_pool -> Result<PgPool, PostgresAdapterError>`, impl returned `TaskError` to dodge a `private_interfaces` lint. Caught in review, resolved by amending the AC.
   - 1A.2: AC said `save` should use `sqlx::query!`, impl used `query_as!`. Caught in review, resolved by amending the AC.
   - 1A.3: AC 8 said `tokio = { workspace = true }` should be in `[dependencies]`, impl had it only in `[dev-dependencies]`. Caught in review, patched.
   - **Lesson:** the implementation pass should re-read the AC text after writing each piece of code, or the create-story phase should call out tradeoffs the implementer is likely to make so they're explicit decisions instead of silent deviations.

2. **The implementation pass tends to under-test the critical new code.** In 1A.2 the integration test for `last_error` truncation had a comment admitting it didn't verify the storage-side enforcement. In 1A.3 the central new code path (`TaskHandlerAdapter::execute`) had zero coverage. In both cases, adversarial review caught the gap before merge — but it's a recurring failure mode of the same implementer-then-test workflow.
   - **Lesson:** during create-story, identify the SINGLE most load-bearing test for each story (the one that, if it false-passes, would let a real bug ship) and call it out explicitly in the AC. Story 1A.2's `last_error_is_truncated_to_4_kib` AC was specific about "persisted-and-read-back value is exactly 4096 bytes" — but the implementer interpreted "persisted-and-read-back" as the in-memory return value of `save()`. Tighter wording: "verified via raw `SELECT octet_length(last_error)` query, not just the `save()` return value."

3. **`#[non_exhaustive]` types need constructors before they cross crate boundaries.** Story 1A.1 marked `TaskRecord` and `TaskContext` `#[non_exhaustive]` for forward-compatibility. Story 1A.2 needed to construct `TaskRecord` from outside the domain crate (in the `PostgresTaskRepository::TryFrom<TaskRow>`) and discovered the constraint mid-implementation, requiring an unplanned `pub fn TaskRecord::new` addition.
   - **Lesson:** when adding `#[non_exhaustive]` to a type, immediately add a `pub fn new` constructor — the next story will need it.

4. **Spec text vs code-review-driven amendments piled up unevenly.** 1A.2 had 3 amendments encoded inline; 1A.3 had 3. The spec drifts from the original architecture document silently as we go. This is fine for now (the code is the source of truth), but Epic 5's "production readiness" pass should sweep the architecture document to align with the actual implementation.
   - **Lesson:** at the start of each new epic, do a 30-minute pass to reconcile architecture document vs code. Trust drifts otherwise.

### Technical debt incurred

See `docs/artifacts/implementation/deferred-work.md` for the full ledger. Highlights:

- **`(claimed_by, claimed_until)` cross-field invariant** is unguarded in storage. Defer to Epic 1B claim flow — that's where the consistency is established.
- **`attempts <= max_attempts` cross-field invariant** is unguarded. Same — Epic 1B retry executor.
- **`save()` is INSERT-only** with no upsert / no distinct duplicate-id error variant. Partially resolved in 1A.3 (enqueue always generates fresh `TaskId`), but caller-supplied idempotency keys are still deferred to whenever the REST API ships.
- **`OnceCell<Option<TestDb>>` caches Docker failure** for the entire test binary lifetime. Defer — needs `IRON_DEFER_REQUIRE_DB=1` env-var opt-in.
- **`MIGRATOR.run()` partial migration recovery undefined.** Defer to Epic 5.
- **`task_trait_compiles_with_native_async_fn` no-op test** — pre-existing 1A.1 cleanup ticket.
- **`MIGRATOR.run()` opaque error wrapping** loses `MigrateError` variant info. Defer to Epic 5 typed-error-model review.
- **`InvalidPayload` and `ExecutionFailed` remain stringly-typed.** Defer to Epic 1B once `TaskExecutor` redesign establishes the concrete source shapes.
- **`Task` trait `Serialize + DeserializeOwned` supertrait bounds** — RESOLVED in 1A.3.
- **`TaskError::NotFound` reconciliation** — RESOLVED in 1A.2 (variant removed).
- **`TaskError::Storage` typed source** — RESOLVED in 1A.2 (now boxes the underlying error).
- **`last_error` length cap** — RESOLVED in 1A.2 (4 KiB UTF-8-boundary truncation on both write and read paths after 1A.2 review).

### Process friction

1. **The `.sqlx/` cache regeneration loop required spinning up a local Postgres container** every time a query string changed. Three regenerations across 1A.2 (initial + after ORDER BY tiebreaker patch). Manageable but adds friction. Epic 5's CI work could automate this via a pre-commit hook.

2. **Code review surfaces the same recurring spec-text vs implementation-text mismatch every story.** A short pre-implementation checklist ("Re-read each AC line by line, mark each as verbatim/amended/disputed before writing code") could front-load the decisions instead of catching them in review.

---

## Key Lessons (most actionable)

1. **The spec is a contract, not a starting point.** Every "small" deviation — `query!` vs `query_as!`, `PostgresAdapterError` vs `TaskError` return type, instrument decoration scope — became a code-review finding. The implementation pass should treat the AC text as binding and either honor it or escalate the deviation as an explicit decision before writing the code.

2. **The Critical Conventions section in Dev Notes is load-bearing.** Story 1A.3's `#[instrument]` violation came from skimming past the Critical Conventions during implementation. Story 1A.2's `(claimed_by, claimed_until)` cross-field deferral was clean precisely because the Out-of-Scope section was explicit about what belonged where. **More effort into Dev Notes pays off in fewer review rounds.**

3. **The single most useful test is the one that checks the actual storage state, not the code-path return value.** 1A.2's `last_error` test had to be rewritten to query Postgres directly via `SELECT octet_length` because the original test was checking the in-memory value AFTER the read-side truncation ran. Pattern: when validating a write-side guarantee, write a test that reads through a different code path than the one being tested.

4. **Adversarial review with multiple layers catches more than any single layer.** The Blind Hunter (no project context) caught the false positives the Acceptance Auditor would have rationalized away (claim of `floor_char_boundary` being unstable — verified false; claim of `exclude-dev` not being a valid option — verified false). The Acceptance Auditor caught the spec-text deviations the Blind Hunter couldn't see (no spec, no deviation). The Edge Case Hunter caught the cross-field invariant gaps the other two missed.

5. **Decisions deserve their own first-class artifact.** Every code-review session produced 1–3 decisions. Encoding them inline in the story file (with the rejected option also documented) creates a paper trail that future story authors can read instead of re-litigating.

6. **`cargo deny` is paper-only until you have real production deps in the resolved graph.** Story 1A.1's deny rule was a placeholder. Story 1A.2's review caught false positives that prompted the option (b) narrowing. Lesson: don't trust a deny rule until you've verified it bites on real production deps AND doesn't false-positive on dev deps.

---

## Action Items

| # | Item | Owner | Target |
|---|---|---|---|
| 1 | Pre-implementation AC walkthrough — read each AC line by line and explicitly mark verbatim / amended / disputed BEFORE writing code | Dev | Story 1B.1 onwards |
| 2 | When marking a type `#[non_exhaustive]`, immediately add a `pub fn new` constructor in the same commit | Dev | Story 1B.1 onwards |
| 3 | Reconcile architecture document vs code at the start of Epic 1B (30-minute pass) | Dev | Epic 1B kickoff |
| 4 | For each story's "central new code path", call out the SINGLE most load-bearing test in the AC text with explicit storage-state verification wording | Story author | Story 1B.1 onwards |
| 5 | Run `code-review` with a different LLM than the implementer when possible (already noted in dev-story workflow tip) | User | Each story |
| 6 | Add `IRON_DEFER_REQUIRE_DB=1` env-var opt-in to `OnceCell` test pool helpers | Dev | Whenever CI is set up |
| 7 | Sweep `deferred-work.md` at start of Epic 1B and confirm each item has a target story | Dev | Epic 1B kickoff |

---

## Next Epic Preview — Epic 1B: Claiming & Execution Engine

**Stories ahead:**
- 1B.1 — Atomic claiming and task completion (`SKIP LOCKED` claim query, lease management)
- 1B.2 — Worker pool and execution loop (`tokio::JoinSet` + `Semaphore` + poll loop)
- 1B.3 — REST API submit and query tasks (`axum` handlers + DTOs)

**Carry-over from Epic 1A that affects Epic 1B:**

1. **`TaskExecutor` port trait is a stub** (deferred from 1A.1). Story 1B.1 must redesign its signature to take a `TaskContext` and dispatch through the registry.

2. **`(claimed_by, claimed_until)` cross-field invariant** must be enforced in 1B.1 — either via SQL `CHECK ((claimed_by IS NULL) = (claimed_until IS NULL))` migration or via the claim query's atomicity guarantee.

3. **`attempts <= max_attempts` cross-field invariant** must be enforced when the retry executor lands. Terminal-failure transition to `failed` status happens when `attempts == max_attempts` and another failure occurs.

4. **`TaskError::ExecutionFailed` typed source variant** should land alongside the `TaskExecutor` redesign. Story 1A.2's deferred-work entry tracks this — once 1B.1 establishes the concrete source shapes, the stringly-typed `reason` can become `#[from] source: ConcreteError`.

5. **Worker pool pulls `Arc<TaskRegistry>` from `IronDefer::registry()`** — already plumbed in 1A.3. The worker pool's dispatch loop calls `registry.get(task.kind)` and panics with a descriptive message on `None` per the architectural contract (1A.3 review decision option (b) tightens enqueue but NOT the dispatch site — that's still 1B's responsibility).

6. **The 1A.2 `idx_tasks_claiming` partial index** is in place and ready for the `SKIP LOCKED` claim query. 1B.1 must use it via the query plan.

7. **Mockall is wired up.** 1B.1's worker pool can mock both `TaskRepository` AND `TaskExecutor` for unit tests, no testcontainers needed for the dispatch logic.

**Critical path before starting Epic 1B:**

- Nothing blocking. Epic 1A is fully committed (`b3fbebd`), all quality gates green, deferred-work logged.

---

## Readiness Assessment

| Dimension | Status |
|---|---|
| Testing & Quality | ✅ 70 tests passing, all gates green |
| Deployment | N/A (library mode only; standalone binary is Epic 4) |
| Stakeholder Acceptance | N/A (greenfield single-developer project) |
| Technical Health | ✅ Production graph rustls-only, no unsafe code, no `unwrap()` outside tests, no `anyhow` |
| Unresolved Blockers | None |
| Documentation | ✅ Story files + Change Log + deferred-work.md + this retrospective |

**Verdict: Epic 1A is fully complete. Clear to proceed to Epic 1B.**

---

## Closing Note

Epic 1A delivered the foundational scaffolding for everything that comes next. The hexagonal architecture (ADR-0001) earned its keep — domain stayed clean, application stayed thin, infrastructure absorbed the complexity, api crate became the only place where the wiring lives. The adversarial review process caught real defects that would have shipped otherwise. The mockall integration unblocks every future application-layer story from needing testcontainers.

The biggest single risk going into Epic 1B is the `TaskExecutor` redesign — the current stub trait is a placeholder, and the worker pool's dispatch loop has to work end-to-end with real handler invocation. The good news: `TaskHandlerAdapter::execute` is already covered by 1A.3's unit tests, so the bridge from `Task` → `dyn TaskHandler` is exercised. Epic 1B just needs to wire it into the polling loop.
