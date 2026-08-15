# Project Context：先有坐标，后有上下文

> 本文解释 Carryforth 的一项核心设计：为什么项目上下文不是先保存一段“相关文本”，
> 而是先建立稳定坐标，再保存坐标之间精确、可修订的关联上下文。
>
> “坐标上下文”和“关联上下文”是本文为了说明设计使用的认知分层，不是两个新增的
> wire schema、数据库表或权限域。精确领域合同见
> [Project Context 领域规范](../../stage/project-context/project-context.md)。

## 1. 核心判断

> 上下文不是一段脱离对象而独立存在的知识。上下文总是“关于什么”，并且只在一定范围内成立。
> 先要有可稳定识别、可验证、可回读的“什么”，才能准确保存“它们为什么相关”。

在 Carryforth 中，这个“什么”就是**坐标**。

Requirement、Work、Resource、Document、Meeting 等对象先在 Project 内取得稳定身份；
坐标及其在来源领域中指向的规范内容，构成 Agent 理解单个对象的基础上下文。

当一段信息只在多个对象共同出现时才成立，Project Context 再用无向 Edge / Hyperedge
锚定准确的坐标集合，并由版本化 Project Document 承载这组坐标之间的原因、依赖、影响、
例外和边界。

```text
稳定坐标
  │
  ├── 回到来源领域，读取对象当前的规范内容 ─────── 坐标上下文
  │
  └── 与其他坐标组成精确集合
          │
          ├── Edge / Hyperedge：关系准确适用于谁
          └── Context Documents：为什么相关 ────── 关联上下文
```

这套设计的目标不是建立一个能够自动理解全部项目的知识图谱，而是让 Human 与 Agent
可以围绕稳定项目对象，持续发现、读取、验证和维护开放语义。

## 2. 上下文首先缺少的不是更多文本，而是明确对象

项目里很容易出现这样的记录：

> 前端适配依赖新的授权接口，切换时要保留旧状态。

这句话看起来有用，但后来的成员无法确定：

- “前端适配”是哪一项 Work；
- “授权接口”是哪一个 Requirement 或 Resource；
- 它针对当前设计还是已经废止的方案；
- 这段说明应在什么对象变化时重新检查；
- 读取者是否有权访问它所暗示的其他内容。

继续增加摘要、标签或向量，并不能自动解决这些问题。没有稳定对象，检索只能找到“相似文本”，
却无法可靠回答“它说的是谁”“现在是否仍成立”“应该回写到哪里”。

因此，Carryforth 先把 Project 中需要持续存在的事物建模成可引用对象，再允许围绕这些对象
建立关联上下文。

## 3. 坐标是什么

坐标是一个 Project 内可以长期、无歧义地指向某个对象的稳定引用。

当前 Project Context 使用三类坐标：

1. **Project View 对象坐标**：Project Profile、Goal、Role、Plan、Stage、Requirement、Issue、
   Work、Resource 的对象类型与稳定 `object_id`；
2. **Project Document 坐标**：稳定 `document_id`；
3. **Meeting 坐标**：稳定 `meeting_id`。

坐标不是：

- 标题或名称；
- 当前正文或摘要；
- 某个 Revision；
- 某一次 Nostr event；
- 某条 Meeting Speech；
- 创建或处理该对象的 Agent Runtime；
- embedding 或向量索引中的一行。

这些内容都可以变化，坐标身份不随之改变。

```text
Work coordinate: { type: Work, id: W-17 }

revision 1  设计 Desktop 入口
revision 2  补充错误恢复状态
revision 3  完成交互验收

三次状态变化仍然指向同一个 W-17。
```

坐标还必须属于当前 Project / Community。裸 URL、文件路径、聊天片段或模型输出不会因为
“看起来相关”就自动成为 Project Context 坐标。外部资产需要先通过 Resource 或 Document
取得项目内身份；新增坐标类型也必须先定义稳定身份、生命周期、权限和规范化规则。

## 4. 坐标上下文：理解对象本身

“坐标上下文”是本文对一种读取过程的称呼：

> 从一个稳定坐标出发，在当前权限范围内解析该对象的规范内容、当前 Revision、生命周期、
> 来源证据和必要的直接引用。

内容仍然属于对象原来的来源领域，而不是被复制进 Project Context：

