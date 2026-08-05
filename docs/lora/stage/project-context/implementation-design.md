# Project Context Edge V0 后端实现设计

> 状态：待实现
> 日期：2026-08-05
> 目标分支：`feat/project-context-v0`
> 领域语义来源：[Project Context V0 领域规范](./project-context.md)

## 1. 文档目的

本文把已经确认的 Project Context 最小领域语义映射到 Buzz 当前架构，固定首版的：

- Rust 领域与协议边界；
- Nostr command 与 Relay-signed projection；
- PostgreSQL 规范状态和事务不变量；
- `exact`、`incident`、`contains-all` 查询；
- Project Document 删除保护；
- Community 权限与私有读取；
- Agent-first CLI 与 ACP 稳定认知；
- capability、初始化、状态恢复和验收顺序。

本文不重新讨论 Context 的领域含义。若实现细节与领域规范冲突，以领域规范为准。

本文所说的“后端”包括 Relay、数据库、领域 crate、SDK、CLI 和 ACP Agent surface；不包括
Desktop、Mobile 或 Web。图形客户端的数据层、缓存、交互和 UI 需要另行设计。

## 2. 实现结果

首版实现完成后，Human / Agent 可以：

1. 使用两个或多个 Project View / Project Document 坐标和一份 active Project Document
   建立 Context Document 绑定；
2. 由第一份绑定自动得到唯一的无向 Context Edge；
3. 在同一 Edge 下继续增加 Context Document；
4. 通过 `exact`、`incident`、`contains-all` 发现 Edge；
5. 按需读取关联 Document 的当前正文；
6. 通过现有 Document update / patch 修正上下文正文，而不改变 Edge 或 Context Revision；
7. 解除 Document 绑定，并在最后一份 Document 解除后使 Edge 从当前领域状态消失；
8. 在坐标 tombstone 后继续发现原 Edge；
9. 在 Context Document 仍绑定时得到明确的 Document 删除拒绝。

实现不向每一 turn 注入 Edge 或 Document 正文，也不尝试判断 Gap、过期、语义冲突、语义
是否完整或内容是否正确。

## 3. 已确认的实现决策

| 主题 | 决策 |
|---|---|
| 领域归属 | 新建独立 `buzz-project-context` crate，不把 Edge 塞入某个 Project View 对象 |
| 写入原语 | 对一份 Context Document 执行 `attach` / `detach`；Edge 随绑定自动产生或消失 |
| 领域身份 | `Project + canonical exact coordinate set`，实现句柄由该集合确定性派生 |
| 并发边界 | 独立、Project-scoped `context_revision`，不推进 Project View 或 Document Revision |
| 当前投影 | 每份 Document binding 一个 active / deleted head，加一个 Project Context meta head；Edge 由 active bindings 按确定性 key 聚合 |
| 内容版本 | 只引用稳定 `document_id`；正文和历史继续由 Project Document 管理 |
| 查询入口 | Nostr projection + 现有 `POST /query`；不增加业务专用 HTTP endpoint |
| 权限 | 复用 Community、Project View、Document 和可选 Assignment / Runtime fence 规则 |
| Agent 交付 | CLI 按需查询；只更新稳定 `[Project Space]` 认知，不改 Role Brief closure |
| 现有引用 | Context Reference 保持独立，不迁移、不投影、不自动同步 |

## 4. 当前实现基线与能力隔离

### 4.1 可直接复用的能力

当前分支已经提供：

- Project View v3 的稳定对象身份、对象类型、tombstone、投影签名和 Project-scoped 权限；
- Project Document v1 的稳定 `document_id`、Current / Revision、删除保护和按需正文；
- member-signed command、Relay-signed head / meta projection 和 replay-first 事务模式；
- Community-private protocol 的 REQ、COUNT、`/query`、subscription 和 point-read 防泄漏门；
- Agent CLI 的 NIP-98 / WebSocket 读写、结构化 JSON 和冲突 exit code `5`；
- `[Project Space]` 独立 system contract 与 session contract 失效机制。

新实现沿用这些模式，不建立第二套 Document、权限或 Runtime 模型。

### 4.2 不复用已有 capability 名称

`buzz-project-context-v1` 已经表示现有 Project View Context Reference capability。新 Edge
与它结构和语义不同，不能复用或改变这个 extension。

新能力固定为：

```text
buzz-project-context-edge-v1
```

并新增独立 Community flag：

```text
project_context_edge_enabled
```

它要求 Project View schema v3 与 Project Document v1 已就绪，但不要求已有
`project_context_enabled`。两种 Context capability 可以独立启用。

`project_context_edge_enabled` 只表达运行时可用性。它控制 capability 广告与新的
`attach`；已经初始化且投影完整的状态仍可被授权成员读取，并允许 `detach` 解除 Document
删除保护。具体门控见第 7、15、18 节。

## 5. Crate 与模块边界

### 5.1 新建 `buzz-project-context`

新 crate 只包含纯领域与 closed wire types：

```text
crates/buzz-project-context/
├── src/lib.rs
├── src/coordinate.rs
├── src/command.rs
├── src/model.rs
├── src/projection.rs
├── src/reducer.rs
├── src/error.rs
└── tests/
```

主要职责：

- 坐标规范化与集合判等；
- 确定性 Edge key；
- command / receipt / binding projection contract；
- active / deleted Document binding head；
- attach / detach 纯 reducer；
- 数量、字节、JSON 深度与 Revision 边界；
- property tests 和 golden fixtures。

它可以依赖 `buzz-core` 和 `buzz-project-view` 的稳定对象类型；Document 坐标只需要稳定
UUID，不依赖 Document 正文或 reducer。`buzz-project-view` 与
`buzz-project-document` 不反向依赖新 crate，避免领域依赖环。

### 5.2 现有 crate 的职责

- `buzz-core`：kind registry、protocol classifier 和 Community-private registry；
- `buzz-sdk`：严格 Nostr event builder / parser / projection verifier；
- `buzz-db`：规范状态、索引、事务、完整性和 replay；
- `buzz-relay`：能力门、授权、签名、提交和 fan-out；
- `buzz-cli`：Agent-facing 查询和 attach / detach；
- `buzz-acp`：稳定可发现性，不加载动态 Edge；
- `buzz-project-document`：领域 reducer 不改；DB 删除检查增加 Context binding 来源。

wire contract 另写入 `docs/nips/NIP-PCE.md`，共享 golden fixtures 放在
`docs/nips/fixtures/project-context-edge-v1/`。实现设计可以解释取舍，但 kind、closed JSON、
canonical tags、坐标编码和 receipt 的跨 crate 合同以该协议文档与 fixtures 为准。

## 6. 领域与 wire 数据结构

### 6.1 `ProjectContextCoordinate`

首版使用 closed tagged union：

```rust
#[serde(tag = "coordinate_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectContextCoordinate {
    ProjectViewObject {
        object_type: ProjectViewObjectType,
        object_id: Uuid,
    },
    Document {
        document_id: Uuid,
    },
}
```

Project / Community 身份由请求 host 和投影 `project_id` 决定，不接受客户端提交另一个
Project ID。

首版 wire 安全限制：

```text
MIN_EDGE_COORDINATES = 2
MAX_COMMAND_CONTENT_BYTES = 65_536
MAX_PROJECTION_CONTENT_BYTES = 65_536
MAX_COMMAND_JSON_DEPTH = 16
MAX_SAFE_REVISION = 9_007_199_254_740_991
```

V0 不另设坐标数量或每 Edge Document 数量的领域硬上限。坐标集合必须整体出现在一次
`attach` / `detach` command 和派生的单 binding projection 中，因此实际输入自然受 command、
projection 字节与 JSON 深度上限约束；Document 则逐份 attach，并通过独立 binding
projection 分页读取，不把全部 Document ID 累积进一个有界事件。SDK / Relay 必须在 commit
前同时验证 command content、派生 projection content 和完整 signed `EVENT` frame 不超过
NIP-11 / Relay 配置边界。达到传输上限时不得通过创建第二条相同 exact-set Edge 绕过。

坐标 ID 必须满足其来源领域已有的身份规则：普通 Project View 对象与 Document 使用
非 nil UUID v4；Project Profile 的 `object_id` 必须等于当前 Project / Community UUID。
Project View `object_type` 必须与规范对象记录一致。

### 6.2 规范顺序

输入在提交前规范化：

