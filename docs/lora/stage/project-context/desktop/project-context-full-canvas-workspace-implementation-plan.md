# Desktop Project Context 全画布与可折叠右侧工具栏分阶段实现计划

> 状态：Phase F0–F6 纯前端实现与自动化/视觉验收通过；实现提交待回填
>
> 日期：2026-08-12
>
> 实现基线：`feat/context-desktop` @ `2e006c7dc`
>
> 当前验收记录：
> [Desktop Project Context 全画布工作区验收记录（草稿）](./project-context-full-canvas-workspace-acceptance.md)
>
> 已确认的当前证据：Desktop unit `3715 / 3715`、`pnpm typecheck`、`pnpm check`、
> `pnpm build:e2e`通过；workspace E2E `12 / 12`通过；既有Project Context spec排除一项
> 本分支既有Community baseline后`34 / 34`通过。dense default `1280×800`与`1440×900`
> 的初始`data-auto-fit-count`均为`1`，`44 / 44` Coordinate均通过HUD / Rail safe-area检查；
> 六张light / dark / Details / semantic / narrow / stale截图已人工复核且SHA-256互异。完整证据见
> 验收记录。
>
> 上游 Desktop 计划：
> [Project Context Desktop 分阶段实现计划](../desktop-implementation-plan.md)
>
> 图布局设计：
> [Desktop Project Context 有机网状图布局实现设计](../desktop-organic-graph-layout-implementation-design.md)
>
> 语义查询 Desktop 计划：
> [语义路径查询分阶段实现计划](../../semantic/desktop/project-context-semantic-query-desktop-implementation-plan.md)
>
> 图领域规范：[Project Context V2 领域规范](../project-context.md)
>
> 本计划范围：Desktop Project Context 页面信息架构、全画布工作区、画布 HUD、默认折叠的
> 右侧 Tool Rail、Structure / Semantic / Details 单面板、响应式 Drawer、viewport 连续性、
> 可访问性、组件拆分及 Desktop 测试
>
> 明确排除：Relay / DB / Tauri wire、Project Context 或语义查询协议、图 topology / layout 算法、
> 查询评分、权限规则、URL schema、语义结果持久化、Web 与 Mobile

本文在 Desktop presentation 与信息架构上取代以下旧位置描述：

- `desktop-spec.md` 中“顶部 Query Bar + Canvas + 右 Inspector”的固定布局；
- 语义 Desktop 计划中“Semantic Query Bar 位于结构 Query Bar 下方”的固定布局。

以上文档中的领域、查询、权限、currentness、selection、semantic session、All Context pairing、
canonical Inspector 与错误语义继续有效。本计划不重写历史验收证据；本次布局改造的独立验收草稿
已经建立，最终证据只在该记录中继续回填。

## 0. 已确认的产品决策

本计划以以下决定为固定前提：

1. Project Context 图是页面主体，不再作为查询控件下方的结果预览；
2. Header 以下的可用区域默认由完整图画布占据；
3. 首次进入有效 All Context 时执行一次 `Fit all`，让完整上下文图默认可见；
4. Structure query、Semantic query、统计与 Inspector 收入右侧可折叠工具区；
5. 右侧工具区默认折叠，只保留紧凑 Tool Rail；
6. Structure、Semantic、Details 共用同一个右侧内容槽，任何时刻最多展开一个；
7. 宽窗口使用 docked panel；空间不足时使用覆盖画布的 Drawer，而不是继续压缩图；
8. 打开、折叠、切换或调整面板宽度不能隐式执行 `Fit all`、重新布局或重新查询；
9. expand、collapse、tool switch、resize 等纯 presentation 操作不能清除普通 selection、
   未提交 draft、active semantic result 或 semantic overlay；显式 Inspector Close 不属于纯
   presentation 操作；
10. 点击 Coordinate、Hub 或 Spoke 仍建立普通 selection，并打开 Details；
11. 点击空白、再次点击已选对象或 Inspector Close 仍可清除普通 selection；宽屏 Details Escape
    等同 Inspector Close，窄屏第一下 Escape 只关闭 modal Drawer；
12. 普通 selection 的建立或清除绝不能清除 semantic overlay；
13. semantic overlay 继续只由显式 Clear / Cancel 或既有安全失效规则撤下；
14. active semantic result 即使右栏折叠，也必须在画布上保留可识别状态及 `Fit paths` / `Clear`；
15. Tool Rail 展开态、当前工具、面板宽度和 viewport 都是当前页面内存状态，不进入 URL、
    Local Storage 或 canonical Project Context；
16. 本次不改变 `ProjectContextQuery`、`SemanticGraphQuery`、route selection 或任何 Native DTO；
17. 初始空白 problem 仍使 `Find paths` disabled，但在用户触碰或提交前不显示橙色必填错误。

一句话目标：

> Human 进入 Project Context 时首先看到一个可阅读、可平移和缩放的完整关系画布；查询与详情按需从
> 右侧展开，工具操作不会夺走画布、重置视角或破坏已经存在的语义路径高亮。

## 1. 当前问题与实现基线

### 1.1 当前纵向信息层级反转

当前 `ProjectContextScreen` 按以下顺序渲染：

```text
Header
→ sync / stale banner
→ full-width structural Query Bar
→ full-width Semantic Query Bar
→ loading / error / empty state
→ three result count cards
→ graph card summary / Island navigation
→ React Flow canvas
→ optional right Inspector
```

这导致查询控件和摘要成为视觉主体，图只能使用最后剩余的高度。高密度 All Context 在 `Fit all`
之后必须把全部节点缩到很小，截图中的 44 Coordinates / 21 Edges 已经清楚暴露该问题。

### 1.2 GraphSlot 额外限制宽度与高度

`ProjectContextGraphSlot` 当前还施加：

- `p-4 / p-6` 外边距；
- `max-w-6xl` 宽度上限；
- 三张大统计卡；
- `min-h-96` 的图卡容器。

`ProjectContextGraph` 内部又有一整行 summary 与 Island navigation。即使在超宽窗口中，更多空间也
不会直接转化成更大的图画布。

### 1.3 Inspector 已经占用独立右侧面板

`ProjectContextInspector` 当前复用 `AuxiliaryPanel`：

- 默认宽度 440px；
- 宽屏可拖动调整；
- 共享 breakpoint 以下改为 overlay；
- 自身注册 Escape 并在 Close 时清 selection、恢复图目标焦点。

如果再新增一个独立 Query sidebar，工具面板与 Inspector 同时出现会占用约 880px，重新制造图过小
问题。因此本计划要求一个统一右侧槽，不允许两个并排辅助面板。

### 1.4 当前状态归属不能被布局重构打乱

当前状态分别属于：

| 状态 | 当前 owner | 本次要求 |
|---|---|---|
| applied structural query | route / URL | 不变 |
| Coordinate / Edge selection | route / URL | 不变 |
| structural query draft | `ProjectContextQueryBar` local state | 提升为workspace-owned受控状态；切工具或折叠时不得丢失 |
| semantic draft / attempt / active | `ProjectContextScreen` reducer | 不变 |
| semantic overlay | verified result + All Context join | 不变 |
| pan / zoom | React Flow viewport | 面板操作时保持 |
| panel open / active tool / width | 尚不存在 | 新增为页面内存状态 |

Structure Query Bar 的 draft 当前由组件内部保存。如果 tool switch、collapse 或 dock / Drawer
presentation切换造成组件重挂载，用户未 Run 的Coordinate选择会丢失。本计划固定把structural draft
提升到`ProjectContextWorkspace`拥有的受控状态；`ProjectContextQueryBar`改为接收`draft`与
`onDraftChange`。semantic draft已经由Screen reducer拥有。这样inactive pane可以安全卸载，打开的
Coordinate Picker Portal也会随pane关闭，不会残留在不可见工具之外。

### 1.5 文件尺寸已没有继续堆叠空间

当前主要文件行数：

