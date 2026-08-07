# Meeting 作为 Project Context 坐标与 Community 可见性实现设计

> 状态：产品与技术边界已确认，待实现
>
> 日期：2026-08-07
>
> 范围：Meeting 读取权限、Project Context Coordinate、NIP-PCE、Relay/DB、CLI/ACP、Desktop 与非破坏迁移
>
> 关联文档：
> [Project Context 领域规范](./project-context.md)、
> [Project Context Edge 后端实现设计](./implementation-design.md)、
> [Project Context Desktop 规格](./desktop-spec.md)、
> [Meeting Desktop 产品规格](../meeting/desktop/meeting-desktop-spec.md)

## 1. 文档目的

本文把已经确认的两个产品决策映射为可实施方案：

1. Meeting 与 Project View、Project Document 一样，是 Community / Project 范围的治理资产；
2. Project Context 增加稳定的 `MeetingCoordinate { meeting_id }`，使 Meeting 可以和 Project
   View 对象、Project Document 一起组成准确的 Context Edge。

这里的“Community 范围”只改变 Meeting 的读取边界，不改变会议行动边界。所有当前有效的
Community member 都可以发现并读取会议；谁可以申请发言、提交 Speech、主持、维护 Board 或执行
Action Finalization，仍由 Meeting 创建时冻结的 participant roster 和现有协议状态决定。

本次不是在 Project Context 查询中叠加逐 Meeting ACL，而是主动统一三个项目资产领域的读取模型，
从根源上移除原 TODO 中的权限交集阻塞。

## 2. 最终领域模型

### 2.1 Meeting 是项目级会议记录

Meeting 的正式记录包括：

- 稳定 `meeting_id`；
- 标题、讨论目标、议程和创建时间；
- 冻结 participant roster、participant type 与主持身份；
- 当前或最终 Board；
- Relay 接受的正式 Speech；
- Intent、Offer、Grant、Handoff 和 Floor 生命周期记录；
- Action Finalization 状态与终态；
- 可选 source Channel 定位。

这些内容属于当前 Community / Project。Meeting 不再被建模为“只有 roster 能知道其存在的私有房间”。
frozen roster 只表示正式参会与行动资格。

### 2.2 Project Context 仍由 Edge 与 Document 分工

扩展后的模型是：

```text
ProjectContextEdge
├── project
├── coordinates: Set<CoordinateRef>       2..*
└── context_documents: Set<document_id>   1..*

CoordinateRef =
  ProjectViewObjectCoordinate {
    object_type,
    object_id
  }
  | ProjectDocumentCoordinate {
      document_id
    }
  | MeetingCoordinate {
      meeting_id
    }
```

Meeting 是来源证据和治理记录坐标，不承担 Edge 的解释正文。为什么某场会议与某个 Goal、Requirement、
Resource 或 Document 共同相关，仍由 Edge 关联的普通 Project Document 解释。

### 2.3 不自动推断 Context Edge

以下行为都不会自动创建或扩大 Context Edge：

- Meeting 结束；
- 主持人更新最终 Board；
- Action Finalization 写入 Project View；
- Meeting 物化出 Goal、Plan、Stage、Requirement、Issue、Work、Role、Resource 或 Document；
- Agent 在 Speech 中提到某个对象。

Human 或 Agent 必须显式选择准确坐标集合和 Context Document，再执行现有 `attach`。系统不从会议内容
推断语义关系。

## 3. 权限模型

### 3.1 读取与行动必须分离

权威权限矩阵固定为：

| 身份 | 发现/读取 | 请求发言/提交 Speech | 主持与 Board 操作 | Action Finalization | 管理性终止 |
|---|---:|---:|---:|---:|---:|
| Community owner，且不在 roster | 是 | 否 | 否 | 否 | 按现有 owner 应急规则 |
| Community admin，且不在 roster | 是 | 否 | 否 | 否 | 按现有 admin 应急规则 |
| Community member，且不在 roster | 是 | 否 | 否 | 否 | 否 |
| roster participant | 是 | 按现有 Floor 协议 | 仅主持身份 | 仅现有授权身份 | 仅同时为 owner/admin 时 |
| Community 外身份 | 否 | 否 | 否 | 否 | 否 |
| 已移除、停用或封禁的旧成员 | 否 | 否 | 否 | 否 | 否 |

Human 与 Agent 使用同一读取规则。Role、Assignment、managed-by、Runtime supervisor、ACP slot 和当前
Meeting roster 都不额外授予 Community 外身份读取权。

现有 owner/admin 的安全性 Meeting End 是 Community 治理应急能力，不属于主持、Floor 或普通会议参与。
本次保持该能力，不把它扩大给普通 member，也不允许 owner/admin 借此提交 Speech、维护 Board 或确认
Action output。

### 3.2 Community membership 是唯一读取权威

Meeting 的所有读取入口必须复用当前、未缓存过期的 Community membership / principal security
判定：

- 新加入 Community 的成员可以查看历史 Meeting；
- 从 Community 移除或封禁后，立即失去活动与历史 Meeting 读取权；
- 重新加入后，按当前 Community membership 恢复读取，不再使用每场 Meeting 的 durable reader
  revocation 作为历史读取门；
- 未授权响应不得泄漏 Meeting 是否存在、标题、ID、roster、计数或关联 Context Edge。

这与 Project View、Project Document 和 Project Context 的 Community 边界保持一致。

“与 Project View 同级”指 principal membership 语义相同，不表示绕过 credential scope。Community-wide
Meeting 读取要求现有 Community-global read credential；channel-scoped token 不能因为知道 Meeting ID
就升级成 global credential。若未来需要显式 Meeting-scoped token，应独立设计，不进入本次迁移。

### 3.3 frozen roster 继续保持不可变

实现不得通过以下方式获得 Community-wide 读取：

- 把所有当前 Community member 插入 `meeting_participants`；
- 把所有成员加入 Meeting 的 `channel_members`；
- 在新成员加入时修改历史 Meeting roster；
- 把 observer 伪装成 Human participant；
- 放宽 Speech、Intent、Grant、Handoff、Board 或 Action command 的现有 actor 校验。

