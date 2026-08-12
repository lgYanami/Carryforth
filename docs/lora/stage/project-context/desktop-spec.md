# Project Context Desktop 产品规格

> 状态：产品设计已确认，待实现。
>
> 目标客户端：Buzz Desktop。
>
> 领域语义来源：[Project Context Edge V0 领域规范](./project-context.md)。
>
> 已交付后端边界参考：[Project Context Edge V0 后端实现设计](./implementation-design.md)。
>
> Desktop 信息架构更新：
> [Project Context 全画布与可折叠右侧工具栏分阶段实现计划](./desktop/project-context-full-canvas-workspace-implementation-plan.md)
> 取代本文 §8 中“顶部 Query Bar + Canvas + 右侧 Inspector”的固定位置描述。本文的领域、
> 查询、权限、selection 与 canonical Inspector 产品语义继续有效。
>
> 本文记录 Project Context 在 Desktop 中的产品语义、信息架构、图形表达、查询行为、
> Inspector、页面状态与首版边界。本文不重新定义领域或协议，不规定组件拆分、Tauri
> command、缓存结构、图库选择、具体视觉参数或阶段开发计划。Desktop 的实现设计与开发
> 计划另行编写。

## 1. 文档目的

Project Context 后端已经提供由 Human / Agent 显式维护的无向 Edge 与超边：一条 Edge
连接同一 Project 内两个或多个 Project View / Project Document 坐标，并由一份或多份
Project Document 承载解释性语义。Agent 可以按需使用 `exact`、`incident` 和
`contains-all` 查询 Edge，再读取所需 Document 正文。

Desktop 尚未提供 Human 可以直接观察这些结构的客户端。本文回答：

1. Human 从哪里进入当前 Project 的 Context 空间；
2. 二元 Edge、超边、重叠 Edge 和 Context Document 应如何准确画成图；
3. 三类查询如何改变当前图，而不改变领域状态；
4. 点击 Coordinate、Edge 和 Context Document 后看到什么；
5. 完整 Context 图中的互不连通部分如何表现为有意义的“岛”；
6. Desktop 如何呈现 tombstone、暂不可用、刷新和完整性失败；
7. 哪些能力属于 Desktop 首版，哪些继续由 Documents、Project View 或 Agent / CLI 承担。

本文不把视觉关系变成新的领域关系，也不让 Desktop 推断项目还缺少什么上下文。

## 2. 产品基础与领域边界

### 2.1 Project Context 是项目的二阶解释面

Project View 继续表达 Project 的一阶对象、状态和直接关系；Project Document 继续承载
可修订的 Markdown 内容；Project Context 则表达多个稳定坐标为什么需要被共同理解。

因此 Desktop 中三者的产品位置是并列且互相可导航的：

```text
Project Space
├── Overview / Project View   项目当前直接状态
├── Project Context           跨坐标解释性关系
└── Documents                 可修订内容
```

Project Context 不是 Project View 层级树的另一种布局，也不是 Documents 的分类页。

### 2.2 Context Edge 与 Context Reference 保持独立

现有 Project View 对象可以拥有 Context Reference，引用 Resource 或 Document。新
Project Context Edge 则连接一组准确的 Coordinate，并把一份或多份 Document 关联为该
Edge 的内容载体。

Desktop 必须保留这种区别：

- 不把 Context Reference 投影为 Edge；
- 不因建立 Edge 自动增加对象的 Context Reference；
- 不因删除 Context Reference 改变 Edge；
- 不提供二者之间的迁移、同步或合并视图；
- Project View Inspector 中现有的用户可见名称应明确为 `Context References`；
- 新图页面和入口使用 `Project Context`。

### 2.3 Desktop 只呈现可信状态

Desktop 展示的图结构必须来自完整验证的当前 Context 投影。客户端不能根据未经验证的
实时事件、局部分页或缓存残片自行拼出 Edge。

Context、Project View 和 Document 各自具有独立的 Revision / Generation 观察边界。
Desktop 可以把它们组合为可阅读界面，但不能声称它们形成一个不存在的跨领域全局
Revision。

### 2.4 图形入口不扩大权限