| 文件 | 当前约行数 | 风险 |
|---|---:|---|
| `ProjectContextScreen.tsx` | 991 | 接近 Desktop 1000 行硬上限 |
| `ProjectContextGraph.tsx` | 821 | HUD / viewport 逻辑继续加入后会接近上限 |
| `ProjectContextSemanticQueryBar.tsx` | 517 | 需要改造成窄面板单列布局 |
| `ProjectContextQueryBar.tsx` | 242 | draft owner 必须保留 |
| `ProjectContextInspector.tsx` | 172 | 需要拆出可嵌入统一面板的 content |

实现必须先拆分组件，不能提高 file-size limit，也不能增加 override。目标是让
`ProjectContextScreen.tsx` 与 `ProjectContextGraph.tsx` 均保留至少约 20% 的 1000 行上限余量。

## 2. 范围与不变量

### 2.1 本计划包含

- Header 以下的 full-canvas workspace；
- 默认折叠 Tool Rail；
- Structure / Semantic / Details 单一面板；
- 统计、Island navigation、Fit 与 semantic session 的画布 HUD；
- wide docked / narrow Drawer 切换；
- panel reducer、route-driven selection observation、return-tool 行为和受控 draft 保留；
- panel resize 时 graph-world center 与 zoom 保持；
- loading、empty、error、stale 在全画布外壳中的呈现；
- keyboard、focus、ARIA、Desktop `0.75 / 1 / 1.5` text zoom 与 reduced motion；
- Project Context unit / presentation / Playwright / screenshot 更新；
- 相邻 Desktop 与 semantic 计划的 presentation 文档适配。

### 2.2 本计划不包含

- 修改 All Context、Exact、Incident、Contains all 的查询语义；
- 修改 `routeState.ts`、URL query 或 browser history 合同；
- 修改 semantic request、provider egress、ranking、floor 或 result DTO；
- 修改 All Context revision pairing、source freshness 或安全失效规则；
- 修改 graph incidence model、Hyperedge、Island 或有机布局算法；
- 根据 semantic score 改变节点位置、大小、方向或因果表达；
- 将 sidebar 状态、problem、result、selection 或 viewport 持久化；
- 增加新的 Native command、Relay endpoint、migration、event kind 或 capability；
- 自动再次运行 semantic query；
- Project Context 编辑能力；
- Web / Mobile 同步实现。

### 2.3 必须保持的可信边界

1. 直接进入页面仍读取 verified `contains-all({})` / All Context；
2. active semantic mode 仍使用同 revision verified All Context substrate；
3. semantic overlay 仍通过 complete Edge / Coordinate / binding join 才可显示；
4. topology mismatch 时 render-time gate 仍在首个不匹配 render 隐藏 overlay；
5. Inspector 继续读取 canonical Project View / Document / Meeting，不使用 semantic preview；
6. active semantic 期间 Structure Run 继续 disabled；
7. capability off、restricted、verification failure、Community / trusted identity 变化的现有处理不变；
8. stale / refreshing 时继续保留上一份经过验证的图，不把未验证 replacement 画到 canvas；
9. sidebar 开关不能触发 Project Context、Project View、Document、Meeting 或 semantic 网络调用；
10. Community keyed remount 继续清 route-scoped UI、semantic state 和新增 panel state。

## 3. 目标信息架构

### 3.1 默认折叠态

```text
┌──────────────────── Project Context Header ────────────────────┐
├──────────────────── optional slim sync banner ─────────────────┤
│ ┌─ graph summary / semantic HUD ─┐                         ┌─┐ │
│ │ 2 islands · 44 coords · ...    │                         │S│ │
│ └─────────────────────────────────┘                         │Q│ │
│                                                            │D│ │
│                                                            └─┘ │
│                                                                  │
│                    full React Flow canvas                        │
│                                                                  │
│      Pan / zoom guidance                          zoom / fit      │
└──────────────────────────────────────────────────────────────────┘
```

默认态要求：

- Header 与必要的 sync / stale 安全提示之外，没有任何全宽表单或统计卡；
- graph workspace 从 Header / banner 下缘延伸到页面底部；
- collapsed Tool Rail 作为画布 chrome 叠在右侧，不缩小 canvas；
- 第一次有效 graph identity 完成后只执行一次 `Fit all`；
- graph summary、selection、semantic snapshot 与操作状态在紧凑 HUD 中可见；
- 没有 selection 时 Details 入口 disabled，但仍有可理解 tooltip。

### 3.2 宽屏展开态

```text
┌──────────────────── Project Context Header ────────────────────┐
│                                                        ┌───────┤
│                                                        │ Rail  │
│                 graph canvas                           ├───────┤
│                                                        │       │
│                                                        │ one   │
│                                                        │ tool  │
│                                                        │ pane  │
│                                                        │       │
│                                                        └───────┤
└─────────────────────────────────────────────────────────────────┘
```

宽屏中 panel dock 在右侧。本文中的宽度统一定义为：

```text
expanded assembly width = 48px Tool Rail + panelContentWidthPx
default panelContentWidthPx = 440px
default expanded assembly = 488px
```

并且：

- 同一时刻只显示 Structure、Semantic、Details 之一；
- panel body 自己纵向滚动，页面与 canvas 不产生纵向滚动；
- panel content默认440px，可拖动；
- panel width 被限制为仍给 graph 留出最小可用宽度；
- 切换工具不卸载 graph、不重新计算 topology、不自动 Fit；
- 折叠 panel 后 graph 恢复完整宽度，但保持原 zoom 与 graph-world center。

### 3.3 窄屏 Drawer 态

当 docked expanded assembly会把画布压到不可用宽度时：

- panel 覆盖画布，不参与 graph 宽度计算；
- 使用 backdrop、focus trap、`role="dialog"`、`aria-modal="true"`；
- `workspaceWidth >= assemblyWidth + 40rem`时使用docked；等号归docked；
- `37.5rem <= workspaceWidth < assemblyWidth + 40rem`时使用右侧Drawer；
- `workspaceWidth < 37.5rem`时使用单列全宽Sheet；等号归Drawer；
- Drawer 开关前后 graph viewport 完全不变；
- 关闭后焦点恢复到对应 Tool Rail 入口或先前图目标。

响应式判断必须基于 Project Context workspace 实际宽度和 root rem，不只使用当前共享的
600px window breakpoint。440px content加48px Rail在800–900px窗口只会给图留下312–412px，不能算
docked 可用。

## 4. Canvas 与 HUD 设计

### 4.1 Graph 成为真正的 workspace substrate

最终 `ProjectContextGraphSlot` / workspace 必须移除：

- `max-w-6xl`；
- `mx-auto`；
- `p-4 / p-6`；
- 三张独立统计卡；
- graph 外层 rounded card 的独立 header row；
- 为 card layout 保留的额外纵向 gap。

`ProjectContextGraph` 最终直接使用：

```text
position: relative
width: 100%
height: 100%
min-width: 0
min-height: 0
overflow: hidden
```

边框、圆角和背景只用于浮层 chrome，不再把图包装成页面中的一张卡片。

### 4.2 HUD 区域分工

HUD 需要避免互相遮挡：

| 区域 | 内容 |
|---|---|
| 左上 | compact graph summary；Island count；当前 query mode |
| 左上第二行 | semantic snapshot / stale legend；paths / roots；Fit / Clear |
| 上方中部或右栏左侧 | 当前 selection 的紧凑 label |
| 右侧 | Tool Rail / expanded panel |
| 右下 | zoom out / zoom in / Fit all / Fit selection / Fit paths |
| 底部中间 | undirected / placement carries no rank or causality 提示 |

HUD 容器默认 `pointer-events: none`，只有真实Button、disclosure control、link恢复`pointer-events: auto`，避免透明
浮层阻止画布 pan、zoom 与 pane click。

### 4.3 Graph summary 与统计

collapsed 状态下仍应有一行紧凑摘要，例如：

```text
2 islands · 44 coordinates · 21 edges · 22 context docs
```

展开 Structure 时再提供完整统计和当前 query 说明。三张大卡不再存在，但保留
`project-context-result-counts` 测试边界，并把它迁移为 panel 内 compact definition list 或 stats grid，
减少既有测试的无意义重写。

