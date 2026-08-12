# [已废弃] Project Context Agent 渐进检索实现设计

> 状态：已废弃，不得作为实现依据
>
> 日期：2026-08-08
>
> 废弃日期：2026-08-09
>
> 代码基线：`version/v1.0.0` @ `f5cec1716`
>
> 废弃原因与后续设计方向见
> [Meeting 上下文讨论历程](meeting-context-discussion-history.md)中的“第六次纠正”。

## 0. 废弃声明

本文保留，仅用于追溯 2026-08-08 时形成、随后被否定的实现推演。不要据此创建 migration、event kind、
数据库表、CLI 命令、ACP Prompt 或其他实现。

本文的主要失效假设包括：

- 把统一的坐标摘要实现成 Project Context 自有的独立 Node canonical state；
- 为此新增 Coordinate Node 协议、Node Head、Node Meta、Node Catalog 和独立 Revision；
- 把 Edge 与 Node 的分页、快照和投影一致性提升为实现主轴；
- 在确定 Agent 的实际遍历动作之前，先围绕大规模 Frontier、全量水合和迁移建立复杂协议。

后续讨论重新确定了更直接的语义边界：

- 摘要属于可读取的内容，不属于 Edge；
- Edge 只保留精确 Coordinate 集合与 Context Document membership；
- 普通 Project Document 自己拥有 title、summary 和正文；作为 Edge Context Document 时直接用其
  Document summary 判断是否值得读取；
- Node Preview 中的摘要描述 Node 所指内容，其语义与生命周期属于来源内容实体；查询层只能读取、
  适配和水合来源 metadata，不能成为摘要 owner 或生成第二份 canonical summary；
- Agent 的标准检索路径是“选中 Node → 读取 Node 内容 → 查看 incident Edge 的 Context Document
  摘要 → 选择并读取关系文档 → 查看相邻 Node 摘要 → 选择下一个 Node”；
- 分页只是在单步扇出过大时保持响应有界的低层机制，不是语义路径检索本身。

新的实现设计尚未在本文中给出，必须另行撰写。

## 1. 文档目的

本文把已经确认的设计映射到 Buzz 当前的 Nostr-first 协议、PostgreSQL canonical state、
Agent-first CLI 与 ACP Prompt 架构，说明首版如何实现：

- 同一 Project 中每个图内 Coordinate 的统一检索摘要；
- Coordinate 首次进入图时的摘要写入，以及原内容更新后的乐观维护；
- title / summary 优先、正文按需的渐进检索；
- Coordinate 完整内容和 Edge Context Document 两类并列读取目标；
- Agent 基于当前问题、Role、Work 和 Runtime Context 自主选择图路径；
- 有界分页、来源校验、覆盖边界、迁移与权限闭环。

这不是 Meeting 实现。能力应由 Project Context、来源领域、CLI 与稳定 Agent Prompt 共同提供，
任何普通 Agent 在自己的当前工作 Turn 中都可以使用。Meeting 以后若要调用，只能适配这项通用能力，
不能在 Meeting 内复制一套检索协议。

本文也不实现“检索结果跨 ACP Session 搬运”。首版让 Agent 在已有普通工作 Session 中直接调用工具，
因此当前 Runtime Context 天然参与相关性判断；另开 Retrieval Session、Meeting 入场装载和
participant 调度属于后续 orchestration 设计。

## 2. 实现目标与成功形态

实现完成后应满足：

1. `ProjectContextCoordinate` 继续只表示稳定身份；title、summary、状态和 Revision 都不进入身份。
2. Coordinate 出现在至少一条 active Edge 中时，Project Context 有一份 Project-scoped Node 读模型。
3. 新 Coordinate 首次或重新进入图时，写入者必须先读其当前 canonical 完整内容，并原子提交摘要。
4. 同一 Coordinate 被多条 Edge 使用时复用同一摘要，不生成 Role-specific 或 Edge-specific 副本。
5. 来源内容修改后，更新者按“是否改变未来加载决策”判断是否更新摘要；系统不自动失效或重写。
6. Agent 能先有界读取 title / node summary，再决定读取 Coordinate 正文、关系 Document 或继续展开。
7. `exact`、`incident`、`contains-all` 按完整 Hyperedge 分页；Context Documents 单独分页。
8. 图查询不接收 Role 并替 Agent 打分。Role 是 Prompt 中的必要视角，不是 ACL 或硬过滤。
9. 高出度、循环、正文过大、来源变化和历史缺摘要均有明确、有界、可继续的表达。
10. 不创建 Role Context、Meeting Context、Agent 私有知识库或第二张 canonical graph。

## 3. 当前代码基线

### 3.1 可以直接复用的部分

当前代码已经提供：

| 能力 | 当前落点 |
|---|---|
| Coordinate closed union、排序与稳定 tag | `../../../../crates/buzz-project-context/src/coordinate.rs` |
| `Project + exact Coordinate set` 派生 EdgeKey | `../../../../crates/buzz-project-context/src/model.rs` |
| attach / detach、全局 Context revision CAS、pure reducer | `../../../../crates/buzz-project-context/src/command.rs`、`reducer.rs` |
| 一份 Context Document 一条 Relay-signed Binding Head | `../../../../crates/buzz-project-context/src/projection.rs` |
| canonical Edge / Binding / Meta、事务和 reprojection | `../../../../crates/buzz-db/src/project_context.rs`、migration 0049～0053 |
| `exact`、`incident`、`contains-all` 与 metadata hydration | `crates/buzz-cli/src/commands/project_context.rs` |
| Project Space 稳定合同 | `../../../../crates/buzz-acp/src/project_space.rs` |
| 每 Turn verified Role Brief / compact binding | `../../../../crates/buzz-sdk/src/role_brief_v3.rs`、`../../../../crates/buzz-acp/src/pool.rs` |

现有 `ProjectContextBindingProjection` 已包含：

```text
edge_key
完整 canonical coordinates[]
context_document_id
binding state
source event / revision
```

因此，按 Edge 分页不需要新增 Edge Head。数据库只需按 canonical EdgeKey 选择一条 active Binding
作为这条 Edge 的 verified transport carrier；完整 Document membership 再按 EdgeKey 单独分页。

### 3.2 当前必须补齐的断点

1. Coordinate 只有身份，没有统一 Node summary。
2. attach 不能为本次首次入图的 Coordinate 原子提交摘要。
3. 当前 CLI 以 500 条为一页，但会一直读取到空页后全量聚合和水合，不能充当渐进 Frontier。
4. 当前 Query DTO 没有统一 node summary；Document coordinate 已有来源摘要也未作为 Node 摘要使用。
5. Project View hydration 为少量对象仍会读取完整 verified snapshot。
6. 当前 Binding event page 以 Document 为单位，不能直接表达“一页 Edge”。
7. Context Meta 的 ordinary transition 固定为一个 changed Binding，不能表达 summary-only 变更。
8. 当前 Project Context search 明确被通用 NIP-50 路径拒绝。
9. Full Role Brief 未暴露 typed Role Coordinate，Responsible Work 也未暴露 typed Work ID。
10. Meeting history 读取没有可证明“已读到指定 speech revision”的分页合同。

## 4. 总体实现决策

### 4.1 采用增量 Node 协议，不重写 Edge v2

本实现保持现有 Edge v2 作为结构事实协议，新增 Coordinate Node v1 作为同一 Project Context Graph
的节点检索读模型：

```text
Project Context Edge v2                 Coordinate Node v1
-----------------------                 ------------------
exact Coordinate set                    Coordinate identity
Context Document bindings               retrieval summary
edge context_revision                   graph membership state
40908 Binding / 40909 Meta               40910 Node / 40911 Node Meta
```

两者不是两张图：

- Node 是否 active 完全由 active Edge 的 Coordinate 并集派生；
- 新 Coordinate 进入 Edge 与 Node summary 在同一数据库事务中提交；
- Node 没有独立建点命令，不能形成逻辑孤立节点；
- Node summary 不参与 EdgeKey 或 Edge 判等。

拆开 revision 的原因是摘要维护不应制造无关的 Edge 结构 revision，也不应让一次普通摘要更新使所有
Edge 分页 cursor 冲突。Edge 结构仍使用现有 `context_revision`；Node Head 变化使用
`node_catalog_revision`。

### 4.2 协议与 kind

保留：

```text
KIND_PROJECT_CONTEXT_EDGE_BINDING = 40908
KIND_PROJECT_CONTEXT_META         = 40909
KIND_PROJECT_CONTEXT_COMMAND      = 44302
PROJECT_CONTEXT_EDGE_CAPABILITY   = "buzz-project-context-edge-v2"
```

新增：

```text
KIND_PROJECT_CONTEXT_COORDINATE_NODE = 40910
KIND_PROJECT_CONTEXT_NODE_META       = 40911
PROJECT_CONTEXT_NODE_SCHEMA_VERSION  = 1
PROJECT_CONTEXT_NODE_CAPABILITY      = "buzz-project-context-coordinate-node-v1"
```

当前 kind registry 中 40910、40911 未使用；编码前仍要通过 registry compile-time assertion 固定。

`44302` 按 `schema_version` 解析 v2 / v3 command：

- v2 command 保持现有 attach / detach wire，不修改其 canonical JSON；
- v3 command 支持带 entering summaries 的 attach，以及 summary update；
- Relay 先做有界 JSON parse，再按版本进入 closed struct，不用一个大量 optional 字段的兼容结构。

当前单一`PROJECT_CONTEXT_SCHEMA_VERSION`同时被v2 command、40908、40909和v2 receipt使用。实现前必须
先拆成明确常量，而不是把全crate的常量直接改成3：

```text
PROJECT_CONTEXT_EDGE_SCHEMA_VERSION       = 2
PROJECT_CONTEXT_COMMAND_V2_SCHEMA_VERSION = 2
PROJECT_CONTEXT_COMMAND_V3_SCHEMA_VERSION = 3
PROJECT_CONTEXT_NODE_SCHEMA_VERSION       = 1
```

