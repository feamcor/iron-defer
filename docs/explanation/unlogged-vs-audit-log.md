# UNLOGGED vs Audit Log

## Question

When should `database.unlogged_tables` or `database.audit_log` be enabled?

## Short Answer

Choose based on whether throughput or durable traceability is the priority.

- `database.unlogged_tables=true` favors write throughput, but task data can be lost after crash recovery
- `database.audit_log=true` records lifecycle transitions durably, but adds write overhead and storage cost
- both flags cannot be enabled together

## Tradeoffs

- UNLOGGED mode: better performance, weaker durability and forensic history.
- Audit log mode: stronger compliance and debugging evidence, higher operational cost.

## Related Docs

- [How to Enable Audit Trail](../guides/how-to-enable-audit-trail.md)
- [Configuration Reference](../reference/config-reference.md)
