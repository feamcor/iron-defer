# How to Create Idempotent Producers

## Goal

Make task submission retries safe by using stable idempotency keys.

## When to Use

Use this when producers may retry requests due to network failures or transient API errors.

## Prerequisites

- producer can generate deterministic business keys
- submission path supports `idempotencyKey` or `--idempotency-key`

## Steps

1. Define key scope and format (queue-scoped):

   - `order:<order-id>:invoice`
   - `user:<user-id>:welcome-email`

2. Submit with idempotency key.

   REST:

   ```json
   {
     "queue": "default",
     "kind": "send_invoice",
     "payload": {"orderId": "o-123"},
     "idempotencyKey": "order:o-123:invoice"
   }
   ```

   CLI:

   ```sh
   iron-defer submit \
     --queue default \
     --kind send_invoice \
     --payload '{"orderId":"o-123"}' \
     --idempotency-key 'order:o-123:invoice'
   ```

3. Retry with the same key on transient failures.

4. Set retention expectations using `worker.idempotency_key_retention`.

## Verification

- duplicate submissions with same key return existing task semantics
- downstream business side effects execute once for the business entity

## Troubleshooting

- If duplicates appear, confirm the producer reuses the exact key string.
- If key reuse is needed sooner, shorten retention with care.
