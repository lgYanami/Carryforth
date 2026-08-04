# 角色连续性实现设计

> 本文定义[角色连续性概念设计](./role-continuity.md)如何在 Buzz 中落地。
> 它覆盖领域对象、Project View v2 协议、PostgreSQL 规范状态、Community 等级同步、
> Relay 事务、CLI、Desktop、managed Agent 接入、故障恢复、测试、迁移与阶段交付计划。
>
> 本文是一份实现设计，不在本阶段提交 Rust、SQL 或客户端代码。

> 2026-08-03 局部覆盖：第 9.1 节中“普通 Role 沿用 Project View member 能力”的旧结论已被
> [Project Role 治理授权与 admin Role 创建修复设计](../bug/project-role-governance-authorization-and-admin-role-creation-fix-design.md)
> 收敛。普通 member 不再能维护 Role 定义；owner 可治理 admin/member，Active Leader 只可
> 治理 member。Assignment 仍不成为非 Role Project 对象的 ACL。

## 1. 文档目的

[项目定位与目标](../../project-positioning.md)已经确定：

> 连续性属于 Project，不属于任何单一 Agent。

[角色连续性概念设计](./role-continuity.md)进一步固定了以下概念：

- 一个 Buzz Community 等于一个 Project；
- Role 是 Project 长期持有的稳定责任位置；
- 一个 Role 同一时点最多由一个 Member 承担；
- 一个 Member 同一时点最多承担一个 Role；
- Leader Role 等同 Community `admin`，普通 Role 等同 Community `member`；
- Agent 不能自行结束自己的 Assignment；
- Runtime 可以重启或替换，但 Assignment 和项目状态不随 Runtime 消失；
- Work、Checkpoint、Handoff 和 Role Brief 共同构成可接续的角色局势。

本文以已经交付的
[Project View 后端实现设计](../project-view/backend-implementation-design.md)和
[NIP-PV v1](../../../nips/NIP-PV.md)为实现基线，不重新设计一套旁路基础设施。

本文继续回答：

1. 这些概念如何复用已经交付的 Project View；
2. Role、Assignment、Work、Checkpoint 和 Handoff 的规范状态分别存放在哪里；
3. Human 与 Agent 如何通过同一 Relay 协议读取和修改它们；
4. 如何让 Assignment 与 Community `owner/admin/member` 始终保持原子一致；
5. 如何阻止 Agent 通过卸任、离开 Community、停用 Role 或旧 Runtime 延迟命令绕过规则；
6. 如何逐阶段交付，并让每个阶段都有独立的可验收闭环。

## 2. 实现结论

首版采用以下总体方案。

### 2.1 作为 Project View v2 实现

角色连续性不是 Buzz 旁边的新服务，也不是一套与 Project View 并行的“角色记忆系统”。
它作为 **Project View v2** 落地：

```text
Project View v1
    Project Profile / Goal / Role / Plan / Stage
    Requirement / Issue / Work / Resource

Project View v2
    保留上述对象
    + Role Level
    + Work Responsibility
    + Proposal / Assignment / Commitment
    + Checkpoint / Handoff
    + 派生 Role Brief
```

选择 v2，而不是另建 Role Continuity revision，主要因为下列变化必须构成同一个一致性
边界：

- Assignment 激活、结束或替换；
- Role Level 与 Work Responsibility；
- Community `admin/member` 等级；
- Work Commitment；
- Relay 签名的当前态投影。

如果它们分别使用 Project View revision 和 Role Continuity revision，客户端可能读到
“Assignment 已替换，但 Community 仍是旧等级”或“Commitment 已结束，但 Work 仍指向
旧状态”的撕裂快照。Project View 已经具有 Community 级锁、项目 revision、规范表、
命令收据和 Relay 投影，v2 应直接扩展这条边界。

### 2.2 不把连续状态塞进 Role 的一段 Markdown

Role 本体只保存稳定定义：

- 名称；
- 目的；
- 职责；
- 边界；
- 等级；
- 是否 active。

以下内容使用独立、可关联、可追溯的规范实体：

- Proposal；
- Assignment；
- Work Responsibility；
- Work Commitment；
- Role Checkpoint；
- Role Handoff。

Role Brief 是从这些实体和 Project View 当前快照组装出的派生读取结果。它可以输出
JSON 或 Markdown，但 Markdown 不是事实源，也不存在一份需要 Agent 实时覆盖保存的
`role.md`。

### 2.3 复用现有 Nostr kind

Project View v2 继续使用：

| Kind | 用途 | 作者 |
|---:|---|---|
| `44300` | Project View 命令 | 当前 Community Member |
| `40903` | 对象或连续性实体的当前 head | Relay only |
| `40904` | Project View 当前 meta | Relay only |

版本由 content 中的 `schema_version: 2` 和 NIP-11 capability
`buzz-project-view-v2` 表达。kind 表示协议族，不表示具体 schema major。

不新增 Role Continuity 专用 HTTP endpoint。写入仍走 WebSocket `EVENT` 或
`POST /events`，读取仍走 Nostr `REQ`、`POST /query` 和实时订阅。

### 2.4 规范状态仍在 PostgreSQL

数据分为三层：

```text
authenticated change source
    成员签名 kind:44300 / NIP-43 event
    或有幂等键与 hash-chain audit 的 operator/system action

PostgreSQL canonical tables
    当前状态、历史任期、关系与数据库不变量

Relay 签名 kind:40903 / 40904 projection
    Human、Agent、CLI 和 Desktop 共同读取的可信 read model
```

客户端永远不直接查询数据库。数据库也不是给 Agent 拼 SQL 使用的接口。

### 2.5 Community 权限直接等同

首版不增加第二套 Project ACL：

```text
Role.level = admin   ⇒ Leader Assignment 的非 owner 承担者为 Community admin
Role.level = member  ⇒ 普通 Assignment 的非 owner 承担者为 Community member
非 owner Community admin ⇔ 恰有一个 active admin Role Assignment
```

Community owner 是唯一例外：它始终保持 `owner`，不会因 Role 变化被降级。
普通 Community member 可以仍是候选者或观察者，因此 Community member 不反向推出
它已经具有 active Assignment。

这也意味着 Leader 获得的是当前 Buzz `admin` 的完整既有能力，而不只是一个界面标签。
所有现有成员管理入口必须接入同一个一致性内核，不能绕开 Assignment。

### 2.6 Assignment 与 Member 身份分离

Assignment 绑定：

```text
Project + Role + Member pubkey + tenure
```

它不绑定：

- Agent owner 的公钥；
- Desktop 本地 Agent 记录；
- ACP session；
- OS 进程；
- Runtime start nonce；
- 某个模型或 Persona。

同一个 Agent Runtime 重启后继续使用同一个 Member pubkey 和 Assignment。成员被替换后，
旧 Assignment 永久结束，旧 Runtime 即使恢复也不能重新获得该任期。

### 2.7 Runtime 可用性最后实施

Runtime availability 与 Assignment 生命周期分别维护：

```text
Assignment: active | ended
Availability: available | recovering | unavailable
```

presence、断线或一段时间没有消息都不足以自动结束 Assignment。自动
`unrecoverable` 只在最后阶段、且仅对受可信 supervisor 监督的 managed Agent 实施。

## 3. 当前代码边界与 v2 变化

现有 Project View v1 已经提供可复用的完整基础：

| 现有能力 | v2 如何复用或扩展 |
|---|---|
| `buzz-project-view` 纯 reducer | 增加 v2 模型、命令和跨实体不变量 |
| Community 级 advisory lock | 所有 v2 命令和受影响的 membership 写入共用 |
| `expected_project_revision` CAS | v2 继续使用一个全局 project revision |
| command、canonical state、receipt、projection 同事务 | 扩展到 Assignment、Commitment 和 `relay_members` |
| `40903/40904` Relay 投影 | 扩展 projection union，不改变 kind |
| CLI / Tauri 验证 Relay signer | 增加 v2 entity parser 和 Role Brief assembler |
| Desktop `View` 页面 | 在现有 Roles 区域增加承担与连续性界面 |

当前 `ProjectRole` 只含描述字段，并明确不参与 Buzz 权限；当前 `ProjectWork` 也没有
responsible Role。v2 是一次有意的 major 语义升级，不能把新字段静默塞进 v1。

## 4. 总体架构

