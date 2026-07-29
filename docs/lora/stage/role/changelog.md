# 角色连续性变更记录

## 2026-07-29 — 阶段 5：Work Responsibility 与 Commitment

### Project-owned Work 责任

- v2 Work head 增加独立于既有 v1 object body 的 `responsible_role_id` 关系；普通 Project
  View Work 内容仍复用原模型，Role 责任则由 Relay 签名的同一 Work projection 携带。
- `set_work_responsibility` 只允许 Community owner 或携带当前 Leader Assignment fence
  的 admin 执行；目标必须是 active Role。有 active Commitment 时不能单独改派或清除
  责任，Role 仍被 Work 引用时也不能停用或删除。
- Work responsibility 的改变与 Work object revision、全局 project revision、旧 head
  retirement、receipt 和 v2 meta head 在同一事务中推进，不产生第二份旁路状态。

### Assignment-owned Commitment

- 增加 first-class `WorkCommitment` 生命周期实体，固定记录 Work、Assignment、Member、
  接受者、开始时间、终止者、终止时间与原因。Member 归因在创建后不可改写，继任者只能
  创建自己的新 Commitment。
- 增加 `accept_work`、`end_commitment` 和原子 `replace_commitment` command。只有
  Work responsible Role 的当前 assignee 能接受；释放与替换必须携带该 Member 的精确
  active Assignment fence，并以 observed Commitment ID 防止覆盖并发状态。
- Assignment 结束会在同一 project revision 中结束其全部 active Commitment，但不修改
  Work status 或 responsible Role。Work 进入 completed/cancelled 或被删除时，也会在
  普通对象事务内以 `work_closed` 结束 active Commitment。
- additive migration `0028_project_work_commitments.sql` 补齐 immutable
  `member_pubkey`、entity revision、latest change 和更新时间，并用 deferred constraint
  校验 Assignment/Member/签名者一致、active Commitment 只指向非终态 Work。materialized
  active Commitment count、canonical row、signed entity head 和 meta count 同事务提交。

### 同一 verified state 的 Agent 与 Human 入口

- 共享 `VerifiedRoleBriefSnapshot` 现在验证 Work responsibility、Commitment、
  Assignment 和 meta count 的一致性，并把当前 Role 的非终态 Work 派生为
  `committed` 或 `waiting_for_continuation`。JSON、共享 Markdown renderer、ACP 动态
  Role Brief 与 Desktop 继续来自同一个 assembler。
- CLI 增加 `buzz roles work assign|unassign|accept|release|recommit`；managed Agent
  继续在签名前刷新完整 verified snapshot，并自动携带当前 Assignment fence。
- Desktop Role Inspector 展示 responsible Work 与接续状态；Work Inspector 允许 Human
  owner/Leader 分配责任，允许当前 assignee 接受或释放 Work。React 只提交 revision-fenced
  intent，Tauri/Relay 执行最终校验并在冲突时要求 Human 查看新状态。

### 验证与范围

- 纯领域测试覆盖跨 Role 接受拒绝、active Commitment 改派拒绝、Assignment 结束不改变
  Work、等待接续，以及 recommit 保留前任归因；SDK 测试覆盖责任 projection、
  Commitment verified snapshot 和 Role Brief 派生。
- migration/schema 静态门禁、workspace all-targets check、Desktop Tauri all-targets
  check、TypeScript 与 Biome 检查通过；Desktop E2E 覆盖 verified Commitment 展示和
  owner 的 revision-fenced Work 分配。
- 本阶段不实现结构化 Checkpoint、成员撰写的完整 Handoff、Role 历史分页或完整 Role
  Brief 时间线；这些属于阶段 6。

## 2026-07-29 — 阶段 4：Managed Agent 绑定与最小 Role Brief

### 共享 verified Role Brief

- 在 `buzz-sdk` 增加唯一的 `VerifiedRoleBriefSnapshot` 组装边界。CLI、ACP 和 Desktop
  先使用既有 v2 parser 验证 Relay 签名，再由共享组装器统一校验 project/generation/
  revision、meta counts、membership pointer、Project View 关系、Role/Assignment 唯一性
  以及 Leader 与 Community `admin` 的一致性；任一部分不一致都不会产出 Brief。
- 增加 canonical `RoleBrief` JSON DTO 和共享 Markdown renderer，区分
  `candidate` 与 `assigned`。最小 Brief 包含 Project Profile、全部 active Goal、Role
  定义与 level、当前 Assignment fence、与 Role 直接相关的 Issue/Work 切片，以及
  meta、membership、对象和实体各自的 signed source reference。
