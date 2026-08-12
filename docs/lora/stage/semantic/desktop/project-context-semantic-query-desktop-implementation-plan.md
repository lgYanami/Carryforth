# Desktop Project Context 语义路径查询分阶段实现计划

> 状态：Phase D0–D5 已提交；Phase D6 的本地 synthetic 性能资格、light / dark 语义路径截图、
> local single-pod真实 Provider canary与feature-off / rollback已完成；known-negative仍产生候选，
> relevance / floor质量校准、source / revision stale smoke及production LB / multi-pod qualification未完成；
> 当前不构成 production-ready
>
> 日期：2026-08-11
>
> D0–D5 交付提交：`507790180 feat(desktop): add project context semantic paths`
>
> 上游查询计划：
> [Project Context 图语义检索分阶段实现计划](../project-context-graph-semantic-query-implementation-plan.md)
>
> 图领域规范：
> [Project Context V2 领域规范](../../project-context/project-context.md)
>
> Desktop 现有计划：
> [Project Context Desktop 分阶段实现计划](../../project-context/desktop-implementation-plan.md)、
> [Desktop Project Context 有机网状图布局实现设计](../../project-context/desktop-organic-graph-layout-implementation-design.md)
>
> Desktop 信息架构更新：
> [Project Context 全画布与可折叠右侧工具栏分阶段实现计划](../../project-context/desktop/project-context-full-canvas-workspace-implementation-plan.md)
> 取代本文 §4.1 中 Semantic Query Bar 位于结构 Query Bar 下方的固定位置描述。自然语言输入、
> session、All Context pairing、overlay、currentness 与安全边界继续有效。
>
> 本计划范围：Desktop 自然语言问题、可选初始 Coordinate、可选上下文 Coordinate、
> Native 可信查询、验证后的语义路径展示 DTO、All Context 图底座、持久语义高亮图层、
> selection / hover 正交组合、Context Document 路径标记及 Desktop 测试
>
> 明确排除：Relay / DB 查询协议改造、正文 chunk、Runtime 自由文本、ANN、自动 Agent 调用、
> 语义结果持久化、URL 分享、路径写回 Project Context、图编辑、Web 与 Mobile

## 0. 已确认的产品边界

本计划以以下决定为固定前提：

1. Desktop 提供独立的语义查询输入；
2. `problem` 必填；
3. `initial_coordinates[]` 可选，用于显式指定遍历起点；
4. `context_coordinates[]` 可选，用作相关性环境，不是 ACL、硬过滤或必经起点；
5. 当前点击的 Coordinate / Edge 不会被隐式加入任何查询输入；
6. 查询结果在图中形成独立的、持续存在的 semantic overlay；
7. 点击 Coordinate、Edge、Spoke、空白画布，或关闭 Inspector，只改变普通 selection；
8. 普通 selection 的建立或取消都不能清除 semantic overlay；
9. 用户显式 Cancel 才清除普通交互中的当前语义会话；Community / Project / 已观察到的权限 / capability、
   topology revision 或 trusted identity / 完整性边界变化属于安全失效例外；
10. 多条返回路径首版取并集展示；
11. 每个经过的 Hyperedge 仍按完整 Coordinate set 展示，不能伪装成二元边；
12. 语义结果是检索 snapshot，不是新的项目事实；Inspector 继续读取当前 canonical source；
13. Request / Result 继续使用已经交付的未版本化 closed schema，不新增 Desktop 专属 V1 / V2；
14. 本次增加 Desktop downstream consumer；除共享 SDK serializer与 Carryforth consumer回归外，不修改已经
    交付的 Relay `/query`、kind `40912` 或 pgvector 查询合同。

一句话目标：

> Human 用问题和可选环境找到一组相关上下文路径；语义路径作为持久图层留在完整上下文图上，
> 普通点击仍可自由检查任意节点或 Edge，且两种视觉状态互不覆盖。

## 1. 交付前实现基线

### 1.1 当前画布是准确的 incidence graph

`desktop/src/features/project-context/graph.ts` 已将 verified `ProjectContextQueryResult` 映射成：

```text
Coordinate Node ── Spoke ── Edge Hub ── Spoke ── Coordinate Node
```

现有不变量已经正确：

- 每个真实 Coordinate 只有一个 Node；
- 每条领域 Hyperedge 只有一个 Hub；
- Hub 与完整 Coordinate set 之间各有一条 Spoke；
- Context Document 是 Edge binding，不被画成 Node；
- 图无向、无箭头；
- layout 只负责 presentation，不改变领域成员关系。

本计划不修改 `graph.ts` 的领域投影，也不修改有机网状布局算法。

### 1.2 当前只有一套 selection emphasis

当前普通交互由 route search 中的 `selected` 驱动：

- 点击 Coordinate 选择 Coordinate；
- 点击 Hub 或 Spoke 选择完整 Edge；
- 再次点击同一对象清除 selection；
- 点击空白画布清除 selection；
- 关闭 Inspector 或按 Escape 清除 selection；
- selection 可通过 URL deep link 恢复。

`buildProjectContextFlowElements()` 目前只接收一个 target，并为全部 Coordinate、Hub、Spoke 计算：

```text
normal / active / dimmed
```

CSS 再通过 `data-emphasis` 控制 opacity、border、shadow 和 stroke。Hover 只在没有 selection 时通过
`data-hover-emphasis` 临时修改 DOM。

因此不能把语义路径塞进现有 `selection` 或复用同一个 `emphasis`：一旦用户点击空白、关闭 Inspector
或选择另一个对象，语义路径就会被覆盖。

### 1.3 当前结构查询与语义查询不是同一种操作

现有 `ProjectContextQueryBar` 提供：

- All Context；
- Exact；
- Incident；
- Contains all。

它们回答的是“按已知 Coordinate 集选择哪些真实 Edge”，并改变 canvas 的结构集合。

语义查询回答的是“根据问题和环境，哪些入口与 Hyperedge 路径更相关”。它不应被追加成第四种结构
query union，也不应改变 `ProjectContextQuery` DTO。

### 1.4 交付前 Desktop 尚无 semantic transport

Desktop Native 已有：

- 当前 Human Keys；
- Relay HTTP base URL；
- NIP-11 读取；
- canonical Relay `self` 校验；
- NIP-98 builder；
- `POST /query` 基础设施；
- Project View / Project Context verified reads。

但普通 `query_relay_at_with_keys_typed()` 不能直接承载语义查询，因为它不保留：

- exact authenticated body bytes；
- NIP-98 auth Event id；
- semantic request binding observation；
- semantic success body 上限；
- strict single canonical Event array；
- `SemanticGraphQueryResult::validate_for_request()` 所需的完整观察。

本次 Phase D1 已据此交付专用、one-shot 的 Tauri Native 调用面；本节保留为实施动机，
不再描述交付后的当前能力。

## 2. 总体架构

### 2.1 两个正交图层

Desktop 图状态拆成：

```text
Canonical graph layer
└── verified All Context ProjectContextQueryResult

Semantic overlay layer
└── verified SemanticGraphQueryResult 派生的 path/root identity set

Interaction layer
└── route-owned Coordinate / Edge selection + transient hover
```

三层职责不同：

- canonical graph 决定哪些 Node、Hub、Spoke 与 Context Document 当前存在；
- semantic overlay 只标记其中哪些身份参与某次 retrieval snapshot；
- selection / hover 只决定用户此刻检查什么。

Semantic overlay 不得增加任何 synthetic Node、Edge、Document 或 Spoke。

### 2.2 执行链

```text
problem + explicit initial Coordinates + context Coordinates
                         │
                         ▼
Desktop Tauri trusted command
├── capture Community / Relay / caller / verified Project
├── build unversioned closed SemanticGraphQuery
├── exact serialize + NIP-98 sign
├── one-shot POST /query
└── SDK request-aware verification
                         │
                         ▼
verified Desktop display DTO
├── result observation / coverage
├── roots
└── ordered path hops with complete Hyperedge membership
                         │
                         ▼
verified All Context graph structural join
                         │
                         ▼
semantic overlay union
├── complete Edge Hub / Spokes / Coordinate members
├── roots / continued targets
└── selected relation Documents by Edge
```

### 2.3 不使用 `cf` 作为 Desktop 子进程

Desktop 不启动、shell out 或解析 `cf project-context semantic-query`。Carryforth 和 Desktop 是同一查询
能力的两个独立 consumer：

- 共同复用 `buzz-semantic-query` 的 pure contract；
- 共同复用 `buzz-sdk::semantic_graph` 的 verifier；
- 各自在自己的 trusted native boundary 完成签名和 transport。

## 3. Desktop 查询输入

### 3.1 输入合同

Desktop 表单完整支持：

```text
SemanticQueryDraft
├── problem: String                         // 必填
├── initialCoordinates[]                    // 可选，最多 16
└── contextCoordinates[]                    // 可选，最多 8
```

