# How to Rotate Config by Profile

## Goal

Apply environment-specific configuration safely using profile overlays.

## When to Use

Use this when promoting settings across dev, staging, and production without code changes.

## Prerequisites

- profile files committed or provisioned (`config.<profile>.toml`)
- access to runtime environment variables

## Steps

1. Create base and profile files:

   - `config.toml`
   - `config.staging.toml`
   - `config.production.toml`

2. Select a profile at runtime:

   ```sh
   IRON_DEFER_PROFILE=staging iron-defer serve
   ```

3. Override sensitive values with environment variables:

   ```sh
   DATABASE_URL=postgres://... \
   IRON_DEFER_PROFILE=production \
   iron-defer serve
   ```

4. Validate resolved configuration:

   ```sh
   iron-defer config validate --json
   ```

## Verification

- resolved config reflects selected profile and env overrides
- service starts and readiness checks pass under target profile

## Troubleshooting

- If wrong values load, check `IRON_DEFER_PROFILE` and config file paths.
- If startup fails, validate merged config and environment variable names.