现有v2`ProjectContextOperation::{Attach,Detach}`保持原类型和wire，不直接加入summary variant。v3使用
独立`ProjectContextCommandRequestV3 / ProjectContextOperationV3`，只有通用审计层再把二者映射为内部
operation class。这样不会破坏v2 parser、receipt、SQL operation CHECK或现有exhaustive match。

NIP-11 同时广告 Edge v2 与 Coordinate Node v1 capability。后者只有在 Node schema、projection、权限、
查询和 canonical parity 全部 ready 时出现。

### 4.3 明确不新增 Edge Head

Edge page 的实现单位为 canonical `project_context_edges.edge_key`，wire carrier 仍是 40908：

```text
一页 EdgeKey
  → 每个 EdgeKey 选择 context_document_id 最小的一条 active Binding
  → 返回该 verified 40908 event
  → reader 从 event 得到完整 Edge identity
```

这条代表 Binding 只承载 Edge identity，不声称代表完整 Document membership。调用方需要关系材料时，
必须调用 `edge-bindings` scope 分页读取这条 Edge 的全部或部分 Context Documents。

## 5. 领域模型

### 5.1 Coordinate identity 保持不变

```rust
pub enum ProjectContextCoordinate {
    ProjectViewObject {
        object_type: ProjectViewObjectType,
        object_id: Uuid,
    },
    Document {
        document_id: Uuid,
    },
    Meeting {
        meeting_id: Uuid,
    },
}
```

不得给这个 enum 增加 title、summary、status 或 revision。现有 canonical order、`c` tag、EdgeKey v1
hash domain 全部保持不变。

### 5.2 `RetrievalSummary`

```rust
pub struct RetrievalSummary(String);
```

确定性校验：

- 输入必须已经 trim，trim 后非空；
- UTF-8 字节数 `1..=1024`；
- 单段纯文本，禁止 CR、LF、NUL 和其他 ASCII control；
- command、canonical row、projection parser 三处重复验证；
- 不用程序伪装验证“两句话”“Role-neutral”等语义质量。

语义质量由稳定 Agent 合同约束：摘要说明“内容有什么”与“什么类型的问题下可能值得加载”，通常
80～200 个汉字；不写当前任务、Role、Meeting、Edge、工具命令、操作步骤、详细事实或关键词堆积。

### 5.3 摘要来源 observation

写摘要时必须提交 Agent 实际依据的当前来源 observation。它只防止“写入时已经过期”，并保留
provenance；以后来源继续更新不会自动让摘要失效。

```rust
pub enum CoordinateContentObservation {
    ProjectView(ProjectViewContentObservation),
    Document(DocumentContentObservation),
    Meeting(MeetingContentObservation),
}

pub enum RevisionedMeetingProtocol {
    ModeratedBatonV1,
    ModeratedBoardV2,
    ModeratedBoardActionsV2Legacy,
    ModeratedBoardActionsV2,
}

pub enum ProjectViewContentObservation {
    Object {
        object_type: ProjectViewObjectType,
        object_id: Uuid,
        object_revision: u64,
        projection_generation: u64,
        head_event_id: EventId,
        object_body_digest: [u8; 32],
        observed_project_revision: u64,
        resource_guide: Option<DocumentContentObservation>,
    },
    ActiveRoleDefinition {
        role_id: Uuid,
        object_revision: u64,
        projection_generation: u64,
        head_event_id: EventId,
        role_body_digest: [u8; 32],
        observed_project_revision: u64,
    },
}

pub struct DocumentContentObservation {
    pub document_revision: u64,
    pub head_event_id: EventId,
    pub body_digest: [u8; 32],
}

pub enum MeetingContentObservation {
    Baton {
        protocol: RevisionedMeetingProtocol,
        create_event_id: EventId,
        metadata_event_id: EventId,
        state_event_id: EventId,
        state_revision: u64,
        board_event_id: Option<EventId>,
        speech_revision: u64,
        end_event_id: Option<EventId>,
    },
    UniformV0 {
        create_event_id: EventId,
        metadata_event_id: EventId,
        state_event_id: EventId,
        state_revision: u64,
        end_event_id: Option<EventId>,
        board_event_id: Option<EventId>,
        formal_speech_count: u64,
        formal_speech_ids_digest: [u8; 32],
    },
}
```

约束：

- `Object.resource_guide`只允许且必须用于active Resource；其他Project View类型必须为`None`；active
  Role必须使用`ActiveRoleDefinition`，因为当前40903将它投影为
  `buzz-project-view-v3-entity / RoleDefinitionV3`，不是ordinary object；Role tombstone才是ordinary
  `buzz-project-view-v3-object`，但tombstone不能作为新summary basis；
- `object_body_digest` 使用 source-domain canonical object bytes，不使用 CLI 展示文本；
- Document digest 使用 canonical Markdown bytes；
- Meeting Board 由 immutable event ID 标识，不发明不存在的 `board_revision`；
- Baton Meeting 使用当前已有 `state_revision` / `speech_revision`；
- UniformV0 Meeting也分别携带verified State与End evidence，不把它们压成一个`terminal_event_id`；其formal
  history使用有序Speech event ID集合的domain-separated digest；
- Meeting attachability 与内容 observation 分开。attach 仍执行当前
  `MeetingCoordinateResolution::Terminal | FinalizingActions` 校验；summary update 只验证来源可读与
  observation 当前，不能借摘要更新绕过或重演 attachability；
- Relay 在 source owner 的 current-row lock 内重算并精确比较 observation。

所有digest都必须在对应source owner中定义domain-separated canonical bytes，不能对CLI展示JSON直接
hash。例如Project View object digest覆盖closed canonical object body，Document body digest覆盖当前
revision Markdown原始UTF-8 bytes；UniformV0 Meeting formal history digest覆盖按正式顺序排列的
`speech_event_id`集合。具体domain字符串随source wire一同进入golden fixture，Project Context只消费
typed resolver结果，不信任调用方任意提供的digest。

Project View basis的currentness以`projection_generation + current head_event_id + object_revision + body
digest`为准；`observed_project_revision`只记录读取时背景，不因另一个无关对象更新而使summary command
冲突。RoleDefinitionV3与ordinary object分别定义digest domain和parser，不能把两种projection JSON
混为同一canonical bytes。

### 5.4 Node summary 状态

正常新数据始终有摘要；只有 v2 历史迁移允许缺失：

```rust
pub enum ProjectContextNodeSummaryState {
    MissingLegacy,
    Present {
        summary: RetrievalSummary,
        basis: CoordinateContentObservation,
        source_command_event_id: EventId,
        authored_by: PublicKey,
        updated_at: DateTime<Utc>,
    },
}
```

`MissingLegacy` 不是一段内容为 `"unknown"` 的伪摘要。它明确表示：此坐标在 Node 协议出现前已经
进入图，尚无人基于完整当前内容补写摘要。查询遇到它时：

- 可以用来源 title 作为弱候选；
- 不能因 summary 缺失断言内容不相关；
- 可以 point read 或进入 `nodes missing` 回填队列；
- 不阻断整个 Project 的渐进检索 capability。

### 5.5 `ProjectContextCoordinateNode`

```rust
pub struct ProjectContextCoordinateNode {
    pub coordinate: ProjectContextCoordinate,
    pub graph_state: ProjectContextNodeState, // active | deleted
    pub node_revision: u64,
    pub last_changed_node_catalog_revision: u64,
    pub summary_state: ProjectContextNodeSummaryState,
    pub membership_source: ProjectContextNodeMutationSource,
    pub updated_at: DateTime<Utc>,
}

pub enum ProjectContextNodeMutationSource {
    Command { event_id: EventId },
    V2Import { migration_id: Uuid, entry_digest: [u8; 32] },
}

pub enum ProjectContextNodeProjectionSource {
    Command { event_id: EventId },
    V2Import { migration_id: Uuid, entry_digest: [u8; 32] },
    Reprojection { run_id: Uuid, canonical_row_digest: [u8; 32] },
}
```

Node 不在 projection 中携带 incident degree。数据库可以维护
`active_incident_edge_count` 作为完全可重建的 transition index；当 degree 从 2 变 3 而 Node 仍 active
时，只更新derived count / `degree_updated_at`，不改Node revision、semantic `updated_at`、last change或
current projection pointer，也不推进Node catalog revision。

Node 的逻辑不变量：

```text
node.graph_state == active
iff
至少一条 active exact Edge 包含该 Coordinate
```

`deleted` Head 只用于历史、审计和 reentry CAS。它不参与 active list、candidate search、Frontier、
结构 seed 或 traversal。

### 5.6 Node Catalog

```rust
pub struct ProjectContextNodeCatalog {
    pub node_catalog_revision: u64,
    pub active_coordinate_count: u64,
    pub missing_summary_count: u64,
    pub projection_generation: u64,
}
```

`active_coordinate_count` 等于所有 active Edge Coordinate 的并集大小；`missing_summary_count` 只统计
active `MissingLegacy` Node。摘要 update 不改变 Edge v2 的 `context_revision`。

## 6. 写协议与生命周期

### 6.1 v3 command

```rust
pub struct ProjectContextCommandV3 {
    pub schema_version: u16, // 3
    pub acting_assignment_id: Option<Uuid>,
    pub runtime_fence: Option<RuntimeFence>,
    pub request: ProjectContextCommandRequestV3,
}

pub enum ProjectContextCommandRequestV3 {
    Attach {
        expected_context_revision: u64,
        coordinates: Vec<ProjectContextCoordinate>,
        context_document_id: Uuid,
        entering_nodes: Vec<EnteringCoordinateNode>,
    },
    Detach {
        expected_context_revision: u64,
        coordinates: Vec<ProjectContextCoordinate>,
        context_document_id: Uuid,
    },
    UpdateCoordinateSummary {
        coordinate: ProjectContextCoordinate,
        expected_node_revision: u64,
        summary: RetrievalSummary,
        basis: CoordinateContentObservation,
    },
}

pub struct EnteringCoordinateNode {
    pub coordinate: ProjectContextCoordinate,
    pub expected_previous_node_revision: Option<u64>,
    pub summary: RetrievalSummary,
    pub basis: CoordinateContentObservation,
}
```