底层 `lifecycle_filter=all_current` 与 `SemanticGraphQueryBudget::default()` 首版固定使用，不向普通用户
暴露 lifecycle 或十一个预算字段。后续若要提供 lifecycle filter，必须单独确认其 Human 文案与行为，
不能借本次 Initial / Context 输入顺带加入。

### 3.2 Problem

Problem 使用可换行输入：

- trim 后必须非空；
- 禁止 NUL；
- UTF-8 最大 16 KiB；
- 字节数必须用 `TextEncoder` 计算，不能使用 JavaScript `string.length`；
- Cmd / Ctrl + Enter 提交；
- Enter 本身保留换行；
- UI 不渲染 Problem Markdown；
- active 状态只显示截断后的 plain-text 摘要，完整文本仍只存在内存中。

### 3.3 Initial coordinates

Initial 文案固定表达为：

> Start from — 从这些坐标开始遍历。

语义：

- 它们是显式 structural roots；
- 不占自动 semantic root 预算；
- 合法但暂无 embedding 的 in-graph Coordinate 仍可能沿真实 Edge 展开；
- graph-external initial 会返回 `not_in_graph` observation；
- lifecycle filter 不会偷偷删除调用者明确指定的 initial root。

### 3.4 Context coordinates

Context 文案固定表达为：

> Query context — 影响相关性排序，不是过滤条件、权限或必经起点。

语义：

- 每个 Context Coordinate 独立生成一个 conditioned query channel；
- Context 不自动成为 root；
- Context 不扩大权限；
- Context 不把查询限制为它的邻域；
- graph-external、但当前可读且有 semantic head 的 source 仍可成为 lens。

### 3.5 Coordinate Picker

将当前 `ProjectContextQueryBar` 内部的 `CoordinatePicker` 抽成可复用组件。来源继续是：

- Project View 九类对象；
- Project Documents；
- Meetings。

两组 chips 独立管理：

- 同一组内 canonical 去重、稳定排序；
- 同一 Coordinate 允许同时出现在 Initial 与 Context，因为两种角色正交；
- 达到 16 / 8 上限后对应 picker 禁用并说明原因；
- tombstoned / unavailable 项不得被伪装为可用，Native 最终仍以 current source 验证为准。

### 3.6 当前 selection 绝不隐式进入查询

用户点击图节点仅表示“检查这个对象”，不表示：

- 以它为 initial root；
- 以它为 context lens；
- 限制 query 子图；
- 修改 active semantic query。

如果未来提供 Inspector 快捷按钮，只能是用户显式点击：

- `Use as starting point`；
- `Add to query context`。

快捷按钮只修改 draft，不能直接提交或清除现有 overlay。

## 4. UI 信息架构

### 4.1 独立 Semantic Query Bar

新增 `ProjectContextSemanticQueryBar`，放在现有结构 Query Bar 下方，不修改现有
`ProjectContextQuery` union：

```text
Semantic paths
[ Ask a question about this project…                         ] [Find paths]
[Start from: Issue C] [Context: Role A] [Context: Work B]       [Inputs]
```

紧凑布局下：

- Problem 占主宽度；
- `Inputs` 展开 Initial 与 Context；
- 已选择 Coordinate 以 chips 留在主栏下方；
- `Find paths` 在 blank、invalid、capability-off 或当前 attempt running 时禁用；
- capability 未广告时保留入口并显示明确 unavailable 文案，不静默隐藏。

### 4.2 Active 状态条

成功查询后显示：

```text
Semantic snapshot · “为什么发布后这个问题持续复发？”
6 paths · 4 roots · Context revision 42 · complete
[Fit paths] [Re-run] [Cancel]
```

状态条必须区分：

- zero paths：有效成功结果，不等于 idle；
- partial coverage：成功但索引覆盖不完整；
- budget exhausted：成功但达到有界预算；
- omitted context：部分 Context Coordinate 未形成 Qi；
- stale snapshot：来源或图在查询后发生变化；
- transport stale：断线期间只保留已验证 snapshot 提示。

分数不能标成 confidence。首版不展示 cosine / CandidateScore 数值，只保留 path 排序和 coverage 文案。

### 4.3 Draft 与 active 分离

编辑 Problem、Initial 或 Context 不改变当前高亮：

- draft 与 active submitted query 独立；
- draft 变化显示 `Draft · not applied`；
- 新查询成功后才原子替换 active overlay；
- 新查询发生 busy / timeout / unavailable 等 transient failure 时保留旧 overlay；
- restricted、verification failure、observed capability loss 或 identity mismatch 属于安全失效，清除旧 overlay；
- Cancel 清除 active / pending，但保留 draft 便于修改重跑。

### 4.4 结构 Query Bar 在 semantic mode 中的行为

Semantic overlay 必须建立在 All Context 底图上，而非当前 Exact / Incident 子图。

因此 active semantic session 期间：

- route 中原本的结构 query 不被改写；
- canvas 临时显示 verified All Context substrate；
- 现有结构 Query Bar 可继续编辑 draft，但 `Run` 禁用；
- guidance 显示 `Cancel semantic paths before applying a structural query`；
- Cancel 后恢复 route 当前 applied structural result；
- semantic mode 不向浏览器历史增加一个隐式 All Context navigation。

这样不会出现 Query Bar 显示 Incident、canvas 却静默混入全图路径的双重所有权。

### 4.5 Fit paths

首次成功激活可以对高亮子图自动 fit 一次。之后：

- 点击 / 取消 selection 不再自动 fit semantic path；
- 提供显式 `Fit paths`；
- 原有 Fit all / Fit Island / Fit selection 继续工作；
- reduced-motion 下所有 viewport 操作 duration 为 0；
- Cancel 不要求恢复精确旧 viewport，但必须恢复旧结构 result。

## 5. 前端状态机

### 5.1 状态形状

避免把所有状态塞进一个大 enum，采用正交字段：

```text
SemanticUiState
├── draft
│   ├── problem
│   ├── initialCoordinates[]
│   └── contextCoordinates[]
├── attempt
│   ├── idle
│   ├── running { token, submitted }
│   ├── pairing { token, submitted, verifiedDisplayResult }
│   └── failed { token, error }
├── active
│   └── null | SemanticSession
└── freshness
    ├── topology: matched | advanced
    ├── sources: no_change_observed | change_observed
    └── transport: live | uncertain
```

`SemanticSession` 至少包含：

```text
SemanticSession
├── localAttemptToken
├── requestId
├── submittedDraft
├── verifiedDisplayResult
├── overlay
├── projectContextRevision
├── snapshotObservedAt
└── community / applied workspace token / caller / relay / project identity
```

这里的 `verifiedDisplayResult` 只指 Native 在 SDK verifier 成功后映射出的 Desktop DTO；它不是 raw
`SemanticGraphQueryResult`，不含 preview、raw Event、exact body 或 Provider 数据。

### 5.2 转移规则

| 事件 | attempt | active | selection |
|---|---|---|---|
| 编辑 draft | 不变 | 不变 | 不变 |
| Run A | running(A) | 保留旧 active | 不变 |
| Native A 验证成功且 identity/token 有效 | pairing(A) | 保留旧 active，等待 All substrate | 不变 |
| A pairing：graph revision `< R` | pairing(A) | 保留旧 active，refetch graph | 不变 |
| A pairing：graph revision `== R` 且 atomic join成功 | idle | 原子替换为 A，freshness 重置 | 目标在 All substrate 仍存在才保留 |
| A pairing：graph revision `> R` 或 atomic join失败 | failed(A/conflict) | 保留旧 active但仍受当前 render gate约束 | 只保留仍存在的目标 |
| A transient 失败 | failed(A) | 保留旧 active | 不变 |
| A conflict | failed(A) | 先刷新 substrate；旧 active只在 render gate仍匹配时可见 | 只保留仍存在的目标 |
| A verification / restricted 失败 | failed(A) | 安全清除不再可信的 active | 由现有权限边界决定 |
| observed capability loss / trusted identity mismatch | reset | null，freshness 重置 | 由现有权限边界决定 |
| Cancel | idle，token 递增 | null，freshness 重置 | 目标在恢复的 focused substrate 仍存在才保留 |
| 点击 Node / Edge / Spoke | 不变 | 不变 | 选择或切换 |
| 点击 pane / 关闭 Inspector / Escape | 不变 | 不变 | null |
| Community / reinitKey 改变 | reset | null | 由现有 route / App 边界重建 |
| 新图 revision | 不变 | `topology=advanced`，同步停止覆盖新图 | 只保留仍存在的目标 |
| source hint | 不变 | 只调度 typed canonical refresh，不直接改 freshness | 不变 |
| verified canonical refresh 后 fingerprint mismatch | 不变 | `sources=change_observed` | 不变 |
| transport disconnect / reconnect uncertainty | 不变 | `transport=uncertain` | 不变 |

Freshness 三个轴可以同时变化，不能使用会互相覆盖的单一 enum。Derived 规则固定为：