active semantic时必须区分两套身份：

```text
applied route query       // 例如 Incident；Clear semantic后恢复
displayed canvas substrate // semantic active时固定为verified All Context
```

HUD summary与counts永远描述当前`displayed canvas substrate`。Structure pane同时显示“Applied route
query”和“Displayed for semantic result”两行，不能把Incident route的label配到All Context counts。
Project-level Island count只在displayed substrate确实是All Context时出现；focused result继续只显示
matching edges / visible coordinates，不冒充项目级Island统计。

### 4.4 Island navigation

- `Fit all` 继续在画布右下可达；
- 少量 Island 可在 HUD 使用紧凑按钮；
- Island 较多时使用可滚动列表或 Structure pane 中的 Overview；
- Island navigation 不占据整行 canvas 高度；
- 点击 Island 仍只改变 viewport，不改变 query 或 selection；
- 视觉位置仍不表达 rank、因果、owner 或语义相似度。

### 4.5 Semantic session HUD

active semantic result 即使 Tools Panel 折叠也必须显示：

- snapshot / stale 状态；
- path / root 数；
- Context revision；
- `Fit paths`；
- `Clear semantic result`；
- partial coverage / budget exhausted 的紧凑状态入口。

HUD 不显示完整 problem，不把 problem 放入 `title`、URL、日志或持久状态。完整结果说明继续位于
Semantic pane；HUD 只显示 content-free counts 与既有安全状态。

### 4.6 Canvas chrome safe area

HUD、collapsed Rail、selection label和zoom controls不能覆盖刚刚被Fit到视口边缘的节点。新增统一
`ProjectContextCanvasInsets`，由可见chrome refs与`ResizeObserver`实测得到：

```text
top    = visible summary / semantic HUD bottom + gap
right  = max(collapsed Rail width, visible right-control width) + gap
bottom = guidance / zoom controls top edge + gap
left   = base canvas gap
```

所有Fit / Focus必须调用同一个safe-area helper：

- initial query Fit all；
- Fit all / Fit Island；
- Fit selection / Focus selection；
- automatic / explicit Fit paths。

helper使用React Flow的viewport-for-bounds能力，在扣除insets后的矩形内计算zoom与translation，不能只
增加一个模糊padding。Drawer / Sheet打开期间不执行Fit；从Drawer触发Fit时先关闭modal，等待full canvas
尺寸稳定，再执行该显式Fit。zoom controls在collapsed状态按Rail宽度增加right inset；expanded docked
状态下canvas本身已排除assembly，不重复扣除panel。selection label进入同一HUD stack，不再与Rail争用
`right: 3`。

chrome测量不是“ref存在即ready”。新增闭合测量状态：

```text
ProjectContextChromeMeasurement
├── generation: u64
├── expectedContributors: closed set
├── measuredRects: contributor -> rect | intentionally_absent
├── insets: ProjectContextCanvasInsets
└── ready: boolean
```

- 当前render先冻结`expectedContributors`；未显示的HUD必须显式记为`intentionally_absent`，不能沿用上一代
  rect；
- contributor mount / unmount或`ResizeObserver`得到不同的量化rect时推进`generation`并令`ready=false`；
- graph root非零、全部expected contributor都有本代observation，并且量化rect在连续两个
  `requestAnimationFrame`中相等后，当前generation才变为ready；
- 两帧之间再次变化就废弃候选并从最新generation重新稳定，不能用固定timeout猜测布局已经结束；
- auto / semantic / explicit Fit request都绑定执行时的chrome generation。generation在回调前变化时，
  旧回调直接丢弃且不得把request key标记为已处理；同一request改为等待最新generation ready；
- Fit真正提交后才把对应query identity或semantic / manual request generation记为handled。之后普通panel
  开关或HUD badge尺寸变化不重跑已经完成的auto-fit，只走第7.2节的viewport连续性合同；
- 新semantic result挂载Semantic HUD与请求首次Fit paths属于同一新request：必须等新HUD实测完成后只Fit
  一次，禁止先用零inset Fit、再用“补偿Fit”制造第二次跳动。

## 5. 右侧工具栏信息架构

### 5.1 Tool Rail

Rail 提供三个互斥入口：

```text
Structure
Semantic
Details
```

要求：

- collapsed Rail宽固定3rem（base 48px）；
- 使用 icon + tooltip + accessible name；
- 每个入口使用互斥 disclosure button，提供 `aria-pressed`、`aria-expanded`、`aria-controls` 与
  非颜色active标记；
- Semantic icon 显示 running / active / stale badge；
- Semantic入口始终可打开；capability off时pane解释原因，只有`Find paths`不可用；
- Details 在没有 selection 时保持可聚焦并设置`aria-disabled=true`，以便键盘用户读取tooltip / 原因；
- 点击当前已展开 tool 的入口可折叠 panel；
- 点击另一个 tool 只切换 pane，不重置任何 draft / active state；
- Tool Rail 本身不进入 URL，不产生网络请求。

本计划明确不使用`tablist / tab / tabpanel`：active disclosure可以再次点击折叠，而标准tab不应以
“没有可见tabpanel”作为普通状态。展开内容使用带可访问名称的`region`；窄屏改由Sheet提供dialog语义。

### 5.2 Structure pane

Structure pane 包含：

- All Context / Exact / Incident / Contains all；
- Coordinate picker 与已选 chips；
- Applied / Draft 状态；
- Run / Clear；
- matching edges / visible coordinates / context documents；
- Island overview 与 Fit actions；
- active semantic 时的互锁说明。

宽栏中的横向 toolbar 要改为单列或自然换行布局，不能依赖 `max-w-6xl`。Structure Run 的 query、
history 和 URL 行为不变。

### 5.3 Semantic pane

Semantic pane 包含：

- problem textarea；
- UTF-8 byte count；
- `Find paths` / `Re-run`；
- Start from；
- Query context；
- active snapshot、coverage、omissions、budget、observed time；
- `Fit paths` / `Clear semantic result`；
- closed error / retry guidance。

Start from 与 Query context 在 panel 中按单列堆叠，不再使用宽屏两列 grid。两组仍分别最多 16 / 8，
同一 Coordinate 跨组合法，当前 selection 仍绝不隐式加入。

初始 pristine problem：

- `Find paths` disabled；
- textarea `aria-invalid=false`；
- 显示中性帮助，不显示“Problem must not be blank”；
- 用户触碰后离开空白字段，或尝试提交后，才显示必填错误；
- NUL、16KiB、Cmd/Ctrl+Enter 与现有 validator 合同不变。

### 5.4 Details pane

Details pane 复用现有 Coordinate / Edge Inspector 内容：

- canonical Coordinate / Edge 详情；
- relation Document semantic badges；
- Focus selection；
- Show incident Context；
- Open in Project View / Documents / Meeting；
- canonical source currentness 与错误行为。

`ProjectContextInspector` 必须拆成 panel shell 与可嵌入的 `ProjectContextInspectorContent`。统一
Tools Panel 是唯一 outer shell，不能在里面再嵌套一个 `AuxiliaryPanel`。

## 6. Workspace panel 状态机

### 6.1 状态形状

新增纯 presentation state，不与 semantic reducer 或 route state 合并：

```text
ProjectContextWorkspacePanelState
├── expanded: boolean
├── activeTool: structure | semantic | details
├── returnTool: structure | semantic | null
├── returnExpanded: boolean
├── panelContentWidthPx: number             // 不含48px Rail
├── observedSelectionKey: String | null
├── pendingSelectionOrigin
│   ├── expectedSelectionKey: String
│   └── origin: graph(target) | rail(tool)
└── openOrigin: rail(tool) | graph(target) | route | null
```

`selection` 仍由 route owner 持有，不复制到 panel reducer。`semantic active` 仍由既有 semantic reducer
持有，不复制到 panel reducer。`presentation = docked | drawer | sheet`由workspace width、root rem与
`panelContentWidthPx`纯派生，不存进reducer。

### 6.2 初始状态