`meeting_participants` 继续是协议参与者与 participant type 的权威集合。读取授权改为 Community
membership 后，会议状态机、终止原因、participant revocation 和主持控制权不随之改变。

### 3.4 可见不等于需要通知

Community-wide 读取不能使每场会议打扰所有成员：

- 所有成员可以从 Meetings 历史或搜索中发现会议；
- roster participant 继续收到现有 Offer、Grant、Floor、Action 和未读提醒；
- 非 roster observer 默认不产生 Meeting 未读、attention 或 Agent 工作提示；
- 以后如需 Follow / Watch，应作为独立偏好，不进入本次权限设计。

## 4. source Channel 与内容发布边界

### 4.1 新 Meeting 只允许 Community-readable source

当前 Meeting Create 允许可选 `source_channel_id`，并以“全部 frozen roster 是否可读”校验 private
source。改为 Community-wide Meeting 后，该规则不足以保护 Community 外或更窄范围来源。

新规则固定为：

- 无 source 始终合法；
- Community-wide 普通 Channel 可以作为 source；
- private Channel、DM、仅部分成员可读的 source 不得绑定到新 Meeting；
- Relay 是最终校验者，Desktop 和 CLI 只做提前提示；
- source deep link 不授予目标 Channel 权限。

判断依据必须是 source 的权威读取范围，而不是枚举当前成员后逐个试读。否则未来新成员加入时仍可能
读到一个来源于更窄范围的会议记录。

Relay 必须在 Meeting Create 事务内解析并锁定 source，验证：

- source 属于同一 host-derived Community；
- Channel 存在且未删除；
- `room_kind=standard`，不是 DM、Meeting 或其他窄域 room；
- 权威 visibility / read policy 对当前和未来 Community member 都可读；当前模型中等价于
  `visibility='open'`；
- 提交 Meeting 前再次验证，避免检查与写入之间的 TOCTOU。

source 在 Meeting 创建后被归档不影响既有会议；若以后从 open 改成 private，Meeting 仍是已经完成的
Community 发布，不能倒退为 roster-private。source deep link 按目标的新权限失败，但全局 Meeting
capability 不因此掉线。

### 4.2 Meeting 正文是明确的 Community 发布

Meeting 创建界面和 Agent contract 必须明确提示：

> 会议标题、Board、正式发言和最终记录对当前及未来 Community 成员可见；只有 roster 成员可以参与。

Human 或 Agent 不应把密码、私钥、访问令牌或只允许更窄受众读取的内容写入 Meeting。Board 或 Speech
中的普通链接继续执行目标资源自身权限，不通过 Meeting 获得访问权。

### 4.3 全部存量 Meeting 的可见性切换审计

所有旧 Meeting 都是在“只有 roster 可读”的旧合同下写入。即使没有 source，Board 或 Speech 也可能
包含原本面向窄受众的信息。因此 private source 不能成为唯一审计集合。

权限切换需要短暂 maintenance / drain：

1. 暂停新 Meeting Create；
2. 等待所有 active Meeting 正常结束；owner/admin 应急终止只能按现有治理规则使用，不能为迁移伪造
   正常结论；
3. 在所有旧 Meeting 终态且不可再写后，取得 Community event/security high-water mark；
4. 对 watermark 之前的完整 Meeting corpus 生成稳定 digest，至少绑定 Meeting ID、Create event、最终
   State revision、End event、最终 Board、Speech 集合摘要和 Action terminal head；
5. 附加 source 风险报告，将 source 分类为 Community-wide、private、missing；
6. operator 对“全部 legacy Meeting 扩大为 Community 可见”做持久、可审计的显式确认；
7. 在同一锁定窗口验证 digest 和 watermark 未变化，再原子启用新读取合同与新 Create 合同。

新合同启用后的 Meeting 从创建时就明确 Community-visible，不进入 legacy digest。历史 Create event 不得
被数据库改写。private/missing source 的 deep link 仍执行目标 Channel 权限，不会把 source 本身公开；
operator 的 legacy approval 只确认 Meeting 记录的可见性扩张。

迁移不得静默删除 Meeting、Board、Speech、Project View、Document 或 Context 数据。

## 5. Meeting Coordinate 协议

### 5.1 closed union 新增第三类坐标

Rust 领域模型追加：

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

wire JSON：

```json
{
  "coordinate_type": "meeting",
  "meeting_id": "0ed366aa-6f94-4eff-83db-b8bf081fbf35"
}
```

Desktop DTO 使用既有 camelCase 边界：

```json
{
  "type": "meeting",
  "meetingId": "0ed366aa-6f94-4eff-83db-b8bf081fbf35"
}
```

CLI token 固定为：

```text
meeting:<meeting-uuid>
```

### 5.2 身份与 Project scope

`meeting_id` 必须：

- 是非 nil RFC 4122 UUID v4；
- 对应当前 host-derived Community / Project 内的 `meeting_sessions.session_id`；
- 同时对应 `room_kind=meeting`，不能把普通 Channel UUID 伪装成 Meeting；
- 不接受调用者提交另一个 Project ID；
- 不包含 title、status、Board revision、Speech event ID 或 Action Run ID。

Meeting title、Board、roster 和终态变化都不改变坐标身份。

### 5.3 canonical 顺序与 tag

为保持现有坐标和 Edge identity，family rank 只能追加：

```text
0x00  Project View object
0x01  Project Document
0x02  Meeting
```

规范顺序变为：

1. Project View object；
2. Project Document；
3. Meeting；
4. 每个 family 内继续按已有 subtype 与 UUID bytes 排序。

Meeting 的 canonical coordinate bytes：

```text
0x02 || meeting_uuid_bytes
```

projection `c` tag：

```text
meeting:<project-uuid>:<meeting-uuid>
```

新 family 不得插入现有 rank 中间，也不得改变 Project View object type rank。