```text
overlayEligible =
  active != null
  && freshness.topology == matched
  && active.projectContextRevision == canvas.contextRevision
  && atomicStructuralJoin(active, canvas) == valid

semanticFreshness =
  freshness.sources == change_observed || freshness.transport == uncertain
    ? stale
    : snapshot
```

只有 `overlayEligible` 时才向 Graph 传入 overlay。`semanticFreshness` 只决定 snapshot / stale 样式与提示，
不得令 topology advanced 的 overlay重新出现。成功、Cancel、Community reset 与 observed capability loss都显式重置
三个轴；Graph DOM 只接收闭合的 `data-semantic-freshness="snapshot|stale"`。

Selection 与 semantic overlay 的正交合同是“selection 操作不改变 overlay”。它不承诺在 substrate 被替换时
保留一个已经不存在的 target：进入 All Context 或 Cancel 恢复 focused result 时，只有目标 identity 仍存在于
next substrate 才保留 selection，否则沿用现有 `replace:true` 清除逻辑。

### 5.3 Cancel 与迟到响应

Tauri invoke 没有协议级 HTTP cancel。首版 Cancel 的合同是：

- UI 立即清除 semantic overlay；
- 本地 attempt token / epoch 递增；
- 已在途的 Native / Relay / Provider 工作可以完成；
- 任何旧 token 对应的 success / error 都被忽略；
- Cancel 后旧 response 不得恢复高亮或弹出过期 toast。

每次显式重试生成新的 Native request UUID 和新的 NIP-98 Event，不自动 replay 上一个请求。
Cancel、newer Run、Community reset或 workspace token变化也会使 `pairing` candidate失效；它不得在后续 graph
refetch完成后复活。Pairing DTO只存在组件内存，不进入持久 React Query cache。

### 5.4 状态存储边界

Problem 和 result 只存在当前 Community 的 Project Context React 内存状态：

- 不进入 route search；
- 不进入 URL；
- 不进入 Local Storage / Session Storage；
- 不进入持久 React Query cache；
- 不进入 module singleton；
- 不进入日志、analytics 或 crash breadcrumb；
- 页面 reload、离开 Project Context 或 Community remount 后清除。

这一定义中的“保持到 Cancel”特指当前 Project Context 会话内的点击、取消点击与 Inspector 操作；
不把敏感问题持久化成跨页面会话。

## 6. All Context substrate 与 snapshot 语义

### 6.1 为什么不能直接覆盖当前 focused graph

Semantic result 可能包含当前 Exact / Incident / Contains-all focused result 之外的 Edge。如果只高亮可见交集：

- 路径会缺步；
- coverage 会被 UI 静默缩小；
- Human 会把“未加载”误读为“未命中”；
- relation Document 所属 Edge 可能根本不在 canvas。

因此 semantic mode 使用 `contains_all([])` 的 verified All Context snapshot 作为唯一底图。

### 6.2 结构对齐合同

本计划选择：

> 使用全局 `project_context_revision` + 每个 hop 的完整结构集合匹配；首版不扩展 Desktop Edge DTO
> 携带逐 Edge / Binding provenance。

一个 semantic result 只有满足以下条件才能形成 overlay：

1. Community key、Relay signer、Project id 与当前 substrate 匹配；
2. `semantic.observations.project_context_revision === substrate.context.contextRevision`；
3. 每个 hop 的 `edge_key` 在 substrate 中存在；
4. `complete_coordinate_keys` 与 substrate Edge 的完整 Coordinate set 精确相等；
5. `current_context_document_ids` 与 substrate Edge 的完整 binding set 精确相等；
6. `selected_context_document_id` 属于该 binding set；
7. entered / continued Coordinate 都是该完整 set 成员。
8. 每个 Coordinate structural root在同 revision substrate中真实存在；
9. 每个 Context Document structural root的 `edge_key` 真实存在，且 `document_id` 属于该 Edge完整 binding set。

任一 hop或root不匹配，整份 overlay fail closed；不得高亮“能对上的部分”，也不得只保留 root / path计数而
没有真实画布落点。

全局 Context revision 会在 Edge / binding 改变时推进，因此 revision 相等与完整集合相等共同构成首版
结构 join。文档不得宣称 Desktop 已逐行验证未暴露的 Edge provenance。

### 6.3 初始 pairing

获得 semantic result revision `R` 后：

- All Context snapshot revision `< R`：普通结构读可能滞后，允许 refetch；
- revision `== R`：执行完整集合 join，成功后激活；
- revision `> R`：当前只读面无法重建历史 `R`，不自动再次调用 Provider，提示 `Context changed · Run again`。

不允许为了对齐 revision 而静默重放语义请求，因为每次 query 都有 Provider 成本和数据出域。

### 6.4 Result 是 snapshot，不宣称持续 current

`SemanticGraphQueryResult.observations.snapshot_observed_at` 表示 Stage C writer-DB snapshot。Stage D 只保证
释放时安全，并不把结果升级成一个持续授权或持续 current 的 lease。

同时，Project View title / summary、Document revision / summary、Meeting summary 更新不一定推进
`project_context_revision`。因此 revision 相等只证明图结构一致，不证明评分所依据的 overview 仍是最新文本。

首版明确采用 snapshot overlay：

- 状态条显示 snapshot time 与 Context revision；
- 状态文案始终使用 `Semantic snapshot`；`no_change_observed` 也只表示 Desktop 尚未观察到后续变化，
  不能证明 HTTP 到达时 source 仍 current；
- 不把 semantic preview 用作 Node / Inspector title 或 summary；
- Human 点击对象时继续看到当前 canonical content；
- 激活时从 verified All Context substrate 建立仅驻内存的 source-observation fingerprint baseline；
- 该 baseline只用于发现激活后的变化，不能证明 Stage C 到 HTTP arrival之间没有变化，也不能把 snapshot
  升级成 current；
- Project View / Document / Meeting 的 Relay-signed projection hint 只负责触发 typed canonical refresh；
- refresh 后 fingerprint 与 baseline 不同才设置 `sources=change_observed`，lookback replay若未改变 canonical
  observation不能误报；
- graph-external Context lens 无法从 All substrate形成 baseline；其 source family 出现经验证的新 observation时，
  保守设置 `sources=change_observed`；
- transport uncertainty 只设置独立 `transport=uncertain`，不能覆盖 topology 轴；
- stale overlay 改用静态虚线 / warning 状态，不再把非路径对象强 dim；
- 提供显式 Re-run；
- 不自动再次调用 Provider。

Fingerprint 只覆盖 source identity、typed revision / event observation、lifecycle 与当前 canonical title / summary
的本地 digest；不持久化、不进日志，也不用于重新评分。交付前 `liveSync.ts` 尚未覆盖 Meeting；Phase D4 已增加
独立但共用同一 scheduler的 Meeting hint subscription：只查询 Relay-signed kind `39000`，`authors=[relay_self]`，
并用 `#d`精确限制当前 All graph与 submitted Initial / Context中的 Meeting IDs。IDs canonical排序后按最多
64个一组拆 filter；每组使用 `since=subscription_start-5s`、`limit=256`，全部 subscription ready 后再执行一次
complete trusted All Context refresh关闭 snapshot-to-subscription race。Topology / submitted input变化时原子
重建这些 scoped subscriptions。

Kind `42113` Meeting summary command是 author-signed，不能错误塞进 `authors=[relay_self]` 的 projection
filter。Kind `39000` side effect本身 best-effort且只作为 refresh hint；无关 39000风暴不能因 global limit挤掉
相关 Meeting hint。收到 hint后仍先完成 verified Meeting / All Context reread，不能把任意 raw Event直接当成
canonical change。即便 hint缺失，UI也始终只称 snapshot，不声称 source current。

### 6.5 图 topology 更新

当 All Context 从 revision `R` 推进到 `R+1`：

- render-time 同步 gate先让 `visibleOverlay=null`，再呈现 `R+1` canonical graph；
- active session保留，`freshness.topology=advanced`，overlay处于 suspended 状态，便于用户 Re-run 或 Cancel；
- 不把 `R` 的 Edge key 集直接套到 `R+1` 图；
- 不允许出现一帧“新图 + 旧 overlay”；
- 不保存旧 graph snapshot 来冒充当前 canonical graph。

这个边界必须在 render / memo 派生时同步执行，不能依靠随后运行的 `useEffect`：

```text
visibleOverlay =
  active != null
  && active.community/workspace-token/caller/relay/project == current trusted boundary
  && active.projectContextRevision == canvas.contextRevision
  && atomicStructuralJoin(active, canvas) == valid
    ? active.overlay
    : null
```

Overlay memo identity 必须显式包含 Community、workspace token、caller、Relay、Project、request id、result revision、substrate revision
以及 complete Edge / binding fingerprint。现有不含 Context revision 的 `projectContextResultIdentity()` 与不含
Document binding 的 layout topology fingerprint都不能作为这个安全 gate。

这属于数据边界失效，不是普通点击取消，因此不违背“selection 不清除高亮”的产品要求。

