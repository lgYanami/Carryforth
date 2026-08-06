# Project Context Desktop 分阶段实现计划

> 状态：实施中；阶段一、阶段二已提交，阶段三已交付，待 Human 确认。
>
> 产品规格：[Project Context Desktop 产品规格](./desktop-spec.md)。
>
> 领域语义：[Project Context Edge V0 领域规范](./project-context.md)。
>
> 已交付后端基线：[Project Context Edge V0 后端实现设计](./implementation-design.md)。
>
> 本文只规划 Buzz Desktop 交付，包括 Desktop Tauri/Rust 可信读取适配、React 前端、图形
> 呈现、Desktop mock bridge 与 Desktop 测试。Relay、数据库、领域协议、SDK 基础、CLI、
> ACP、Web 和 Mobile 均视为已经交付或不在本计划范围内。
>
> 本文中的“阶段”只表示有依赖关系的开发与 review 顺序，不表示分阶段发布。当前系统没有
> 上线与迁移负担，本计划不包含 rollout、灰度、双写、兼容发布或回滚方案。

## 1. 计划目的

Project Context 后端与 Agent surface 已经交付，但 Desktop 当前还没有新 Context Edge 的
读取或呈现能力：

- Desktop 只识别旧 `buzz-project-context-v1` Context Reference capability，不识别独立的
  `buzz-project-context-edge-v1`；
- `desktop/src-tauri` 没有 Context meta / binding 的可信查询边界；
- TypeScript 没有 Context Coordinate、Edge、查询结果或结构化错误模型；
- `/project-context` 路由、侧栏入口与 deep link 不存在；
- 当前 `ProjectViewMap` 是 Project View 一阶层级的 DOM 卡片树，不是关系图引擎；
- Desktop 没有 graph viewport、Edge Hub、超边、Context Island 或确定性布局；
- 现有 Inspector 与 Document Viewer 包含各自完整维护行为，不能直接作为 Context 的紧凑
  只读 Inspector；
- live invalidation、Community 切换、mock bridge 与 E2E 尚不知道 Context Edge kinds 和
  查询状态。

本计划把 Desktop 交付拆成七个严格有依赖关系的阶段：

```text
阶段一：可信 Context 查询边界
    ↓
阶段二：页面纵向链路与状态外壳
    ↓
阶段三：完整图、超边与 Context Islands
    ↓
阶段四：三类查询、URL 状态与跨页面深链
    ↓
阶段五：Coordinate / Edge Inspector 与按需正文
    ↓
阶段六：实时恢复、视觉收口、响应式与可访问性
    ↓
阶段七：E2E、真实数据验收与质量收口
```

最终目标不是让 React 复制 Context 协议，而是让 Human 通过 Desktop 观察同一份经过验证的
Project Context，并在需要时进入现有 Project View / Documents 或 Agent 能力继续维护。

## 2. Desktop-only 范围

### 2.1 本计划包含

- `desktop/src-tauri` 中 Context Edge capability、查询、分页一致性、验签、聚合和 hydration；
- Context query 的 Desktop Rust DTO、结构化错误与 TypeScript API；
- 当前 Project 的 Context query hooks、可信刷新和 Community 隔离；
- `/project-context` 路由、侧栏入口、导航 helper 与可恢复 URL 状态；
- Project View Object / Project Document Coordinate picker；
- Coordinate Node、Edge Hub、Spoke、Context Island 与查询感知布局；
- Coordinate / Edge 只读 Inspector 与 Document Markdown 按需读取；
- Project View / Documents 与 Project Context 的双向导航；
- loading、empty、unsupported、restricted、unavailable、stale 与 verification failure；
- Desktop 单元测试、Tauri 测试、mock bridge、Playwright E2E、截图和一次真实 Relay 穿行。

这里的 `desktop/src-tauri` 是 Desktop 自身的 native trust boundary，属于 Desktop 实现，不是
新增 Relay 后端。

### 2.2 本计划不包含

- Relay handler、数据库 migration、projection、kind、wire schema 或权限修改；
- `buzz-project-context` 领域语义修改；
- `buzz-cli` 或 `buzz-acp` 修改；
- Desktop attach / detach command 或写入 UI；
- Desktop 内创建、更新或删除 Context Document；
- Desktop 内编辑 Project View 对象；
- 持久化节点位置、岛名称、岛颜色或岛身份；
- 自动推断 Context Edge、Gap、过期、冲突、完整性或“应该连接”的岛；
- Context Reference 与 Context Edge 的迁移或同步；
- Web 或 Mobile 实现；
- 上线、数据迁移、分阶段发布与 release gate。

实现中若发现已交付后端契约不足，应记录准确阻断并单独讨论后端修正，不得在 Desktop 中
通过 raw event、宽松解析、本地猜测或旁路 HTTP API 补出新的权威语义。

### 2.3 Preview 与 runtime capability

Project Context 页面复用现有 `projectView` Desktop preview feature，因为它属于同一个
Project Space，并依赖 Project View v3 与 Project Document v1。首版不新增仅用于发布的
preview flag。

preview 只控制 Desktop 入口是否公开；runtime capability 与 verified Context meta 决定当前
Project 实际能否读取：

- `buzz-project-context-edge-v1` 与旧 `buzz-project-context-v1` 必须独立识别；
- extension 可用时正常读取；
- extension 关闭但仍存在同 signer / generation 的 verified Context meta 时继续只读；
- extension 关闭且不存在 verified meta 时返回 unavailable；
- Project View 非 v3 或 Project Document 未就绪时返回 unsupported；
- preview 或 capability 都不能改变 Relay 中已有 Context 状态。

## 3. 现有 Desktop 实现映射