1. Project View 坐标在前，Document 坐标在后；
2. Project View 坐标按显式稳定的 `object_type` 顺序，再按 UUID canonical bytes；
3. Document 坐标按 UUID canonical bytes；
4. 完全重复坐标拒绝，不静默删除；
5. 排序后仍必须至少包含两个坐标，并满足 command 字节与 JSON 深度限制。

对象类型顺序写成显式协议函数，不能依赖 Rust enum discriminant 或数据库文本排序。

### 6.3 Edge key

Edge 没有调用者选择的独立 ID。实现从 Project 和规范坐标集合确定性计算：

```text
edge_key = SHA-256(
  "buzz-project-context-edge-v1\0"
  || project_uuid_bytes
  || coordinate_count_u32_be
  || canonical_coordinate_bytes...
)
```

每个坐标使用固定 variant byte、显式对象类型 byte 和 UUID bytes 编码。不得以 JSON 文本、
标题或当前 Revision 作为 hash 输入。

`edge_key` 是存储、投影和 CLI 使用的实现句柄，不是坐标集合之外的另一份领域身份。遇到
相同 hash 但不同 canonical coordinate bytes 时必须以内部完整性错误失败，不能合并。

### 6.4 当前 active Edge

```rust
pub struct ProjectContextEdge {
    pub edge_key: EdgeKey,
    pub coordinates: Vec<ProjectContextCoordinate>,
    pub context_document_ids: Vec<Uuid>,
}
```

`coordinates` 与 `context_document_ids` 都使用 canonical order。active Edge 始终具有至少
两个坐标和至少一份 active Context Document。

Edge 不增加独立领域 Revision；它的当前结构由本 Project 的 `context_revision` 和 active
binding 集合共同确定。修改 Context Document 正文不会推进 `context_revision`。

### 6.5 Context catalog

```rust
pub struct ProjectContextCatalog {
    pub project_id: CommunityId,
    pub context_revision: u64,
    pub active_edge_count: u64,
    pub bound_document_count: u64,
    pub projection_generation: u64,
    pub initialized_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

`context_revision` 是独立的 Project-scoped CAS 和一致读取边界：

- 初始化为空时为 `0`；
- 每个成功的新 attach / detach command 恰好增加 `1`；
- replay 不增加；
- Project View 对象变化不增加；
- Document 正文或 Revision 变化不增加；
- 一个 Project 的变化不与另一个 Project 冲突。

`projection_generation` 只用于 Relay signer materialization；初始化为 `1`，reproject 时增加，
不推进 `context_revision`。counts、Revision 和 generation 都必须位于 JavaScript-safe integer
范围内，`updated_at` 不早于 `initialized_at`。catalog 还必须满足：

- `context_revision = 0` 只可能对应从未发生业务 mutation 的空 catalog，两个 counts 都为
  `0`；meta 必须是 reset（可以是初始 bootstrap，也可以是该空 catalog 的 reproject）；
- `active_edge_count <= bound_document_count`；
- `active_edge_count = 0` 当且仅当 `bound_document_count = 0`；
- `projection_generation` 始终为正数。

### 6.6 后续坐标类型扩展

v1 parser 对未知 `coordinate_type` 继续严格拒绝。增加新坐标类型时必须同时提供：

- 新的显式 variant 与 canonical byte discriminator；
- Project-scoped typed identity resolver、tombstone 规则与 normalized index branch；
- canonical `c` tag 编码与 hydration；
- 来源领域权限检查；
- schema / capability 兼容方案。

若旧客户端可能读到新 variant，必须提升 schema / capability版本，不能在 closed v1 wire
中静默加入。Edge key、无向集合、Document binding、生命周期与三类查询算法保持不变。

## 7. Command contract

### 7.1 command envelope

```rust
pub struct ProjectContextCommand {
    pub schema_version: u16,              // 1
    pub expected_context_revision: u64,
    pub acting_assignment_id: Option<Uuid>,
    pub runtime_fence: Option<RuntimeFence>,
    pub request: ProjectContextRequest,
}
```

`acting_assignment_id` 与 `runtime_fence` 复用 Project View v3 的可选归因与监督检查。它们
不赋予权限，也不在 Edge 上建立 maintainer。两者必须同时省略或同时出现；Human command
必须省略，managed Agent 的普通 Community-authority write 默认也省略。只有 managed Agent
显式声明 Assignment 归因时，才同时提交 active Assignment 与 exact supervised Runtime fence。

### 7.2 两个写入操作

```rust
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectContextRequest {
    Attach {
        coordinates: Vec<ProjectContextCoordinate>,
        context_document_id: Uuid,
    },
    Detach {
        coordinates: Vec<ProjectContextCoordinate>,
        context_document_id: Uuid,
    },
}
```

首版不提供 `create_edge`、`delete_edge`、`update_coordinates` 或隐式 `move`：

- `attach` 到不存在或 deleted 的 exact set 时建立 active Edge；
- `attach` 到 active exact set 时增加 Document；
- `detach` 移除 Document；
- 每次 detach 都产生该 Document 的 deleted binding head；最后一份 detach 时当前领域 Edge
  同时消失；
- 修改坐标范围使用显式 detach，再向新 exact set attach；
- Context Document 已属于另一 Edge 时，新的 attach 拒绝，不自动移动。

### 7.3 attach 前置条件

同一事务内必须证明：

1. capability 已启用且 projection signer / generation 就绪；
2. `expected_context_revision` 等于当前 catalog revision；
3. 所有坐标属于当前 Project，并且稳定身份存在；
4. 所有坐标当前均为 active；
5. `context_document_id` 是当前 Project 的 active Document；
6. 该 Document 尚未作为 Context Document 绑定其他 Edge；
7. actor 通过现有 Community 与可选 Assignment / Runtime 检查。

Document 作为坐标进入 tombstone 后，身份仍保留在已有 Edge 中；Document 作为 Context
Document 时始终必须 active。两种角色由不同字段验证。V0 的 attach 统一要求 active 坐标，
避免把领域规范已经确认的“tombstone 后已有 Edge 保留”扩张为尚未确认的新关系创建或成员
追加语义。这是新 attach event 的 transition proof；已经 accepted 的 event replay 按第 7.5
节通过当前 credential / availability gate 后只返回原 receipt。

### 7.4 detach 前置条件

同一事务内必须证明：

1. revision、structural readiness 与 actor 检查通过；
2. exact coordinate set 对应当前 active Edge；
3. `context_document_id` 当前正绑定该 Edge。

`project_context_edge_enabled = false` 时，新的 attach 与 attach replay 都返回 unavailable；
当 state 已初始化且 signer / generation / projection parity 可验证时，精确 detach 及其 replay
仍允许执行，以解除 Document 删除保护。未初始化或 projection 不安全时仍然 fail closed。

调用者必须提交 exact set，而不只提交 `edge_key`。Relay 重新规范化并计算 key，防止
调用者以 opaque handle 绕过坐标校验。

### 7.5 no-op、冲突与 replay

- replay 仍先重新验证当前 credential、actor、scope、signer 和该 operation 的 availability
  gate；通过后，同一 signed command event 返回第一次保存的 receipt；
- 新 event 重复 attach 同一绑定，返回 `invalid:project_context:no_change`；
- detach 不存在的绑定，返回 `not_found:project_context:binding`；
- stale `expected_context_revision` 返回 write conflict，CLI 映射 exit code `5`；
- Document 已属于另一 Edge，返回带现有 `edge_key` 的稳定 conflict；
- 所有业务失败发生在 commit 前。

### 7.6 receipt

成功 receipt 至少包含：

```text
schema_version
change_id
operation: attach | detach
context_revision
edge_key
edge_state: active | deleted
edge_document_count
context_document_id
```

`change_id` 对 member-signed command 等于 event ID。receipt 不返回 Document 正文。

## 8. Reducer 与生命周期

### 8.1 首次 attach

```text
catalog revision N
edge absent / historical deleted
all coordinates active
active Document D
        │ attach({A,B}, D)
        ▼