## 7. Desktop 展示 DTO

### 7.1 Native Input

```text
SemanticProjectContextQueryInput
├── community_key: String
├── applied_workspace_token: String
├── problem: String
├── initial_coordinates: ProjectContextCoordinateDto[]
└── context_coordinates: ProjectContextCoordinateDto[]
```

Native 不接受：

- project id；
- request id；
- Relay signer；
- caller pubkey；
- lifecycle filter；
- query budget；
- raw filter；
- Provider 配置。

这些值由 trusted Native 状态和 frozen defaults 生成；首版固定
`lifecycle_filter=all_current`。

### 7.2 Native Output

Native 不向 TS 透传 raw Event 或完整 `SemanticGraphQueryResult`。建议闭合展示 DTO：

```text
SemanticProjectContextQueryResult
├── community_key
├── applied_workspace_token
├── caller_pubkey
├── request_id
├── project_id
├── relay_pubkey
├── project_context_revision
├── snapshot_observed_at
├── completion_reason
├── exhausted_dimensions[]
├── coverage
├── input_outcomes
│   ├── initial[]  { coordinate_key, state, reason? }
│   └── context[]  { coordinate_key, state, reason? }
├── roots[]
│   ├── root_id
│   ├── coordinate_entrypoints[]
│   └── context_document_entrypoints[] { edge_key, document_id }
└── paths[]
    ├── path_id
    ├── root_id
    ├── branch_stop_reason
    └── hops[]
        ├── ordinal
        ├── edge_key
        ├── complete_coordinate_keys[]
        ├── current_context_document_ids[]
        ├── entered_from_coordinate_key?
        ├── selected_context_document_id
        └── continued_to_coordinate_key
```

Coverage 只映射 UI 需要的闭合计数：

- roots / paths returned；
- authorized / current-indexed sources；
- title-only count；
- omitted initial / context；
- partial index coverage；
- completion / exhausted dimensions；
- response-budget omissions。

不得返回：

- raw vectors；
- Provider transcript；
- exact authenticated body；
- NIP-98 Event；
- semantic unit text；
- preview 作为 canonical fallback。

### 7.3 Root 与 zero-hop

Root 可能是：

- Coordinate entrypoint；
- Context Document 在其 bound Edge 上的 entrypoint；
- 同一 source 的两个结构角色。

即使 `paths=[]`，返回 roots 仍可能是有效的 zero-hop candidate。UI 可以显示 `N candidate roots, 0 paths`，
并用较轻的 `Semantic root` marker 标记可映射的 Coordinate / Edge；不得把 zero-hop root 称为完整 path。

## 8. Overlay 纯模型

### 8.1 派生结构

新增纯函数模块 `semanticOverlay.ts`：

```text
SemanticOverlay
├── requestId
├── projectContextRevision
├── pathCount
├── rootCount
├── edgeKeys: Set<EdgeKey>
├── rootEdgeKeys: Set<EdgeKey>
├── memberCoordinateKeys: Set<CoordinateKey>
├── routeCoordinateKeys: Set<CoordinateKey>
├── rootCoordinateKeys: Set<CoordinateKey>
├── terminalCoordinateKeys: Set<CoordinateKey>
├── relationDocumentIdsByEdge: Map<EdgeKey, Set<DocumentId>>
├── rootRelationDocumentIdsByEdge: Map<EdgeKey, Set<DocumentId>>
└── boundsTargetIds[]
```

函数输入是 verified display DTO 与 verified All Context graph；输出前执行 §6.2 的原子结构 join。
Coordinate zero-hop root必须先验证它存在于同 revision All substrate，再进入 `rootCoordinateKeys`。
Context Document zero-hop root必须独立验证其
`edge_key` 存在且 `document_id` 属于当前完整 binding set，再进入 `rootEdgeKeys` 与
`rootRelationDocumentIdsByEdge`；不能因为 `paths=[]` 就丢掉它的视觉落点。

### 8.2 Hyperedge 完整性

对每个 returned hop：

1. 将 `edge_key` 加入 overlay；
2. 将 `complete_coordinate_keys` 全部加入 member set；
3. 该 Edge Hub 与全部 Spokes 都属于 overlay；
4. entered / continued Coordinate 加入 route set；
5. 所有 entered / continued Coordinate 都加入 route marker；
6. 只有每条 path 最后一 hop 的 continued Coordinate 加入 terminal marker；
7. selected Document 加入 `relationDocumentIdsByEdge`。

不能只高亮 entered / continued 两根 Spoke。那会把一个多元 Hyperedge 视觉化为虚构的二元关系。

为兼顾路径可解释性：

- 完整 Hyperedge 成员使用中等 semantic emphasis；
- actual root / entered / continued Coordinate 使用更强 marker；
- Edge Hub 使用强 semantic outline；
- 所有 Spokes 保持可见，且不画箭头。

### 8.3 多路径并集

首版不增加 path chooser。所有 returned paths：

- 按 identity 做集合并集；
- 不按路径次数加粗；
- 不把重叠次数解释为更高置信度；
- 状态条明确 `N paths shown as one highlighted subgraph`；
- path 排序仍保留在 DTO 中，供后续 UI 使用，但不改变首版视觉权重。

### 8.4 多文档 Edge

一个 Edge 仍只显示一个 Hub。点击高亮 Edge 后，现有 Edge Inspector：

- 继续列出全部 current Context Documents；
- 仍允许逐个展开 summary；
- 仍提供 `Open in Documents`；
- 只在实际参与某条 returned path 的 Document 上增加 `Semantic path relation` badge；
- 其他当前 binding 不隐藏、不降格、不被误说成无关。

Badge 来源只能是 `relationDocumentIdsByEdge`，不能根据 summary 文本在前端重新猜测。

## 9. 视觉状态组合

### 9.1 拆分两个 emphasis 轴

保留现有：

```text
selectionEmphasis = normal | active | dimmed
```

新增：

```text
semanticEmphasis = none | outside | member | route
```

Coordinate / Hub / Spoke 分别输出：

- `data-emphasis`；
- `data-semantic-emphasis`；
- root / terminal marker；
- request id 不进入 DOM；
- hover 仍只写 `data-hover-emphasis`。

### 9.2 组合规则

| Semantic | Selection / Hover | 结果 |
|---|---|---|
| none | 任意 | 完全保持当前视觉 |
| route | dimmed / none | 路径保持最强 semantic accent |
| member | dimmed / none | 完整 Hyperedge 成员保持中等 accent |
| outside | active | 被选择对象完整可见，但没有 semantic outline |
| route / member | active | 同时显示 semantic 与 selection 两种反馈 |
| outside | dimmed | 降低透明度 |

CSS 必须按“两个轴合成”计算，不能靠 selector 顺序碰巧覆盖。Selection active 或 hover active 可以临时提升
非路径对象可见度，但不能移除其他路径元素的 semantic accent。

### 9.3 颜色与形状

- semantic 使用固定 query accent，不复用 Island hue；
- selection 继续使用 Island hue；
- Coordinate 使用静态外轮廓 / marker；
- Hub 使用 semantic outline；
- Spoke 使用更明确的静态 stroke；
- stale 使用虚线 / warning badge，而非继续使用普通 snapshot accent；
- 不使用流动线、箭头、脉冲或位置表达方向、因果、排名或置信度；
- `prefers-reduced-motion` 下结果完全一致，只移除过渡。

### 9.4 Hover

当前 hover 仅在没有 selection 时运行。Semantic mode 中：

- hover 仍可以临时检查 Coordinate / Edge incidence；
- hover 不能改写 semantic data 属性；
- semantic request / overlay identity 改变时清除遗留 hover DOM 属性；
- `hoverResetKey` 加入 overlay generation，而不是 problem 明文。

### 9.5 可访问性

- Query Bar 有明确 `<label>`、Initial / Context fieldset 和字节错误；
- Cmd / Ctrl + Enter 有可见提示；
- loading / success / partial / stale / error 使用 `aria-live` 的短文本；
- Node / Hub accessible description 增加 `In semantic result`、`Semantic root` 或 `Semantic path member`；
- 不只靠颜色区分路径；
- Cancel 是真实 button，并有稳定焦点；
- selection 的 `aria-pressed` 语义不被 semantic overlay 篡改；
- Fit paths 不改变 Tab 顺序；
- text zoom 继续使用 rem token，不新增任意 px / rem text size。

## 10. Native / Tauri 信任边界

### 10.1 依赖与文件

`desktop/src-tauri/Cargo.toml` 增加直接路径依赖：

```toml
buzz_semantic_query_pkg = { package = "buzz-semantic-query", path = "../../crates/buzz-semantic-query" }
```

不增加第三方依赖。现有 `buzz-core`、`buzz-project-context`、`buzz-sdk`、`nostr`、`reqwest`、`uuid`、
`serde`、`base64` 和 `sha2` 足够。

新增：

- `desktop/src-tauri/src/commands/project_context/semantic.rs`；
- 可选 `desktop/src-tauri/src/commands/project_context/semantic_model.rs`。