| 现有能力 | 当前主要位置 | Project Context 使用方式 |
|---|---|---|
| Project View identity / snapshot | `desktop/src-tauri/src/commands/project_view.rs`、`project_view/identity.rs`、`project_view/v3.rs` | 扩展独立 Context Edge capability；复用 verified v3 snapshot 做 Coordinate hydration |
| Project Document verified boundary | `desktop/src-tauri/src/commands/project_document.rs`、`project_document/model.rs` | 复用 community race fence、structured error、meta/head 验证和按需正文读取 |
| Context domain / SDK | `crates/buzz-project-context`、`buzz-sdk::project_context` | 规范化 Coordinate、派生 Edge key、严格解析 / observation 验证和 Edge 聚合 |
| Agent 查询参考 | `crates/buzz-cli/src/commands/project_context.rs` | 作为 Desktop 查询算法的行为基线；不得 shell out 到 CLI |
| Tauri command 注册 | `desktop/src-tauri/src/commands/mod.rs`、`desktop/src-tauri/src/lib.rs` | 注册只读 `query_project_context` |
| Tauri TypeScript API | `desktop/src/shared/api/tauriProjectView.ts`、`tauriProjectDocument.ts` | 增加独立 `tauriProjectContext.ts`，不把 Edge 塞进 Project View DTO |
| React Query / live signal | `features/project-view/hooks.ts`、`features/project-documents/hooks.ts` 与两个 `liveSync.ts` | Community-scoped query、projection event 只作 invalidation、重新进入 native 验证 |
| Project Space 路由 | `app/routes.ts`、`app/routes/view.tsx`、`app/routes/documents.tsx` | 新增 lazy `/project-context` route 与 closed search state |
| App 导航 | `app/navigation/useAppNavigation.ts`、`AppShell.helpers.ts`、`AppShell.tsx` | 新增 `goProjectContext`、selected view 与导航编排 |
| Project Space 侧栏 | `features/sidebar/ui/AppSidebarPinnedHeader.tsx` | 在 Overview 与 Documents 之间增加 Project Context |
| Project View 内容 | `features/project-view/model.ts`、`ProjectViewInspector.tsx` | 复用标题、类型、状态、actor 与关系格式化，不嵌入维护型 Inspector |
| Document 内容 | `features/project-documents/hooks.ts` 与 Markdown primitives | Edge / Document Coordinate 选择后按需读取当前正文，不嵌入完整编辑器 |
| 右侧面板 | `shared/layout/AuxiliaryPanel*`、现有 responsive Sheet 模式 | 承载 Coordinate / Edge 只读 Inspector |
| Relay reconnect invalidation | `shared/api/relayQueryInvalidation.ts` | 将 Context query root 纳入 reconnect / auto-heal |
| Community reset | `features/communities/useCommunityInit.ts` | React 生命周期内隔离；若新增 module singleton 必须提供 reset |
| Desktop mock / E2E | `desktop/src/testing/e2eBridge.ts`、`desktop/tests/e2e`、`playwright.config.ts` | 增加 verified query seed、source content、错误、live 与截图场景 |

`ProjectViewMap` 虽然名为 Map，但只表达 Project View 的一阶规范层级，不提供 viewport、图布局
或通用 Edge。Project Context 不应扩展它，而应建立独立 `features/project-context` 边界。

## 4. 总体实现结论

### 4.1 一个高层只读 Tauri command

首版只新增一个业务读取入口：

```text
query_project_context({
  communityKey,
  query
}) -> ProjectContextQueryResult
```

`query` 使用 closed union：

```text
exact        { coordinates[] }
incident     { coordinate }
contains_all { coordinates[] }
```

Coordinate 同样使用 closed union：

```text
project_view_object { objectType, objectId }
document            { documentId }
```

不新增单独的 `get_project_context_meta`：页面默认就是 `contains-all({})`，一次 verified query
已经能够返回 meta、catalog counts、结果 Edge 与轻量 hydration。若实现阶段发现有不依赖完整
查询的真实产品需求，再单独评审，而不是预先增加接口。

Tauri command 不接受 raw Nostr filter、event JSON、tag 或调用方计算的 Edge key。所有输入都
重新规范化并验证 Project scope。

### 4.2 Native trust boundary 与查询算法

新增 `desktop/src-tauri/src/commands/project_context.rs` 及相邻 model / tests。Command 必须：

1. 在第一次 await 前捕获 `communityKey`、Relay URL 与 signing identity，避免 Community 切换
   把进行中的读取重定向；
2. 从 NIP-11 验证 Relay `self`，要求 Project View v3 与 Project Document v1；
3. 独立读取 `buzz-project-context-edge-v1`，但不因 extension 关闭直接拒绝已有 verified meta；
4. 读取 Context meta `M1`；
5. 按 query 选择精确 filter 并分页到空页；
6. 读取 Context meta `M2`；
7. 只有 M1 / M2 event ID、Context Revision 与 Generation 全部相同时接受，否则完整重试；
8. 对分页结果按 event ID 去重，严格验证 signer、Project、generation、tags、Edge key 与
   observation；
9. 使用 SDK 的 `aggregate_project_context_edges` 聚合 binding，不在 Desktop 另写宽松 reducer；
10. 再应用并验证 `exact`、`incident`、`contains-all` 的结果约束；
11. 完整 catalog 查询校验 signed `active_edge_count` 与 `bound_document_count`；
12. 分别在独立 verified Project View / Document observation 中批量 hydration；
13. 返回 body-free、稳定排序的 Desktop read model。

查询行为与已交付 CLI 保持一致：

- `exact` 使用 Edge query coordinate，只能得到 0 / 1 Edge；
- `incident` 使用一个 canonical Coordinate tag；
- 非空 `contains-all` 使用 canonical 第一坐标作为 anchor，再做严格 subset filter；
- 空 `contains-all` 分页读取全部 active binding；
- page size、到空页、重复事件、无进展、meta 变化与重试耗尽全部 fail closed；
- Project View / Document hydration 的瞬时不可用保留 Edge 并标记 unavailable；
- verified Context Document tombstone 或身份矛盾属于 verification failure，不能降级
  unavailable。

CLI command crate 不能作为 Desktop 依赖。实现复用 `buzz-project-context` 与 `buzz-sdk` 已公开的
纯领域 / 验证函数；transport orchestration 可以在 Tauri 适配层实现，并用 parity tests 固定与
CLI 相同的可观察行为。`desktop/src-tauri/Cargo.toml` 应以
`buzz_project_context_pkg = { package = "buzz-project-context", path = "../../crates/buzz-project-context" }`
显式声明直接依赖，保持
与现有 `buzz_project_view_pkg`、`buzz_project_document_pkg` 和 `buzz_sdk_pkg` 命名一致。

### 4.3 Capability 命名必须拆开

当前 Desktop 的 `PROJECT_CONTEXT_EXTENSION` 与 `project_context_supported` 实际表示旧
Context Reference。实现时：

- 将 Rust 内部命名明确为 `PROJECT_CONTEXT_REFERENCE_EXTENSION` / reference support；
- 保留现有 Project View serialized 字段与 TypeScript `contextCapability` 行为，避免旧 UI 回归；
- 新增独立 `PROJECT_CONTEXT_EDGE_EXTENSION` 与内部 `project_context_edge_enabled`；
- Context query result 单独返回 capability 当前是否 enabled；
- 不用旧 boolean 门控新页面、查询或 deep link；
- 增加测试证明两个 extension 可分别开关且互不冒充。

### 4.4 Desktop read model 保持领域形状

Tauri 输出建议规范化为：

```text
ProjectContextQueryResult
├── communityKey
├── projectId
├── relayPubkey
├── context
│   ├── contextRevision
│   ├── projectionGeneration
│   ├── activeEdgeCount
│   ├── boundDocumentCount
│   ├── updatedAt
│   ├── metaEventId
│   └── capabilityEnabled
├── query
├── projectViewObservation
├── documentObservation
├── edges[]
│   ├── edgeKey
│   ├── coordinates[]
│   └── contextDocumentIds[]
├── coordinateDetails[]
└── documentDetails[]
```

具体 Rust / TypeScript 字段在阶段一固化，但必须满足：