| 输入 | 初始 panel |
|---|---|
| 普通 All / focused route，无 selection | collapsed；last tool = Structure |
| URL deep link 带有效 Coordinate / Edge selection | expanded Details |
| selection 在当前 substrate 不存在 | route 既有清理逻辑处理；panel 不展示空 Details |
| Community remount | 回到默认 collapsed；不复用旧 Community width / tool |

### 6.3 转移规则

| 事件 | Panel 结果 | Selection | Semantic overlay |
|---|---|---|---|
| 点击 collapsed Structure | expanded Structure | 不变 | 不变 |
| 点击 collapsed Semantic | expanded Semantic | 不变 | 不变 |
| 再点 active tool | collapsed，记住 activeTool | 不变 | 不变 |
| 点击另一 tool | expanded target tool | 不变 | 不变 |
| 点击 graph Coordinate / Edge / Spoke | expanded Details；记录此前 tool / expanded | route 建立 selection | 不变 |
| top-level Collapse | collapsed | 保留 | 不变 |
| 在 selection 存在时手动切 Structure / Semantic | 显示目标 tool | 保留 | 不变 |
| 点击空白 / 再点 selected item | 回 returnTool 或原 collapsed | route 清 selection | 不变 |
| Inspector Close / 宽屏Details Escape | 回 returnTool 或原 collapsed | route 清 selection并恢复图焦点 | 不变 |
| 窄屏Drawer Escape / backdrop | collapsed | 保留 | 不变 |
| Clear semantic result | panel按当前 tool保持 | 不变 | 清除并恢复route substrate |
| Community / trusted boundary reset | 默认 collapsed | 由现有route边界处理 | 由现有安全边界处理 |

top-level Collapse 与 Inspector Close 必须是两个不同 action：前者只隐藏工具 UI；后者明确结束当前
selection inspection。不能继续把 Inspector 的 `onClose` 同时当成整个 sidebar 的折叠操作。

### 6.4 Route-driven selection observation

panel不能只响应graph click。URL deep link、Back / Forward、Inspector内部A→B导航和substrate清理都会
直接改变route selection，因此Screen必须把每次canonical selection变化派发为
`selection_observed(previous, next)`：

- `null → A`：记录当前tool / expanded作为return state，展开Details；
- `A → B`且当前仍是Details：只更新`observedSelectionKey`，不得覆盖最初return state；
- `A → B`但Human已手动切到Structure / Semantic：以当前tool作为新的return state，再进入Details；
- `A → null`且当前是Details：回return state；
- `A → null`但Human已手动切到Structure / Semantic：保持当前tool；
- collapsed Details中的selection被清除：保持collapsed；
- invalid / missing selection由现有route清理后走同一`A → null`路径。

Human在Details中执行top-level Collapse时，把`returnExpanded`同步设为false；之后pane clear或Back清除
selection仍保持collapsed。Human在selection仍存在时手动切到Structure / Semantic，则该tool立即成为
新的return target；以后观察到另一个selection才再次自动进入Details。

仅凭`previous / next selection`无法判断来源，因此在graph click调用route `onSelectionChange`之前，先派发
一次presentation-only `selection_open_intent(expectedSelectionKey, graphTarget)`；Rail触发则记录
`rail(tool)`。随后`selection_observed`只有在next key与pending expected key精确相等时才消费该origin，
并立即清空pending；不相等、selection被拒绝、Back / Forward、deep link或substrate清理都清空pending并
使用`route`fallback。pending intent不是第二份selection，不负责决定选中了什么。

`openOrigin`据此记录真正打开当前surface的来源：Rail button打开返回该button；graph item自动打开
Details返回该graph target；Back / Forward或deep link没有仍存在的trigger时，回graph canvas或Details
Rail button。不得把所有Drawer close一律恢复到Rail，也不得尝试focus已经从新substrate消失的target。

route selection仍是唯一事实owner；intent只补足focus presentation信息。所有Details转移最终仍由
`selection_observed`提交，避免click、Back / Forward与Inspector导航产生不同selection语义。

### 6.5 Pane lifecycle、Portal 与 focus 边界

- structural / semantic draft由panel subtree之外的owner持有；
- 只mount当前expanded pane，inactive或collapsed pane不留在layout、tab order或accessibility tree；
- tool switch、collapse和Sheet关闭必须unmount当前Coordinate Picker，从而同步关闭其Portal；
- Inspector content只在有效selection时mounted；
- inactive Details不注册全局Escape；
- panel collapsed后内部任何input都不能继续接收键盘焦点；
- dock↔Drawer↔Sheet remount不能丢structural / semantic draft或active result；
- 重新打开时优先恢复该tool上一次有意义焦点，而不是重置draft。

## 7. Viewport 与 layout 连续性

### 7.1 哪些动作可以改变 viewport

只有以下动作可以主动 Fit / Focus：

1. 首次有效 query identity / substrate 完成初始化；
2. structural query identity 真正改变；
3. 新 semantic generation 按既有合同首次 `Fit paths`；
4. Human 显式点击 Fit all / Fit Island / Fit selection / Fit paths；
5. Human 显式请求 Focus selection；
6. text scale 改变时按既有 focal-point 保持算法修正 viewport。

以下动作绝不能调用 Fit：

- expand / collapse panel；
- Structure / Semantic / Details 切换；
- panel resize；
- 编辑 draft；
- 打开 Coordinate picker；
- selection 建立或清除本身；
- semantic status badge变化但 generation未变；
- stale / refreshing badge变化；
- Inspector 内部折叠 section。

auto-fit只有一个owner：删除`<ReactFlow fitView>` prop，保留并收口query-identity scheduler。scheduler
只有在nodes initialized、canvas非零、第4.6节当前chrome generation ready、当前query identity尚未处理且
不存在更高优先级viewport operation时，才调用一次统一safe-area Fit helper。等待期间query identity或
chrome generation变化会取消旧callback但不会消费request；最终只允许最新组合提交一次。

manual Fit和semantic generation Fit也调用同一helper，但使用各自独立request / generation去重。显式从
Drawer触发Fit时，必须在关闭Drawer之前建立pending Fit generation；Drawer resize只更新测量基线，不做
中心校正，full-canvas chrome generation稳定后该Fit才提交。不得同时保留React Flow初始fit和外部
`fitBounds` effect两条auto路径。

### 7.2 Docked panel resize 的中心保持

panel dock / undock 或 width 变化前后，必须保持：

```text
zoom_after == zoom_before
world_point_at_canvas_center_after ~= world_point_at_canvas_center_before
```

实现可在 `ProjectContextGraphInner` 中观察 graph root size。每个resize anchor必须携带：

```text
ViewportResizeFence
├── queryIdentity
├── textScaleGeneration
├── fitGeneration
└── resizeSequence
```

执行顺序：

1. 记录旧 canvas `width / height` 与当前 viewport；
2. 计算旧画布中心对应的 graph-world point；
3. 为本次observation分配单调`resizeSequence`，捕获上述三项authority fence；
4. ResizeObserver观察到新尺寸后，只允许最新sequence且三项fence仍与当前值完全相等的correction提交；
5. 使用相同zoom调整`x / y`，把该world point放到新画布中心；
6. duration固定为0，不制造panel resize相机动画。

query identity变化、text-scale focal correction开始、以及任何auto / semantic / explicit Fit request建立时，
分别推进对应authority generation，从而同步失效全部旧resize anchors。只要存在pending authoritative Fit或
text-scale correction，ResizeObserver仍记录最新canvas size，但不得提交中心校正；高优先级操作完成后以
其结果重置resize baseline。manual Fit generation必须在关闭Drawer或改变panel chrome之前推进，不能让
关闭动作产生的迟到resize correction覆盖Fit。新的query auto-fit同理：旧correction丢弃，最新query等
nodes与chrome ready后一次性Fit。较旧的RAF、observer callback或promise completion不得回写较新的
viewport。

新增纯函数 `recenterProjectContextViewportForResize()`，对浮点结果量化或使用明确 tolerance 测试。

### 7.3 Graph 不得因 panel state 重挂载