避免继续膨胀现有 `commands/project_context.rs`。父模块负责 re-export，`lib.rs` 注册
`query_project_context_semantic`。

### 10.2 Capability

在 `commands/project_view/identity.rs` 解析：

```text
buzz-project-context-semantic-query-http
```

给 `ProjectViewIdentity` 增 `semantic_query_http_available`。该字段表示 Relay 当前 readiness 全部通过后的
动态广告，因此命名使用 `available`，不暗示静态支持。

普通 `ProjectContextQueryResult.context` 增独立的 `semanticQueryAvailable`，供 UI 展示；不能复用当前
`capabilityEnabled`，后者属于 Project Context Edge capability。

Semantic command 每次仍重新读取 NIP-11 并 fail closed，不能信任页面里可能过期的 boolean。

### 10.3 Captured request boundary

当前 `AppState.keys` 与 `relay_url_override` 是两个独立 Mutex，`apply_workspace` 也分别更新它们。Semantic
query 不能直接连续读取这两个值，否则 Community 切换中点可能得到 `Relay A + Keys B` 的 hybrid snapshot。

Phase D1 必须先增加一个 Desktop-native workspace transition boundary：

```text
WorkspaceTransitionState
├── opaque_community_key
├── opaque_applied_workspace_token
├── normalized_relay_origin
├── caller / signing identity
└── signing eligibility: ready | keyring_locked | identity_lost | reset_failed
```

- `apply_workspace` 输入增加当前 opaque `community_key`；
- relay、keys、applied community key、fresh token与signing eligibility在同一个 transition lock内发布；
- `apply_workspace` 返回 `{communityKey, appliedWorkspaceToken, callerPubkey}`，前端把它放入当前
  Community-scoped context；
- identity import / replacement 也使用同一 transition lock；
- semantic command先在该 lock内比较 input `community_key + applied_workspace_token`，再原子 clone完整 tuple；
- tuple clone 后释放 lock，后续任何网络操作只使用该 pinned snapshot；
- 不允许从独立 Mutex重新拼接 relay / caller；
- locked / lost / reset-failed identity没有 signing资格，必须在任何签名或HTTP前失败；
- workspace token只用于本地竞态拒绝与 response acceptance，不进入 Relay wire或日志。

顺序合同固定为：如果 workspace B 先完成发布，旧 A invoke在任何 HTTP 前因 key / epoch mismatch失败；
如果 A query先取得完整 snapshot，则它只能向 pinned Relay A、使用 captured caller A发送，绝不能被中途切换
重定向到 B。前端发起 workspace / identity mutation时先同步递增 acceptance generation、使旧 attempt失效；
mutation成功后安装新的 applied token并触发既有 reinit / remount边界。

Native 在任何网络 await 前原子捕获：

- `community_key`；
- applied workspace token；
- relay HTTP origin；
- current Human Keys / pubkey；
- submitted closed query。

随后只在 pinned Relay 上 await NIP-11 / identity reads，解析 expected Relay `self` 与 verified Project View v3
project id。TS 不提供 project id、caller 或 Relay key。Native 生成 UUIDv4 request id。

Community 或同 Community identity在请求中途切换不能把请求重定向到新 Relay / caller；response 必须 echo
captured `community_key + applied_workspace_token + caller_pubkey`。TS 只有在它们与当前 applied identity、
reinitKey、acceptance generation和attempt token全部 exact匹配时才进入 pairing。

### 10.4 Canonical filter 与 exact body

为避免 Carryforth 与 Desktop 重复协议形状，将 semantic HTTP filter / exact request serializer 抽到
`buzz-sdk::semantic_graph` 的 public documented helper，并让 Carryforth 与 Desktop 共同使用。

唯一 filter 只能包含：

```text
kinds: [40912]
authors: [relay_self]
#p: [human_pubkey]
limit: 1
buzz_project_context_semantic: <canonical SemanticGraphQuery>
```

流程：

1. Native 构造并 `validate_and_canonicalize()`；
2. exact serialize `[filter]` 一次；
3. 该 bytes 同时作为 HTTP body 与 NIP-98 payload hash 输入；
4. 签名后不再重序列化替换 body；
5. 保留 NIP-98 Event id 与 exact bytes，供 SDK verifier；
6. 不把 bytes 或 Event 暴露给 TS。

现有 NIP-98 helper 抽成 observed variant：

```text
{ authorization_header, auth_event_id }
```

原有只需 header 的调用委托该实现，避免签名逻辑分叉。

### 10.5 One-shot transport

Semantic query：

- 只执行一次 `POST /query`；
- 不进入普通 query retry；
- 不自动重放 429 / 502 / 503 / 504 / timeout；
- 总 timeout 45 秒；
- 先执行 Desktop 本地 admission wait，再生成 fresh NIP-98；
- 不跟随 redirect；
- success Content-Length 与 chunk stream 上限使用 request `max_response_bytes`；
- non-2xx body 独立上限 16 KiB；
- 超上限立即停止读取；
- 429 可以更新 Desktop rate-limit gate，但不会自动 retry。

成功 response：

1. 解析成 JSON array；
2. 严格要求恰好一个元素；
3. 反序列化 `nostr::Event`；
4. 要求 Event 序列化值与原始 JSON value canonical 相等；
5. 调用 request-aware SDK verifier；
6. verifier 成功后才映射 Desktop DTO。

### 10.6 SDK 验证

调用：

```text
parse_semantic_graph_query_result(
  event,
  expected_relay_self,
  SemanticGraphHttpRequestObservation {
    project_id,
    authenticated_caller,
    request,
    nip98_auth_event_id,
    exact_authenticated_body,
  }
)
```

SDK 已负责验证：

- Schnorr Event；
- Relay signer；
- caller `p` tag；
- kind / tag exactness；
- request binding；
- project / request / budget；
- closed content；
- root / path / Hyperedge 完整性；
- lifecycle；
- score recomputation；
- coverage accounting。

任何 verifier error 都映射为 `verification_failed`，不产生部分 DTO 或部分 overlay。

### 10.7 错误闭集

Desktop semantic error shape 与普通 Project Context 保持同类字段，但使用独立闭集：

| Code | 典型来源 | retryable | UI |
|---|---|---:|---|
| `invalid_input` | blank / NUL / >16KiB / invalid coordinate / Relay 400 | false | 修正输入，旧 verified overlay不变 |
| `unsupported` | NIP-11 未广告 | false | observed capability off，清 active |
| `restricted` | 401 / 403 | false | 清 overlay，不缓存 preview |
| `busy` | 429 | true | 显示有界 retry hint，不自动 retry |
| `conflict` | 409 generation / context changed | true | refresh substrate；结构 gate决定旧 overlay是否暂停 |
| `timeout` | local 45s / 504 | true | Run again |
| `too_large` | 413 / body cap | false | 不激活新结果，旧 verified overlay不变 |
| `unavailable` | connect / 500 / 502 / 503 / readiness | true | 保留旧 overlay，人工 retry |
| `verification_failed` | malformed / signer / binding / invariant | false | fail closed，清 active |
| `internal` | signing / serialization impossible state | false | 清 active；diagnostic id不含原文 |

`retryable=true` 只表示按钮可用，不授权自动 Provider replay。

## 11. TypeScript API 与页面编排

### 11.1 独立 API 文件

新增：

```text
desktop/src/shared/api/tauriProjectContextSemantic.ts
```

职责：

- closed TS input / result / error types；
- coordinate canonicalization；
- UTF-8 problem validation；
- Tauri invoke；
- response `communityKey` / applied workspace token / caller / project / Relay identity 基础回显检查；
- `TauriInvokeError` 到 closed semantic error 映射。

不要把 semantic DTO 塞进现有普通 `tauriProjectContext.ts`，也不要把 result 放 React Query cache。

### 11.2 One-shot 页面编排

首版不新增只包一层的 `useProjectContextSemanticQuery()`。`ProjectContextScreen` 直接调用
`queryProjectContextSemantic()`，并由独立 reducer / token helper 实现 one-shot mutation semantics：

- `retry: false`；
- mutation object 本身不作为不稳定 callback dependency；
- 暴露稳定 `run` / `cancel`；
- local token 处理 A / B 乱序；
- Community key、applied workspace token、caller与 reinitKey进入 response acceptance，不进入明文
  problem cache key；
- failure 是否保留旧 active 由 reducer 决定，而不是 React Query 默认状态决定。

### 11.3 All Context 按需读取

当 `attempt ∈ {running, pairing}` 或 active 非空时，按需启用 All Context query：

- 若当前 applied query 已是 All，复用同一 React Query cache key；
- 若当前是 focused query，后台读取 `contains_all([])`；
- success 前不替换 canvas；
- verified semantic result 与 All snapshot 完成 §6 pairing 后一次性激活；
- active 期间 canvas / Header / counts / Inspector 都使用同一 All substrate；
- Cancel 后回到原 applied result。