Project Context 继续复用当前 Community、Project View 与 Project Document 的既有权限。
图、查询、结果计数、Inspector 和 deep link 都不能因为“存在 Context 关系”而授予读取
权限，也不能泄露当前身份无权读取的坐标或正文。

复制或打开 Project Context 定位只恢复查询目标，不授予目标 Project、Coordinate 或
Document 的访问权。

## 3. 设计目标

Desktop 首版需要让当前 Community / Project 的成员能够：

1. 打开完整的当前 Project Context 图；
2. 准确区分 Coordinate、Context Edge 与 Context Document；
3. 准确阅读二元 Edge、超边和坐标集合重叠的多条 Edge；
4. 使用 `exact`、`incident` 和 `contains-all` 查询改变当前可见子图；
5. 点击 Project View Coordinate 查看对象内容；
6. 点击 Document Coordinate 查看当前 Document 内容；
7. 点击 Edge 查看准确坐标集合及其全部 Context Documents；
8. 只在需要时读取 Document Markdown 正文；
9. 在完整图中识别互不连通的 Context 岛及其规模；
10. 明确看到 tombstoned 或暂不可读取的 Coordinate，而不使 Edge 静默消失；
11. 在 Project View、Documents 与 Project Context 之间使用稳定定位往返；
12. 始终知道当前展示的是已验证状态、正在刷新状态还是失败状态。

## 4. 首版非目标

Desktop 首版不包含：

- 在画布上拖线创建 Edge；
- Context Document 的 attach / detach；
- 在 Project Context 页面内创建、编辑或删除 Document；
- 在 Project Context 页面内编辑或删除 Project View 对象；
- 拖动并持久化节点位置；
- 给 Edge 增加名称、方向、关系类型、状态、owner 或 maintainer；
- 给 Context 岛增加领域身份、名称、颜色字段或持久化布局；
- 从正文、对象关系、聊天或工作状态中自动推断 Edge；
- 自动判断 Context Gap、过期、冲突、错误、完整性或可信度；
- 自动建议哪些岛应该连接；
- 把 Context Reference 与 Context Edge 合并；
- Web 或 Mobile 客户端设计；
- 修改已经确认的 Project Context 领域、协议或权限模型。

Human 需要修改 Context Document 时，从 Inspector 明确进入现有 Documents 页面。Human
或 Agent 需要维护 Edge 时，继续使用已经存在的显式维护能力，直至后续单独设计 Desktop
写入交互。

## 5. 核心设计结论

### 5.1 独立的项目级页面

Project Context 使用独立页面，而不嵌入现有 Project View Map。首版稳定路由为：

```text
/project-context
```

它属于当前激活的 Community / Project，不在页面内部再选择 Project。

### 5.2 首版是只读的查询与阅读界面

Project Context 页面负责发现、观察和按需阅读。点击图中元素不会直接修改 Edge、
Coordinate 或 Document；所有写入都需要离开本页面或使用现有 Agent-facing 能力完成。

### 5.3 图是查询结果的呈现，不是第二份状态

画布中的节点位置、岛颜色、选中状态、缩放和动画都属于 Desktop presentation。它们不写回
Project Context，也不成为可供其他客户端依赖的项目事实。

三类查询的匹配结果必须与领域查询一致。Desktop 不能通过视觉邻近、标题搜索或本地启发式
增加、删除或重解释结果。

### 5.4 正文始终按需读取

初始图与查询结果只需要 Coordinate 状态、轻量标题 / 状态，以及 Context Document 的轻量
元数据。Document Markdown 在用户打开对应 Coordinate 或 Edge Document 后才读取。

多份 Context Document 保持独立，不在界面中自动拼接为一份虚构正文。

## 6. 用户可见概念

| 领域 / 图形概念 | Desktop 含义 |
|---|---|
| Coordinate | Project View 对象或 Project Document 的稳定项目坐标 |
| Coordinate Node | 图中代表一个真实 Coordinate 的可点击节点 |
| Context Edge | 连接准确无序坐标集合的领域 Edge |
| Edge Hub | Context Edge 在图中的视觉汇合点与点击目标；不是 Coordinate |
| Spoke | Edge Hub 与 Coordinate Node 之间的无向视觉线段 |
| Context Document | 关联在 Edge 下、承载解释性语义的普通 Project Document |
| Context Island | 完整当前 Context 图中的一个连通分量；只是派生展示 |
| Query Anchor | 当前查询明确选择的 Coordinate；只是查询强调状态 |