```text
Human Desktop                       Agent / ACP / buzz CLI
      │                                      │
      ├──────── signed kind:44300 v2 ────────┤
      └──── existing NIP-43 member command ──┘
                         │
                         ▼
                Buzz 通用 WS/HTTP ingest
                         │
            签名 / credential / ban / scope
                         │
                         ▼
              Project View v2 coordinator
                ├── 解析 v2 command / 拦截 membership mutation
                ├── 取得 Community Project lock
                ├── 事务内重验 actor / Assignment
                ├── 执行纯领域 reducer
                └── 构造所有 Relay projection
                         │
                         ▼
              同一个 PostgreSQL transaction
                ├── events：存在时保存成员 command
                ├── project_view_changes / audit receipt
                ├── project_view_state / objects
                ├── proposals / assignments
                ├── responsibilities / commitments
                ├── checkpoints / handoffs
                ├── relay_members
                ├── kind:40903 heads
                ├── kind:40904 meta
                └── NIP-43 membership snapshot
                         │
                       commit
                         │
                         ▼
               Redis / local live fan-out
                         │
             ┌───────────┴───────────┐
             ▼                       ▼
      verified Desktop View   verified CLI / Role Brief
```

任何一个 canonical row、membership 变化或 projection 写入失败，整个事务回滚。
提交后的 fan-out 失败不回滚数据库；客户端通过 meta revision 和重连快照恢复。

## 5. 代码位置与模块边界

### 5.1 `buzz-project-view`

不新增部署单元，也不必新增第二个领域 crate。建议保留 v1 API，并新增明确的 v2 模块：

```text
crates/buzz-project-view/src/
├── ...                     # 已有 v1 实现
└── v2/
    ├── mod.rs
    ├── model.rs
    ├── command.rs
    ├── state.rs
    ├── validation.rs
    ├── projection.rs
    └── role_brief.rs
```

职责：

- v2 的 closed wire types；
- Proposal、Assignment、Commitment、Checkpoint、Handoff 状态机；
- Role Level、Work Responsibility 和跨实体最终态校验；
- 稳定错误 code；
- 从验证后的投影组装 Role Brief；
- 不依赖 SQLx、Relay、Redis、Tauri 或 Runtime。

### 5.2 `buzz-core`

不增加新 kind，只扩充和验证现有 Project View classifier 的版本无关语义：

- `44300` 仍是 command；
- `40903/40904` 仍是 relay-only projection；
- `40903/40904` 仍具有 indexed `d` tag；
- 三类 event 仍由 Project View 的严格成员读取门禁保护；
- Workflow、NIP-09 和普通 replaceable 逻辑不能接管这些 event。

### 5.3 `buzz-sdk`

增加：

- v2 typed command builders；
- v2 object/entity/meta projection parser；
- exact tags、signer、Community、coordinate、revision 和 content/tag 一致性验证；
- v2 typed change source 与 membership snapshot reference 验证；
- v1 历史 command 的只读识别；
-共享 Role Brief assembler 的输入适配。

SDK 不允许调用方手写任意 operation JSON，也不能把 `acting_assignment_id` 只放在可伪造
tag 中。

### 5.4 `buzz-db`

扩展 `crates/buzz-db/src/project_view.rs`，并把成员写入重构为可在外层事务中使用的
`*_in_tx` primitives。

DB 层负责：

- Community Project lock；
- project revision CAS；
- canonical entity persistence；
- partial unique index 和 deferred constraint trigger；
- v1 command receipt 与 v2 unified change receipt；
- Assignment 与 `relay_members` 原子同步；
- Project View 与 NIP-43 projection 的事务内 event insert；
- v1→v2 preflight、cutover、reproject 和 integrity audit。

Relay handler 不直接拼 SQL。

### 5.5 `buzz-relay`

Relay 增加一个 v2 coordinator，但仍从现有 Project View handler 进入。

它负责：

- 按当前 Community schema version 解析 v1 或 v2；
- 通用安全检查后进行粗粒度成员预检；
- 在持有 Project lock 的事务内重新读取并验证权威权限；
- 生成 Relay 签名的全部 changed heads、meta 和 membership snapshot；
- 把稳定 domain error 映射到 WS/HTTP 回执；
- commit 后复用现有 fan-out；
- 按 Community 在 NIP-11 中只宣告一个 Project View major capability。

### 5.6 `buzz-cli`

保留：

```text
buzz project-view ...
```

它继续维护 Profile、Goal、Role 定义和其他 Project View 对象。

新增：

```text
buzz roles ...
```

它负责 Proposal、Assignment、Role Brief、Work Commitment、Checkpoint 和 Handoff。

### 5.7 Desktop、ACP 与 managed Agent

- Desktop React 只消费 Tauri 返回的 verified DTO；
- Tauri 与 CLI 共用 Rust SDK/parser/assembler；
- ACP 在启动和每次工作前从 Relay 解析 active Assignment；
- Role 或 Assignment 不写入本地 `ManagedAgentRecord`；
- Role Brief 每个 turn 动态注入，不写进长期固定的 system prompt 或大段环境变量。

## 6. Project View v2 领域模型

### 6.1 Role

v2 Role：

```text
Role
├── id
├── name
├── purpose
├── responsibilities[]
├── boundaries[]
├── level: admin | member
└── active
```

规则：

- 新建普通 Role 可以使用 `member`；
- 新建 `admin` Role、任何涉及 `admin` 的等级转换（`member → admin` 或
  `admin → member`），以及停用或重新启用 `admin` Role，都只能由 Community owner
  授权；
- `level` 不进入通用 Role patch，使用专门的 `set_role_level` 命令；
- `active` 不进入通用 Role patch，使用专门的 `deactivate_role` /
  `reactivate_role` 命令；
- 有 active Assignment 时，等级变化必须在同一事务同步承担者的 Community role；
- 已产生 Assignment 历史的 Role 不允许删除，只能治理性停用；
- 有未处理 responsible Work 时不能静默停用。

Role 允许空缺。一个空缺的 `admin` Role 不会凭空产生 Community admin。

### 6.2 Role Assignment Proposal

Proposal 表达“候选者同意”和“项目授权”尚未全部满足的过程：

```text
RoleAssignmentProposal
├── id
├── role_id
├── candidate_pubkey
├── proposal_type: request | offer
├── candidate_accepted_at?
├── authorized_by?
├── authorized_at?
├── expected_target_assignment_id?
├── expected_candidate_assignment_id?
├── expires_at
├── status
├── reason?
├── created_by / created_at
└── resolved_at?
```

状态：

```text
status = open
  ├── candidate_accepted_at 可以先被填充
  ├── authorized_by 可以先被填充
  └── 双方条件齐备后 → consumed

open → rejected
open → withdrawn
open → expired
```

`request_role` 由候选者发起，天然包含候选者同意，等待 owner 或有权 Leader 授权。
`offer_role` 由治理者发起，天然包含项目授权，等待候选者接受。

`authorized_by` 不是一张永久授权票。它记录谁明确授权了 Proposal 所描述的完整换岗，
包括结束目标 Role 原任期、结束候选者原任期和授予目标 Role。最后一个必要确认发生时，
Relay 必须重新验证该 authorizer 仍具有完成这组治理动作的权限；若其已失去 owner、
Leader、membership 或相应管理资格，Proposal 保持 open，但本次确认以明确冲突拒绝，
需要由当前有权主体重新授权或另建 Proposal。

最后一个必要确认命令在同一事务内创建 Assignment；不暴露一个任何成员都能调用的
通用 `activate_assignment`。若 Role 或候选者状态已经变化，该命令发生 revision
conflict，Proposal 不会被部分消费。

两个 expected Assignment ID 分别 fence：

- 目标 Role 当前是否已有承担者；
- 候选 Member 当前是否正在承担另一个 Role。

二者可以同时存在。此时项目授权必须明确同意完整换岗，最后确认在一个事务内结束两段
受影响的旧任期、处理各自 Commitment/Handoff，并创建候选者的新 Assignment。候选者
不能在接受时更改这两个 expected ID；因此接受一个已经由治理者完整授权的换岗，不等于
Agent 单方面卸任原 Role。

`expires_at` 由 Relay canonical time 校验。到期后任何 accept/authorize 都失败；
读取时即使尚未运行清理任务也必须报告 effective status `expired`。后续任一连续性写
事务可以幂等物化该终态，不能让后台清理是否及时影响授权结果。

### 6.3 Role Assignment

Assignment 是不可复活的任期记录：

```text
RoleAssignment
├── id
├── role_id
├── member_pubkey
├── started_at
├── started_by
├── source_proposal_id
├── status: active | ended
├── replacement_requested_at?
├── ended_at?
├── ended_by?
├── end_reason?
└── replaced_by_assignment_id?
```

Agent Assignment 的结束原因：

- `revoked`；
- `replaced`；
- `unrecoverable`；
- `membership_ended`；
- `role_deactivated`。

没有 `released`。成员提交 `request_replacement` 或
`report_unable_to_continue` 只更新请求/报告状态，不结束 Assignment。

Assignment 一旦 ended：

- 永远不能重新 active；
- 原 Role、Member、时间和归因不被覆盖；
- 旧 Assignment ID 不能用于新的角色动作；
- 继任者获得新的 Assignment ID。

