# Meeting 上下文讨论历程

> 状态：讨论过程与当前结论记录，不是实现方案
>
> 代码基线：`version/v1.0.0` @ `f5cec1716`
>
> 初始日期：2026-08-08
>
> 最近更新：2026-08-09

后续形成的通用设计见
[Project Context Agent 渐进检索设计规范](project-context-progressive-retrieval-spec.md)。该设计独立于
Meeting；Meeting 只是它的潜在调用方之一。

## 1. 文档目的

本文记录围绕 Meeting、Agent 上下文和 Project Context 的讨论是如何逐步修正并收敛的。
重点不是只保留最后结论，而是保留：

- 最初怎样理解问题；
- 哪些判断被证明只触及表面；
- 用户如何指出更深一层的问题；
- 最新代码与文档如何改变判断；
- 哪些方向已经被明确排除；
- 当前为何把问题收敛到统一上下文图的语义检索路径。

本文不定义图语义、检索算法、索引、Prompt、Meeting 阶段、协议或实现计划。

此前的 [Meeting 上下文问题理解](meeting-context-understanding.md) 是讨论中途形成的阶段性记录。
其中关于 Meeting 分担上下文、Project Context 已有能力和跨 Session 断点的观察仍然有效，但其
最终问题定义已被本文后续阶段继续修正。

## 2. 讨论起点：Meeting 在自主 Agent 空间中是否有实际作用

讨论最初从一个产品问题开始：

> 在无需 Human 深度参与、依靠 Agent 自发持续运行的协作空间中，Meeting 是否真的能产生价值？

最初判断把注意力放在：

- Agent 是否会自主召集；
- 会议决定如何收敛；
- 会议结果是否物化；
- 物化后如何验证；
- Agent 是否会执行分配给自己的工作。

用户指出，这些不是核心问题：

- 自主召集可以由 Agent 行为契约约束；
- 需要长期承载的会议结果可以进入 Project View；
- 相关 Agent 能从项目状态看到后续工作；
- 决定落地不要求所有参会者继续深度参与。

由此，讨论第一次转向真正影响 Meeting 价值的问题：Agent 当前交给 LLM 的上下文是有限且会
压缩、重建和丢失的。

## 3. 第一轮收敛：识别跨 Session 上下文断裂

用户给出的关键场景是：

1. Agent A 曾参与或形成某个方案；
2. 经过许多轮工作和上下文压缩后，A 当前窗口已经没有该方案；
3. Agent B 对方案产生疑问并邀请 A 参会；
4. A 的 Meeting Session 只能重新读取方案、Project View 和相关材料；
5. 如果 B 也能读取相同材料，邀请 A 不一定比 B 自己读取更有价值。

代码核对确认了这个断点：

- ACP Session 按 `channel_id -> session_id` 隔离；
- Meeting 使用独立 Channel，因此通常建立独立 Meeting Session；
- 首次 Meeting Turn 不会继承原工作 Channel 的模型历史；
- 调度只优先复用当前 Meeting Channel 的 Session，不寻找持有相关 Work 上下文的来源 Session；
- Role Brief、Meeting Board 和 Meeting 历史能恢复身份、当前状态和会内连续性，但不能自动恢复
  原工作 Session 的全部工作上下文。

相关代码入口包括：

- [`SessionState`](../../../../crates/buzz-acp/src/pool.rs)；
- [`MeetingV2CreateParams`](../../../../crates/buzz-sdk/src/builders.rs)；
- [`meeting_participant_intent_prompt.md`](../../../../crates/buzz-acp/src/meeting_participant_intent_prompt.md)。

这一阶段形成的正确观察是：

> 邀请同一个 Agent identity，不等于邀请到了该 Agent 形成相关判断时的上下文。

但讨论随后把这个问题过度抽象成了“认知连续性”，并提出保存 Claim、Evidence、Assumption、
Alternative、Unknown、Snapshot 等持久认知状态。这个方向虽然试图解决 Session 失忆，却没有充分
尊重 Project View 与 Project Context 已有的领域边界。

## 4. 第一次纠正：Meeting 的本质是分担问题上下文

