# 角色连续性变更记录

## 2026-07-28 — 阶段 1：Membership 一致性内核

### v2 边界与变更来源

- 增加独立的 `buzz-project-view::v2` 领域入口，固定 closed
  `SchemaVersion`、`RoleLevel`、admin Role 创建与生命周期的 owner-only
  治理规则，以及 Nostr、NIP-98、operator、system 四类 typed change source。
- NIP-98 request hash、operator/system idempotency-key hash 和 change ID 使用不同的
  SHA-256 domain separator，并以固定测试向量锁定算法；Nostr change 直接复用 source
  event ID。
- 增加 additive migration `0026_project_role_continuity.sql` 并同步
  `schema/schema.sql`。所有既有与新建 Community 仍默认
  `project_view_schema_version = 1`，本阶段不宣告 `buzz-project-view-v2`。
- `project_view_changes` 对 source event、audit sequence、idempotency hash 和 project
  revision 建立 Community-scoped 唯一性；operator/system source 必须引用同一
  Community 的 hash-chain audit row。Proposal、Assignment、Commitment、Checkpoint、
  Handoff 与引用表只建立规范存储和约束骨架，尚未开放协议写入。
- migration 26 使用兼容触发器镜像 v1 Relay 仍只写入的 `last_event_id`，保证 additive
  migration 后旧 v1 写入形态仍可运行。迁移测试实际覆盖 fresh、0024→0026、
  ledger-less schema、并发 migrator、旧 v1 INSERT/UPDATE 形态和 schema drift。

### Community / Project 共同一致性锁

- 将原 Project View advisory lock 提取为唯一的 Community/Project lock primitive。
  Project View writer、全部 `relay_members` mutation、持久 ban/unban、timeout、
  managed Agent owner materialization、NIP-43 snapshot publisher 与 reconcile
  使用同一 lock key。
- 固定锁顺序为 Community/Project lock 在前、NIP-43 replacement lock 在后。并发测试
  证明 snapshot publisher 会等待未提交的 membership transaction，并在取得锁后读取
  最新完整成员集合。
- schema-version 查询只依赖已持有的 advisory lock，不额外获取 Community 行级
  `FOR UPDATE`，避免阻塞 event 外键所需的 `KEY SHARE`。

### v1 兼容与 v2 fail-closed

- v1 Community 继续保留既有 add/remove/change、invite、bootstrap、owner transfer、
  ban/unban 和 allowlist backfill 语义；新内核只改变事务组织和锁，不把 v2 Role
  规则提前施加给 v1。
- v2 Community 禁止通用入口直接创建 `admin/owner`、降级 active Leader、移除有 active
  Assignment 的 Member、移除仍拥有 active managed-Agent Assignment 的 Human、
  self-leave 绕过、admin invite、managed Agent 成为 Community owner，以及在转移
  ownership 前 ban 当前 owner。
- 单 Community 与批量 v1 Project View 开关都会拒绝启用 v2 Community，避免出现“v2
  不宣告 capability、却被旧开关标成 enabled”的误导状态。
- Assignment 激活后，partial unique index 保证一个 Role 和一个 Member 各自最多一条
  active Assignment；deferred constraint 在 commit 时验证 Role active/level、
  Community 等级、managed Agent owner 资格、persistent ban、Work responsibility
  和 Commitment 关系。约束触发器会同时验证跨 Community UPDATE 的旧、新两侧；直接
  SQL 提权、删/挪成员或 ban owner 同样会被拒绝。
- known managed Agent 即使已经 materialize 为 direct member，也继续依赖 verified
  owner 的有效 Community membership，且 Agent 与 owner 都必须未被 ban；direct row
  不再成为绕过入口，managed Agent 也不能被递归地用作另一个 Agent 的治理 owner。
  Relay 用一条 SQL 快照同时解析 direct membership、managed owner 和双方 ban 状态，
  避免授权读取拼接 membership/ban 变化前后的两代状态。
- owner 变更所需的“旧 owner 按 active Assignment 推导为 `admin/member`”已形成事务内
  primitive。由于完整 v2 source/audit/meta/NIP-43 projection coordinator 尚未开放，
  本阶段对会成功改变 v2 membership 的通用入口统一返回
  `unavailable:project_view_v2:membership_coordinator`，而不是提交不增加 project
  revision 的半套状态。配置中的 owner 与当前 v2 owner 相同时，startup bootstrap
  保持幂等 no-op。

### 验证

- 纯领域测试覆盖未知 schema 拒绝、change ID 固定向量、operator/system domain
  separation、非法 audit sequence，以及 admin Role 创建、升降级、停用和重新启用的
  owner-only 规则。
- PostgreSQL 测试覆盖 v2 直接提权/删除/ban 旁路、active Assignment membership
  守卫、managed Agent owner 资格、owner ban、owner role 推导、v1 capability 隔离和
  v1 writer fail-closed。
- Project View 16 项实库事务测试、relay-members 10 项实库测试、moderation 4 项实库
  测试、user 8 项实库测试，以及 migration/schema-drift gate 均通过。

### 范围边界

- 本阶段不提供 v2 cutover、不宣告 v2 capability，也不开放 Proposal、Assignment
  command、v2 projection、CLI 或 Desktop。
- 成功的 v2 membership mutation 必须等待后续 coordinator 将 typed change source、
  audit receipt、project revision、canonical membership、NIP-43 snapshot 与 v2 meta
  放入同一事务；在此之前保持 fail closed。