### 6.4 Work Responsibility

v2 给 Work 增加一个显式关系：

```text
Work optionally responsible_role_id → active Role
```

规则：

- 一个 Work 最多一个 responsible Role；
- 一个 Role 可以负责多个 Work；
- Work 开始被某个 Member 接受执行前必须具有 responsible Role；
- 有 active Commitment 时不能单独改变 responsible Role；
- Assignment 替换不改变 responsible Role。

它是关系列，不埋在 Work 的自由文本 description 中。

### 6.5 Work Commitment

```text
WorkCommitment
├── id
├── work_id
├── assignment_id
├── member_pubkey
├── status: active | ended
├── started_at / started_by
├── ended_at? / ended_by?
└── end_reason?
```

active Commitment 必须满足：

```text
Commitment.assignment is active
Assignment.role_id = Work.responsible_role_id
Commitment.member_pubkey = Assignment.member_pubkey
```

一个 Work 同一时点最多一个 active Commitment。Assignment 结束时，其未完成
Commitment 原子进入明确终态，但 Work 本身不自动完成、取消或改写。

继任者必须创建自己的 Commitment；不能把前任的 Commitment 改成继任者所作。

### 6.6 Role Checkpoint

Checkpoint 是追加式结构化快照：

```text
RoleCheckpoint
├── id
├── role_id
├── assignment_id
├── based_on_project_revision
├── summary
├── current_focus[]
├── progress[]
├── blockers[]
├── risks[]
├── open_questions[]
├── next_steps[]
├── references[]
├── supersedes_checkpoint_id?
├── created_by
└── created_at
```

Checkpoint：

- 创建后不更新、不删除；
- 纠正通过新 Checkpoint 的 `supersedes_checkpoint_id` 表达；
- 必须由其引用的 active Assignment 承担者创建；治理者可以补充 Issue 或 Handoff，
  但不能冒充该任期形成 Checkpoint；
- 不能替代 Work、Issue 或其他 Project View 对象的当前事实；
- 不保存完整聊天、草稿或隐藏推理。

当前阻塞、风险和未决问题因此有两个来源：

1. Work、Issue 等规范对象的当前状态；
2. Checkpoint 对某一时点局势的结构化组织。

它们不作为一个不断覆盖的字段塞进 Role 行。

### 6.7 Role Handoff

Handoff 也是追加式记录：

```text
RoleHandoff
├── id
├── role_id
├── from_assignment_id
├── to_assignment_id?
├── checkpoint_id?
├── affected_commitment_ids[]
├── summary?
├── unresolved_items[]
├── references[]
├── cause: planned | revoked | unrecoverable | other
├── created_by
└── created_at
```

正式 replace 时系统总是生成一条最小 Handoff，记录 cutover 和受影响的 Work，即使旧
Agent 没有提交总结。成员提供的内容可以改善交接，但不能成为替换的前置条件。

### 6.8 Role Brief

Role Brief 不是表，也不是可修改对象：

```text
RoleBrief =
  Project Profile / Goal 的必要摘要
  + Role 定义与 level
  + 当前 Assignment 或候选 Proposal
  + responsible Work
  + active / waiting Commitment
  + 最新 Checkpoint
  + 当前 Work / Issue 中的阻塞和风险
  + 最近 Handoff
  + 相关 Resource / event 引用
  + Community 等级与治理边界
```

输出必须包含：

- `generated_at`；
- `project_revision`；
- `projection_generation`；
- Role 和 Assignment ID；
- 每段信息的 source reference。

共享 Rust assembler 输出 canonical DTO。JSON 用于程序，compact JSON 用于 CLI，
Markdown 只用于 Human 展示或 Agent prompt。

如果未来 Project View 增加 Decision 或更完整的 Context 类型，Brief 再引用它们。
当前实现不凭空新建一份 Decision 真相；已有决定只能通过现有 Project View
Resource、Issue 或 Nostr event reference 引用。

### 6.9 Runtime Availability

Availability 不放进 Role 或 Assignment 行，也不随每次 heartbeat 增加 project
revision。

后续阶段增加低频状态转换：

```text
available → recovering → available
                       └→ unavailable
```

heartbeat、lease 和重试计数属于 supervisor 运行数据；只有状态转换和最终治理结果形成
Project 可读的审计记录。

## 7. 核心不变量

v2 reducer、数据库约束和事务提交时校验必须共同保证：

1. 所有引用都属于同一个 Community / Project。
2. 一个 Role 同一时点最多一个 active Assignment。
3. 一个 Member 同一时点最多一个 active Assignment。
4. active Assignment 必须引用 active Role 和当前有效 Community Member。
5. 非 owner 的 active Assignment 承担者等级必须与 Role level 相同。
6. 非 owner Community admin 必须恰好具有一个 active admin Role Assignment。
7. Community owner 始终保持 `owner`。
8. managed Agent Assignment 绑定 Agent 自己的 pubkey。
9. Agent 不能直接结束自己的 active Assignment；v0 通用命令采用更保守的 self-end
   拒绝策略，见第 9.2 节。
10. active Leader 不能结束自己或另一个同级 Leader。
11. ended Assignment 不能恢复。
12. 一个 Work 最多一个 responsible Role。
13. 一个 Work 最多一个 active Commitment。
14. active Commitment 的 Assignment Role 必须等于 Work responsible Role。
15. active Commitment 存在时不能单独改变 Work responsible Role。
16. Assignment 结束不自动改变 Work status。
17. Checkpoint 和 Handoff 追加后不可覆盖或删除。
18. Role Brief 不成为第二份规范状态。
19. Runtime 停止或短暂离线不自动结束 Assignment。
20. 来自 ended Assignment 的角色动作必须被拒绝。

应用层校验用于产生清晰错误；partial unique index、CHECK、FK 和 deferred constraint
trigger 是防止旁路和未来回归的最后防线。

## 8. Project View v2 命令协议

### 8.1 通用信封

kind `44300` v2 content：

```json
{
  "schema_version": 2,
  "expected_project_revision": 42,
  "acting_assignment_id": "4a95c550-c365-49a7-8d7e-07ce2e9e92b8",
  "request": {
    "type": "append_checkpoint",
    "checkpoint_id": "d3521bdc-4578-4a4b-b617-614574c54dd6",
    "role_id": "54abc82f-0957-4d2d-b690-0d363ee746c8",
    "summary": "..."
  }
}
```

规则：

- `schema_version`、`expected_project_revision` 必填；
- `acting_assignment_id` 在 Role-bearing 和非 owner Leader 治理动作中必填；
- owner 治理、Human 或 managed Agent 的普通 Project View 编辑，以及候选 Proposal 操作
  可以为空；
- Community `owner/admin/member`（或 owner 仍合格的 verified managed Agent）是普通
  Project View 读写权限来源；Project Role 不建立第二套 Project ACL；
- 尚未分配的 managed Agent 不能执行 Role-bearing 或 Leader 行为，但可按 Community
  资格执行普通 Project View CRUD、`request_role` 和自己的 candidate/creator operation；
- `acting_assignment_id` 必须属于 event signer，且在事务内仍 active；
- 普通写入若显式携带 Assignment，Relay 仍严格验证，失败时不能静默删除字段并降级重试；
- operation、Role、Member 和 Assignment 只从签名 content 取得，不相信 display name；
- payload 使用 closed typed schema，未知字段、枚举和 operation 拒绝；
- 一个成功 command 只增加一次 project revision，即使它改变多个实体；
- event ID 是幂等键；当前安全门仍满足时，重放同一 event 返回原 receipt，不再次增加
  revision。

v2 延续 v1 的 exact protected tags。Assignment ID 放在签名 content 中；可以在 Relay
projection 上增加可验证的索引 tag，但 tag 不是授权事实源。

当前 membership、ban、capability 和 Assignment fencing 仍在 receipt lookup 之前执行。
过去被接受的 event 不能利用幂等 receipt 绕过后来发生的卸任、ban 或 owner 失效。

### 8.2 命令集合

v2 保留 v1 的 `initialize/create/update/delete`，用于九类 Project View 对象的普通业务
字段。Role 初始 `level` 在 create 时校验；后续 level、active Assignment、
responsible Role 等治理字段不能通过通用 patch 修改。以下是 v2 新增或收紧的领域命令。

#### Role 与 Proposal

- `set_role_level`；
- `deactivate_role`；
- `reactivate_role`；
- `request_role`；
- `offer_role`；
- `accept_proposal`；
- `reject_proposal`；
- `withdraw_proposal`；
- `expire_proposal`（任何 Member 只能在 Relay canonical time 已到期后调用）；
- `authorize_proposal`。

#### Assignment