`expected_previous_node_revision=None` 只用于从未存在的 Node；reentry 必须传 deleted Head revision。

产品支持路径仍是Agent依照第14节合同读取完整内容并生成摘要，但wire不能证明签名者背后是否为某个
模型，也不新增“只有模型才能写”的身份类别。Relay验证现有Project Context writer权限、Assignment /
Runtime fence、内容shape与source observation；协议层只保留实际签名author，不能把author类型冒充成
摘要语义质量证明。

v3继续服从现有`MAX_COMMAND_CONTENT_BYTES=65_536`，并固定：

```text
MAX_ENTERING_NODE_SUMMARIES_PER_ATTACH = 16
```

`entering-node-manifest`只是CLI本地输入，签名时仍内联进一个closed command，不是另一个持久对象。
CLI必须在签名前按最终canonical JSON检查精确字节数；Relay重复检查。超过16个entering Node或最终
command超过64KiB时，返回`unsupported:project_context:entering_summary_batch_too_large` /既有
`content_too_large`，不能拆成虚假Edge规避。首版明确不支持这种单次新建 / reentry；若真实数据证明
需要，后续协议应设计可原子消费、非graph Node的chunked pending manifest，而不是静默放宽event frame。

当前假定每个 operation 都有 `coordinates()` 和 `context_document_id()` 的 API 要拆成：

```text
operation()
affected_coordinates()
edge_coordinates() -> Option<&[Coordinate]>
context_document_id() -> Option<Uuid>
updated_coordinate() -> Option<&Coordinate>
```

### 6.2 attach 的 entering set

Relay 在 Community / Edge write lock 内，以 pre-state 计算：

```text
entering = 激活本次 exact Edge 后，incident active Edge 数从 0 变成 1 的 Coordinates
```

规则：

1. `entering_nodes.coordinates` 必须与 `entering` 精确相等；少、重、额外均拒绝。
2. 每项来源必须仍 active / 可读，basis 必须当前。
3. 已 active Node 直接复用，不允许 attach 顺带覆盖它的摘要。
4. 同一 active Edge 增加第二份 Context Document 时 `entering` 为空。
5. deleted Node reentry 必须重新读取当前内容并提交当前摘要，即使文本碰巧未变。
6. 任一 Node projection、Binding、Meta、Receipt 或 canonical write 失败，整个 attach 回滚。
7. summary 不放宽现有 Coordinate family、同 Project、Document 或 Meeting attachability 条件。

历史 `MissingLegacy` Node 已经 active，不属于 entering。新 attach 可以复用它，但不得把“缺摘要”
解释为内容无关；补写通过独立 summary update 完成。

### 6.3 v2 command 兼容

Node capability 启用后：

- v2 detach 始终继续接受，确保旧客户端和恢复路径可以移除 Binding；
- v2 attach 只有在 Relay 计算出的 `entering` 为空时才接受；
- v2 attach 若会引入新 / deleted Coordinate，返回
  `conflict:project_context:node_summary_required`；
- v2 command 被接受且导致 Node 离图时，Relay 仍在同一事务中产生 deleted Node Heads / Node Meta；
- v2 parser 和 canonical bytes 不被 v3 parser“宽松兼容”改写。

### 6.4 summary update

`UpdateCoordinateSummary`：

- 只允许 active Node；
- `expected_node_revision` 必须精确匹配；
- `MissingLegacy → Present` 与 `Present → Present` 使用同一 operation；
- Relay 在来源锁内重算 basis；
- 更新 Node Head，推进一次 `node_catalog_revision`；
- 不改变 Coordinate、EdgeKey、Edge、Binding、Context Document 或 Edge `context_revision`；
- 同一摘要文本且当前状态已经 `Present` 时沿用现有 `NoChange` 拒绝语义：不存 command / receipt，
  不推进 revision；
- deleted Node 不能用 update 形成孤立节点，必须在合法 attach reentry 中复核。

### 6.5 乐观维护

来源对象 update 与 Node summary update 不做跨领域强事务：

```text
source update 成功
  → 更新者读取/查看当前 active Node summary
  → 判断变化是否改变未来加载决策
  → 必要时显式 UpdateCoordinateSummary
```

系统不会：

- 因来源 Revision 变化自动 stale；
- 自动调用模型重写摘要；
- 阻止来源对象正常更新；
- 在摘要未更新时阻断图读取。

来源更新成功、摘要维护随后失败时，Agent 必须报告 partial maintenance；不得把来源写入伪装为失败，
也不得声称摘要已经维护。

### 6.6 detach、来源 tombstone 与 reentry

- 非最后一条 incident Edge 消失：只更新派生 degree，不签 Node Head。
- 最后一条 incident Edge 消失：Node 变 deleted，保留最后 summary / basis / author。
- 来源 tombstone 不级联删除 Node / Edge；Preview 显示来源状态，最后摘要仍可导航历史。
- deleted Head 不可遍历。
- 后续重新入图必须基于可读的当前来源重新提交 `Present` summary；不能无判断复用旧文本。

## 7. Relay-signed Node projections

### 7.1 Coordinate Node Head：40910

```rust
pub struct ProjectContextCoordinateNodeProjection {
    pub schema_version: u16,             // 1
    pub projection_type: NodeProjectionType, // coordinate_node
    pub project_id: Uuid,
    pub projection_generation: u64,
    pub projection_source: ProjectContextNodeProjectionSource,
    pub node: ProjectContextCoordinateNode,
}
```

canonical tags：

```text
["-"]
["d", "project-context-node:<canonical-c-value>"]
["t", "buzz-project-context-coordinate-node"]
["t", "coordinate_node"]
["s", "active" | "deleted"]
["c", "<canonical-coordinate-tag>"]
["projection_generation", "<decimal>"]
["node_catalog_revision", "<last-changed-decimal>"]
["node_revision", "<decimal>"]
["source", "command", "<event-id>"]
  or
["source", "v2_import", "<migration-uuid>", "<entry-digest>"]
  or
["source", "reprojection", "<run-uuid>", "<row-digest>"]
```

summary、basis、author 只在 closed body 中，不进入可搜索 tag。`source` tag映射projection envelope的
`projection_source`，即“为什么产生当前这个Head”：summary update时是该command，detach时是detach
command，初次导入时是migration。Node body中的`membership_source`仍表示最后一次membership状态变化，
`summary_state=present`内的source表示最后一次摘要写入；reprojection只改变projection source，不改前两
项。三者不能混成一个含义。

### 7.2 Node Meta：40911

```rust
pub struct ProjectContextNodeMetaProjection {
    pub schema_version: u16,
    pub projection_type: NodeProjectionType, // node_meta
    pub project_id: Uuid,
    pub projection_generation: u64,
    pub node_catalog_revision: u64,
    pub active_coordinate_count: u64,
    pub missing_summary_count: u64,
    pub reset: bool,
    pub changed_node_count: u32,
    pub changed_nodes_digest: [u8; 32],
    pub source_event_id: Option<EventId>,
    pub updated_at: DateTime<Utc>,
}
```

签名顺序：

1. 先签所有 changed Node Heads；
2. 按 canonical Coordinate bytes 排序；
3. 计算：

```text
SHA-256(
  "buzz-project-context-changed-nodes-v1\0"
  || u32_be(count)
  || repeated(
       canonical_coordinate_bytes
       || node_event_id
       || node_revision_u64_be
       || state_byte
     )
)
```

4. 最后签 Node Meta。

Node Head 不重复 change-set digest，避免 event ID 与 digest 的签名循环。零项使用相同 domain 加
`count=0` 的确定性 digest。`state_byte`固定为`active=0x01`、`deleted=0x02`。

普通 operation 只有在至少一个 Node Head 改变时才生成新 Node Meta。migration / reprojection 使用
`reset=true`、`source_event_id=None`、`changed_node_count=0`及零项digest；reset要求reader丢弃旧cache
并重新分页读取current Node Heads，不把全catalog change set塞进Meta。普通command使用`reset=false`
且source必须存在。

Node Meta exact tags：

```text
["-"]
["d", "project-context-node-meta:<project-id>"]
["t", "buzz-project-context-coordinate-node"]
["t", "node_meta"]
["projection_generation", "<decimal>"]
["node_catalog_revision", "<decimal>"]
["e", "<source-command-event-id>", "", "source"]   // ordinary only
```

40911镜像现有40909的canonical indexed `d`做法，并加入`has_indexed_d_tag`断言；它不是无d-tag的
singleton特例。reset Meta省略source `e` tag。

增量Head不要求与当前Meta revision相等。准确不变量是：

```text
Node Head.node_revision == canonical row.node_revision
Node Head.last_changed_node_catalog_revision
  == canonical row.last_changed_node_catalog_revision
  <= node_state.node_catalog_revision

current Node Meta.node_catalog_revision == node_state.node_catalog_revision
ordinary Meta changed set中的每个Head.last_changed_node_catalog_revision
  == 该Meta.node_catalog_revision
```

未变化Node继续指向旧revision Head是正常状态；全量parity不能因此要求重签全图。

### 7.3 Receipt

v2 receipt和现有`project_context_edge_changes.result`保持原样。Node侧新增closed receipt：

```text
ProjectContextNodeReceiptV1
├── schema_version = 1
├── change_id / actor / acting_assignment_id? / accepted_at
├── operation                    edge_attach | edge_detach | summary_update
├── previous_node_catalog_revision
├── node_catalog_revision        previous + 1
├── changed_node_count / changed_nodes_digest
├── node_meta_event_id
├── edge_change_id?              required for attach / detach
└── summary_target?              coordinate + expected/new node revision for summary_update
```

Receipt不内联无界Node列表；精确集合由durable change items、Node Meta digest与signed Node Heads验证。
v3 command响应外层固定
`ProjectContextCommandResponseV3 { schema_version: 3, command_event_id, result }`，其中`result`是closed
union：