用户进一步指出，Meeting 的本质不是恢复某个 Agent 的完整历史，而是：

> 一个问题的相关上下文分散在项目各处，也被不同 Agent 分别接触。单个 Agent 既难以完整收集，
> 也难以在有限窗口内完整承载，因此需要多个 Agent 分别带着同一问题的相关上下文参会。

由此形成了新的理解：

```text
问题上下文
  ≈ 小型共享部分
  + Agent A 当前承载的相关内容
  + Agent B 当前承载的相关内容
  + Agent C 当前承载的相关内容
```

Meeting 的作用是让多个有限窗口共同处理一个大于任意单个窗口的问题上下文，并通过 Speech、
Directed Handoff 和 Board 逐步对齐，而不是要求某个 Agent 先获得完整上下文。

这一纠正同时暴露了前一阶段“持久认知模型”的问题：它试图重新定义上下文内容和归属，而没有先
确认项目已经怎样提供稳定上下文坐标与按需发现能力。

## 5. 同步最新分支：确认 Project Context 已经完成什么

讨论随后将 `version/v1.0.0` 从 `4b9ed6ee4` 快进同步到 `f5cec1716`。同步内容包括：

- Project Context 领域、存储、Relay、CLI 和 ACP 的阶段一至阶段七；
- Desktop Project Context 图与查询；
- Meeting Community-wide read；
- Meeting Coordinate；
- Action Finalization 中的 Project Context 显式写回。

最新实现确认：

1. [Project View](../../project-view/project-view.md) 已经是项目上下文的稳定坐标系。
2. [Project Context](../../project-context/project-context.md) 使用两个或多个稳定坐标的精确无向
   Edge / Hyperedge 表达“这些坐标共享上下文”。
3. 普通 Project Document 承载解释正文。
4. 当前坐标包括 Project View 对象、Project Document 和 Meeting。
5. Agent 可以使用 `exact`、`incident` 和 `contains-all` 从已知坐标发现 Edge。
6. 查询先返回轻量 metadata，Document 正文、Meeting Board 和 Speech 按需读取。
7. Meeting Coordinate 解决终态会议如何成为后续工作的来源证据。

这意味着 Project Context 已经回答：

```text
上下文锚定在哪里
哪些对象共同关联一段上下文
如何从坐标发现相关 Edge
到哪里读取解释正文和历史来源
```

因此，“Epistemic Thread / Record / Claim Graph”等新领域模型被明确放弃。它们会在现有 Project
Context 之外建立第二套上下文事实和关系，重复已经完成的工作。

## 6. 第二轮阶段性判断：上下文选择、路由与跨 Session 携带

核对最新代码后，讨论一度将剩余问题描述为：

```text
会议议题
  → 得到上下文种子坐标
  → 找到每个 Agent 的相关来源 Session
  → 各 Agent 从 Project Context 中选择相关内容
  → 把有界分片带入各自 Meeting Session
```

这个判断正确识别了几个当前断点：

- Meeting Create 没有结构化的 Project Context 议题坐标；
- `source_channel_id` 只是导航引用；
- Meeting Session 不继承原工作 Session；
- participant Intent 只允许小型定向读取，不适合临场执行完整上下文调查；
- 当前调度按 participant identity 和 Meeting Channel 工作，而不是按相关来源 Session 工作。

但是，这一阶段仍默认“每个 Agent 应得到不同上下文分片”，并开始讨论共同基线、分片覆盖、重复率
和上下文并集。用户指出，这仍然把 Meeting 问题理解成了对同一份项目上下文进行人为拆分。

## 7. 第二次纠正：不能靠会议前人工拆分共享上下文制造差异

Project Context 本来就是为了给所有成员提供一致项目视图和共同可发现的上下文。Community 成员拥有
相同读取边界，从同一个问题坐标执行相同的结构查询，很可能得到相同 Edge、Document 和 Meeting。

如果系统在会议前再决定：

- 哪部分给开源管理 Agent；
- 哪部分给后端 Agent；
- 哪部分作为共同基线；
- 如何减少覆盖重叠；

那么系统实际是在同一上下文上人为分片。这个过程增加调度和判断负担，却没有利用 Agent 长期工作
中已经存在的真实差异，因此被判断为负功。