- `ProjectContextGraph` 不能以 `activeTool`、`expanded` 或 `panelWidth` 作为 React key；
- `ReactFlowProvider` 在 panel 开关过程中保持同一实例；
- graph topology / geometry memo 输入不加入 panel state；
- workspace state改变时向 graph传递的 result、selection、overlay和callbacks必须保持引用稳定；
- 不向 memoized graph传入 inline JSX、全量 mutation object 或每 render 新建的 Map / array；
- 在 DevTools关闭、无perf probe条件下评估输入 problem时的交互延迟。

## 8. 响应式与可访问性

### 8.1 Dock / Drawer 判定

定义 Project Context 专属约束，而不是直接采用共享600px breakpoint。所有`rem`先使用当前root
font-size换算成CSS px：

```text
rail width                    = 3rem       // base 48px
default panel content        = 27.5rem    // base 440px，不含Rail
minimum panel content        = 22.5rem    // base 360px
maximum panel content        = 35rem      // base 560px
minimum readable canvas      = 40rem      // base 640px
sheet boundary               = 37.5rem    // base 600px
```

定义：

```text
railPx = 3rem
minCanvasPx = 40rem
panelContentPx = clamp(savedInMemoryWidth, 22.5rem, 35rem)
assemblyPx = railPx + panelContentPx

docked iff workspacePx >= assemblyPx + minCanvasPx
drawer iff 37.5rem <= workspacePx < assemblyPx + minCanvasPx
sheet  iff workspacePx < 37.5rem
```

等号归属如上，测试使用`threshold - 1px / threshold / threshold + 1px`。resize handle只在docked显示；
拖动content width时最大值再clamp到`workspacePx - railPx - minCanvasPx`。Drawer宽度使用
`min(panelContentPx, workspacePx - 3rem)`；Sheet宽度为workspace的100%。panel content width仅保存在
当前Community页面内存，不跨remount持久化。

Desktop当前只支持`0.75 / 1 / 1.5`文本缩放。本次不扩展zoom产品合同；因为公式使用rem，1.5时
minCanvas与breakpoint会自然放大并更早进入Drawer / Sheet。

### 8.2 Shared AuxiliaryPanel 复用

实现路径固定为：

- docked使用`AuxiliaryPanel`及其Header、Body和resize handle；
- Drawer / Sheet使用现有Radix-backed `shared/ui/sheet.tsx`；
- 两种shell调用同一个`ProjectContextToolPaneContent` renderer；
- pane自身只接收workspace-owned controlled draft / result / selection props；
- presentation跨threshold造成shell remount时，状态不会丢失；
- Sheet负责`role=dialog`、modal focus trap、background inert、backdrop与Escape。

不修改`AuxiliaryPanel`的共享600px auto breakpoint：Project Context只有在自己的公式判定为docked时
才render该组件，此时workspace必然足够宽；narrow直接renderSheet。Drawer打开后Tool Rail位于Sheet
content内部，用户仍可在modal内切换Structure / Semantic / Details。modal open期间外部collapsed Rail
直接unmount，只允许DOM中存在一份Rail、每个`id / aria-controls / data-testid`也只能有一份；Sheet关闭后
外部Rail重新mount，workspace根据`openOrigin`在该ref可用后的下一animation frame程序化恢复焦点。不得
用现有floating`aside + backdrop`冒充modal Drawer，也不得复制一套无focus trap的panel。

### 8.3 Keyboard 与 Escape 层级

Escape 按最内层交互消费：

1. 打开的 Coordinate picker / popover；
2. 其他 panel 内 modal / disclosure；
3. Drawer / Sheet；
4. Details inspection；
5. graph 页面。

第一下 Escape 关闭 picker 时不能穿透并同时清 selection、关闭 Drawer 或清 semantic result。

宽屏中：

- Structure / Semantic Escape 只折叠 panel；
- Details Escape 延续 Inspector Close：清 selection、恢复图目标焦点、回 return tool；
- top-level Collapse button 只折叠，不清 selection。

窄屏中：

- Structure / Semantic 中的 Escape 关闭 Drawer，保留 draft / selection / semantic result；
- Details 中的第一下 Escape同样只关闭modal Drawer并保留selection；
- top-level Collapse 仍只隐藏 Drawer，不清 selection；
- Details header内的显式 Close selection同样执行 Inspector Close；
- focus按`openOrigin`回到Rail button、graph target或安全canvas fallback。

### 8.4 ARIA 与文本缩放

- Tool disclosure buttons提供 `aria-pressed`、`aria-expanded`、`aria-controls`；
- panel有可访问名称；
- expanded docked content使用带label的`region`，Drawer / Sheet使用dialog语义；
- inactive pane不mount；受控draft在pane之外保留；
- 动态公告只有一个owner：`ProjectContextWorkspaceAnnouncement`维护单一polite `aria-live` region，
  按selection、semantic attempt/result、stale / error的closed event key去重；
- Semantic pane result status、guidance、Graph sr-only summary与HUD均为非live描述，通过
  `aria-describedby`关联；panel切换不能重复播报semantic result；
- graph summary仍由 screen reader读取；
- semantic overlay Node / Hub / Spoke描述不变；
- 所有正文使用 stock rem tokens或现有 `text-2xs / text-3xs`；
- 不新增任意 px / rem text literal；
- `0.75 / 1 / 1.5` text zoom下panel内部纵向滚动，CTA与状态不能被裁掉；
- `prefers-reduced-motion`下 panel与camera都使用零时长或静态切换。

唯一live owner仍可依次公告不同Human事件，但同一result request、同一selection和同一stale transition
只能公告一次。Rail badge与HUD文本变化本身不触发第二条announcement。

## 9. Loading、错误与 currentness

### 9.1 Loading

- 初次没有 verified result时，workspace仍占满剩余区域；
- loading skeleton / message居中显示在 canvas substrate；
- Tool Rail可见，但依赖 graph data 的 actions disabled；
- loading不能把 query bars重新放回顶部。

### 9.2 Empty 与 no-match

- All Context `activeEdgeCount=0` 使用全画布 empty state；
- Structure pane仍可使用；
- focused no-match继续显示 Query Anchors，不创造 Island / Edge；
- semantic zero-path仍是有效 active snapshot，HUD显示 `No paths`，不伪装成 idle。

### 9.3 Stale / refreshing

- 安全相关 sync / stale banner继续位于 Header下方，不能藏进折叠 panel；
- banner保持单行或紧凑多行，不重新成为大块页面内容；
- stale semantic snapshot在HUD与Rail badge同时可识别；
- source stale / topology mismatch合同不变；
- panel开关不触发refresh。

### 9.4 Fatal / restricted / verification failure

- 没有可展示可信图时，failure state占据workspace主体；
- restricted / verification / observed capability loss继续按既有安全合同撤下active semantic UI；
- 不把旧semantic preview用于填充graph或Inspector；
- Retry仍只执行现有trusted read，不自动重放semantic Provider query。

## 10. 组件与文件设计

### 10.1 新增文件

建议新增：

- `desktop/src/features/project-context/workspacePanelModel.ts`
  - pure panel reducer；
  - return tool / expanded规则；
  - responsive presentation派生；
- `desktop/src/features/project-context/workspacePanelModel.test.mjs`；
- `desktop/src/features/project-context/projectContextViewport.ts`
  - resize中心保持纯函数；
- `desktop/src/features/project-context/projectContextViewport.test.mjs`；
- `desktop/src/features/project-context/ui/ProjectContextWorkspace.tsx`
  - Header下full-canvas shell；
  - canvas / rail / panel composition；
- `desktop/src/features/project-context/ui/ProjectContextToolRail.tsx`；
- `desktop/src/features/project-context/ui/ProjectContextToolsPanel.tsx`；
- `desktop/src/features/project-context/ui/ProjectContextCanvasHud.tsx`；
- `desktop/src/features/project-context/ui/ProjectContextInspectorContent.tsx`；
- 本次布局独立验收文档。

实际拆分可合并极小文件，但不得把新逻辑继续堆入 `ProjectContextScreen.tsx` 或新增 file-size override。