保留现有 Project Context projection subscription，另按 §6.4增加 scoped Meeting metadata subscriptions；它们
共用一个 invalidation scheduler与同一 complete trusted refresh，不形成第二套状态系统。Hint只调度 refresh；
刷新后的 All Context source fingerprint与 activation baseline不一致时才标记 `sources=change_observed`。不把
lookback replay或未验证 Event直接当成 change。

NIP-11 capability 没有实时 push。Desktop 只能在 query command、窗口重新 focus、Relay reconnect或显式
refresh时重新观察。文案与测试必须写“observed capability loss”：重新观察为 off 时立即清 active；未观察到
更新时只保留 snapshot语义，不能声称 capability仍 current。

## 12. Component 改动面

### 12.1 新增文件

- `ui/ProjectContextSemanticQueryBar.tsx`；
- `semanticQueryModel.ts`；
- `semanticOverlay.ts`；
- `semanticSession.ts` 或 reducer；
- 对应 `.test.mjs`；
- `tauriProjectContextSemantic.ts`；
- Native `project_context/semantic.rs`；
- E2E mock DTO / fixture。

### 12.2 修改文件

`ProjectContextScreen.tsx`：

- 持有 draft / attempt / active / freshness；
- 选择 canvas result；
- 配对 All Context；
- capability / error / stale status；
- 传递 overlay；
- Community reset。

`ProjectContextGraph.tsx`：

- 接收 semantic overlay；
- composition / hover reset；
- Fit paths；
- 语义 legend 与 screen-reader summary；
- Node / Edge / Spoke / pane 的 selection event contract不变；substrate replacement仍执行 target存在性校验。

`presentation.ts`：

- 保留 selection emphasis；
- 新增 semantic emphasis；
- 不修改稳定 node / edge id；
- 不修改 layout input。

`ProjectContextCoordinateNode.tsx`、`ProjectContextEdgeHub.tsx`、`ProjectContextSpoke.tsx`：

- 输出 semantic data 属性；
- root / member accessible description；
- 不改变 click target。

`ProjectContextEdgeInspector.tsx`：

- 给 selected relation Documents 增 badge；
- Context Document 展开 summary / Open in Documents 行为不变。

`project-context-graph.css`：

- 两轴组合；
- light / dark；
- stale；
- reduced motion；
- 现有无 semantic 状态视觉不回归。

`ProjectContextQueryBar.tsx`：

- 抽取 Coordinate Picker；
- semantic active 时禁用结构 Run 并显示 guidance；
- 不合并两种 query DTO。

`commands/project_view/identity.rs`、`commands/project_context/model.rs`、
`desktop/src-tauri/src/lib.rs`、`desktop/src/testing/e2eBridge.ts`：

- capability、DTO、command 注册与 mock。

`app_state.rs`、`commands/workspace.rs`、Desktop `applyCommunity` / `useCommunityInit` 与 identity mutation paths：

- 增加原子 applied-workspace transition boundary；
- `apply_workspace` 发布 opaque Community key / epoch / relay / caller tuple；
- semantic capture禁止混合读取独立 relay / keys state。

`liveSync.ts` 与对应 verified refresh wiring：

- 增 Relay-signed Meeting source hints；
- hint只触发 typed refresh；
- fingerprint未变化的 lookback replay不标 stale。

本计划还会修改两个共享 Rust consumer 面，但不改变 Relay / DB wire：

- `crates/buzz-sdk/src/semantic_graph.rs`：提供 documented canonical filter / exact-body serializer；
- `crates/carryforth-cli` semantic query transport：改用同一 helper，并保持现有 one-shot / binding行为。

因此 Phase D1 必须同时跑 `buzz-sdk` 与 Carryforth semantic query回归；这项共享 helper重构不属于
Desktop-only 文件改动。

### 12.3 明确不改

- `graph.ts` 的 incidence model；
- `radialLayout.ts` / `layout.ts`；
- `routeState.ts` 的结构 query 与 selection wire；
- Project Context Edge / Document / Meeting 协议；
- Relay semantic query Request / Result；
- canonical source title / summary owner。

## 13. 分阶段开发计划

### Phase D0：冻结 Desktop 合同（已交付）

交付：

- 本文确认的 input / display DTO；
- structural join 规则；
- snapshot alignment / stale 文案；
- error closed set；
- state transition table；
- overlay union / Hyperedge 完整性；
- fixed `all_current` lifecycle 与 default budget；
- privacy / persistence 边界。

退出门：

- 没有 V1 / V2 或 parallel parser；
- 同一 Coordinate 跨 Initial / Context 合法；
- selection 不隐式进入 query；
- graph revision 与 source currentness 明确区分；
- 语义 result 不被称为 canonical content。

### Phase D1：Native trusted query（已交付）

交付：

- direct `buzz-semantic-query` dependency；
- NIP-11 semantic capability；
- verified Project identity capture；
- shared SDK filter serializer；
- observed NIP-98；
- atomic applied-workspace capture；
- one-shot bounded transport；
- SDK request-aware verifier；
- closed result mapper / error mapper；
- Tauri command 注册。

退出门：

- capability off 时零 `/query`；
- wrong signer / caller / project / request / binding 全部 fail closed；
- 429 / 5xx / timeout 每次只发送一次；
- success / error body 有硬上限；
- problem 不出现在 Debug / log；
- raw Event / exact body 不越过 Native；
- apply / query 并发不产生 mixed relay / caller，也不向错误 Community发送。

### Phase D2：TS contract、draft 与 mock（已交付）

交付：

- TS closed API；
- UTF-8 validator；
- extracted Coordinate Picker；
- Initial / Context chips；
- reducer / token race；
- E2E bridge semantic command；
- success / error / delay / sequence fixtures。

退出门：

- 16 / 8 上限与组内去重；
- 跨组相同 Coordinate 保留；
- Cancel / B supersedes A；
- 仅 transient failure保留 prior overlay；restricted / verification / observed capability loss清除；
- Community / applied workspace token / caller mismatch response rejected；
- URL / cache key 不含 problem。

### Phase D3：Semantic Query Bar（已交付）

交付：

- problem input；
- Options / picker；
- Find paths / Cancel / Re-run；
- loading / zero result / partial / exhausted / omitted / errors；
- active snapshot banner；
- capability unavailable state。

退出门：

- problem-only query 完整可用；
- full Initial / Context query 完整可用；
- 编辑 draft 不改变 active highlight；
- 普通 selection 不改变 draft；
- 当前 selection 不被隐式提交；
- keyboard 与 screen reader 文案通过。

### Phase D4：All Context pairing（已交付）

交付：

- semantic-mode All Context on-demand query；
- exact revision pairing；
- complete Edge / Coordinate / binding join；
- focused graph 与 All substrate 切换；
- source / topology stale detector；
- stale / rerun / cancel；
- 结构 Query Bar 互锁。

退出门：

- result `R` 只与 graph `R` 激活；
- graph `< R` 只 refetch graph；
- graph `> R` 不自动重放 Provider；
- topology 更新前先撤 overlay；
- render-time revision / complete-set gate在首个新 substrate render即返回 null；
- no synthetic / partial path；
- Cancel 恢复原 route query result；
- substrate切换时 selection只在 target仍存在时保留。

### Phase D5：Graph overlay 与 Inspector（已交付）

交付：

- pure overlay mapper；
- selection / semantic 双轴 presentation；
- semantic Node / Hub / Spoke style；
- root / route / member marker；
- Fit paths；
- Edge Inspector relation Document badge；
- a11y / reduced motion / text zoom。

退出门：

- 点击 / 取消节点、Edge、Spoke、pane、Escape 都不清 semantic overlay；
- 完整 Hyperedge 全部成员与 Spokes 可见；
- overlap Edge 不被错误高亮；
- selected relation Document 标记准确；
- 无 semantic session 时 current screenshots / presentation contract 不回归。

### Phase D6：集成、真实 Relay 与灰度资格（进行中；local single-pod canary已完成）

当前证据边界：

- D0–D5 已由 `507790180 feat(desktop): add project context semantic paths` 提交；
- 本地隔离 PostgreSQL / pgvector 合同、10,000 source synthetic exact-kernel benchmark以及light / dark
  语义路径截图已经形成可复现证据；
- local single-pod在受控feature窗口内完成9项真实Volcengine查询与Desktop Native全链路验签，随后完成
  `query-disable`、fleet revoke、zero Provider reservation及canonical Incident read回归；
- 真实查询的四类返回分数均无provisional floor违规，但known-negative仍返回6个roots / 12条paths，
  relevance / floor质量校准未通过；source / revision stale smoke仍待补；
- production LB inventory、multi-pod fleet / Provider contention与长soak没有部署证据，不能由本地canary或
  synthetic benchmark替代。

资格记录见：
[Project Context Desktop 图语义查询资格报告](./project-context-semantic-query-desktop-qualification.md)。

交付：