这一阶段进一步明确：

> 上下文差异不应该由 Meeting 临时制造。差异应来自不同 Agent 长期承担不同责任、执行不同 Work
> 和沿不同项目关系工作时自然形成的关注路径。

## 8. 第三轮阶段性判断：增加独立 Role Context 层

为了表达开源管理 Agent 与后端 Agent 长期工作的自然差异，讨论一度提出在 Project Context 与 Agent
Runtime Context 之间增加 Role Context：

```text
Project Context
  全项目共享

Role Context
  某个 Role 长期积累的局部工作上下文

Agent Runtime Context
  当前 Session 的临时工作集
```

这个方向正确识别了现状只有两个极端：

```text
要么进入全项目共享上下文
要么只留在短命 Runtime Context
```

它也正确观察到：开源管理 Role 与后端 Role 即使共享同一 Project Context，长期工作中仍会分别关注
发布、许可证、社区反馈，或数据模型、迁移、事务和故障路径。

但把这种差异固化为新的 Role Context 存储层仍然存在根本问题。

## 9. 第三次纠正：Role Context 作为独立存储层没有必要

用户指出了独立 Role Context 的三个主要问题。

### 9.1 Project Context 与 Role Context 边界模糊

一段内容何时只影响某个 Role，何时已经具有项目影响，很难稳定判断。要求 Agent 持续在两个上下文
仓库之间分类，会增加维护负担、重复内容和漂移风险。

### 9.2 对 LLM 来说二者都是外置上下文

无论内容存放在 Project Context 还是 Role Context，只要没有加载进当前窗口，它对 Agent 都是外置
内容。真正决定 Agent 当前能够使用什么的不是存储名称，而是 Agent 如何检索和加载。

### 9.3 隔离本身不产生价值

其他 Role 能够读取后端相关上下文没有问题。关键是它们在当前任务中是否沿路径读取了这些内容，
而不是是否被权限或存储边界隔离。

由此，独立 Role Context 层被放弃。所有具有持续价值的项目上下文仍然进入统一 Project Context；
不同 Agent 不需要默认加载与当前 Role、Work 和问题无关的内容。

## 10. 当前收敛：Role Context 是检索视图，不是存储层

讨论最终把“角色上下文”重新解释为：

> Agent 围绕当前问题，以自己的 Role 和 Work 作为检索视角，从统一 Project Context 图中沿一条
> 语义路径取得并加载的动态上下文结果。

形式上可以表示为：

```text
统一 Project Context Graph = G
会议问题或议题坐标       = Q
Agent 当前 Role          = R
Agent 当前相关 Work      = W

RoleContext(Q, R, W)
  := Load(Traverse(G, seed=Q, lens={R, W}))
```

这里的 `RoleContext`：

- 不是新的领域对象；
- 不是新的权限边界；
- 不是 Project Context 的副本；
- 不拥有另一份正文或 Revision；
- 只表示一次有 Role / Work 视角的检索和加载结果。

因此，不同 Agent 仍然使用同一张图、相同 Community 读取权限和相同项目事实，但检索路径可以自然
不同：

```text
问题 A
├── 开源管理路径
│   ├── 开源发布以来的相关问题
│   ├── 社区反馈与贡献者体验
│   ├── 相关开源 Work
│   └── 历史发布 Meeting / Document
│
└── 后端路径
    ├── 后端 Issue 与故障记录
    ├── 数据模型、迁移和事务 Work
    ├── 服务依赖 Context
    └── 历史后端 Meeting / Document
```

差异不是通过人为分片产生，而是由问题在不同 Role、Work 和既有项目关系上的路径自然产生。

## 11. 为什么当前 Project Context 图还不足以支持这种路径

当前图已经具备稳定拓扑，但查询语义仍然很小：

- Edge 是精确、无向的坐标集合；
- Context Document 承载解释正文；
- `exact` 只查精确坐标集合；
- `incident` 只查包含一个坐标的 Edge；
- `contains-all` 只查包含全部输入坐标的 Edge；
- 查询不表达方向、关系类型、重要性或多跳路径；
- 系统不理解 Context Document 正文为什么与当前 Role / Work / 问题相关。