catalog revision N+1
active Edge {A,B}
documents [D]
```

事务创建 active Edge 规范 row，并为 D 生成一条 active binding head。若同一 exact set
曾经消失，事务重新激活同一个确定性 `edge_key`，而不是创建第二个领域身份。

### 8.2 增加 Document

对 active Edge attach D2：

- Edge coordinates 不变；
- canonical Document set 增加 D2；
- `context_revision + 1`；
- `active_edge_count` 不变；
- `bound_document_count + 1`；
- 为 D2 写入 active binding head，已有 Document 的 binding head 不重写。

### 8.3 非最后一份 detach

- 将这一份 Document binding 的当前 head 置为 deleted；
- Edge 保持 active；
- `context_revision + 1`；
- active edge count 不变；
- bound document count 减少 `1`。

### 8.4 最后一份 detach

- 将最后一份 Document binding 的当前 head 置为 deleted；
- 当前 active Edge 不再存在；
- 规范 Edge row 进入 deleted transport state，保留 canonical coordinates 与 key 以支持
  完整性、replay，以及在届时全部坐标均 active 时对相同 exact set 的后续重新建立；
- `active_edge_count - 1`，`bound_document_count - 1`。

deleted binding head 与 deleted Edge row 只用于让订阅者、缓存、replay 和恢复流程观察
关系消失，不是 active 空 Edge，也不改变领域规范中“空 Edge 同时消失”的结论。

### 8.5 坐标 tombstone

Project View / Document 自己的 tombstone 事务不调用 Context reducer：

- 不改变 Edge row 或任何 binding head；
- 不增加 Context Revision；
- 不删除坐标索引；
- 查询 hydration 时读取目标领域当前 head，并显示 tombstoned；
- 后续仍允许 Human / Agent 更新已有 Context Document 或执行 detach；存在 tombstoned 坐标
  时 attach 拒绝。最后一份 Document detach 后 Edge 消失，不能仅用 tombstoned 坐标重建。

## 9. Nostr 协议

### 9.1 新 kind

当前 registry 中以下值未使用，首版固定为：

| kind | 常量 | 作者 | 作用 |
|---:|---|---|---|
| `40908` | `KIND_PROJECT_CONTEXT_EDGE_BINDING` | Relay | 一份 Context Document binding 的 active / deleted current head |
| `40909` | `KIND_PROJECT_CONTEXT_META` | Relay | 当前 Context catalog metadata |
| `44302` | `KIND_PROJECT_CONTEXT_COMMAND` | Member | append-only attach / detach command |

两个 `409xx` kind 具有 indexed `d` tag，但不采用 NIP-33 LWW；替换由 Context Revision
事务控制。它们加入 relay-only classifier。`44302` 加入 command、global-only 和
`messages:write` classifier。

三种事件全部加入新的 Project Context Edge protocol classifier，并进入 Community-private
protocol registry。

### 9.2 command event

member-signed command 使用精确 tags：

```text
["-"]
["t", "buzz-project-context-edge-command"]
```

不接受 `h`、客户端 `d` 或坐标 tags。坐标的唯一事实是 canonical JSON command body；SDK
对签名、kind、exact tags、closed JSON 和 canonical scalar spellings 做严格验证。

### 9.3 Binding head projection

Relay-signed content 使用 closed active / deleted union，共同字段包括：

```text
schema_version: 1
projection_type: context_edge_binding
project_id
projection_generation
context_revision
edge_key
coordinates[]
context_document_id
state: active | deleted
source_event_id
updated_at
```

一条 active binding 表示这份 Document 当前属于该 exact-set Edge；deleted binding 只表示该
Document 当前已解除这次归属。Edge 是所有 `state = active` 且 `edge_key` 相同的 binding
投影聚合结果，不把全部 Document ID 累积进单个事件。

binding 的 `d` 坐标和 Edge 的 query coordinate 分别为：

```text
binding d = project-context-edge:<project-uuid>:binding:<document-uuid>
edge g    = project-context-edge:<project-uuid>:<edge-key-hex>
```

每个 binding head 的完整 canonical tag sequence 固定为：

```text
["-"]
["d", <binding-coordinate>]
["t", "buzz-project-context-edge"]
["t", "binding"]
["s", "active" | "deleted"]
["g", <edge-query-coordinate>]
["c", <canonical-coordinate-tag>]  # 每个坐标一条，保持 canonical order
["projection_generation", <canonical-decimal>]
["context_revision", <canonical-decimal>]
["e", <source-event-id>, "", "source"]
```

parser 必须从 content 重新计算 `edge_key`、`d`、`g` 和每个 `c` tag，并要求 tags 是上述
exact canonical sequence；不能用“至少包含”或只做宽松 multiset 匹配。`source_event_id`
必须等于最后一条 `e` tag，并指向本次 accepted member command。

坐标 tag 值包含 Project 身份：

```text
pv:<project-uuid>:<object-type>:<object-uuid>
document:<project-uuid>:<document-uuid>
```

### 9.4 Meta projection

meta head 坐标：

```text
project-context-edge:<project-uuid>:meta
```

closed content 包含：

```text
schema_version
projection_type: context_meta
project_id
projection_generation
context_revision
active_edge_count
bound_document_count
reset
changed_bindings[]
source_event_id?  # reset 时省略，普通 mutation 必须存在
updated_at
```

初始化 / signer reproject 使用 `reset = true`、空 `changed_bindings` 并省略
`source_event_id`。普通 mutation 使用 `reset = false`，并携带本次唯一 changed binding 的
`context_document_id`、`edge_key`、binding coordinate、event ID 和 state。

meta 的完整 canonical tag sequence 为：

```text
["-"]
["d", "project-context-edge:<project-uuid>:meta"]
["t", "buzz-project-context-edge"]
["t", "meta"]
["projection_generation", <canonical-decimal>]
["context_revision", <canonical-decimal>]
["e", <source-event-id>, "", "source"]  # 仅普通 mutation
```

### 9.5 投影验证与替换

- Relay event 必须由当前 configured projection signer 签名；
- `project_id` 必须等于 host-derived Community；
- generation、Revision 和 canonical time 必须在合法范围；
- binding 与 meta 必须同 signer、Project 和 generation；
- binding 的 `context_revision` 必须为正数且不得大于当前 verified meta；对于
  `reset = false` 的增量 meta，二者相等时，meta 的唯一 `changed_bindings` entry 必须精确
  绑定该 binding event；对于 `reset = true` 的初始化 / reproject observation，
  `changed_bindings` 必须为空，同 generation 的 bindings 通过规范内容和全查询前后的相同
  reset meta 验证；Relay / DB 完整性检查还必须证明 current pointers 与 counts 一致，全
  catalog loader 才使用全局 counts 校验完整集合；
- current event store 中同一 `d` 只保留 Context domain 选择的 binding / meta head；
- 普通 attach / detach 将一个 accepted command、恰好一个 changed binding head、incremental
  meta、receipt 与规范 DB 状态在一个事务提交；
- bootstrap 将空 catalog 与一份 reset meta 原子提交，不生成 command 或 binding；reproject
  将 `0..N` 个重建 binding heads、reset meta 与新的 generation / pointers 原子提交，不生成
  command 或业务 change；
- commit 后只做 fan-out，不再执行可能产生业务拒绝的工作。

## 10. PostgreSQL 设计

### 10.1 migration

新增：

```text
migrations/0047_project_context_edge.sql
```

迁移只建表、约束、索引和 capability flag，不从 Context Reference 生成数据。flag 初值为
false，因为 SQL migration 无法持有 Relay signer、不能伪造 revision-zero reset meta；这是
初始化安全边界。`schema/schema.sql` 与
`crates/buzz-db/src/migration.rs` 的 migration 数量、顺序和内容断言必须同步更新。

Community constraint 至少保证：Edge capability 开启时，Project View schema 必须为 `3`、
Project View enabled / read-ready、maintenance 为 normal，且 Project Document enabled /
read-ready。Project View 降级或 Document disable 的 operator preflight 必须先要求
`project_context_edge_enabled = false`，不做隐式联动关闭。

### 10.2 `project_context_edge_state`

每个 Community 一行：

```text
community_id PK
schema_version = 1
context_revision
active_edge_count
bound_document_count
last_change_id / last_actor_pubkey
projection_pubkey
projection_generation
meta_projection_event_id
initialized_at / updated_at
```

初始化为空 catalog，Revision 和 counts 均为 `0`，并保存一份 Relay-signed reset meta。row
checks 固定 JavaScript-safe 范围、正 generation、时间顺序、第 6.5 节的 zero/count shape；
deferred validator 再把 counts 与 Edge / binding 规范行逐项核对。

### 10.3 `project_context_edges`

```text
community_id
edge_key BYTEA(32)
state active | deleted
canonical_coordinates JSONB
last_context_revision
current_source_change_id
updated_at / updated_by
PRIMARY KEY (community_id, edge_key)
```

deleted row 保留坐标集合，用于 replay、recreate 和完整性检查；active 查询只返回
`state = 'active'`。Edge row 没有独立 projection pointer 或领域 Revision；对外 current state
由 active binding projections 聚合。

`updated_by` 只记录最近一次结构变化的 actor，不表示该 actor 或其 Role 成为 Edge
maintainer / owner。

### 10.4 `project_context_edge_coordinates`

规范化坐标索引：

```text
community_id
edge_key
ordinal
coordinate_type       # project_view_object | document
coordinate_subtype?   # PV object_type；Document 时省略
coordinate_id
canonical_key
```

约束：

- ordinal 在 Edge 内唯一且连续；
- `(coordinate_type, coordinate_subtype, coordinate_id)` 必须符合 closed v1 shape；
- `canonical_key` 在 Edge 内唯一并等于 canonical coordinate bytes 的稳定文本编码；
- typed resolver 在共同 Community lock 下验证对应 Project View / Document identity row 存在、
  Project 相同且 PV `object_type` 匹配；active 与 tombstone identity 对已有规范行的完整性都
  有效，但 attach 的 operation-specific proof 仍按第 7.3 节要求 active；
- 来源领域继续禁止 hard delete，deferred validator 复查所有 typed identities；
- canonical JSON、normalized rows 与 `edge_key` 必须一致。

通用索引为：

```text
(community_id, coordinate_type, coordinate_subtype, coordinate_id, edge_key)
```

增加新坐标类型时不增加 nullable ID 列；扩展 closed resolver、subtype 规则、partial integrity
检查和来源权限即可。这样数据库形状保持可扩展，同时不把 generic UUID 当作未经验证的引用。

### 10.5 `project_context_document_bindings`

```text
community_id
context_document_id
edge_key
state active | deleted
binding_context_revision
current_source_change_id
current_projection_event_id
updated_at / updated_by
PRIMARY KEY (community_id, context_document_id)
FOREIGN KEY edge -> project_context_edges
FOREIGN KEY document -> project_documents
```

以 `context_document_id` 为 Project-scoped 主键，直接保证一份 Document 最多作为 Context
Document 属于一条 active Edge。detach 后保留 deleted transport row 和当前 deleted binding
projection pointer；它不是领域归属。另建
`(community_id, edge_key, state, context_document_id)` 索引以生成 active canonical 成员集合。

Document 作为 Edge 坐标不写入本表，因此可以作为坐标出现在多条 Edge，也可以继续成为
Context Reference 或 Resource Guide 目标。

### 10.6 `project_context_edge_changes`

append-only accepted change ledger 保存：

```text
community_id / change_id
source_type / source_event_id / actor
acting_assignment_id
operation
expected_context_revision / context_revision
edge_key / edge_state / edge_document_count
context_document_id
canonical_coordinates
result
accepted_at
```

对 member command，`change_id = source_event_id`。表使用 immutable trigger，并以 change ID
提供 replay-first 查询。

### 10.7 deferred integrity

事务末尾的 deferred constraint trigger 或等价的 commit-time validator 必须验证：

- active Edge 至少有一条 `state = active` binding；
- deleted Edge 没有 `state = active` binding；
- 每条 active binding 必须指向 active Edge，且其 `edge_key` / Project 与该 Edge 完全一致；
- 每个 Edge 至少有两条 canonical coordinate rows；
- state counts 等于规范表实际 counts；
- JSON、normalized rows、hash 和当前 signed binding projections 一致；
- active binding 指向的 Context Document 当前仍为 active；
- 新 accepted attach change 在共同 lock 内观察到的全部坐标均为 active；这个 transition proof
  不转化为持久状态约束，坐标后续 tombstone 仍不会使已有 Edge 非法；
- deleted binding 不参与 Edge 聚合或 Document 删除保护。

Document state 更新也必须触发或调用 binding parity 检查，使绕过正常 reducer 的受信维护
路径不能把仍绑定的 Context Document 直接改成 deleted。

## 11. Relay 写事务

### 11.1 统一流程

```text
bind host → Community
        ↓