- Native / TS / E2E 全矩阵；
- light / dark 语义路径截图；
- actual semantic-enabled test Community smoke；
- revision / source stale smoke；
- Desktop runbook；
- feature-off / rollback 验证。

退出门：

- 上游语义 query 的真实 Volcengine、目标 PG、fleet 和 Community qualification 已满足；
- NIP-11 不广告时 Desktop 不发送 problem；
- revoke / ban先在 Relay线性化时零 Provider egress且不释放 result；
- 真实 query result 能在 verified All graph 高亮；
- canonical Inspector 不读取 semantic preview；
- Community switch本地立即清；Relay拒绝当前请求，或 Desktop重新观察到 revoke / 403 / capability off /
  trusted identity change后立即清；未观察期间只保留明确 snapshot且不宣称 current；
- Desktop 全部质量门通过。

已完成截图证据：

| Theme | 路径 | SHA-256 |
|---|---|---|
| light | `desktop/test-results/semantic-d6/project-context-semantic-light.png` | `fe9634ab7f81e06dc27dd0e690a8633bbd04cbac76e038e2b17685fc6950103b` |
| dark | `desktop/test-results/semantic-d6/project-context-semantic-dark.png` | `bd505532b29de929a15980796a9811463e0aa246664739a2c88e5ba3e59ab8ea` |

两张图覆盖真实Desktop渲染的语义root、route Edge与terminal Coordinate，并已在截图前等待动画结束；
它们不代表production Relay、Provider、LB或multi-pod资格已经通过。semantic-only / semantic+selection的
交互状态仍由E2E合同覆盖；如发布验收要求四种状态各有独立视觉基线，须另行补图。

## 14. 测试矩阵

### 14.1 Pure model

输入：

- blank / whitespace；
- NUL；
- 中文 UTF-8 16 KiB 边界；
- Initial 16 / 17；
- Context 8 / 9；
- 组内 duplicates；
- 同 Coordinate 跨组；
- canonical ordering；
- Native fixed `all_current` wire default。

Overlay：

- 单路径；
- 多路径并集；
- 重叠路径；
- 三元及更大 Hyperedge；
- relation Document root；
- zero-hop root；
- missing Coordinate structural root使整份 overlay fail closed；
- zero-hop Context Document root的 Edge / Document marker；
- selected Document map；
- missing Edge；
- Coordinate set 少 / 多；
- binding set 少 / 多；
- selected Document 非成员；
- entered / continued 非成员；
- deterministic output；
- 两跳路径中只有最后一 hop的 continued Coordinate是 terminal；
- 不 mutate input。

Reducer：

- idle → running → active；
- draft edit keeps active；
- B success atomically replaces A；
- Native success先进入 pairing，不提前替换 active；
- 首个查询 active=null且 All Context尚未完成时，pairing仍持续启用 substrate读取；
- pairing graph `<R`继续等待、`==R`原子激活、`>R`失败且旧active仍受render gate约束；
- transient B failure keeps A；
- Cancel ignores late A；
- B starts before A resolves；
- Cancel / newer Run / Community reset使 pairing candidate失效；
- Community / reinit reset；
- applied workspace token / caller mismatch拒绝迟到 result；
- restricted / verification failure clears；
- observed capability loss clears；
- source + transport stale可同时存在；
- topology advanced + transport uncertain不会恢复 overlay；
- success / Cancel / Community reset显式重置 freshness。

Live source observation：

- 现有 Relay projection filter不被 Meeting `#d`条件污染；
- author-signed kind `42113`不会被误期待为 `authors=relay_self`；
- 相关 Meeting kind `39000`触发 typed refresh；
- 无关 kind `39000`风暴不命中 scoped filters；
- 65+ Meeting IDs稳定拆分且不遗漏；
- hint后 fingerprint不变不标 stale；
- graph-external Meeting lens的新 verified observation保守标 stale。

### 14.2 Native Rust

1. NIP-11 extension on / off；
2. canonical Relay self；
3. project id 只来自 verified source；
4. full problem / initial / context request，Native固定 `all_current`；
5. exact outer filter key set；
6. 无 `schema_version`；
7. observed NIP-98 method / URL / payload / Event id；
8. exact body byte preservation；
9. one-shot 429 / 503 / timeout；
10. success Content-Length over cap；
11. chunked success over cap；
12. error body 16 KiB cap；
13. zero / two Event；
14. unknown outer Event field；
15. wrong Relay signer；
16. wrong caller `p`；
17. wrong Project / request id / request binding；
18. changed exact body / auth Event；
19. malformed result / budget violation / Hyperedge violation；
20. Community switch while request is in flight；
21. barrier race：apply B先发布时旧A零HTTP；A先capture时只发送到A；apply中点无hybrid relay/caller；
22. 同 Community / Relay / Project中 caller A→B，旧token response不得进入pairing；
23. keyring-locked / identity-lost / reset-failed均零签名、零HTTP；
24. ordered Desktop DTO mapper；
25. no raw preview / vector / exact body in DTO；
26. redacted Debug；
27. shared serializer下 buzz-sdk / Carryforth exact-body与binding回归。

### 14.3 Presentation unit

扩展 `presentation.test.mjs`，覆盖 semantic × selection：

| Semantic | Selection | 断言 |
|---|---|---|
| none | none | current normal |
| none | active | current selection contract |
| route | none | route full accent |
| member | dimmed | member remains visible |
| outside | active | selected outside item visible，无 semantic marker |
| route | active | 两种 marker 同时存在 |
| outside | dimmed | lowest opacity class |

再覆盖：

- Edge Hub + all Spokes；
- complete Coordinate set；
- overlap Edge 不误亮；
- hover 不移除 semantic；
- overlay generation 清 stale hover；
- query anchor 与 semantic root marker 并存；
- tombstone / unavailable 状态不被 semantic style 覆盖；
- 1000 Edge 完整性与有界映射。

### 14.4 Playwright E2E

1. problem-only success；
2. Initial / Context 完整输入；
3. 同一 Coordinate 同时在两组；
4. invoke payload 不含 project / request id；
5. loading 时旧 overlay 保持；
6. success 原子替换；
7. transient failure 保留旧 overlay；
8. Cancel 后 late response 不复活；
9. A / B response 乱序；
10. semantic result 在 All Context graph 显示；
11. focused route Cancel 后恢复；
12. Coordinate click 后 overlay 保持；
13. Edge / Spoke click 后 overlay 保持；
14. pane click 后 overlay 保持；
15. Escape / Inspector close 后 overlay 保持；
16. path 外 selection 可见而 path 仍亮；
17. 进入 All substrate时，selection目标存在则保留、不存在则按现有逻辑清除；
18. Cancel恢复 focused substrate时，同样覆盖 target存在 / 不存在两支；
19. complete Hyperedge 全部 Coordinate / Spoke；
20. 多文档 Edge 只给 selected relation Documents badge；
21. Context Document 展开 summary 与 Open in Documents；
22. zero paths / Coordinate root；
23. zero-hop Context Document root标记正确 Hub与Document；
24. 两跳路径中间 Coordinate不是 terminal；
25. partial coverage / budget exhausted / omitted context；
26. capability off / busy / conflict / timeout / restricted / verification failure；
27. transient 503保留旧 snapshot；restricted / verification / observed capability off清除；
28. canonical All Context refresh 403立即清除，不走 stale-preserve；
29. revision `< / == / >`；
30. 不发 stale reducer signal，直接把 substrate `R` 替换为 `R+1`，首个 render即无 overlay；
31. binding-only revision变化同样同步暂停 overlay；
32. source + transport stale可同时显示，且 topology advanced时不恢复 overlay；
33. Project View / Document / Meeting summary变化经 verified refresh后标 stale；
34. lookback replay但 canonical fingerprint未变时不标 stale；
35. graph-external Context lens的 source family出现新 verified observation时保守标 stale；
36. source在 Stage C到HTTP arrival之间变化时，UI仍只称 snapshot，绝不显示 current；
37. Community switch或同 Community caller/token变化后无旧 problem / result / highlight；
38. URL 无 semantic input；
39. keyboard / aria-live / node Enter / Space；
40. reduced motion / Desktop text zoom；
41. Fit paths 只自动发生一次；
42. light / dark / semantic-only / semantic+selection screenshots。

所有 screenshot 前使用 `waitForAnimations(page)`，并检查 SHA-256 distinctness。

## 15. Privacy、权限与诚实性

### 15.1 Problem 出域

必须区分两个边界：

1. Desktop → selected Relay：只有用户显式点击 Find paths、pinned Relay的 NIP-11动态广告 capability、
   pinned caller可签名且 Project identity已验证后，Desktop才把 problem发送给该 Relay；
2. Relay → external Provider：Relay继续按照上游 Stage B final egress permit与fleet/current auth fence决定是否
   把 problem / overview发送给火山引擎；Desktop不复制、弱化或声称替代该线性化边界。