因此，当前图可以回答：

```text
哪些 Edge 直接包含问题 A？
```

但不能稳定回答：

```text
从问题 A 出发，站在开源管理 Role 与其 Work 上，应沿哪些关系和历史继续读取？

从同一问题 A 出发，站在后端 Role 与其 Work 上，又应沿哪些关系和历史继续读取？
```

这不是缺少新的上下文内容层，而是缺少统一上下文图上的语义可检索性和路径能力。

当前 Project Context 规范曾明确把语义搜索、向量检索和 Context Compiler 列为首版非目标。
Meeting 的真实使用问题现在提供了重新评估这些延期能力的具体理由，但不预先证明某一种技术方案
就是正确实现。

## 12. 已经排除的方向

| 已排除方向 | 排除原因 |
|---|---|
| 新建 Epistemic Thread、Claim、Evidence 等持久认知模型 | 重复 Project View、Project Context 和 Document 已有职责 |
| 以 Capsule 或摘要作为新的上下文事实源 | 增加第二份状态，并产生压缩漂移 |
| 在 Meeting 前人工拆分统一 Project Context | 从相同材料制造分片，增加负担且不产生真实差异 |
| 为每个 Role 建立独立 Context 仓库 | 与 Project Context 边界模糊，增加 Agent 分类和维护负担 |
| 通过 ACL 隔离 Role Context | 访问隔离不等于上下文激活差异 |
| 让所有 Agent 从相同问题执行相同语义相似度搜索 | 仍会得到相同结果，不能产生 Role / Work 路径差异 |
| 为了证明 Meeting 有价值而强制不同路径 | 如果真实相关上下文本来相同，系统不应伪造差异 |

## 13. 当前共同结论

当前讨论收敛到以下认识：

1. Project View 继续提供一致的一阶项目状态和稳定坐标。
2. Project Context 继续是唯一的持久项目上下文图，不新增 Role Context 存储层。
3. 所有 Community Member 可以继续读取同一项目上下文；差异不依赖权限隔离。
4. Role 与 Work 不拥有另一份上下文，而是成为检索统一上下文图时的语义视角。
5. 所谓 Role Context，是问题、Role、Work 共同决定的一次动态路径检索和加载结果。
6. Meeting 不拆分 Project Context，也不负责临时制造差异。
7. Meeting 中各 Agent 的上下文差异应来自同一问题沿不同 Role / Work 语义路径得到的结果。
8. 当前 Context Edge 与三种集合查询提供了图拓扑，但尚未提供足够的语义路径检索能力。
9. 下一步讨论对象应是如何增强统一上下文图的语义与路径能力，而不是增加新的上下文层。

可以用一句话概括：

> 不建立独立 Role Context；让 Role 与 Work 成为统一 Project Context Graph 的检索视角，使同一
> 问题沿不同语义路径得到不同但可追溯的上下文，并由 Meeting 让这些路径结果发生作用。

## 14. 当前仍未回答的问题

以下问题留待后续讨论，本文不提供方案：

1. 图的“语义”究竟来自 Project View 结构关系、Context Edge、Document metadata、Document 正文、
   历史 Meeting，还是它们的组合？
2. Role purpose、responsibilities、boundaries 与负责 Work 如何形成检索视角，而不成为硬隔离？
3. 当前无向 Hyperedge 如何参与多跳路径，又如何保留“为什么经过这里”的解释？
4. 如何在有限上下文预算下停止路径扩展，同时不把路径压成无法解释的一组搜索结果？
5. 如何区分结构上可达、文本上相似和对当前问题真正有用？
6. 如何让检索结果保留完整坐标与路径来源，避免 LLM 凭空补全关系？
7. 相同问题的不同 Role 路径高度重合时，系统应如何如实表达，而不是强制差异？
8. 路径结果如何进入 Agent 当前 Runtime / Meeting Session，仍需与现有 Session 隔离问题一起考虑。
9. Agent 在工作中发现新的长期语义后，如何继续通过现有 Document 与 Context Edge 显式写回统一图？

