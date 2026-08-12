# Project Document Stage 7 单机 hardening / recovery runbook

本 runbook 只适用于当前未发布、单机开发阶段。它证明本地实现具备可重复的 signer rotation、
backup / restore、容量和安全证据；不授权部署、默认启用、在线无停机 rotation或 broad rollout。

## 交付边界

Stage 7 增加 migration `0035_project_document_reproject.sql`。新 signer 的 revision、head和 reset
meta event先写入 `project_document_reproject_events`，不进入普通 `events` 表，因此 Nostr point
query、history、REQ、`/query`和 event lookup 都看不到未完成 generation。

激活只允许在 `project_document_enabled=false` 时执行，并在一个 Community独占事务中：

1. 固定 source signer / generation、catalog revision、Document / revision counts；
2. 验证 staging 对每条 canonical revision、每个 current head和唯一 reset meta 的完整覆盖；
3. 把 staged event加入 `events`；
4. 退役 source generation 的 live Document projection；
5. 只更新 revision/head/meta materialization pointer和 signer generation，不改任何 business字段；
6. 执行 canonical、全历史 generation / pointer和 cryptographic parity；
7. 标记 operation为 `activated`，清除 staging payload并提交审计。

事务失败时上述变化全部回滚。`staging` / `ready` operation按同一 target signer安全重跑；basis或
target不同会 fail closed，不会覆盖未完成 operation。

## Signer rotation

准备权限为 `0600` 的新 signer key file。Community必须先 disable：

```bash
buzz-admin project-document disable --community "${COMMUNITY_HOST:?}"

buzz-admin project-document reproject \
  --community "${COMMUNITY_HOST:?}" \
  --all-revisions \
  --relay-key-file "${NEW_RELAY_KEY_FILE:?}" \
  --expected-pubkey "${NEW_RELAY_PUBKEY:?}"

buzz-admin project-document verify \
  --community "${COMMUNITY_HOST:?}" \
  --expected-pubkey "${NEW_RELAY_PUBKEY:?}"
```

`status` 必须显示：

- `enabled=false`；
- `projection_generation`恰好推进 1；
- `projection_pubkey`为新 signer；
- `meta_parity=true`；
- `orphan_projection_count=0`；
- `pointer_mismatch_count=0`；
- `reproject.state=activated`。

随后用新 signer启动 Relay，再执行 checked enable。不要用旧 signer启动已旋转 Community；它会因
signer mismatch保持 unavailable。当前不支持在线无停机 rotation。

## Backup / restore

在 capability disabled、reproject verify通过后获取 PostgreSQL一致备份。恢复到全新的 scratch
database，并在启动 Relay前执行：

```bash
DATABASE_URL="${RESTORED_DATABASE_URL:?}" \
  buzz-admin project-document verify \
    --community "${COMMUNITY_HOST:?}" \
    --expected-pubkey "${NEW_RELAY_PUBKEY:?}"
```

至少比较 source / restored 的 canonical revision digest、catalog / active / revision counts、
generation、signer和完整 projection parity。digest只用于 synthetic scratch验收；真实 Secret
incident记录不得包含正文或正文 hash。

真实本地演练入口：

```bash
just project-document-stage7-recovery
```

该脚本使用真实 Relay、Redis、PostgreSQL、CLI和 signed Document events执行：Secret incident
disable / reviewed re-enable、disable → signer rotation → full-history reproject → verify、`pg_dump` →
新 database restore → verify、新 signer Relay read，以及低配额 bounded abuse burst。所有 database、
key file、backup和 Cargo incremental目录由 EXIT trap精确清理。

## 单机容量

```bash
just project-document-stage7-capacity
```

默认数据集是 100,000 条 revision：50,000 条同一 hot Document历史和 50,000 个宽 catalog
Document，正文为 256–1,024 bytes。工具先用 1,000 条同形数据估算实际 database增长，增加 50%
margin并保留至少 2 GiB剩余空间；熔断不通过就不会创建正式数据集。

报告写入 `test-results/stage7-capacity/<run>/`：

- `capacity-report.json`：body / table / index / event partition增长、seed时间、disk preflight；
- `history-probe.json`：closed keyset page、`limit=50`、`EXPLAIN ANALYZE`、单页耗时和 RSS；
- `capacity-report.md`：便于人工复核的摘要。

PostgreSQL可合法选择 `project_document_revisions_pkey`的 backward Index Scan或
`idx_project_document_revisions_history`；两者都满足 exact `(community_id, document_id,
document_revision)` keyset。验收拒绝 `project_document_revisions` Seq Scan。工具还强制持有一次
250 ms Community writer lock并测量 shared reader wait；这只是当前粗锁成本，不模拟分布式生产负载。
没有真实 contention证据时保持 Community lock，不提前引入 per-document lock。

百万 revision只通过显式 `STAGE7_REVISION_COUNT=1000000`运行，并仍受 pilot disk fuse控制；未运行
或熔断不影响 Stage 7完成。

## Retention / compliance待决

当前 Document delete是 bodyless tombstone，历史 revision、projection、备份和已下载副本继续存在。
Stage 7不实现 hard delete、privacy scrub或 retention scheduler。首次部署前必须由明确的数据 owner、
security / legal owner和 operator决定：

- canonical history、soft-retired projection、audit和backup各自保留期；
- legal hold / export / deletion request的责任边界；
- backup过期、副本和客户端 cache处置；
- 必须擦除数据的独立 scrub协议、审计和恢复语义。

在这些政策形成前，不得把 tombstone描述为合规擦除，也不得为上线临时绕过 append-only guard。

## Adapter观察标准

Stage 7不交付 Repository / MCP / Skill / Plugin Adapter。首次部署后只有同时出现以下证据才提出
Adapter proposal：

- 同一种外部资源需要重复、显式地按 Guide操作；
- 手工步骤产生可量化失败或明显时延；
- 外部事实源具有稳定身份、权限、审计和撤销接口；
- proposal不把 Secret写入 Document，不让 Context Reference自动执行，也不绕过现有 approval / ACL。

没有真实 usage evidence时维持 `Document coordinate → explicit fetch → existing tools`，不以测试 fixture
数量代替产品需求。

## 完整本地 gate

```bash
just project-document-stage7-test
```

该 gate顺序运行 migration、recovery / security、100k capacity，并在最终代码上各重跑一次 Stage 5和
Stage 6 canary。首次真实部署仍需另立 deployment / rollout gate。
