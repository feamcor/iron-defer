# Error Codes Reference

## Scope

REST error codes and CLI failure classes.

REST error envelope:

```json
{"error":{"code":"SCREAMING_SNAKE_CASE","message":"..."}}
```

## Canonical Tables/Entries

### Common API codes

| Code | HTTP | Meaning |
|---|---:|---|
| `INVALID_PAYLOAD` | 422 | body or field validation failed |
| `INVALID_QUERY_PARAMETER` | 422 | invalid query value |
| `TASK_NOT_FOUND` | 404 | task id not found |
| `TASK_ALREADY_CLAIMED` | 409 | task is running or claimed |
| `TASK_IN_TERMINAL_STATE` | 409 | task is completed, failed, or cancelled |
| `TASK_SUSPENDED` | 409 | task is suspended for requested operation |
| `TASK_NOT_IN_EXPECTED_STATE` | 409 | state precondition failed |
| `INTERNAL_ERROR` | 500 | unexpected server-side failure |

### CLI failure classes

- database connection failures
- invalid JSON payload or invalid flags
- invalid queue/status filters
- configuration validation failures

Use `--json` for machine-readable CLI errors.

## Related Docs

- [REST API Guide](../guides/rest-api.md)
- [CLI Reference](cli-reference.md)