```text
edge command     { edge_receipt: <existing v2 receipt>, node_receipt: <v1 receipt?> }
summary update   { node_receipt: <v1 receipt> }
```

Edge command没有Node Head变化时`node_receipt=None`。v2 caller仍只收到原v2 receipt，wire不被组合响应
破坏；其command若导致Node离图，Node change仍在服务端审计和projection中记录。

## 8. PostgreSQL canonical storage

### 8.1 Migration

新增 additive migration：

```text
0054_project_context_coordinate_node_v1.sql
```

核心表：

```text
project_context_coordinate_nodes
├── community_id
├── canonical_key
├── coordinate_type / coordinate_subtype / coordinate_id
├── graph_state                         active | deleted
├── active_incident_edge_count          derived
├── degree_updated_at                    derived audit only
├── node_revision
├── last_changed_node_catalog_revision
├── summary_state                       missing_legacy | present
├── summary                             nullable by state
├── summary_basis                       nullable closed JSONB by state
├── summary_source_event_id             nullable by state
├── summary_updated_by / at             nullable by state
├── membership_source                   closed JSONB
├── last_node_change_id
├── current_projection_event_id
└── created_at/by, updated_at/by

project_context_node_state
├── community_id PK
├── schema_version
├── migration_phase                     building | projecting | ready
├── node_catalog_revision
├── active_coordinate_count
├── missing_summary_count
├── projection_generation
├── projection_signer_pubkey
├── last_node_change_id?
├── last_reset_migration_id?
├── current_meta_event_id
└── updated_at

project_context_node_changes
├── community_id / change_id            member command Event ID
├── actor / acting_assignment_id?
├── operation                           edge_attach | edge_detach | summary_update
├── previous_node_catalog_revision / node_catalog_revision
├── edge_change_id? / edge_context_revision?
├── changed_node_count / changed_nodes_digest
├── node_meta_event_id
├── result                              exact ProjectContextNodeReceiptV1 JSON
└── accepted_at

project_context_node_change_items
├── community_id / change_id / canonical_key
├── previous_node_revision?
├── node_revision / graph_state
├── node_head_event_id
└── summary_changed

project_context_node_migrations
├── community_id / migration_id
├── source_edge_context_revision / active_union_digest
├── target_generation / candidate_signer_pubkey
├── row_count / rows_digest
├── state                              building | projecting | ready | failed
└── created_at / completed_at?

project_context_node_migration_items
├── community_id / migration_id / canonical_key
├── entry_digest
└── staged row / staged event pointer
```

`membership_source` 使用 closed JSONB shape，不能用一个 `source_type + source_id` 强塞 command Event 和
migration UUID / digest 两种不同来源。
当前Head的`projection_source`从`last_node_change_id`对应ledger、initial migration item或reprojection run
重建，不另存一段无法校验的自由JSON；reprojection只改pointer / generation，不覆写membership与summary
provenance。

现有`project_context_edge_changes`不做泛化：它的attach/detach CHECK、unique context revision和v2
receipt JSON继续成立。一个会改变Node Heads的attach / detach在同一事务中同时写Edge change与Node
change，两表可以共享同一个member command Event ID；summary update只写Node change。Node
`last_node_change_id`与Node state `last_node_change_id`分别指向新ledger。

Replay必须发生在任何current-state / expected-revision校验之前：

- v2 edge command从Edge change返回原v2 receipt；
- v3 edge command从Edge change加可选Node change重建原组合响应；
- summary update从Node change返回原Node receipt；
- 同一SQL事务保证不会只存在组合响应的一半；检测到半边记录属于integrity failure，不重新执行command。

### 8.2 约束

- PK `(community_id, canonical_key)`；typed identity 另有 unique。
- identity columns 与 canonical key 禁止 update；Node row 禁止 hard delete。
- `graph_state=active iff active_incident_edge_count>0`。
- `summary_state=present` 时 summary / basis / author / source 全部非空；`missing_legacy` 时全部为空。
- `missing_legacy` 只能由 v2 import 产生；普通 command 不得新建这种状态。
- summary trimmed、1..=1024 UTF-8 bytes、无 control。
- active Coordinate count、missing count 与 rows 严格相等。
- active Edge 的 Coordinate 并集与 active Nodes 严格相等。
- `migration_phase=ready`时current Node / Meta projection pointers必须指向同Project、generation、对应
  last-changed revision、Node catalog signer的事件；building / projecting不允许public Node rows / pointers
  被read gate观察，state pointer可以为空。
- ready Node state的`projection_signer_pubkey`必须与Edge state当前stable signer一致；独立readiness不
  表示允许两套current signer分叉。
- Node change header的`node_catalog_revision`在Community内唯一且等于previous + 1；items数量/digest、
  Meta event与receipt精确一致。
- Node Head只需匹配row的`last_changed_node_catalog_revision`，该revision可以小于current Node Meta；
  degree-only update不得改任何projected字段。

`project_context_edge_coordinates` 不增加指向 Node 的普通 FK。该表保留历史 deleted Edge，而迁移只为
当前 active Coordinate 并集建立 Node；active Edge → active Node 由事务和 parity函数证明。

### 8.3 索引

新增：

```text
project_context_edge_coordinates
  (community_id, canonical_key, edge_key)

project_context_coordinate_nodes
  (community_id, canonical_key)
  WHERE graph_state = 'active'

project_context_coordinate_nodes
  (community_id, summary_state, canonical_key)
  WHERE graph_state = 'active'
```

首版 candidate search不把 summary复制到第二份 canonical表。若性能数据证明必要，可以加
`pg_trgm` 或可重建 search projection；它始终是派生索引，Node row仍是summary唯一事实源。

### 8.4 事务与 parity

attach / detach继续使用当前 Community exclusive advisory lock和Edge/source locks，并在同一SQL事务中
更新派生Node membership。summary update只锁：

```text
Community Project Context write lane
→ node_state
→ target Node
→ source owner current rows
```

避免把现有“每个受影响 row 都运行一次全 Community validator”的模式复制到 Node 表。实现采用：

- Node / Edge / Binding row trigger只校验本行与局部transition；
- attach / detach最终只由catalog/state row触发一次完整Edge+Node Community parity，或coordinator在
  commit前显式调用一次；
- summary-only不改变Edge union / degree，只验证target Node、Node change/items、Meta、catalog count
  delta和projection pointer；不为每次摘要改写全扫整图；
- admin integrity / startup preflight仍执行完整Community audit，migration / reprojection cutover也必须
  完整audit；
- 一次大Hyperedge attach / detach不得退化成 `O(changed_nodes × whole_graph)` 校验。

完整 parity证明：active union、degree、counts、Node Heads、Node Meta digest、Binding events和source
provenance全部一致。

## 9. 写事务

### 9.1 attach / detach

事务顺序：

1. 验证签名、Community、schema、Assignment / Runtime fence与command replay。
2. 锁Edge state、目标Edge / Binding、Node state、受影响Node与来源current heads。
3. 重算 exact Edge、entering / leaving set和所有source basis。
4. pure reducer生成Binding transition、Node transitions、两个catalog的新counts。
5. 在写canonical rows前构造并验证全部Relay-signed events；Node Heads先签，Node Meta后签。
6. 写command、Binding、Edge、Node、projection pointers、既有Edge change，以及需要时的Node change /
   items / receipt。
7. CAS推进Edge context revision；有Node Head变化时推进一次Node catalog revision。
8. 执行一次完整parity，commit后fan-out。

任一签名、basis、pointer、revision或parity失败均整单回滚。

### 9.2 summary update

1. 解析v3 command并验证作者现有Project Context write权限。
2. 锁Node state、目标active Node与来源current rows。
3. 检查node revision、source observation和NoChange。
4. 签一个Node Head，再签一个Node Meta。
5. 写command / Node change / item / receipt、更新Node和node_state。
6. 不锁或改写无关Edge / Binding；commit前验证目标Node仍active、Node Meta / ledger与catalog count
   delta，不执行全Edge union scan。

### 9.3 projection generation

Node protocol有独立`projection_generation`，但必须使用当前stable Relay signer。Relay signer rotation时：

- Node capability未ready时，Edge v2沿用现有reprojection流程；
- Node capability ready后，signer rotation必须作为Edge+Node联合reprojection run；两套catalog的
  candidate signer和target generation一致；
- Node Heads按canonical key分页签入专用staging table，header同时pin Edge context revision、Node
  catalog revision、两套完整row digest和candidate signer；
- staging bytes不进入普通`events`，不能由WS、HTTP、COUNT、by-id或search提前读取；
- 普通写入使任一pinned revision / digest变化时finalize冲突并重做受影响staging；有限重试后进入短暂
  joint write-maintenance；
- 全量验证后在同一个Community maintenance cutover中切换Edge与Node signer / generation / pointers /
  Meta，NIP-11只在两者都ready后广告新signer；
- 失败时两套旧generation与旧signer一起继续可读，不能先切Edge导致旧40910被current-author gate拒绝。

不得声称只切一个Meta就完成所有current pointer切换。

## 10. 渐进读取协议

### 10.1 读取原则

结构读取分五种scope：

```text
edges               按完整Hyperedge分页
edge_bindings       按一条Edge的Context Documents分页
nodes               point / batch / active / missing Node读取
coordinate_search   title + node summary的lexical candidate发现
node_meta           读取唯一current Node Catalog observation
```

这些semantic keyset scopes复用HTTP `POST /query`的raw two-pass bridge，不新增业务专用endpoint。
当前WS `ClientMessage::Req`会直接反序列化标准`nostr::Filter`并丢弃unknown extension，所以首版不宣称
WS支持`buzz_project_context` cursor语义。WS / COUNT只支持标准raw projection读取并执行同样的kind级
权限gate；所有HTTP custom scopes先经过Community-private gate，再在DB shared lock内重新授权。

### 10.2 closed filter

Edge page：