- `end_assignment`；
- `end_member_membership`；
- `request_replacement`；
- `report_unable_to_continue`。

Assignment 的创建或替换由 Proposal 的最后一个必要确认原子触发，不提供通用 CRUD。

#### Work

- `set_work_responsibility`；
- `accept_work`；
- `end_commitment`；
- `replace_commitment`。

#### 连续状态

- `append_checkpoint`；
- `append_handoff_note`。

正式替换生成的系统 Handoff 由 Relay 事务产生，不依赖旧承担者另发命令。

#### Runtime 系统动作不属于成员命令

`report_runtime_transition`、`record_recovery_attempt` 和最终
`end_unrecoverable_assignment` 不加入 Community Member 可提交的 kind `44300`
operation allowlist。阶段 7 使用独立的内部 `ProjectSystemChange` typed entry：

- 入口只接受已经注册的可信 supervisor evidence 或 Relay scheduler；
- 调用与成员命令相同的 Project lock、reducer、约束和 projection commit；
- 使用 `project_view_changes.source_type = system`、稳定 change ID 和 hash-chain audit；
- 不能伪装成某个 Community Member，也不能绕过普通 Assignment 不变量。

### 8.3 不允许通用 batch

v2 仍不开放任意 JSON batch。需要多对象原子变化的业务动作使用明确命名的 command：

- Assignment 替换；
- Role 等级变化；
- Role 停用；
- Membership 结束；
- Commitment 接续。

这样可以为每种动作固定权限、不变量、changed heads 和错误语义。

## 9. 权限与 Assignment 控制

权威权限判断必须在取得 Community Project lock 后、同一事务内基于最新状态执行。
事务外 membership gate 只用于尽早拒绝，不能作为最终授权。

### 9.1 初版矩阵

下表中的结束权限针对 Agent Assignment；Human Assignment 的完整退出与争议治理不在
首版展开。表中的 Leader 能力还受第 9.2 节 actor/target 可识别性限制。

| 操作 | Community owner | active Leader/admin | 普通 Role | verified human owner |
|---|---:|---:|---:|---:|
| 创建/修改普通 Role 定义 | 是 | 是 | 否 | 否 |
| 创建 admin Role、任意涉及 admin 的等级变化、停用/启用 admin Role | 是 | 否 | 否 | 否 |
| 授权/替换普通 Role Assignment | 是 | 是 | 否 | 否 |
| 授权/替换 admin Role Assignment | 是 | 否 | 否 | 否 |
| 结束普通 Agent Assignment | 是 | 是 | 否 | 仅限自己拥有的 Agent |
| 结束 admin Agent Assignment | 是 | 否 | 否 | 仅限自己拥有的 Agent |
| 结束自己的 Assignment | 否 | 否 | 否 | 不适用 |
| 结束另一个同级 Leader | 是 | 否 | 否 | 仅限自己拥有的 managed Agent |
| 提交本 Role Checkpoint | 可按自己的 Assignment | 可按自己的 Assignment | 可按自己的 Assignment | 否 |
| 接受本 Role Work | 可按自己的 Assignment | 可按自己的 Assignment | 可按自己的 Assignment | 否 |

verified human owner 特例来自服务端验证的 managed Agent owner 关系，不来自 event
自报。

### 9.2 Human 与 Agent 的可识别边界

Buzz 可以可靠识别“已登记并验证 owner 的 managed Agent”，但不能只看一个 Nostr
pubkey 就证明另一端一定是 Human 或外部 Agent。

因此首版安全规则采用：

- Agent 不能 self-end active Assignment；
- 因 Human 自助卸任规则尚未设计，v0 wire protocol 暂时对所有 Member 拒绝 self-end。
  这是避免未知外部 Agent 利用身份不可识别性绕过规则的保守实现策略，不把“Human
  永远不能主动卸任”提升为概念结论；未来只能通过新的 Human 治理设计显式放宽；
- known managed Agent 额外受 owner 连续有效性和 Runtime fencing 约束；
- managed Agent Leader 不能结束不能确认是 managed Agent 的 active Assignment，
  以免把未知 Human 当成 Agent；
- 不能可靠识别的 target Assignment 由 Community owner 处理；
- 自动故障恢复只覆盖可信 managed Agent；
- 外部 CLI Agent 可以承担 Role，但不会仅因沉默被自动卸任。

这比依赖可伪造的 `member_type` 字段更保守。

## 10. Community Membership 一致性内核

### 10.1 等级映射

除 owner 外：

```text
激活 admin Assignment   → upsert/promote relay_members.role = admin
激活 member Assignment  → ensure relay_members.role = member
结束 admin Assignment   → relay_members.role = member
结束 member Assignment  → 保持 member
```

结束 Assignment 不默认删除 Community Membership。移除成员是另一个显式治理动作。

owner 承担任何 Role 时仍是 `owner`。

### 10.2 必须统一的现有入口

Project View v2 启用前，以下所有 `relay_members` 写入路径必须改为调用同一个事务内核：

- NIP-43 add member、remove member、change role；
- member self-leave；
- invite claim；
- `buzz-admin` add/remove/change；
- startup owner bootstrap；
- operator ownership transfer；
- 持久 Community ban / unban；
- managed Agent direct-member materialization；
- 任何未来的成员导入或恢复工具。

这个重构必须按 Community schema version 分支：v1 Community 保持现有成员语义；只有
v2 Community 启用下面的 Assignment 一致性约束。阶段 1 不能为了准备 v2 提前改变
仍在运行的 v1 Community。

v2 Community 中：

- 直接提升 `admin` 被拒绝，必须创建/激活 Leader Assignment；
- 直接降级 active Leader 被拒绝；
- 移除有 active Assignment 的 Member 被拒绝，调用者必须使用治理命令原子结束任期；
- active Assignment 的 self-leave 被拒绝；
- 一个 Human owner 仍有自己拥有的 managed Agent active Assignment 时，其 membership
  不能被单独移除；必须先原子结束这些 Assignment，或拒绝本次移除；
- v2 启用前签发的 admin invite 不能在启用后直接产生 admin；
- 停用或删除 Role 不能绕过 Assignment；
- owner transfer 必须按 Assignment 重新计算旧 owner 等级。

成功的 v2 membership mutation 即使没有改变 `40903` entity，也必须增加一次 project
revision、生成新的 NIP-43 membership snapshot，并更新 `40904` meta。存在 Nostr
event 时 source 可以引用原 NIP-43 command；invite、operator 等非 event 入口使用第
11.2 节的 typed change source，不要求把所有既有成员操作伪装成 kind `44300`。

### 10.3 Owner transfer

```text
new owner → owner

old owner:
  有 active admin Assignment → admin
  否则                       → member
```

owner transfer、Assignment 检查、两个 membership row 和 NIP-43 snapshot 必须处于同一
事务。新 owner 还必须持续满足 Human 治理根约束：

- 已登记的 managed Agent identity 不能成为 Community owner；
- transfer transaction 必须执行与 cutover preflight 相同的 owner eligibility 检查；
- Buzz 目前只能可靠排除“已知 managed Agent”，不能仅凭一个未知 pubkey 从密码学上证明
  对端是 Human；因此 transfer 仍要求原 owner 的显式治理授权，并把这一身份边界记入
  audit。

### 10.4 Managed Agent owner 依赖

Assignment 激活时可以为 owner-backed managed Agent materialize 一条 direct
`relay_members` row，以落实 `member/admin` 等级。

但 direct row 不能绕过 owner 依赖。对于已知 managed Agent，授权始终要求：

```text
Agent direct/effective membership
AND verified owner 仍是当前 Community Member
AND Agent 与 owner 均未被当前安全策略禁止
```

现有“direct member OR owner-backed member”短路逻辑必须改写。owner membership
结束事务必须同时结束或拒绝其 managed Agent 的 active Assignment；无论如何，相关
Agent 都不能凭 materialized row 继续访问或执行旧 Assignment。

### 10.5 Ban 与资格变化

会使 Member “禁止参与当前 Community”的持久 ban 也是 Assignment 资格变化，必须接入
同一个 Project lock 和治理事务：

- 被 ban 的 Member（Human 或 Agent）自身有 active Assignment 时，原子结束该
  Assignment；若调用者无权结束它，则拒绝整次 ban；
- verified owner 被禁止参与时，原子结束其 managed Agent active Assignment，或在
  无权执行这些结束动作时拒绝 ban；
- Community owner 在完成 ownership transfer 前不能被 ban；
- 相关非 owner admin 按最终 Assignment 状态降级；
- ended reason 使用 `membership_ended`，并保留 ban/source audit；
- unban 不恢复旧 Assignment，Member 只能重新经过 Proposal；
- Agent 不能通过请求 ban 自己间接卸任。