### 5.4 Edge key 保持稳定

Project Context wire schema 升级不等于必须更换 Edge key。实现应把两者解耦：

- Project View / Document-only exact set 继续得到与当前完全相同的 SHA-256 `edge_key`；
- `buzz-project-context-edge-v1\0` 继续作为 edge-key algorithm v1 的 domain separator；
- Meeting 使用此前未分配的 `0x02` variant byte，不与旧输入产生歧义；
- golden fixtures 固定旧 Edge key 不变，并新增 Meeting mixed-set fixtures；
- 数据库内任何旧 binding、change、projection identity 都不因协议升级换 key。

如果实现选择更换 domain separator，则必须迁移所有 Edge key、binding、change 和事件引用，风险和收益
不匹配，本设计明确不采用。

## 6. schema、capability 与协议切换

### 6.1 Project Context 升级为 v2

当前 v1 是 `deny_unknown_fields` 的 closed union，旧客户端无法安全解析 Meeting。因此新增坐标必须形成
新 schema / capability：

```text
schema_version = 2
buzz-project-context-edge-v2
```

NIP-PCE、shared fixtures、SDK、Relay、CLI、ACP 与 Desktop 必须一起升级。v2 使用同一 event kind，
通过 `schema_version`、严格内容和 capability 区分，不新增业务 HTTP endpoint。

### 6.2 不提供双写或降级写

切换采用单一当前版本：

- capability 启用前完成数据库迁移和 v2 reprojection；
- 启用后只公告 `buzz-project-context-edge-v2`；
- 新 command 只接受 schema 2；
- 不同时写 v1/v2 binding；
- 不把 Meeting coordinate 从结果中剥离后伪装成 v1 Edge；
- 旧客户端得到明确 `migration_required` / `unsupported`，不能观察半个 Edge。

现有 v1 Edge 数据保留并在 v2 中完整可读；“不兼容旧客户端”不等于删除旧项目数据。

已有 `buzz-project-context-v1` 表示 Project View Context Reference，是另一项独立能力。升级 Edge capability
时不能误删、改名或复用它。

### 6.3 Meeting 读取语义 capability

Meeting 的可见性变化是独立的安全合同，Relay 应新增并公告：

```text
buzz-meeting-community-read-v1
```

Project Context v2 只有在以下条件同时满足时才公告 ready：

- Project View v3 ready；
- Project Document v1 ready；
- 当前 host 的 Context state 为 schema 2；
- 所有 current binding/meta 都是 schema 2、属于同一 current `projection_generation`；
- current projection signer 与当前稳定 Relay signer 一致；
- Meeting community-read capability ready；
- 完整 legacy Meeting visibility approval 与 source risk preflight 已通过；
- host-derived Community identity 正常。

这样不会出现 Context 已接受 Meeting Coordinate，但普通 member 无法打开目标 Meeting 的状态。
切换时移除 `buzz-project-context-edge-v1` 公告，但保留独立的 `buzz-project-context-v1` Reference
capability。capability off 或半迁移状态下，普通 v2 read path 必须 fail closed，不能拼接 v1/v2 current
heads 返回结果。

## 7. 数据库迁移

### 7.1 Project Context 表约束

新增非破坏迁移应：

1. 第一阶段把 `project_context_edge_state.schema_version` 约束扩展为 `IN (1, 2)`，保留非空 v1
   Community 逐个迁移的合法过渡；
2. 扩展 `project_context_edge_coordinates_shape_check`，允许：

   ```text
   coordinate_type = 'meeting'
   coordinate_subtype IS NULL
   canonical_key = 'meeting:' || community_id || ':' || coordinate_id
   ```

3. 扩展数据库 edge-key guard，使 Meeting 编码为 `0x02 || uuid_send(coordinate_id)`；
4. 更新完整性验证函数，验证 canonical JSON、ordinal、canonical key 与 hash；
5. 增加 Meeting coordinate lookup 索引需要时复用现有
   `(community_id, coordinate_type, coordinate_subtype, coordinate_id)`；
6. 不 hard delete 或重建现有 Edge / binding / change 行；
7. 不重置 `context_revision`；
8. 不改变已有 Edge key。

只有所有目标 Community 都完成 operator cutover 后，后续 migration 才可以选择把约束收紧为 `= 2`。
普通 schema migration 不能在仍有 v1 行时直接改成 `= 2`，也不能代替需要 Relay signer 的 projection
重签。

当前 SQL hash guard 的非 Project View 分支会把未知 coordinate type 当成 Document。迁移必须改为
`project_view_object | document | meeting` 三个显式分支，并对未知 type `RAISE`；否则未来错误类型可能
计算出一个貌似合法的 Edge key。

### 7.2 Meeting identity 与终态外键

不建议从 Context coordinate 表直接建立会阻碍历史保留的硬级联外键。`meeting_sessions` 是运行期权威
resolver，Context Edge 需要像 tombstoned Project View / Document 坐标一样保留稳定身份。

Relay / DB 应提供一个结构化 resolver，而不是让各入口分别拼接 Channel、Session、State 与 End 查询：

```text
MeetingCoordinateResolution =
  Terminal {
    meeting_id,
    normalized_outcome,       closed | aborted
    state_revision,
    create_event_id,
    state_event_id,
    end_event_id
  }
  | Active
  | OrdinaryChannel
  | MissingOrForeign
  | InvalidTerminal
```

只有调用者先通过 Community write authorization 后，attach 才可以把 `Active`、`OrdinaryChannel` 和
`InvalidTerminal` 映射为精确诊断。`MissingOrForeign` 始终合并不存在与跨 Community，避免利用写接口枚举
其他 Community。读取入口继续使用无侧信道的统一 not-found 语义。

旧协议 Meeting 的终态需要一次确定性 normalization，不能仅凭某个 nullable status 猜测：