```json
{
  "kinds": [40908],
  "authors": ["<relay-pubkey>"],
  "limit": 21,
  "buzz_project_context": {
    "schema_version": 1,
    "scope": "edges",
    "project_id": "<uuid>",
    "projection_generation": 3,
    "query": {
      "type": "incident",
      "coordinate": { "...": "..." }
    },
    "cursor": null
  }
}
```

其他closed shape：

```text
scope=edges
  query = exact{coordinates}
        | incident{coordinate}
        | contains_all{coordinates}

scope=edge_bindings
  edge_key
  cursor(after_context_document_id)

scope=nodes
  query = exact{coordinates[]}
        | active
        | missing
  cursor(after_canonical_coordinate_key)

scope=coordinate_search
  query_text
  coordinate_types[]?
  cursor(after_canonical_coordinate_key)

scope=node_meta
  无query、无cursor，只返回当前40911 Node Meta
```

约束：

- outer filter只允许`kinds`、`authors`、`limit`、`buzz_project_context`；
- 一次只允许一个kind、一个Relay author和一个extension scope；
- extension中的`project_id`必须等于host-derived Community / Project，不能由调用方切换租户；
- 禁止同时出现`ids`、`search`、普通tags、OFFSET page或多个filters；
- exact coordinates canonical、去重且非空；contains-all至少一个Coordinate；
- unknown field、错误kind、错误scope、错误cursor全部fail closed；
- user page limit最大100；CLI wire请求`user_limit + 1`，移除额外项后派生`has_more`和cursor；
- `/query`仍只返回event array，不伪造不存在的响应envelope。

### 10.3 Cursor

```text
ProjectContextCursorV1
├── schema_version = 1
├── project_id
├── scope
├── projection_generation
├── started_edge_context_revision
├── started_node_catalog_revision
├── query_digest
└── after_key
```

编码为base64url canonical JSON，最大4096 bytes。`query_digest`：

```text
SHA-256(
  "buzz-project-context-query-v1\0"
  || canonical_json(scope + query + type filters)
)
```

page size不进入digest，允许续页时调小limit。`after_key`按scope closed：

- Edge：32-byte EdgeKey hex；
- Binding：canonical lowercase Document UUID；
- Node / search：canonical Coordinate tag value。

Cursor不是授权token。Relay每页重新验证Community、Project、kind、scope、query digest和generation。

### 10.4 一致性策略：有界live traversal

首版不把全局`context_revision`伪装成可读历史snapshot。当前投影只保留current Heads；在持续运行的
空间中，任何无关写入都让严格revision cursor重启，会造成检索饥饿。

因此分页采用generation-pinned、revision-observed的弱一致keyset：

- generation变化：409 conflict，必须重启；
- revision变化：允许继续，CLI在coverage中记录start/end revision与`changed_during_scan=true`；
- 每项保留自己的Binding / Node / source revision；
- keyset保证已经越过的key不重复，但并发插入到cursor之前的项可能本轮看不到；
- 删除、摘要修改、title变化同样作为coverage边界，不伪装成完备snapshot；
- 单页水合前后变化最多重试2次，仍不稳定则返回`source_churn` / `graph_churn`，不无限循环。

这符合渐进检索“有界且诚实”的目标。若未来需要严格全图审计，必须另建可读历史snapshot，而不是
把当前Head query强称为snapshot。

### 10.5 按Edge分页

DB先按canonical Edge rows执行keyset：

```text
exact          derived edge_key + active
incident       anchor canonical_key + edge_key > after
contains_all   GROUP BY edge_key HAVING matched_count = required_count
```

选出一页EdgeKey后，对每个key选择`context_document_id`最小的active Binding pointer并返回对应40908
event。SDK必须再次验证：

- event的EdgeKey与Project /完整coordinates一致；
- event是该Edge的active Binding；
- event signer / generation / source合法。

代表Binding中的`context_document_id`只证明至少一份active关系Document，不表示membership完整。

`edge_bindings`按`context_document_id > after`返回该Edge的40908 Heads。CLI输出：

```text
returned_document_count
documents_complete
next_document_cursor
```

`documents_complete`由`limit + 1`结果派生，不要求40908额外携带总数。任何partial page不得聚合成
“完整ProjectContextEdge”；代表Binding自身的Document ID也必须标成carrier membership，而不是完整
Document集合。

### 10.6 Node读取

40910 Node query支持：

- 一个Coordinate point get，含deleted diagnostic；
- 最多100个Coordinate batch get；
- active list；
- active `MissingLegacy` list；
- 为一个大Hyperedge分批水合Node Previews。

active list、search、Frontier和traversal必须排除deleted Head。batch hydration永远保留原Edge的完整
Coordinate identity set，并用coverage报告哪些Node/source未水合；不能静默删除成员后输出缩小的
Hyperedge。

## 11. title + summary 候选搜索

### 11.1 语义

`coordinate_search`只返回“可能值得查看”的少量Node候选：

```text
problem text → lexical candidate Coordinate
```

这是候选发现，不是Edge。它不接收Role、Work或Runtime Context，也不输出相关度结论。Agent结合
自己的语境选择query词并判断结果。

### 11.2 首版匹配规则

输入：

- trim后非空，最大1024 UTF-8 bytes；
- 按Unicode whitespace切分，去空、保留首次出现顺序并去重；
- 最多16 terms；连续中文短语可作为一个term；
- `%`、`_`与escape字符按literal转义。

候选集合只含active Node。每个term对当前来源title和node summary做case-insensitive literal substring
match，terms之间采用OR，以召回为优先；`MissingLegacy`只匹配title。结果按canonical key排序，不把
不稳定rank写入cursor。

每次search输出当前Node Meta的`missing_summary_count`，并把semantic routing coverage标成：

```text
complete_for_current_node_summaries
legacy_summaries_missing { count }
```

第二种状态下，空搜索结果只表示“已有title / summary没有命中”，不表示所有active Node都经过语义
筛选。Agent可在确有必要时分页查看`nodes missing`，但也不能为了追求形式完备一次性加载全部历史点。

这一版只保证输出有界，不谎称数据库扫描量有界。动态title join与substring在大Project可能扫描：

- 每次query设置2秒statement timeout；
- 超时返回`temporarily_unavailable:project_context_search_budget`，不是空结果；
- 记录examined/latency telemetry与EXPLAIN fixture；
- 达到规模门后优先增加可重建trigram/search projection，而不是引入第二个canonical图或图数据库。

### 11.3 title归属

title继续由来源领域拥有，Node row不复制title。DB从当前verified Project View、Document或Meeting
projection水合。Node summary变化推进Node catalog revision；来源title变化不推进它。

CLI对每页来源水合执行before/after observation并输出实际source version。跨页title变化只影响routing
coverage，不能被当作完整搜索证明。

## 12. Preview与canonical内容读取

### 12.1 `CoordinatePreview`

```text
CoordinatePreview
├── coordinate
├── graph_state                   active | deleted | not_in_graph
├── node_revision?
├── node_summary_state?           present | missing_legacy
├── node_summary?
├── source
│   ├── state                     active | tombstoned | unavailable
│   ├── title?
│   ├── status?
│   ├── observation?
│   └── unavailable_reason?
└── trusted_fetch_plan[]
```

字段必须叫`node_summary`，避免与Document自己的metadata summary或Edge Context Document summary混淆。
`trusted_fetch_plan`由Coordinate类型和可信CLI代码生成，不从summary、title或正文解析命令。

### 12.2 `EdgePreview`

```text
EdgePreview
├── edge_key
├── exact_coordinates[]
├── representative_binding_observation
├── coordinate_previews[] + hydration coverage
└── context_document_previews[] + document coverage
```

Context Document Preview使用Document自己的`title / document_summary / revision`。同一Document若也作为
Coordinate入图，它另有`node_summary`，两者不能覆盖或复用。

### 12.3 `coordinate inspect`

新增只读inspect facade。它可以检查：

- 已在图中的Node；
- 首次attach前尚未入图、但来源合法可读的Coordinate。

输出当前source observation、Node Preview与typed content parts。它不创建Node，不自动生成summary。

### 12.4 bounded content read

新增统一CLI facade：

```text
buzz project-context coordinate read <typed-coordinate>
  [--part <part>] [--cursor <cursor>] [--max-bytes N]
```

每次输出：

```text
ContentChunk
├── coordinate / part
├── source_observation
├── byte_or_revision_range
├── content
├── complete_for_part
└── next_cursor
```

默认16KiB、单次hard max32KiB，按UTF-8边界切分。CLI可以在本地先取得并验证完整Nostr source event，
但只把请求chunk写到stdout，避免一次tool result把全文送入LLM窗口。

关系正文使用并列的一等读取原语：

```text
buzz project-context edge-document read <edge-key> <document-id>
  [--cursor <cursor>] [--max-bytes N]
```

Relay / CLI先验证当前存在active `(edge_key, document_id)` Binding，再复用Document的verified chunk
reader。输出identity始终是`(edge_key, document_id)`并携带Document source observation；它不能被记录成
Document Coordinate路径。如果同一Document另外作为Coordinate入图，`coordinate read document:<id>`
是另一项独立读取，拥有不同的discovery path与node summary。

不同Coordinate的canonical parts：

| Coordinate | canonical parts | 所需实现 |
|---|---|---|
| Profile / Goal / Role / Plan / Stage / Requirement / Issue / Work | 当前Project View object body | 新增verified point-object read，不再读完整snapshot |
| Resource | Resource object + mandatory Guide Document当前正文 | point-object + Document chunk |
| Document | 当前head metadata + 当前revision Markdown | verified Document read + UTF-8 chunk |
| Meeting Baton | metadata + current state + current Board + formal Speeches through speech revision | state observation + revision-keyset Speech pages |
| Meeting UniformV0 | metadata + State / End + Board +完整formal Speech event set | event-set digest + round/event keyset pages |

Project View point read沿用HTTP `/query`的`buzz_project_view` bridge，但新增两个exact scope，不能省略
outer `#t`：

