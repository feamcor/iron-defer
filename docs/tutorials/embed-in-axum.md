# Tutorial: Embed in Axum

## Objective

Integrate iron-defer as a library inside an Axum application.

## Prerequisites

- Axum application scaffold
- reachable Postgres database

## Steps

1. Add dependencies:

   ```toml
   [dependencies]
   iron-defer = { git = "https://github.com/feamcor/iron-defer" }
   sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "postgres"] }
   axum = "0.8"
   serde = { version = "1", features = ["derive"] }
   tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
   ```

2. Define a task type and `Task` implementation.

3. Build the engine at startup:

   ```rust
   let pool = sqlx::PgPool::connect(&std::env::var("DATABASE_URL")?).await?;

   let engine = iron_defer::IronDefer::builder()
       .pool(pool)
       .register::<EmailTask>()
       .queue("default")
       .build()
       .await?;
   ```

4. Start workers in a background task:

   ```rust
   let engine = std::sync::Arc::new(engine);
   let token = iron_defer::CancellationToken::new();
   let bg_engine = engine.clone();
   let bg_token = token.clone();

   tokio::spawn(async move {
       let _ = bg_engine.start(bg_token).await;
   });
   ```

5. Use `engine.enqueue(...)` from Axum handlers.

6. On shutdown, cancel the token.

## Verification

- enqueue calls return task records
- background workers process queued tasks
- shutdown cancels worker loop cleanly

## Next Steps

- See [Embedded Library Guide](../guides/embedded-library.md)
- Continue with [Suspended Workflow](suspended-workflow.md)
