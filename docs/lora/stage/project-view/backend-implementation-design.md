# Project View 后端实现设计

> 本文定义 Project View 首版如何作为 Buzz 原生后端能力落地。
> 实现范围包括领域模型、Nostr 事件、Relay、PostgreSQL、读取投影、SDK、Agent CLI、
> 测试、CI、迁移、发布和回滚；暂不实现 Desktop、Web 或 Mobile 界面，也不实现项目连续性。

## 1. 文档目的

[项目视图定义](./project-view.md)已经说明 Project View 要回答什么，
[基本对象与关系设计](./object-relation-design.md)已经固定首版对象、基数和关系约束。
本文继续回答：

1. 这些对象在 Buzz 后端中放在哪里；
2. Human 与 Agent 如何通过同一套协议读取和修改它们；
3. 如何复用 Buzz 现有 Community、认证、事件、PostgreSQL、Redis、审计和发布体系；
4. 如何保证项目级唯一状态、并发安全、多租户隔离和失败可恢复；
5. 如何通过现有测试与 CI 链路交付，而不是形成一套旁路系统。

本文是一份实现设计，不在本阶段提交 Rust、SQL 或客户端代码。

## 2. 实现结论

首版采用以下总体方案：

1. **一个 Buzz Community 等于一个 Project。**
   Project 身份只从服务端 `TenantContext.community()` 取得，客户端不能提交
   `project_id` 或 `community_id`。
2. **Project View 是 Buzz Relay 内的原生领域能力。**
   不新增服务、不新增数据库、不新增消息系统，也不引入独立部署单元。
3. **成员签名修改命令，Relay 维护权威当前态。**
   Human 或 Agent 提交成员签名的 Project View mutation event；Relay 校验后，在同一
   PostgreSQL 事务内写入命令事件、规范对象状态、幂等收据和 Relay 签名读取投影。
4. **规范对象表是当前状态的权威来源。**
   成员签名命令是不可变变更证据；Relay 签名 projection 是 Nostr 可读取、可订阅的
   当前态 read model。三者不允许各自独立提交。
5. **写入继续走 Buzz 通用事件入口。**
   WebSocket `EVENT` 和 HTTP `POST /events` 共用同一个 ingest 与领域事务，不增加
   `/project-view/*` 专用 REST API。
6. **读取继续走 Buzz 通用查询入口。**
   单对象、meta 和实时订阅使用普通 Nostr filter；完整大视图通过现有
   `POST /query` 的 Buzz 扩展字段进行 revision-pinned keyset 分页，不增加新 endpoint。
7. **首版采用项目级乐观并发控制。**
   每个 mutation 必须携带 `expected_project_revision`。任意并发修改只允许一个成功，
   其他写入明确返回 conflict，客户端重新读取并重放自己的意图。
8. **所有 Project View 协议事件只对当前 Community 成员可见。**
   这里的“成员”是 `relay_members` 中的直接成员，或其 owner 是直接成员、且已经过
   NIP-OA 验证的 managed Agent。该门禁独立于 `BUZZ_REQUIRE_RELAY_MEMBERSHIP`：
   即使 Buzz 以 open-relay 模式运行，普通已认证身份和 channel-scoped token 也不能
   读取或修改 Project View。首版不做对象级 ACL；Project Role 仍是语义职责，不参与
   Buzz 权限判断。
9. **状态变化全部显式。**
   Work 完成不自动改变 Requirement、Issue、Stage、Plan 或 Goal；关系变动也不触发
   状态级联。
10. **实时通知沿用 Buzz 的提交后 fan-out 语义。**
    Redis 或本地 fan-out 失败不回滚已提交事务；客户端通过 revision 和重连查询恢复。
    首版不为 Project View 单独建立一套可靠消息基础设施。

## 3. 与 Buzz 的整体关系

```text
Human / Agent
    │
    │ signed kind:44300 mutation
    ▼
现有 WS EVENT / POST /events
    │
    ▼
buzz-relay ingest
    ├── 现有签名、身份、scope、ban/timeout、限流检查
    ├── Project View 强制 Community member gate
    ├── communities.project_view_enabled 中心开关
    └── Project View 专用 command handler
            │
            ▼
      同一个 PostgreSQL 事务
            ├── events：成员签名 mutation
            ├── project_view_state：项目 revision
            ├── project_view_objects：规范当前对象
            ├── project_view_mutations：幂等收据
            └── events：Relay 签名 kind:40903/40904 当前态 projection
            │
            ▼ commit
      现有 audit + Redis + 本地 subscription fan-out
            │
            ▼
普通 WS REQ / POST /query / buzz project-view
```

这套实现只在 Buzz 现有分层中增加一个领域模块，不形成“Buzz 旁边的 Lora 服务”。

## 4. 代码位置与模块边界

### 4.1 新增纯领域 crate

新增：

```text
crates/buzz-project-view/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── model.rs
    ├── mutation.rs
    ├── patch.rs
    ├── validation.rs
    └── read_model.rs
```

职责：

- 定义九类 Project View 对象及枚举；
- 定义 wire schema v1 的 mutation、typed patch 和 projection payload；
- 执行不依赖数据库的字段、关系和最终态校验；
- 从扁平对象集合确定性组装关系设计中定义的逻辑读取结构；
- 提供稳定的错误 code；
- 提供属性测试所需的纯内存状态机。

约束：

- 不依赖 SQLx、Tokio、Redis、Axum 或 Nostr 网络；
- 不读取环境变量；
- 不持有数据库连接；
- 不签名事件；
- 不把 Buzz Community 权限角色混入 Project Role。

新增 crate 是代码边界，不是新基础设施。它随 Relay、SDK 和 CLI 一起编译和发布。

### 4.2 `buzz-core`

修改：

```text
crates/buzz-core/src/kind.rs
```

职责：

- 注册 Project View kind；
- 把 kind 加入 `ALL_KINDS`；
- 定义 mutation、projection 和 relay-only classifier；
- 提供 `is_project_view_projection_kind()` 等共享 classifier；
- 保证 Project View protocol event 不触发普通 Workflow。

不把完整 Project View 领域模型塞入 `buzz-core`。`buzz-core` 继续保持 Buzz 通用的
零 I/O 基础层；Project View 的领域演进留在独立 crate。

### 4.3 `buzz-sdk`

新增：

```text
crates/buzz-sdk/src/project_view.rs
```

职责：

- 从强类型 mutation 构造 kind `44300` 事件；
- 构造唯一且严格的 tags；
- 解析和验证 Relay projection；
- 校验 projection 签名者、坐标、revision、对象类型和 content/tag 一致性；
- 暴露 Project View wire types，避免 CLI 手写 JSON event。

### 4.4 `buzz-db`

新增：

```text
crates/buzz-db/src/project_view.rs
```

并修改：

```text
crates/buzz-db/src/lib.rs
crates/buzz-db/src/event.rs
crates/buzz-db/src/migration.rs
crates/buzz-db/src/usage.rs
```

职责：

- Project View 的全部 SQL；
- writer-pool 事务和社区级 advisory lock；
- revision CAS、幂等收据、对象查询和反向引用查询；
- 让 event store 为 Project View projection 提取并索引 `d` tag；
- mutation event 与 projection event 的事务内插入；
- revision-pinned 完整视图分页；
- 数据库错误到稳定领域结果的映射。

Relay handler 不直接拼 SQL。`buzz-db` 提供一个受限的
`ProjectViewWriteTx`，只暴露本领域需要的事务操作。

### 4.5 `buzz-relay`

新增：

```text
crates/buzz-relay/src/handlers/project_view.rs
```

并修改：

```text
crates/buzz-relay/src/handlers/mod.rs
crates/buzz-relay/src/handlers/ingest.rs
crates/buzz-relay/src/handlers/event.rs
crates/buzz-relay/src/handlers/req.rs
crates/buzz-relay/src/handlers/count.rs
crates/buzz-relay/src/api/bridge.rs
crates/buzz-relay/src/api/mod.rs
crates/buzz-relay/src/nip11.rs
crates/buzz-relay/src/main.rs
```

职责：

- 在 Buzz 通用安全检查之后路由 mutation；
- 解析成员签名 payload；
- 从 `TenantContext` 注入唯一 Project 身份；
- 调用领域校验和 DB 事务；
- 使用现有 Relay key 签名 meta/object projection；
- 把 domain conflict 映射到 WS/HTTP 协议；
- 提交后复用现有 Redis 与本地 fan-out；
- 在 REQ、COUNT、search、HTTP bridge 和本地/Redis fan-out 的共同 chokepoint 强制
  Project View member-only 读取；
- 在 NIP-11 宣告能力；
- 在 `/query` 内处理 Project View 分页扩展。
- 把 Project View 对象数量接入现有定时 DB usage rollup。

### 4.6 `buzz-cli`

新增：

```text
crates/buzz-cli/src/commands/project_view.rs
```

并接入：

```text
crates/buzz-cli/src/commands/mod.rs
crates/buzz-cli/src/client.rs
crates/buzz-cli/src/lib.rs
```

CLI 是 Agent 的一等操作面，也可供 Human 使用。它属于本后端切片；Desktop、Web 和
Mobile UI 不属于本阶段。

### 4.7 `buzz-admin`

新增：

```text
crates/buzz-admin/src/project_view.rs
```

并接入：

```text
crates/buzz-admin/src/main.rs
```

职责：

- 查看 Project View schema、Community 开关和 signer/projection readiness；
- 原子启用或停用一个/全部 Community；
- 在 Relay key 轮换期间执行受 Community lock 保护的 projection rebuild；
- 为发布、回滚和故障处置提供与数据库同源的控制面。

这些是 operator 操作，不放进成员可调用的 `buzz-cli`，也不新增管理服务。
需要签名的 admin 操作只从 `BUZZ_RELAY_PRIVATE_KEY` Secret 环境变量或权限受限的
`--relay-key-file` 读取私钥；argv 最多接受 `--expected-pubkey`/fingerprint 防误操作，
绝不接受 `--relay-key <hex>`，避免私钥进入 shell history、进程列表或 CI 日志。

### 4.8 协议、迁移与期望状态

新增或修改：

```text
docs/nips/NIP-PV.md
migrations/0025_project_view.sql
schema/schema.sql
Cargo.toml
Cargo.lock
Justfile
.github/workflows/ci.yml
.github/workflows/docker.yml
deploy/charts/buzz/values.yaml
deploy/charts/buzz/values.schema.json
deploy/charts/buzz/templates/deployment.yaml
deploy/charts/buzz/tests/
deploy/compose/README.md
```

`docs/nips/NIP-PV.md` 是 wire contract；本文是 Buzz 内部实现设计。二者不能只写一份。
Chart/Compose 文档只描述部署顺序、稳定 Relay key 和 admin 开关操作；不再为每个 Pod
增加 `BUZZ_PROJECT_VIEW_ENABLED`，避免滚动更新期间出现彼此矛盾的开关状态。

## 5. 依赖方向

| Crate | 可以依赖 |
|---|---|
| `buzz-core` | 不依赖 Project View |
| `buzz-project-view` | `buzz-core` |
| `buzz-sdk` | `buzz-core`、`buzz-project-view` |
| `buzz-db` | `buzz-core`、`buzz-project-view` |
| `buzz-relay` | `buzz-core`、`buzz-project-view`、`buzz-db`、`buzz-sdk` |
| `buzz-cli` | `buzz-core`、`buzz-project-view`、`buzz-sdk`、现有 client crates |
| `buzz-admin` | `buzz-core`、`buzz-project-view`、`buzz-db`、`buzz-sdk` |