```json
{
  "kinds": [40903],
  "authors": ["<relay-pubkey>"],
  "#t": ["buzz-project-view-v3-object"],
  "limit": 1,
  "buzz_project_view": {
    "scope": "v3_object_point",
    "projection_generation": 3,
    "object_type": "work",
    "object_id": "<uuid>"
  }
}
```

普通active对象和Role tombstone走`v3_object_point`并使用
`parse_project_object_projection`；active Role走：

```text
kind=40903
#t=["buzz-project-view-v3-entity"]
scope=v3_role_point
projection_generation / role_id
```

并使用`parse_entity_projection`后要求exact `RoleDefinitionV3`。Role inspect先查active entity，未命中再
查ordinary Role tombstone；summary basis只接受active Role entity。两个scope都在DB shared lock内按
current pointer读取，前后校验Project View Meta，不要求全局project revision仍完全相同；currentness
由generation、head event、object revision和body digest证明。Bridge的allowed outer fields、exact tag、
DB pointer branch与两种parser必须有独立测试。不得继续为了一个Role或Work读取整个snapshot。

Meeting formal history新增HTTP `/query`扩展，不能继续复用当前CLI `query_paginated`后整体排序的路径：

```json
{
  "kinds": [9],
  "#h": ["<meeting-uuid>"],
  "limit": 21,
  "buzz_meeting_history": {
    "schema_version": 1,
    "scope": "formal_speeches",
    "protocol": "revisioned",
    "through_speech_revision": 84,
    "cursor": { "after_speech_revision": 20, "after_event_id": "<hex>" }
  }
}
```

UniformV0使用：

```text
protocol=uniform_v0
through={formal_speech_count, formal_speech_ids_digest}
cursor={after_round, after_event_id}
```

outer filter只允许exact kind 9、一个`#h`、limit和extension。HTTP custom branch在任何early return前必须
调用现有Meeting read-scope授权，DB从verified formal Speech记录而不是channel内所有kind 9消息选择，
按`(speech_revision,event_id)`或`(round,event_id)`keyset返回raw events。`/query`仍只有event array；CLI从
verified tags派生actual range，并由limit+1派生`has_more`，最终核对Baton speech revision或Uniform
event-set digest。

protocol resolver逐项处理`UniformV0`、`ModeratedBatonV1`、`ModeratedBoardV2`、
`ModeratedBoardActionsV2Legacy`和`ModeratedBoardActionsV2`。后四者使用revisioned history，但State / End
evidence从DB canonical protocol row解析，不能复用当前CLI中不接受所有legacy shape的
`parse_baton_state` helper。生成摘要前必须把所有canonical parts读到`complete=true`且最终observation
与command basis一致；partial read不能生成或更新摘要。

### 12.5 Runtime已有内容

渐进检索形成事实判断时，如果当前Runtime已经持有同一canonical identity、当前source observation且
内容完整，可以不重复读取。Agent必须能指出provenance；模糊回忆、旧压缩摘要或只含title / node
summary不算完整内容。

对于摘要写入，Relay仍验证command basis当前；Runtime已有内容不能绕过来源CAS。

## 13. Agent-first CLI

### 13.1 命令面

```text
buzz project-context discover
  --query <text> [--type <coordinate-type>] [--limit N] [--cursor C]

buzz project-context coordinate inspect <typed-coordinate>
buzz project-context coordinate read <typed-coordinate>
  [--part P] [--cursor C] [--max-bytes N]

buzz project-context nodes get <typed-coordinate> [--include-deleted]
buzz project-context nodes batch --coordinate ... [--limit N] [--cursor C]
buzz project-context nodes missing [--limit N] [--cursor C]
buzz project-context nodes meta

buzz project-context exact --coordinate ...
buzz project-context incident <typed-coordinate> [--limit N] [--cursor C]
buzz project-context contains-all --coordinate ... [--limit N] [--cursor C]
buzz project-context edge-bindings <edge-key> [--limit N] [--cursor C]
buzz project-context edge-document read <edge-key> <document-id>
  [--cursor C] [--max-bytes N]

buzz project-context attach
  --context-document <uuid>
  --coordinate ...
  [--entering-node-manifest <file-or->]

buzz project-context update-summary <typed-coordinate>
  --expected-node-revision N
  --summary-manifest <file-or->

buzz project-context detach ...
```

Manifest采用closed JSON，避免多语言summary和复合basis通过shell `key=value`传输。CLI可以先做明显的
canonical / shape检查，最终entering set和basis仍由Relay在锁内重算。

### 13.2 分页默认值

```text
Node / Edge preview       default 20, hard max 100
Context Document preview default 5,  hard max 50
Content chunk             default 16KiB, hard max 32KiB
Search query              hard max 1024 bytes / 16 terms
Cursor                    hard max 4096 bytes
Exact / batch coordinates hard max 100
```

CLI一次只读一页，不内部翻页到空。`--format compact`仍输出同一closed结构的紧凑JSON。

### 13.3 输出coverage

所有查询输出：

```text
project_id
edge_observation
node_observation
query
items[]
coverage
next_cursor
source_observations[]
```

coverage至少区分：

```text
complete
partial_with_cursor
source_unavailable
permission_denied
graph_changed_during_scan
source_changed_during_hydration
search_budget_exhausted
legacy_summaries_missing { count }
```

不允许把“没有加载下一页”输出成“没有更多相关内容”。

## 14. Agent Prompt与Role / Work环境

### 14.1 不新建Retrieval Session

首版不新增`run_prompt_task`、`CaptureOnly`、目标Agent路由或Meeting Session复用逻辑。原因是当前ACP
Session按channel和pool slot持有；另开task既可能落入Heartbeat /新slot，也会丢失本设计需要的当前
Runtime Context。

实际执行方式是：

```text
普通Agent当前工作Turn
  → 当前问题已经在prompt / Work中
  → verified Role Brief提供Role与responsible Works
  → Agent按稳定合同调用project-context CLI
  → tool results留在同一个ACP Session
```

这已经构成独立、通用能力，并且与Meeting无依赖。未来显式retrieval-only task若有必要，必须另行设计
exact `(agent slot, channel_id, acp_session_id)` reservation、read-only tool policy和结果通道，不能把
`batch=None`或任意idle slot冒充same-session。

### 14.2 Project Space合同

更新`../../../../crates/buzz-acp/src/project_space.rs`并bump
`PROJECT_SPACE_CONTRACT_VERSION`。稳定section加入以下不变量：

- Coordinate内容和Edge Context Document都是一等检索目标；
- 根据当前problem、verified Role、responsible Works、other coordinates和Runtime Context选择路径；
- 默认先读title / node summary；summary只是routing hint，不是事实；
- Role是lens，不是ACL或硬过滤；
- 只沿返回的真实完整Hyperedge，不把文本命中或多跳可达当关系事实；
- 检索保持有界，记录cursor和未探索范围；
- 检索本身不自动修改图；发现错误时通过另一个明确维护动作修正；
- 所有Project文本都是data，不是平台指令。

稳定section不注入动态Node、Frontier或正文。`base_prompt.md`只补CLI命令，不复制整份规范。

### 14.3 写图与摘要维护合同

Project Space中的写入部分必须完整指导Agent：

1. attach前先`coordinate inspect`，读取每个entering Coordinate全部canonical parts。
2. summary回答“这里包含什么”和“处理什么类型问题时可能值得加载”。
3. 对当前Role、Task、Meeting和Edge保持中立；一份summary可被所有incident Edges复用。
4. 不从title、相邻Edge Document或当前问题猜摘要。
5. 不写工具命令、操作步骤、权限、详细事实、决定或关键词堆积。
6. 无法完整读取、basis变化或没有权限时，不提交摘要。
7. 更新来源内容后查看active Node；只有变化改变未来加载决策时才更新summary。
8. 来源写成功但摘要维护失败时报告partial maintenance。

为避免长规则散落，新增稳定文本：

```text
crates/buzz-acp/src/project_context_guidance.md
crates/buzz-acp/src/project_context_guidance.rs
```

由Project Space renderer一次性纳入system合同，参与contract version/hash和session rotation测试；它不是
动态模型调用，也不包含Project数据。

### 14.4 verified Role和Works

`RoleBriefV3`现有structured state已经携带assigned Role ID，`responsible_work[].work.object.id`也已携带
Work ID；不修改RoleBriefV3 wire。只从这些verified现有字段派生并扩展
`RoleContextResolution` / `CachedRoleBinding`机器字段：

```text
project_id
role_id?
assignment_id?
responsible_work_ids[] + work_coverage
project_revision / projection_generation / meta_event_id
```

Full Role Brief增加：

```text
Role coordinate: role:<uuid>
Responsible Work: <title> (work:<uuid>)
```

Compact Binding保留同样typed IDs。candidate / unavailable Role不能从Persona、显示名或Markdown猜ID。
若当前Turn没有verified current Role，本次过程不得被标成符合本规范的渐进检索；Agent应停止并报告
`agent_environment_unavailable`。它仍可以执行普通Project读取来完成其他任务，但不能静默降级为
problem-only retrieval并声称使用了Role视角。

Responsible Work沿现有`object_order`稳定排序，Full / Compact最多直接展示32项，并输出：

```text
work_coverage { total, shown, omitted }
```

机器resolution同样只把shown IDs激活进本Turn上下文；omitted Work通过现有Project View关系或新增的
`v3_responsible_work`有界page按需读取：outer filter固定40903、Relay author、
`#t=["buzz-project-view-v3-object"]`，extension携带projection generation、role ID、after object key与
limit，DB只返回current active Work且`responsible_role_id`精确匹配。Role是required，Work在概念输入中
本来就是optional，因此该coverage不把遗漏项伪装成“不属于当前Role”，也不让Compact Binding因大量
open Work无界增长。

检索语义输入映射：

```text
RetrievalInputView
├── problem
│   ├── description                  required, 当前任务数据
│   └── involved_coordinates[]       optional
└── agent_environment
    ├── role                         required, verified
    ├── works[]                      optional, verified + coverage
    └── other_coordinates[]          optional, canonical provenance
```