用户无需理解 Nostr kind、tag、投影事件或 canonical hash。Edge key、Revision、Generation
等诊断信息可以在 Inspector 或页面状态中按需显示，但不应取代内容标题和类型。

## 7. 导航与定位

### 7.1 Project Space 入口

当前 Community 的项目入口顺序为：

```text
Overview
Project Context
Documents
```

`Project Context` 与 `Documents` 同为 Project Space 的直接入口。进入 Context 页面时，
默认打开当前 Project 的完整 Context 图。

### 7.2 跨页面入口

以下页面应能够定位到 Project Context：

- active Project View 对象：`Show in Project Context`，打开该坐标的 `incident` 查询；
- active Project Document：`Show in Project Context`，打开该坐标的 `incident` 查询；
- Context Coordinate Inspector：`Open in Project View` 或 `Open in Documents`；
- Edge Inspector 中的 Context Document：`Open in Documents`。

进入目标页面只改变 Human 当前导航位置，不改变 Edge 或 Document。

### 7.3 可恢复查询与选择

当前查询类型、查询坐标和选中的 Coordinate / Edge 应可以随页面定位恢复，并支持前进、
后退和可复制定位。

画布平移、缩放、临时聚焦和动画进度不属于稳定定位，不需要成为链接的一部分。

### 7.4 Community 切换

切换 Community 就是切换 Project：

- 清除旧 Project 的 Query Anchor、选择和 Inspector；
- 不把旧 Project 的 Coordinate token 应用于新 Project；
- 不复用旧 Project 的岛颜色、图布局或内容缓存；
- 新 Project 重新进入其默认完整 Context 图；
- 返回原 Project 时重新确认当前可信状态，不假设旧图仍然有效。

## 8. 页面信息结构

宽屏页面采用“查询栏 + 图画布 + 右侧 Inspector”的结构：

```text
┌─────────────────────────────────────────────────────────────────────────┐
│ Project Context                 Verified · Live · Context revision      │
├─────────────────────────────────────────────────────────────────────────┤
│ All Context | Exact | Incident | Contains all   [Coordinates]   [Run]  │
│ 2 islands · 7 coordinates · 4 edges · 6 context documents              │
├──────────────────────────────────────────────────┬──────────────────────┤
│                                                  │ Coordinate / Edge    │
│                 Interactive graph                │ Inspector            │
│                                                  │                      │
│  ╭──────── Island 1 ───────╮   ╭── Island 2 ──╮ │ details              │
│  │ [A] ── ◇E1 ── [B]      │   │ [D]─◇E3─[E] │ │ documents            │
│  │          └──── [C]      │   ╰──────────────╯ │ current Markdown      │
│  ╰─────────────────────────╯                     │ open in source page   │
│                                                  │                      │
├──────────────────────────────────────────────────┴──────────────────────┤
│ Fit all · Fit island · Zoom controls · verified observation details    │
└─────────────────────────────────────────────────────────────────────────┘
```

该示意图只固定信息关系，不规定最终尺寸、控件形状或像素布局。

### 8.1 页面 Header

Header 至少表达：

- 页面名称 `Project Context`；
- 当前结果已经验证；
- live / reconnecting / stale 等刷新状态；
- 当前 Context Revision 的轻量提示；
- 手动刷新入口。

低层 Relay signer、Projection Generation 与各来源观察 Revision 可以按需展开，不要求持续
占用主要视觉空间。

### 8.2 Query Bar

Query Bar 包含：

- `All Context`；
- `Exact`；
- `Incident`；
- `Contains all`；
- Coordinate picker；
- 已选择 Coordinate chips；
- `Run` 与清除 / 返回完整图操作；
- 当前结果数量摘要。

`All Context` 是 `contains-all({})` 的用户友好名称，不建立第四种领域查询。

### 8.3 Graph Canvas