strict command parse + signature/tags
        ↓
operation-specific capability/readiness gate
        ↓
BEGIN; acquire shared Community exclusive advisory lock
        ↓
lock project_context_edge_state FOR UPDATE
        ↓
revalidate current actor / scope / optional fence
        ↓
replay lookup
        ↓
expected revision + coordinate/document proof
        ↓
pure reducer
        ↓
build and Relay-sign binding + meta projections
        ↓
atomic DB rows + event store + receipt commit
        ↓
fan-out committed command/projections
```

### 11.2 锁与并发

首版首先取得 Project View v3 与 Project Document 已使用的同一把 Community exclusive
advisory lock，再使用每 Project 一行 catalog lock 和 global `context_revision` CAS。共同
advisory lock 负责序列化 Context attach / detach、Project View tombstone / reference 更新
和 Document update / delete；Context catalog lock 负责本领域 Revision 与 counts。

这一顺序不得反转，也不得为 Context Edge 发明另一把独立 Community lock。它与 Project
View 的 Project Revision 模式一致，先保证完整一致 snapshot，再根据真实写入压力决定
是否引入 per-Edge CAS。

两个不同 Edge 的并发写可能发生 revision conflict，但不会死锁或静默覆盖。CLI 必须重新
读取 meta 后由调用者明确重试，不做自动 rebase。

### 11.3 attach commit

事务按以下顺序准备规范结果：

1. lock catalog；
2. 校验当前 actor / scope / fence；
3. replay lookup；
4. canonicalize coordinates 并计算 key；
5. 加锁读取全部坐标 head 与 Context Document；
6. 检查 Document 单 Edge binding；
7. reducer 产生新 Edge / catalog；
8. 生成并验证 signed binding 与 meta events；
9. upsert Edge、coordinates 与 binding，insert change；
10. replace current projection events，更新 catalog；
11. deferred integrity 通过后 commit。

### 11.4 detach commit

detach 加锁读取 exact Edge 与 Document binding，将 binding 规范 row 与 signed head 置为
deleted。非最后一份时 Edge row 保持 active；最后一份时 Edge row 也进入 deleted transport
state并保留 coordinates。任何失败回滚整个事务。

## 12. 与 Project View / Document 生命周期集成

### 12.1 Context Document 删除保护

扩展 `buzz-db/src/project_document.rs` 的 `deletion_blocked` 查询：

```sql
EXISTS (
  SELECT 1
  FROM project_context_document_bindings
  WHERE community_id = $1
    AND context_document_id = $2
    AND state = 'active'
)
```

它与现有 Resource Guide 和 Live Context Reference 删除保护做 OR。拒绝只提示 active
binding，调用者必须先执行 Context detach。

Document write 与 Context Edge write 都在同一把 Community advisory lock 下完成，因此
删除检查和新的 attach 不会发生“双方都在提交前看到不存在 binding”的竞态。

Pinned Context Reference 现有规则保持不变。

### 12.2 Document 坐标删除

如果 Document 只作为 `coordinates` 中的坐标，而不是 Context Document binding，普通
tombstone 允许成功。typed identity resolver 指向保留的 Document identity row，Edge 不变。

若同一 Document 同时是坐标和 Context Document，binding 删除保护仍阻止 tombstone，
直到先 detach。

### 12.3 Project View tombstone

Project View 对象 tombstone 不检查 incoming Context Edge，也不级联修改 Edge。现有对象
row 与 type 继续提供稳定 FK / hydration identity。

### 12.4 Revision 相互独立

- attach / detach 只推进 `context_revision`；
- Context Document 正文继续通过 `buzz documents update` / `patch` 修正，只推进 Document
  revision 与 Document catalog revision；Edge 坐标、binding 和 `context_revision` 均不变；
- Project View 更新只推进 Project / object revision；
- Context 查询 hydration 可以同时报告三个独立观察 Revision，不制造跨领域总 Revision。

## 13. 查询实现

### 13.1 读取原则

查询返回 current Edge 结构和轻量坐标 / Document 信息，不返回 Markdown 正文。所有结果
由 verified active binding heads 按 `edge_key` 聚合，并经过 SDK 的 Relay signature、Project、
generation、exact tags、Edge hash 与 meta observation 验证。同一聚合组必须具有完全相同的
canonical coordinates，且不能出现重复 `context_document_id`；否则 fail closed。

### 13.2 `exact(Q)`

1. 客户端 canonicalize `Q`；
2. 计算 `edge_key` 与 `g` query coordinate；
3. 在第 13.5 节的 meta 双读边界内，用 kind `40908 + #g + #s=active` 分页取得全部
   active binding heads；