- Requirement 提供希望实现或满足的内容、状态和规划位置；
- Work 提供要做的工作、处理目标、责任与当前状态；
- Resource 提供项目内的资源身份，并由 Guide Document 说明如何找到和使用它；
- Document 提供稳定身份、当前不可变 Revision 和历史 Revision；
- Meeting 提供稳定会议身份和经权限验证的 Board、Speech 与结果入口。

坐标上下文先回答：

1. 我正在处理的究竟是哪一个对象？
2. 当前权威内容是什么，来源和 Revision 是什么？
3. 对象变化后，我能否继续回到同一个对象？

一个 Agent 接到 Work 坐标后，可以先回读该 Work、它处理的 Requirement 或 Issue、
responsible Role 和直接引用，而不必把整个 Project 历史放进每一轮提示词。

### 4.1 直接事实仍属于原领域

如果一项信息会驱动权限、生命周期或自动行为，它应写回明确的领域状态。例如：

- Stage 属于哪个 Plan；
- Work 处理哪个 Requirement 或 Issue；
- Work 是否完成；
- 哪个 Role 对 Work 负责；
- 谁正在承担 Role；
- Resource 使用哪份 Guide Document。

这些事实不能只写在 Context Document 中。否则项目会出现两份真相：机器读取一份状态，
Human 和 Agent 又从说明文档推断另一份状态。

### 4.2 Context Reference 不是 Context Edge

除 Resource 外的 Project View 对象可以直接持有指向 Resource 或 Document 的 Context Reference；
Resource 自身只能引用 Document，并通过主要的 Guide Document 提供使用说明。这些是单个对象
自己的直接阅读入口。

Project Context Edge 不同：它连接两个或更多坐标，解释这一整组对象为什么需要共同理解。
两种结构可以同时存在，但不会互相自动生成或同步。

## 5. 关联上下文：解释对象之间

有些信息不属于任何一个对象自身，而只在多个对象共同出现时成立。例如：

- 两项 Requirement 为什么必须同时交付；
- 一个前端 Work 为什么受某个 Relay Resource 的协议边界约束；
- 一场 Meeting 为什么改变了后续 Work 的实现顺序；
- 一份 Document 为什么只适用于某个 Stage、Role 与 Resource 的组合。

关联上下文由两个部分组成：

```text
ProjectContextEdge
├── coordinates            精确、无序的坐标集合，回答“适用于谁”
└── context_documents      一份或多份版本化文档，回答“为什么”
```

### 5.1 Edge 只锚定精确范围

Edge 本身只保存一个结构事实：**这组坐标共享一段需要被共同理解的上下文。**

```text
{Requirement A, Work B} == {Work B, Requirement A}
{Requirement A, Work B} != {Requirement A, Work B, Resource C}
```

Edge 是无向的，不预设 `depends_on`、`causes`、`blocks`、`influences` 等关系类型。

三个或更多坐标构成 Hyperedge。Hyperedge 表达整体条件，不会被系统自动拆成两两关系：

```text
{A, B, C}

不自动产生 {A, B}、{A, C} 或 {B, C}。
```

这可以防止 Agent 把只在 `{A,B,C}` 整体条件下成立的解释，错误应用到 `{A,B}`。

### 5.2 Document 承载开放语义

Edge 不保存正文。普通 Project Document 被关联到 Edge 后，承担 Context Document 的结构角色，
并继续复用 Document 已有的稳定身份、不可变 Revision、当前版本、作者信息和 tombstone 规则。

Context Document 可以说明：

- 关系形成的原因与证据；
- 依赖的方向、条件和强弱；
- 一个对象变化可能造成的影响；
- 适用范围、例外、兼容条件和风险；
- 实际工作中验证或推翻该解释的方法。

同一组精确坐标只有一条 Edge，但可以关联多份 Context Document。一份记录历史原因，
另一份记录兼容限制，第三份记录回滚边界；不需要复制三条相同 Edge。

一份 Document 同时最多作为 Context Document 属于一条 active Edge，避免同一段解释拥有两个
互相竞争的适用范围。它仍然可以在其他 Edge 中作为被解释的 Document 坐标出现。

### 5.3 Document 的两种结构角色

同一份 Document 在模型中可能承担两种不同角色：

- 出现在 `coordinates` 中：它是被解释的对象之一；
- 出现在 `context_documents` 中：它承载这条 Edge 的解释。

把 Document 绑定为 Context Document，不会自动把它加入坐标集合；把 Document 作为坐标，
也不会让它自动成为解释载体。

## 6. 为什么必须先有坐标