Graph Canvas 是页面主体，支持：

- 平移与缩放；
- Fit all；
- Fit island；
- Coordinate、Edge 与 Query Anchor 的键盘定位；
- 点击或键盘激活后打开 Inspector；
- 查询切换后的稳定重排；
- 在不混淆状态的前提下保留上一份可信图进行刷新。

首版节点不能拖动或连线，避免临时视觉操作被误解为领域修改。

### 8.4 Inspector

宽屏使用右侧面板，窄屏使用抽屉或单面板内容。打开 Inspector 不应丢失当前查询和画布
上下文；关闭后焦点返回触发它的 Coordinate 或 Edge。

## 9. 图形语义

### 9.1 Coordinate 是真实节点

首版只绘制两类真实节点：

1. Project View Object Coordinate；
2. Project Document Coordinate。

Project View 对象类型通过图标和 Type Badge 区分。Document Coordinate 使用一致的
Document 标识。节点主标题来自当前可用的轻量内容；标题不可用时显示对象类型和稳定 ID，
不能伪造名称。

### 9.2 所有 Edge 统一使用 Edge Hub

二元 Edge 与超边都使用同一套 incidence graph 表达：

```text
binary Edge {A, B}

[A] ───── ◇E1 ───── [B]

hyperedge {A, B, C}

[A] ─────┐
[B] ─────◇E2
[C] ─────┘
```

`◇E1` / `◇E2` 是 Edge 的视觉点击目标，不是领域节点。Spoke 不独立拥有含义，也不能被
解释为 A→B、A→C 等二元 Edge。

画布不使用箭头。节点在左、右、上、下的位置只是布局结果，不表达 source、target、先后、
因果或重要性。

### 9.3 重叠坐标集合仍是独立 Edge

`{A,B}` 与 `{A,B,C}` 必须同时画成两个独立 Edge Hub：

```text
[A] ── ◇E1 ── [B]          E1 = {A, B}
  └──── ◇E2 ───┴── [C]     E2 = {A, B, C}
```

客户端不能把超边拆成多条二元 Edge，也不能因为 Edge 共享多个 Coordinate 而合并它们。

### 9.4 Context Document 不是隐式 Coordinate

Edge Hub 可以显示关联 Context Document 的数量，但 Document 不因此自动成为该 Edge 的
Coordinate Node。

同一份 Document 可能：

- 在 Edge X 中作为 Context Document；
- 在 Edge Y 中作为明确 Document Coordinate；

只有第二种结构角色会让它在 Edge Y 上成为图节点。这两个角色不能在渲染时自动合并或
传播。

### 9.5 图只显示查询范围

完整 Context 图显示所有 active Edge 及其 Coordinate 并集，不显示没有参与任何 Edge 的
Project View 对象或 Document。

这样可以避免把孤立对象错误暗示为“缺少 Context”。Coordinate picker 仍可以提供 active
Project View 对象与 Document，供 Human 主动查询。

具体查询没有匹配 Edge 时，画布可以保留 Query Anchor 来解释“0 条匹配结果”，但这些
Anchor 不构成 Context Island，也不能被标成 Gap。

## 10. Context Island

### 10.1 定义

Context Island 是完整当前 Context incidence graph 的一个连通分量：

> 岛内任意两个 Coordinate / Edge Hub 之间存在由 Spoke 组成的路径；不同岛之间不存在
> 这样的路径。

计算连通性时：

- 两条 Edge 共享 Coordinate，则进入同一岛；
- Context Document binding 本身不连接两个岛；
- tombstoned Coordinate 继续连接原 Edge；
- 暂不可读取但身份仍可验证的 Coordinate 继续按稳定身份连接；
- Query Anchor 若没有匹配 Edge，不形成岛。

### 10.2 岛只在完整图中表达项目级事实

Project 级岛数量只在 `All Context = contains-all({})` 的完整结果中成立。

这是由查询语义直接决定的：

- `exact(Q)` 最多返回一条 Edge；
- `incident(A)` 的所有结果共享 A；
- 非空 `contains-all(Q)` 的所有结果共享 Q；
- 因此，上述查询存在结果时天然连通；
- 只有完整 Context catalog 可能产生多个互不连通的分量。