- verified normal End 对应 `closed`；
- verified abort / administrative abort 对应 `aborted`；
- Create、State、End 证据无法形成有效终态链时保持可读，但标记 `InvalidTerminal`，不能新 attach；
- normalization 结果及其证据 ID 纳入 legacy visibility audit，不改写历史 Nostr event。

attach 使用固定锁顺序：先锁当前 Community 的 Project Context state / Edge mutation scope，再以同一
Community 锁定目标 Meeting session/state，重新执行 terminal resolver，最后写 binding 与 projection。
Meeting 终态不可 reopen，Meeting End 路径不反向获取 Context lock，因此不会形成锁环。若未来允许 reopen，
必须重新设计该事务边界，不能沿用本约束。

detach 只读取已经持久化的 canonical exact set 并解除 binding，不调用 live Meeting resolver，也不要求
Meeting 仍存在或可 hydration。这样未来归档、tombstone 或异常终态都不会锁死 Context Document。

### 7.3 schema 2 reprojection

迁移后执行幂等 reprojection：

- 保留 canonical DB state、Context Revision、Edge key 和 source change；
- 提升 `projection_generation`；
- 为所有当前 binding/meta 生成 schema 2 Relay-signed projection；
- 验证 active/deleted binding 数量、Document ownership 与 Edge 聚合不变；
- 在完整验证成功前不公告 v2 capability。

reprojection 是投影升级，不是业务 Revision，也不创建虚假 attach / detach。

新 runtime 保留一个冻结、只供 admin cutover 使用的严格 v1 DTO/parser/verifier。它先验证 v1 current
meta、binding、canonical DB state、Edge key 和 Relay signer 一致，再构造 v2 projection。该 verifier
不能进入普通 read/write path；普通客户端命令和 capability 启用后的读取仍只接受 schema 2。

每个新 v2 binding/meta head 必须满足仓库 current-projection replacement contract：时间/tie-break 明确
晚于对应 v1 head，在同一事务更新 current projection event IDs。提交后通过 point-read 验证每个 `d`
identity 只解析到 v2 current head。历史 v1 events 保留为审计记录，不继续作为 current projection。

当前 reproject guard 默认禁止 schema version 原地变化。迁移需要提供一次 operator-controlled、事务内的
`1 -> 2` transition，而不是绕过 trigger 手工更新。transition 必须同时验证旧 key 不变、current head
全部重签和新 generation meta 完整后才提交。任一验证失败时，schema、generation、current event IDs 和
capability 必须全部保持原值。

### 7.4 Meeting Community-read enable state

授权切换需要持久、host-scoped 的 operator 状态，不能只依赖进程环境变量。数据库至少记录：

```text
meeting_community_read_enabled
legacy_meeting_visibility_watermark
legacy_meeting_visibility_audit_digest
legacy_meeting_visibility_approved_at
legacy_meeting_visibility_approved_by
```

digest 覆盖 watermark 之前的全部 Meeting 终态证据；source 分类只是风险报告的一部分。只有完整 corpus
digest、watermark 与批准记录一致，且没有 active legacy Meeting 时，才能启用并公告 capability。
approval 与 enable 必须在阻止 legacy Meeting 新写入的窗口中完成，消除审批后的 Board/Speech TOCTOU。

启用后创建的新 Meeting 使用 Community-public contract，不进入 legacy digest；合法新 Meeting 数量增长
不会使 capability readiness 回退。source 后续变窄只影响 deep link，不撤回已发布的 Meeting。

该状态只控制读取语义 readiness，不修改 frozen roster、Meeting event 或 Channel membership。

## 8. Meeting Community 读取实现

### 8.1 DB read predicate

当前 `meeting_channel_ids_for_frozen_reader()`、`is_meeting_reader_authorized_for_channel()` 和 live recipient
filter 把 roster 与 durable per-Meeting revocation 当作读取门。需要引入统一 predicate：

```text
is_meeting_community_reader(community_id, pubkey)
  = community_global_authorized_principal(community_id, pubkey)
    AND credential_can_read_community_global_scope
```

所有 Meeting read path 调用该 predicate。frozen participant 查询继续保留给写命令、participant type、
主持和状态机使用，不能被删除或误用为 observer roster。

建议从现有 Project View authorization 中抽出语义中性的
`community_global_authorized_pubkey(s)` DB helper，由 Project View、Project Document、Project Context 和
Meeting 共用。不要复制一份近似 membership SQL，否则 managed Agent、owner、ban、deactivation 和
credential scope 会再次漂移。

共享 principal predicate 必须显式覆盖：

- Human：当前 direct relay member，actor 未 deactivated / banned；
- managed Agent：Agent actor 未 deactivated / banned，其权威 owner 当前有效且满足现有 Community
  membership 规则；
- owner 被 deactivated、移除或 banned 后，其 managed Agent 不再借旧 owner 关系读取；
- Relay-managed / 其他系统 identity 继续使用现有明确登记规则，不通过 roster 推断 membership；
- 读取时检查当前状态，缓存只能作为优化，不能延迟安全撤权。

如果现有 Project View helper 缺少任一 deactivation 检查，应先补齐共享 helper，再让各项目资产复用；
不能把“抽取 helper”本身视为已经满足新权限合同。

### 8.2 必须统一覆盖的 Relay 表面

权限迁移必须同时覆盖：

- WebSocket `REQ`；
- WebSocket `COUNT`；
- `POST /query`；
- `POST /count`；
- IDs-only、mixed-kind、wildcard 和 search candidate 过滤；
- point read；
- initial subscription snapshot；
- live fan-out recipient filtering；
- reconnect / resubscribe；
- Meeting list、history、show、participants、Board、Speech、activity 与 action status；
- Desktop Tauri read aggregation；
- CLI `meetings list|show|participants` 和其他只读命令。

任一表面继续使用 roster read fence 都会形成“能从 Context 看到坐标，却打不开会议”或侧信道泄漏。

其中 targeted `#h` 查询必须先判定目标 Channel 是否为 Meeting，再选择 Meeting Community reader
规则；不能先按普通 private Channel membership 拒绝。全局查询则要把当前 member 可读的全部 Meeting
ID 合并到 authorized Channel scope。两条路径仍受调用凭据自己的 global / channel scope 限制。

