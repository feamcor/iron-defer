# How to Use Regions

## Goal

Route tasks to region-specific workers while preserving fallback behavior for unpinned work.

## When to Use

Use this when data residency, latency, or operational ownership requires regional execution.

## Prerequisites

- worker region labels planned
- optional producer allowlist requirements defined

## Steps

1. Configure producer allowlist (optional):

   ```toml
   [producer]
   allowed_regions = ["us-east-1", "eu-west-1"]
   ```

2. Configure worker region:

   ```toml
   [worker]
   region = "us-east-1"
   ```

3. Submit region-pinned tasks:

   ```json
   {
     "queue": "default",
     "kind": "send_email",
     "payload": {"to": "u@example.com"},
     "region": "us-east-1"
   }
   ```

4. Validate queue distribution:

   ```sh
   curl -sS "http://localhost:8080/queues?byRegion=true"
   ```

## Verification

- region-pinned tasks are claimed by workers in matching region
- unpinned tasks are still processed by workers without region labels

## Troubleshooting

- Rejected region: check `producer.allowed_regions`.
- Region backlog with no progress: verify workers are running in that region.