这里的“先”首先是**权威建模和产品设计上的先于**。

### 6.1 身份先于引用

标题、正文和关键词都会变化，也可能重名。稳定坐标把“对象是谁”与“对象现在写了什么”分开，
让后来的成员能够持续引用同一个对象，而不是重新猜测文本指向。

### 6.2 事实先于解释

Project View 先回答“项目是什么、有哪些、当前在哪里”；Project Context 再回答“为什么相关”。
如果当前状态本身都不能直接读取，关系解释就会承担不属于它的职责，并形成影子状态。

### 6.3 作用域先于语义

一段关系说明必须先回答它精确适用于哪些对象，才能讨论原因和影响。`{A,B}` 与 `{A,B,C}`
是不同语义范围，不能靠自然语言或相似度自动互换。

### 6.4 Project 与权限边界先于相关性

系统必须先验证：

- 对象是否属于同一个 Project；
- 调用者是否有权发现和读取它；
- 引用是否指向真实的规范对象；
- 对象当前是 active、terminal 还是 tombstoned；
- 读取和修改基于哪个 Revision。

“语义上相关”不会扩大任何读取、写入、Runtime、Sandbox 或外部系统权限。

### 6.5 修订边界先于自动传播

坐标、Edge 和 Document 分开后，三类变化可以分别处理：

- 对象内容变化：修改来源对象；
- 关系解释变化：产生新的 Document Revision；
- 关联范围变化：显式 detach / attach。

系统不需要猜测一次文本修改是否意味着重写对象状态、改变图拓扑或删除历史关系。

### 6.6 可回读来源先于派生结果

来源对象自己拥有的标题、summary、正文和状态仍是 canonical content；其中 summary 可以作为检索提示，
但不会覆盖对象的完整内容。embedding、语义路径、UI cache，以及客户端或模型另行生成的摘要才是
派生读取。它们可以帮助发现内容，但最终结果必须能够回到坐标指向的 canonical object、Revision
或 Document，不能把 embedding、缓存或模型输出变成事实源。

### 6.7 可发现入口先于全量注入

Agent 的工作通常已有一个起点：当前 Role、Work、Requirement、Document 或 Meeting。
稳定坐标让 Agent 可以按需扩展上下文，而不是全量注入项目历史：

```text
当前 Work 坐标
  → 回读 Work 的规范内容
  → incident(Work) 发现直接相关 Edge
  → 检查 Edge 的精确坐标集合与 Document 元数据
  → 只读取当前问题需要的 Context Document
  → 必要时沿相关坐标继续发现
```

## 7. “先有坐标”不是什么

### 7.1 不是现实发现的时间顺序

Human 或 Agent 可能先在实践中发现一段上下文，再意识到项目缺少 Requirement、Issue 或 Work。
这段洞察可以反过来促成新对象。

“先有坐标”只表示：当关系要进入项目规范状态时，真正的参与对象应先取得稳定身份，
再建立 Edge。不能为了满足 Edge 形式而强造虚假对象；只关于单个对象的说明，也不应制造 self-edge。

### 7.2 不是检索前必须知道坐标

Agent 通常从当前 Work、Requirement、Issue、Stage 或 Meeting 已经给出的可靠 Coordinate 起步。
如果没有，`coordinate-search` 可以把自然语言需要映射为 ranked Coordinate 候选。候选不是已经选定的
起点；Agent 要先观察其 canonical 轻量状态，再结合当前 Role 和其他相关上下文环境事实筛选。

因此，“先有坐标”是持久关联成立的条件，不是 Agent 开始发现前必须已经知道坐标。

### 7.3 不是坐标永远有效

稳定坐标只保证身份连续，不保证对象含义永远不变，也不保证对象仍处于 active 状态。
读取者必须同时检查当前 Revision、生命周期和来源证据。

## 8. 为什么不为每种关系建立类型和状态机

结构化关系并非越多越好。

需要机器执行的关系应当成为明确领域模型。例如 Stage 必须属于 Plan、Work 必须处理一个
Requirement 或 Issue、Assignment 与 Commitment 有明确授权和生命周期。这些关系会驱动状态和行为，
值得拥有类型与状态机。

跨对象的解释性语义则是开放的。同一组对象之间可能同时存在历史原因、实现依赖、组织约束、
兼容风险、阶段性例外和经验判断。如果不断增加 `depends_on`、`derived_from`、
`compatible_with`、`supersedes_when` 等关系类型，系统将面对：