短期限流、连接 timeout、presence 消失和普通网络故障不属于这里的“禁止参与”，它们只
阻止当前操作，不结束 Assignment。

## 11. PostgreSQL 设计

建议 migration：

```text
migrations/0026_project_role_continuity.sql
```

并同步更新 `schema/schema.sql`。

### 11.1 现有表扩展

`communities`：

```text
project_view_schema_version SMALLINT NOT NULL DEFAULT 1
```

一个 Community 在同一时点只能处于 v1 或 v2。capability 只在 enabled、schema ready、
projection signer ready 且 preflight 通过时宣告。

`project_view_state`：

- 增加当前 wire schema version；
- v2 meta 的各 entity type head count；
- 以 `last_change_id` 和可选 `last_source_event_id` 取代“每次变化都必有成员 event”的
  假设；
- project revision 继续单调递增；
- projection generation 在 v1→v2 cutover 时增加。

`project_view_objects`：

- Role v2 body 增加 `level`；
- Work 增加 `responsible_role_id` 关系列；
- schema CHECK 允许已保留的 v1 history 与当前 v2 rows；
- Role level 和 Work relation 增加 Community-leading index；
- deferred validation 检查 target type、active 状态和 Commitment。

`project_view_mutations` 保留全部 v1 receipt，不把非 event 来源硬塞进
`event_id NOT NULL` 的旧模型。

### 11.2 新表

```text
project_view_changes
project_role_assignment_proposals
project_role_assignments
project_work_commitments
project_role_checkpoints
project_role_handoffs
project_role_continuity_references
```

`project_view_changes` 是 v2 的统一审计与幂等收据：

```text
change_id                  32-byte stable digest
source_type                nostr_event | nip98_request | operator | system
source_event_id?           有真实 Nostr source 时填写
source_request_hash?       NIP-98 / HTTP source
source_audit_entry_id?     operator / system hash-chain audit
idempotency_key_hash?
actor_pubkey?
acting_assignment_id?
operation
subject
project_revision
result
accepted_at
```

规则：

- `44300` 或 NIP-43 event 的 `change_id` 直接使用 event ID；
- invite claim 保存已验证 NIP-98 auth event ID 与 canonical request hash，并由二者确定
  `change_id`；
- operator/admin 非 event 操作必须提供一次性 idempotency key，并引用
  `buzz-audit` hash-chain entry；
- stage 7 system change 引用 supervisor evidence 与 system audit entry；
- `(community_id, change_id)` 幂等，
  `(community_id, project_revision)` 唯一；
- 相同 source 重放返回原 result，不再次增加 revision；
- v2 meta/head 使用 typed `source` union，至少包含 `change_id` 和
  `source_type`，而不是强制一个并不存在的 `source_event_id`。

每张表都以 `community_id` 为第一键，并按实体需要保存：

- stable UUID；
- entity revision；
- created/updated actor 与 canonical time；
- source change ID 和可选 source event ID；
- project revision；
- projection event ID；
- typed foreign keys。

Checkpoint/Handoff 的摘要、关注点、阻塞、风险、未决问题和下一步以有上限、closed
schema 的 JSONB body 保存；它们不是一整篇 Markdown。对 Project View object、
Assignment、Commitment 或 Nostr event 的引用进入
`project_role_continuity_references`，保留顺序并便于 FK 校验、反向查询和跨 Project
拒绝。渲染 Markdown 时再把 body 与引用组合起来。

历史 Assignment、Checkpoint 和 Handoff 不对 `relay_members` 使用级联删除 FK。
成员离开后历史归因必须继续存在。

阶段 7 另行增加 runtime lease/observation 表。heartbeat 表不进入 Project View
projection，也不参与每次 project revision。

### 11.3 唯一约束

至少包含：

```sql
CREATE UNIQUE INDEX ... ON project_role_assignments
  (community_id, role_id)
  WHERE ended_at IS NULL;

CREATE UNIQUE INDEX ... ON project_role_assignments
  (community_id, member_pubkey)
  WHERE ended_at IS NULL;

CREATE UNIQUE INDEX ... ON project_work_commitments
  (community_id, work_id)
  WHERE ended_at IS NULL;
```

可再增加 open Proposal `(community_id, role_id, candidate_pubkey)` 去重。

### 11.4 Deferred constraints

commit-time trigger 验证：

- Role/Work UUID 实际指向正确 `object_type`；
- active Assignment 的 Role 未 tombstone 且 active；
- active Assignment 的 Member 有效；
- 非 owner 的 admin Assignment 与 `relay_members.admin` 双向等价；
- Commitment 的 Assignment/Role/Work 三者一致；
- ended Assignment 没有 active Commitment；
- active Commitment 存在时 Work responsibility 未改变；
- Role 有 active Assignment 或未完成 responsible Work 时不能被普通删除/停用。

应用 reducer 和 trigger 使用相同的不变量测试向量。

## 12. 事务、锁与原子替换

### 12.1 统一锁顺序

建议固定：

1. Community Project advisory lock；
2. 现有 NIP-43 membership snapshot replacement advisory lock（本次会发布 snapshot
   时）；
3. `project_view_state FOR UPDATE`；
4. Role / Work rows，按 UUID 排序；
5. Proposal / Assignment / Commitment rows；
6. `relay_members` rows，按 pubkey 排序；
7. ownership-transfer 专用锁。

所有入口遵循同一顺序，避免 Project View、NIP-43 和 owner transfer 相互死锁。
v2 Community 的周期 membership snapshot reconcile/publisher 也必须先取得 Community
Project lock，再取得同一个 snapshot replacement lock；不能继续使用一条独立锁路径
覆盖刚刚与 Assignment 同事务提交的新 snapshot。若 canonical membership 没有变化，
reconcile 复用当前 snapshot event ID；若确实替换 snapshot，则必须在同一事务增加
project revision 并更新引用它的 v2 meta。

### 12.2 一次 Assignment replace

一次替换只增加一个 project revision，并在一个 transaction 中：

1. 再次验证 actor、Proposal、Role、候选 Member；
2. 分别核对 `expected_target_assignment_id` 与
   `expected_candidate_assignment_id`；
3. 验证当前 actor 只对本次 `accept_proposal` 或 `authorize_proposal` 动作有权；
4. 重新验证 Proposal 中记录的 `authorized_by` 当前仍有权批准完整换岗，包括授予目标
   Role，以及结束目标 Role 原任期和候选者原任期中实际存在的每一段旧 Assignment；
5. 将目标 Role 原承担者和候选者原任期中实际存在的旧 Assignment 置为
   `ended(replaced)`；
6. 结束这些旧 Assignment 的 active Commitments；
7. 为每个被腾空或接替的 Role 生成最小系统 Handoff；
8. 按全部最终 Assignment 结果重新计算受影响 Member 的 Community 等级；
9. materialize 或升级新承担者的 membership；
10. 创建候选者在目标 Role 上的新 active Assignment；
11. 将 Proposal 置为 consumed；
12. 写 authenticated change source 与幂等 receipt；
13. 写全部 canonical rows；
14. 写新的 NIP-43 membership snapshot；
15. 写所有 `40903` changed heads；
16. 写一个引用该 membership snapshot 的 `40904` meta；
17. commit 后才 fan-out。

任一步失败都不能出现两个 active 承担者，也不能留下“Assignment 已换、admin 未换”的
中间状态。

### 12.3 全局 revision 的取舍

v2 首版继续使用项目级 CAS。它会让不同 Role 同时写 Checkpoint 时发生显式 conflict，
但换来：

- 一个可验证的完整项目顺序；
- Project View、Assignment、Work 和 membership 的一致快照；
- 复用已经验证的 reducer、receipt、meta 和重连逻辑。

Checkpoint 只在重要局势变化时写入，不用于 heartbeat。上线后观察 conflict rate；只有
真实负载证明全局 CAS 成为瓶颈时，才设计子 aggregate revision，不能提前引入双真相。

## 13. Relay 投影与读取

### 13.1 v2 Projection union

kind `40903` 的 v2 content 可以表达：

- 九类 Project View object；
- Assignment Proposal；
- Assignment；
- Work Commitment；
- Role Checkpoint；
- Role Handoff。

稳定坐标：

```text
project-view:<community>:role:<role-id>
project-view:<community>:work:<work-id>
project-view:<community>:role-assignment-proposal:<proposal-id>
project-view:<community>:role-assignment:<assignment-id>
project-view:<community>:work-commitment:<commitment-id>
project-view:<community>:role-checkpoint:<checkpoint-id>
project-view:<community>:role-handoff:<handoff-id>
```

这些仍是 Project View 领域维护的 Relay current heads，不是通用 NIP-33 LWW。

### 13.2 Meta

kind `40904` v2 meta 至少包含：