4. 验证并聚合 bindings；
5. 没有 active binding 返回空，否则返回唯一一条 exact Edge。

`exact` 输入必须至少具有两个不同坐标，并满足通用 command / filter 字节限制。它不因
结果在一页内就省略 meta 双读。

### 13.3 `incident(A)`

每条 binding head 为 Edge 的每个坐标携带一个 canonical `c` tag。客户端查询：

```json
{
  "kinds": [40908],
  "#c": ["<canonical-coordinate-tag>"],
  "#s": ["active"],
  "limit": 500,
  "page": 1
}
```

CLI 使用现有 NIP-98 `POST /query` 的 1-based `page` 扩展遍历到空页，按 event ID 去重，再按
`edge_key` 聚合。同一 Edge 有多份 Context Document 时会返回多条 binding event，但最终只
形成一条 Edge 结果。底层事件分页继续使用现有 `created_at DESC, event_id ASC` 全序；聚合后
Edge 按 `edge_key`、Document 按 UUID canonical bytes 排序，保证相同 snapshot 的输出稳定。

Relay 为 exclusively-kind-40908 的 `#c` / `#g` / `#s` 增加 JSONB GIN pushdown，且必须同时
覆盖 `query_events`、`count_events`、WS REQ / COUNT、HTTP `/query` / `/count` 和
`filter_fully_pushable` 判断，避免 custom tag 在 SQL `LIMIT` 后才被 Rust post-filter。

### 13.4 `contains-all(Q)`

客户端：

1. canonicalize `Q`；
2. `Q` 非空时，对 canonical 第一坐标执行完整 incident 分页；`Q` 为空时，在相同 meta
   双读边界内分页读取全部 `#s=active` binding；
3. 严格验证并按 `edge_key` 聚合每个 binding；
4. 保留满足 `Q ⊆ edge.coordinates` 的 Edge。

这不会把 `{A,B,C}` 拆成二元边，也不会把只包含部分 Q 的 Edge 返回。
按集合公式，`contains-all({})` 返回当前 Project 的全部 active Edge；它仍然分页且不返回正文。

后续可以根据 normalized DB 统计选择更稀疏的 anchor coordinate，但首版不增加 query
planner协议。

### 13.5 一致分页

为了避免分页期间 mutation 导致 offset 漂移，SDK / CLI 使用：

```text
read meta M1
page all matching active binding heads
read meta M2
accept only when M1 / M2 event ID、context revision 和 generation 全部相同
otherwise retry complete query（最多 3 次）
```

超过重试预算返回明确 snapshot conflict，不返回可能缺页的结果。

这个流程用于 `exact`、`incident` 和 `contains-all`，不只是多页列表。每个 binding 必须满足
`0 < binding.context_revision <= M1.context_revision`。M1 是增量 meta 且二者相等时，还必须
命中 M1 的唯一 `changed_bindings` entry；M1 是 reset meta 时，其 `changed_bindings` 必须为
空，并按第 9.5 节的 reset observation 规则验证。若查询目标是全 catalog，可进一步用
`active_edge_count / bound_document_count` 校验聚合总数；子集查询不能误用全局 counts。

M1 / M2 只冻结 Context Edge 结构。Project View 与 Document hydration 各自验证并报告其
观察 Revision，不声称三个领域共享一个全局快照。

### 13.6 hydration 与输出

查询完成后按坐标批量读取当前 Project View / Document head：

- active 坐标显示类型、ID 和轻量标题 / 状态；
- tombstone 坐标明确显示 `tombstoned`；
- Context Document 显示 ID、title、summary、current revision、updated actor/time；
- 每份 Document 提供 `buzz documents get <id> --content-only` fetch command；
- metadata fetch 在重试后仍发生瞬时 I/O / availability 失败时，不把 Edge 静默删除，而以
  `unavailable` coordinate / document state 呈现，并明确这不是语义 Gap 判断。

Context Document binding 的规范状态保证 Document active。已成功取得并验证的 Document
head 若显示 deleted、错误 Project / signer / generation，或与 binding 的 Project /
`context_document_id` 不一致，则属于规范完整性错误，必须 fail closed 并要求 repair；这种
verified contradiction 不能降级成普通 `unavailable`，也不能假装 Edge 没有内容。

## 14. CLI 设计

### 14.1 新顶层 command group

新增：

```text
buzz project-context
```

不复用 `buzz project-view context`，后者继续操作现有 Context Reference。

### 14.2 坐标参数

CLI 使用统一、可扩展的 token：

```text
project_profile:<uuid>
goal:<uuid>
role:<uuid>
plan:<uuid>
stage:<uuid>
requirement:<uuid>
issue:<uuid>
work:<uuid>
resource:<uuid>
document:<uuid>
```

CLI 转为 closed `ProjectContextCoordinate`，不把 token 原文写入 wire。

### 14.3 查询命令

```bash
buzz project-context exact \
  --coordinate requirement:<uuid-a> \
  --coordinate requirement:<uuid-b>

buzz project-context incident requirement:<uuid-a>

buzz project-context contains-all \
  --coordinate requirement:<uuid-a> \
  --coordinate resource:<uuid-r>
```

`--format compact` 继续是全局参数。JSON 输出至少包含：

```text
context_revision
projection_generation
query
edges[]
  edge_key
  coordinates[]
  context_documents[]
```

### 14.4 写命令

```bash
buzz project-context attach \
  --context-document <document-uuid> \
  --coordinate requirement:<uuid-a> \
  --coordinate requirement:<uuid-b>

buzz project-context detach \
  --context-document <document-uuid> \
  --coordinate requirement:<uuid-a> \
  --coordinate requirement:<uuid-b>
```

CLI 在提交前读取 verified meta，填入 `expected_context_revision`；发生 CAS conflict 时 exit
`5`，不自动重试。普通 Human 和普通 managed-Agent Community write 默认都省略 Assignment /
Runtime fence；CLI 支持调用者显式提供成对归因字段，但本功能不承诺新增自动装载 Role Binding
或 runtime-fence 的隐式路径。

缺少 NIP-11 extension 时，CLI 禁止 attach；若仍能取得同 signer / generation 的 verified
Context meta，则允许只读查询和精确 detach，支持 capability-off cleanup。不存在可验证 meta
时统一返回 unavailable。

CLI 不组合创建 Document。调用者先使用 `buzz documents create` 创建内容，再 attach；
这样 Document 仍是可独立存在的普通项目内容对象。

## 15. 权限、隐私与安全

### 15.1 写权限

command transport 要求现有 `messages:write` scope。领域授权复用：

- direct Community member；或
- owner 仍是 active member 的 managed Agent；
- ban / timeout / active write restriction 拒绝；
- Human 必须省略 Assignment / Runtime fence；
- managed Agent 可以省略二者并使用现有 Community authority；
- managed Agent 显式归因时，Assignment 必须是 actor 当前 active Assignment，且成对的
  Runtime fence 必须通过现有 supervised Runtime 校验。

attach / detach 复用当前 Project Document writer 的 Community authority，不要求 actor 能修改
任一坐标对象，也不把坐标当作 owner/maintainer。V0 的 Project View 与 Project Document
没有对象级私有 ACL，因此上述 Community writer gate 就是两者现有写授权的组合；若以后任一
来源域增加 finer-grained ACL，Context resolver 必须同步组合检查，不能继续只检查 membership。
不要求某个专用 Context Role，不创建 Edge ACL，也不因 Edge 关联授予 Document 权限。

### 15.2 读权限

command 和 projection 都是 Community-private protocol。必须更新所有读取门：

- WebSocket REQ historical；
- live subscription fan-out；
- NIP-45 COUNT；
- NIP-98 `POST /query` / `POST /count`；
- IDs-only 和 kindless filters；
- event point read；
- search / fallback candidate paths；
- Community-private protocol allow / exclude helpers。

未认证者、非成员和错误 host 不能观察 event 是否存在、数量、tags、坐标或 Document ID。

V0 中 Project View 与 Project Document 的读可见性都是 Community-wide member visibility，
所以 Community-private binding projection 不会扩大现有可见范围。`enabled` 只控制 capability
广告与 attach；授权成员在 structural readiness 健康时仍可读取已存在投影和执行 detach。
未来若坐标或 Document 获得不同的对象级读 ACL，必须先改变 projection / query 授权模型，
不能让共享 binding event 先暴露受限 ID、再仅在 hydration 阶段标成 `unavailable`。

### 15.3 内容安全

