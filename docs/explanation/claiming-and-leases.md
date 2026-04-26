# Claiming and Leases

## Question

How do workers coordinate task ownership without a broker?

## Short Answer

Workers claim tasks using Postgres row locking (`FOR UPDATE SKIP LOCKED`) and lease timestamps.

- workers can poll concurrently without locking each other on already-claimed rows
- claimed tasks move to `running` with lease metadata
- expired leases are recoverable by the sweeper

## Tradeoffs

- Pros: simple operational footprint, robust crash recovery, horizontal worker scaling.
- Cons: lease tuning is required to avoid slow recovery or false-positive recoveries.

## Related Docs

- [Consistency and Delivery Semantics](consistency-and-delivery-semantics.md)
- [How to Debug Stuck Tasks](../guides/how-to-debug-stuck-tasks.md)