Desktop无法在 HTTP body到达 Relay前原子证明 Relay端 membership / ban / query gate仍 current。诚实合同是：
capability-off preflight时 Desktop零 `/query`；若撤权先在 Relay线性化，Relay必须零 Provider egress且不释放
result。不能宣称客户端会消除 authorization check之前的 body arrival。

Desktop 落地不会扩大已经确认的 problem / title / summary出域授权，也不授权正文 chunk。

### 15.2 结果不是事实

语义结果只能表示：

> 在该 snapshot、模型和预算下，这些路径被检索引擎选中。

它不表示：

- Edge 方向；
- 因果关系；
- 重要性；
- 唯一正确路径；
- 未高亮内容不相关；
- selected relation Document 是 Edge 唯一解释；
- score 是概率或 confidence。

### 15.3 Canonical Inspector

Node / Edge / Document Inspector 继续使用 verified canonical reads：

- Project View current object；
- Project Document current revision；
- Meeting verified snapshot；
- Project Context current Edge / binding。

`SemanticSourcePreview` 不得作为 missing current source 的 fallback。Source tombstone、delete 或 restricted 后，
不能保留旧 preview 供 UI 展示。

### 15.4 权限变化

- query release 时授权由 Relay 保证；
- Desktop 不把该结果当持续授权 lease；
- 观察到 restricted / 403 / Community boundary change、trusted identity mismatch或 NIP-11 capability off时清 active；
- capability没有 push，因此只承诺在 command / focus / reconnect / explicit refresh重新观察后清除，不宣称
  未观察期间仍 current；
- All Context refresh的 401 / 403属于安全失效，立即清 active；普通 5xx / disconnect只设置
  `transport=uncertain`并保留已验证 snapshot；
- 当前 action 仍重新鉴权；
- 不宣称已经显示到 Human 屏幕的内容可以被远程追回；
- 不把 cached semantic preview留作权限失效后的内容来源。

## 16. 性能边界

- semantic response 已受 256 KiB 硬上限；
- returned paths 最多 64，每 path 最多 6 hops；
- overlay mapper 只遍历 returned roots / hops / complete sets；
- 集合使用 stable string key；
- 不在每个 React Flow render 中重复解析 raw DTO；
- `SemanticOverlay` 在显式 safe identity变化时 memo：Community / workspace token / caller / Relay / Project /
  request / result revision /
  substrate revision / complete Edge-and-binding fingerprint全部进入 key；
- title / summary 更新不触发布局 topology 重算；
- overlay 只改变 presentation data，不运行第二套 layout；
- 不增加持续动画；
- 不为每条 path 克隆一份 React Flow element；
- 多路径使用一个 union overlay；
- All Context可复用现有 query cache与 layout geometry；overlay安全 gate不能复用缺少 Context revision或
  Document binding的现有 identity / topology fingerprint。

如果 overlay mapping 在目标最大图上成为 long task，先 profile pure mapper；不得用截断 canonical Hyperedge
来换取速度。

## 17. Rollout 与回滚

### 17.1 发布顺序

1. 上游 semantic Foundation / Query 保持 feature-off 完成资格；
2. Relay / DB 版本先部署；
3. Desktop consumer 可在 capability-off 状态发布；
4. 测试 Community 建 generation、完成索引与 query readiness；
5. 实际 HTTP fleet attestation；
6. operator 显式 query-enable；
7. Desktop NIP-11 看见 capability 后开放 Find paths；
8. 单 Community smoke 与 UI E2E；
9. 再扩大 Community。

### 17.2 Desktop 回滚

Desktop 侧回滚只需要：

- 隐藏 / 移除 Semantic Query Bar；
- 不调用 Tauri semantic command；
- semantic overlay 为空；
- 普通结构查询与图交互保持原样。

不需要：

- down migration；
- 删除 semantic index；
- 修改 Project Context；
- 清理 URL；
- 重写 source summary。

Operator 可先 `semantic query-disable`，新旧 Desktop 都会因 NIP-11 不广告而 fail closed。

## 18. 风险与收口

### 18.1 两套高亮互相覆盖

风险：复用 `emphasis` 使 selection / pane click 清除路径。

收口：两个独立 data 属性与完整组合矩阵；semantic state 不进入 route selection reducer。

### 18.2 Focused graph 缺路径

风险：只高亮当前子图交集。

收口：semantic mode 使用 verified All Context substrate；结构 query route 保留，Cancel 恢复。

### 18.3 Hyperedge 被画成二元路径

风险：只高亮 entered / continued Spokes。

收口：完整 Edge Hub、完整 Coordinate set、全部 Spokes都进入 member overlay；actual route 只增加 marker。

### 18.4 旧路径覆盖新图

风险：Context revision 更新后稳定 key 仍能匹配部分元素。

收口：render-time revision / complete-set gate同步撤 overlay；不做 partial join；要求用户显式 Re-run。

### 18.5 Graph revision 相等但 summary 已变

风险：把旧 score/path称为 current。

收口：明确 snapshot semantics；显示 observed time；verified refresh确认 fingerprint变化后标 stale；Inspector只读
current canonical content。

### 18.6 Cancel 后迟到响应复活

风险：Tauri invoke 无协议取消。

收口：Community-bound monotonic token；Cancel / newer Run / remount 使旧 response无资格落 state。

### 18.7 Problem 泄漏

风险：放进 URL、React Query key、Debug、日志或 screenshot fixture。

收口：只存在内存；redacted Debug；测试检查 route / storage / logs；E2E 使用非敏感 fixture。

### 18.8 Semantic preview 冒充 canonical content

风险：为避免二次读取，直接渲染 result preview。

收口：Native display DTO不输出 preview fallback；现有 canonical Inspector保持唯一内容展示面。

## 19. 明确禁止的反例

以下实现不接受：

1. 把 semantic result 写进 route search；
2. 把 problem 写进 React Query key；
3. 当前选中节点自动成为 Initial / Context；
4. 用 selection `emphasis` 保存 semantic path；
5. pane click 或 Escape 清除 semantic overlay；
6. TS 直接 `fetch('/query')` 并信任 JSON；
7. Desktop 调用 `cf` 子进程；
8. 对 429 / timeout 自动 retry；
9. raw Event / NIP-98 exact body越过 Tauri；
10. 在当前 Incident 子图只展示路径交集；
11. result revision `R` 覆盖 graph revision `R+1`；
12. 只高亮 Hyperedge 的两根 Spoke；
13. 把 Context Document画成隐式 Node；
14. 多 path 重叠次数改变线宽并暗示 confidence；
15. 用 semantic preview补当前 canonical title / summary；
16. summary 缺失推断为 irrelevant；
17. 未高亮内容被 UI 标成“不相关”；
18. capability off 时仍尝试发送 problem；
19. 新查询失败时无条件清除旧 overlay；
20. Cancel 无条件清除一个在恢复 substrate中仍存在的 Inspector selection；
21. source update静默自动重放 Provider query；
22. 为 Desktop 增加另一个 SemanticGraphQueryV1 / V2 wire。

## 20. 最终验收不变量

交付完成必须同时满足：

1. Problem 必填，Initial / Context 均可选；
2. problem-only query 可完整工作；
3. 同一 Coordinate 可同时作为 initial root 与 context lens；
4. 当前 selection 不会隐式修改查询；
5. Semantic overlay 与 route selection操作完全正交；substrate切换时只保留仍存在的 selection target；
6. Node / Edge / Spoke / pane / Escape / Inspector 操作不清 overlay；
7. 普通用户交互只有 Cancel 清路径；Community / Project / topology /权限 / capability / trusted identity /完整性
   变化按已定义安全规则清除或暂停 overlay；Cancel本身不额外清仍存在于目标 substrate的 selection；
8. Community boundary本地立即 fail closed；revoke /权限 / capability / trusted identity变化在 Relay拒绝或
   Desktop重新观察后立即 fail closed，未观察期间只保留明确 snapshot；
9. 迟到 response不能复活已取消或被替换的 query；
10. 语义路径只覆盖 verified All Context graph；
11. revision与完整 Hyperedge / binding set必须精确匹配；
12. 不产生 synthetic topology；
13. 每个经过的 Hyperedge按完整成员展示；
14. zero-hop Context Document root能标记其真实 Edge与Document；
15. 多文档 Edge只标记实际 selected relation Documents，但不隐藏其他 binding；
16. 多 path 首版以一个 union subgraph展示；
17. selection与semantic视觉可同时识别；
18. stale snapshot不冒充 current；
19. canonical source内容永远来自现有 verified reads；
20. problem / raw result不进入 URL、storage、日志或持久 cache；
21. Native applied workspace tuple原子捕获，不产生 mixed Relay / caller；
22. Native 完成 exact-body NIP-98 与 SDK request-aware verification；
23. transport one-shot、bounded、no automatic retry；
24. observed capability off时零问题出域；
25. 不修改 Relay / DB query协议；
26. 不修改图布局、Edge identity或普通结构 query wire；
27. Desktop全部 unit、Native、Playwright、a11y、light/dark与真实灰度门通过。