聚焦查询中的局部布局不能被描述成 Project 级岛统计。

### 10.3 岛的视觉表达

每个岛使用一套可区分但克制的视觉主题：

- 浅色半透明背景、柔和边界或类似地形的围合区域；
- Coordinate Node 的小面积 accent；
- Edge Hub、Spoke 和选中轮廓的同组 accent；
- 岛之间保留明确留白；
- 每个岛显示当前视图内的编号和事实计数；
- 同时可见的岛不能只靠相近颜色区分。

岛标签可以使用：

```text
Island 1 · 4 coordinates · 3 edges · 5 context docs
```

`Island 1` 是当前视图的展示编号，不是稳定身份。颜色、编号与围合形状不写入领域状态。
其中 Document 计数只表示岛内 Edge 关联的 Context Documents，不表示 Document
Coordinate Node 数量。

颜色只编码“属于同一个连通分量”，不编码项目主题、重要性、健康度、维护者或语义类别。
相同完整图重复打开时，颜色与排序应保持稳定；图拓扑改变后允许重新计算。

### 10.4 岛导航

完整图提供岛摘要和快速聚焦：

```text
2 context islands · 7 coordinates · 4 edges
[Island 1 · 3 edges] [Island 2 · 1 edge] [Fit all]
```

点击岛标签只调整当前 viewport。它不改变查询，不选中全部对象，也不形成新的领域操作。

### 10.5 拆分与合并

当前 Edge 变化可能使岛发生变化：

- 新增一条跨岛 Edge 后，两个岛合并；
- 移除最后一条连接路径后，一个岛可能拆分；
- tombstone Coordinate 不会自动切断路径；
- Context Document 正文变化不会改变岛。

Desktop 应在下一份可信图中重新计算岛，并通过克制的布局过渡帮助 Human 理解变化。过渡
动画不具有领域含义，并必须尊重 reduced-motion 设置。

### 10.6 不把岛解释为 Gap

Desktop 可以客观表达：

```text
The current Project Context contains 2 disconnected components.
```

Desktop 不能表达：

- 两个岛“应该”连接；
- 项目“缺少”某条 Context；
- 已经检测到 Context Gap；
- 某个岛过期、错误、不完整或不可信；
- 较大的岛更重要或更健康。

是否需要建立跨岛 Edge，只能由理解实际项目语义的 Human / Agent 判断。

## 11. 查询体验

### 11.1 默认完整图

直接进入 Project Context 页面时，默认执行：

```text
contains-all({})
```

界面名称为 `All Context`。它返回所有当前 active Edge，但不加载所有 Document Markdown。

### 11.2 Exact

`Exact` 要求至少两个不同 Coordinate：

```text
exact({A, B})
```

结果为：

- 精确坐标集合存在时的一条 Edge；或
- 明确的 0 条匹配结果。

它不得返回 `{A,B,C}`，也不得因输入顺序不同得到不同结果。

### 11.3 Incident

`Incident` 要求一个 Coordinate：

```text
incident(A)
```

结果包含所有拥有 A 的二元 Edge 与超边。A 作为 Query Anchor 被明确突出，但画布不把 A
解释为 source 或中心领域对象。

### 11.4 Contains all

`Contains all` 接受一组 Coordinate：

```text
contains-all({A, B})
```

结果包含所有同时拥有 A 和 B 的 Edge，包括 `{A,B}`、`{A,B,C}` 等超集。它不得返回只
包含部分查询坐标的 Edge。

空集合通过 `All Context` 呈现；单坐标输入虽然与 `incident` 等价，界面仍可以保留领域
允许的集合语义。

### 11.5 Coordinate picker

Picker 应按来源区分：

- Project View Objects；
- Documents。

每一项至少显示类型、标题和必要状态。选中的 Coordinate 以无方向 chips 表达，并使用
规范稳定顺序，不能通过选择先后暗示 Edge 方向。

Picker 主要列出当前 active Coordinate。已经从可信 Context 图中发现的 tombstoned 或
暂不可读取 Coordinate 仍可以从节点或 Inspector 加入查询，客户端不能仅因为来源 catalog
不再 active 就拒绝查询已有 Edge。