- `schema_version: 2`；
- Community / Project ID；
- projection generation；
- project revision；
- 各 entity type 的当前 head count；
- 当前 NIP-43 membership snapshot event ID；
- changed heads；
- typed source change（`change_id`、`source_type`、可选 event/audit reference）；
- canonical updated time。

一个 command 可能产生多个 changed heads；客户端只在验证同一 meta 后接受它们。
当 Assignment 或 Community role 变化时，客户端按 meta 中的 event ID 精确读取并验证
对应 membership snapshot，不用“再查一次最新 membership”拼出可能跨 revision 的状态。

### 13.3 当前态与历史

- Role、Work 等对象按完整 snapshot 读取；
- active Assignment、open Proposal 和 active Commitment 属于默认当前态；
- ended Assignment、Checkpoint、Handoff 属于分页历史；
- Role Brief 只读取最新 Checkpoint、最近 Handoff 和当前需要的 Work；
- Desktop 打开历史时间线时再按 `role_id` 分页，不让默认 View 随项目年龄无限增长。

需要按 Role 查询的历史通过现有 `POST /query` Project View 扩展完成，并返回普通、可验
签的 `40903` event；不增加 `/roles/history` endpoint。

### 13.4 一致读取

客户端沿用：

```text
读 v2 meta
→ 按 generation + project_revision 分页读所需 heads
→ 校验 signer / coordinate / count / revision
→ 重读同一 meta
→ 组装 Project View / Role Brief
```

实时 event 只用于失效和增量提示；当出现 gap、generation 变化或完整性错误时，客户端
重读可信快照，不在本地猜测补齐。

### 13.5 v1 历史

v2 cutover 后：

- 当前 `40903/40904` heads 全部是 v2；
- 旧 v1 heads 被 soft-retire；
- 已接受的 v1 `44300` commands 和 receipts 永久保留；
- v2 历史读取器至少能把 v1 command 识别为 opaque legacy evidence；
- 不能假设某个 Project 的全部历史 command 都是 schema v2。

## 14. Agent Assignment fencing

### 14.1 写入 fencing

每个角色身份动作必须携带 `acting_assignment_id`。Relay 验证：

```text
assignment.status == active
assignment.member_pubkey == event.pubkey
assignment.role_id == operation required role
assignment belongs to current Community
```

因此：

- Runtime 重启仍能使用同一 active Assignment；
- A 被 B 替换后，A 的延迟命令因 Assignment ended 被拒绝；
- A 后来获得另一个 Role，也不能把旧命令误归因到新任期；
- 本地 start nonce 只能防本机旧进程，不替代 Project Assignment fencing。

### 14.2 Managed Agent 启动

现有 managed Runtime key：

```text
(agent pubkey, relay_url)
```

已经对应 Member × Project。启动流程：

1. ACP 使用 Agent 自己的 pubkey 和精确 Relay URL；
2. 读取 NIP-11，确认 `buzz-project-view-v2`；
3. 查询自己的 active Assignment；
4. 未分配时进入 candidate 状态：可读取、按 Community 资格执行普通 Project View CRUD、
   处理自己的 Proposal 或请求角色，但不能执行 Role-bearing / Leader 行为；
5. 已分配时读取 Role Brief；
6. 每次 turn 前确认 meta/Assignment 未变化；
7. 将最新 Brief 作为动态 `[Role Brief]` section 注入；
8. 角色写命令自动带上 Assignment ID。

Role 不写入长期固定的 `BUZZ_ACP_SYSTEM_PROMPT`。该 prompt 可能跨 turn 存活，替换后会
陈旧。Role Brief 短期缓存必须以
`(Community, assignment_id, project_revision, projection_generation)` 为键。

### 14.3 读取失败

如果 ACP 不能验证当前 Assignment 或 Brief：

- 不使用旧 Assignment 继续角色写入；
- 不把缓存 Brief 当作当前授权；
- 可以保留诊断能力，以及经 Relay 重新验证的 Community-only 普通 Project View 操作；
- 向 Human 明确报告 `assignment_unavailable` 或 `project_view_unavailable`。

这是 fail closed，不是把一次网络失败解释为 Agent 已卸任。

## 15. Desktop 设计接入

不新增顶级导航入口。继续在 `View` 页面扩展 Roles 区域：

- Role 卡显示 `admin/member`、当前承担者和 vacant；
- Inspector 显示当前 Assignment、Proposal 和历史任期；
- owner/Leader 可以邀请、指派、替换或治理性结束；
- 普通成员看到符合当前权限的动作；
- 最新 Checkpoint、Handoff 和 responsible Work 在 Role Inspector 中呈现；
- conflict、权限不足和 stale Assignment 明确显示，不自动重放 Human intent。

Native 边界：

- 新增 typed Tauri commands；
- Rust 验证 Relay signer 与 v2 projection；
- React 不接收未验证的 raw Nostr event；
- Role Continuity live event 到达后使当前 Community 的相关 Query 失效；
- Community key-based remount 后不得保留旧 Community 的 Assignment 或 Brief cache。

现有 Community Members 设置页也必须同步调整：

- 不能直接把 Member 提升为 admin；
- 不能直接降级 active Leader；
- 不能删除有 active Assignment 的 Member；
- UI 引导用户进入 Role 指派/结束流程；
- Relay 仍是最终授权者。

## 16. CLI 设计

建议首批命令：

```text
buzz roles list
buzz roles get --role <uuid>
buzz roles current
buzz roles proposals list
buzz roles request --role <uuid>
buzz roles offer --role <uuid> --member <pubkey>
buzz roles proposal accept --proposal <uuid>
buzz roles proposal reject --proposal <uuid>
buzz roles proposal withdraw --proposal <uuid>
buzz roles assignment end --assignment <uuid>
buzz roles assignment request-replacement --assignment <uuid>
buzz roles brief --role <uuid>
```

后续阶段增加：

```text
buzz roles work assign
buzz roles work accept
buzz roles work release
buzz roles checkpoint append
buzz roles checkpoint list
buzz roles handoff list
```

约束：

- reads 只消费 verified Relay projection；
- writes 使用 SDK typed builder；
- 不要求 Agent 手写 kind、tags 或通用 JSON event；
- 延续现有 write receipt；
- revision conflict 使用现有 exit code `5`；
- `--format compact` 仍是全局 flag；
- structured Checkpoint 可以从 stdin 读取 JSON，但先在本地执行相同 schema 校验。

## 17. Runtime 故障恢复

这一阶段建立在 Assignment fencing 已经可靠的前提上。

### 17.1 可信信号

只有受信 managed Runtime supervisor 可以报告：

- runtime identity；
- assignment ID；
- 单调 runtime epoch；
- lease renewal；
- abnormal exit；
- recovery attempt 及结果。

以下信号不能独立触发卸任：

- presence offline；
- WebSocket 断开；
- 一段时间没有发消息；
- 正常 session 结束；
- Desktop 暂时离线；
- 外部 CLI Agent 沉默。

### 17.2 恢复流程

```text
可信异常
  → Availability = recovering
  → Assignment 保持 active
  → 有限次数、带退避地恢复
      ├── 成功：available，Assignment ID 不变
      └── 超过期限：unavailable
                    → Project system action
                    → Assignment ended(unrecoverable)
```

多 Runtime 时，只要同一 Member 的任一受信 Runtime 仍健康，就不能将整个 Assignment
判为不可恢复。

### 17.3 系统安全要求

自动 `unrecoverable` 前必须具有：

- assignment-scoped lease；
- 单调 epoch/fencing token；
- 明确恢复窗口和最大重试；
- 多 Relay Pod 幂等 scheduler；
- supervisor/监控自身恢复后的额外宽限；
- 完整故障证据和审计；
- late Runtime 的命令拒绝。

最终结束动作通过第 8.2 节的内部 `ProjectSystemChange` 进入统一事务，不复用
Community Member 签名身份。supervisor 只能提交证据，不能直接改 Assignment row。

在这些条件完成前，只交付 Human/Leader 的人工 revoke/replace。

## 18. 错误模型

建议稳定 code：

```text
invalid:project_view_v2:schema
invalid:project_view_v2:change_source
invalid:role:not_active
invalid:role:has_assignment
invalid:role:has_responsible_work
invalid:proposal:expired
invalid:assignment:ended
invalid:assignment:actor_mismatch
invalid:commitment:role_mismatch
conflict:project_revision
conflict:role_occupied
conflict:member_already_assigned
conflict:candidate_assignment_changed
conflict:work_already_committed
forbidden:assignment:self_end
forbidden:assignment:peer_leader
forbidden:role_level:owner_required
forbidden:membership:assignment_active
forbidden:managed_agent:owner_ineligible
unavailable:role_brief
unavailable:runtime_supervision
```