### 8.3 写路径保持原状

以下写入仍必须验证 frozen roster、participant type、主持身份、grant、revision、lease 和 action fence：

- Human floor request / withdraw；
- Intent、Offer、ACK / decline、Grant、Speech、Yield、Handoff；
- Board maintenance 与 Floor decision；
- 主持人的正常 close / abort；
- Action Finalization renew、confirm、block、retry、return-to-board；
- Agent direct actions。

Community owner/admin 不是当然主持人；Community-wide observer 不能借助 Desktop、CLI 或原始 Nostr event
绕过这些校验。现有 owner/admin 的 administrative Meeting End 继续作为独立应急治理命令保留；它只能
结束会议并留下明确的管理性终态证据，不能因此获得 Speech、Board、Floor 或 Action Finalization 权限。

## 9. Meeting Coordinate 生命周期

### 9.1 只有终态 Meeting 可以新 attach

首版 attach resolver 只接受：

```text
status = ended
terminal_outcome IN (closed, aborted)
```

活动 Meeting 对所有 member 可读，但不出现在 Context Coordinate picker 中，也不能建立新的 binding。
原因不是权限，而是 Meeting 尚未形成稳定的最终记录。

这里的 `status` / `terminal_outcome` 指第 7.2 节 resolver 从 verified Create → State → End 链得到的
normalized terminal，不是客户端字段或单条 event 的自述。旧 Meeting 无法证明终态时保持可读但不可
attach，直到 operator 修复或补齐可验证投影；系统不得把它猜成 `closed`。

### 9.2 终态与 Action Finalization

`closed` 不要求一定产生外部写入；`aborted` 也可能具有长期解释价值。两者都可作为坐标。Context 不把：

- `actions-recorded`；
- `returned-to-board`；
- `blocked`；
- 某个 Action Run

建模为独立坐标。Inspector 可以把这些作为 Meeting 状态摘要显示。

### 9.3 归档、不可用和 detach

- Meeting 归档不改变坐标；
- 终态 Meeting 不重新打开；
- Meeting metadata 暂时读取失败时，Edge 仍保留，detail 为 `unavailable`；
- 如果未来支持 Meeting tombstone，Edge 仍按稳定 ID 保留并标记 `tombstoned`；
- detach 使用已存 canonical exact set，不因 Meeting unavailable 被阻塞；
- 不级联删除 Context Edge 或 Context Document。

## 10. Project Context 查询与 hydration

### 10.1 集合查询语义不变

新增 Meeting family 不改变：

- `exact({A,B})`；
- `incident(A)`；
- `contains-all({A,B})`；
- 空 `contains-all` 表示 All Context；
- exact coordinate set 唯一性；
- Edge 无向性；
- 一份 Context Document 最多属于一条 active Edge。

Meeting 可以和任意现有坐标组成 binary Edge 或 hyperedge。

### 10.2 metadata-first hydration

Project Context 初次查询只返回 Meeting 轻量信息：

```text
meeting_id
title
discussion_goal
lifecycle_status
terminal_outcome
host_pubkey
participant_count
participant_preview[] { pubkey, participant_type }   最多 3 项
created_at
ended_at
source_channel_id?   仅在 source 合法且当前调用者可定位时
action_finalization_summary?
```

Profile 名称、头像等属于 presentation enrichment，不进入 Meeting 坐标身份或 Edge key。最终 Board、完整
Speech、完整 roster、Intent/Floor log 和 Action evidence 不嵌入 Project Context query result。用户选中
Meeting node / Inspector 后才按需读取完整 roster；点击 `Open Meeting` 后再读取 Board 与 Speech。

hydration 必须有明确资源上限，避免 Context 结果形成 N×M Meeting 查询：

- 先对 query page 内的 Meeting ID 去重；
- metadata DB 查询按最多 32 个 ID 一批，最多 4 批并发；
- 总量受现有 Context page/result limit 约束，不因 participant 数量扩大主结果；
- profile enrichment 只覆盖 host 与 preview，完整 roster enrichment 在 Inspector 中单独分页；
- 单个 Meeting 失败只产生该项 `unavailable` observation，不重试风暴或拖垮整个 Edge query。

对 canonical DB 中已经存在的 Meeting coordinate，verified resolver 返回 missing、foreign、ordinary Channel
或无效终态表示完整性失配，应标记 `verification_failed` 并告警；只有网络、依赖或短暂读取错误才标记
`unavailable`。两者都不能伪造 metadata 或从 Edge 中静默删除坐标。

### 10.3 独立 Meeting observations

Meeting 没有 Project View / Document 那样的全 Project catalog revision，不能伪造一个全局 Meeting
observation。Desktop 查询结果应为每个唯一 Meeting coordinate 返回独立验证边界：

```text
meetingObservations[] {
  meetingId
  state                    observed | unavailable | verification_failed
  stateRevision?
  createEventId?
  stateEventId?
  endEventId?
  updatedAt?
}
```

每项都来自同一 verified Meeting snapshot；某一场失败只降级该 coordinate，不污染其他 Meeting detail。
Context Revision 只描述 Edge/binding 变化。Meeting metadata 或 state 的变化不能伪造 Context Revision；
客户端通过 per-Meeting observation 和 live invalidation 刷新 detail。

### 10.4 无额外逐坐标 ACL

完成 Community-wide Meeting 读取迁移后，Project View、Document、Meeting 与 Context Edge 都复用当前
Community membership。Context 查询无需因 Meeting 再建立 roster 权限交集。

仍需验证：

- 所有坐标属于同一 host-derived Project；
- Context Document 属于同一 Project；
- caller 是当前 Community member；
- attach 时 Meeting 已终态；
- 完整 legacy visibility approval 与 Meeting Community-read gate 已通过。

## 11. CLI 与 ACP

### 11.1 CLI surface

现有 Agent-first CLI 扩展：

