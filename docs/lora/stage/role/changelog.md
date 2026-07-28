# 角色连续性变更记录

## 2026-07-28 — 阶段 2：Proposal、Assignment 与 CLI 纵向闭环

### Role Continuity 状态机

- 增加 closed `RoleCommand` 与纯领域 reducer，覆盖
  `request/offer/accept/reject/withdraw/expire/authorize` Proposal、人工结束
  Assignment、`request_replacement` 和 `report_unable_to_continue`。
- Proposal 创建时同时记录目标 Role 和候选 Member 的 active Assignment fence；最后一个
  必要确认到达时重新验证候选资格、授权者资格和两段 fence。任一条件变化都会拒绝整个
  command，不会先消费 Proposal 再留下半套状态。
- Assignment 激活或替换是一次 project revision 内的 compound transition。候选者原有
  Role 与目标 Role 的旧任期会一起结束，新任期只创建一次，并为每条被替换任期生成最小
  Handoff。
- 固定治理边界：assignee 不能主动结束自己的 Assignment；owner 可以治理所有层级；
  active Leader 只能治理普通 Role，并且必须携带与 signer 完全一致的当前
  `acting_assignment_id`；verified human owner 可以结束自己名下 managed Agent 的普通
  Assignment。
- 普通 Role 对应 Community `member`，Leader Role 对应 Community `admin`。Assignment
  变化产生期望等级，由同一事务更新 canonical membership 和 NIP-43 snapshot。

### v2 原子协调器与 cutover

- 增加 additive migration `0027_project_role_assignment_state.sql` 并同步
  `schema/schema.sql`：补齐连续性实体 revision/change pointer、Assignment 的替换请求与
  unable report、最小 Handoff，以及 v2 meta 的实体计数和 membership snapshot pointer。
- 增加统一 v2 write transaction：在 Community Project lock 内完成 command receipt、
  canonical Proposal/Assignment/Handoff、Community 等级、NIP-43 snapshot、`40903`
  entity heads、`40904` meta 和旧 head retirement。签名投影在 commit 前会被重新解析并与
  canonical state 对照，任一步失败全部回滚。
- 增加显式 `buzz-admin project-view cutover-v2`。cutover 只接受 disabled、已初始化的
  v1 Community；现有非 owner admin 必须逐一显式映射到唯一 Leader Role 或显式降级，
  不能按名称猜测。操作绑定 hash-chain audit、稳定 idempotency hash，并原子提升 schema、
  project revision 和 projection generation。
- cutover 会把全部 v1 当前对象与 tombstone 重投影为 v2 head，而不是只迁移 Role；旧
  current heads 同事务退休。重复相同 operator idempotency key 返回原 receipt。
- `buzz-project-view-v2` 只在 feature enabled、Relay signer 一致、meta/membership/counts
  完整、全部 canonical current rows 都有同 generation 的 live projection head 时由
  NIP-11 宣告；残缺 cutover 或手工退休任一 head 都保持 fail closed。

### Relay、SDK 与 CLI

- Relay 按 Community 的 schema version 分派 v1 mutation 或 v2 Role command，并在 receipt
  lookup 前执行当前 membership、ban、feature、signer 与 Assignment fence 检查。已接受
  event 不能借重放绕过后续卸任或权限变化。
- SDK 增加 v2 command、Role continuity entity、普通 Project object、membership 和 meta
  的 typed builder/parser。parser 校验 kind、Relay signer、签名、closed content、精确
  tag 顺序、schema、project/generation coordinate 与 source pointer。
- 增加 `buzz roles list/get/current/proposals/request/offer/proposal/assignment`。CLI
  先从 NIP-11 固定 Relay identity，再验证 meta、全部 Role continuity heads 和 meta 指向
  的唯一 membership snapshot；Assignment、Role level 与 Community 等级不一致时拒绝
  展示快照。

### 验证

- 纯领域测试覆盖双旧任期原子替换、授权者在最终确认前失权不产生部分消费、assignee
  自卸任禁令与 replacement request、Leader 的精确 active Assignment fence。
- 13 项 Project View PostgreSQL 测试全部通过，包含显式 v1→v2 cutover、cutover
  idempotency、残缺 generation 启用拒绝、owner 首次指派、A→B 替换、唯一 active
  Assignment、最小 Handoff、自卸任拒绝和旧 Assignment 延迟命令拒绝。
- 真实进程 E2E 使用隔离 PostgreSQL/Redis、真实 Relay、`buzz-admin` 和 `buzz` CLI，完成
  v1 对象写入、停用、v2 cutover、NIP-11 capability 切换、Agent A 接受 Offer、自卸任
  拒绝和 A→B 替换；最终 CLI 读取到两条任期历史与一条 Handoff。
- 9 项 `buzz-project-view`、243 项 `buzz-sdk`、259 项 `buzz-cli`、6 项 Relay
  Project View、13 项 Project View 实库事务、6 项 migration/schema-drift 测试及相关
  crates 的 `clippy --all-targets -D warnings` 均通过。

### 范围边界

- cutover 是显式 operator 动作，不会自动迁移或启用任何现有 Community；阶段 2 只适合
  受控 CLI pilot。
- v2 普通 Project View 对象已有完整 cutover/read projection，但 v2
  `initialize/create/update/delete` 写入尚未开放；schema-v2 Community 会明确拒绝旧 v1
  mutation，避免被旧客户端误写。恢复普通对象写入是 Desktop 阶段开始前的后端前置项。
- 本阶段不包含 Desktop Human 治理、managed Agent 自动携带 Assignment、Work
  Commitment、完整 Checkpoint/Role Brief 或可信 runtime 自动故障恢复。

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
