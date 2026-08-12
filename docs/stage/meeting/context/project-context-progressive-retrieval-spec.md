# Project Context Agent 渐进检索设计规范

> 状态：待重写，部分核心结论已经失效，不得作为实现依据
>
> 失效日期：2026-08-09
>
> 本文不是实现设计，不定义 event kind、wire schema、数据库、索引、事务、CLI、UI、
> 具体 Prompt 文本、模型调用方式或 Meeting 流程。

本文仍保留统一 Project Context Graph、Role / Work 作为检索视角、Agent 自主渐进遍历以及
Edge Context Document 按需读取等有效讨论。但其中把 `CoordinateNode.summary` 定义为图层独立摘要、
在 Coordinate 入图时生成并由 Project Context 维护的章节已经失效。最新结论是：摘要的语义与生命
周期属于它所描述的内容实体；查询层只负责读取和水合。完整变更记录见
[Meeting 上下文讨论历程](meeting-context-discussion-history.md)的第 17～20 节。

此前形成的
[已废弃的 Project Context Agent 渐进检索实现设计](project-context-progressive-retrieval-implementation-design.md)
已于 2026-08-09 废弃，只保留用于追溯，不再是实现依据。废弃原因与后续方向记录在
[Meeting 上下文讨论历程](meeting-context-discussion-history.md)中；新的实现设计将另行撰写。

## 1. 文档目的

本文源自 [Meeting 上下文讨论历程](meeting-context-discussion-history.md)，但所定义的能力不是
Meeting 子机制，而是一项独立、通用的 Project Context 能力：

> 任意 Agent 面对一个问题时，结合自己的 Role、Work、其他项目坐标和当前 Runtime Context，
> 在统一 Project Context Graph 上先查看轻量描述，再选择性加载坐标完整内容或关系完整内容，
> 并沿真实图关系继续探索，直到取得当前问题所需的上下文。

Meeting 只是潜在调用方之一。对 Meeting 而言，议题、开会目标及涉及的 Work、Requirement、
Issue 等组成问题输入，participant 自身的 Role、Work 和其他坐标组成 Agent 上下文环境。

本设计不创建 Meeting Context、Role Context、Agent 私有上下文仓库或第二套项目事实。检索结果
只是 Agent 在当前 Runtime 中针对当前问题动态激活的上下文。

## 2. 设计背景与已经形成的结论

[Project View](../../project-view/project-view.md) 已经提供 Project 的一阶当前状态和稳定坐标。
[Project Context](../../project-context/project-context.md) 已经使用真实的无向 Edge / Hyperedge
连接 Project View 对象、Project Document 和 Meeting 坐标，并由 Context Document 解释一组坐标
为什么共同相关。

此前讨论依次排除了：

- 为 Meeting 人工拆分同一份 Project Context；
- 为不同 Role 建立独立 Context 仓库；
- 通过 ACL 隔离上下文来制造 Agent 差异；
- 新建 Claim、Evidence、Epistemic Record 等第二套认知模型；
- 让一套图引擎预先替 Agent 固化语义路径；
- 让所有 Agent 一次性加载相同的完整项目上下文。

最终形成的判断是：

1. 所有 Agent 继续使用同一 Project Context Graph 和同一 Project 事实；
2. Role 是 Agent 检索同一张图时的必要视角；Work 和其他坐标是存在时的附加视角和可选起点；
3. 不同 Agent 的上下文差异来自 `problem + Role + Work + Runtime Context` 形成的实际检索路径；
4. 图负责提供真实、轻量、有界和可追溯的候选；
5. Agent 通过稳定 Prompt 契约判断哪些候选值得加载，以及下一步沿哪里检索；
6. 坐标指向的完整内容和 Edge Context Document 都属于上下文，二者都是一等检索目标。

## 3. 设计范围与边界

### 3.1 本文定义

本文定义：

- 通用渐进检索的输入；
- `CoordinateNode` 与现有 `CoordinateRef`、Context Edge 的概念关系；
- `CoordinateNode.summary` 的语义、生成规范和乐观生命周期；
- 坐标内容上下文与关系内容上下文两类检索目标；
- 图能力与 Agent Prompt 的职责边界；
- Agent 渐进检索的逻辑动作、过程、停止语义和输出；
- 必须保持的领域与行为不变量。

### 3.2 本文不定义

本文不定义：