它是当前Turn中Prompt与Role Context共同形成的逻辑输入，不是一个新的持久Event或让调用方传入任意Role的
授权DTO。具体映射：

```text
problem.description
  = 当前task / prompt / Work所要求解决的问题

problem.involved_coordinates
  = 当前task明确涉及的Work / Issue / Requirement / Document / Meeting等

agent_environment.role
  = verified current Role

agent_environment.works
  = Role Brief / Assignment verified responsible_work_ids

agent_environment.other_coordinates
  = Runtime已知且带canonical identity的其他坐标
```

调用方给出的另一个Work只能是problem involved或other hint，不能冒充Agent当前responsible Work。
Role / Work即使未入图也能作为语义视角；只有active Node Coordinate才能作为结构seed。

### 14.5 Agent渐进循环

稳定Prompt指导：

```text
1. 理解problem、Role职责边界、current Works和Runtime已有内容
2. 有显式Coordinate则从它开始；否则discover少量候选
3. 查看一页title / node summary / status
4. 选择read-coordinate、expand-coordinate、read-edge-context、defer或stop
5. 每次完整读取后重新判断未知与Frontier
6. 保留visited Node / Edge / Document，避免无向图循环
7. 在上下文足够、Frontier耗尽、来源不可用或预算到达时诚实停止
```

Prompt不规定BFS、DFS、最短路径或不同Role必须得到不同结果。
Prompt还必须检查Node Meta的`missing_summary_count`：只要非零，candidate search就存在明确的历史
semantic coverage gap，摘要未命中不能被写成“图中没有相关上下文”。

## 15. 预算、路径与运行时输出

### 15.1 两类预算

确定性工具hard caps见第13.2节。普通Turn内的整次检索没有独立orchestrator，因此聚合预算是稳定
Prompt行为合同，不冒充Harness强制计费器。建议默认：

```text
max logical steps                  24
max candidate previews            120
max coordinate content selections 12
max Edge Context Documents         8
max loaded canonical content       192KiB
```

建议上限：48 steps、300 previews、24 Coordinate contents、16 Edge Documents、512KiB canonical content。
一个logical step是一次`discover / page / read chunk / expand / defer / stop`动作；翻下一页或下一chunk各算
一步。Agent应根据剩余LLM窗口主动调低，不因有上限就必须用完。

每个ContentChunk输出`content_bytes`，Prompt累计读取量。若当前ACP/工具链以后提供可验证的per-turn
tool ledger，可以下沉为确定性总量限制；首版不修改ACP Session调度来实现它。

### 15.2 实际发现路径

Agent在当前Runtime维护轻量trace，严格区分候选命中与真实Edge：

```rust
pub enum RetrievalPathStep {
    ExplicitProblemSeed { coordinate: Coordinate },
    AgentEnvironmentSeed { kind: Role | Work | Other, coordinate: Coordinate },
    RuntimeSeed { coordinate: Coordinate, source_observation: SourceObservation },
    LexicalCandidate { query_digest: [u8; 32], coordinate: Coordinate },
    HyperedgeTraversal {
        from: Coordinate,
        edge_key: EdgeKey,
        exact_coordinates: Vec<Coordinate>,
        to: Coordinate,
    },
    EdgeContextDocument {
        edge_key: EdgeKey,
        document_id: Uuid,
    },
}
```

同一内容可保留多条真实到达路径。`A → E1 → B → E2 → C`只记录可达过程，不声称A与C有可传递关系。

### 15.3 Retrieved Context在首版中的形态

正文已经通过tool result进入当前Session，不再复制到Role Brief、Memory、Node或另一持久对象。Agent
需要在任务输出中说明检索依据时，只输出：

- 完整读取并实际采用的Coordinate identities；
- 实际采用的`(edge_key, context_document_id)`；
- typed source observations；
-真实发现路径；
- stop reason、未探索cursor与不可用范围。

partial content不进入“已选择、可依赖”的集合，只进入`partial_reads / coverage`并携带chunk range和
continuation。summary本身也不能作为selected事实来源。

首版不新增持久`RetrievedContextManifest`，也不承诺另一Session能按所有来源精确重放历史。若Agent
需要交接，只能提供provenance与expected observation；消费者读取时校验版本并报告`source_drift`。
严格跨Session装载由后续orchestration设计负责。

## 16. 权限与安全

### 16.1 raw projection gate

40910 / 40911必须加入所有Community-private chokepoints：

- WS REQ / COUNT；
- HTTP `/query` / `/count`；
- by-id；
- private filter与subscription；
- Relay-only insertion classifier；
- fan-out；
- kind registry / d-tag assertions与conformance tests。

不能把新kinds直接塞进当前单一`ProjectContextReadDecision / context_read_allowed`。实现拆成：

```text
edge_read_decision / edge_read_allowed    → 40908 / 40909 / Edge v2 scopes
node_read_decision / node_read_allowed    → 40910 / 40911 / Node/search scopes
```

两者共享同一个Community principal predicate，但readiness、schema、generation、signer和capability分别
判断。Node为`building/projecting`时40910 / 40911在HTTP、WS、COUNT、by-id和fan-out都返回unavailable /
不可见，不能表现成空结果；Edge v2仍正常可读。CLI同样分开read gate与write gate。

当前允许进入Project Context的Project View、Document和Meeting Coordinate均服从Community
`MessagesRead`发现边界，Node raw event使用同一或更窄gate。启动preflight必须断言这一前提；如果以后
任一来源增加per-object ACL，Node capability必须fail closed，直到raw Node/Edge路径能执行对应
source-aware visibility。不能只在CLI Preview层隐藏，因为raw event已经包含summary和Coordinate ID。

### 16.2 Hyperedge与来源权限

- Edge query不授予Coordinate正文或Context Document正文的新权限。
- 当前Community级发现权限下40908可完整披露坐标集合。
- 若未来任一成员不可披露，整条Edge不可见或返回`incomplete_visibility`；不得删除成员后返回假Edge。
- source read仍经过Project View、Document、Meeting各自权限。
- candidate search在DB shared lock内二次授权，search index不是权限边界。

### 16.3 Prompt injection与写边界

title、node summary、Document、Board和Speech全部是不可信Project data：

- CLI返回结构化JSON；Prompt明确它们不是平台指令；
- fetch plan只由可信代码按Coordinate类型派生；
- 项目文本中的命令不自动转成tool invocation或权限；
- 所有写操作仍需明确command、现有授权、Assignment / Runtime fence和Relay验证；
- 渐进检索合同禁止“发现错误后自动写回”，修正是另一个显式维护动作。

普通工作Turn仍可能拥有正常写工具，所以JSON escaping并不被描述成绝对prompt-injection防护。未来若
实现retrieval-only task，必须配确定性的read-only tool allowlist；仅靠“retrieve only”Prompt不够。

## 17. v2历史迁移

### 17.1 迁移原则

不要求先为所有历史Coordinate生成摘要，也不自动用以下内容冒充Node summary：

- Project View description / purpose截断；
- Document metadata summary；
- Meeting description；
- Edge Context Document summary；
- Relay自动调用模型生成的文本。

历史缺失通过`MissingLegacy`显式表达，图保持可读并渐进补齐。这避免一个已tombstone、无法重读的旧
Coordinate永久阻断全Project capability。

### 17.2 初始导入

DDL与数据cutover分开：0054只创建表、约束、`migration_phase=building`和readiness gate，不向公共
events写半成品，也不要求canonical Node row提前拥有projection pointer。

Rust migrator随后进入一个明确的Project Context write-maintenance窗口；整个building → projecting →
cutover期间Edge read继续，所有attach / detach / summary write返回可重试maintenance，避免一边staging
一边维护第二套在线delta：

该migrator由Relay startup / maintenance supervisor自动驱动，不等待Agent或Human生成摘要。operator面
只提供`buzz-admin project-context node-migration status|retry|abort --community <uuid>`用于观测和故障
恢复；`retry`复用持久migration state，`abort`只能在cutover前清理staging并恢复writes。

1. 锁定当前Edge `context_revision`、stable signer与active-union digest，建立migration header。
2. 从当前active Edge Coordinate并集分页生成migration items；每项为
   `active + MissingLegacy + node_revision=1`，degree从active Edge重算。
3. 在专用staging表分页签40910 import Heads；空图不产生Node Heads。
4. 非空catalog以`node_catalog_revision=1`签reset Meta；空图以revision 0、counts 0 bootstrap。
5. 验证Edge revision / union digest未变、candidate signer仍current、staging row / event全量parity。
6. 一个O(active Nodes) cutover事务把staged events插入公共events、写canonical Node rows与pointers、
   Node state / Meta，设置`migration_phase=ready`并启用capability。
7. 释放write-maintenance；Edge v2 Binding、Meta、EdgeKey、context revision、command / receipt历史不改。

staging bytes不进入WS、HTTP、COUNT、by-id、search或fan-out。maintenance超时或任一步失败时，标记本次
migration failed、丢弃staging并恢复Edge writes；因为cutover前没有public Node pointer，不存在半成品
read。重试使用新migration ID和新的active-union snapshot。

### 17.3 兼容矩阵

| 阶段 | Edge v2 read | v2 detach | v2 attach | v3 attach/update | Node read/search |
|---|---:|---:|---:|---:|---:|
| migration前 | 是 | 是 | 是 | 否 | 否 |
| building / projecting | 是 | maintenance | maintenance | maintenance | fail closed |
| cutover后 | 是 | 是 | entering=0时是 | 是 | 是 |

CLI要把Edge structural read gate与Node capability gate拆开，不能因Node migration暂未ready而让现有v2
Edge读取全部失效。

### 17.4 渐进补齐

```text
buzz project-context nodes missing --limit 20 --cursor ...
  → authorized writer逐项inspect/read完整内容
  → UpdateCoordinateSummary(expected_node_revision=1, ...)
  → MissingLegacy变Present
```

来源tombstoned或不可读时保留missing，并如实展示；这不阻断其他Node。新attach / reentry永远不能生成
新的missing状态。

## 18. Crate与文件影响面