```text
buzz project-context incident meeting:<meeting-id>
buzz project-context exact meeting:<meeting-id> requirement:<requirement-id>
buzz project-context contains-all meeting:<meeting-id> document:<document-id>
buzz project-context attach \
  --coordinate meeting:<meeting-id> \
  --coordinate requirement:<requirement-id> \
  --document <context-document-id>
```

具体命令层级沿用当前 CLI，不新增专用 HTTP endpoint。compact / JSON 输出必须返回 typed Meeting
coordinate 和轻量 metadata；非法 UUID、跨 Community Meeting、普通 Channel 和 active Meeting 使用精确错误。

### 11.2 ACP 稳定认知

`[Project Space]` 稳定 contract 增加最小语义：

- Meeting 是 Community-visible 的项目会议记录；
- frozen roster 控制参与和行动，不控制读取；
- 终态 Meeting 可以作为 Project Context 坐标；
- Context Edge 仍需普通 Project Document 解释；
- 需要 Board / Speech 时按需执行 Meeting read，不把全部历史注入每个 Agent Turn；
- 工作影响 Meeting、View、Resource、Document 或 Context 时显式写回，不自动推断 Edge。

Role Brief 不应默认携带所有 Meeting。只有通过现有 Project Context / Role provenance 发现相关坐标后，Agent
才按需读取 Meeting。

## 12. Desktop 设计

### 12.1 Meetings 导航

Meetings section 改为 Community-readable：

- 当前 member 可以打开所有活动和历史 Meeting；
- 列表显示自己是 Host、Participant 或 Observer；
- Observer 不显示申请发言、Speech composer、主持或 Action 控件；
- roster participant 的现有交互不变；
- 非 roster Meeting 不产生 attention / unread；
- Community 切换后清空 Meeting 与 Context 的 module-level cache，不能串 Community。

当前 Desktop shell 主要从 `memberChannels` 派生 Meeting；权限迁移后必须改为经过 Relay 授权的全部
`roomKind=meeting` Channel，否则 native 已经可读、侧栏仍无法发现。Meeting Create、State、Board、
Speech 和 End 的 live invalidation 也要从 roster-only recipients 扩大到当前 Community readers。

当前 `useUnreadMeetingIds` 一类逻辑不能直接对新的 Community-wide `meetingItems` 全量计算未读；它必须先
按当前 principal 是否为 frozen participant / host 过滤，observer 只获得可见列表与 live refresh，不产生
未读点、attention 计数或 Agent working 信号。该过滤需要独立单元测试和 Desktop E2E，防止读取扩权
意外演变为通知扩权。

### 12.2 Coordinate picker

Project Context Query Bar 增加 `Meetings` 分组：

- 只列终态 `closed` / `aborted` Meeting；
- 支持按标题、讨论目标、主持人和 participant 名称搜索；
- 选项显示标题、终态、结束时间与简短 participant 摘要；
- 使用稳定 `meeting:<uuid>` key；
- Incident、Exact、Contains all 复用现有安全 draft transition；
- active Meeting 不显示为可 attach 候选。

### 12.3 Graph node 与 Inspector

Meeting Coordinate 使用独立图标和标签，不伪装成 Project View object。Inspector 至少显示：

- Meeting 标题；
- `closed` / `aborted`；
- Discussion goal；
- Host；
- participant 名称、类型和 roster 数量；
- 创建和结束时间；
- Action Finalization 摘要；
- `Open Meeting`。

`Open Meeting` 复用现有 Channel route：

```text
/channels/<meeting_id>
```

`ChannelRouteScreen` 根据 `room_kind=meeting` 挂载 `MeetingScreen`，不新增第二套 Meeting detail route。
返回 Project Context 时恢复原 query、selection 和 graph viewport。

### 12.4 详情按需读取

Project Context Inspector 不内嵌完整 Board / Speech timeline。点击 `Open Meeting` 后，由 Meeting 领域读取并
验证完整记录。这样保持：

- Project Context 首屏有界；
- Meeting 只有一个详情体验；
- Board/Speech live state 不复制进 Context cache；
- Context Edge 不承担会议正文版本管理。

## 13. 主要代码影响面

| 层 | 主要位置 | 变更 |
|---|---|---|
| 领域 | `crates/buzz-project-context/src/coordinate.rs` | Meeting variant、rank、tag、identity bytes |
| 协议 | `docs/nips/NIP-PCE.md`、fixtures、`buzz-sdk` | schema 2、builder/parser/projection verifier |
| DB Context | `crates/buzz-db/src/project_context.rs`、新 migration | shape、resolver、hash guard、query/reproject |
| DB Meeting | `crates/buzz-db/src/meeting.rs`、`meeting_v2.rs` | Community reader predicate、终态 metadata batch read |
| Relay | query/count/side effects/state/NIP-11 | 所有 read surface 与 live fan-out、capability readiness |
| CLI | `crates/buzz-cli/src/commands/project_view_snapshot.rs`、Project Context commands | token、typed output、attach/query |
| ACP | Project Space contract 与 Meeting coordinator read path | 稳定语义、按需读取，不自动注入 |
| Desktop native | `desktop/src-tauri/src/commands/project_context*`、`meetings*` | DTO、batch hydration、community reader read model |
| Desktop frontend | `desktop/src/features/project-context/*`、`meeting/*` | picker、graph、Inspector、observer UI、导航 |
| 文档 | Project Context / Meeting specs 与 acceptance | 删除 roster-private read 旧语义，固化新矩阵 |

实际实现前应通过 `rg` 再次枚举所有 Meeting reader fence，不能只修改上述入口。

## 14. 错误语义

新增或调整的稳定错误至少包括：