- 使用独立图数据库、关系数据库、内存图或其他存储；
- 全文、向量、混合搜索或图算法的选择；
- 排序公式、相关度评分、预计算路径或静态语义权重；
- 分页大小、跳数、token、时间和正文读取量的具体数值；
- `CoordinateNode`、Edge Preview 或 Retrieved Context 的 wire / API 结构；
- 具体 CLI 命令、UI 交互、event kind、数据库表和迁移；
- 完整 Prompt 文案、模型选择和推理调用方式；
- Retrieved Context 如何进入另一个 ACP Session 或 Meeting Session；
- Meeting 的 participant 选择、调度、入场、发言或 Board 协议。

## 4. 核心概念模型

### 4.1 统一 Project Context Graph

Project Context 继续是唯一的持久项目上下文图。它不按 Role、Agent、Meeting 或 Session 拆分。

本设计在现有稳定坐标和 Hyperedge 语义之上，引入一个逻辑上的节点检索描述：

```text
CoordinateNode
├── coordinate: CoordinateRef
└── summary: RetrievalSummary
```

这里的 `CoordinateNode` 是本文定义的期望概念模型，不表示当前代码中已经存在同名结构。

### 4.2 CoordinateRef 与 CoordinateNode

`CoordinateRef` 继续是 Project-scoped 稳定身份：

```text
Coordinate identity
  = coordinate family / object type
  + stable object id
  + Project scope
```

对象标题、内容、状态、Revision 和 `CoordinateNode.summary` 的变化均不改变 Coordinate identity。

同一 Project 中，同一个 CoordinateRef 逻辑上只有一份节点摘要。一个坐标进入多条 Edge 时复用
同一份摘要，不为每条 Edge 创建不同副本。某条 Edge 特有的语义继续由该 Edge 的 Context
Document 表达。

`summary` 不参与：

- 坐标判等；
- 坐标规范化；
- Edge 坐标集合；
- EdgeKey；
- Edge 唯一性。

Edge 仍由 Project 与规范化后的精确 Coordinate identity 集合定义。

### 4.3 CoordinateNode 不拥有坐标完整内容

CoordinateNode 只提供图层检索描述，不复制坐标指向的 canonical 完整内容。

完整内容继续由原领域实体拥有：

- Role、Goal、Plan、Stage、Requirement、Issue、Work、Resource 等由 Project View 拥有；
- Project Document 正文由 Project Document 领域拥有；
- Meeting metadata、Board 和 Speech 由 Meeting 领域拥有。

Project Context 查询通过 CoordinateRef 提供读取这些原内容的能力，但不把它们复制为新的
Project Context 内容实体。

本文中的“canonical 完整内容”是指来源领域当前定义的、描述该 Coordinate 自身的 canonical
内容范围。它不默认包括全部历史 Revision、相邻对象、相邻 Edge 或 Edge Context Document；不同
坐标类型具体由哪些读取面共同构成其当前内容，属于来源领域和后续实现设计。

### 4.4 Coordinate Preview

渐进检索首先取得统一、轻量的坐标预览。概念上至少包括：

```text
CoordinatePreview
├── coordinate
├── title
├── summary
├── lifecycle / status metadata
└── full-content read capability
```

`title` 说明“它叫什么”，`summary` 说明“其中大致有什么、何时可能值得加载”。标题从原对象
解析、投影或以其他方式提供的具体设计不由本文规定。

只有已经显式进入 Project Context Graph 的坐标才要求存在 CoordinateNode 摘要。本设计不要求
把所有 Project View 对象自动加入 Project Context Graph，也不创建没有任何图关系的逻辑孤立节点。

一个 CoordinateRef 出现在至少一条当前 Context Edge 中时，它是图中的逻辑 CoordinateNode。最后
一条包含它的 Edge 消失后，它离开当前图，后续原内容更新不再承担本设计规定的摘要维护责任。
该 Coordinate 以后重新进入图时，写图 Agent 必须重新读取其当前 canonical 内容，并保证当前摘要
准确；实现可以缓存旧描述，但不能未经判断直接把旧描述视为当前摘要。

### 4.5 Context Edge 与 Edge Context Document

现有 Context Edge 语义保持不变：

```text
ProjectContextEdge
├── coordinates: Set<CoordinateRef>       2..*
└── context_documents: Set<document_id>   1..*
```