### 10.2 修改文件

`ProjectContextScreen.tsx`：

- 保留 trusted query / semantic orchestration；
- 将 presentation composition下放到Workspace；
- 接panel reducer；
- 保持result / overlay render-time gates；
- 提供稳定callbacks；
- 移除full-width bars和内联GraphSlot。

`ProjectContextGraph.tsx`：

- 移除card header / outer rounded card；
- 抽离HUD与Island navigation；
- 接canvas resize中心保持；
- 删除React Flow `fitView` prop，以query-identity effect作为唯一auto-fit owner；
- 所有Fit使用CanvasInsets safe-area helper；
- graph root输出content-free auto-fit / viewport-correction counters供E2E断言；
- 保持现有fit / semantic / selection逻辑；
- 不修改topology / geometry输入。

`ProjectContextQueryBar.tsx`：

- 改为panel-friendly单列/自然换行；
- 改为controlled `draft / onDraftChange`，保留test ids；
- applied query identity变化时由workspace draft controller执行现有同步规则；
- collapse、tool switch和dock / Sheet remount时不丢draft；
- active semantic互锁不变。

`ProjectContextSemanticQueryBar.tsx`：

- 改为panel-friendly单列；
- 增pristine/touched错误显示门；
- active summary与HUD分工；
- 保留input/result/test ids和closed error行为。

`ProjectContextInspector.tsx`：

- 抽出content；
- outer panel由unified Tools Panel拥有；
- 收口Escape注册范围；
- 保留canonical read和focus restore。

`project-context-graph.css`：

- full-bleed canvas；
- HUD / rail / docked / drawer组合；
- light / dark；
- semantic + selection组合不回归；
- reduced motion。

`desktop/tests/e2e/project-context.spec.ts`：

- 现有直接寻找top bars的场景先打开对应tool；
- 增full-canvas / panel / viewport / responsive / focus矩阵；
- 保留existing query / semantic / Inspector / graph test ids。

`desktop/tests/e2e/project-context-workspace.spec.ts` 与 `desktop/playwright.config.ts`：

- 新建独立workspace / panel / viewport / responsive spec，避免继续增长已约2926行的existing spec；
- 抽取复用的Project Context fixtures / helpers；
- 把新spec注册进smoke project；
- 原spec继续负责领域query、Inspector、semantic与历史graph行为回归。

相邻文档：

- `desktop-spec.md`；
- `desktop-implementation-plan.md`；
- semantic Desktop implementation plan；
- 新增本次布局独立验收记录，并从当前计划添加forward link。

`project-context-semantic-query-desktop-qualification.md`及其既有截图hash属于`2e006c7dc`时点的历史
资格证据，本次保持只读。新验收记录可以引用它说明semantic安全/查询基线，但必须把新layout截图、hash
和实现提交记录在新文件中。

### 10.3 明确不改

- `desktop/src-tauri`；
- `tauriProjectContext.ts` / `tauriProjectContextSemantic.ts` wire；
- `routeState.ts` query / selection schema；
- `semanticSession.ts`安全状态机；
- `semanticOverlay.ts`结构join语义（实现只增加HUD所需的content-free presentation flags）；
- `graph.ts` incidence model；
- `layout.ts` / `radialLayout.ts`；
- Relay / DB / provider / fleet / migration；
- ACP prompt、Carryforth、Web、Mobile。

## 11. 分阶段实现计划

> 当前进度：Phase F0–F6均已实现并通过独立
> [验收记录](./project-context-full-canvas-workspace-acceptance.md)中的自动化与视觉门；本节保留为实现与
> 退出门合同。实现commit尚待回填，且该结论不改变semantic production qualification。

### Phase F0：冻结 presentation 合同与 test selectors（已完成）

交付：

- 本计划；
- final workspace / panel / HUD术语；
- panel reducer事件表；
- dock / drawer判定；
- viewport允许变化白名单；
- stable test ids：`project-context-tools-rail`、`project-context-tool-structure`、
  `project-context-tool-semantic`、`project-context-tool-details`、`project-context-tool-panel`、
  `project-context-canvas-hud`、`project-context-semantic-session-hud`；
- 旧文档position描述的supersession说明。

退出门：

- 不改变任何领域或wire合同；
- collapsed与Inspector Close语义不混淆；
- semantic overlay清除条件不扩张；
- structural draft受控owner唯一；
- narrow overlay不是实现自由项；
- unrelated worktree changes不被覆盖或带入交付。

### Phase F1：无视觉行为变化的组件拆分（已完成）

交付：

- `workspacePanelModel`纯状态机与测试；
- viewport resize纯函数与测试；
- `ProjectContextInspectorContent`；
- `ProjectContextCanvasHud` / Island navigation抽取；
- `ProjectContextWorkspace`初始composition；
- structural query draft提升为workspace-owned controlled state；
- Screen内联GraphSlot迁出；
- stable callback / memo边界。

此阶段先保持当前可见布局，减少后续一次同时改变状态与DOM的风险。

退出门：

- current Project Context unit / E2E不回归；
- Screen和Graph文件均有明显headroom；
- graph不因panel reducer测试seam重挂载；
- Inspector canonical行为不变；
- 不新增file-size override。

### Phase F2：全画布 substrate 与 HUD（已完成）

交付：

- Header下full-canvas workspace；
- 移除max-width / outer padding /统计卡 / graph card header；
- compact graph summary；
- Island / Fit / zoom controls HUD；
- semantic session HUD；
- loading / empty / failure全画布呈现；
- 默认collapsed rail占位。

退出门：

- 默认graph从Header/banner下缘延伸到screen底部；
- 页面无纵向scroll；
- 初次query identity只Fit一次；
- 初次Fit等待当前HUD / Rail chrome generation连续两帧稳定，不使用零值或上一代insets；
- graph-slot矩形等于Header/banner以下workspace可用矩形；
- safe-area Fit后全部fitted node bounds位于扣除HUD / Rail后的可视矩形内；
- semantic overlay / selection样式不回归；
- HUD不阻止pan / zoom / pane click。

### Phase F3：Structure / Semantic 单一工具面板（已完成）

交付：

- Tool Rail；
- expanded panel shell；
- Structure pane；
- Semantic pane；
- workspace-owned controlled structural draft；
- inactive pane unmount与Picker Portal teardown；
- panel宽度与resize；
- pristine semantic validation；
- Rail semantic status badges。

退出门：

- 默认panel collapsed；
- Structure / Semantic切换、折叠、重开不丢draft；
- panel操作零额外trusted/semantic network call；
- active semantic时Structure Run仍disabled；
- collapsed仍可Fit / Clear semantic result；
- inactive pane完全退出DOM / tab order / accessibility tree；
- tool switch / collapse后没有残留Picker Portal。

### Phase F4：Details统一与selection返回行为（已完成）

交付：

- Details pane接入统一panel；
- graph selection自动打开Details；
- returnTool / returnExpanded；
- top-level Collapse与Inspector Close拆分；
- pane clear / duplicate click返回；
- deep-link selection初始化；
- Edge relation Document semantic badge回归。

退出门：

- node / Hub / Spoke selection都进入Details；
- collapse保留selection；
- Inspector Close / 宽屏Details Escape清selection并恢复焦点；
- 窄屏Details Escape只关闭Sheet并保留selection；
- pane click清selection后回此前tool / collapsed；
- 所有以上动作都不清semantic overlay；
- URL Back / Forward selection行为不变。

### Phase F5：响应式、viewport、a11y与性能（已完成）

交付：

- workspace-specific dock / drawer派生；
- Radix Drawer / Sheet focus trap / backdrop；
- resize中心保持；
- `0.75 / 1 / 1.5` text zoom；
- reduced motion；
- Escape分层；
- graph memo / stable prop收口；
- 高密度交互性能审计。

退出门：