- DTO 不包含 graph node、position、island、palette 或 viewport；
- 相同 Coordinate 只在 `coordinateDetails` 中 hydration 一次；
- Edge 只引用 canonical Coordinate 与 Context Document ID；
- Document metadata 统一去重，但“作为 Coordinate”与“作为 Context Document”的结构角色由
  Edge 字段分别表达；
- Coordinate detail 区分 `active | tombstoned | unavailable`；
- Context Document detail 区分 `active | unavailable`；verified tombstone 不跨边界返回；
- observation 分别报告 Project Revision / Document Catalog Revision 与各自 Generation；
- observation 携带足以启动无丢失 live refresh 的 verified 更新时间；
- 不返回 Markdown body、raw event、raw tag、Relay response body 或内部数据库信息；
- Edge、Coordinate 和 Document 顺序稳定。

### 4.5 复用现有来源读取，不制造混合状态

Context native query hydration 需要读取 tombstone，因此不能只复用当前 TypeScript active
Project View / Documents 列表。

Rust 侧优先提取现有命令内部可复用的 crate-private helper：

- Project View：取得包含 active / tombstone entry 的 verified v3 snapshot，再只 hydration
  请求坐标；现有 `get_project_view` public result 继续只返回 active View；
- Documents：在 Document meta 双读边界内按 ID chunk 读取 current heads，返回 active /
  tombstone metadata；现有 Document command contract 不变；
- 如果提取 helper 会扩大不必要 public API，可保留在 `commands` 内 `pub(crate)`；
- 不通过 TypeScript 把两个 active catalog 与 Context Edge 拼成“可信”结果。

React 仍使用现有 Project View 与 Document hooks：

- 为 Coordinate picker 提供所有 active 候选；
- 为 active Project View Coordinate Inspector 提供当前完整对象内容；
- 为选中的 Document Coordinate / Context Document 按需读取当前 Markdown；
- tombstone / unavailable 内容使用 Context query detail，不假装 active；
- 三个来源 Revision 独立显示，不承诺跨领域原子 snapshot。

### 4.6 Structured error 与 fail-closed UI

Context Tauri error 对齐 Project Document pattern，使用 body-free structured payload：

```text
unsupported
restricted
unavailable
snapshot_conflict
invalid_input
verification_failed
internal
```

错误至少包含 `code`、安全 message、retryable、可选 HTTP status / retry-after。规则为：

- query 无匹配是成功的空结果，不是 not-found error；
- 403 映射 restricted；
- connect / timeout / rate-limit / service unavailable 映射 retryable unavailable；
- M1 / M2 重试耗尽映射 retryable snapshot conflict；
- malformed / signer / Project / generation / binding contradiction 映射
  `verification_failed`；
- 错误不包含 Relay 私有响应正文或 URL token；
- React 对 `verification_failed` 不展示局部图；
- 同一 query 的刷新错误可以保留上一份 verified 图并标 stale；
- 新 query 失败不能继续展示旧 query 图。

### 4.7 React Query 与 live invalidation

新增独立 `features/project-context/hooks.ts` 与 `liveSync.ts`。Query key 至少包含：

```text
project-context
  + Community reinit key
  + Relay origin
  + canonical query descriptor
```

规则为：

- 默认 descriptor 是 `contains-all({})`；
- descriptor 在进入 query key 前使用与领域一致的 canonical order；
- Community 或 Relay boundary 改变后不能复用旧 graph result；
- query 变化不使用 previous query 作为 placeholder；
- 同一 query live refresh 可以保留 previous verified data；
- signer / generation 变化后清理同 Community 的旧 business result；
- `project-context` 加入 Relay reconnect / auto-heal invalidation root；
- 不建立 module-level Context graph cache。

live filter 只把 Relay-authored projection event 当失效信号，并覆盖：

- Context binding / meta（`40908` / `40909`）；
- Project View object / meta（`40903` / `40904`）；
- Project Document head / meta（`40905` / `40907`）。

Context command `44302` 不订阅；任何权威变化最终都必须以 Relay projection 的重新读取为准。

这样 Context topology、Coordinate 标题 / 状态和 Document metadata / body 的独立变化都会
触发可信重读。订阅建立后立即 invalidate 一次，关闭 snapshot→subscription race；burst 使用
debounce / trailing refresh 合并。raw live payload 永远不直接 patch graph。

### 4.8 Incidence graph presentation adapter

领域 DTO 与图渲染类型之间增加纯 presentation adapter：

```text
verified ProjectContextQueryResult
    ↓
domain graph index
    ├── Coordinate nodes
    ├── Edge Hub presentation nodes
    └── Spokes
    ↓
connected components / query anchors / layout
    ↓
graph renderer nodes + edges
```

稳定 presentation IDs 建议为：

```text
coordinate:<canonical-coordinate-token>
context-edge:<edge-key>
spoke:<edge-key>:<canonical-coordinate-token>
island:<derived-component-key>
```

必须保持：

- Coordinate 才是真实领域节点；
- Edge Hub 与 Island container 都是 presentation node；
- 每条领域 Edge 只对应一个 Hub；
- 每个 Hub 到其所有 Coordinate 各有一条 Spoke；
- 所有 Spoke 点击都选择同一完整 Edge；
- `{A,B}` 与 `{A,B,C}` 产生两个 Hub；
- Context Document binding 不生成 Coordinate node；
- 同一 Document 明确作为 Coordinate 时才生成一个 Document Coordinate node；
- 查询无结果的 Anchor 可以生成 placeholder node，但不生成 Edge 或 Island；
- 适配过程只做结构派生，不重新执行领域 query filter。

该 adapter、component discovery 和 layout 都应是无 React、无 DOM 的纯函数，以便穷尽测试。

### 4.9 Context Island 与确定性布局

完整 `All Context` 结果使用无向 incidence graph 的 connected components 派生 Islands。算法
保持线性或近线性，并遵守：

- shared Coordinate 连接 Edge；
- Context Document binding 不连接 Island；
- tombstoned / unavailable Coordinate 仍按稳定 ID 连接；
- 无 Edge Query Anchor 不形成 Island；
- Island key / number / color 都不是领域身份。

首版不增加 D3、ELK 或 Dagre。布局作为纯确定性函数实现：

- Exact：唯一 Hub 居中，Coordinate 稳定环绕或分层；
- Incident：Anchor 层、matching Hub 层、其余 Coordinate 层；
- 非空 Contains all：所有 Anchor 固定为共同查询层，matching Hub 与额外 Coordinate 分层；
- All Context：先独立布局每个 connected component，再根据 bounds 与稳定排序打包 Islands；
- 相同 graph + query 必须得到相同 position / bounds；
- 不运行持续力导向模拟；
- renderer 先取得节点的实际尺寸，再把只读 size map 传给纯 layout 函数；layout 本身不读取
  DOM；
- 节点尺寸、label 截断与 collision spacing 使用集中 token；root text zoom 导致尺寸变化时通过
  `ResizeObserver` 重新布局，并用幂等 guard 避免循环；
