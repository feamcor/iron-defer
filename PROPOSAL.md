# IronDefer Durable Task Engine

## Summary

**IronDefer** is a distributed system built in Rust that ensures _at-least-once_ execution of critical background tasks. Unlike traditional _fire-and-forget_ message queues, **IronDefer** treats the database as a persistent state machine. This allows the system to recover from worker crashes, network partitions, and unexpected reboots without losing task progress or metadata.

Designed with a flexible dual-target architecture, it can run as a standalone binary or be embedded as a library within other Rust applications. Additionally, the standalone binary can be packaged as a Docker image and deployed using Docker Compose (for local or small deployments) or Kubernetes (for scaled production).

## System Architecture & Load Balancing

The system achieves horizontal scalability and load balancing through a _Pull-based Distributed Queue_ pattern using Postgres. Instead of a central _Load Balancer_ pushing tasks to workers (which creates a bottleneck and requires workers to be reachable via IP), **IronDefer** uses competing consumers with advisory locking.

- **Distributed Coordination**: Every worker node runs an asynchronous `Fetcher` loop.
- **Atomic Claiming**: Workers execute a `SELECT ... FOR UPDATE SKIP LOCKED` query. This allows multiple nodes to query the same table simultaneously. Postgres automatically skips rows currently locked by other nodes and gives the next available task to the requesting worker.
- **Lease Management**: When a worker claims a task, it _leases_ the job for a specific duration (e.g., 5 minutes). If the worker node dies, the lease expires, and the task becomes visible to other nodes for a retry.
- **Implicit Load Balancing**: Faster workers or nodes with more available CPU cycles will naturally poll more frequently, effectively balancing the load based on actual capacity rather than simple _Round Robin_.

## Implementation Roadmap

### Week 1: Core Persistence & Inbound Interface

- **Objective**: Establish the durable storage layer and the task submission entry points.
- **Project Structure**: Set up a dual-target Cargo project (providing both `lib.rs` for the core engine and `main.rs` for the standalone executable) to enable embedded and standalone execution modes.
- **Database Modeling**: Implement the Postgres schema using `sqlx`. This includes the core tasks table, custom types for state tracking (`Pending`, `Running`, `Failed`, `Completed`), and metadata columns (`retry_count`, `claimed_until` for lease management).
- **API Layer**: Develop a high-performance REST API using `axum`. This service will handle task validation and transactional insertion into the database.
- **Administrative CLI**: Build a `clap`-based CLI for operators to submit tasks manually and inspect the global queue state.
- **Knowledge Spike**: Finalize the strategy for serialization (leveraging `serde` and `serde_json`) to ensure the _Payload_ remains flexible for different task types. Establish a baseline for unit testing.

### Week 2: Distributed Execution & Concurrency

- **Objective**: Build the worker engine capable of handling multiple concurrent tasks per node.
- **The Claiming Engine**: Implement the `SKIP LOCKED` logic to facilitate distributed coordination across multiple worker instances.
- **Async Worker Pool**: Utilize `tokio` to manage a local concurrency pool on each node, with rules to prevent resource exhaustion.
- **Task Abstraction**: Define a `Task` trait in Rust, allowing the engine to execute different types of logic (e.g., shell scripts, HTTP webhooks, or internal Rust functions) through a unified interface.
- **Error Handling**: Implement robust error mapping and initial retry logic (immediate vs. delayed).

### Week 3: Resilience & Observability

- **Objective**: Ensure the _Durable_ promise and provide production-grade monitoring.
- **Reaper Service**: Implement a background `Sweeper` process that identifies and recovers `Zombie` tasks stuck in a `Running` state where the task lease (`claimed_until`) has expired.
- **Structured Logging**: Integrate the `tracing` crate to provide distributed context. Every log line will be tagged with a `task_id` for cross-node debugging.
- **Telemetry**: Expose OpenTelemetry-compatible metrics to monitor queue depth, execution latency, and success/failure ratios.
- **Graceful Termination**: Implement signal handling (`SIGTERM`) to allow workers to _check-in_ their final state before the node shuts down.

### Week 4: Optimization, Hardening & Review

- **Objective**: Finalize the system for production deployment and perform architectural audits.
- **Containerization**: Package the application as a Docker image and provide examples/manifests for deployment using Docker Compose and Kubernetes.
- **Performance Tuning**: Optimize Postgres indices and analyze query plans for the `Fetcher` loop to ensure minimal overhead as the tasks table grows.
- **Backoff Strategies**: Implement exponential backoff for retries to prevent _thundering herd_ issues during downstream service outages.
- **Integration Testing**: Execute _Chaos Tests_ using `testcontainers-rs`, where worker processes are intentionally killed mid-execution to verify that tasks are correctly recovered by other nodes.

### Post-MVP: Review & Refinement

- **Code Review**: Perform a deep dive into the use of concurrency primitives to ensure optimal memory safety and performance.
- **Documentation**: Complete the technical specification, including the API contract and CLI manual.
- **Benchmarking**: Conduct load tests to determine the maximum tasks-per-second throughput of a single Postgres instance.