### 18.1 `buzz-project-context`

- 保留`coordinate.rs` identity / EdgeKey语义；
- 新增Node、Node Catalog、Summary、Source Observation types；
- v2 / v3 command closed parser与generalized operation accessors；
- Node projection / Meta / v3 receipt；
- Edge + Node组合reducer transition；
- summary / observation / event size validation和property tests。

### 18.2 `buzz-core` / `buzz-sdk`

- 注册40910 / 40911及所有private、Relay-only、indexed d-tag classifier；两者都验证各自canonical
  `d`，40911镜像现有40909 Meta；
- 新增Node builders / parsers / current observation verifier；
- Node Head集合digest与Meta verifier；
- v2 Binding继续作为完整Edge carrier；
- 更新`docs/nips/NIP-PCE.md`或拆出Coordinate Node extension章节与golden fixtures。

### 18.3 `buzz-db`

- migration 0054、Node/state/change ledger/items/migration staging表、索引和parity；
- source observation resolver；
- attach / detach组合事务与summary-only coordinator；
- Edge representative Binding keyset query；
- Node / Binding / searchquery scopes；
- Project View point read、Meeting speech revision pagination；
- Node reprojection、readiness和NIP-11 capability。

### 18.4 `buzz-relay`

- 44302 schema dispatch；
- 40910 / 40911全部raw read gates；
- Edge / Node独立read-decision与strict `buzz_project_context` HTTP query extension；
- private candidate search branch；
- capability advertisement；HTTP semantic query scopes与WS / COUNT raw projection gates分别验收。

### 18.5 `buzz-cli`

- discover、inspect、chunked coordinate read、Node point/batch/missing；
- Edge一页查询、edge-bindings分页；
- `(edge_key, document_id)`关系正文的bounded read；
- entering summary manifest与update-summary；
- typed Preview、coverage、source observation和cursor；
- 去掉当前“读到空页再全量hydrate”的路径。

### 18.6 `buzz-acp` / Role Brief

- Project Space contract bump与`project_context_guidance`；
- Base Prompt命令面；
- Role / responsible Work typed IDs；
- stable contract / role renderer /恶意Project文本边界测试；
- 不新增Meeting或Retrieval Session调度。

## 19. 实施顺序

### Phase 0：协议固定

- 固定Node kind、Node v1 body / tags、v3 command和source observations；
- 固定64KiB aggregate command与16项entering summary上限；
- 固定query extension、cursor和CLI closed DTO；
- 更新NIP、kind registry与golden fixtures；
- 先完成pure parser / reducer tests。

### Phase 1：Node canonical state与写入

- migration、Node/state/change ledger/items表、counts、indexes、triggers；
- attach entering summaries、detach membership、summary update；
- Node Heads / Meta和组合事务；
- 空图bootstrap与新Project行为。

### Phase 2：历史导入

- active union → MissingLegacy staging；
- O(Node) cutover、双capability gate与v2兼容；
- missing分页和显式补齐；
- tombstoned / unreadable历史测试。

### Phase 3：有界结构查询

- Edge representative Binding keyset；
- edge-bindings、nodes scopes与weak-consistency cursor；
- CLI一页输出、coverage和去重；
- 高出度 / 大Hyperedge测试。

### Phase 4：候选与canonical read

- title + node summary search；
- Project View point read；
- Document chunk、Resource Guide、Meeting speech revision pagination；
- Preview hydration与source churn语义。

### Phase 5：Agent合同

- Project Space / Base Prompt；
- typed Role / Work环境；
- summary write / maintenance与progressive loop合同；
- fake Agent tool-sequence验收，不接Meeting。

### Phase 6：性能与安全门

- EXPLAIN、statement timeout、CJK / Latin query集；
- raw projection权限撤销与跨Community碰撞；
- event frame、command size、large summary / Hyperedge边界；
- 以数据决定是否增加trigram /派生search projection。

## 20. 测试与验收矩阵

### 20.1 领域与wire

- summary trim、UTF-8字节、control、canonical JSON；
- summary变化不改变Coordinate tag / order / EdgeKey；
- source observation各variant exact shape、wrong type / old basis拒绝；
- Resource缺Guide observation拒绝，非Resource带Guide拒绝；
- Meeting Board只使用event ID，speech revision / legacy digest变化产生conflict；
- Node / Meta exact tags、wrong signer、wrongProject、unknown field、frame超限拒绝；
- changed Node digest无签名循环、排序确定、零集合确定。
- v2 / v3 schema dispatch互不宽松解析，v2 operation / receipt golden bytes保持不变；
- 16项entering与64KiB command边界产生精确错误，不允许虚假拆Edge。

### 20.2 生命周期与事务

- 新Edge：entering set与summary exact match，Node / Binding原子active；
- active Edge第二份Document：entering为空，Node不变；
- 新Edge复用active Node：一份summary；
- v2 attach entering非空拒绝，entering为空接受；
- 非最后Edge detach：仅degree变化；
- 最后Edge detach：Node deleted且保留summary author；
- reentry要求当前完整内容与fresh basis；
- MissingLegacy fill、Present update、same text NoChange；
- source update不自动stale；partial maintenance可见；
- 任一签名 / pointer / parity失败全事务回滚；
- command replay不二次推进Edge或Node revision。
- edge command同时产生Edge / Node ledgers时原子存在，summary-only只落Node ledger，半边记录触发
  integrity failure；
- unchanged Node Head允许last-changed revision小于current Meta，degree-only不改semantic Head字段；
- summary-only走局部parity，attach / detach只运行一次完整combined parity。

### 20.3 查询

- incident / contains-all使用`(community, canonical_key, edge_key)`索引；
- 一条Edge多Document时只占一个Edge page item；
- representative Binding始终携带完整Hyperedge identity；
- edge-bindings page明确partial / cursor；
- keyset无重复，revision变化记录coverage而非无限重启；
- generation变化409；单页churn两次后明确停止；
- deleted Node不参与list / search / Frontier / traversal；
- large Hyperedge Node hydration可继续且不缩边；
- CLI不再内部读到空页。

### 20.4 搜索与内容读取

- 中文连续词、Latin case、mixed terms、重复、空、超长、literal `%/_`；
- search timeout返回unavailable，不返回伪空集合；
- MissingLegacy title-only候选；
- Project View point read不读取全snapshot；
- ordinary object / active Role entity exact `#t`、parser、current pointer与digest domain分别验收；
- Document UTF-8 chunk无断码，cursor连续；
- Resource同时读取object和Guide；
- Meeting history严格读到basis speech revision / event digest；
- Meeting HTTP custom history在early return前执行Meeting read gate，WS不宣称支持extension；
- partial chunk不能生成summary或进入selected facts；
- source changed during hydration最多重试2次。

### 20.5 权限与Prompt

- 40910 / 40911在WS、HTTP、COUNT、by-id、subscription均Community-private；
- 非成员、撤权、wrong host、wrong signer、capability未ready不可观察存在性；
- kindless / mixed / ordinaryNIP-50 Project Context search继续拒绝；
- Role来自verified Role Context，Responsible Work不能由caller伪造；
- Full / Compact均保留typed Role / Work IDs；
- Responsible Work超过32项时稳定排序并输出total / shown / omitted，按需page可继续；
- summary从不作为事实引用，Role不是硬filter；
- 恶意title / summary / body不能改变system section或授予权限；
- 渐进检索不自动触发attach / update / source write。

### 20.6 Agent路径

- 有seed：Preview → incident page → read Coordinate；
- 无seed：discover → candidate → read / expand；
- 只读Coordinate、不读Edge Document的合法路径；
- 只读Edge Context Document的合法路径；
- Edge Context Document读取先验证active Binding，并保留`(edge_key, document_id)`身份；
- 文本命中与Hyperedge traversal在trace中严格区分；
- visited去重、无向循环、高出度、defer后重新判断；
- budget到达时返回cursor和coverage，不声称不存在；
- 两个Role走出相同路径仍被接受，不强制制造差异。

### 20.7 迁移

- 空图revision 0 bootstrap；
- 非空active union精确导入MissingLegacy，Edge v2完全不变；
- staging中断时Edge v2继续可读；
- building / projecting期间所有Project Context writes明确maintenance，Node raw gate返回unavailable而非空；
- cutover前Node raw kind fail closed；
- tombstoned / unreadable历史不阻断其他Node capability；
- 新attach永不创建MissingLegacy；
- v2 read / detach兼容矩阵逐项验收；
- Node ready后的signer rotation只能Edge+Node联合cutover，任一失败保留两套旧signer / generation。

## 21. 明确非目标

本实现不包括：

- 独立图数据库、第二份canonical graph、embedding、vector或ontology；
- typed / directed Edge、权重、最短语义路径或Role预计算排名；
- Role Context、Meeting Context或Agent私有持久Context；
- 按Role复制、切分或隔离统一Project Context；
- 自动摘要、强制摘要同步或Revision驱动的自动stale；
- 把summary命中 /未命中作为事实证据；
- 每Turn注入全图或全部候选正文；
- 持久Retrieved Context、跨Session精确重放或Meeting入场装载；
- 为不同Agent强制分流或减少结果重合；
- 让图或数据库替Agent判断最终语义相关性。

## 22. 结论

实现的核心不是新建“语义图引擎”，而是在现有Project Context结构上补齐两个能力：

1. 每个图内Coordinate统一提供一份由写入者基于当前完整内容显式生成、乐观维护的Node summary；
2. 图向Agent提供真实、轻量、有界、可继续的结构与内容读取原语。

Edge v2继续拥有结构事实，40908本身已经足以承载完整Hyperedge identity；新增Node v1只承担节点检索
描述和membership read model。Agent在自己的现有工作Session里，用当前问题、verified Role、
responsible Works和Runtime Context决定查询词、读取内容与下一跳。

这样，所有Agent仍面对同一Project Context Graph，却能够因为真实工作环境不同而走出不同的语义
检索路径；如果路径相同，也只是共同事实导致的自然结果，而不是系统人为拆分或制造差异。
