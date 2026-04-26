#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-http://localhost:8080}"
MAX_WAIT=60
POLL_INTERVAL=2

echo "=== iron-defer smoke test ==="
echo "Target: $BASE_URL"
echo ""

# --- Wait for health ---
echo -n "Waiting for /health to return 200..."
elapsed=0
while true; do
    if curl -sf "$BASE_URL/health" > /dev/null 2>&1; then
        echo " OK (${elapsed}s)"
        break
    fi
    elapsed=$((elapsed + POLL_INTERVAL))
    if [ "$elapsed" -ge "$MAX_WAIT" ]; then
        echo " TIMEOUT after ${MAX_WAIT}s"
        echo ""
        echo "FAIL: /health did not return 200 within ${MAX_WAIT}s"
        echo "Check container logs: docker compose logs iron-defer"
        exit 1
    fi
    sleep "$POLL_INTERVAL"
done

# --- Readiness ---
echo -n "Checking /health/ready..."
READY_STATUS=$(curl -sf -o /dev/null -w "%{http_code}" "$BASE_URL/health/ready" 2>/dev/null || echo "000")
if [ "$READY_STATUS" = "200" ]; then
    echo " OK"
else
    echo " FAIL (HTTP $READY_STATUS)"
    echo "FAIL: /health/ready returned $READY_STATUS, expected 200"
    exit 1
fi

# --- Submit a task ---
echo -n "Submitting test task via POST /tasks..."
SUBMIT_RESPONSE=$(curl -sf -X POST "$BASE_URL/tasks" \
    -H "Content-Type: application/json" \
    -d '{"queue":"smoke-test","kind":"smoke-test","payload":{"test":true}}' 2>/dev/null)
# Robust parsing: look for "id":"..." at the start of the object or after a comma, 
# ensuring we don't pick up nested IDs in the payload.
TASK_ID=$(echo "$SUBMIT_RESPONSE" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
if [ -z "$TASK_ID" ]; then
    echo " FAIL"
    echo "FAIL: POST /tasks did not return a task ID"
    echo "Response: $SUBMIT_RESPONSE"
    exit 1
fi
echo " OK (id=$TASK_ID)"

# --- Retrieve the task ---
echo -n "Retrieving task via GET /tasks/$TASK_ID..."
GET_STATUS=$(curl -sf -o /dev/null -w "%{http_code}" "$BASE_URL/tasks/$TASK_ID" 2>/dev/null || echo "000")
if [ "$GET_STATUS" = "200" ]; then
    echo " OK"
else
    echo " FAIL (HTTP $GET_STATUS)"
    echo "FAIL: GET /tasks/$TASK_ID returned $GET_STATUS, expected 200"
    exit 1
fi

# --- Metrics ---
echo -n "Checking /metrics..."
METRICS_STATUS=$(curl -sf -o /dev/null -w "%{http_code}" "$BASE_URL/metrics" 2>/dev/null || echo "000")
if [ "$METRICS_STATUS" = "200" ]; then
    echo " OK"
else
    echo " FAIL (HTTP $METRICS_STATUS)"
    echo "FAIL: /metrics returned $METRICS_STATUS, expected 200"
    exit 1
fi

echo ""
echo "=== All smoke tests passed ==="
exit 0