这些问题共同指向“统一 Project Context 图的语义路径检索”，但尚未构成实现设计。

## 15. 第四次纠正：语义路径不应由图引擎预先决定

后续讨论比较了两种方向：

1. 引入完整图引擎，由系统计算语义路径；
2. 让 Agent 基于当前问题、Role、Work 和 Runtime Context，在统一图上渐进查找。

讨论选择第二种方向。现有 Edge 只表达一组精确坐标共享上下文，具体语义由普通 Project Document
承载。图引擎能够执行邻接、模式匹配或最短路径，却不能仅凭当前拓扑判断“站在这个 Role 和 Work
上，哪一段上下文对当前问题真正相关”。如果为了让图引擎作出该判断而预先增加方向、关系类型、
权重、向量或 ontology，会重新引入需要持续维护的语义模型。

因此形成的分工是：

- 图提供真实 Coordinate、Hyperedge 和 Context Document 引用；
- 系统提供轻量、按需、可继续的读取能力；
- Agent 使用当前问题和自己的上下文环境判断下一步读取什么；
- 读取到的正文进入当前 Runtime Context，不形成新的上下文存储层。

## 16. 第五次阶段性设计：为图内 Coordinate 建立独立摘要

为了让 Agent 在读取正文之前判断候选是否值得加载，讨论一度形成如下设计：

- 每个进入 Project Context Graph 的 Coordinate 都有一份统一摘要；
- 摘要在 Agent 首次把 Coordinate 关联进图时，基于其完整当前内容生成；
- 来源内容更新后，由更新 Agent 乐观判断是否需要维护摘要；
- 摘要不参与 Coordinate identity、EdgeKey 或 Edge 判等；
- Agent 先读取 title / summary，再决定是否读取完整内容。

这一抽象随后被展开成独立 Coordinate Node canonical state、Node 协议、Node Head、Node Meta、
Node Catalog、独立 Revision、迁移与分页协议。对应实现推演保留在
[已废弃的 Project Context Agent 渐进检索实现设计](project-context-progressive-retrieval-implementation-design.md)
中。

该推演虽然试图保持 Node 与 Edge 属于同一张逻辑图，但实际上把“提供轻量内容预览”扩张成了
第二套复杂协议，也让分页和投影一致性逐渐取代 Agent 如何沿图查找上下文，成为设计重心。

## 17. 第六次纠正：摘要属于内容，Edge 只负责连接

讨论随后从 Agent 实际遍历图的过程重新审视摘要归属，并形成新的原则：

需要先准确区分：上一阶段设计本来也没有定义 `Edge.summary`，而是让 Edge Preview 返回其绑定的
Context Document metadata。此次纠正不是“删除一个已经设计的 Edge 摘要”，而是重新确认 Edge
没有摘要，并停止把 Node 内容预览扩张成 Project Context 自有的 canonical Node summary 协议。

> 摘要属于可读取的内容，不属于连接关系。

据此，三种结构角色应明确分开：

```text
Node
├── Coordinate identity
├── 内容的 title / summary 预览
└── Coordinate 指向的 canonical 完整内容

Edge
├── exact Coordinate set
└── Context Document membership

Project Document
├── title
├── summary
└── body
```

Edge 本身没有 title、summary、聚合语义、方向、权重或相关度。一条 Edge 为什么值得经过，由其绑定
的普通 Project Documents 解释。给普通 Document 维护摘要本身就具有独立价值：Agent 在任何场景都
可以先看 Document 的 title / summary，再决定是否加载正文。当该 Document 作为 Edge Context
Document 时，同一份摘要自然成为判断这份关系解释是否值得读取的提示，无需再为 Edge 生成一份
重复的聚合摘要。

Node 仍然需要轻量预览，以便 Agent 在一条已选择 Edge 的成员中决定下一个 Node。但
`CoordinatePreview.summary` 只应理解为对 Node 所指内容的预览。摘要的语义与生命周期属于 Node
所指的来源内容实体；查询层只能读取、适配并水合来源的 summary / description 等 metadata，不能
成为摘要 owner，也不能生成第二份 canonical summary。后续需要确定的是不同来源类型如何统一暴露
Preview，而不是摘要究竟归来源领域还是查询层所有。不能因为需要统一 Preview 就直接推出一套独立
的 Node canonical 协议。