Binding projection 不含 Markdown。按需读取的 Context Document 继续是 untrusted project
content，不能提升 system / user instruction 优先级，不能授予外部权限，也不能自动执行
其中命令。Relay 不读取正文来判断它是否“足够解释性”，也不据此拒绝 attach、产生 Gap、
标记冲突或改写关系；解释性二阶语义的边界仍由实际工作的 Human / Agent 维护。

## 16. ACP 与 Agent Context

### 16.1 `[Project Space]` contract v4

`crates/buzz-acp/src/project_space.rs` 将 contract version 从 `3` 提升到 `4`，加入稳定且
capability-neutral 的最小语义：

```text
Buzz supports Project Context Edges that connect an exact unordered set of two
or more Project View or Document coordinates. One or more Project Documents
carry the explanatory context for that set. Discover relevant edges with the
Project Context query commands and read only the needed Document bodies on
demand. Buzz does not infer missing, stale, conflicting, or incorrect context;
when your work materially discovers, creates, or corrects cross-coordinate context,
explicitly write it back.
```

实现时可以做不改变含义的英语润色。不得加入当前 Edge、Document、Project ID 或 Revision。

### 16.2 Base prompt

在 CLI 表加入：

```text
| `buzz project-context` | `exact`, `incident`, `contains-all`, `attach`, `detach` |
```

补充坐标 token 和 metadata-first / body-on-demand 的一句说明，不复制完整协议手册。

### 16.3 明确不修改

- 不向每一 turn 注入 Edge；
- 不把 Edge 加入 Full Role Brief / compact Role Binding DTO；
- 不扩展现有 Context Reference closure；
- 不在 ACP 自动执行 `incident`；
- 不缓存 Document 正文；
- 不由系统自动写回 Context。

contract ID 变化会复用现有 session invalidation，确保旧 session 获得新的稳定认知。

## 17. 与现有 Context Reference 的实现边界

首版明确：

- 不修改 `ProjectContextReference` enum；
- 不修改 `project_view_*_context_references` tables；
- 不修改 `project_context_enabled` flag 或 `buzz-project-context-v1` extension；
- 不从已有 Reference backfill Edge；
- 不把 Edge Document 写回各坐标的 `context_references`；
- 不因同一 Document 同时承担两种角色而报 duplicate；
- Context Document 单 Edge 约束只查询 `project_context_document_bindings`。

Role Brief 继续只沿现有 Context Reference 生成 body-free closure。Project Context Edge 只
通过新查询命令按需发现。

## 18. Capability、初始化与状态恢复

### 18.1 bootstrap 与 readiness

SQL migration 不能生成 Relay-signed meta。实现必须提供明确的 admin surface：

```text
buzz-admin project-context status
buzz-admin project-context preflight
buzz-admin project-context bootstrap
buzz-admin project-context verify
buzz-admin project-context enable
buzz-admin project-context disable
buzz-admin project-context reproject
```

`bootstrap` 在共同 Community exclusive lock 下验证 stable signer、Project View v3 enabled /
read-ready / maintenance normal 和 Project Document enabled / read-ready，然后原子创建 revision
`0`、counts 为 `0`、generation 为 `1` 的 Context state 与 Relay-signed reset meta。它不创建
Edge、binding 或 Context Document，也不从 Context Reference 生成数据。对完全相同的已初始化
状态幂等；任何 signer、pointer 或 counts 不一致都 fail closed，交给 `verify` / `reproject`
处理，不能静默覆盖。

structural read readiness 要求：

1. Relay projection key 已配置；
2. Project View schema v3 已初始化、enabled、read-ready 且 maintenance normal；
3. Project Document v1 已启用且 read-ready；
4. Context Edge state 已初始化；
5. state projection signer / generation 等于当前 Relay；
6. current binding pointers、reset / current meta、规范 counts 与 hashes 可验证。

NIP-11 只有在 structural read readiness 全部满足且
`project_context_edge_enabled = true` 时广告 `buzz-project-context-edge-v1`。已有 Context
Reference capability 是否启用不参与判断。

### 18.2 disable 与恢复

disable 后：

- 停止广告 capability；
- 新 attach 返回 unavailable；
- structural read readiness 健康时，授权 member 仍可读取，并可以精确 detach 已有 binding；
- 规范 DB 数据与 Community-private projections 保留；
- operator verify / reproject 仍可运行；
- 不删除 Edge、binding 或 Document。

重新 enable 前必须通过 signer generation、binding pointers、meta、counts、hash 和 deletion
guard audit。disable / enable 只用于运行时可用性与故障恢复。

### 18.3 signer rotation / reproject

增加 Context Edge reproject maintenance：

1. lock Context state；
2. 增加 projection generation；
3. 从规范 DB 重建所有 current active / deleted binding heads；
4. 生成 reset meta；
5. 原子替换 event store pointers；
6. readiness 恢复后再广告 capability。

reproject 不推进业务 `context_revision`，也不改变 Edge、binding 或 Document 的领域状态。

## 19. 代码改动清单

### 19.1 新增

- `crates/buzz-project-context/**`；
- `crates/buzz-sdk/src/project_context.rs`；
- `crates/buzz-db/src/project_context.rs`；
- `crates/buzz-relay/src/handlers/project_context.rs`；
- `crates/buzz-cli/src/commands/project_context.rs`；
- `crates/buzz-admin/src/project_context.rs`；
- `crates/buzz-test-client/tests/e2e_project_context.rs`；
- `migrations/0047_project_context_edge.sql`；
- `docs/nips/NIP-PCE.md` 与 `docs/nips/fixtures/project-context-edge-v1/**`；
- protocol fixtures、DB integration 和 relay E2E tests。

### 19.2 修改

- workspace `Cargo.toml` 与各消费者依赖；
- `crates/buzz-core/src/kind.rs`；
- Relay ingest / event / req / count / community-private / NIP-11 routing；
- DB event query / count 的 `#c` / `#g` / `#s` pushdown；
- `schema/schema.sql` 与 `crates/buzz-db/src/migration.rs`；
- Project Document deletion-blocked query；
- SDK exports；
- CLI root command 与 client；
- ACP `project_space.rs`、`base_prompt.md` 和 contract tests；
- admin status / preflight / bootstrap / verify / enable / disable / reproject surface；
- test client protocol registry。

### 19.3 首版不修改

- Project View object body、relations 和 Context Reference；
- Role Brief v3 DTO / cache key / renderer；
- Role、Assignment、Checkpoint、Handoff schema；
- Document command、Revision 与正文 wire；
- Desktop、Mobile、Web；
- Meeting 和 Work finalization。

Human UI 在 backend / CLI 行为通过真实使用验证后单独设计。Agent-facing capability 必须先
进入 `buzz-cli`。

## 20. 测试计划

### 20.1 domain unit / property

- 坐标 variant、UUID、数量和 duplicate 校验；
- 任意输入排列得到相同 canonical set / edge key；
- `{A,B}` 与 `{A,B,C}` key 不同；
- project 不同则 key 不同；
- attach 首份 / 附加，以及大量 Document 逐份绑定不依赖单 head 聚合上限；
- detach 非最后 / 最后；
- deleted exact set 在坐标均 active 时 recreate，仍得到相同 deterministic key；
- Document 单 Edge 冲突；
- active 坐标可 attach；tombstone 坐标仍可查询 / detach，但不能 attach；
- Revision overflow、no-op 和 closed JSON；
- revision-zero、counts 和 generation 的非法 shape。

### 20.2 SDK golden protocol

- command exact tags与canonical JSON；
- active / deleted binding projection；
- reset / incremental meta；
- reset meta 下 `binding.context_revision == meta.context_revision` 的合法 reproject fixture；
- content、`d`、`g`、`c`、hash、Revision、signer任一不一致均拒绝；
- projection kind client-submit拒绝；
- unknown fields、explicit null和错误 variant拒绝。

### 20.3 DB integration

- bootstrap empty initialization、幂等与readiness；
- attach / detach完整事务与counts；
- stale CAS、并发attach、replay；
- exact set uniqueness与hash collision fail-closed分支；
- cross-Project / missing coordinate拒绝；
- 坐标 tombstone 后已有 Edge 与 binding 保留，且仍可 detach；
- 使用任一 tombstoned 坐标 attach（包括给 retained Edge 追加 Document）均被拒绝；
- tombstone 之前已经 accepted 的 attach event replay 只返回原 receipt，不重新执行 attach proof
  或推进 Revision；