- Role Brief 仍是从当前投影即时派生的 read model，不新增表、不保存 `role.md`，也不成为
  第二份规范状态。相同 verified snapshot 供 JSON、CLI Markdown、ACP prompt 和 Desktop
  展示共同使用。

### Managed Agent 动态绑定与写入 fencing

- ACP 在 Relay 连接后的启动阶段解析一次 Role 状态，并在每个 channel turn 和 heartbeat
  创建 session 前重新读取 NIP-11 Relay identity、meta、完整 v2 heads 和 meta 精确指向的
  membership snapshot。读取使用前后 meta bracket；Brief 不能验证或超时时只注入
  `State: unavailable`，不缓存、不回退到旧 Assignment。
- 最新 Brief 作为动态 `[Role Brief]` user-context section 注入。它不进入长期
  `systemPrompt`；slash command 仍保持首 block，Brief 紧随其后。ACP 同时发出
  `role_context_resolved` observer frame，记录状态、Assignment、project revision、
  projection generation 和失败类别。
- 所有 managed Agent 的首次启动、lazy wake、slot refill、panic recovery 和 respawn
  都由 harness 强制携带 `BUZZ_MANAGED_AGENT=1`；persona 和父进程环境不能覆盖。该标记也
  传入 developer MCP，使 Agent 通过 shell 或 MCP 调用 `buzz` 时进入同一 managed 模式。
- managed CLI 在每次 Role 或 Project View v2 写入签名前重新读取 verified snapshot。
  已分配 Agent 自动携带当前 `acting_assignment_id`；显式提供的旧 fence 会在本地拒绝，
  Relay 继续执行最终 Assignment fencing。未分配 Agent 可以读取、请求或处理属于自己的
  Proposal，但不能进行普通 Project View 角色身份写入。
- `buzz project-view` 补齐 schema-v2 read/create/update/delete，使 Agent 与 Desktop
  修改同一份 Project View；v1 路径保持兼容。v2 写入后的 receipt 使用完整 verified
  snapshot 确认，不把数据库或未验证事件作为客户端事实源。

### CLI 与 Desktop 展示

- 增加 `buzz roles brief [--member <pubkey>] [--markdown]`。默认输出 canonical JSON，
  `--markdown` 输出与 ACP prompt 相同的共享 renderer 结果。
- Desktop Tauri 从同一个共享组装器为每条 active Assignment 生成 Brief；React 只消费
  该 verified DTO。Role Inspector 展示 Project 摘要、Goals、相关 Issue/Work 数量、
  project revision 和 projection generation，并继续与当前任期、Proposal、Handoff
  共用同一 v2 snapshot。

### 验证

- `buzz-sdk` 完整测试 `246/246`、`buzz-cli` `259/259`、`buzz-acp` library
  `600/600` 和 pool lifecycle `9/9` 通过。
- Desktop 单元测试 `3507/3507`、Biome/file-size/px-text/pubkey guards、TypeScript
  production build 和 E2E build 均通过。
- Project View Playwright smoke `24/24` 通过；新增断言覆盖 Inspector 的 verified
  Role Brief。Desktop Tauri Project View 定向测试 `12/12` 通过。
- `buzz-sdk`、`buzz-cli`、`buzz-acp` 和 Desktop Tauri 的
  `clippy --all-targets -D warnings` 均通过；workspace Rust fmt 与差异 whitespace
  检查通过。

### 范围边界

- 本阶段完成的是 Assignment 身份绑定和最小上下文，不包含 Work
  `responsible_role_id`、Commitment 或 `waiting-for-continuation`；这些属于阶段 5。
- 结构化 Checkpoint、成员撰写的完整 Handoff、Role 时间线和完整 Role Brief 属于阶段
  6；runtime lease、epoch、可信 supervisor 和自动 `unrecoverable` 属于阶段 7。

## 2026-07-28 — 阶段 3：Desktop Human 治理

### v2 普通 Project View 对象写入

- 补齐 schema-v2 `initialize/create/update/delete` command 信封，复用既有对象 reducer，
  但显式携带 `schema_version = 2`、`expected_project_revision` 和可选
  `acting_assignment_id`。v1 wire command 不会被当成 v2 接受。
- Relay 按 v2 command 的 closed shape 区分普通对象写入与 Role Continuity command；
  普通对象同样经过 Community Project lock、全局 project revision、幂等 receipt、
  canonical row、旧 head retirement、`40903` object/Role head 和 `40904` meta 的单事务
  提交。