- 持续膨胀且难以稳定的关系词表；
- 每种关系的方向、基数、生命周期和迁移；
- 复杂含义被压缩成一个貌似确定、实际含糊的标签；
- Agent 为满足 schema 而伪造确定性；
- 新语义必须先修改代码和数据库才能写回。

Carryforth 因此把两类信息分开：

- **机器必须严格判断的结构事实**进入明确领域模型；
- **开放、解释性、需要人类语言表达的二阶语义**由精确 Edge 范围和版本化 Document 承载。

这不是拒绝结构化，而是把结构化用在需要稳定执行的地方。未来某种关系如果确实需要驱动权限
或自动状态变化，可以基于真实需求升级为独立领域合同；在此之前，不预先穷举开放语义。

代价也必须明确：系统不会自动验证 Context Document 中声称的因果、方向或真实性。
这些解释仍需要实际参与项目的 Human 和 Agent 持续验证和修订。

## 9. 前端与后端上下文示例

假设同一个 Requirement 同时产生前端和后端 Work：

```text
Requirement R：用户可控的 Agent 召回

Work F：Desktop 召回配置与解释界面
Work B：Relay 召回、授权与结果签名
Role F：Frontend
Role B：Backend
Resource UI：Desktop 交互规范
Resource API：Relay 查询合同
```

Project 可以保存两组不同的关联上下文：

```text
Edge {R, Work F, Role F, Resource UI}
└── Document：前端展示、输入状态、错误恢复与可解释性边界

Edge {R, Work B, Role B, Resource API}
└── Document：Provider 出境、NIP-98 授权、遍历预算与签名边界
```

两个 Edge 都包含 Requirement R，但作用域不同。

从 Work F 执行精确 `incident` 查询，Agent 会确定性发现前端局部关系；从 Work B 出发，
会发现后端局部关系。这种差异来自显式坐标与 Edge，不依赖模型猜测。

上下文环境感知的 Agent 检索可以直接从 Work F 或 Work B 起步。在每个 Coordinate 上，语义选边只按
relation Documents 排列其 incident 关系；选定 Edge 后，语义排 Coordinate 只在完整成员集合内工作。
Agent 结合当前 Role 与相关工作环境观察和选择，而不是自动采用最高分。

必须区分：

- 精确和结构读取回答其声明范围内的完整集合问题；
- 语义操作排列候选，但不替 Agent 选择对象或路径；
- Role 和工作环境不是 ACL、硬过滤器或新的持久 Edge；
- 相似度分数不是事实置信度、因果证明或项目优先级；
- Relay 签名证明结果来源和请求绑定，不证明语义候选天然正确或完整。

## 10. Agent 如何读取和维护上下文

### 10.1 读取

1. 确认当前 Project 和调用者身份；
2. 从当前 Role、Work、Requirement、Document 或 Meeting 取得稳定坐标；
3. 回读坐标当前的规范内容、Revision 与生命周期；
4. 没有相关起点时只执行一次 `coordinate-search`，观察候选后再选择；
5. 使用 `coordinate edge-search` 或 `coordinate edges` 观察 incident 关系；
6. 把 relation Documents作为轻量依据，只读取任务真正需要的正文；
7. 使用 `edge coordinate-search` 或 `edge coordinates` 选择或枚举下一成员；
8. 在循环、快照和预算边界内渐进继续，把每个语义结果视为候选而不是规范事实。

### 10.2 写入

1. 先判断新信息是否属于某个明确领域字段或状态；
2. 如果是，更新对应 Requirement、Issue、Work、Role、Document 等规范对象；
3. 如果只在多个对象共同出现时成立，选择真实、准确的坐标集合；
4. 查询该集合是否已经存在 Edge；
5. 新建或修订普通 Project Document，写清原因、影响、证据和边界；
6. 显式 attach 到准确坐标集合，并回读 canonical Edge；
7. 不从聊天、Meeting Board、工具成功或模型推断自动创建关系。

这是“先有坐标”作为写入纪律的含义：先把事实写回它真正所属的对象，再为跨对象解释建立关联；
不能用一篇 Context Document 代替本应存在的 Requirement、Issue、Work 或其他领域状态。

## 11. 生命周期与历史