- Context Document必须active；
- Context Document删除被阻止，detach后可删除；
- 仅作为coordinate的Document可tombstone；
- Project View endpoint tombstone不改变Edge；
- Context Document 正文在坐标 active 或 tombstoned 时都可更新；Edge / binding / Context
  Revision 不变，hydration 取得新的 Document Revision；
- 同一Document可同时作为Context Reference目标、Resource Guide、某Edge的Context Document和
  其他Edge的coordinate；只有第二个active Context binding冲突；
- 同一 Document 也可同时作为同一 Edge 的 coordinate 与 Context Document，两种结构角色
  不互相去重或隐式转换；
- 重叠Edge和Context Reference都不产生backfill、同步或级联；
- fault injection下无半提交；
- reproject / signer rotation不推进业务Revision；
- reset meta 能验证当前全部 binding；最后一次业务 mutation 的 binding 即使与 meta
  `context_revision` 相等，也不要求不存在的 `changed_bindings` entry。

### 20.4 Relay privacy / auth

- 非成员、错误host、缺scope、ban / timeout拒绝；
- Human携带归因字段被拒绝；managed Agent无归因Community write允许；显式Assignment /
  Runtime fence必须成对且当前有效；
- WS REQ、COUNT、`/query`、`/count`、IDs-only、kindless和live fan-out无泄漏；
- capability off时read / detach可用而attach不可用；structural not-ready时全部失败关闭；
- Relay-only projection不能由client伪造。

### 20.5 query / CLI

- exact 0/1语义；
- incident返回binary与hyperedge；
- contains-all只返回superset；
- binding按edge_key正确聚合且Document不重复；
- active filter不被deleted binding head分页挤出；
- 三种查询都覆盖多页、去重、meta双读retry和retry耗尽；
- `#c` / `#g` / `#s`在query与count的WS / HTTP路径都发生LIMIT前pushdown；
- tombstone hydration；
- 瞬时 metadata fetch 失败显示 `unavailable` 且不丢 Edge；verified Document contradiction
  fail closed；
- compact输出不含正文；
- attach / detach receipt；
- conflict exit `5`；
- coordinate token parser与错误提示。

### 20.6 ACP

- Project Space contract version `4`；
- modern与legacy system delivery都包含Edge可发现性；
- `base_prompt = None`时稳定Project Space仍存在；
- contract变化使旧session失效；
- system contract无动态ID / Revision / Document正文；
- Role Brief / Binding payload不新增Edge。

### 20.7 E2E acceptance flow

```text
创建 Requirement A / B 与 Document D1 / D2
        ↓
attach D1 到 {A,B} → 自动建立 Edge
        ↓
exact / incident / contains-all 验证
        ↓
attach D2 → 同一 Edge 两份 Document
        ↓
tombstone A → Edge 仍可查询且 A 显示 tombstoned
        ↓
update D1正文 → Edge / Context Revision不变，查询显示Document新Revision
        ↓
尝试删除 D1 → 被 binding 阻止
        ↓
detach D1 → Edge 仍 active
        ↓
detach D2 → Edge 从当前状态消失
        ↓
删除 D1 / D2 → 成功（若无其他活跃引用）
```

## 21. 验收标准

实现只有同时满足以下条件才算完成：

1. 同 Project exact set 永远只有一个 active Edge。
2. 一条 Edge 可关联多份 Document，一份 Document 最多作为内容属于一条 Edge。
3. 第一份 attach 与 Edge 建立原子发生，最后一份 detach 后没有 active 空 Edge。
4. Context Document 删除保护与 detach 后释放均由事务测试证明。
5. 坐标 tombstone 不触发 Context 级联，三类查询仍能发现 Edge。
6. exact / incident / contains-all 与领域规范完全一致，并能在并发分页时失败关闭。
7. 所有 command / projection 读取路径保持 Community-private。
8. Context Reference 的协议、数据和 Role Brief 行为没有变化。
9. Agent 能从稳定 system contract发现能力，并通过CLI按需读取正文。
10. Edge和正文不自动注入每一turn，系统不推断Gap、过期或冲突。
11. bootstrap产生可验证的revision-zero meta，NIP-11只在完整readiness后广告。
12. signer rotation、replay、reproject和完整性恢复均有测试支持。

## 22. 首版非目标

- Context Reference迁移、替代或自动联动；
- directed / typed Edge；
- 同exact set多Edge；
- Context Document多Edge归属；
- 单坐标Edge；
- 使用 tombstoned 坐标执行新的 attach；已有 Edge 的查询、正文修正与 detach 不受此项影响；
- 自动Gap / freshness / conflict / trust判断；
- 自动摘要、向量索引或Context Compiler；
- Document正文自动注入Role Brief；
- 跨Project Edge；
- 自动移动Context Document；
- Context专用ACL或maintainer；
- Desktop / Mobile / Web UI；
- 新业务HTTP endpoint。

## 23. 当前结论

实现以“Document binding”为最小写入单位，以“规范坐标集合”为Edge唯一身份：

```text
attach(active Document, exact coordinate set)
        ↓
事务保证Document单Edge归属
        ↓
同exact set的bindings聚合为唯一无向Edge
        ↓
Relay为每份binding签名current head，并维护meta observation
        ↓
Agent通过exact / incident / contains-all按需发现
        ↓
buzz documents get按需读取正文
```

这条路径复用了Buzz已经具备的Project View坐标、Project Document内容、Nostr协议、
Community权限和Agent CLI，同时保持系统只记录结构状态、Human / Agent负责真实语义的
边界。

## 24. 阶段开发计划

这里的“阶段”只表示代码依赖和可验证的开发顺序，不构成独立可用版本。全部阶段共同形成
一个完整的 Project Context Edge 后端能力；任何阶段都不能以牺牲第 21 节领域与安全不变量
来换取局部可运行。

### 阶段一：领域与 wire contract

实现：

- 新建 `docs/nips/NIP-PCE.md` 和共享 golden fixtures；
- 注册 `40908`、`40909`、`44302` 及 collision assertions；
- 新建 `buzz-project-context` pure crate；
- 实现 `buzz-sdk/src/project_context.rs` 的 event builders、strict parsers 和 projection
  verifiers；
- 固定坐标 closed union、canonical order、确定性 Edge key、command / projection / frame
  字节边界、JSON depth 与 Revision limits；
- 固定 command、receipt、active / deleted binding、meta 的 closed JSON 与 exact tags；
- 实现 attach / detach pure reducer、canonical errors 和 projection plan；
- 先把全部新 kind 注册为 Community-private，并固定 relay-only / command / global-only 分类；
- 在任何新 kind 可进入 event store 前，完成 WS REQ / COUNT、HTTP `/query` / `/count`、
  IDs-only、kindless、point-read、live subscription 与 wildcard candidate paths 的 privacy
  floor。

验证：

- 任意坐标排列得到相同 canonical set 与 Edge key；
- duplicate、非法 UUID、未知字段、错误 variant、非 canonical JSON 与越界输入全部拒绝；
- command 本身合法但派生 binding content 或完整 EVENT frame 越界时，也必须在 commit 前
  稳定拒绝；
- 私有 kind 的显式、混合、IDs-only 和 kindless filters 在全部 read paths 都经过 membership /
  host gate；
- `{A,B}`、`{A,B,C}` 和不同 Project 的 Edge key 不混淆；
- 一份 binding event 只承载一份 Context Document，Document 数量不会累积到一个 projection；
- command、binding、meta golden fixtures 可以独立 round-trip；
- content、tags、`d`、`g`、`c`、hash、Revision 或 signer 任一不一致时 verifier 拒绝。

完成条件：领域 / wire 部分不连接数据库即可证明不变量和唯一 canonical 表示；privacy
部分使用现有 generic event-store harness 证明新 kind 即使被测试写入，也不能从任何
wildcard read path 泄露。

### 阶段二：规范存储、初始化事务与完整性

依赖阶段一。

实现：

- 增加 migration `0047_project_context_edge.sql`，同步 `schema/schema.sql` 和 migration tests；
- 实现 state、Edge、generic coordinate index、stateful Document binding 与 accepted change
  ledger；
- 实现 exact-set hash collision guard、Document 单 active binding 和 deferred validator；
- 在 DB commit boundary 阻止 active Context Document 进入 tombstone，并建立删除保护索引；
- 实现 empty catalog 的存储事务、storage-level status / preflight / verify 与 integrity audit；
- 实现 replay-first lookup、Project-scoped `context_revision` CAS 和规范 counts；
- 建立 coordinate、edge query 与 active binding indexes。