依赖保持单向且无环。CLI 不链接 Relay；它只通过现有 Buzz 网络协议与 Relay 通信。
`buzz-admin reproject` 必须复用领域层的 `ProjectionPlan`、SDK event builder 和同一个
`ProjectViewWriteTx` maintenance API，不复制一份 projection JSON 或 SQL。

## 6. 领域对象

### 6.1 通用对象信封

所有对象使用同一个稳定信封：

```rust
ProjectViewObject {
    id: Uuid,
    object_type: ProjectViewObjectType,
    object_revision: u64,
    project_revision: u64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    created_by: PublicKey,
    updated_by: PublicKey,
    data: ProjectViewObjectData,
    relations: ProjectViewRelations,
}
```

规则：

- `id` 在当前 Project 内全局唯一，而不是只在某一对象类型内唯一；
- `object_type` 和 `id` 创建后不可修改；
- 对象 revision 从 `1` 开始，只在该对象变化时递增；
- project revision 在任何成功 mutation 后递增；
- 时间和 actor 由 Relay 根据已验证事件写入，客户端不能伪造；
- canonical `created_at`、`updated_at`、`deleted_at`、`accepted_at` 以及 projection
  `created_at` 都使用取得 exclusive advisory lock 后从 DB 捕获的一次
  `clock_timestamp()`；不能使用 transaction 开始时间，因为等待锁的事务可能比先提交的
  revision 拿到更早时间。已有 state 时取
  `GREATEST(clock_timestamp(), state.updated_at + interval '1 microsecond')`，保证数据库
  时钟校正也不会让 canonical time 随 revision 倒退。客户端 Nostr `event.created_at`
  只用于 admission 和审计，不能操纵领域排序；
- Profile 的 ID 固定等于服务端 Community UUID；
- 其他对象 ID 由客户端生成 UUID v4；
- Community UUID 是 Profile ID 的保留值，其他对象不得使用。

### 6.2 对象类型

Rust 类型避免和 Buzz 已有概念混淆：

| Wire 类型 | Rust 类型 | 最小字段 |
|---|---|---|
| `project_profile` | `ProjectProfile` | `name`、`positioning`、`purpose`、`problem`、`scope` |
| `goal` | `Goal` | `title`、`desired_outcome`、`directions[]` |
| `role` | `ProjectRole` | `name`、`purpose`、`responsibilities[]`、`boundaries[]`、`active` |
| `plan` | `ProjectPlan` | `title`、`description`、`status` |
| `stage` | `ProjectStage` | `title`、`description`、`status` |
| `requirement` | `Requirement` | `title`、`description`、`status`、`priority` |
| `issue` | `ProjectIssue` | `title`、`description`、`status`、`priority` |
| `work` | `ProjectWork` | `title`、`description`、`status`、`priority` |
| `resource` | `ProjectResource` | `name`、`resource_type`、`locator`、`description` |

命名原则：

- `ProjectIssue` 不等于 NIP-34 Git Issue；
- `ProjectPlan` 不等于 `buzz-workflow` Workflow；
- `ProjectWork` 不等于 Agent Job、Workflow Run、PR 或 Git commit；
- `ProjectRole` 不等于 Buzz owner/admin/member，也不等于 Persona；
- Buzz Desktop 当前 `Project` 仍表示 Git Repository，本阶段不改其既有行为。

Resource locator 使用强类型信封，而不是无法判断含义的裸字符串：

```rust
ResourceLocator {
    locator_type: Url | NostrAddress | NostrEvent | BuzzDeepLink,
    value: String,
}
```

Repository Resource 优先以 NIP-34 `30617:<pubkey>:<repo-id>` 地址登记。Locator 只是
稳定入口；Relay 不自动访问、复制或同步目标内容。

### 6.3 状态与优先级

首版固定闭集：

```text
Priority:
  low | normal | high | urgent

PlanStatus:
  draft | active | paused | completed | cancelled

StageStatus:
  planned | active | paused | completed | cancelled

RequirementStatus:
  proposed | ready | in_progress | satisfied | withdrawn

IssueStatus:
  open | in_progress | resolved | closed

WorkStatus:
  pending | in_progress | paused | submitted | completed | cancelled

ResourceType:
  repository | document | design | service | environment | artifact | url
```

Goal 首版没有达成状态。Relay 只验证枚举值，不定义复杂状态机；任何合法枚举之间的
改变都必须由显式 mutation 完成。

### 6.4 关系字段

关系不埋入自由 JSON：

```text
Plan.under_goal_id?             -> Goal
Stage.under_plan_id             -> Plan
Requirement.planned_in_stage_id? -> Stage
Issue.planned_in_stage_id?      -> Stage
Issue.about?                    -> 任意 active Project View object
Work.handles                    -> Requirement XOR Issue
```

`about` 和 `handles` 使用：

```rust
ObjectRef {
    object_type: ProjectViewObjectType,
    object_id: Uuid,
}
```

所有关系都由服务端验证目标存在、未删除、类型正确且位于当前 Community。客户端无法
通过 payload 选择另一个 Project。

首版允许显式更换 Work 的 `handles` 对象，但不允许清空；任何时点仍必须恰好指向一个
Requirement 或 Issue。后续 Work 生命周期如果需要限制更换时机，再单独设计。

### 6.5 顺序

首版不把 Plan 强制建模为线性序列，也不提前引入 DAG、依赖或 LexoRank。

逻辑读取结果在没有显式结构语义时按：

```text
created_at ASC, id ASC
```

稳定排序。这只是确定性输出顺序，不表示业务优先级或 Stage 执行顺序。

## 7. Mutation 协议

### 7.1 Kind 分配

首版预留：

| Kind | 名称 | 作者 | 存储语义 |
|---:|---|---|---|
| `44300` | `KIND_PROJECT_VIEW_MUTATION` | Community 成员 | append-only command |
| `40903` | `KIND_PROJECT_VIEW_OBJECT` | Relay only | 每对象坐标只保留当前 head |
| `40904` | `KIND_PROJECT_VIEW_META` | Relay only | 每 Project 只保留当前 meta |

三者都位于 Buzz 现有 `40000–49999` 自定义 kind 范围内。`40903`/`40904` 延续
`40901`/`40902` 的 Relay-only sidecar 布局；`44300–44399` 作为 Project View
协议范围保留。

实现前必须再次检查：

- `buzz-core::kind::ALL_KINDS` 无重复；
- Buzz 自己的 kind registry 无碰撞；
- Nostr registry-of-kinds 尚未登记这些值。