| 场景 | 结果 |
|---|---|
| caller 不是当前 Community member | `restricted:community:membership_required`，不泄漏 Meeting |
| Meeting UUID 不存在或属于其他 Community | `invalid:project_context:meeting_not_found`，写入路径不区分跨 Community |
| UUID 指向普通 Channel | `invalid:project_context:not_a_meeting` |
| attach active Meeting | `invalid:project_context:meeting_not_terminal` |
| Meeting 存在但无法验证有效终态链 | `invalid:project_context:meeting_terminal_invalid` |
| private source 创建 Community-wide Meeting | `restricted:meeting:source_not_community_readable` |
| Relay 尚未完成 v2 reprojection | `unavailable:project_context:not_ready` |
| client 只支持 v1 | `migration_required:buzz-project-context-edge-v2` |
| Meeting metadata 暂时不可读 | Edge 结构保留，detail=`unavailable`；不伪造 tombstone |

读取路径对 unauthorized、not found 和跨 Community 必须保持无侧信道；上表中的精确原因只用于已经通过
Community membership 的受信写入诊断。

## 15. 非破坏迁移与发布顺序

### 15.1 阶段 A：代码 dark launch

- 更新领域、SDK、DB、Relay、CLI、ACP 与 Desktop；
- 默认不公告 `buzz-meeting-community-read-v1` 和 `buzz-project-context-edge-v2`；
- 完成 isolated database / Mock Bridge / E2E；
- 禁止使用主开发数据库执行 destructive migration test。

### 15.2 阶段 B：schema 与投影升级

- 对真实数据库先做备份与只读完整性检查；
- 应用 additive migration；
- 验证所有旧 Edge key、Context Revision 和 binding counts；
- 执行 schema 2 reprojection；
- 运行 `project_context_validate_community()` 等全量不变量检查。

### 15.3 阶段 C：Meeting visibility preflight

- 暂停 Meeting Create，等待全部 active Meeting 结束并冻结 legacy watermark；
- 审计 watermark 前的完整 legacy Meeting corpus，生成稳定 visibility digest；
- 生成 Community-wide / private / missing source 风险报告，但不把 source 报告当成完整审计的替代品；
- 验证 active membership predicate；
- 验证 owner/admin/member Human 与 Agent 读取矩阵；
- 验证普通 member observer 写命令全部被拒，并保持 owner/admin 既有 administrative End；
- 验证历史 Meeting、Board、Speech、Action 数据数量未减少。

建议提供幂等 operator surface，命令名可按现有 `buzz-admin` 风格调整，但必须覆盖：

```text
buzz-admin meeting-community-read status
buzz-admin meeting-community-read audit
buzz-admin meeting-community-read approve \
  --watermark <watermark> \
  --audit-digest <digest>
buzz-admin meeting-community-read enable
buzz-admin project-context migrate-v2 --verify
```

`approve` 必须同时绑定当前 watermark 与完整 corpus digest；数据变化后旧批准不能继续复用。`enable` 必须
再次确认 Create 仍暂停、没有 active legacy Meeting、watermark 和 digest 未变化。它只切 gate，不执行
schema migration、数据删除或隐式 source 修改。

### 15.4 阶段 D：原子启用

同一 operator workflow 中：

1. 启用 Meeting Community read；
2. 验证 `/info` 公告 `buzz-meeting-community-read-v1`；
3. 启用 Project Context schema 2；
4. 验证 `/info` 公告 `buzz-project-context-edge-v2`；
5. 用普通 member 执行真实 Meeting read 和 Meeting Coordinate query；
6. 最后允许 Meeting Coordinate attach；
7. 仅在新 Create 已强制 Community-public contract 与 source gate 后恢复 Meeting Create。

任一步失败都保持 capability fail closed，不删除或重建数据。

### 15.5 回滚边界

- capability 启用前可以回滚代码，保留 additive schema；
- 已生成 Meeting Coordinate binding 后，旧 v1 runtime 无法安全解释当前状态，不能直接降级运行；
- 回滚应先关闭 attach、保留 v2 read/detach runtime，再修复 forward；
- 不通过删除 Meeting Edge、清空 Context 表或重置数据库实现回滚。

## 16. 测试方案

### 16.1 Project Context 领域与协议

- Meeting UUID v4 校验；
- canonical family rank 为 `2`；
- mixed coordinate 排序稳定；
- Meeting tag 与 identity bytes golden fixture；
- v1 Project View / Document-only Edge key 在 v2 实现中逐字节不变；
- Meeting + Requirement、Meeting + Document、三类混合 hyperedge key；
- duplicate Meeting 拒绝；
- schema 1 parser 不接受 Meeting，schema 2 parser 严格接受；
- builder、Relay parser、projection verifier 与数据库 hash 一致。

### 16.2 Meeting 权限矩阵

至少覆盖：

1. owner/admin/member Human 非 roster 读取活动与终态 Meeting；
2. Community Agent 非 roster 读取；
3. 新成员读取加入前历史 Meeting；
4. 移除/封禁后立即无法读取；
5. Community 外身份无法 list/count/show/deep-link；
6. 普通 member observer 无法 request、say、yield、host action、close 或 finalize；
7. owner/admin 非 roster 只能执行既有 administrative End，不能获得其他 Meeting action；
8. roster participant 和主持现有流程不回退；
9. participant revocation 仍影响状态机，但不成为独立历史 read ACL；
10. REQ、COUNT、HTTP query/count、point-read 与 live fan-out 结果一致；
11. reconnect 不重放未授权 Meeting。

### 16.3 source

- Community-wide source 成功；
- private Channel、DM、missing source 失败；
- Desktop 和 CLI 预检不能替代 Relay 校验；
- 存量 source preflight 报告准确且不修改数据；
- source deep link 继续执行目标权限。

### 16.4 attach、detach 与 lifecycle

- active Meeting attach 拒绝；
- closed / aborted Meeting attach 成功；
- legacy Meeting 只有 verified End 可以 normalize；证据不完整时可读但 attach 拒绝；
- 跨 Community / 普通 Channel 拒绝；
- Meeting unavailable 后 Edge 仍查询得到；
- unavailable / tombstoned 状态仍可 detach；
- attach 在固定锁顺序内重验终态，detach 不调用 live Meeting resolver；
- Meeting 结束和 Action Finalization 不自动创建 Edge；
- Context Revision 只随 attach/detach 推进；
- Meeting metadata 更新不推进 Context Revision。