- Inspector 开关和 viewport 变化只改变 fit / focus，不改变同一图的布局；
- Island 背景使用由 component bounds 加 padding 得到的非交互 presentation container；
- Island palette 由稳定 component seed 映射到有限的 light / dark theme tokens；
- topology 不变时颜色与顺序稳定；merge / split 后允许重算；
- 颜色之外始终保留 Island number、边界与事实 counts。

如果实现阶段的真实图 fixture 暴露明显交叉或 label collision，优先改进同一个纯 layout
函数；不通过允许用户拖动并持久化位置来掩盖算法问题。

### 4.10 图形依赖边界

Desktop 当前没有 graph viewport。计划只新增 `@xyflow/react`：

- 负责 pan、zoom、fit view、custom node / edge rendering 与基础 keyboard / ARIA；
- 不负责领域 query、island discovery 或 layout；
- 页面 route 保持 lazy load，避免图依赖进入不打开 Context 时的首屏路径；
- Coordinate、Edge Hub 和 Island container 使用 Buzz 自己的 semantic tokens；
- `nodesDraggable=false`、`nodesConnectable=false`；
- 不开放 delete、reconnect、lasso mutation 或 whiteboard 行为；
- Spoke 不显示箭头；
- keyboard 的主要 Edge 操作目标是 focusable Hub，Spoke 提供足够 pointer hit target；
- built-in controls 若不能满足 Buzz 视觉 / accessibility，则只复用 viewport API 并使用现有
  Button primitives 实现控制条。

依赖通过 Desktop package manifest 与根 `pnpm-lock.yaml` 正常加入，不复制 vendor 源码。

### 4.11 URL 状态与 query draft

`/project-context` route 使用 closed search schema，语义上包含：

```text
mode         exact | incident | contains_all | omitted-for-all
coordinates canonical Coordinate tokens
selected    coordinate:<token> | edge:<edge-key> | omitted
```

具体数组编码由 route serializer 固定并测试。规则为：

- search 省略 query 时规范化为 `All Context`；
- `contains_all` 空集合规范化为相同的 All 状态；
- malformed mode、Coordinate token、重复坐标或非法 selection 在调用 native 前拒绝；
- query coordinates 在 URL 中使用 canonical stable order；
- picker 修改本地 draft，只有 `Run` 才提交 route search 并触发 query；
- selection 更新 URL 但不改变 query；
- selection 不再属于新 result 时使用 replace 清除；
- pan、zoom、Fit Island 与当前 Edge 内 Document tab 不写入 URL；
- Community 切换 replace 为目标 Project 的默认 All 状态。

canonical Coordinate token 只用于 UI identity / deep link；调用 Tauri 前必须解析成 typed union，
Rust 再次验证，不能让字符串 token 进入原始 Relay filter。

### 4.12 Inspector 复用边界

Project Context 新增紧凑只读 Inspector，不直接嵌入：

- 完整 `ProjectViewInspector` 的 edit / delete / Role governance / Context References；
- 完整 `DocumentViewer` 的 edit / delete / history / diff。

推荐复用：

- Project View `model.ts` 的 title、description、status、priority 与 relation formatter；
- `ProjectViewActor` 与 profile lookup；
- Project Document verified query identity 与 `useProjectDocument`；
- 现有 Markdown renderer；
- `AuxiliaryPanel` / responsive Sheet、header、body 与 focus-return patterns。

如果提取 Project View read-only detail primitive 能在不重构 mutation 路径的前提下复用，阶段
五可以提取；否则先在 `features/project-context` 内实现小型 read-only detail，并复用格式化
helper，避免为了本功能重写整个 Project View Inspector。

## 5. 建议目录与代码边界

具体文件可以在阶段 review 时微调，但职责边界建议为：

```text
desktop/src-tauri/src/commands/
├── project_context.rs
└── project_context/
    ├── model.rs
    └── tests.rs 或相邻 test module

desktop/src/shared/api/
└── tauriProjectContext.ts

desktop/src/features/project-context/
├── hooks.ts
├── liveSync.ts
├── coordinate.ts
├── graphModel.ts
├── connectedComponents.ts
├── layout.ts
├── projectContext*.test.mjs
└── ui/
    ├── ProjectContextScreen.tsx
    ├── ProjectContextQueryBar.tsx
    ├── ProjectContextGraph.tsx
    ├── ProjectContextCoordinateNode.tsx
    ├── ProjectContextEdgeHub.tsx
    ├── ProjectContextIsland.tsx
    ├── ProjectContextInspector.tsx
    ├── ProjectContextCoordinateInspector.tsx
    ├── ProjectContextEdgeInspector.tsx
    └── ProjectContextStates.tsx

desktop/src/app/routes/
└── project-context.tsx

desktop/tests/e2e/
└── project-context.spec.ts
```

页面、graph、Inspector 与 query control 必须按现有 file-size guard 保持小而独立；不得把全部
功能堆入 `AppShell.tsx`、`ProjectViewScreen.tsx` 或一个超大 `ProjectContextScreen.tsx`。

## 6. 分阶段交付

### 6.1 阶段一：可信 Context 查询边界

#### 目标

> Desktop native boundary 可以对当前 Project 执行三类 Context query，返回 body-free、完整
> 验证且可供图形客户端使用的稳定 read model。

#### 本阶段关键工作

1. **依赖与 capability**
   - Desktop Tauri 增加 `buzz_project_context_pkg` direct dependency alias；
   - 增加独立 Context Edge extension 常量和 internal identity field；
   - 保持旧 Context Reference serialized capability 不变；
   - 测试 old/new extension 独立组合。

2. **Closed DTO 与错误**
   - 固化 typed Coordinate、query union、result、observation、Edge、Coordinate detail、Document
     detail 与 structured error；
   - input 使用 camelCase + deny unknown fields；
   - Rust public API 添加 doc comments；
   - TypeScript 增加等价 domain type、safe error class、community response guard 与 query
     canonicalizer。

3. **Verified query**
   - 实现 meta 双读、分页到空、event 去重、无进展拒绝与有限重试；
   - exact `#g`、incident `#c`、contains-all anchor / empty catalog；
   - SDK strict parser、observation verifier 与 aggregate；
   - 完整 catalog signed counts 验证；
   - canonical stable output。

4. **Cross-domain hydration**
   - 提取或复用 verified Project View v3 entry snapshot；
   - Document meta 双读 + requested heads chunk；
   - active / tombstone / unavailable 区分；
   - Context Document verified tombstone contradiction fail closed；
   - 不读取 Markdown body。

5. **Command 接入**
   - 注册 `commands` module 与 Tauri invoke handler；
   - 不增加 Context write command；
   - 不增加 Relay endpoint。

#### 本阶段不做

- React route、sidebar 或 graph；
- live subscription；
- Document body；
- attach / detach。

#### 自动化与验收