### 11.6 显式执行与结果切换

修改模式或 chips 先形成查询草稿；点击 `Run` 后才替换当前图，避免每次选择都触发昂贵
刷新和布局跳动。

查询切换后：

- 仍存在于新结果中的当前选择可以保留；
- 不再存在的 Coordinate / Edge 选择必须关闭；
- viewport 适配新结果；
- 新查询失败时不能把旧查询结果伪装成新查询结果；
- 返回 `All Context` 后重新显示完整图与项目级岛统计。

### 11.7 从节点继续查询

点击 Coordinate 的首要行为是查看内容，不自动改变当前查询。Coordinate Inspector 可以
提供明确的 `Show incident Context` 操作，由 Human 主动切换为该坐标的 Incident 查询。

## 12. 有意义的视觉编码与布局

### 12.1 视觉编码原则

不同视觉通道只表达已经存在的事实：

| 视觉表达 | 允许表达的事实 |
|---|---|
| 岛主题色 / 围合区域 | 当前完整图中的连通分量 |
| 节点图标与 Type Badge | Coordinate 来源与 Project View 对象类型 |
| Edge Hub badge | 关联 Context Document 数量 |
| Query Anchor halo | 当前查询显式选择的 Coordinate |
| 选中轮廓与淡化 | 当前查看目标及其直接图范围 |
| 虚线 / tombstone 标识 | Coordinate 当前已 tombstoned |
| unavailable 标识 | 当前内容暂不可读取 |
| Verified / stale 状态 | 当前画面可信读取与刷新状态 |

Document 数量不通过 Edge 粗细表达“强度”；岛面积不表达重要性；节点位置不表达方向。

### 12.2 确定性布局

相同可信结果应得到稳定布局，避免刷新时随机漂移。布局应优先：

1. 保持每个岛内部关系可读；
2. 减少 Spoke 交叉；
3. 给 Edge Hub 和 Coordinate label 留出命中与阅读空间；
4. 将不同岛分开排列；
5. 在聚焦查询中突出 Query Anchor、匹配 Edge 与额外 Coordinate；
6. 在结果变化时使用短暂、可关闭的过渡，而不是持续运动。

允许根据查询模式选择不同的确定性布局，但任何横向或环形层次都只是阅读组织，不是领域
方向。

### 12.3 选择与聚焦

选择 Coordinate 时：

- 高亮该节点；
- 在当前结果内高亮其 incident Edge Hub 与 Spoke；
- 其他内容可以适度淡化；
- 打开 Coordinate Inspector。

选择 Edge 时：

- 高亮唯一 Edge Hub、全部 Spoke 和准确 Coordinate 集合；
- 不高亮未属于该 Edge 的重叠邻接关系；
- 打开 Edge Inspector。

点击空白区域可以关闭选择，但不清除查询。

### 12.4 可访问性

图不能只靠颜色传递信息：

- Coordinate 类型同时使用图标、文字和可访问名称；
- Island 同时使用边界、编号和计数；
- tombstone / unavailable 同时使用图形、文字和状态说明；
- Coordinate 与 Edge Hub 可以通过键盘聚焦并使用 Enter / Space 打开；
- 聚焦顺序稳定且与当前布局相符；
- 缩放不能破坏可读文字的 Desktop zoom 行为；
- 动画尊重 reduced-motion；
- Inspector 关闭后恢复合理焦点。

## 13. Coordinate Inspector

### 13.1 Project View Object Coordinate

active 对象至少展示：

- 对象类型；
- 标题或名称；
- 当前显式状态 / 优先级；
- 对象正文和主要直接关系；
- 当前 Revision、修改者与时间的轻量信息；
- 稳定对象 ID；
- `Open in Project View`；
- `Show incident Context`。

Inspector 是紧凑只读内容面，不复制完整 Project View 编辑器。

### 13.2 Document Coordinate

active Document 至少展示：

- 标题；
- Summary；
- 当前 Document Revision；
- 修改者与时间；
- 按需读取的当前 Markdown；
- 稳定 Document ID；
- `Open in Documents`；
- `Show incident Context`。