验证：

- 同 Project exact set 永远解析到同一 Edge row；
- 一份 Document 最多有一条 active binding，但 detached transport row 可被安全复用；
- active Edge 至少有一条 active binding，deleted Edge 没有 active binding；
- canonical JSON、normalized coordinates、Edge key、binding pointers 与 counts 完全一致；
- cross-Project、missing identity、错误 PV object type、inactive Context Document 和模拟 hash
  collision 全部 fail closed；
- empty catalog 存储事务原子、幂等，并只接受与已签 revision-zero reset meta 完全一致的
  projection pointer / counts；
- replay 不重复推进 Revision、counts 或 projection pointers。

完成条件：数据库能保存 reducer 的全部合法结果，并在 commit time 拒绝任何违反 spec 的
状态；除必须由 Relay key 完成的签名外，不存在依赖 Relay post-check 才成立的规范不变量。

### 阶段三：Relay 原子写入与私有协议

依赖阶段二。

实现：

- 接入 Project Context command ingest 与 `messages:write` scope；
- 接入 admin status / preflight / bootstrap / verify / enable / disable，并由 Relay signer 完成
  revision-zero reset meta 后再原子初始化 catalog；
- 实现 structural readiness、capability advertisement 和 operation-specific gate；
- 复用 Community advisory lock，并在其后取得 Context state row lock；
- 实现当前 actor / scope / Assignment / Runtime fence 重验、replay、CAS 和 receipt；
- 实现 attach / detach 的 binding + meta 签名与原子 event-store replacement；
- commit 后 fan-out command、binding 和 meta，不在 commit 后制造业务拒绝；
- 将阶段一已经建立的 Community-private floor 与真实 command ingest、projection replacement
  和 fan-out 路径联调，不能在业务 handler 中另开旁路。

验证：

- 首 attach、追加 Document、非最后 detach、最后 detach 与 all-active exact-set recreate 均
  产生正确 state、counts、binding head 和 meta；
- bootstrap 原子、幂等，且只有签名 reset meta、规范空 catalog 和 event-store pointer
  同时提交后才达到 structural readiness；
- stale Revision 返回稳定 conflict，同一 event replay 返回已保存 receipt；
- disabled 时 read / detach 可用、attach 及 attach replay unavailable；not-ready 时全部失败关闭；
- Human 不能伪装 Assignment；managed Agent 无归因 write 可用，显式归因必须成对且当前有效；
- 任一 fault injection 不留下半提交状态或签名投影与规范 DB 不一致；
- 非成员、错误 host、缺 scope、ban / timeout 与 client 伪造 Relay projection 全部被拒绝；
- 未授权调用者不能从任何读取路径观察事件、count、tag、坐标或 Document ID 是否存在。

完成条件：原始 Nostr command 已能安全、原子地完成全部 Edge 结构写入，且协议在所有读取
入口都保持 Community-private。

### 阶段四：Document 与坐标生命周期集成

依赖阶段三。

实现：

- 将 `state = active` Context binding 接入 Project Document 的 user-facing
  `deletion_blocked` precheck；底层 commit guard 已由阶段二建立；
- 保持 Document coordinate 与 Context Document binding 两种结构角色独立；
- 保持 Project View / Document coordinate tombstone 不修改 Edge、binding 或 Context Revision；
- 保证 Document delete、Context attach 和 detach 使用相同 Community lock order；
- 复用 `buzz documents update` / `patch` 作为 Context Document 语义修正路径；
- 保持 Context Reference、Resource Guide 与 Context Edge 完全独立。

验证：

- 已绑定的 Context Document 不能删除，detach 后在没有其他保护时可以删除；
- 仅作为坐标的 Document 可以 tombstone；
- PV / Document 坐标 tombstone 后，DB / resolver 仍保留 Edge、coordinate index 与 tombstone
  identity，且不推进 Context Revision；
- Edge 含 tombstoned 坐标时 attach 被拒绝，现有 Context Document update 与精确 detach 仍可用；
- attach 与 Document delete 并发时不能同时成功并产生悬空 binding；
- Context Document 正文在坐标 tombstone 后仍可更新；Edge / binding / Context Revision
  不变，hydration 取得新的 Document Revision；
- 同一 Document 可同时承担 Context Reference target、Resource Guide、一个 Edge 的 Context
  Document、同一 Edge 的 coordinate 和其他 Edge 的 coordinate；只有第二个 active Context
  binding 冲突；
- 重叠 Edge、对象更新和 Context Reference 都不发生隐式 backfill、同步或级联。

完成条件：跨领域生命周期、角色独立性和并发行为全部符合领域规范。

### 阶段五：查询与 Agent CLI

依赖阶段四。

实现：

- 为 kind `40908` 的 `#c` / `#g` / `#s` 增加 query 与 count 的 SQL pushdown；
- 实现 meta 双读、稳定分页、完整重试、event 去重和 binding-to-Edge 聚合；
- 实现 `exact`、`incident`、`contains-all` verified loaders；
- 批量 hydrate Project View / Document 轻量 metadata、tombstone 与 unavailable state；
- 新增 `buzz project-context` 坐标 token parser；
- 实现三个查询命令与 attach / detach，并保持 compact output body-free；
- 为每份 Context Document 返回明确的按需正文 fetch command。

验证：

- `exact` 只有 0 / 1 结果且不返回超集；
- `incident` 返回所有包含单坐标的 binary Edge 与 hyperedge；
- `contains-all(Q)` 只返回 Q 的超集，空 Q 返回全部 active Edge；
- 多份 binding 正确聚合为一条 Edge，deleted binding 不进入 active 结果；
- 三类查询在 mutation 穿越分页时完整重试，重试耗尽明确返回 snapshot conflict；
- custom tag 在 WS / HTTP query 与 count 路径都于 SQL `LIMIT` 前过滤；
- tombstoned 坐标仍显示；瞬时 hydration unavailable 与 verified contradiction 使用不同错误
  路径；Markdown 正文不出现在列表或 compact output；
- CLI conflict 使用 exit code `5`，不自动 retry、rebase 或移动 Document。

完成条件：Human / Agent 仅通过 CLI 就能完成 spec 中的发现、关联、修正正文和解除流程。

### 阶段六：ACP 稳定认知

依赖阶段五已经固定 CLI contract。

实现：

- 将 `[Project Space]` contract 提升到 version `4`；
- 告知 Agent Edge 的精确无向集合、Document 内容载体、三类查询和显式写回责任；
- 在 base prompt 中登记 `buzz project-context` 与 metadata-first / body-on-demand 原则；
- 复用 contract ID 变化触发的现有 session invalidation。

验证：

- modern 与 legacy system delivery 都包含稳定的 Project Context 可发现性；
- system contract 不包含动态 Edge、Project ID、Revision 或 Document 正文；
- Role Brief、compact Role Binding 和 Context Reference closure 不新增 Edge；
- ACP 不自动执行查询、不缓存正文、不向每一 turn 注入 Context；
- contract 明确系统不判断 missing、stale、conflicting、incorrect，也不自动产生 Gap；
- Agent 只在真实工作发现、创造或修正跨坐标语义时显式写回。

完成条件：Agent 知道能力存在、如何按需使用和何时维护，但系统没有越过 Human / Agent 的
语义责任边界。

### 阶段七：状态恢复与整体验收

依赖前六个阶段。

实现：

- 完成 signer reproject，并强化 status / preflight / verify / disable / enable 的恢复审计；
- 补齐 domain、SDK、DB、Relay、CLI、ACP 和端到端测试；
- 在本地真实 Relay、PostgreSQL、Redis、CLI 与 ACP 进程上执行完整验收；
- 运行定向测试、`just test-unit`、需要基础设施的 `just test` 和最终 `just ci`。

验证：

- signer reproject 后全部 current active / deleted binding heads 与 reset meta 可重新验证；
- reproject 前后 `context_revision`、Edge、binding 和 Document 领域状态不变；
- 完整通过“创建坐标与 Document → attach → 三类查询 → tombstone → 更新正文 → 删除保护 →
  detach → Edge 消失 → Document 删除”的真实 E2E；
- Context Reference 协议与行为回归不变；
- Desktop、Mobile 和 Web 没有实现改动；
- 第 21 节全部验收标准通过。

完成条件：后端与 Agent surface 形成一个完整闭环，不依赖后续阶段补齐任何已承诺的领域、
隐私、事务或恢复不变量。
