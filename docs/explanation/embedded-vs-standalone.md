# Embedded vs Standalone

## Question

Should iron-defer run inside the application process or as a separate service?

## Short Answer

Both models are supported; choose based on coupling and operational boundaries.

- Embedded model integrates directly into your app process and release lifecycle
- Standalone model (`iron-defer serve`) provides independent deployment and scaling

## Tradeoffs

- Embedded: fewer moving parts, but stronger resource and release coupling.
- Standalone: clearer isolation and scaling control, but adds service management overhead.

## Related Docs

- [Tutorial: Embed in Axum](../tutorials/embed-in-axum.md)
- [Tutorial: Operate Standalone](../tutorials/operate-standalone.md)