| 变化 | 结果 |
|---|---|
| 对象内容变化 | 坐标身份不变；读取新的规范状态或 Revision |
| Context Document 内容变化 | 产生新的 Document Revision；Edge 坐标集合不变 |
| 关联范围变化 | 显式 detach / attach；旧范围不会被静默解释成新范围 |
| 坐标 tombstone | 已有 Edge 保留该稳定坐标，并呈现生命周期状态 |
| Context Document 请求 tombstone | active binding 会阻止操作；必须先从 Edge detach |
| 最后一份 Context Document detach | 空 Edge 消失，Document 本身不被删除 |

新建关联时，坐标和 Context Document 必须满足当前 Project 与生命周期校验；Meeting 还要满足
当前可 attach 阶段。关系成立后，坐标 tombstone 不会静默缩小 Edge，也不会级联删除解释文档。

这让后来者不仅能读到“现在认为怎样”，还能知道解释曾对应哪些对象，以及历史关系为什么
没有随着对象删除而被无痕改写。

## 12. 容易混淆的几种 Context

| 概念 | 是否持久 | 作用 |
|---|---:|---|
| Project View Context Reference | 是 | 单个 Project View 对象直接引用 Resource 或 Document |
| Project Context Edge | 是 | 保存两个或更多坐标之间的精确、显式关联范围 |
| Agent 上下文环境 | 否 | 当前 Role 与相关任务事实，用来让 Agent 选择候选和路径 |
| Agent 检索路径 | 否 | 带 relation 依据的临时 `Coordinate → Edge → Coordinate` 阅读轨迹 |
| 完整路径型 semantic-query context | 否 | 保留的有界查询产品中的可选软召回与排序输入 |

Agent 检索路径和完整路径型语义结果都是派生读取，不是新的持久关系。只有显式写入并通过 Relay
验证的 Project View、Document、Context Edge 或其他领域状态，才成为 Project 的规范事实。

## 13. 设计边界

这套模型有意不承诺：

- 从消息、文档或对象状态自动推断 Edge；
- 用 Edge 取代 Project View 已有的强类型关系；
- 为开放语义预先穷举关系类型；
- 把 Context Document 自动注入每一轮 Agent 对话；
- 让语义相似度决定事实、权限、责任或行动；
- 让一个 Project 的坐标跨越到另一个 Project；
- 因为对象 tombstone 而自动改写历史关联；
- 保证保存的解释天然完整、最新、无冲突或正确。

系统保证的是稳定身份、精确范围、规范写入、Revision、生命周期和权限边界；上下文是否准确、
有用，仍由实际参与项目的 Human 和 Agent 在工作中验证、修订和维护。

## 14. 由此得到的设计原则

1. **先识别对象，再解释关系。** 没有稳定参与者，就没有可维护的项目上下文。
2. **身份与内容分离。** 内容可以演进，坐标必须保持可引用。
3. **事实与解释分离。** 直接状态回到来源领域，跨对象含义进入 Context。
4. **范围与语义分离。** Edge 定义准确适用范围，Document 承载开放解释。
5. **强类型状态与开放语义分离。** 需要机器执行的关系进入领域模型，其余不强行编码。
6. **显式关系优先于隐式推断。** 语义检索帮助发现，不替代规范事实。
7. **按需读取优先于全量注入。** Agent 从当前工作坐标渐进发现上下文。
8. **修订优先于无痕覆盖。** 对象、解释和关系范围的变化分别留下可验证记录。
9. **相关性不产生权限。** 所有读取、写入和查询继续服从 Project / Community 边界。

“先有坐标，后有上下文”的含义是：先让项目中的事物拥有稳定、可验证、可回读的存在，
再让 Human 与 Agent 围绕这些存在持续积累关系和解释。这样保存下来的不是一堆等待重新猜测的文本，
而是一张能够随 Project 演进、又不丢失边界、来源和历史的项目认知网络。

## 继续阅读

- [Carryforth 核心模型](../core-model.md)
- [核心设计：Role Continuity](role-continuity.md)
- [核心设计：Agent 自主的上下文环境感知 Project Context 图检索](context-aware-semantic-graph-retrieval.md)
- [核心设计：Meeting](meeting.md)
- [Project View 定义](../../stage/project-view/project-view.md)
- [Project Document](../../stage/document/document.md)
- [Project Context 领域规范](../../stage/project-context/project-context.md)
- [Project Context Desktop 设计](../../stage/project-context/desktop-spec.md)
- [项目空间宪章](../../project-space-constitution.md)
- [当前状态与能力边界](../current-status.md)