- panel开合前后zoom相同、world center在tolerance内；
- overlay Drawer不改变graph container尺寸；
- threshold−1 / threshold两侧行为确定；
- 560px宽窗口无横向overflow；
- picker第一下Escape不穿透；
- focus回到正确rail button或graph target；
- typing / tool switching不重新计算graph topology / geometry；
- graph root DOM marker与auto-fit counter在typing / tool switching后保持；
- resize correction在尺寸稳定后两帧内停止，不形成observer feedback loop；
- query identity、text scale或Fit generation推进后，任何旧resize correction都被fence拒绝；
- pending authoritative Fit期间只更新resize baseline，不提交会覆盖Fit的中心校正。

### Phase F6：E2E、视觉证据与文档收口（已完成）

交付：

- pure / presentation / E2E全矩阵；
- dense full-canvas fixture；
- light / dark截图；
- docked Details / semantic+selection / narrow Drawer截图；
- 新布局截图与旧semantic D6截图并列记录，不修改旧报告/hash；
- 本布局独立验收记录；
- 父Desktop / semantic presentation文档链接与状态更新。

退出门：

- 全Desktop质量门通过；
- 截图等待animation完成且hash互异；
- 历史验收文档不被改写成“当时就是新布局”；
- 没有problem / title / summary / identity进入截图fixture之外的日志或文档证据；
- 没有Native / Relay / DB diff；
- 最终UI满足第14节全部验收标准。

## 12. 测试矩阵

### 12.1 Pure panel model

新增测试：

- 默认collapsed + last Structure；
- Structure / Semantic / Details互斥；
- 同tool toggle折叠；
- selection自动进入Details；
- route / Back / Forward的`selection_observed`与graph click行为一致；
- matching pending origin被消费一次；mismatch / rejected / route observation清空pending并使用fallback；
- Details中selection A→B不覆盖原return state；
- collapsed Details中graph click把selection A→B时，matching intent会重新展开Details且不覆盖最初return
  state；
- 手动切Semantic / Structure后，下一次selection以当前tool作为return target；
- collapsed来源进入Details后Close回collapsed；
- Structure来源进入Details后Close回Structure；
- Semantic来源进入Details后pane clear回Semantic；
- top-level Collapse不清selection；
- collapsed Details清selection后仍collapsed；
- Details Close与Collapse是不同action；
- openOrigin分别为Rail / graph target / route fallback；
- Community keyed reset回默认；
- details without selection fail closed；
- width clamp；
- docked / drawer / sheet threshold−1、threshold、threshold+1；
- root text scale改变会更早进入Drawer。

### 12.2 Viewport pure tests

- width减少保持zoom与旧center world point；
- width增加保持zoom与旧center world point；
- height变化；
- fractional zoom；
- zero / invalid dimensions fail closed；
- repeated same-size observation无漂移；
- query identity变化不使用旧resize anchor；
- text-scale generation变化不使用旧resize anchor；
- semantic / explicit Fit generation变化不使用旧resize anchor；
- resize sequence只有最新observation可提交；
- pending authoritative Fit时resize只更新baseline；
- chrome contributor缺失、零尺寸或generation未稳定时Fit request保持pending且不计handled；
- chrome generation变化会取消旧callback并在最新generation ready后只提交一次；
- reduced motion不影响数值结果。

### 12.3 Pure presentation / model tests

仓库当前没有React DOM component-test harness，本次不为该功能引入第二套浏览器测试框架。pure tests只
覆盖无DOM的模型：

- displayed substrate summary与applied route query分离；
- focused result不生成项目级Island summary；
- semantic+selection两个emphasis轴继续组合；
- relation Document badge identity只落到正确Edge；
- CanvasInsets合并与safe rectangle计算；
- auto-fit request / generation去重纯状态；
- responsive formula与width clamp；
- structural draft applied-key同步helper；
- 无新的arbitrary text-size literal由现有guard验证。

pointer-events、ARIA、textarea、Portal、focus、CTA裁切与视觉状态全部放进Playwright。

### 12.4 Playwright：默认全画布

在 1280×800 与 1440×900：

- Tool Rail默认`aria-expanded=false`；
- Query / Semantic表单不位于graph上方；
- graph-slot顶部紧贴Header / status下缘；
- graph-slot底部与screen主体底部对齐；
- 页面没有纵向scroll；
- compact HUD显示counts / islands；
- dense 44 Coordinate / 21 Edge / 22 Document fixture可见；
- graph-slot矩形等于workspace在Header/banner以下的可用矩形；
- safe-area Fit后所有fitted node bounds不与HUD / Rail安全区相交；
- content-free `data-auto-fit-count`证明首次load只Fit一次；
- 延迟挂载或延迟测量HUD时，Fit保持pending；chrome连续两帧稳定后恰好Fit一次且节点避开HUD；
- Fit all / Island / zoom仍工作。

### 12.5 Playwright：tool state与no-remount

- 填写structural draft；
- 填写semantic problem与Initial / Context；
- Structure → Semantic → collapse → reopen；
- 所有未提交值保持；
- panel操作不增加Tauri invoke / semantic query call；
- graph root DOM identity保持；
- canonical geometry不变；
- tool switch不重新触发initial Fit；
- problem typing不使React Flow root / nodes重建；
- panel state切换不改变content-freeauto-fit counter。

### 12.6 Playwright：selection / Details

- 点击Coordinate打开Details；
- 点击Hub / Spoke打开完整Edge Details；
- collapse后selection URL与graph emphasis保留；
- 重开Details仍检查同一对象；
- pane click清selection并回last tool；
- 再次点击selected item清selection；
- Inspector Close / 宽屏Details Escape清selection并恢复graph focus；
- 窄屏Details Escape保留selection，显式Close selection才清除；
- 显式Structure / Semantic切换不清selection；
- deep-link selection首次进入自动开Details；
- selection在substrate不存在时按既有规则清理。

### 12.7 Playwright：semantic overlay

- Run成功后折叠panel；
- HUD仍显示paths / roots / revision；
- route / root / terminal / complete Hyperedge data属性保持；
- Coordinate / Hub / Spoke / pane / tool switch / collapse / reopen / Inspector Close都不清overlay；
- Structure pane仍disabled；
- collapsed HUD可Fit paths；
- Clear才清普通交互中的semantic result；
- topology / trusted boundary不匹配仍按既有安全规则同步隐藏；
- delayed response在Cancel后不能复活；
- zero path仍是active snapshot。

### 12.8 Playwright：viewport连续性

- pan / zoom到非默认位置；
- 记录zoom与canvas center对应world point；
- open docked panel；
- resize panel；
- switch tool；
- collapse panel；
- 每一步zoom不变、world point在tolerance内；
- 不能只比较transform字符串，因为canvas width改变时合法x会变化；
- query identity或semantic generation未变时不得auto Fit；
- 显式Fit actions仍改变viewport；
- pending panel resize与query identity变化同时发生时，旧resize callback不能覆盖新query Fit；
- pending panel resize与semantic / manual Fit同时发生时，旧callback不能覆盖Fit结果；
- pending panel resize与text-scale focal correction同时发生时，旧callback不能覆盖缩放后的焦点；
- 从Drawer点击Fit先建立fit generation、关闭Drawer、等待full-canvas chrome稳定，再只提交一次Fit；
- reduced motion时duration=0。

### 12.9 Playwright：responsive与a11y

- dockThreshold−1使用Drawer，dockThreshold使用docked；
- sheetThreshold−1使用Sheet，sheetThreshold使用Drawer；
- 560×800在base text scale使用full-width Sheet；
- Drawer不压缩canvas；
- Drawer / Sheet打开时外部Rail不在DOM，整页Rail、受控panel id与stable testid各自恰好一份；
- backdrop / Escape关闭Drawer且保留selection / draft / semantic result；
- focus trap不进入background graph；
- 关闭后focus按openOrigin返回rail trigger、graph target或canvas fallback；
- picker打开时第一下Escape只关picker；
- `0.75 / 1 / 1.5` text zoom下panel内部scroll且CTA可达；
- disclosure button的`aria-pressed / expanded / controls`一致；
- no-selection Details为可聚焦`aria-disabled`并能读到原因；
- capability-off Semantic pane可打开并解释不可用；
- inactive pane不在DOM / accessibility tree，Picker Portal同步消失；
- panel切换不重复semantic `aria-live`；
- workspace只有一个`aria-live`owner；
- pristine blank problem无warning且`aria-invalid=false`；
- touched / submit blank显示warning；
- HUD只有真实controls接收pointer，透明区域允许pane click；
- text zoom下CTA不被裁切；
- 无横向overflow。