Edge 仍然：

- 是无向 Edge / Hyperedge；
- 表达“这组精确坐标共享上下文”的结构事实；
- 不自动拥有方向、关系类型、权重或相关度；
- 不隐式拆成多个二元 Edge；
- 由一份或多份普通 Project Document 承载关系解释。

Edge Preview 中 Context Document 的 title / summary 来自该 Project Document 自身的轻量 metadata，
不是 `CoordinateNode.summary`。如果同一份 Document 另外作为 Coordinate 出现在图中，它还拥有一份
用途不同的 CoordinateNode summary：前者预览关系解释，后者描述该 Document 坐标本身何时值得加载。

### 4.6 两类并列的上下文内容

渐进检索面对两类并列的一等上下文目标：

```text
坐标内容上下文
  Role / Work / Issue / Requirement / Document / Meeting / ...

关系内容上下文
  Edge 所关联的 Context Document
```

坐标内容回答“这个项目对象、文档或会议本身包含什么”。Context Document 回答“这些坐标为什么
共同相关”。二者用途不同，但没有固定读取先后关系。

Agent 可以：

- 只读取某个 Coordinate 指向的完整内容；
- 只读取某条 Edge 的 Context Document；
- 同时读取两者；
- 只根据轻量预览沿 Coordinate 继续发现候选，而暂不加载完整内容。

读取 Context Document 不是读取 Coordinate 完整内容的前置条件；Coordinate 也不只是通向
Context Document 的导航点。

Project Document 可能同时承担两种独立角色：作为 Coordinate 时，其正文是坐标内容；作为某条
Edge 的 Context Document 时，其正文解释该 Edge。两种角色不得混淆。

### 4.7 Retrieved Context

Retrieved Context 是：

> Agent 针对当前问题，经过渐进检索后实际选择并加载的一组坐标完整内容和关系完整内容。

它：

- 是当前 Runtime 的临时工作集；
- 不是新的领域对象或持久上下文层；
- 不拥有另一份正文或 Revision；
- 不自动写回 Project Context；
- 不等同于答案本身；
- 应保留每项内容的稳定来源；在来源领域支持时同时保留实际读取版本及真实发现路径。

## 5. 通用检索输入

概念输入为：

```text
RetrievalRequest
├── problem
│   ├── description                 required
│   └── involved_coordinates[]      optional
│
└── agent_environment
    ├── role                         required
    ├── works[]                      optional
    └── other_coordinates[]          optional
```

### 5.1 Problem

`problem.description` 必须存在，用自然语言说明需要理解、调查、判断或解决的问题，也可以包含期望
结果或目标，例如定位原因、评估风险、形成方案、验证 Requirement 或理解一项历史决定。

问题已经涉及明确的 Work、Issue、Requirement、Document、Meeting 或其他项目坐标时，应将这些
坐标作为 `involved_coordinates` 提供。自然语言问题本身不是 Coordinate，也不会因为一次检索而
自动写入图。

### 5.2 Agent Context Environment

Role 必须存在，并且必须是当前 Project 中可验证、可读取的 canonical Role，而不是调用方任意提供的
文本标签。它为 Agent 提供 purpose、responsibilities、boundaries 等必要语义视角，但 Role：

- 不是硬过滤器；
- 不是新的 ACL；
- 不限制跨 Role 内容被发现；
- 不把检索结果变成 Role 专属事实；
- 不要求 Role 本身必须已经作为 CoordinateNode 进入图。

Agent 当前 Work 和其他已知项目坐标可以作为额外语义视角和检索起点。Agent 当前 LLM Runtime
已经持有的上下文自然参与判断，但不因此被持久化成 Role Context 或 Session Context 对象。

## 6. CoordinateNode.summary

### 6.1 摘要定义

`CoordinateNode.summary` 是：

> 由 Agent 在坐标首次进入 Project Context Graph 时，基于该坐标当前 canonical 完整内容显式生成
> 的、Project-scoped、Role-neutral、task-neutral 的轻量检索描述，用来帮助未来 Agent 判断是否
> 值得加载该坐标的完整内容。

它类似 Skill description，回答两个问题：

```text
这份坐标内容大致包含什么？
在什么类型的问题下可能值得加载？
```

它不回答：

```text
完整事实、参数和最终结论是什么？
这个坐标为什么与当前 Edge 相关？
未来 Agent 应该执行什么操作？
```