- exact 0 / 1 且不返回 superset；
- incident 返回 binary + hyperedge；
- contains-all 只返回 superset，空集合返回完整 catalog；
- 同 Edge 多 binding 聚合为一条 Edge、多 Document 稳定排序；
- `{A,B}` 与 `{A,B,C}` 不合并；
- 多页、重复 event、空页、无进展、M1/M2 变化与重试耗尽；
- wrong signer / Project / generation / tags / Edge hash fail closed；
- Project View 与 Document active / tombstone / unavailable hydration；
- capability on、capability off + verified meta、capability off + no meta；
- Community 在 await 期间切换不会重定向结果；
- TypeScript canonical query key 与 structured error mapping tests。

#### 完成标志

> `query_project_context` 对三类查询提供与 CLI 可观察语义一致的 trusted DTO；TypeScript 不需要
> 理解 raw event，就能区分 Edge、Coordinate、Context Document、source observations 与错误。

### 6.2 阶段二：页面纵向链路与状态外壳

#### 目标

> Human 可以从当前 Project Space 打开 `/project-context`，页面默认取得完整 verified Context
> 结果，并准确呈现所有非图形状态。

#### 本阶段关键工作

1. **Route 与导航**
   - 在 route manifest 增加 lazy `/project-context`；
   - 定义 closed search validator 与默认 All normalization；
   - 增加 `goProjectContext`、`SidebarSelectedView` 与 AppShell path mapping；
   - 在 Overview 与 Documents 之间增加侧栏入口；
   - 复用 `projectView` FeatureGate；
   - route tree 通过既有生成流程更新，不手写不一致类型。

2. **Query hooks**
   - 建立 Community reinit key、Relay origin 与 canonical descriptor query key；
   - 默认调用 `contains-all({})`；
   - signer / generation 变化清理同 Community 旧 Context result；
   - 将 `project-context` 纳入 reconnect invalidation；
   - 手动 refresh 重新进入 native boundary。

3. **Screen shell**
   - Top chrome、Verified / capability-off read-only / revision / refresh；
   - loading / verifying；
   - unsupported / restricted / unavailable；
   - initialized empty catalog；
   - snapshot conflict / verification failure；
   - verified result counts 与 graph slot；
   - 同 query refresh 保留上一份可信 result 并标 stale。

4. **Context Reference 命名澄清**
   - Project View Inspector 现有 section 的可见标题改为 `Context References`；
   - 保持原行为、capability、test ID 与 mutation 不变。

5. **基础 mock**
   - e2e bridge 接受 Context query result / error / delay seed；
   - 能按 active Relay / Community 返回不同结果；
   - 增加 sidebar、route、empty 和 error state smoke tests。

#### 本阶段不做

- Graph renderer 与 islands；
- Query Bar 的可操作 picker；
- Coordinate / Edge Inspector；
- live event subscription。

#### 自动化与验收

- 侧栏 Project Space 顺序与 active state；
- `/project-context` lazy route 与直接打开；
- default query descriptor 等于 empty contains-all；
- unsupported、restricted、unavailable、empty、verification failure 使用不同页面；
- capability off + verified result 显示只读而不是 unsupported；
- refresh error 保留同 query verified state；
- Community A→B 时旧 counts / state 不闪现；
- 旧 Context References UI 仅改名、不改变行为。

#### 完成标志

> 页面纵向可信链路成立：Human 能稳定进入、刷新并理解 Context 是否可读；还未接入图时也不存在
> raw event、混合状态、capability 混淆或跨 Community 泄漏。

### 6.3 阶段三：完整图、超边与 Context Islands

#### 目标

> `All Context` 结果以可平移缩放、准确表达二元 Edge / 超边并具有有意义 Island 视觉的完整图
> 呈现。

#### 本阶段关键工作

1. **Graph viewport dependency**
   - 正常加入 `@xyflow/react` 与 lockfile；
   - 只在 lazy Project Context feature 中加载；
   - 导入必要 base style，再用 Buzz tokens 覆盖外观；
   - 禁止 drag、connect、delete、reconnect 与 whiteboard mutation。

2. **纯 graph adapter**
   - canonical Coordinate node / Hub / Spoke IDs；
   - deduplicate shared Coordinate；
   - one domain Edge → one Hub；
   - all Spokes → same selected Edge；
   - Context Document 不生成 node；
   - tombstone / unavailable detail 进入 node presentation；
   - exact-set overlap 保持独立 Hub。

3. **Connected components**
   - 在 All 结果上计算 Islands；
   - shared Coordinate 合并 component；
   - Context Document role 不连岛；
   - stable component key、stable current-view order 与 counts；
   - counts 区分 Coordinate、Edge、Context Document。

4. **确定性布局与 Island packing**
   - 独立布局每个 component；
   - 计算 bounds + padding；
   - 使用稳定 packing 留出岛间空白；
   - 相同 input 得到相同 positions；
   - 节点 label / badge 不互相遮挡；
   - 不运行持续 force animation。

5. **视觉节点与 Edge**
   - Project View type icon / badge；
   - Document Coordinate；
   - Edge Hub + Context Document count；
   - undirected Spoke、无箭头、足够 pointer target；
   - tombstone dashed、unavailable 独立状态；
   - selected / hover highlight 不跨错 Edge。

6. **Islands UI**
   - 派生 palette、浅色围合与边界；
   - Island label / current-view number / facts；
   - `N context islands` summary；
   - Island chips、Fit Island、Fit All；
   - 单一 Island 与多 Island 都保持可读；
   - 文案只说 disconnected components，不出现 Gap 建议。

#### 本阶段不做

- Exact / Incident / Contains all picker；
- Inspector body；
- 持久化布局；
- 自动命名或解释 Island。

#### 自动化与验收

- AB binary、ABC hyperedge、AB + ABC overlap fixtures；
- shared Coordinate 合并 Island；完全不相交图形成两个 Islands；
- Context Document 同 ID 出现在另一 Edge 的 Coordinate role 时只按 Coordinate membership
  连接；
- tombstone / unavailable 继续参与 connectivity；
- graph adapter 不生成隐式 Document node；
- deterministic layout 重复运行完全一致；
- Island bounds 不覆盖其他 Island bounds；
- Hub / Spoke click 都选择完整 Edge；
- Island color 之外存在 label、border 与 count；
- All graph screenshot 能清晰区分两个 Islands。

#### 完成标志

> Human 打开 All Context 后可以准确看到完整 verified hypergraph、重叠 Edge 与多个有意义但不被
> 过度解释的 Context Islands；图形 presentation 没有进入领域 DTO。

### 6.4 阶段四：三类查询、URL 状态与跨页面深链

#### 目标

> Human 可以通过 Query Bar 或 Project View / Document deep link 执行 exact、incident、
> contains-all，并通过 URL 恢复当前查询与选择。

#### 本阶段关键工作

1. **Query Bar**
   - `All Context | Exact | Incident | Contains all` mode switch；
   - mode-specific input constraints；
   - grouped searchable Coordinate picker；
   - Project View Object type / title / status；
   - Document title / summary；
   - canonical chips、remove、clear 与 `Run`；
   - draft 与 applied query 明确分离。