### 16.5 CLI、ACP 与 Desktop

- 三类 query 支持 Meeting token；
- Agent 按需读取 Meeting，不向每 turn 注入完整历史；
- picker 只列终态 Meeting；
- Meeting node、首屏有界 metadata、Inspector 分页 roster enrichment；
- `Open Meeting` 跳转并可返回原 Context selection；
- observer 页面只读且无瞬时写控件；
- observer Meeting 不进入 unread / attention / Agent working 统计；
- roster participant 页面和 Meeting Activity 不回退；
- Community A → B → A 不泄漏 Meeting / Context cache；
- active Meeting live update 不让 Project Context 页面崩溃或伪造 Edge 变化。

### 16.6 数据安全回归

真实迁移验收前后比较：

- Community、Channel、Meeting、participant、Board、Speech、Action Run 数量；
- Project View revision 与对象数量；
- Document catalog、revision 和正文 hash；
- Context Revision、Edge key、coordinate、binding 与 change 数量；
- Nostr event 数量和投影 generation；
- Docker volumes 与数据库 identity。

所有 destructive 测试必须使用显式 scratch database，并通过与
`docs/lora/stage/bug/destructive-migration-test-main-database-data-loss.md` 相同的主库保护门禁。

## 17. 分阶段实现计划

### 阶段 1：领域规范与权限读取链

- 更新 Meeting / Project Context 领域规范；
- 实现 Community reader predicate；
- 迁移 list/query/count/live fan-out；
- 保持参与/主持写命令 roster-scoped，并保留 owner/admin administrative End；
- 完成 source create gate。

完成标准：任意有效 member 可以读，observer 无法写，非 member 无侧信道。

### 阶段 2：Project Context v2 协议与数据库

- Meeting coordinate、canonical bytes、tag、fixtures；
- schema 2 NIP-PCE / SDK；
- additive migration、DB resolver/hash guard；
- v2 reprojection 与 readiness。

完成标准：旧 Edge key 不变，Meeting mixed Edge 可在 isolated Relay 完整 attach/query/detach。

### 阶段 3：CLI 与 ACP

- CLI token、typed output、错误语义；
- Project Space stable contract；
- Meeting metadata / Board / Speech 按需读取；
- live Agent 真实命令验收。

完成标准：Agent 不依赖 Desktop 即可建立、发现和解释 Meeting Context Edge。

### 阶段 4：Desktop

- Community Meetings 导航与 observer 模式；
- Meeting Coordinate picker、graph、Inspector；
- `Open Meeting` 与返回定位；
- live invalidation、Community reset 与 E2E。

完成标准：Human 可以从 Context 图理解会议坐标并进入完整会议记录，非 roster 不获得行动控件。

### 阶段 5：非破坏迁移与真实验收

- 全量 legacy visibility audit、watermark 与 source 风险报告；
- schema 2 reprojection；
- capability 原子启用；
- Community member / Agent / non-member 真实矩阵；
- 数据前后对账与长期运行日志。

完成标准：真实 Community 数据完整，v2 与 Meeting read capabilities ready，端到端验收通过。

## 18. 最终验收标准

本功能只有同时满足以下条件才算完成：

1. 所有当前 Community member，不区分 owner/admin/member、Human/Agent，均可发现和读取所有 Meeting；
2. Community 外、被移除或封禁身份无法通过任何读表面观察 Meeting；
3. 普通 member 非 roster observer 无法执行 Meeting 行动；owner/admin 只保留既有 administrative End；
4. frozen roster、participant type、主持与 Action Finalization 行为没有回退；
5. 新 Meeting 不接受比 Community 更窄的 source；
6. 只有 closed / aborted Meeting 可以新建 Context binding；
7. `MeetingCoordinate { meeting_id }` 在协议、DB、CLI、ACP 和 Desktop 中语义一致；
8. 旧 Project View / Document-only Edge key、Context Revision 和 Document binding 全部保持；
9. Exact、Incident、Contains all 查询语义不变；
10. Project Context Inspector 显示轻量会议信息，并可跳转现有 Meeting 详情；
11. Board / Speech 不复制到 Edge，也不默认注入 Agent Turn；
12. Meeting 结束或物化不会自动推断 Context Edge；
13. capability 只在 Meeting Community read、schema 2 和 projection 全部 ready 后公告；
14. 迁移不删除、重置或重建任何现有业务数据；
15. unit、property、DB、Relay E2E、CLI、ACP、Desktop E2E 与真实验收全部通过。

## 19. 非目标

本次不实现：

- 动态 Meeting roster、Join、RSVP 或 observer follow；
- 对不同 Meeting 建立独立 ACL；
- 私有 Meeting 模式；
- 把 Board revision、单条 Speech、Intent、Grant、Action Run 或 Nostr event 作为坐标；
- 自动总结 Meeting 或自动创建 Context Document；
- 根据 Meeting 内容推断 Context Edge；
- 在 Project Context Inspector 内复制完整 Meeting UI；
- 通过 Role、Assignment 或 Runtime supervisor 改变 Meeting 读取权限；
- Mobile / Web 的首轮 UI；它们可在协议稳定后独立交付。

## 20. 已确认决定

1. Meeting 与 Project View / Document 同为 Community-visible 项目资产；
2. Community membership 控制读取，frozen roster 控制参与和行动；
3. Meeting Coordinate 只使用稳定 `meeting_id`；
4. Project Context 只在终态 Meeting 上允许新 attach；
5. Inspector metadata-first，完整记录跳转 Meeting 详情读取；
6. Context Document 继续承载解释性语义；
7. 新 Meeting source 必须 Community-readable；
8. 新坐标以 family rank `0x02` 追加，旧 Edge key 保持；
9. Project Context 以 schema / capability v2 完整切换，不双写 v1；
10. 迁移必须非破坏、可验证、fail closed。