Inspector 不在图页面内提供编辑、历史切换或删除。

### 13.3 Tombstoned Coordinate

tombstoned Coordinate 继续显示在 Edge 中。Inspector 至少展示：

- Coordinate 类型与稳定 ID；
- `Tombstoned` 状态；
- 可验证时的删除 Revision、操作者与时间；
- 它仍属于哪些当前查询结果 Edge。

客户端不能因当前 Project View / Document active catalog 找不到它而关闭整个 Edge，也不能
导航到 active 编辑页面并假装对象仍存在。

### 13.4 Unavailable Coordinate

暂不可读取时，Inspector 保留稳定身份与 Edge 成员关系，并说明当前无法取得内容。它与
tombstone、权限拒绝、完整性失败和 Context Gap 都是不同状态。

## 14. Edge Inspector

点击 Edge Hub 或其 Spoke 后，Inspector 至少展示：

- `Context Edge` 标题；
- 完整、无序的 Coordinate 集合；
- 每个 Coordinate 的类型、标题 / ID 和生命周期状态；
- Context Document 数量；
- Context Document 列表；
- 当前选择 Document 的标题、Summary、Revision、修改者与时间；
- 按需读取的当前 Markdown；
- 完整 Edge key 的诊断入口；
- 每份 Document 的 `Open in Documents`。

多份 Context Document 使用明确列表或切换器：

- 默认选择第一份可用 Document；
- 每次只展示一份正文；
- 切换时按需读取；
- 不合并正文；
- 不把 Document 标题提升成 Edge 名称；
- Document 暂不可用时保留其列表项和错误状态。

Edge 的 Spoke 都属于同一个 Edge 点击范围。点击某一段 Spoke 不应只显示该段两端的虚构
二元关系。

## 15. 页面状态与可信刷新

### 15.1 Capability 与可用性

页面必须区分：

- 当前 Relay / Project 不支持所需 Project Context Edge 能力；
- Project View 或 Documents 前置能力未就绪；
- Context 尚不可读取；
- 当前身份无权读取；
- Context 已初始化但没有 active Edge；
- Context capability 当前关闭，但已有可信投影仍允许只读观察；
- 当前图已成功加载。

现有 Context Reference capability 不能被当作 Context Edge capability。

当前 capability 广告不是只读可用性的唯一判断：如果 capability 已关闭，但同一 Project
仍存在可验证的 Context 投影，Desktop 继续允许只读观察并明确显示该状态；如果不存在可
验证投影，则不能据此构造空图。

### 15.2 加载与刷新

首次加载可以显示图骨架或明确的 verifying 状态。已有可信图刷新时：

- 保留上一份同 Project、同查询的可信结果；
- 显示 refreshing / reconnecting / stale；
- 不把未验证实时事件直接加入图；
- 新可信结果整体替换旧结果；
- 不在分页过程中逐条长出可能不完整的 Edge。

查询本身变化时，旧查询结果不能作为新查询的占位内容。

### 15.3 空状态

必须区分：

1. 完整 Context catalog 没有 active Edge；
2. 当前 `exact` / `incident` / `contains-all` 没有匹配 Edge；
3. Context 不可用或尚未初始化；
4. 当前身份无权读取。

空 catalog 可以说明“当前没有已记录的 Context Edge”，但不能说明项目缺少上下文。查询
无结果只说明当前没有匹配该集合条件的 Edge。

### 15.4 Hydration unavailable

若 Edge 结构已经验证，但某个 Coordinate 或 Document 的当前轻量内容暂不可读取：

- 保留 Edge；
- 保留 Coordinate / Document 稳定身份；
- 显示 unavailable 状态；
- 不把 unavailable 改写成 tombstone；
- 不把它解释为语义 Gap。

### 15.5 完整性失败

若客户端发现签名、Project、Generation、Edge key、准确坐标集合、Document binding 或
来源状态存在已验证矛盾，必须 fail closed：

- 不显示局部拼接图；
- 不隐藏矛盾对象后继续展示；
- 明确显示 Context verification failed；
- 允许重新验证；
- 保留诊断信息但不暴露不必要的底层响应正文。