2. **Picker data**
   - 复用 Project View verified snapshot 与 Document active metadata list；
   - 一个来源 unavailable 不把已验证 Context graph 清空；
   - visible tombstoned / unavailable node 可以加入 query；
   - 重复 Coordinate 在提交前拒绝；
   - 不根据标题猜测跨 Project identity。

3. **Query route state**
   - Run 写入 canonical mode / coordinate search；
   - Back / Forward 恢复 applied query；
   - invalid route input 显示明确 validation state；
   - query result 替换时清理失效 selection；
   - selection 单独更新 search，不重新解释 query；
   - return All 规范化为空 search 或唯一 canonical form。

4. **Query-aware graph**
   - Exact 使用唯一 Hub focus layout；
   - Incident 突出一个 Anchor；
   - Contains all 突出全部 Anchors；
   - 结果 Edge 始终显示完整 Coordinate set，而不是只显示 query subset；
   - no-match 显示 Anchor + `0 matching edges`，不产生 Island / Gap；
   - focused query 不显示项目级 Island count；
   - query 变化 fit new result，pan / zoom 不进 URL。

5. **Deep links**
   - active Project View Object 增加 `Show in Project Context`；
   - active Project Document 增加相同入口；
   - 两者显式打开 `incident(coordinate)`；
   - 不因旧 Context Reference capability false 隐藏新入口；
   - deep link 不授予权限；
   - Community 不匹配时不执行旧 token。

#### 本阶段不做

- 点击 Coordinate 自动运行 Incident；
- 前端本地模拟后端 query 结果；
- 跨项目 Coordinate picker；
- Edge attach / detach。

#### 自动化与验收

- Exact order invariance、0 / 1 与 no superset；
- Incident binary + hyperedge；
- Contains all exact + supersets、single-coordinate equivalence、empty→All；
- picker 分组、键盘、重复与 mode constraints；
- draft 修改不触发 query，Run 才更新；
- Back / Forward、reload 与 copied route；
- selected Edge / Coordinate 不改变 query；
- query 变化清除失效 selection；
- no-match Anchor 不是 Island / Gap；
- View / Documents incident deep link 与返回导航。

#### 完成标志

> 三类领域查询在 Desktop 中可发现、可恢复且结果准确；Human 可以从已有项目内容进入相关
> Context，也不会因为点击节点或视觉过滤而产生隐式查询语义。

### 6.5 阶段五：Coordinate / Edge Inspector 与按需正文

#### 目标

> Human 点击 Coordinate 可以阅读当前对象内容，点击 Edge 可以阅读准确坐标集合与多份 Context
> Documents，而初始图仍保持 body-free。

#### 本阶段关键工作

1. **Inspector shell**
   - 宽屏右侧 panel、窄屏 Sheet / single panel；
   - URL selection 驱动 Coordinate / Edge content；
   - Escape / close 恢复 graph focus；
   - 打开 panel 后保持 selected node 可重新聚焦；
   - selection 不改变 applied query。

2. **Project View Coordinate**
   - 从当前 verified Project View snapshot 找 active object；
   - 展示 type、title、description、status、priority、主要直接关系、Revision、actor / time 与 ID；
   - `Open in Project View`；
   - `Show incident Context` 显式改 query；
   - 不显示 edit / delete / role governance 控件。

3. **Document Coordinate**
   - 使用 Context document observation 形成 verified Document identity；
   - 选择后才调用现有 current Document read；
   - 展示 title、summary、Revision、actor / time、Markdown 与 ID；
   - `Open in Documents` 与 `Show incident Context`；
   - 不嵌入 editor / history / diff / delete。

4. **Tombstone / unavailable Coordinate**
   - tombstone 显示 type、ID、known revision / actor / time；
   - unavailable 保留 stable identity 与当前 Edge membership；
   - 二者使用不同状态和文案；
   - tombstone 不提供 active edit navigation；
   - 不出现 Context Gap 判断。

5. **Edge Inspector**
   - Header、完整 Edge key 诊断入口；
   - canonical unordered Coordinate set；
   - 每个 Coordinate state 与定位动作；
   - Context Document count 与 stable list；
   - 默认首份 available Document；
   - tab / list 切换后按需读取一份 Markdown；
   - 不拼接多份正文、不把 Document title 当 Edge name；
   - 每份 Document `Open in Documents`。

6. **Document read freshness**
   - current body query 与 pinned Context structure 分开；
   - Document update 后刷新 metadata 与当前 body，但不重排 Edge topology；
   - source observation unavailable 时不发起无 identity body read；
   - body error 只影响 Document content area，不隐藏已验证 Edge。

#### 本阶段不做

- 在 Inspector 中编辑正文；
- Edge attach / detach；
- Document revision history；
- 根据多份正文自动摘要或命名 Edge。

#### 自动化与验收

- active Project View 各对象类型的 read-only detail；
- active Document Coordinate lazy body；
- tombstone / unavailable distinct；
- Edge 多 Document 默认选择、切换与 body lazy count；
- Document 同时承担 Coordinate / content 两种角色时 UI 分区正确；
- Spoke 点击打开完整 Edge，不只显示两个端点；
- Open in View / Documents 和返回；
- Inspector close focus restoration、responsive Sheet；
- 初始 All graph 不调用任何 Markdown body read。

#### 完成标志

> 图的两类点击闭环完整：Coordinate 内容可读，Edge 的准确范围与全部 Context Documents 可读；
> 正文仍按需验证，Project Context 页面没有获得维护型副作用。

### 6.6 阶段六：实时恢复、视觉收口、响应式与可访问性

#### 目标

> 前五阶段收敛为在实时变化、断线、Community 切换、不同窗口尺寸和键盘 / screen reader 下
> 都稳定可信的 Desktop 体验。

#### 本阶段关键工作

1. **Live refresh**
   - 增加 Context binding / meta kinds 常量；
   - 组合监听 Context、Project View 与 Document projection kinds；
   - explicit author + kinds、lookback、subscribe 后 revalidation；
   - debounce burst、running + trailing refresh；
   - raw event 不进入 graph state；
   - Edge change、Coordinate change、Document update 分别验证 UI 结果。

2. **断线与 stale**
   - 同 query 保留 last verified graph；
   - header 明确 reconnecting / stale；
   - manual retry；
   - reconnect 后整体 trusted replacement；
   - snapshot conflict 自动进入有限 query retry，耗尽后保留旧图并显示冲突；
   - verification failure 不保留为“仍可信的当前图”。

3. **Community 隔离**
   - Query、draft、selection、Document tab、viewport 和 subscriptions A→B 清理；
   - route search replace 为 B 的 All；
   - A 的 delayed native response 被 community guard 拒绝；
   - B→A 重新确认，不恢复未经确认的旧 graph；
   - 不新增 module singleton；如性能实现必须新增，则接入 `resetCommunityState()`。