错误不泄露另一个 Community 是否存在同 UUID、Member 或 Assignment。

## 19. 迁移、启用与回滚

### 19.1 为什么不能原地扩展 v1

NIP-PV v1 command 是 closed schema，Rust 类型也使用 `deny_unknown_fields`。
旧客户端不认识 Role `level`、Work responsibility 或新 operation。因此：

- 不在 v1 增加“可选”写字段；
- 不让同一 Community 同时广告 v1 和 v2，除非未来真正实现双投影；
- Relay binary 可以同时服务不同 Community 的 v1 或 v2；
- 一个 Community 只能按明确 cutover 从 v1 进入 v2。

### 19.2 Cutover preflight

目标 Community 至少必须满足：

- Project View v1 已初始化且当前 aggregate 校验通过；
- Project View 写入已经暂停，没有在途 mutation；
- migration、Relay signer 和 v2 projection builder ready；
- 恰好一个 Community owner，且该治理根不是已登记的 managed Agent identity；
- owner 已显式提供所有现有非 owner admin 的 Leader Role 映射或降级决定；
- 映射后的 Role/Member 不违反双向一对一；
- 所有被映射 Role active，且 admin Role 只能由 owner 授权；
- managed Agent 的 verified owner 关系当前有效；
- 不存在会被 v2 Role/Work 约束立即判为非法的 active 对象。

preflight 只报告问题，不根据名字、profile 或历史聊天自动猜测映射，也不部分修改数据。

### 19.3 Server-first

推荐顺序：

1. 合并 v2 domain、migration、Relay、SDK、CLI、preflight 和测试；
2. additive 应用 migration 0026，但所有 Community 保持 v1；
3. 升级全部 Relay Pod；
4. owner 在 v1 仍可写时先创建/修正有业务含义的 Leader Role，并准备显式 mapping；
5. 对目标 Community 暂停 Project View 写入；
6. 带 mapping 运行 v2 preflight；
7. backfill v1 Role 为 `level=member`，再按 mapping 设置 admin Role，Work
   responsibility 为空；
8. 创建明确映射的初始 Assignment；
9. 执行 integrity audit；
10. project schema 切到 2，projection generation 加一；
11. 全量重签 v2 `40903/40904` heads；
12. 只宣告 `buzz-project-view-v2`；
13. 恢复写入并运行 CLI/Desktop E2E。

步骤 7–12 由一个持有 Project/snapshot locks 的 cutover operation 完成；任一步失败都
保持 v1 disabled 状态，不对客户端暴露半套 v2 heads。

目标 Community 只有在它实际依赖的客户端都已支持 v2 后才可 cutover：

- 阶段 2 只启用受控的 CLI pilot / staging Community；
- 依赖 Desktop Human 操作的 Community 至少等待阶段 3；
- 依赖 managed Agent 自动写入的 Community 至少等待阶段 4。

安全地显示 `unsupported` 是兼容底线，不代表可以让生产成员长期失去操作入口。

### 19.4 Admin bootstrap

现有非 owner admin 无法从名字或当前 Role 描述中可靠推断对应哪个 Leader Role。
preflight 必须要求 owner 对每个 admin 显式选择：

- 映射到一个唯一 admin Role 并创建 Assignment；或
- 降级为 member。

不能：

- 根据 Role 名称猜测；
- 自动创建一个无业务含义的 Leader Role；
- 让没有 Assignment 的旧 admin 留在 v2。

普通 Role 和普通 Member 不自动配对。

### 19.5 回滚

采用：

```text
应用可关闭或前滚修复，数据库只前进
```

- 不 hard-delete v2 canonical history；
- 不提供常规 down migration；
- v2 写入发生后，不把 Community 无损降回 v1；
- 故障时先在共同锁下关闭该 Community 的 Project View capability；
- 已有 v2 heads 时，不能直接启动只理解 v1 的旧 Relay 并重新广告 v1；
- 优先使用仍理解 migration 0026、kind read gate 和 v2 disabled state 的
  rollback-compatible binary；
- 重新启用前再次运行 preflight/reconcile/reproject。

## 20. 测试与质量门

### 20.1 纯领域测试

- Proposal 双方确认状态机；
- Proposal 到期以及目标/候选两段 expected Assignment fencing；
- Proposal authorizer 在候选者最终确认前失去权限时，确认失败且不产生部分换岗；
- Role/Member 双向一对一；
- ended Assignment 不可恢复；
- self-end 和同级 Leader 拒绝；
- 非 owner 不能降级、停用或重新启用 admin Role；
- Role level 与 Community role 映射；
- Work responsibility/Commitment 一致；
- Assignment 结束不改变 Work；
- Checkpoint/Handoff 追加不可变；
- Role Brief 确定性组装；
- 全部概念设计验证清单的属性测试。

### 20.2 数据库与并发

- 两个 Member 同时争抢一个 Role，恰一成功；
- 一个 Member 同时争抢两个 Role，恰一成功；
- replace 与 NIP-43 remove/change role 竞争；
- replace 与 owner transfer 竞争；
- Project writer 与周期 NIP-43 snapshot reconcile 竞争；
- NIP-98 invite、operator 和 system source 的 change ID 幂等；
- partial unique index 和 deferred trigger 防止旁路；
- command replay 返回同 receipt、不增 revision；
- projection、membership snapshot 或 canonical insert 故障时全回滚；
- v1→v2 fresh、upgrade、cutover、reproject 和 schema drift。

### 20.3 Relay 与安全

- owner、Leader、普通 Role、verified owner、candidate 的完整授权矩阵；
- raw event 也不能 self-unassign；
- self-leave、Role deactivate、member remove、admin promote 旁路全部被封；
- admin Role 的降级、停用和重新启用同样不能绕过 owner-only 约束；
- managed Agent direct row 不能绕过 owner；
- persistent ban 原子结束被 ban 的 Human/Agent 自身 Assignment 及其相关 managed
  Agent Assignment；权限不足时整次拒绝，unban 不复活旧任期；
- ownership transfer 前不能 ban Community owner；
- owner transfer 拒绝 known managed Agent 成为治理 owner；
- v1 client 不误读 v2 head；
- v2 client 识别 v1 historical command；
- WS、HTTP、COUNT、search 和 live fan-out 保持严格成员读取门禁；
- NIP-09 不能删除 accepted command 或 canonical history。

### 20.4 CLI、Desktop 与 ACP

- CLI Assignment 纵向 E2E；
- Desktop 空缺、指派、替换、conflict、权限不足 Playwright；
- Community 切换无缓存泄漏；
- Runtime 重启恢复相同 Assignment；
- 替换后旧 Runtime 延迟动作失败；
- Brief JSON、CLI Markdown 和 Desktop 来自同一 assembler；
- Brief 读取失败时 Agent fail closed。

实现代码提交前按仓库要求运行：

```text
. ./bin/activate-hermit
just ci
just test
cargo test --manifest-path desktop/src-tauri/Cargo.toml
```

具体阶段只运行与改动相称的定向门禁，合并前运行完整门禁。

## 21. Observability 与运维

低基数 metrics：

```text
buzz_project_view_v2_commands_total{operation,result}
buzz_project_view_v2_command_duration_seconds{operation}
buzz_role_assignment_transitions_total{transition,result}
buzz_role_assignment_conflicts_total{constraint}
buzz_role_membership_reconciliations_total{result}
buzz_role_brief_build_duration_seconds{result}
buzz_role_runtime_recovery_total{result}
buzz_project_view_v2_preflight_failures_total{reason}
```

结构化日志：

```text
community_host
change_id
source_type
command_event_id
actor_pubkey
acting_assignment_id
operation
role_id
work_id
expected_project_revision
committed_project_revision
result_code
```

正文、Checkpoint 内容、Resource locator 和 owner 私密信息不进入普通日志。Community
UUID、Role ID、Assignment ID 不作为 metric label。

建议 admin 能力：

```text
buzz-admin project-view v2 preflight
buzz-admin project-view v2 cutover --mapping <file>
buzz-admin project-view reconcile
buzz-admin project-view reproject
```

## 22. 阶段开发计划

### 阶段 0：Project View v2 协议与迁移骨架

交付：

- 更新 `docs/nips/NIP-PV.md`，固定 v2 wire schema；
- `buzz-project-view::v2` 纯领域类型和状态机；
- v1/v2 per-Community version/readiness；
- migration 0026 和 `schema/schema.sql` 骨架；
- v2 `project_view_changes` source/audit/idempotency 模型；
- v2 projection parser/builder；
- preflight/reconcile 只读能力；
- v1/v2 compatibility tests。

验收：

- v1 Community 行为完全不变；
- v2 未通过 preflight 时不宣告 capability；
- v1 客户端遇到 v2 Community 会明确显示 unsupported，而不是误读；
- generation 提升且收到未知 schema meta 时，客户端丢弃旧 current-state cache、
  重新读取 NIP-11，而不是继续展示可写的 v1 快照；