摘要是图层的检索索引。它不替代原内容，不因存在于图中而成为高于原内容的事实来源。

### 6.2 生成前提

Agent 首次使用一个此前不在当前图中的 Coordinate 建立或关联一条真实 Context Edge 时，必须：

1. 读取该 Coordinate 当前可用的 canonical 完整内容；
2. 根据完整内容生成摘要；
3. 在同一逻辑写入中保证该 CoordinateNode 摘要与合法的 Edge 关联同时成立。

本文不允许先建立逻辑孤立节点，也不允许给既有 Hyperedge “增加坐标”。`{A,B}` 与 `{A,B,C}`
仍是不同 Edge；坐标范围变化必须继续遵守现有精确坐标集合语义。

Agent 不得：

- 只根据标题猜测摘要；
- 根据当前问题或当前会议反推摘要；
- 使用相邻 Edge 的 Context Document 替坐标自身生成摘要；
- 在无权或无法读取完整内容时编造摘要；
- 依赖系统从正文自动生成并静默写入摘要。

摘要由实际写图 Agent 显式生成，这不等于系统自动摘要，也不改变现有“系统不自动推断项目语义”
的边界。

### 6.3 内容规范

摘要应：

1. 说明坐标内容的主题、覆盖范围、主要对象、模块、问题或关键约束；
2. 说明处理什么问题、决策、设计、故障或工作时可能需要加载；
3. 使用有区分度的项目术语、模块名、平台名和领域名称；
4. 对当前任务、当前 Meeting 和创建 Agent 的 Role 保持中立；
5. 对任意包含该 Coordinate 的 Edge 均能成立；
6. 在容易误解时简要说明不覆盖的边界；
7. 足够简短，使 Agent 可以在一个有界 Frontier 中同时比较多个候选。

摘要不得：

- 只是改写或重复标题；
- 写入当前任务、当前 Meeting 或当前 Edge 的临时关系；
- 针对创建 Agent 的 Role 定制；
- 使用“必须读取”“始终采用”等对未来 Agent 的指令；
- 包含工具命令、操作步骤或权限声明；
- 堆积搜索关键词；
- 承载详细事实、参数、证据、决定或最终结论；
- 声称 canonical 完整内容无法支持的信息；
- 重复对检索没有帮助的 ID、状态、优先级和 Revision。

### 6.4 推荐表达结构

推荐使用一段纯文本：

```text
第一句：这份坐标内容包含什么、覆盖什么。
第二句：处理什么类型的问题时可能需要加载。
第三句可选：说明容易误解的范围边界。
```

摘要通常为两句话，目标长度为 80～200 个汉字，硬上限为 1 KiB UTF-8。摘要不使用 Markdown
标题、列表、大段代码或工具命令。

“何时可能值得加载”应使用描述性表达，例如：

- “当问题涉及……时可能需要加载”；
- “适用于调查……”；
- “可用于理解……”；
- “涉及……的设计或验证时可能相关”。

这些表达是项目数据中的检索提示，不是平台 Prompt 指令。

### 6.5 不同坐标类型的关注点

| 坐标类型 | 摘要应重点说明 |
|---|---|
| Role | 责任范围、关键边界，以及什么类型的问题会涉及该 Role |
| Work | 处理的目标或问题、主要影响区域，以及相关实施、调查或风险场景 |
| Issue | 问题现象、影响范围、相关领域，以及可能参考它的相似故障或反馈 |
| Requirement | 期望行为、主要约束、适用范围，以及相关设计或验证场景 |
| Goal / Plan / Stage | 目标与规划范围、阶段意图，以及相关方向或优先级判断场景 |
| Resource | 资源提供的能力、使用边界，以及可能需要它的工作 |
| Document | 文档主题、覆盖范围和材料性质，例如规范、历史、操作说明或调查记录 |
| Meeting | 会议主题、讨论或决定范围，以及适合追溯的历史问题 |

### 6.6 示例

合适的 Work 摘要：

> 实现 Project Context 邻接查询的分页与 metadata 水合，涉及游标、Edge 去重以及 Project View、
> Document 和 Meeting 坐标预览。当排查高出度坐标、查询结果过大、分页一致性或 Agent 渐进检索
> 时，可能需要加载此 Work。

不合适的摘要：