4. **视觉与 motion 收口**
   - light / dark Island palette；
   - 同时可见 Islands 具有足够差异；
   - selected / hover / query anchor / tombstone / unavailable 不争用同一颜色语义；
   - Island merge / split 与 query layout transition 克制且可关闭；
   - reduced-motion 下立即布局；
   - labels、badges、Spokes 在常用 zoom 范围清晰。

5. **响应式与 viewport**
   - 宽屏 graph + resizable Inspector；
   - 窄屏 Sheet / single-panel；
   - Query Bar 在窄宽折叠但保留 mode 与 chips；
   - Fit All / Island / Selection；
   - trackpad / Magic Mouse pan、wheel zoom 与页面 scroll chaining；
   - Desktop root text zoom 与 graph viewport zoom 独立；
   - 不引入 px / arbitrary text size。

6. **Keyboard 与 accessibility**
   - Coordinate / Hub stable tab order；
   - Enter / Space inspect；
   - Escape close、focus return；
   - Island navigator、query picker 与 controls 完整 keyboard；
   - ARIA label 明确“Context Edge connecting N coordinates with M documents”；
   - 颜色之外使用 icon、text、border、number 与 status；
   - loading / refresh / error live region 不反复噪声播报。

7. **性能与规模**
   - graph adapter 与 components 为 O(nodes + spokes)；
   - memoization 绑定 immutable verified result / query，不绑定每次 render；
   - stable node / edge props，避免 hover 导致全图重建；
   - 验证 100、500、1000 Edge fixture 的 query result、component discovery、layout 与交互；
   - 使用 viewport culling / lazy inspector body；
   - full result 不静默截断；
   - 若 profiling 证明纯 layout 造成明显主线程 long task，再把同一纯函数移入 worker，不改变
     DTO 或产品语义。

#### 本阶段不做

- 自动连接 Island；
- 可拖动 / 可编辑 graph；
- 将 performance threshold 变成领域 Edge 数量上限；
- 用局部 query 替代默认完整图。

#### 自动化与验收

- live binding attach / detach 造成 Island merge / split；
- Document body update 只更新 content / metadata，不改变 topology；
- Project View title / status update 刷新 Coordinate；
- offline→stale→reconnect、snapshot conflict 与 verification failure；
- Community A→B→A delayed response / subscription isolation；
- light / dark、reduced motion、keyboard-only、screen reader labels；
- 600px 附近 Inspector panel / Sheet 行为；
- app text zoom 与 graph zoom；
- large fixtures 不截断、不产生随机 layout、不发生明显全 App rerender。

#### 完成标志

> Project Context 图在动态项目、断线、Community 切换、窄窗口、键盘和大结果下仍保持可信、
> 可读和可恢复；视觉增强始终编码结构事实而不是系统推断。

### 6.7 阶段七：E2E、真实数据验收与质量收口

#### 目标

> 用完整自动化和一次真实 Tauri / Relay 数据穿行证明 Desktop spec 已闭环，且没有回归现有
> Project View、Documents 或 Context References。

#### 本阶段关键工作

1. **Mock bridge 完整化**
   - 按 Relay / Community 与 canonical query descriptor 返回不同 verified result；
   - 支持 result sequence、delay、structured error 与 stale refresh；
   - Project View / Document source seed 与 Context observation 对齐；
   - live Context / View / Document projection signal；
   - capability on/off、旧 reference capability 独立；
   - Community delayed response isolation。

2. **Playwright smoke**
   - 新增并注册 `project-context.spec.ts`；
   - route / sidebar / deep link；
   - All / Exact / Incident / Contains all；
   - binary / hyperedge / overlapping exact sets；
   - two Islands / merge / split / Fit Island；
   - multi Document Edge 与 lazy Markdown；
   - Document 两种结构角色；
   - tombstone / unavailable / verification failure；
   - selection does not query；
   - reconnect、Community switch、responsive 与 keyboard。

3. **视觉验收截图**
   - 完整双岛图；
   - AB + ABC 重叠 Edge；
   - Incident Anchor focus；
   - Edge Inspector 多 Document；
   - tombstone / unavailable；
   - dark theme；
   - narrow Sheet；
   - 截图前等待 animation 完成，并验证各状态 hash 不重复。

4. **真实 Tauri / Relay 穿行**
   - 使用现有 CLI 创建 Project View / Documents 与至少两条不相连 Context Edge；
   - Desktop All 显示两个 Islands；
   - 创建跨岛 Edge 后刷新为一个 Island；
   - 三类 query 与 CLI 对照；
   - 更新 Context Document 正文，Edge / Context Revision 不变而 Inspector 正文更新；
   - tombstone 一个 Coordinate，Edge 保留且节点标 tombstone；
   - capability off + verified meta 仍可读；
   - Context Document 绑定删除保护不由 Desktop 绕过。

5. **现有功能回归**
   - Project View Map / Inspector / Context References；
   - Documents list / viewer / editor / history；
   - Community Overview 与工作位置恢复；
   - Relay reconnect / auto-heal；
   - Sidebar selected state 与 Desktop zoom。

6. **文档证据**
   - 更新本文阶段状态与 commit / test evidence；
   - 记录真实穿行结果与已知非阻断体验问题；
   - 不新增发布章节；
   - 后续写入 UI 继续单独设计，不在验收阶段临时加入。

#### 自动化与验收

- Project Context feature unit / native / E2E 全部通过；
- Desktop 现有 Project View / Document tests 通过；
- screenshot states 可辨识且语义正确；
- real query output 与 CLI 在相同 Context Revision 下 Edge / Coordinate / Document membership
  一致；
- 全部 spec acceptance criteria 有自动化或真实穿行证据；
- Desktop full quality gate 通过。

#### 完成标志

> Project Context Desktop spec 的所有关键场景均有证据；Human 可以稳定查询、观察 Islands、
> 打开 Coordinate / Edge 内容，并且 Desktop 没有引入 Context 写入、Gap 推断或可信边界旁路。

## 7. 阶段依赖与交付纪律

七个阶段按 `1 → 2 → 3 → 4 → 5 → 6 → 7` 推进：

- 阶段二依赖阶段一的 trusted query / errors；
- 阶段三依赖阶段二的 default All result 与 screen states；
- 阶段四依赖阶段三的 graph adapter 与 query-aware layout extension point；
- 阶段五依赖阶段四的 URL selection 与 source catalogs；
- 阶段六对前五阶段统一增加 live、recovery、a11y 与 performance，不新增领域操作；
- 阶段七只补证据与回归，不临时扩展产品范围。

每个阶段完成时必须：

1. 对照 [Desktop spec](./desktop-spec.md) review 可观察行为；
2. 对照 [领域规范](./project-context.md) review Coordinate、Edge、Document 与 query 语义；
3. 确认新 Context Edge 与旧 Context Reference capability / UI 没有混用；
4. 确认 raw event、局部分页和 mixed observations 没有进入可见 graph；
5. 运行本阶段 targeted tests 与受影响现有回归；
6. 更新阶段状态与交付证据；
7. 经 Human 确认后提交并进入下一阶段。