### 15.6 Context 与内容独立变化

Context Document 正文更新不会改变 Edge 拓扑或 Context Revision，但页面仍应在 Document
来源变化后刷新其轻量元数据与当前打开正文。

Project View 对象内容变化同样不自动改变 Edge。图中标题 / 状态可以更新，Edge 坐标与岛
结构保持不变，除非 Context binding 本身发生变化。

## 16. 响应式 Desktop 行为

### 16.1 宽屏

宽屏同时显示 Graph Canvas 与可调整宽度的右侧 Inspector。打开 Inspector 后画布可缩小，
但当前选择必须保持可见或可以一键重新聚焦。

### 16.2 窄屏

窄屏使用右侧抽屉或单面板 Inspector：

- 图仍是主页面；
- 打开内容时不重新执行查询；
- 关闭后回到原节点 / Edge 的画布位置；
- Query Bar 可以折叠 Coordinate picker，但当前模式与已选 chips 必须可见；
- 不要求在极窄尺寸同时完整展示图与正文。

### 16.3 缩放与可读性

图节点中的可读文字遵守 Desktop 现有文本缩放。画布 zoom 与应用文本 zoom 是不同操作，
界面必须避免将二者混为一个无边界缩放手势。

## 17. 首版产品边界总结

Project Context Desktop 首版交付以下完整闭环：

```text
进入 Project Context
    ↓
查看完整 verified Context 图与 Context Islands
    ↓
运行 exact / incident / contains-all
    ↓
点击 Coordinate 或 Edge
    ↓
按需读取 Project View 内容或 Document Markdown
    ↓
需要维护时进入 Project View / Documents 或使用现有 Agent 能力
```

页面自身不承担 Edge 写入。这是明确的首版产品边界，不代表 Human 永久不能通过 Desktop
维护 Context；写入交互需要在后续设计中单独处理坐标选择、Document 创建 / 选择、显式
attach / detach、并发冲突和删除保护。

## 18. 产品验收标准

Desktop 产品实现只有同时满足以下条件，才符合本规格：

1. Project Context 是当前 Project Space 的独立入口，不嵌入或替换 Project View Map。
2. 直接进入页面默认展示 `contains-all({})` 的完整 active Edge 图。
3. Coordinate 是真实节点，Edge Hub 只是无向 Edge / 超边的统一视觉表达。
4. `{A,B}` 与 `{A,B,C}` 始终显示为两条独立 Edge，不发生拆分或合并。
5. Context Document 不会仅因 binding 自动成为 Coordinate Node。
6. `exact`、`incident`、`contains-all` 的可见结果与领域集合语义一致。
7. 完整图只绘制 active Edge 的 Coordinate 并集，不用孤立对象暗示 Context Gap。
8. 完整图能够识别、分色、围合并导航多个 Context Islands。
9. 岛颜色只表达连通分量，并同时使用编号、边界和事实计数，不能只靠颜色识别。
10. 项目级岛统计只出现在完整图，不从局部查询推断 Project 全局结构。
11. 点击 Coordinate 可以查看当前内容；点击 Edge 可以查看准确坐标集合与全部 Context
    Documents。
12. Document Markdown 只在 Human 明确打开后按需读取，多份正文不自动合并。
13. tombstoned / unavailable Coordinate 保留在 Edge 中并具有不同的明确视觉状态。
14. 页面不产生 Gap、过期、冲突、重要性或“应该连接”的自动判断。
15. 未验证实时事件、混合分页或来源矛盾不能形成局部可见图。
16. 刷新、重连、空 catalog、查询无结果、无权限、不可用与完整性失败均有独立状态。
17. 查询、图、结果计数、Inspector 与 deep link 均不扩大权限或泄露无权内容。
18. Community 切换不会泄漏旧 Project 的查询、选择、布局或内容。
19. Project Context 页面不提供拖线、attach、detach、正文编辑或节点位置持久化。
20. 现有 Context References 保持独立，并在用户可见命名上避免与 Project Context Edge
    混淆。
21. 键盘、颜色替代信息、焦点恢复、Desktop 文本缩放和 reduced-motion 均得到支持。