Kind 选择和协议内容写入 `docs/nips/NIP-PV.md`。实现时依据
[NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md)与
[registry-of-kinds](https://github.com/nostr-protocol/registry-of-kinds/blob/master/schema.yaml)
再次核对。若发现碰撞，应在合并任何持久数据前整体更换；发布后不得静默复用同一 kind
表达另一语义。

`40903`/`40904` 虽然带 `d` tag，但不是 NIP-33 addressable kind。Project View head
由 Relay 按照 `(community_id, kind, d)` 和领域 revision 管理，不受 NIP-33 的作者
与秒级 `created_at` LWW 规则影响。

Buzz 当前只为 `30000–39999` kind 提取 `events.d_tag`。实现必须把
`extract_d_tag()` 扩展为也识别这两个 Relay projection kind，并增加回归测试。这样普通
`#d` 查询可以继续使用现有 `events.d_tag` 与索引；这不把 `40903`/`40904`变成
NIP-33 replaceable event。

只改写入提取还不够。`handlers/req.rs::filter_to_query_params()` 当前只在 filter 的
所有 kind 都位于 `30000–39999` 时才把 `#d` 下推 SQL；否则 `#d` 可能在 SQL `LIMIT`
之后才匹配，point read 会误报不存在。实现必须在 `buzz-core` 定义共享的：

```rust
has_indexed_d_tag(kind) =
    kind in 30000..=39999
    || kind == KIND_PROJECT_VIEW_OBJECT
    || kind == KIND_PROJECT_VIEW_META
```

并让 `extract_d_tag()` 与 WS/HTTP/COUNT 共用的 query builder 都依赖这一个 classifier。
只有 filter 明确给出非空 `kinds`，且其中每个 kind 都满足 classifier 时，才下推单值或
多值 `#d`；kindless 或 mixed-kind filter 仍走安全的 post-filter/fallback。测试必须覆盖
数据库中同 kind 对象数大于 limit、目标对象不是最新行的情况。

### 7.2 Mutation event tags

kind `44300` 必须具有且只具有以下两个协议 tag：

```json
[
  ["-"],
  ["t", "buzz-project-view-mutation"]
]
```

首版拒绝：

- 缺失或重复的 NIP-70 `-` protected tag；
- 任意 `h` tag；
- `d`、`e`、`a`、`p` 等会产生另一种所有权或引用解释的 tag；
- 重复或非精确 `t` tag；
- 任何其他额外 tag；
- channel-scoped API token。

NIP-70 tag 标记这是只应提交给当前 authenticated Community Relay 的受保护命令，避免
第三方把成员签名命令搬运到另一个 Relay；它本身不提供 NIP-09 不可删除语义，第
11.5/13.3 节仍需领域删除保护。

操作类型、对象类型和 ID 只从经过强类型解析的 content 取得，不维护一份可与 content
冲突的 tag 副本。

### 7.3 通用 mutation 信封

```json
{
  "schema_version": 1,
  "expected_project_revision": 12,
  "request": {
    "type": "update",
    "object_type": "issue",
    "object_id": "7a80c3c2-73d2-4b58-b43d-10947609f9df",
    "patch": {
      "status": "in_progress",
      "planned_in_stage_id": "43a642c4-b3aa-4216-bd76-a59c0730a91a"
    }
  }
}
```

规则：

- `schema_version` 是 wire schema，不是 DB migration 版本；
- `expected_project_revision` 对所有操作必填；
- wire 上 revision/generation 使用 JSON number，并统一限制为
  `0..=9_007_199_254_740_991`（JavaScript safe integer）；Rust/SQL 也执行同一上界
  CHECK，避免未来 Desktop 读取时发生精度损失；
- 未知 major schema 明确拒绝；
- projection/read payload 在 v1 内可以增加可选字段，旧 reader 必须忽略；
- 已知字段类型错误必须拒绝，不能静默忽略；
- mutation 输入是封闭 schema，使用 `deny_unknown_fields`，避免字段拼写错误被静默
  接受；因此任何新的 client-to-server 字段或枚举语义都必须使用新 schema version 和
  capability，不能在 v1 mutation 中悄悄追加“可选字段”；
- projection/read model reader 忽略未知可选字段；
- 新的写入语义使用新的 capability 或 schema version，不让旧 Relay 猜测执行；
- event content 使用规范 JSON 序列化，但签名权威仍是完整 Nostr event ID。

### 7.4 四类操作

#### Initialize

```json
{
  "schema_version": 1,
  "expected_project_revision": 0,
  "request": {
    "type": "initialize",
    "profile": {
      "name": "Buzz",
      "positioning": "...",
      "purpose": "...",
      "problem": "...",
      "scope": "..."
    },
    "goals": [
      {
        "id": "d90bfa24-22df-4602-9a39-7da5a5c09561",
        "title": "...",
        "desired_outcome": "...",
        "directions": []
      }
    ]
  }
}
```

约束：

- 只允许 Project View 尚未初始化时执行；
- Profile ID 由 Relay 设为 Community UUID；
- 必须一次创建 Profile 和 `1..=32` 个 Goal；
- 整次初始化只增加一次 project revision，结果 revision 为 `1`；
- 任一 Goal 非法时全部回滚。

#### Create

```json
{
  "schema_version": 1,
  "expected_project_revision": 12,
  "request": {
    "type": "create",
    "object": {
      "object_type": "plan",
      "id": "773c47e0-c189-47c5-8d9f-0870a2b2a465",
      "title": "MVP",
      "description": "...",
      "status": "active",
      "under_goal_id": null
    }
  }
}
```

约束：

- Project View 必须已经初始化；
- ID 必须是 UUID v4，且从未在本 Project 使用；
- tombstoned ID 不可复用；
- Profile 不能通过 Create 创建；
- 所有非空关系在同一事务内验证。

#### Update

Update 使用对象类型专属 patch，不使用 JSON Patch，也不做完整对象替换。

语义：

- 字段缺失：保持不变；
- nullable 字段为 `null`：显式清空；
- nullable 字段为值：设置或替换；
- 非 nullable 字段为 `null`：拒绝；
- `id`、`object_type`、创建信息不可修改。
- 空 patch 或应用后与当前对象完全相同的 patch 拒绝为
  `invalid:project_view:no_changes`，不制造空 revision。

这样旧客户端不会因完整覆盖而删除自己不认识的新可选字段。

#### Delete

```json
{
  "schema_version": 1,
  "expected_project_revision": 12,
  "request": {
    "type": "delete",
    "object_type": "resource",
    "object_id": "ca09c37f-bcdf-44d4-9e8d-7db890aab4cf"
  }
}
```

首版 Delete 是逻辑 tombstone：

- 不提供 restore；
- 不提供普通 API hard delete；
- ID 永久保留且不可复用；
- tombstone projection 不包含原业务正文；
- mutation event 仍是历史证据，因此 Delete 不是隐私擦除；
- 即使使用 tombstone，首版仍要求先清理所有 active 入向引用；
- Profile 永远不能 Delete；
- 最后一个 active Goal 不能 Delete；
- 不发生任何隐式级联。

如果要替换最后一个 Goal，先提交创建替代 Goal 的合法命令，再删除旧 Goal。每次成功
提交后的状态都必须合法。

### 7.5 首版不支持通用 batch

除 Initialize 外，一个 mutation 只改变一个规范对象。

原因：

- 项目级 revision 已经提供串行、可重放的简单变更序列；
- 单对象 typed patch 更容易审计、授权、冲突处理和生成 projection；
- 当前关系的移动都可以在一个对象 patch 中原子替换外键；
- 不提前引入 operation ordering、部分失败或任意图变换语义。

真实使用证明需要多对象原子业务动作后，再增加明确命名的领域命令，而不是开放任意
JSON operation batch。

## 8. 输入限制与安全

Project View mutation 使用比 Buzz 通用 256 KiB event 限制更小的领域限制：

| 内容 | 限制 |
|---|---:|
| mutation content | 64 KiB |
| `name` / `title` | 256 UTF-8 bytes |
| 单个长文本字段 | 32 KiB |
| 字符串列表 | 最多 64 项 |
| 列表单项 | 512 UTF-8 bytes |
| Resource locator | 4096 UTF-8 bytes |
| Initialize goals | 1–32 |
| JSON 嵌套深度 | 16 |

其他规则：

- trim 后必填文本不能为空；
- 服务端不自动抓取 Resource URL，不产生 SSRF 面；
- URL locator 不允许 userinfo、内嵌密码或控制字符；
- Repository Resource 优先使用 NIP-34 address；
- Project View 是 Community 全员可读区，禁止写入私钥、token、密码和受限系统凭据；
- 日志只记录 community、event ID、object type、object ID、revision 和错误 code，
  不记录正文或完整 locator；
- 错误不能透露另一个 Community 是否存在同 ID 对象。

## 9. Relay 签名当前态 projection

### 9.1 为什么不直接使用成员 NIP-33 event

Buzz 的普通 parameterized-replaceable key 是：

```text
(pubkey, kind, d_tag)
```

如果 Human 和多个 Agent 分别发布同一对象，会形成多个作者 head，而不是一个 Project
当前态。Project View 需要先验证项目关系和 project revision，再由 Relay 发布唯一
权威读取投影，因此不能把普通成员 NIP-33 LWW 当作领域状态。

### 9.2 Projection 坐标

kind `40904` 的 meta `d` tag 与 kind `40903` 的 object `d` tag：

```text
Project meta:
  project-view:<community_uuid>:meta

Object:
  project-view:<community_uuid>:<object_type>:<object_uuid>
```

Community UUID 必须进入坐标，因为当前 Buzz 进程使用一个 Relay signer 服务多个
Community。数据库本身仍以服务端 `community_id` 隔离，客户端不能提交这个值。

每个坐标在当前 Community 的 events 中最多保留一个未退休 projection。替换由
Project View 专用事务按 revision 完成，不调用普通 NIP-33 replacement helper。
旧 projection 通过 `events.deleted_at` soft-retire，普通 REQ、COUNT 和 search 不再
返回它；Project View 历史以 accepted command 和 mutation receipt 为准，不承诺查询
旧 projection。

### 9.3 Object projection

Object projection 使用 kind `40903`。

active object tags：

```json
[
  ["-"],
  ["d", "project-view:<community>:issue:<id>"],
  ["t", "buzz-project-view"],
  ["t", "buzz-project-view-active"],
  ["type", "issue"],
  ["projection_generation", "1"],
  ["revision", "4"],
  ["project_revision", "18"],
  ["e", "<source-command-event-id>", "", "source"]
]
```

content：

```json
{
  "schema_version": 1,
  "projection_type": "object",
  "project_id": "<server-resolved-community-uuid>",
  "projection_generation": 1,
  "project_revision": 18,
  "object_revision": 4,
  "source_event_id": "<command-event-id>",
  "deleted": false,
  "object": {
    "id": "...",
    "object_type": "issue",
    "created_at": "...",
    "updated_at": "...",
    "created_by": "...",
    "updated_by": "...",
    "data": {},
    "relations": {}
  }
}
```

tombstone 使用同一 `d` 坐标，替换 active head：

- 保留公共 `buzz-project-view` tag；
- 使用 `buzz-project-view-tombstone`，不再使用 active tag；
- content 只保留 project/object ID、类型、revision、删除时间和 source event ID；
- 不携带被删除对象的原业务正文。

kind `40903` 是 Buzz 自定义 current-state event，不会获得通用 NIP-33 客户端的自动
替换语义。SDK 必须按 `d` 坐标维护唯一 head：

- projection generation 更高时丢弃上一代全部本地 head并重新快照；
- 同 generation 内 object revision 更高者替换更低者；
- generation 和 object revision 相同但 event ID 不同视为完整性错误，并根据 meta
  精确补读；
- tombstone 替换本地 active 对象；
- 低 project/object revision 的乱序 live event 直接忽略。

### 9.4 Meta projection

Meta projection 使用 kind `40904`，因此查询 meta 不依赖可能在 SQL `LIMIT` 之后才
执行的 `#t` 过滤。

meta tags：

```json
[
  ["-"],
  ["d", "project-view:<community>:meta"],
  ["t", "buzz-project-view"],
  ["t", "buzz-project-view-meta"],
  ["projection_generation", "1"],
  ["project_revision", "18"],
  ["e", "<source-command-event-id>", "", "source"]
]
```

meta 在每次成功 mutation 时更新，content 至少包含：

```json
{
  "schema_version": 1,
  "projection_type": "meta",
  "project_id": "<community-uuid>",
  "initialized": true,
  "projection_generation": 1,
  "project_revision": 18,
  "active_object_count": 47,
  "reset": false,
  "changed_heads": [
    {
      "coordinate": "project-view:<community>:issue:<id>",
      "event_id": "<new-projection-event-id>",
      "object_revision": 4,
      "deleted": false
    }
  ],
  "source_event_id": "<command-event-id>",
  "updated_at": "..."
}
```

作用：

- 给完整读取提供 project revision 和数量校验；
- 给实时订阅说明本 revision 应出现哪些 object head；
- 在 Redis 乱序或漏发时，让客户端发现 gap 并精确补读或重新快照；
- 避免把“查询刚好返回 N 条”误认为“Project 只有 N 个对象”。

`active_object_count` 包含 Profile 和所有 active 对象，不包含 tombstone。

未初始化 Community 没有 meta projection。客户端在确认 NIP-11 capability 存在后，将
“无 meta”解释为 `initialized: false`，而不是一个非法空 Project View。

### 9.5 Projection 签名者

projection 必须由当前 Relay key 签名：

- kind `40903` 和 `40904` 都加入 `is_relay_only_kind()`；
- 客户端从 NIP-11 `self` 取得预期 pubkey并验证签名；
- 同一部署的所有 Relay Pod 必须共享稳定的 `BUZZ_RELAY_PRIVATE_KEY`；
- projection content、tags、`d` 坐标和服务端 Community 必须相互一致。

Relay key 轮换会使旧 projection 不再由当前 `self` 签名。实现必须：

1. 在 `project_view_state` 保存 `projection_pubkey`；
2. 每个普通 mutation transaction 都断言 `projection_pubkey` 等于本 Pod signer，且当前
   generation 已 ready；不满足时返回可重试的 `unavailable:project_view:signer`，绝不能
   用旧 key 把 head 写回；
3. 由 `buzz-admin project-view reproject` 按 Community lock 重新签名 meta 和
   active/tombstone heads；
4. 重签增加独立的 `projection_generation`，但不增加领域 project revision；
5. 新 meta 设置 `reset: true`，要求长驻 reader 丢弃旧 generation 并重新快照；
6. 当前 signer 的投影准备完成前，不宣告该 Community 的 Project View capability。

`reset: true` 的 meta 不需要把所有对象列入 `changed_heads`；generation 变化本身就是完整
失效信号。首版在一个取得 Community lock 的维护事务内完成重签和 generation 切换；
对象规模证明需要后台分批后，再设计不可见 staging generation。

轮换不能靠多个 Pod 自己“发现后修复”。可执行顺序固定为：

1. 在数据库中心开关中全局停用 Project View，并确认所有 Pod 不再宣告 capability；
2. 等待在途 mutation 完成，确认旧 signer 不再有写入者；
3. 把所有 Pod 滚动到同一个新 `BUZZ_RELAY_PRIVATE_KEY`，期间保持停用；
4. 只用新 Secret/key file 运行 reproject，并用 expected public key 验证 generation、
   事件签名和 NIP-11 `self`；
5. 确认全部 Pod 使用同一 signer 并 ready 后，再原子启用。

这避免旧、新 key Pod 交替重签或在负载均衡后给出不同 `self`。测试必须包含两个不同
signer 并发尝试 mutation/reproject，证明旧 signer fail closed。

首版不要求新增 per-community 私钥基础设施；Community UUID 已进入坐标。未来如果 Buzz
提供 per-community signer，可保持 content 语义并通过新协议版本迁移。

## 10. 读取协议

### 10.0 成员读取门禁

`44300`、`40903`、`40904` 都是 Community-global event，不能沿用
“`channel_id IS NULL` 即公开”的默认路径。新增共享 classifier：

```rust
is_project_view_protocol_kind(kind)
```

以及一次请求只计算一次的 `ProjectViewReaderAccess`。访问成立当且仅当：

- requester 是当前 Community 的直接 `relay_members` 成员；或
- requester 是已验证并持久化 owner 关系的 managed Agent，且其 owner 是当前成员；
- requester 的 `AuthContext.channel_ids` 必须为 `None`；
- requester 的 scopes 包含 `MessagesRead`；为保持 Buzz 现有 WS 兼容语义，空 scopes
  也视为 unrestricted，非空但缺少 `MessagesRead` 才拒绝。
- requester 未被当前 Community ban；managed Agent 的 owner 也未被 ban。

该判断始终执行，不受 `BUZZ_REQUIRE_RELAY_MEMBERSHIP=false` 影响。实现需要把它接入：

- WS `REQ` 的 filter admission、历史结果门和 subscription 注册；
- WS `COUNT` 的精确计数路径；
- HTTP `/query`、`/count` 和 NIP-50 search 的 filter/result gate；
- 本节 revision-pinned pagination；
- 本地 post-commit fan-out 和 Redis cross-Pod fan-out；
- 按 ID 查询或 mixed-kind filter 等不能只靠 filter 预判的读取路径。

仅命中 Project View kind 的 filter 对非成员返回 `restricted`；mixed-kind 查询必须静默
剔除 Project View 结果，COUNT 走可排除这些 kind 的 SQL 或 per-event fallback，不能泄露
存在数量。首版不支持 Project View 的 NIP-50 搜索；显式 search + Project View kind
返回 `unsupported`，其他搜索仍以 result gate 作为防御兜底。

live fan-out 不能信任“订阅注册时曾经是成员”。发送 chokepoint 按一批 recipient pubkey
使用 writer DB 重新确认直接成员或 managed-agent owner membership 与 ban 状态，查询
失败时 fail closed；因此成员被移除或 ban、但旧 socket 尚未断开也收不到后续 Project
View event。timeout 仍沿用 Buzz 现有 write-only 语义，不阻止读取。

读取谓词必须直接接收完整 `AuthContext`，不能只传 scopes：`MessagesRead` 与
`channel_ids` 是两个独立字段。写入不采用“空 scopes unrestricted”的读取兼容规则，
仍由现有 `required_scope_for_kind(44300) == MessagesWrite` 精确校验，并额外要求
`channel_ids.is_none()`。

### 10.1 普通 Nostr 查询

以下查询继续使用标准 Buzz `/query` 或 WS `REQ`：

```json
{"kinds":[40904],"authors":["<nip11-self>"],"limit":2}
```

```json
{
  "kinds":[40903],
  "authors":["<nip11-self>"],
  "#d":["project-view:<community>:issue:<id>"],
  "limit":2
}
```

实时订阅使用两个 filter：

```json
{"kinds":[40903,40904],"authors":["<nip11-self>"]}
```

订阅必须同时接收 object active/tombstone 和 meta；只订阅 active tag 会漏掉删除通知。
Point read 使用 limit `2` 是为了检测数据库中不应存在的重复 active head；返回两个时
客户端必须报完整性错误，不能自行按时间猜一个。

SDK 必须从 NIP-11 `self` 填入唯一 `authors`，并验证返回签名。这样现有
`(community_id, kind, pubkey, d_tag, ...)` event index 可以支持 point read；10k 对象
EXPLAIN 测试若仍不能稳定使用索引，再在 `0025` 增加仅覆盖 `40903/40904` 的
`(community_id, kind, d_tag, pubkey)` partial index，不能凭猜测给整个 events 表增加
宽索引。

### 10.2 完整视图分页

普通事件查询有返回上限，不能假设一个 Project 永远少于 1000 或 2000 个对象。完整读取
使用现有 HTTP `POST /query` 的 raw-filter 扩展：

```json
[
  {
    "kinds": [40903],
    "authors": ["<nip11-self>"],
    "#t": ["buzz-project-view-active"],
    "limit": 500,
    "buzz_project_view": {
      "revision": 18,
      "projection_generation": 1,
      "after": {
        "object_type": "issue",
        "object_id": "..."
      }
    }
  }
]
```

规则：

- 只允许 HTTP `/query`；WS parser 当前不会保留未知 raw filter 字段；
- 一个请求只允许一个 Project View 扩展 filter，不能与 search、feed 或 channel-window
  filter 混用；
- outer filter 使用严格 allowlist：`kinds` 必须恰为 `[40903]`、`authors` 必须恰为当前
  NIP-11 `self`、`#t` 必须恰为 `["buzz-project-view-active"]`，除 `limit` 和
  `buzz_project_view` 外不得出现 `ids`、`#d`、`since`、`until`、`page`、
  `before_id` 或其他会被扩展 handler 忽略的字段；
- `buzz_project_view` 使用 `deny_unknown_fields`；`after.object_type` 必须是已知对象
  enum，`after.object_id` 必须是规范 UUID；
- `revision` 必须来自刚读取的 meta；
- `projection_generation` 必须来自同一份 meta；
- `limit` 为 `1..=500`；
- `after` 为空表示第一页；
- keyset 顺序固定为 `(object_type ASC, object_id ASC)`；
- bridge 在普通 catchall query 之前识别该扩展，并从 `project_view_objects` 的
  community-leading index 取得 projection event IDs；
- 每一页从 writer pool 读取，并在短事务中取得 Community 的 shared advisory lock；
- 如果当前 revision 或 projection generation 不等于请求值，返回 HTTP `409`；
- 结果仍是标准的 Relay 签名 Nostr event 数组；
- 任一 projection event ID 缺失都返回 500 并触发一致性告警，不能把缺失行当作页尾；
- 返回少于 limit 时到达末页；
- 客户端最后再次读取 meta，要求 revision 未改变且对象数与 meta 相等。

这一扩展只解决完整性和分页，不在查询时合成对象 projection。规范 projection 已在写入
事务中持久化。

### 10.3 一致快照算法

`buzz project-view get`：

1. 查询并验证 meta，得到 generation `G`、revision `R` 和 active count `N`；
2. 以 `(G, R)` 分页读取全部 active object projection；
3. 校验每个 projection：
   - Relay 签名；
   - Community 和坐标；
   - 唯一 object ID；
   - object revision 合法；
   - `projection_generation == G`；
   - `project_revision <= R`；
4. 再次查询 meta；
5. 只有前后 generation/revision 都为 `(G, R)` 且对象数为 `N` 时才组装 read model；
6. 中途收到 `409` 或校验不一致时，最多进行有退避的有限重试；
7. 多次变化后返回明确 conflict，不输出貌似完整的混合快照。

对象没有在每次项目 mutation 时全部重发，因此旧对象的 `project_revision` 可以小于
当前 meta revision；它表示该对象最后变化时的项目 revision。

### 10.4 实时启动顺序

未来 UI 或长驻 Agent 需要“快照 + 实时”时：

1. 先打开 kinds `40903,40904` 的 WS subscription 并暂存消息；
2. 通过 HTTP 取得一致快照 generation `G`、revision `R`；
3. 丢弃 generation 低于 `G`，以及同 generation 内 `project_revision <= R` 的消息；
4. 对每个后续 `(generation, project_revision)` 建立 buffer，同一 revision 的 object 和
   meta 不能互相去重；
5. 即使 meta 先到，也必须等 `changed_heads` 中每个精确 event ID 都已收到或按 ID 补读，
   才原子应用该 revision，并把 `applied_revision` 前移；
6. object 先到时先暂存；更高 revision 先到时也暂存，不能越过缺失 revision；
7. revision 出现 gap、声明的 head 补读失败、generation 改变或连接重建时，重新取得快照。

实时 EVENT 的到达顺序不是正确性来源，project revision 和 meta 才是。

### 10.5 逻辑 read model

`buzz-project-view::read_model` 把扁平对象组装为：

```text
ProjectView
├── ProjectProfile
├── Goals[]
│   └── Plans[]
│       └── Stages[]
│           ├── Requirements[]
│           │   └── Works[]
│           └── Issues[]
│               └── Works[]
├── UnboundPlans[]
├── UnplannedRequirements[]
├── UnplannedIssues[]
├── Roles[]
├── Resources[]
└── IssueReferencesByTarget
```

组装器必须：

- 不沿 Issue `about` 递归嵌套；
- 同一个 Issue 完整出现一次；
- `about` 目标只获得反向引用；
- Work 只完整显示在它处理的 Requirement 或 Issue 下；
- 发现非法或缺失关系时失败，不静默把对象放到“未规划”区。

## 11. PostgreSQL 设计

### 11.1 基本原则

- 使用 Buzz 现有 Postgres 和连接池；
- 不新增数据库实例或 schema；
- 所有表都以 `community_id UUID NOT NULL` 开头；
- 所有 PK、UNIQUE、FK 和唯一索引都以 `community_id` 为第一列；
- 不加入 operator-global table allowlist；
- 唯一例外是给现有 operator-global `communities` 路由行增加一个 Community 自身的
  `project_view_enabled` 布尔属性；它不是 Project View 对象数据，也不新增 global 表；
- 不从 Project View 表 FK 到分区的 `events` 表；
- 不使用 `ON DELETE CASCADE`；
- 强一致读取、锁和事务只走 writer pool；
- migration 只做增量建表/索引和一个带常量默认值的小表列变更，不回填 Project View
  对象。

### 11.2 `communities.project_view_enabled`

`0025` 给现有 `communities` 增加：

```text
project_view_enabled BOOLEAN NOT NULL DEFAULT FALSE
```

它是所有 Relay Pod 共用的中心化 capability/write 开关：

- 新旧 Community 初始都为 `false`；
- `buzz-admin project-view enable|disable --community <host>` 先取得与 mutation 相同的
  exclusive Project View advisory lock，再更新一个 Community；
- `--all` 在一个 SQL transaction 中按 Community UUID 稳定顺序取得全部 exclusive
  locks，再更新所有非归档 Community，避免与在途 mutation 交错或多 admin 死锁；
- mutation transaction 在取得 Community advisory lock 后从 writer DB 重新读取它；
- NIP-11 用同一 DB 真值计算该 host 的 capability；
- disable 只停止新 mutation 和 capability 宣告，不删除状态或绕过成员读权限。

不能用 Pod-local 环境变量替代这个列。环境变量随 Kubernetes 滚动更新会产生一部分 Pod
开启、一部分关闭的窗口；同一用户可能从某 Pod 读到 capability，却把写入发到另一个
拒绝该 kind 的 Pod。

### 11.3 `project_view_state`

逻辑结构：

```text
community_id             UUID       PK/FK communities
project_revision         BIGINT     NOT NULL
active_object_count      INTEGER    NOT NULL
initialized_at           TIMESTAMPTZ NOT NULL
updated_at               TIMESTAMPTZ NOT NULL
last_event_id            BYTEA      NOT NULL
last_actor_pubkey        BYTEA      NOT NULL
meta_projection_event_id BYTEA      NOT NULL
projection_pubkey        BYTEA      NOT NULL
projection_generation    BIGINT     NOT NULL
```

没有行表示未初始化。初始化后 revision 从 `1` 开始。

### 11.4 `project_view_objects`

逻辑结构：

```text
community_id              UUID
object_id                 UUID
object_type               TEXT
schema_version            SMALLINT
object_revision           BIGINT
project_revision          BIGINT
body                      JSONB

under_goal_id             UUID NULL
under_plan_id             UUID NULL
planned_in_stage_id       UUID NULL
about_object_id           UUID NULL
about_object_type         TEXT NULL
handles_object_id         UUID NULL
handles_object_type       TEXT NULL

created_at                TIMESTAMPTZ
updated_at                TIMESTAMPTZ
created_by                BYTEA
updated_by                BYTEA
source_event_id           BYTEA
projection_event_id       BYTEA
deleted_at                TIMESTAMPTZ NULL

PRIMARY KEY (community_id, object_id)
```

设计选择：

- `body` 只保存对象自身字段，不保存关系、归属、revision 或 actor；
- 固定关系使用列，避免首版建立通用 relation graph；
- 所有目标 ID 使用 `(community_id, object_id)` 复合自外键；
- objects 与 mutations 使用 `(community_id)` DEFERRABLE FK 指向
  `project_view_state(community_id)`，保证不存在“有领域行但无 Project state”；
- FK 为 `NO ACTION DEFERRABLE INITIALLY DEFERRED`；
- source 侧 CHECK 限制哪些类型可以使用哪些关系列；
- revision、schema version、32-byte event ID/pubkey 和 JSON object shape 使用 CHECK；
- Profile 使用 active partial unique index 保证每 Community 至多一个；
- `Profile.object_id = community_id`；
- tombstone 行保留 ID、类型和历史 metadata；
- Rust 事务校验目标类型与 active 状态；
- ordinary row/statement trigger 根据 active object insert 或 active→tombstone 机械地
  `+1/-1` 更新 `project_view_state.active_object_count`；Relay 不能任意写这个计数；
- deferred constraint trigger 作为数据库兜底，验证目标类型、无 active 悬空引用、
  唯一 Profile 和至少一个 active Goal。

每次 mutation 不执行 `COUNT(*)` 全表扫描。计数与行数的全量比对属于定时 integrity
audit、migration 验证和测试；发现漂移时停止 capability 并告警，不能静默修正后继续写。

不拆成九张表，因为 `Issue.about` 和 `Work.handles` 是受限多态引用；也不建立通用
`project_view_relations` 图，因为首版关系固定且简单。

### 11.5 `project_view_mutations`

逻辑结构：

```text
community_id       UUID
event_id           BYTEA
project_revision   BIGINT
actor_pubkey       BYTEA
operation          TEXT
object_type        TEXT NULL
object_id          UUID NULL
result             JSONB
accepted_at        TIMESTAMPTZ

PRIMARY KEY (community_id, event_id)
UNIQUE (community_id, project_revision)
```

用途：

- accepted mutation 的幂等收据；
- event ID 重试时，在 actor 仍通过当前安全门的前提下返回完全相同的成功结果；
- 按 project revision 定位变更；
- 支持运维检查和未来重建。

只记录 accepted mutation。校验失败和 CAS conflict 不写 event、对象或 receipt。

不额外发布 receipt event。成功的成员签名 command 已经持久化，meta/object projection
明确引用它；DB receipt 只服务幂等执行和标准写响应，避免首版再增加一个公共协议对象。

accepted kind `44300` 是领域命令，不允许通过 NIP-09 删除，也必须排除在未来通用 event
retention/prune 之外。删除不等于撤销；业务撤销只能是新的 typed mutation。NIP-09
side-effect 在发现任一目标是已接受的 Project View command 时，拒绝整个删除请求，
不能存一条看似成功但没有效果的 delete event。这样 `source_event_id`、receipt 和未来
重建始终指向可验证的原始签名事件。

### 11.6 索引

至少增加：

```text
(community_id, object_type, object_id) WHERE deleted_at IS NULL
(community_id, under_goal_id) WHERE deleted_at IS NULL
(community_id, under_plan_id) WHERE deleted_at IS NULL
(community_id, planned_in_stage_id) WHERE deleted_at IS NULL
(community_id, about_object_id) WHERE deleted_at IS NULL
(community_id, handles_object_id) WHERE deleted_at IS NULL
(community_id, source_event_id)
(community_id, project_revision)
```

所有索引先以 `community_id` 缩小租户边界。首版不增加新的搜索引擎、向量库或 JSONB
通用 GIN 索引。

## 12. 原子事务与并发

### 12.1 事务 API

推荐 DB API：

```rust
let mut tx = db.begin_project_view_write(community_id).await?;
let receipt = tx.find_receipt(command_event.id).await?;
let current = tx.load_current_for(&mutation).await?;
let applied = domain.apply(current, mutation, actor, now)?;
let projections = relay_signer.sign(applied.projection_plan())?;
let outcome = tx
    .commit_mutation(command_event, applied, projections)
    .await?;
```

`ProjectViewWriteTx` 持有 SQLx transaction；Relay 不直接访问其中连接。

### 12.2 精确事务顺序

1. `BEGIN`。
2. 取得稳定的 Community 级 `pg_advisory_xact_lock`。初始化前没有 state 行，因此不能
   只依靠 `SELECT FOR UPDATE`。
3. 从 writer DB 读取 `communities.project_view_enabled`；为 false 时返回 unavailable，
   不查询或改变领域状态。
4. 查询 `(community_id, event_id)` receipt：
   - 已存在：返回原结果，不增加 revision，不重复 fan-out；
   - 不存在：继续。
5. 读取 `project_view_state FOR UPDATE`，并在已初始化时断言保存的
   `projection_pubkey/generation` 对当前 Pod signer 已 ready。
6. 捕获 canonical time：未初始化时取一次 DB `clock_timestamp()`；已有 state 时取
   `GREATEST(clock_timestamp(), state.updated_at + interval '1 microsecond')`。
7. 比较 `expected_project_revision`：
   - Initialize 要求 state 不存在且 expected 为 `0`；
   - 其他操作要求 state 存在且 revision 完全相等。
8. 只加载目标对象、关系目标、入向引用和必要计数，不在每次写入时扫描全 Project。
9. 在纯领域层应用 typed mutation 并验证最终状态。
10. 计算新 project revision、object revision 和 active-count delta。
11. Relay 在事务仍打开时：
   - 先签名发生变化的 object projection，取得确定 event ID；
   - 再把这些 ID 写入 `changed_heads` 并签名新 meta projection。
12. 在同一 SQL transaction 中：
    - 插入成员签名 command event；
    - 插入 mutation receipt；
    - 先为 Initialize 插入 count `0` 的 state，再 upsert 对象或写 tombstone，由 DB
      trigger 应用 active-count delta；
    - 更新 `project_view_state` 的 revision/signer/meta 字段，但普通 UPDATE 不直接赋值
      `active_object_count`；
    - 按对象/state 保存的旧 event ID soft-retire 旧 projection head；
    - 插入新 Relay projection event；
    - 更新对象/state 保存的 projection event ID。
13. 运行 deferred constraints，并断言 trigger 产生的 count 等于领域预期 count。
14. `COMMIT`。
15. 仅提交成功后执行 audit、Redis publish 和本地 fan-out。

任何一步失败都回滚 command event、规范状态、receipt 和全部新 projection。

### 12.3 必须抽取的 event-store helper

`buzz-db` 当前普通 event insert/replacement API 会自行取得连接或开启事务。Project View
需要从 `buzz-db/src/event.rs` 抽取 crate-private 的：

```rust
insert_event_in_tx(...)
retire_projection_head_in_tx(...)
```

它们复用现有事件字段、分区和校验逻辑，但由调用方控制 transaction。

不能采用：

```text
先 persist command event
再通过 pool 更新领域表
```

也不能把领域更新放入普通 ingest 的 best-effort side effect。否则会出现“event 已接受但
Project View 未改变”或“对象已改变但 projection 仍旧”的永久分叉。

### 12.4 CAS 语义

首版选择 project-level CAS：

```text
mutation.expected_project_revision == current.project_revision
```

优点：

- 最简单地保证 Human 与 Agent 不基于过期项目图修改；
- 冲突结果确定；
- mutation log 是严格单调序列；
- meta delta 不需要解释并行分支；
- 所有跨对象不变量都在一个明确版本上验证。

代价是两个无关对象的并发修改也会冲突。首版优先正确、直接和可观察；真实使用证明
冲突率过高后，可在协议 v2 增加 object-level CAS 和合并规则，不能由 Relay 静默覆盖。

### 12.5 同秒更新与多节点

Project View projection 不使用 NIP-33 `created_at` 选 head，因此同一秒多次修改不会按
event ID 错选旧状态。数据库 lock 和 project revision 决定唯一顺序。

多 Relay Pod：

- read/write 共同使用一个集中定义的 namespaced advisory key，例如
  `hashtextextended('buzz_project_view:' || community_id, 0)`；write 取 exclusive，
  snapshot page 取 shared，禁止各模块自己发明 key；
- snapshot page 的 revision/generation 复核、active object pointer 查询和对应 events
  批量取回必须位于同一个 writer-pool transaction 和同一 shared lock 内；
- 使用相同 Relay signer；
- 事务提交后由现有 Redis pub/sub 跨节点传播；
- fan-out 可能乱序，客户端按 project revision 收敛；
- DB 和 projection 是恢复源，Redis 不是权威状态。

### 12.6 Fan-out 失败

Buzz 当前语义是：

> `OK accepted` 表示事件和状态已持久化，不表示每个在线订阅者都已经收到 Redis 消息。

Project View 沿用这一语义：

- Redis publish 失败只影响实时性；
- 重连或 meta revision 检查可以恢复；
- 进程在 commit 后、publish 前崩溃不会丢失规范状态；
- 首版不增加 Project View 专属 transactional outbox。

如果后续项目连续性要求“每个变更都必须可靠触发某项执行”，应把可靠 event outbox
作为 Buzz 平台级能力设计，而不是为 Project View 建一条旁路消息系统。

### 12.7 Projection 保留与容量

`40903/40904` 只是当前态 read model，旧 head soft-retire 后不承担历史语义。Relay
后台 janitor 复用现有进程和 writer pool，按 Community 小批量物理删除
`deleted_at` 已超过 30 天的退休 projection：

- 只处理 kind `40903/40904`；
- 取得同一个 Project View exclusive advisory lock 后重新确认目标仍 retired；
- 排除 `project_view_objects.projection_event_id`、
  `project_view_state.meta_projection_event_id` 和当前 meta `changed_heads` 引用；
- 每批最多 1000 行，避免长事务和 event partition 膨胀；
- 删除失败只告警并重试，不影响当前态；
- 永久保留 kind `44300` accepted command、`project_view_mutations` receipt 和规范对象行。

这也清理 brownfield 数据库中旧 projection content 占用的 FTS/GIN 空间。当前 tombstone
head 在仍被规范 pointer/meta 引用时不能清理；若其数量成为问题，再单独设计
tombstone checkpoint，不在 janitor 中猜测安全性。

## 13. Ingest、鉴权与错误

### 13.1 通用安全门

kind `44300` 必须在以下检查全部通过之后才能进入领域事务：

1. relay-only kind 拒绝；
2. Nostr 签名校验；
3. 时间漂移；
4. event 与领域 content 大小；
5. event pubkey 等于 authenticated principal；
6. kind scope allowlist；
7. Project View 强制 Community membership，不受 open-relay 配置影响；
8. ban 和 timeout write-block；
9. 全局 token 检查；
10. 无 `h` tag；
11. 通用 admission 和 rate limit。

`required_scope_for_kind(44300)` 使用现有 `MessagesWrite`。首版不增加新 scope，避免
Agent harness、token minting 和旧客户端形成第二套授权语义。

所有合法 Community 成员等权修改；Project Role 不参与权限。managed Agent 只有在其
已经验证的 owner 当前仍是成员时才通过。channel-scoped token 即使其 channel 可读写，
也不能操作 Community-global Project View。

### 13.2 修正现有 command 早路由

当前 `is_command_kind()` 路由早于普通 ban/timeout 和 global-token 检查。实现不能只把
`44300` 加入 classifier 后照搬这条路径。

推荐：

- `44300` 加入 `is_command_kind()`，使 Workflow 明确跳过它；
- 增加 `is_project_view_mutation_kind()`；
- 现有早期 command router 暂时排除 Project View；
- Project View 在通用 Community write gates 完成后进入专用 handler；
- 专用 handler 再使用严格成员 helper；不能复用 open relay 时短路成功的
  `enforce_relay_membership()`；
- 加回归测试，证明 banned、timed-out 和 channel-scoped actor 均不能修改 Project View。

未来可以统一重构所有 command 的 gate phase，但不把这个行为变化夹带进 Project View
首版。

### 13.3 Projection 的副作用

command event：

- 通过现有 event-created audit 记录真实 Human/Agent actor；
- 不触发 Workflow。

Relay projection：

- 通过现有 Redis 和本地 subscription fan-out；
- 不再次产生一条 actor 为 Relay 的重复业务 audit；
- 不触发 Workflow；
- 不运行普通 message/thread counter side effect。

可将现有 `dispatch_persistent_event` 抽成带明确 options 的内部 API，避免复制 fan-out
实现。默认行为不变，Project View projection 显式设置：

```text
audit = false
workflow = false
thread_side_effects = false
```

领域 mutation 自身只产生一条可归因的审计记录。

NIP-09 side-effect 还必须把 accepted kind `44300` 视为不可删除领域命令；NIP-09 不得
soft-delete 它。`40903`/`40904` 的退休只允许 Project View transaction 按已知 head ID
执行，成员不能通过 NIP-09 控制 Relay projection。

### 13.4 错误模型

扩展 `IngestError`：

```rust
Conflict(String)
Unsupported(String)
Unavailable(String)
```

映射：

| 情况 | WS | HTTP |
|---|---|---:|
| 字段/tag/关系非法 | `OK false invalid:project_view:<code>` | 400 |
| 非成员、scope、ban/timeout | `OK false restricted:...` | 403 |
| revision 已变化 | `OK false conflict:project_view:revision` | 409 |
| 已初始化/未初始化状态冲突 | `OK false conflict:project_view:<code>` | 409 |
| Relay/wire version 不支持 | `OK false unsupported:project_view:<code>` | 400 |
| DB flag disabled | `OK false unavailable:project_view:disabled` | 503 |
| schema/signer/generation 未 ready | `OK false unavailable:project_view:<code>` | 503 |
| 内部 DB/签名错误 | `OK false error:internal` | 500 |

成功 `message` 沿用 Buzz command 约定：

```text
response:{
  "project_revision":13,
  "object_id":"...",
  "object_revision":2,
  "deleted":false
}
```

receipt 保存这份标准结果。幂等契约分成两层：

- **状态幂等**：相同 event ID 永远不会再次改变对象或增加 project revision；
- **响应重放**：只有请求仍通过当前签名/时间窗口、admission、membership、scope、
  ban/timeout、feature 和 signer readiness 门禁时，才返回 receipt 中完全相同的原成功
  结果。

安全门始终先于 receipt lookup。事件已超出 ingest 时间窗口、成员已被移除、actor 已被
ban，或运维已关闭功能时，重试返回当前 admission/授权/可用性错误，但不会改变原有
状态；不能为了“响应完全相同”绕过新的安全决策。

CLI 把 conflict 映射到既有 exit `5`，unsupported 映射到普通能力错误 exit `4`，
unavailable 映射到 relay/network 类 exit `2`。只有 503/网络不确定结果可在事件时间窗
内进行有界退避并重发同一已签名 event；400、403、409 不自动重试。

`IngestError` 是闭集，不能只改 enum/handler。实现还要同步所有 exhaustive seam：

- `conformance::sanitized_reason_for`：
  `Conflict`/`Unsupported -> SanitizedReason::Invalid`，
  `Unavailable -> SanitizedReason::ServerError`；
- WS `handlers/event.rs`：返回上表 message，并使用低基数 reason label
  `conflict`/`unsupported`/`unavailable`；
- HTTP `api/bridge.rs`：分别产生 409/400/503，内部 error 内容仍脱敏；
- ingest rejection metrics、audit/conformance trace 和 HTTP attribution；
- 每个 variant 的 WS、HTTP、metrics label 与 `SanitizedReason` exhaustive test。

## 14. NIP-11 能力发现

Relay 增加：

```json
{
  "supported_extensions": [
    "nip-er",
    "buzz-project-view-v1"
  ]
}
```

只有同时满足以下条件才宣告：

- 当前 Host 已绑定到唯一 Community；
- 该 Community 的 `communities.project_view_enabled = true`；
- catalog probe 证明 Project View 表、索引和必要列就绪；
- Relay 使用显式配置、跨 Pod 稳定的 signer；
- signer rotation repair 已完成或当前没有需要修复的 Project View。

这里不能只检查 `_sqlx_migrations` 的最大版本。Buzz 的部分 CI/本地流程直接应用
`schema/schema.sql`，不会建立等价的 migration ledger。运行时 schema readiness 使用
`to_regclass`、`pg_attribute` 等 catalog/行为 probe；独立 migration gate 才检查
`_sqlx_migrations` 版本与 checksum。

能力开关以同一 Postgres 中的 `communities.project_view_enabled` 为唯一真值，由：

```text
buzz-admin project-view status --community <host>
buzz-admin project-view enable --community <host>
buzz-admin project-view disable --community <host>
buzz-admin project-view enable|disable --all
buzz-admin project-view reproject --community <host>|--all \
  [--relay-key-file <restricted-path>] --expected-pubkey <hex>
```

操作。enable/disable 与 mutation 使用同一个 namespaced advisory lock；`--all` 也只
提交一次。mutation 自身再次读取该列，因此 disable 成功提交后，不可能还有一个先读到
true 的旧 mutation 随后提交。不需要第二次 Deployment rollout，也没有 Pod-local
开关漂移。

`enable` 不是盲写 boolean：它在同一 lock 内先验证 catalog、显式稳定 signer，以及该
Community 当前 projection pubkey/generation/read-model 一致性；未初始化 Community
只需 schema 与 signer ready。`--all` 先对全部目标完成 preflight，再一次提交全部 flag，
任一失败则全部不启用。

该列是 rollout/write kill switch，不是保密开关；已有 projection 始终受第 10.0 节的
强制成员读取门禁保护。

Kubernetes `/_readiness` 是无 Host 的 deployment-global probe，不能被单个 Community
拖垮。它继续检查全局 DB/Redis；Project View schema 完全 absent 且尚不可 enable 时，
仍允许新 binary 处理旧功能，支持“先升级 binary、后迁移”。一旦 0025 已安装并存在
enabled Community，partial/broken catalog 或本 Pod 未配置稳定 signer 属于
deployment-global 错误，可以令该 Pod not-ready。若只是某个 Community 的 projection
generation/read model 不 ready，则只让该 Host 不宣告 capability、mutation 返回 503，
并产生告警；其他 Community 和整个 Pod 继续 ready。所有 Community disabled 时，旧功能
也正常 ready。

CLI 在写入前检查 capability。旧 Relay 没有 capability 时返回
`unsupported: buzz-project-view-v1`，而不是盲发未知 kind。

NIP-11 当前用编译期 fence 保持 `RelayInfo::build` 为纯 scalar builder。实现不能把
`Db`、`AppState` 或 `TenantContext` 传进 `RelayInfo::build`。正确落点是在
`nip11_document` 中先 bind Host/Community、异步读取上面的 scoped readiness，再把一个
预计算 boolean/scalar 交给 builder；保留并扩展现有 conformance fence，防止未来把
跨租户查询藏进 NIP-11 builder。

## 15. SDK 与 CLI

### 15.1 SDK

`buzz-sdk::project_view` 提供：

```rust
build_initialize(...)
build_create(...)
build_update(...)
build_delete(...)
parse_meta_projection(...)
parse_object_projection(...)
verify_projection(...)
```

Builder 必须：

- 生成精确 tags；
- 保留同一份已签名 event 用于网络重试；
- 不允许调用者传 project/community ID；
- 不提供绕过 typed patch 的 raw JSON builder；
- 对 UUID、字符串限制和枚举先做客户端校验，但服务端仍重复验证。

### 15.2 CLI 命令

```text
buzz project-view get
buzz project-view get-object <type> <id>
buzz project-view init --profile <file|-> --goal <file|-> [--goal <file|-> ...]
buzz project-view create <type> --expected-project-revision <n> --data <file|->
buzz project-view update <type> <id> --expected-project-revision <n> --patch <file|->
buzz project-view delete <type> <id> --expected-project-revision <n>
```

写命令流程：

1. 查询 capability；
2. 从参数取得调用者实际读取过的 expected project revision；
3. 从文件或 stdin 读取 typed data；
4. 生成/保留一个已签名 event；
5. 调用现有 `POST /events`；
6. 只对网络不确定结果重发同一个 event；
7. 409 时返回 `CliError::Conflict`，exit code `5`；
8. 成功后读取新 meta/object 进行确认。

`--format compact` 继续是 Buzz 全局 flag，输出保持现有约定。Create 的 UUID 由 CLI 在
签名前生成，因此即使响应丢失，调用者仍知道对象 ID。

CLI 不默认在写入前偷偷改用“最新 revision”，否则 Agent 基于旧视图形成的意图会绕过
CAS。可提供显式 `--use-latest` 作为 Human convenience，但不能与
`--expected-project-revision` 同时使用。

### 15.3 CLI 输出

默认 JSON 输出包含：

```json
{
  "initialized": true,
  "project_revision": 18,
  "project": {},
  "goals": [],
  "unbound_plans": [],
  "unplanned_requirements": [],
  "unplanned_issues": [],
  "roles": [],
  "resources": [],
  "issue_references_by_target": {}
}
```

Agent 可以一次获取当前一阶项目状态，不必自行遍历命令历史。

## 16. Migration

### 16.1 文件

新增：

```text
migrations/0025_project_view.sql
```

同步更新：

```text
schema/schema.sql
```

原因：

- Relay 运行使用 SQLx migrations；
- 当前测试和部分本地流程直接应用 `schema/schema.sql`；
- 只修改其中一个会让生产升级和 E2E 创建出不同数据库。

### 16.2 Migration 约束

`0025`：

- 给现有小表 `communities` 增加
  `project_view_enabled BOOLEAN NOT NULL DEFAULT FALSE`；
- 创建新表、索引、函数/constraint trigger；
- 不修改 `0001`–`0024`；
- 不回填 Project View 对象；已有 Community 只获得 `false` 常量默认值；
- 不重写大 `events` 表；
- 不修改 `search_tsv` generated expression；
- 已有 Community 自然处于 Project View 未初始化状态；
- 可在短时间内完成并适合滚动发布。

更新 `buzz-db/src/migration.rs`：

- migration 固定数量从 `24` 改为 `25`；
- 增加 0025 内容与 tenant-scoping 断言；
- 保持所有既有 checksum；
- 增加 fresh 和 0024→0025 执行测试。

`communities` 列变更是现有租户路由 metadata 的 additive 变化，不是新基础设施。迁移
测试还要证明旧代码常用的 `SELECT id, host ...` 不受影响，并对 archived Community
保持 disabled。

## 17. 测试设计

### 17.1 领域单元测试

放在 `buzz-project-view`：

- 关系设计文档第 17 节的 21 条验证清单逐条成为命名测试；
- Initialize 原子建立 Profile 和至少一个 Goal；
- typed patch 的 absent/null/value 语义；
- 所有状态枚举 round-trip；
- object type 和 ID 不可变；
- Work handles XOR；
- Issue about self 拒绝；
- 无隐式状态级联；
- deterministic read model；
- Issue 相互 about 时读取有限、不递归；
- 未绑定和未规划对象归组正确。

使用 workspace 已有 `proptest`：

- 随机合法 mutation 序列始终保持不变量；
- 非法 mutation 不改变状态；
- 相同 event 重放状态不变；
- read model 对输入排列不敏感；
- 含 Issue 引用环时仍可有限组装。

### 17.2 SDK 与协议测试

- kind 不重复；
- tags 精确基数；
- mutation schema v1 round-trip；
- projection content/tag/d 坐标一致；
- `40903`/`40904` 的 `d` tag 会进入 `events.d_tag`，其他普通 40xxx kind 不受影响；
- `has_indexed_d_tag` 被写入和 WS/HTTP/COUNT query builder 共同使用；
- 同 kind 行数大于 limit 且目标不是最新行时，`#d` point read 仍精确命中；
- Relay 签名验证；
- 错误 signer、错误 Community、错误 revision 拒绝；
- tombstone 不泄露旧 body；
- unknown major 拒绝；
- mutation unknown field 拒绝，projection unknown optional field 可读取；
- `Conflict/Unsupported/Unavailable` 的 WS、HTTP、metrics 和 SanitizedReason 映射闭集；
- v1 reader/new projection、v2 writer/old Relay 的兼容 fixture 固定，防止无意破坏协议。

### 17.3 DB integration

- 未初始化状态；
- Initialize 全部成功或全部回滚；
- command event、state、objects、receipt、meta/object projection 同事务；
- projection insert 失败时所有写入回滚；
- 两个相同 expected revision 并发写恰好一个成功；
- 相同 event ID 重试不增加 revision；
- 相同 event 在时间窗内且权限不变时重放原 receipt；
- 相同 event 在超时、成员移除、ban 或 feature disable 后被当前 gate 拒绝，但状态不变；
- A/B Community 使用同一 object UUID 可共存；
- 跨 Community 引用拒绝；
- 删除入向引用保护；
- 最后 Goal 和 Profile 保护；
- signer rotation 重投影；
- 旧/new signer 并发时只有 ready signer 可写，旧 signer 不能写回 projection；
- active create/tombstone 的 DB trigger 只产生正确 `+1/-1`，事务内 scalar 断言与领域
  expected count 一致；
- integrity audit 能发现人工注入的 `active_object_count`/active rows 漂移并关闭
  capability；
- 同一 advisory key 下 snapshot shared lock 与 mutation exclusive lock 正确串行；
- 两个 transaction 反序取得锁或 DB clock 回拨时，canonical time 仍随 revision 单调；
- revision-pinned 分页在并发 mutation 后返回 409；
- 10k 对象读取无 N+1。
- janitor 只清理超过保留期且无当前 pointer/meta 引用的 retired projection，永不清理
  command/receipt/current head。

DB 测试必须使用专用临时数据库，不能清理开发者数据库，也不能与运行中的 Relay 共用
`public` schema。

### 17.4 Relay E2E

新增：

```text
crates/buzz-test-client/tests/e2e_project_view.rs
```

核心闭环：

```text
Human 初始化
  -> Agent 通过 HTTP /query 读取
  -> Agent 修改 Issue/Work
  -> Human 的 WS subscription 收到 meta/object projection
  -> 断线重连后得到相同当前态
```

至少覆盖：

- HTTP 与 WS 写入共用行为；
- HTTP snapshot + WS live；
- meta-first、object-first、Initialize 多 head 任意顺序最终只原子应用一次 revision；
- `changed_heads` 漏一个 event ID 时不会推进 revision，并会精确补读或重快照；
- 未初始化读取；
- revision conflict 和 CLI exit 5；
- 相同 event 重试；
- client 提交 kind `40903` 或 `40904` 被拒绝；
- `h` tag 和 channel-scoped token 被拒绝；
- banned、timed-out、scope 不足、非成员被拒绝；
- `BUZZ_REQUIRE_RELAY_MEMBERSHIP=false` 时，非成员仍不能读写 Project View；
- channel-scoped token 不能通过 WS REQ/COUNT、HTTP query/count/pagination 读取；
- WS 空 scopes + `channel_ids=None` 保持 read-compatible unrestricted；非空 scopes
  缺少 `MessagesRead` 或任意 `channel_ids=Some(...)` 都拒绝；
- 移除成员但保留旧 socket 后，本地和 Redis live fan-out 都不再投递；
- mixed-kind 查询和 COUNT 不泄露 Project View event 或数量；
- unknown Host fail closed；
- 两 Community 同 UUID 不串数据；
- Relay restart 后 projection 可读；
- Redis publish 失败后重连查询恢复；
- Project View command/projection 不触发 Workflow；
- NIP-09 不能删除 accepted kind `44300` command；
- conformance trace 每个成功/拒绝路径都产生正确 action。

### 17.5 Migration 与 schema drift

新增独立 migration gate：

1. 临时数据库 A 从零执行全部 SQLx migrations；
2. 临时数据库 B 应用 `schema/schema.sql`；
3. 比较规范化 schema 或要求双向 desired-state plan 为 no-op；
4. 从只含 0024 的数据库升级到 0025；
5. 验证既有 Community、event 和 membership 不变；
6. 验证既有/新建 Community 的 Project View 开关默认 false；
7. 两个 migrator 并发运行仍安全；
8. `schema/schema.sql` 直接创建、没有 `_sqlx_migrations` ledger 的数据库也能通过运行时
   catalog readiness probe；
9. migration 25 数据库上以前一版 Relay image、`BUZZ_AUTO_MIGRATE=false` 启动旧功能的
   rollback smoke 通过。

现有真正执行 migrations 的测试多为 ignored，Project View 不能只依赖静态 SQL 文本
检查。

### 17.6 性能与故障基线

非阻塞 benchmark 或记录型测试：

- 10k active objects 的完整 snapshot；
- 单次对象更新；
- 大量 Issue about 反向查询；
- 删除引用检查；
- meta/project revision 高频更新。

观察：

- SQL round-trip 数；
- 是否出现逐对象查询；
- query plan 是否使用 community-leading index；
- transaction lock 等待；
- projection 签名耗时；
- snapshot 重试率。

## 18. Just 与 CI

新增：

```text
just project-view-test-unit
just project-view-test-db
just project-view-test-e2e
just project-view-test
just test-migrations
```

recipes 的实际覆盖不能只靠名字约定：

```bash
# project-view-test-unit
cargo nextest run -p buzz-project-view
cargo nextest run -p buzz-core --lib
cargo nextest run -p buzz-sdk --lib
cargo nextest run -p buzz-relay --lib
cargo nextest run -p buzz-cli --lib

# project-view-test-db；CI 为它创建独占临时 DB
cargo nextest run -p buzz-db --test project_view --test-threads 1

# test-migrations；使用另一组独占 fresh/upgrade DB
cargo nextest run -p buzz-db --test migrations --test-threads 1

# project-view-test-e2e；由脚本启动/复用 CI Postgres、Redis 和 Relay
cargo build -p buzz-cli
cargo nextest run -p buzz-test-client --test e2e_project_view --test-threads 1
```

如果本机没有 nextest，Just recipe 以对应 `cargo test` 命令回退。新增
`scripts/test-project-view-e2e.sh` 负责等待服务、seed Community/direct member、通过
`buzz-admin` 开启 DB flag、运行真实 `target/debug/buzz`，并在退出时只清理它创建的
临时资源。

接入规则：

- 现有 `test-unit` 当前没有完整运行 Relay、SDK 和 CLI；必须显式调用
  `project-view-test-unit`，并同步修改 `scripts/run-tests.sh unit` fallback；
- 因此自动进入 `just ci` 和 pre-push；
- DB、migration 和真实 Relay E2E 留在带 Postgres/Redis 的 CI job；
- `e2e_project_view` 加入 nextest archive 和 backend integration；
- 保留 fmt、Clippy `-D warnings`、unit、cargo-deny 等既有 gate；
- 新 public API 有 doc comments；
- 不引入 `unsafe`；
- production path 不新增 `unwrap()` 或 `expect()`。

CI job 分工：

1. **unit**：运行修改后的 `just test-unit`，无需基础设施。
2. **backend integration**：继续按现状直接应用 `schema/schema.sql`，seed 唯一
   Community 和 `relay_members`，配置一个确定性的 CI-only
   `BUZZ_RELAY_PRIVATE_KEY`，用 `buzz-admin project-view enable` 开启该 Community，
   然后运行 DB、Relay 和真实 `buzz-cli` E2E。capability 必须在没有
   `_sqlx_migrations` ledger 时也能由 catalog probe 正确出现。
3. **migrations**：独立执行 fresh、0024→0025、schema drift、并发 migrator 和 rollback
   smoke。这个 job 必须是 required check，不能只把 Postgres tests 留为 `#[ignore]`。
4. **package/deploy contract**：验证 Relay image 含 `buzz-admin`，Chart 使用稳定 Relay
   key，并确认不存在 Pod-local `BUZZ_PROJECT_VIEW_ENABLED`。启用动作仍由部署后 admin
   命令完成，不由 Helm rollout 隐式完成。

还需要修正：

- `.github/workflows/docker.yml` 的 PR path filter 加入 `migrations/**` 和 `schema/**`；
- `crates/buzz-project-view/`、`schema/` 和协议文档加入 Relay release changelog 的
  `LOG_PATHS`；
- `crates/buzz-cli/`、`crates/buzz-admin/`、`deploy/charts/buzz/` 和
  `scripts/test-project-view-e2e.sh` 加入对应 workflow/path filter；
- Chart values/schema、模板渲染测试和 Compose README 说明“先迁移和升级全部 Pod，再用
  admin 开启”，但不暴露一个会造成 mixed-Pod 状态的 env flag；
- `Cargo.lock` 随 workspace crate 更新；
- migration 和 `schema/schema.sql` 的 drift 检查进入 CI。

实现代码提交前按仓库要求运行：

```text
. ./bin/activate-hermit
just ci
just test
just test-migrations
```

其中 `just test` 适用于修改 `buzz-relay`、`buzz-db` 或 `buzz-auth` 的本功能。

## 19. Observability

新增低基数 metrics：

```text
buzz_project_view_mutations_total{operation,result}
buzz_project_view_mutation_duration_seconds{operation}
buzz_project_view_conflicts_total{operation}
buzz_project_view_snapshot_duration_seconds
buzz_project_view_snapshot_retries_total{reason}
buzz_project_view_objects{type}
buzz_project_view_projection_dispatch_errors_total
buzz_project_view_schema_ready
```

禁止把 Community UUID、object ID、event ID 或 title 作为 metric label。

结构化日志字段：

```text
community_host
command_event_id
actor_pubkey
operation
object_type
object_id
expected_project_revision
committed_project_revision
result_code
```

正文、Resource locator 和 patch 内容不进入普通日志。

运维检查应能回答：

- migration 25 是否生效；
- capability 是否宣告；
- state revision 与 meta projection revision 是否一致；
- object row 保存的 projection ID 是否存在于 events；
- 当前 signer 是否与 projection signer 一致；
- 是否有长时间等待的 Community lock；
- 是否出现 repeated snapshot gap。

## 20. 发布与兼容

### 20.1 版本独立

以下版本相互独立：

- DB migration：`25`；
- Project View wire schema：`1`；
- Relay semver；
- workspace crate version。

`project_revision` 是项目并发控制值，不是任何协议版本。

Project View 是新的 Relay 能力，在 Buzz 0.x 阶段应发布下一个 minor，而不是作为无说明
patch 混入。

### 20.2 Server-first 发布

推荐顺序：

1. 合并领域 crate、migration、Relay、NIP-11 capability、SDK 和全部测试；
2. migration 给所有 Community 的 DB 开关默认写为 `false`，生产客户端不开始发
   kind `44300`；
3. 构建不可变 `sha-*` Relay image；
4. 在 staging 用 `BUZZ_AUTO_MIGRATE=false` 部署全部新 Pod；新 binary 在 0025 尚未应用
   时必须把 Project View 视为 schema-not-ready/disabled，但旧 Buzz 功能正常；
5. 全部 Pod 已是新 binary 后，使用现有 `buzz-admin migrate` 或 migration job 应用
   0025，不能让 mixed-version rollout 中第一个新 Pod 自动把共享 DB 提前升级；
6. 验证 catalog readiness、`_sqlx_migrations`、schema drift、稳定 signer、严格成员读门
   和 Project View E2E；
7. 用相同步骤在生产先滚完全部新 Relay Pod，再运行 migration 25；
8. 确认没有旧 Pod 后，使用 `buzz-admin project-view enable --community ...` 原子开启；
9. 以 `sha-*` image 完成 canary 后发布稳定 `relay-vX.Y.Z`；
10. 再给生产 Agent 发布/固定包含 `buzz project-view` 的 CLI artifact。

旧 Relay 会拒绝未知 kind，因此客户端不能先于服务器启用。

### 20.3 滚动兼容

`0025` 增加新表和 `communities.project_view_enabled` 列；既有查询保持 additive
兼容。滚动期间：

- 中心 DB 开关保持 false；
- 新旧 Pod 都继续处理既有 Buzz event；
- 新 binary 在旧 schema 上必须安全降级，等全部 Pod 升级后才迁移并开启；
- Relay 启用本身不需要 Desktop release；
- 不需要另行部署数据库、Redis consumer 或后台服务。

`buzz-cli` 有三个分发语境：

- 源码和本地构建：`cargo build -p buzz-cli`，可在 Relay enable 后立即使用；
- managed Agent：`buzz-dev-mcp` 已依赖 `buzz-cli`，实现时还要让
  `scripts/build-sprig.sh` 把 `buzz` symlink/manifest entry 放进 archive。main 上的
  `sprig-latest` 是 canary；生产固定版本应在 Relay 稳定后发布并 pin `sprig-v*`；
- Desktop bundle：继续走现有 `v*` Desktop release，首版后端不要求为此同步发 Desktop。

即使 rolling artifact 提前包含命令，CLI 也会因 capability 缺失而拒绝写入；这不是生产
启用手段。

### 20.4 回滚

采用：

```text
应用可回滚，数据库只前进
```

- 不提供常规 down migration；
- 回滚时保留 Project View 表和数据；
- **迁移后、首次 enable 前**，可回滚到上一版 Relay，但必须显式设置
  `BUZZ_AUTO_MIGRATE=false`。旧 binary 的 SQLx migrator 不包含 0025，在看到 DB 已有
  version 25 时可能以 `VersionMissing(25)` 拒绝启动；
- 另一种安全选择是构建“旧业务代码 + 仍内嵌 0025”的 rollback-compatible image；
- CI 必须实际运行“v25 DB + 上一版 Relay + auto-migrate=false”旧功能 smoke，不能只凭
  additive DDL 推断；
- **首次接受 Project View mutation 后**，不能回滚到完全不认识 Project View 的
  pre-feature binary：它没有 `44300/40903/40904` 的严格成员读取 gate，普通全局查询
  可能暴露已存事件。此时优先前滚修复，或部署保留 kind classifier、read gate、DB flag
  和 migration 25 的 rollback-compatible image；
- 已启用后发生故障，先用 admin 在共同 advisory lock 下关闭 capability/写入并确认
  在途 mutation 排空，再前滚或部署上述兼容回滚 image；
- 不删除已产生的 command、object、receipt 或 projection；
- 只有确认数据库真实损坏时才使用备份恢复，不能把恢复备份当普通 rollback。

### 20.5 Buzz 多仓发布边界

本功能不在其他仓复制实现：

| Repo | Project View 影响 |
|---|---|
| `block/buzz` | 唯一源代码、migration、schema、Relay image contract、Chart、SDK、CLI、admin 和测试 |
| `squareup/sprout-oss` | 按现有流水线从同一 source SHA 构建 Relay image；确认 image 同时包含 migration 与 `buzz-admin` |
| `squareup/block-coder-tf-stacks` | 采用本文的 server-first 两阶段 rollout/migration/admin enable，不另建数据库或服务 |
| `squareup/sprout-releases` | 只有以后把新版 `buzz-cli` 随 Desktop 分发时才进入；本后端首版不要求 Desktop release |
| `squareup/sprout-backend-blox` | 若 Blox Agent pin Sprig，则在 Relay 启用后更新到对应 `sprig-v*`；不实现第二套 Project View API |

内部部署仓只编排同一个 Buzz artifact 和中心 DB 开关；协议与领域逻辑始终留在 OSS
`block/buzz`。

## 21. 推荐实施切片

### Slice 1：协议与纯领域

- `docs/nips/NIP-PV.md`；
- kinds 与 classifier；
- `buzz-project-view` 对象、mutation、validation、read model；
- 关系清单和属性测试。

完成标准：不连接数据库即可证明全部对象不变量和 wire schema。

### Slice 2：数据库规范状态

- migration 25 与 `schema/schema.sql`；
- `communities.project_view_enabled` 与 `buzz-admin project-view` 控制面；
- `project_view_state`、objects、mutations；
- `ProjectViewWriteTx`；
- tx-aware event insert helper；
- CAS、幂等和 DB integration tests。

完成标准：command/state/projection 任一步失败都会完整回滚。

### Slice 3：Relay 协议接入

- ingest 安全门；
- WS/HTTP/COUNT/search/live 统一成员读取门禁；
- mutation handler；
- Relay projection 签名；
- `/query` revision-pinned 分页；
- post-commit fan-out options；
- NIP-11 capability；
- signer rotation/reproject；
- WS/HTTP E2E。

完成标准：Human 与 Agent 使用原生 Buzz 协议读写同一当前态。

### Slice 4：SDK 与 Agent CLI

- typed builders/parsers；
- `buzz project-view` 命令；
- snapshot 校验与 read model；
- conflict exit code；
- CLI integration tests。

完成标准：Agent 不需要手写 kind、tags 或关系 JSON。

### Slice 5：CI 与发布

- Just recipes；
- migration/schema drift gate；
- nextest/backend integration；
- Docker path filter；
- metrics、runbook 和 server-first rollout。

完成标准：Project View 与 Buzz 其他后端能力走相同质量门和发布链。

## 22. 首版验收标准

实现完成必须同时满足：

1. 一个 Community 只能形成一份已初始化 Project View。
2. 初始化原子产生一个 Profile 和至少一个 Goal。
3. 关系设计的 21 条验证清单全部自动测试。
4. Human 与 Agent 使用相同 kind、相同 handler 和相同 DB 状态。
5. 客户端不能提交 Project/Community 身份。
6. 任意跨 Community 引用在数据库和领域层都失败。
7. 相同 expected revision 的并发写恰好一个成功。
8. 相同签名 event 在任何重试条件下都不会重复增加 revision；当前安全门仍可拒绝迟到
   或已失去权限的重试。
9. accepted command、规范对象和 Relay projection 不会部分提交。
10. banned、timed-out、channel-scoped 或非成员 actor 不能修改视图。
11. 非成员、channel-scoped token、已 ban 成员和已 ban owner 的 Agent，无法从 REQ、
    COUNT、HTTP、search、pagination 或 stale live subscription 读取协议事件。
12. 普通 `/query` 可读 meta/单对象，HTTP 扩展可完整分页。
13. WS subscription 收齐一个 revision 的 meta/changed heads 后才原子应用该 revision。
14. fan-out 丢失后，重连查询能够恢复相同当前态。
15. Project View protocol event 不触发 Workflow 或 thread side effect，accepted command
    不能被 NIP-09 删除。
16. fresh DB 与 0024 升级 DB 都能通过 migration 测试。
17. `schema/schema.sql` 与全部 migrations 无漂移，ledger-less schema 也能正确判定
    runtime readiness。
18. DB 中心开关在全部 Pod 间一致，disable 提交后不会再提交一个旧 mutation。
19. `just ci` 覆盖快速测试，infra CI 覆盖 DB、CLI 和 E2E。
20. 发布采用 server-first；pre-feature rollback 与已有 Project View 数据后的兼容回滚
    路径分别经过测试。
21. 没有新数据库、外部队列、向量库、搜索服务或独立部署组件。
22. Desktop、Web 和 Mobile 没有为了后端首版被迫同时修改。

## 23. 首版明确不做

- Desktop、Web、Mobile Project View 界面；
- 项目连续性；
- 项目上下文和 Context Compiler；
- Role assignment；
- Project Role 权限化；
- Work assignment、接受、执行、验证和交付协议；
- 对象级 ACL；
- 多 Project Community；
- 跨 Community 关系；
- 任意 mutation batch；
- object-level 自动 merge；
- 多 Goal Plan、多 Stage Requirement/Issue、多 subject Work；
- 状态自动推导；
- 通用关系图；
- 外部 Issue/仓库/文档双向同步；
- NIP-50 Project View 专用搜索；
- 向量索引或语义检索；
- Project View 专属 durable fan-out 基础设施；
- 多 Relay 间的 Project View 合并协议。

## 24. 最终实现模型

首版后端可以压缩为：

```text
Community
  = Project identity
  = tenant boundary

member-signed kind:44300
  = mutation intent
  = immutable accepted command

project_view_state + project_view_objects
  = authoritative current domain state

project_view_mutations
  = idempotency receipt + ordered result

relay-signed kind:40903 / kind:40904
  = Nostr current-state projection
  = query/subscription read model

one PostgreSQL transaction
  = command + state + receipt + projection

existing Buzz post-commit path
  = audit + Redis + local fan-out
```

它让 Project View 成为 Buzz 自己的一个领域能力：沿用 Buzz 的身份、事件、数据库、
实时分发、Agent CLI、测试、CI 和发布体系，同时为下一步项目连续性保留稳定对象坐标、
严格 revision 和可恢复的当前项目状态。