- v2 新建 Role 默认是普通 `member` level；通用 Role 更新保留既有 level，不能借对象
  patch 升降级。Role 仍有 active Assignment 时，通用停用和删除在服务端准备阶段拒绝，
  不依赖 Desktop 按钮守卫。
- Desktop native mutation 根据 NIP-11 capability 生成 v1 或 v2 command，并对返回的
  object/meta projection 做 schema-aware 签名与 source/revision 确认；revision conflict
  返回 typed result，不自动重放 Human intent。

### Desktop verified read 与 Role 治理

- Tauri `get_project_view` 支持 verified v2 snapshot：固定 NIP-11 Relay identity，校验
  meta、普通 object、Role/Proposal/Assignment/Handoff entity head、membership snapshot、
  counts、generation、revision 与 source pointer，再把组装后的 typed DTO 交给 React。
- `View` 的 Role 卡显示 `Leader/Role` level、当前承担者、vacant 与 inactive 状态；Role
  Inspector 显示当前任期、open Proposal、历史任期和最小 Handoff，承担者名称与普通成员
  profile 共用同一解析结果。
- 增加 typed Tauri Role mutation，覆盖 request、offer、accept、reject、withdraw、
  authorize 和治理性 end。owner 可以治理全部 Role，active Leader 只能治理普通 Role；
  普通成员可请求 Role 或处理属于自己的 Proposal，assignee 不获得主动结束自己任期的入口。
- 指派/替换候选人只来自 verified Community membership；写入带当前 project revision。
  409 conflict 会使当前 Community 的 Project View query 失效并读取最新快照，旧表单动作
  不会在新 revision 上自动执行。
- 普通 Role 编辑器在有 active Assignment 时禁用 deactivate，Inspector 禁用通用 delete；
  即使绕过 UI，Relay/DB 仍执行相同硬约束。

### Community Members 设置与 Community 隔离

- v1 Community 保留原 invite、promote、demote、remove 行为；v2 Community 的 Settings
  fail closed，隐藏直接邀请与等级/删除 mutation，并统一引导到 `View` 的 Role 治理流程。
- v2 成员行显示当前 Role，不能从 Settings 直接制造没有 Leader Assignment 的 admin，
  也不能直接删除有任期的 Member；Relay 继续作为最终权限边界。
- Role continuity 不增加 module-level Community cache，沿用包含 Community identity 的
  Project View query key 和现有 key-based remount。切换 Community 后会重新读取对应
  snapshot，不复用上一项目的 Assignment。
- 将 Role continuity API、integrity error 与大型 Tauri 测试夹具拆为独立模块，所有
  Desktop 源文件继续满足 1000 行硬上限。

### 验证

- Desktop 单元测试 `3507/3507`、Biome/file-size/px-text/pubkey guards、TypeScript
  `--noEmit` 均通过。
- Project View Playwright smoke `24/24` 通过；新增场景覆盖 v2 Role 卡与 Inspector、
  owner 携 revision 发起 Offer、conflict 刷新且不重放、Settings 不直接改 admin，以及
  Community 切换后 assigned→vacant 不泄漏。
- Tauri Project View 定向测试 `12/12` 通过，覆盖 verified snapshot、capability 状态、
  typed mutation、签名投影确认、revision conflict 不重试和 Role command 输入边界。
- `buzz-project-view` v2 object command 测试、`buzz-sdk` v2 projection 测试与
  `buzz-db` PostgreSQL 纵向测试通过；实库测试验证 cutover 后普通 Goal 写入进入同一
  revision/projection 流，active Assignment Role 的通用停用不会提交。
- `buzz-project-view`、`buzz-sdk`、`buzz-db`、`buzz-relay` 和 Desktop Tauri 的
  `clippy --all-targets -D warnings` 均通过。

### 范围边界

- 本阶段交付 Human 对 Assignment 的完整首轮治理面，但尚未让 managed Agent Runtime
  自动解析和携带自己的 Assignment；Agent 仍通过阶段 2 的 `buzz roles` CLI 显式操作。
- Role Inspector 目前展示阶段 2 的最小 Handoff；Work Commitment、结构化 Checkpoint、
  完整 Role Brief 和可信 runtime 自动故障恢复分别留在后续阶段。
- 下一阶段是 managed Agent 绑定与最小 Role Brief：Runtime 在启动和每个 turn 前读取
  verified active Assignment，动态注入 Role 上下文，并让所有角色身份写入自动携带
  `acting_assignment_id`。

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