## 18. 重新确认 Agent 的标准渐进遍历

在已经选好起始 Node 后，Agent 的标准检索路径是：

```text
读取或复用当前 Node 的完整内容
  ↓
查询该 Node 的真实 incident Edges
  ↓
查看各 Edge 绑定的 Context Document title / summary
  ↓
选择可能相关的 `(edge_key, context_document_id)` 关系材料
  ↓
按需读取选中 Context Document 的正文
  ↓
独立判断是否沿它所属的完整 Hyperedge 继续
  ↓
查看所选完整 Hyperedge 中其他 Node 的 title / summary
  ↓
选择下一个 Node，并读取其完整内容
  ↓
重复上述过程
```

这条路径包含三个不同的判断，不能合并：

1. 哪份 Context Document 的关系解释值得读取；
2. 是否沿它所属的完整 Hyperedge 继续；
3. 这条 Hyperedge 上哪些 Node 值得成为下一步。

选择一份 Context Document 不表示 Edge 上所有 Node 都与当前问题相关。Agent 仍需查看相邻 Node 的
摘要并逐个选择。与此同时，Hyperedge 的完整坐标集合必须保留，不能把一次移动解释为系统中存在
方向、因果或若干隐含二元 Edge。

如果当前 Runtime 已经持有某个 Node 带有可靠来源的完整内容，Agent 可以直接复用，不需要机械重复
读取。摘要只用于选择路径，不作为项目事实证据；形成判断时仍需读取或确认相应完整内容。

## 19. 分页被降回低层有界机制

“点 → 边 → 点”的渐进遍历是语义过程。分页不是这种遍历的组成部分，也不决定路径。

分页只解决单次邻接读取本身可能过大的工程问题。例如一个 Node 可能拥有大量 incident Edges，
一条 Edge 也可能绑定多份 Context Documents 或包含许多 Coordinate。若一次返回全部轻量 metadata，
仍可能填满 Agent 的固定上下文窗口。因此实现可以为单步读取提供小型 `limit` 和 continuation，或者
由领域上限保证响应天然有界。

这类机制只意味着：

```text
本次只查看这一跳的一小批候选；需要时再继续。
```

它不意味着 Agent 应遍历完所有分页，不需要成为独立语义模型，也不应围绕它建立复杂的 Node
Catalog、双 Revision 或全局快照设计。之前把 cursor、一致性和大规模 Frontier 提升为实现主轴，
偏离了 Agent 按内容逐步选择路径的核心。

## 20. 本次设计变更的结果

2026-08-09 的讨论形成以下变更：

1. 重新确认 Edge 只承载精确 Coordinate 集合与 Context Document membership；旧设计也没有
   `Edge.summary`，本次废弃的不是一个已有 Edge 摘要字段。
2. Edge 的语义预览由其绑定的普通 Project Document 的 title / summary 提供。
3. Agent 先根据 Context Document 摘要选择关系内容，再查看所选 Edge 的相邻 Node 摘要。
4. Node 摘要描述 Node 所指内容，并由来源内容实体拥有；查询层只负责统一水合，不持有第二份摘要。
5. 不再预先假定 Node 摘要必须由独立 Coordinate Node canonical 协议持久化。
6. 渐进检索的主流程明确为内容驱动的“Node → Edge Context Document → Node”。
7. 分页只保留为高扇出情况下的低层有界读取能力，不再主导领域和协议设计。
8. 2026-08-08 形成的实现设计整体废弃，仅保留作讨论与反例追溯。
9. 原概念规范中独立 `CoordinateNode.summary` 的生成和生命周期章节也已失效，等待整体重写。
10. 新的实现设计需要基于上述遍历过程重新撰写，不能在旧文档上继续增补。

当前最简洁的共同结论是：

> 图负责提供真实连接，Document 和 Node 内容负责提供轻量语义预览，Agent 结合自己的问题与上下文
> 环境选择路径；Edge 不拥有摘要，分页也不是检索语义。