> 这个 Work 与 Issue A 相关，后端 Agent 在本次会议中必须优先读取。

后者混入了当前 Edge、创建者 Role、当前任务和对未来 Agent 的指令，也没有稳定说明坐标完整内容
本身的范围。

## 7. CoordinateNode.summary 的乐观生命周期

### 7.1 首次生成

- 一个坐标首次进入 Project Context Graph 时必须同时拥有摘要；
- 写图 Agent 必须先读取其当前 canonical 完整内容；
- 已经存在 CoordinateNode 摘要时，后续加入其他 Edge 直接复用；
- 同一坐标不得因加入不同 Edge 而生成多个摘要；
- 首次摘要写入失败时，该坐标不能以缺少摘要的新节点形态进入图。

旧数据、迁移和历史上缺少摘要的节点如何补齐属于实现设计。检索遇到缺失摘要时不得因此断言其
完整内容不相关。

### 7.2 坐标内容更新

更新 Coordinate 指向的 canonical 内容时，由执行内容更新的 Agent 同时查看当前节点摘要，并判断
此次变化是否会改变未来 Agent 的加载决策。

只有发生实质影响时才更新摘要。

应更新的情形包括：

- 内容主题或作用范围改变；
- 重要模块、平台、约束或问题类型新增或移除；
- “何时可能值得加载”的条件发生变化；
- 关键边界或有区分度的项目术语发生变化；
- 原摘要已经不准确、遗漏核心范围或会误导检索；
- 摘要声称存在的内容已被删除。

通常不更新的情形包括：

- 排版或措辞调整；
- 局部实现细节变化；
- 普通进度更新；
- 单纯状态或优先级变化；
- 新增细节没有改变内容范围；
- canonical 内容发生变化，但未来 Agent 是否需要加载它的判断依据没有改变。

核心判断是：

> 不判断完整内容是否发生变化，而判断此次变化是否改变未来 Agent 的加载决策。

### 7.3 乐观机制的含义

乐观生命周期明确意味着：

- 系统不因原内容 Revision 变化自动重新生成摘要；
- 系统不自动把摘要标记为失效；
- 摘要不是原内容更新的强一致事务依赖；
- 摘要未更新不阻止原内容更新，也不阻止图查询；
- 当前摘要在被 Agent 显式修正前继续作为检索提示；
- Agent 后续发现摘要不准确时，可以显式修正；
- 系统不运行全局摘要维护、强制同步或自动重写任务；
- 如果实现记录生成依据或 Revision，它只能作为 provenance，不能自动使摘要失效或阻断查询。

这一机制接受摘要可能暂时陈旧。渐进检索提供低成本发现能力，不承诺完备召回，也不能用“摘要
未命中”证明某份完整内容与问题无关。

### 7.4 摘要更新的无级联原则

更新摘要：

- 不改变 Coordinate identity；
- 不改变 EdgeKey；
- 不创建、删除或重写 Edge；
- 不修改 Edge 坐标集合；
- 不自动修改 Context Document；
- 不修改 Coordinate 指向的 canonical 内容。

### 7.5 Tombstone

沿用现有 Project Context 历史保留原则：

- Coordinate 进入 tombstone 不自动删除包含它的 Edge；
- 查询必须同时呈现 tombstone 状态；
- 已有最后摘要继续保留，帮助 Agent 判断是否需要读取历史内容；
- tombstone 本身不触发自动摘要重写。

## 8. 图与 Agent Prompt 的职责

### 8.1 图提供的能力

在不规定实现方式的前提下，Project Context Graph 必须能够：

- 统一返回 CoordinatePreview；
- 通过 title 和 summary 发现少量候选 CoordinateNode；
- 从一个或多个已知 Coordinate 发现真实的 Context Edge；
- 有界、可继续地返回候选，而不是一次性注入全部节点或邻接结果；
- 返回 Hyperedge 的完整 Coordinate identity 集合；
- 有界地为相邻坐标返回 title 和 summary，并明确当前已水合范围；
- 为 Edge Context Document 返回 title 和 summary；
- 按需读取任一 Coordinate 指向的 canonical 完整内容；
- 按需读取任一 Context Document 的完整内容；
- 保留 Project 边界、权限、生命周期状态和真实来源；
- 使 Agent 能区分文本候选发现与真实图关系遍历。

图不负责：