### 12.10 视觉证据

至少生成并人工检查：

1. light：default dense full-canvas，panel collapsed；
2. dark：default dense full-canvas，panel collapsed；
3. wide：Details docked + selected Coordinate / Edge；
4. wide：Semantic active + ordinary selection；
5. narrow：Drawer open；
6. stale：semantic snapshot HUD + panel badge。

截图要求：

- `installMockBridge()`先于页面mount；
- screenshot前调用`waitForAnimations(page)`；
- 局部状态优先`locator.screenshot()`；overlay态使用必要的full-page / clip；
- 每张图hash必须互异；
- 不使用relay media或第三方host；
- 若用于PR，遵循`scripts/post-screenshots.sh`；
- 新的布局证据不能覆盖或篡改历史验收结论。

## 13. 质量门与性能门

### 13.1 Desktop质量门

至少运行：

```bash
. ./bin/activate-hermit
just desktop-ci

# Stop only a stale preview process owned by this checkout on 4173, if any.
# CI=1 disables Playwright reuseExistingServer and fails instead of using old dist.
(cd desktop && pnpm build:e2e)
(cd desktop && CI=1 pnpm exec playwright test \
  tests/e2e/project-context.spec.ts \
  tests/e2e/project-context-workspace.spec.ts \
  --project=smoke)

just ci
git diff --check
```

`just desktop-ci`中的`desktop-check`会执行`pnpm check`，覆盖Biome format/lint、file-size、px-text与
pubkey-truncation guard。新workspace spec必须注册到`playwright.config.ts` smoke project；不得依赖CLI
显式文件名才能被默认smoke发现。

### 13.2 性能门

使用small、截图dense fixture和现有1000 Edge pure synthetic scale分别验证：

- 在graph root写出content-free `data-auto-fit-count`与`data-viewport-correction-count`测试seam；
- 同时写出content-free `data-chrome-generation`、`data-chrome-ready`与当前viewport authority generation，
  仅用于确定性测试，不包含query / source identity；
- panel toggle / tool switch / problem typing后graph root外部DOM marker仍存在；
- canonical node geometry和auto-fit count不变；
- 单次settled dock / collapse最多产生一次viewport correction；
- resize结束后等待两个animation frame，correction count不再增长；
- continuous drag可按每个observed settled size修正，但不能自触发新size循环；
- 被query / text-scale / Fit generation废弃的callback不增加viewport-correction count；
- delayed HUD measurement不增加auto-fit count，直到ready后从0变1且之后稳定；
- onlyRenderVisibleElements继续生效；
- inactive pane已unmount，没有隐藏layout测量；
- 无新增module-level Community cache；
- 1000 Edge继续只作为pure topology / layout有限性门，不冒充ReactFlow UI SLO。

本计划不冻结通用毫秒SLO。实现阶段可在DevTools关闭、无console perf probe时记录panel开合、typing、
resize与fit时长作为信息性证据，但退出门只使用上述确定性mount、counter、geometry与observer-settle
断言。如果记录显示明显回归，必须修稳定引用、拆分render owner或限制resize更新频率，不能通过移除
currentness / overlay验证换取速度。

## 14. 最终验收标准

实现只有同时满足以下条件才算完成：

1. 默认进入Project Context时，图占据Header以下全部可用主体区域；
2. 右侧工具栏默认折叠，collapsed chrome不缩小canvas；
3. 初次All Context自动Fit一次，后续panel操作不自动Fit；
4. Structure、Semantic、Details共用一个右侧槽；
5. panel开关、切换和resize不丢draft、selection、semantic result或viewport；
6. 点击graph item能进入Details；top-level Collapse与Inspector Close语义不同；
7. semantic overlay在普通selection全部交互中持续存在；
8. collapsed状态仍可识别semantic session并Fit / Clear；
9. docked panel永远给graph保留最小可读宽度，窄屏自动改Drawer；
10. Drawer具备backdrop、focus trap、Escape层级和焦点恢复；
11. graph topology、Hyperedge、Island、layout、query、URL、权限和currentness合同没有变化；
12. Inspector继续只读canonical source；
13. panel操作不产生任何额外网络请求；
14. graph不因panel state重挂载或重新计算geometry；
15. pristine blank problem不显示提前报错，真实invalid input仍fail closed；
16. loading、empty、stale、fatal、focused no-match在full-canvas shell中正确；
17. light / dark、wide / narrow、semantic / selection组合均有自动化与视觉证据；
18. `ProjectContextScreen.tsx`、`ProjectContextGraph.tsx`不接近1000行上限且不新增override；
19. Desktop全部质量门通过；
20. 文档准确标明本次只改变presentation，不把历史资格证据或semantic production readiness夸大。

## 15. 风险、禁止反例与回滚

### 15.1 主要风险

| 风险 | 后果 | 约束 |
|---|---|---|
| Tools与Inspector同时渲染 | graph再次变窄 | 单一右槽 |
| pane / Sheet重挂载 | structural draft丢失 | workspace-owned controlled draft |
| panel resize触发Fit | Human视角跳动 | center-preserving resize；Fit白名单 |
| Graph key依赖panel | React Flow重挂载 | panel state不进key / topology |
| 只用600px共享breakpoint | 中等窗口graph不可读 | workspace-specific threshold |
| Picker Portal残留 | keyboard落入不可见UI | inactive pane unmount + Portal teardown |
| Escape穿透 | picker、selection、overlay被一起关闭 | layered Escape ownership |
| HUD透明层拦截canvas | pan / pane click失效 | pointer-events分层 |
| stale result覆盖新graph |错误语义路径 | 保留现有render-time substrate gate |
| Screen继续增长 | file-size gate失败、难审计 | 先拆分，无override |

### 15.2 明确禁止的实现捷径

- 不把graph简单设成更大的`min-height`而保留顶部全部控件；
- 不同时显示Query sidebar与Inspector panel；
- inactive pane必须unmount以关闭Portal；禁止因unmount丢失workspace-owned draft、semantic state或
  selection；
- 不把panel开关写入route或Local Storage；
- 不在panel开关时调用`fitView()`掩盖camera问题；
- 不修改query identity迫使React Flow重建；
- 不复制semantic result成为新的graph DTO；
- 不用semantic preview填充Inspector；
- 不降低完整Hyperedge高亮或currentness验证；
- 不新增任意text-size literal；
- 不增加file-size override；
- 不把历史截图hash静默替换为新布局而不更新说明。

### 15.3 回滚

本次没有schema、wire或持久状态变化。代码回滚可以按presentation阶段反向执行：

1. 关闭新workspace composition，恢复旧Screen位置；
2. 保留已抽出的pure model / Inspector content，不影响领域行为；
3. 恢复旧Graph card header / stats位置；
4. 不需要回滚数据库、Relay、semantic generation、Community gate或用户数据。

如果新layout未通过viewport、focus或semantic overlay门，应保持原页面布局，不以部分启用的两个sidebar
或自动Fit workaround发布。

## 16. 交付证据模板

独立[验收记录草稿](./project-context-full-canvas-workspace-acceptance.md)已建立。最终收口时至少填写：

```text
Implementation commit:
Desktop test counts:
Project Context Playwright result:
Default full-canvas viewport(s):
Dock / Drawer thresholds:
Viewport center tolerance:
No-extra-network evidence:
File line counts:
Light screenshot + SHA-256:
Dark screenshot + SHA-256:
Details screenshot + SHA-256:
Semantic + selection screenshot + SHA-256:
Narrow Drawer screenshot + SHA-256:
Known limitations:
```

该验收只证明Desktop布局与交互完成，不自动改变语义查询资格报告中关于known-negative、floor质量校准、
source / revision stale smoke或production multi-pod qualification的结论。
