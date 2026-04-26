# Consistency and Delivery Semantics

## Question

What delivery guarantees does iron-defer provide?

## Short Answer

iron-defer provides at-least-once execution.

- tasks are persisted before execution
- failures can trigger retries
- sweeper recovery can re-dispatch expired leases
- duplicate execution is possible and expected by design

This model prioritizes durability and recoverability over exactly-once complexity.

## Tradeoffs

- Pros: strong recovery properties with simple infrastructure (Postgres-based control plane).
- Cons: handlers and producers must be idempotent to tolerate duplicate execution.

## Related Docs

- [How to Create Idempotent Producers](../guides/how-to-create-idempotent-producers.md)
- [Claiming and Leases](claiming-and-leases.md)