- 判断某个 Coordinate 或 Context Document 对当前问题必然相关；
- 根据 Role 自动划分专属子图；
- 自动把候选完整内容加入 Agent Context；
- 自动创造方向、权重、关系类型或语义边；
- 从文本相似性推断真实 Edge；
- 强制不同 Agent 得到不同结果；
- 从多跳可达性推断依赖、影响或其他可传递语义。

### 8.2 稳定 Agent 写入与维护契约

具体 Prompt 文案由实现设计决定，但写入和维护行为契约必须指导 Agent：

- 首次通过合法 Edge 关联把 Coordinate 引入图前，读取其当前 canonical 完整内容；
- 为新 CoordinateNode 生成符合第 6 节规范的摘要；
- 已有 CoordinateNode 加入其他 Edge 时复用同一摘要，不创建 Edge-specific 副本；
- 更新图中 Coordinate 指向的原内容时，同时检查现有摘要；
- 只有变化影响未来 Agent 的加载判断时才显式更新摘要；
- 发现摘要已经不准确时，通过显式写操作修正；
- 摘要维护失败或当前无维护权限时，不把原内容更新伪装成失败，也不声称摘要已经更新。

该契约不改变既有 Coordinate 和 Context 写入权限。Agent 能否写入或修正摘要继续由实际授权决定；
乐观生命周期不因为 Agent 负有判断责任就额外授予写权限。

### 8.3 稳定 Agent 检索契约

具体 Prompt 文案由实现设计决定，但稳定行为契约必须指导 Agent：

- 始终根据 `problem + role + works / other coordinates + current Runtime Context` 判断相关性；
- 默认先查看 title 和 summary；
- 分别决定读取 Coordinate 完整内容、读取 Edge Context Document、继续展开或暂缓；
- 摘要只能用于路由，不能作为项目事实证据；
- 准备用某份内容形成判断或输出时，读取足够的 canonical 完整内容；
- Role 是必要视角而不是硬过滤器；
- 只沿图返回的真实 Coordinate 和 Edge 继续，不得臆造图关系；
- Hyperedge 必须作为完整坐标集合理解，不能隐式拆成二元边；
- 检索保持有界，不一次加载全部候选；
- 在上下文足够、候选耗尽、来源不可用或预算受限时停止；
- 停止时如实说明覆盖边界，不把“没有继续探索”写成“项目中不存在”。

Prompt 负责语义选择，不负责替代系统的权限、分页、边界检查、真实来源验证或其他确定性保证。

## 9. 渐进检索过程

本文只规定逻辑过程，不规定 BFS、DFS、最短路径、向量搜索或其他算法。

### 9.1 建立检索视角

Agent 首先理解：

- 当前问题描述；
- 期望获得的结果；
- 自己的 Role purpose、responsibilities 和 boundaries；
- 当前 Work 与其他已知坐标；
- 当前 Runtime 已经拥有的相关上下文；
- 当前仍需补充的未知。

### 9.2 发现初始候选

如果问题提供了明确坐标，Agent 可以从这些坐标以及适用的 Role、Work、其他环境坐标开始。

如果只有自然语言问题，图必须允许 Agent 先通过 title 和 summary 有界地发现少量候选 CoordinateNode，
再选择一个或多个候选作为 seed。具体采用关键词、全文、向量或其他索引不由本文规定。

必须区分：

```text
problem text → candidate CoordinateNode
```

这是轻量候选发现，不是图关系；而：

```text
Coordinate → real Hyperedge → Coordinate
```

才是真实图遍历。文本命中不能被宣称为新 Edge。

Role 是必须的判断视角，但如果 Role 没有进入 Project Context Graph，它不构成结构 seed。Work 和
其他显式坐标既可以影响判断，也可以直接作为 seed。

### 9.3 读取轻量 Frontier

Agent 首先查看一个有界候选集合：

```text
CoordinatePreview
  title + summary + lifecycle / status

EdgePreview
  exact Coordinate identity set
  + hydrated Coordinate title / summary and coverage
  + Context Document title / summary
```

Frontier 必须允许继续读取下一批，而不是把整个图或一个高出度坐标的全部邻接结果一次放入 Agent
窗口。具体 cursor、limit 和预算结构由实现设计决定。

### 9.4 选择下一动作

Agent 可以独立选择下列逻辑动作：

