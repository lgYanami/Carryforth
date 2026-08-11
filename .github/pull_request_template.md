## Summary

Describe the outcome and why it is needed.

## Scope

- Components changed:
- Explicit non-goals:
- Related issue/design:

## Validation

List the checks you ran and their results. If a relevant check was not run,
explain why.

## Risk and data safety

- [ ] This change does not delete or reset user data, app-data, keyring state,
      Agent state, databases, or Docker volumes.
- [ ] Migration/integration tests, if any, use an explicit scratch database and
      fail closed against the main local development database.
- [ ] Authorization, networking, storage/wire compatibility, and rollback
      implications are described where relevant.
- [ ] No secret, private key, auth tag, internal URL, or personal information is
      included in code, fixtures, logs, screenshots, or documentation.

## Contributor checklist

- [ ] I activated the repository Hermit environment before running Git hooks.
- [ ] I preserved Apache-2.0 and applicable upstream attribution.
- [ ] I updated public documentation for user-visible behavior.
- [ ] New public APIs have documentation and production paths add no new
      `unwrap()`/`expect()` or `unsafe` code.