阶段交付不是发布。中间阶段可以只在开发分支存在，不需要设计兼容窗口、feature rollout 或
线上数据迁移。

## 8. 跨阶段关键测试矩阵

| 约束 | 主要阶段 | 自动化重点 |
|---|---:|---|
| 新旧 Context capability 独立 | 1、2、7 | 四种 extension 组合、capability-off read、旧 UI 回归 |
| Native verified query | 1、6 | signer、Project、generation、M1/M2、pagination、retry、fail closed |
| 三类集合语义 | 1、4、7 | exact 0/1、incident、contains-all、empty All、order invariance |
| Hyperedge 不拆分 | 1、3、7 | AB、ABC、AB+ABC、Hub / Spoke membership |
| Context Document 结构角色 | 1、3、5 | binding 不成 node、显式 coordinate 才成 node、single-edge membership |
| Context Islands | 3、6、7 | components、colors、counts、merge/split、局部 query 不报项目 Island |
| 无 Gap 推断 | 2、3、4、5、7 | empty、no match、two Islands、unavailable 文案 |
| Tombstone 保留 | 1、3、5、7 | topology、node state、Inspector、无 active edit navigation |
| Hydration unavailable | 1、2、5、6 | Edge 保留、identity 保留、body area 独立失败 |
| Lazy Markdown | 1、5、7 | initial body request=0、selection=1、multi-doc switching |
| Selection 不改 query | 3、4、5 | node / Hub / Spoke click、URL、Back / Forward |
| 权限不扩大 | 1、2、4、5 | restricted query、deep link、无内容泄漏 |
| Live 只作 invalidation | 2、6 | raw payload 不 patch、burst coalesce、subscribe race |
| Community 隔离 | 1、2、4、6、7 | query/draft/selection/viewport/body/delayed response A→B→A |
| Accessibility | 3、4、5、6 | keyboard、focus return、non-color status、reduced motion、zoom |
| 现有功能回归 | 2、4、5、7 | Project View、Context References、Documents、sidebar、reconnect |

## 9. 质量门禁

所有命令执行前先激活仓库 Hermit 环境：

```bash
. ./bin/activate-hermit
```

### 9.1 阶段内 targeted checks

Native 阶段至少运行：

```bash
cargo test --manifest-path desktop/src-tauri/Cargo.toml project_context
just desktop-tauri-fmt-check
just desktop-tauri-clippy
```

React / graph 阶段至少运行：

```bash
just desktop-typecheck
just desktop-check
just desktop-test
just desktop-build
```

E2E 阶段运行：

```bash
just desktop-e2e-smoke
```

### 9.2 最终 Desktop gate

```bash
just desktop-ci
```

最终准备合并时再运行仓库级 `just ci`。本计划不修改 Mobile；若本地 Mobile toolchain 成为与
本功能无关的环境阻碍，应保留准确记录并至少保证 Desktop 全量 gate 完整通过，不能因此跳过
任何 Desktop 检查。

新增或修改的 readable text 继续使用现有 rem-based token；不得通过 arbitrary px/rem text
size、file-size override、lint disable 或降低测试门槛绕过仓库规则。

## 10. 阶段状态

| 阶段 | 状态 | 主要交付证据 |
|---|---|---|
| 1. 可信 Context 查询边界 | 已提交（`e8e1b29fe`） | trusted Tauri query、typed TS API；Rust 14 + SDK 9 + Project View 回归 24；Desktop 3551 tests、typecheck、check、build、Tauri fmt/clippy 通过 |
| 2. 页面纵向链路与状态外壳 | 已提交（`f2823ed22`） | `/project-context` route、sidebar、default All query、Community/Relay cache fence、完整只读状态外壳与 Context References 命名；Desktop 3557 tests、check、typecheck/build；Project Context E2E 10 + Context References 回归 E2E 1 通过 |
| 3. 完整图、超边与 Context Islands | 已交付，待确认 | lazy `@xyflow/react` 只读 viewport、canonical graph adapter、确定性 incidence layout / Island packing、Coordinate / Hub / Spoke / Island UI、选中与 fit controls；graph / layout / presentation 纯测试 18、Desktop 3575 tests、check、production build、Project Context E2E 11 与双 Island 截图通过 |
| 4. 三类查询、URL 状态与跨页面深链 | 待实施 | — |
| 5. Coordinate / Edge Inspector 与按需正文 | 待实施 | — |
| 6. 实时恢复、视觉收口、响应式与可访问性 | 待实施 | — |
| 7. E2E、真实数据验收与质量收口 | 待实施 | — |

状态只记录开发交付，不代表发布阶段。每个阶段提交后记录 commit 与实际通过的测试，不用
“已写代码”代替完成证据。

阶段一已在 `e8e1b29fe` 提交，阶段二已在 `f2823ed22` 提交。阶段三当前尚未提交；Human
确认后补记 commit，再进入阶段四。阶段三没有增加 query picker、query URL 应用态、Inspector、
live subscription、Document body、持久化布局、Island 领域身份或 Context write surface。

## 11. 整体完成定义

Project Context Desktop 可以认为首版交付完成，需要同时满足：

1. `/project-context` 是当前 Project Space 的稳定入口，默认显示完整 verified Context 图；
2. Desktop Rust 对 exact / incident / contains-all 完成与 CLI 等价的严格读取与 hydration；
3. 新 Context Edge 与旧 Context Reference capability、UI 和状态完全分离；
4. Coordinate 是真实节点，Edge Hub / Spoke 准确表达唯一无向 binary Edge 或 hyperedge；
5. `{A,B}` 与 `{A,B,C}` 不拆分、不合并、不产生方向；
6. Context Document binding 不自动生成图节点，多 Document 保持独立并按需读取；
7. All Context 能稳定派生、分色、围合和导航 Context Islands；
8. Island 只表达 connected component，不产生 Gap、重要性、健康度或连接建议；
9. 三类 query、query draft、URL 恢复、no-match Anchor 与 deep link 行为符合 spec；
10. Coordinate / Edge Inspector 能读取所需内容，但不提供 Context write 或内联业务编辑；
11. tombstone / unavailable 保留 Edge，verified contradiction fail closed；
12. Context、Project View、Document live 变化都通过 invalidation → native revalidation；
13. 断线、snapshot conflict、Community 切换和 delayed response 不产生混合图或跨项目泄漏；
14. 宽屏、窄屏、keyboard、screen reader、light / dark、text zoom 与 reduced-motion 可用；
15. 大结果不静默截断，相同结果的 graph / layout / Island visual 稳定；
16. Project View、Context References、Documents、sidebar 与 reconnect 现有回归通过；
17. Desktop unit、Tauri、Playwright、真实 Relay 穿行与 Desktop full gate 均有完成证据；
18. 实现没有增加 Relay / DB / protocol 修改、Context write UI、发布阶段或 Mobile 工作。