```text
discover
  根据问题文本继续发现少量 CoordinateNode 候选

read-coordinate
  读取 Coordinate 指向的 canonical 完整内容

expand-coordinate
  从 Coordinate 查询真实 incident Hyperedge

read-edge-context
  读取某条 Edge 的 Context Document 完整内容

defer
  当前不加载；随着新上下文出现可以重新考虑

stop
  结束本次检索
```

这些动作没有固定全局顺序。Agent 可以因某个节点摘要而直接读取 Work、Issue 或 Requirement，无需
先读取关系正文；也可以先读取关系正文判断整条 Hyperedge 是否值得继续。

### 9.5 根据新内容继续

- 每次读取完整内容后，Agent 重新判断当前未知和候选优先级；
- 新内容可以使此前暂缓的候选重新相关；
- 同一完整内容无需重复加载，但其不同真实到达路径可以保留；
- 图上的结构可达只提供发现能力，不自动成为事实推理；
- `A → E1 → B → E2 → C` 不证明 A 与 C 存在可传递语义；
- 相同问题和不同 Role 得到高度重合路径是允许的，系统不得为了制造差异而强制分流。

## 10. 停止与输出

### 10.1 停止原因

合理停止原因包括：

- 已取得足以处理当前问题的上下文；
- 当前可发现候选中没有进一步值得加载的内容；
- 自然语言问题无法解析出可靠 seed；
- 图中缺少可继续发现的连接；
- 检索预算耗尽；
- 目标内容不存在、已不可用或当前无权读取。

Agent 只能说明在实际探索范围内的结果，不得把没有继续探索或摘要未命中表述为“项目中不存在”。

### 10.2 Retrieved Context 输出

逻辑输出应能够表达：

```text
RetrievedContext
├── selected coordinate contents
├── selected Edge Context Documents
├── stable source identities / available source versions
├── actual discovery paths / seeds
└── termination reason
```

未解决问题、暂缓候选和不可用范围可以作为可选诊断信息。Retrieved Context 不要求保存 Agent 的
隐藏推理或 chain-of-thought。可追溯的是来源、实际读取动作、真实图路径和覆盖边界。

摘要只用于检索路由。当 Agent 需要依赖某个候选中的具体事实形成判断时，必须读取相应 canonical
内容，或确认具有 canonical provenance 的内容已经存在于当前 Runtime Context；summary 本身不能
作为事实证据。来源领域的可信结构化 metadata 仍可以按其原有语义作为事实使用。

## 11. 权限、来源与安全边界

- Project Context 渐进检索复用现有 Project / Community、Project View、Document 和 Meeting 权限；
- Coordinate title 和 summary 本身也属于项目数据，只能在调用者有权发现该 Coordinate 时返回；
- 查看 Edge 不授予其中 Coordinate 或 Context Document 的新权限；
- Preview 和完整内容读取均继续经过对应来源领域的权限检查；
- 如果完整 Coordinate identity 集合不能安全披露，该 Hyperedge 不得以被静默缩小的集合参与遍历；
  具体拒绝或不可用表达由实现设计决定；
- 已读取内容应保留 Coordinate、Document、Meeting 或其他 canonical 来源；来源领域支持版本时还应
  保留实际读取版本；
- summary 与其他项目文本一样是项目数据，不是平台级 Prompt 或权限授予；
- 检索不提升 Runtime、Sandbox、代码仓库或外部系统权限；
- 发现摘要错误、内容缺失或 Edge 不准确时，本次检索不自动修改图；Agent 可以通过既有显式维护
  能力另行写回修正。

新 CoordinateNode 仍必须满足现有 Project Context 的全部坐标准入条件，包括同 Project、受支持的
Coordinate family、来源可验证性和 Meeting 可关联状态等；提供 summary 不会放宽任何既有条件。

## 12. 与 Meeting 的关系

本文件放在 `meeting/context` 下，是因为需求从 Meeting 如何分担上下文的讨论中产生，并不表示
它是 Meeting 专用设计。

Meeting 可以按如下方式适配通用输入：

```text
Meeting title / description / goal
+ involved Work / Issue / Requirement / Document / Meeting
  → RetrievalRequest.problem

Participant Role
+ participant current Work / other coordinates
  → RetrievalRequest.agent_environment
```

Meeting Coordinate 参与检索时，与 Work、Issue、Requirement、Document 等一样只是普通 Coordinate
类型。

本设计不定义：

