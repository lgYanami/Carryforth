-- Fresh-install convenience for Buzz-managed PostgreSQL only.
--
-- Existing volumes and external managed databases do not re-run initdb. Their
-- operator must install the extension before applying semantic migrations and
-- verify it with `buzz-admin semantic preflight`.
CREATE EXTENSION IF NOT EXISTS vector;