- 不连接数据库即可验证 v2 核心不变量和 closed schema。

本阶段不启用任何 v2 Community。

### 阶段 1：Membership 一致性内核

交付：

- 所有 `relay_members` 写路径改用共同事务 primitives；
- Community Project lock 和统一锁顺序；
- NIP-43 snapshot publisher/reconcile 的共同锁；
- NIP-43 add/remove/change、自助退出、invite、admin、bootstrap、owner transfer、
  persistent ban/unban 守卫；
- managed Agent direct-member owner 旁路修复；
- Role `level` 与治理性 Role 更新；
- DB partial index、deferred constraint 和并发测试。

验收：

- 不能直接制造没有 Leader Assignment 的 admin；
- 非 owner 不能创建、升降级、停用或重新启用 admin Role；
- active Assignment 不能通过 self-leave 或通用 remove 消失；
- owner transfer 后旧 owner 等级按 Assignment 决定；
- owner transfer 不能把 known managed Agent 设为治理 owner；
- managed Agent owner 失效后不能凭 direct row 继续；
- persistent ban 不会留下仍 active 的不合格 Agent Assignment；
- invite/operator 变化具有稳定 change ID、audit 和幂等 receipt；
- membership/projection 任一步失败全部回滚。

本阶段仍不对生产 Community 宣告 v2 ready。

### 阶段 2：Proposal、Assignment 与 CLI 纵向闭环

交付：

- request/offer/accept/reject/withdraw/expire/authorize Proposal；
- Assignment 激活、人工结束和原子替换；
- 目标 Role 与候选 Member 两段旧 Assignment 的 compound fencing；
- replace 时生成最小系统 Handoff；
- Agent `request_replacement` / `report_unable_to_continue`；
- Community 等级同步；
- Relay 对 `acting_assignment_id` 的服务端校验与 ended Assignment fencing；
- v2 `40903/40904` projection；
- `buzz roles list/get/current/proposals/assignment`；
- v1→v2 cutover 工具和第一条完整 E2E。

验收：

- owner 将 Agent A 指派到普通 Role；
- A 不能结束自己，即使手写 raw event；
- Leader 可以替换普通 Role，但不能替换自己或同级 Leader；
- Proposal authorizer 在候选者确认前失去权限时，最终确认被拒绝且状态不被部分消费；
- A→B 替换在任意时点都只有一个 active Assignment；
- 候选者原本承担另一 Role 时，两段旧任期与新任期也只产生一个原子结果；
- Assignment 与 Community role、meta revision、CLI 读取完全一致；
- A 使用旧 Assignment ID 的延迟命令被拒绝。

这一阶段完成后，Role Assignment 后端和 Agent CLI 形成第一个可用闭环。

### 阶段 3：Desktop Human 治理

交付：

- `View` Role 卡的 level、承担者和 vacant 状态；
- Role Inspector 的 Proposal、当前任期和历史任期；
- owner/Leader 的邀请、指派、替换和结束流程；
- Community Members 设置页的 v2 守卫与引导；
- live invalidation、conflict 和权限错误体验；
- Desktop unit/Tauri/Playwright tests。

验收：

- Human 与 CLI 实时看到同一个 Assignment；
- Role 有 active Assignment 时不能从通用编辑器静默停用或删除；
- Settings 不能绕过 Leader Assignment 改 admin；
- Community 切换后不泄漏上一项目的 Role/Assignment；
- 并发替换不会自动覆盖另一位 Human 的操作。

### 阶段 4：Managed Agent 绑定与最小 Role Brief

交付：

- ACP 启动/turn 前解析 active Assignment；
- candidate 与 assigned 两种进入状态；
- ACP 自动携带 `acting_assignment_id`、在 turn 前刷新，并消费阶段 2 已交付的 Relay
  fencing；
- 最小 Role Brief：
  Profile/Goal 摘要、Role、level、Assignment、相关 Project View 切片和 source revisions；
- 动态 `[Role Brief]` prompt section；
- CLI 与 Desktop 最小 Brief 展示。

验收：

- Agent Runtime 重启后恢复同一 Assignment；
- 未分配 Agent 不能执行角色身份写入；
- 替换后旧 Runtime 即使仍运行也不能继续写；
- Brief 不能验证时 Agent 不使用旧授权继续；
- JSON、Markdown 和 Desktop Brief 来自同一 verified state。

这一阶段真正完成“Agent 进入 Project 时以 Role 身份行动”。

### 阶段 5：Work Responsibility 与 Commitment

交付：

- Work `responsible_role_id`；
- Work 接受、释放、替换 Commitment；
- Assignment 结束时 Commitment 原子终止；
- waiting-for-continuation 派生状态；
- Role Brief 加入 responsible Work、active Commitment 和待接续 Work。

验收：

- Agent 不能接受其他 Role 的 Work；
- active Commitment 存在时不能直接改变 responsible Role；
- Assignment 结束不改变 Work status；
- B 接任后必须显式接受 A 遗留的 Work；
- A 的历史 Commitment 和贡献仍归 A。

### 阶段 6：Checkpoint、Handoff 与完整 Role Brief

交付：

- 结构化 append-only Checkpoint；
- member-authored Handoff note；
- 在阶段 2 的最小 Handoff 上增加 Checkpoint、遗留事项和引用；
- Role 历史分页；
- 完整 Role Brief；
- Agent 在重要局势变化时提交 Checkpoint；
- Desktop Role 时间线。

验收：

- 连续 Checkpoint 全部保留，Brief 只选最新入口；
- ended Assignment 不能再提交角色 Checkpoint；
- 跨 Project 或不存在的 reference 被拒绝；
- 有 Handoff 时继任者可读取；没有 Handoff 也能从 Project 状态接续；
- Handoff 不改写 Work、Issue 或前任贡献归因。

完成这一阶段后，人工指派、替换和接续条件下的 Role Continuity v0 完整闭环成立。

### 阶段 7：可信 Runtime 监督与自动故障恢复

交付：

- assignment-scoped lease、runtime epoch 和 supervisor identity；
- internal `ProjectSystemChange`、supervisor evidence 与 system audit；
- `available/recovering/unavailable` 状态；
- 有限重试、恢复窗口和幂等 scheduler；
- `unrecoverable` system action；
- 审计、metrics、监控故障宽限和运维开关。

验收：

- 短暂崩溃恢复时 Assignment ID 不变；
- 恢复耗尽后才结束为 `unrecoverable`；
- presence 或普通断线不能自动卸任；
- 监控系统故障不会批量结束 Assignment；
- 外部 CLI Agent 不会仅因沉默被自动卸任；
- ended 后恢复的旧 Runtime 全部角色命令被 fencing。

这是风险最高的阶段，不能提前与 Assignment 基础交付捆绑。

## 23. 总体验收标准

阶段 6 完成即构成 Role Continuity v0。v0 必须能够证明：

1. Human 和 Agent 通过同一 Project View v2 读取同一 Role、Assignment 和 Work 状态。
2. Role 与 Member 的 active Assignment 严格一对一。
3. Leader 与 Community admin 直接等同且始终一致。
4. Agent 无法通过任何已知入口主动卸任。
5. Assignment 替换是原子的，并永久保留前后任归因。
6. Agent Runtime 重启不改变 Assignment。
7. 旧 Runtime 不能在任期结束后继续以旧 Role 行动。
8. Work 责任属于 Role，Commitment 属于具体 Assignment。
9. Checkpoint 与 Handoff 属于 Project，且不是 Role 中的一段可覆盖文本。
10. Role Brief 可从验证后的规范状态随时重建。
11. 没有旧 Agent 的最终总结，继任者仍能继续工作。

阶段 7 是 v0 之后的自动恢复增强。只有交付阶段 7 时，才额外验收：

12. 自动故障结束只发生在可靠监督和明确恢复条件之后。

## 24. 最终模型

```text
Project / Community
├── Project View objects
│   ├── Role
│   │   ├── level: admin | member
│   │   └── active Assignment: 0..1
│   └── Work
│       ├── responsible Role: 0..1
│       └── active Commitment: 0..1
├── Community Member
│   ├── active Assignment: 0..1
│   └── Runtime: 0..*
├── Assignment Proposal
├── Assignment history
├── Checkpoint history
├── Handoff history
└── Role Brief（由上述状态派生）
```

一句话概括实现：

> 用 Project View v2 的同一条命令、revision、事务和 Relay 投影链路维护 Role、
> Assignment、Community 等级与工作连续状态；Agent 只携带当前 Assignment 行动，
> Runtime 可以变化，而 Project 持有的责任、局势和历史持续存在。