- participant 何时运行检索；
- Retrieved Context 如何进入 Meeting Session；
- participant 如何在 Meeting 中共享上下文；
- Meeting 是否以及何时召开；
- 检索结果是否进入 Board；
- 为不同 participant 预分配不同上下文；
- 强制不同 participant 获得不同检索结果。

本设计只解决 Agent 如何从统一 Project Context Graph 中渐进取得当前问题需要的上下文，不解决
多个 Agent 如何在 Meeting 中使用和共享这些上下文。

## 13. 设计不变量

后续实现设计必须保持：

1. Project Context 仍是唯一的持久项目上下文图。
2. 所有 Agent 使用同一张图，不按 Role、Agent、Meeting 或 Session 复制上下文。
3. Coordinate identity 仍由稳定类型、ID 和 Project scope 定义。
4. CoordinateNode.summary 不参与 Coordinate identity、规范化、EdgeKey 或 Edge 唯一性。
5. 同一 Project 中同一个 Coordinate 只有一份节点摘要，多条 Edge 复用。
6. Coordinate 指向的 canonical 内容仍由原领域实体拥有，CoordinateNode 不复制完整内容。
7. Coordinate 完整内容与 Edge Context Document 都是一等检索目标。
8. Coordinate summary 解释“这个点有什么、何时可能值得加载”。
9. Context Document 解释“这组精确坐标为什么共同相关”。
10. 摘要只是导航信息，不是事实证据或完整内容替代品。
11. 新 CoordinateNode 摘要由写图 Agent 基于当前完整内容显式生成，不由系统静默推断。
12. 摘要采用乐观生命周期，不因原内容 Revision 改变自动失效或重写。
13. 摘要更新不改变 Coordinate、Edge、EdgeKey、Context Document 或原对象内容。
14. Role 是每次检索的必要语义视角，不是 ACL 或硬过滤条件。
15. 图提供真实候选、拓扑和读取能力，Agent Prompt 负责语义选择。
16. 检索先取得 title / summary，再按需加载完整内容，不一次注入全部图内容。
17. Hyperedge 保持完整坐标集合，不隐式拆成二元关系。
18. 文本候选发现不是图关系，多跳结构可达也不代表语义传递。
19. 相同环境产生高度重合结果是允许的，不强制制造路径差异。
20. Retrieved Context 是当前 Runtime 的临时结果，不形成 Role Context、Meeting Context 或新的事实层。
21. 逻辑 CoordinateNode 必须属于至少一条当前 Edge；离开图后重新进入时必须重新确认摘要准确。
22. Edge Context Document 的摘要与同一 Document 作为 Coordinate 时的节点摘要是不同语义。
23. 提供摘要不放宽现有 Coordinate、Edge、Project、权限或生命周期准入规则。

## 14. 非目标

本设计不尝试：

- 新建持久 Role Context、Meeting Context 或 Agent 私有知识仓库；
- 按 Role 拆分、复制或隔离 Project Context；
- 为 Meeting 预先分配上下文分片或共同基线；
- 让图替 Agent 判断最终语义相关性；
- 把文本候选命中解释为真实 Edge，或用文本搜索替代 Project Context Graph 的关系事实；
- 为 Edge 引入方向、关系类型、ontology、权重或预计算路径；
- 将 summary 建成 Coordinate canonical 内容的替代副本；
- 每次原对象变化都强制重新生成摘要；
- 通过 summary 命中或未命中证明内容相关或无关；
- 自动把候选或 Retrieved Context 写回 Project Context；
- 定义图引擎、索引、Prompt、Runtime 或 Meeting 的具体实现。

## 15. 结论

本设计可以概括为：

> 每个已进入 Project Context Graph 的坐标统一提供一份由 Agent 显式生成、乐观维护的轻量检索摘要。
> 面对一个问题时，Agent 以自己的 Role 为必要视角，并结合 Work、其他项目坐标和当前 Runtime
> Context，先查看坐标与关系内容的 title / summary，再选择性加载 Coordinate 完整内容、Edge
> Context Document，或沿真实图关系继续探索。

图负责提供可信、紧凑、有界、可追溯的候选和内容读取能力；Prompt 负责指导 Agent 判断相关性和
检索路径。检索结果只是在当前问题下动态激活的上下文，不构成新的 Meeting Context、Role Context
或 Project 事实层。
