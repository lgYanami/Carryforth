# 上下文环境感知的 Project Context 图语义检索

> 本文解释 Carryforth 的一项核心设计：如何让同一个问题在不同 Role、Work 等上下文环境下，
> 从 Project 持有的同一张上下文图中得到不同但仍然相关、可追溯的上下文路径。
>
> 本文讨论产品心智模型，不重新定义查询协议、评分常量、数据库结构或 Provider 运维合同。
> 精确实现边界见
> [Project Context 图语义检索分阶段实现计划](../../stage/semantic/project-context-graph-semantic-query-implementation-plan.md)。

## 1. 核心判断

> 上下文图检索的本质，是根据当前上下文环境，为同一个问题选择不同的项目阅读路径，
> 从而取得不同但相关的上下文；它不是为不同 Agent 分别保存不同版本的项目知识。

Project Context 是 Project 共同持有的上下文图。Role、Work 等环境坐标是观察这张图的软视角：
它们可以改变哪些内容更值得先看、哪些真实关系更值得展开，但不会切割项目上下文、扩大权限，
也不会创建一张属于某个 Agent 的私有图。

```text
                  同一个 Project Context 图
                            │
                       同一个问题 Q
                            │
             ┌──────────────┴──────────────┐
             │                             │
       前端 Role / Work              后端 Role / Work
       上下文环境 E1                  上下文环境 E2
             │                             │
       上下文路径 P1                  上下文路径 P2
             │                             │
       前端相关上下文 C1              后端相关上下文 C2
```

设计希望得到 `P1 != P2` 和 `C1 != C2`，前提是 Project 中确实存在能够区分两种环境的对象、
关系说明和语义证据。系统不会为了制造差异而伪造路径；如果同一条路径对两个环境都最相关，
或者现有项目内容不足以表达差异，两次查询也可以得到相同结果。

## 2. 先区分三个概念

“上下文”“上下文环境”和“上下文路径”相互关联，但不是同一件事。

### 2.1 什么是上下文

这里的上下文，是 Human 或 Agent 为理解当前问题并继续行动，需要装入有限上下文窗口的项目内容。

它由两类内容共同构成：

1. **坐标上下文**：Role、Work、Requirement、Issue、Resource、Document、Meeting 等稳定坐标
   所指向的当前项目内容，回答“这个对象是什么、现在处于什么状态”；
2. **关联上下文**：真实 Edge / Hyperedge 及其 Context Document 所保存的原因、依赖、影响、
   例外和边界，回答“这些对象为什么需要共同理解”。

```text
Coordinate ──回到来源领域──> 对象当前内容
     │
     └──进入真实 Hyperedge──> Context Document ──> 其他相关 Coordinate
```

因此，上下文不是一个脱离项目对象的大文本块，也不是向量数据库中若干“相似内容”的集合。
最终可行动的上下文必须能够回到 Project 中的稳定对象、当前 Revision 和显式关系范围。

关于这两类上下文为什么先从稳定坐标开始，见
[核心设计：先有坐标，后有上下文](coordinate-and-context.md)。

### 2.2 什么是上下文环境

上下文环境描述的是：

> 这一次提出问题时，Human 或 Agent 当前站在 Project 的什么位置、承担什么责任、正在处理什么工作。

它可以由一个或多个稳定坐标表达，例如：

- 当前承担的 Role；
- 当前处理的 Work；
- 正在解决的 Requirement 或 Issue；
- 当前参考的 Document；
- 某次 Meeting 形成的协作现场。

上下文环境属于**这一次查询**，不属于 Agent 身份。同一个 Agent 可以切换不同环境，不同 Agent
也可以从同一个 Role 或 Work 环境观察项目。

环境坐标只声明本次检索的软视角。它不是：

- Agent 私有知识库的名字；
- 持久化的新 Project Context 对象；
- 必须经过的检索起点；
- 只允许访问的子图；
- ACL、成员资格或行动权限；
- 对某个 Role 的 Assignment、全部 Work、图邻域和 Runtime 记忆的自动展开。

当前实现从环境坐标的 current canonical overview 构造有界的查询信号，而不是把该对象全文、
全部相邻对象或 Agent 的私有 Runtime Context 全量注入 Provider。

### 2.3 什么是上下文路径

上下文路径是从统一 Project Context 图中得到的一条可追溯阅读路线。它可以从以下入口开始：

- 被问题语义召回的 Coordinate；
- 被问题语义召回的 Context Document；
- 调用者显式指定的 initial Coordinate。

路径随后只沿 Project 中已经存在的真实关系展开：

```text
Coordinate root
  └── current incident Edge / Hyperedge
        ├── Context Document：这组坐标为什么相关
        └── complete coordinate set
              └── related Coordinate

Context Document root
  └── its current active binding
        └── exact Edge / Hyperedge
              └── related Coordinate
```

上下文路径同时回答两个问题：

1. 接下来值得读取哪些项目对象；
2. 为什么可以从当前对象走到这些对象。

路径不是新的项目事实，也不是复制出来的一份上下文。它是取得上下文的导航和关系证据：

```text
上下文环境决定从什么视角观察
上下文路径决定沿哪些真实关系读取
坐标内容与关联文档构成最终装入窗口的上下文
```

## 3. 为什么始终使用一张统一图

### 3.1 Agent 的上下文需要外置，但不需要私有化

Agent 的上下文窗口有限，Session 和 Runtime 也会结束、压缩或替换。因此，长期项目上下文不能只
存在于某个模型进程的窗口中，必须外置到 Project。

但“外置”不等于“为每个 Agent 建立私有上下文”。真正需要解决的是：

> 这一次应该从 Project 中选择并装入哪些内容？

而不是：

> 哪些 Project 内容属于这个 Agent？

为 Agent、Role 或 Runtime 建立私有上下文，本质上是先隔离原本共同的项目上下文，再增加一层抽象
去管理这种隔离。这绕开了检索问题，却没有解决检索问题。

系统随后还需要处理：

- 公共上下文与私有上下文之间的复制和提升；
- 多个私有空间之间的同步、合并与冲突；
- Assignment、Role、Session 或 Runtime 替换时的迁移；
- 一段项目关系究竟在哪个空间才是当前版本；
- Agent 每次行动前应该先查询哪个上下文空间。

这些机制既增加系统维护成本，也增加 Agent 的认知负担。更重要的是，它们会把原本跨 Role 存在的
真实项目约束隐藏在人工建立的隔离边界之后。

### 3.2 统一的是项目身份、来源和关系

Carryforth 因此只维护一张 Project-owned Context Graph：

- Requirement、Issue、Work、Resource、Document、Meeting 等对象各自保持一个稳定身份；
- 对象内容继续由来源领域和 Revision 管理；
- 跨对象语义继续由精确 Edge / Hyperedge 和版本化 Context Document 承载；
- 不同查询通过不同环境视角选择不同路径，而不是复制对象或关系。

统一图不表示把全部项目内容塞进每次 Prompt，也不表示所有成员无条件读取全部内容。
查询仍受 Project / Community、调用者身份、来源领域、生命周期和能力门约束，只把当前查询获准且
真正需要的内容逐步暴露给调用者。

Agent 仍然可以有本地临时状态、草稿或 Runtime Memory；它们只是该 Agent 的工作材料，
不是 Project 连续性的权威载体。会影响其他成员、后续责任或未来工作的内容，需要显式写回共同的
Project View、Document、Context、Meeting、Checkpoint 或其他规范对象。

### 3.3 隔离观察视角，而不是隔离项目上下文

最终选择可以概括为：

> 不为 Agent 隔离项目上下文，而是为每一次查询指定一个上下文环境。

前端与后端得到不同内容，不是因为它们各自拥有一份知识，而是因为它们从同一份项目知识中，
沿不同观察视角选择了不同阅读路线。

## 4. 问题、起点和环境是三个正交输入

一次图语义查询可以包含三类输入：

| 输入 | 回答的问题 | 语义 |
|---|---|---|
| `problem` | “我现在要解决什么？” | 必填，始终主导召回 |
| `initial_coordinates` | “我明确要从哪里开始？” | 可选的结构起点 |
| `context_coordinates` | “我现在站在什么环境中观察？” | 可选的软召回与排序视角 |

这三者不能互相替代。

### 4.1 Problem 决定寻找什么

自然语言问题是主信号。没有任何 initial 或 context Coordinate 的 problem-only 查询是合法入口，
适合调用者尚不知道图中有哪些坐标时先发现候选根。

更换上下文环境不应把问题改写成另一个问题。无论使用前端还是后端 Role，“召回与用户控制体验
现在是怎么设计的？”仍然是同一个问题。

### 4.2 Initial Coordinate 决定从哪里走

initial Coordinate 是调用者明确选择的结构根。它适合表达“必须从这个 Work 或 Role 开始看”。

它不是上下文环境的同义词，也不会把全局候选召回限制为该坐标的私有子图。一个 Coordinate
可以同时作为 initial 和 context：前者表达行走起点，后者表达观察视角。

### 4.3 Context Coordinate 决定什么更值得先看

context Coordinate 参与相关性判断，但不会自动成为 root、硬过滤器或必经点。

如果产品需要确定地检查某个 Role / Work 的直接关系，应使用 initial Coordinate 或精确的
incident / contains-all 结构查询；不能把确定性结构要求交给软语义视角。

## 5. 如何让环境影响结果而不吞掉问题

### 5.1 Neutral 与 Conditioned 两种观察

当前实现先形成 problem-only 的 neutral 查询，再为每个环境坐标分别形成 conditioned 查询：

```text
Q0 = problem
Qi = problem + context_coordinate_i 的 current canonical overview
```

统一语义索引中的 Coordinate 和 Context Document 候选会分别取得：

- 它与 `Q0` 的 problem relevance；
- 它与每个 `Qi` 的 conditioned relevance；
- conditioned relevance 相对 problem relevance 的正向 environment gain。

环境只贡献“因为站在这个位置，所以这个候选额外值得关注”的增量。它不能用一个低 problem
relevance 的环境相似项完全改写问题，也不能把负增益包装成环境证据。

### 5.2 环境影响是有界的

当前评分结构让 problem 信号占主导，environment 只占有界部分，显式 initial anchor 只提供更小的
结构补充。多个环境中也只有最强和次强的一小部分继续贡献，避免堆叠许多 context Coordinate
把问题本身淹没。

自动 root 还保持两项 neutral 保护：

- 最强 problem-only root 被保留；
- 至少一部分 root 配额留给 neutral 候选。

这些限制意味着环境可以改变候选集合、排序和最终路径，却不能形成“只准从某个 Role 的世界里找”
的检索隧道。

### 5.3 不强制制造差异

“不同上下文环境得到不同上下文路径”是这项设计要达到的能力，而不是对每一次输入强制制造差异。

两次查询可能返回相同路径，常见原因包括：

- 这条路径确实同时是两个环境下最相关的共同上下文；
- Role / Work 的 overview 区分度不足；
- 对应 Work、Document 或 Edge 尚未被项目显式建立；
- 相关来源还没有 current semantic head；
- 候选虽然得到 environment gain，但在 root 或路径预算内仍未胜出；
- 关系说明本身不足以支持更细的语义区分。

系统不能为了满足“结果必须不同”而虚构关系、放宽权限或忽略问题相关性。正确修复方向是改善项目
建模、语义输入、候选召回和有界排序，而不是拆分上下文图。

## 6. 语义只选择路径，真实图决定能走到哪里

语义相似度只负责选择和排序已有候选。它不能创建相邻关系。

一次 Coordinate hop 的结构是：

```text
当前 Coordinate U
  → U 所在的真实无向 Hyperedge E
  → E 当前绑定的一份 Context Document D
  → E 完整坐标集合中的另一个 Coordinate V
```

每份 Context Document 独立提供一份关系语义。系统先判断哪份关系说明与问题和环境更相关，
再在这条完整 Hyperedge 的真实成员中选择后续坐标。

必须保持以下边界：

- Edge / Hyperedge 是无向的；
- `U → E → V` 只是本次查询的行走顺序，不是领域中的因果、依赖或时序方向；
- `{A, B, C}` 是一个精确三元关系范围，不自动产生 `{A, B}`、`{A, C}` 或 `{B, C}`；
- 返回某份 relation Document 不表示它概括了整条 Edge 的所有含义；
- 生命周期和 readiness 可以阻止某个目标继续展开，但不能从结果中的完整 Edge 身份里删除成员；
- 查询不会自动创建、补全、拆分或修改任何 Edge。

因此，语义路径仍然保留显式关系依据，而不是退化成“这两个文本向量很像”。

## 7. 可追溯结果如何变成可用上下文

### 7.1 每条路径保留什么

返回结果会保留理解和复核路径所需的结构与来源证据，包括：

- root 的来源类型和稳定身份；
- 每一 hop 的 Edge key 与完整 Coordinate 集合；
- Edge 当前绑定的 Context Documents；
- 本次选中的 relation Document 和 exact binding；
- Coordinate、Document、Meeting 的来源 Revision / change basis；
- 语义 generation、snapshot 与评分解释；
- 覆盖、停止和省略原因。

这让调用者能够区分：什么是项目显式保存的关系，什么只是本次查询的选择和排序。

### 7.2 Currentness 是查询快照，不是永远最新

召回、水合和图遍历在同一个一致的 Stage C 数据库快照内完成。结果证明的是“在这个快照中，
这些来源和关系具有这些 Revision 与 currentness 证据”，不是“响应到达时它们仍然没有变化”。

如果对象在查询后发生更新，调用者需要比较结果证据和当前 canonical readback，而不能把旧路径
当作永久冻结的项目状态。

### 7.3 签名结果与 canonical readback

Relay 对结果 Event 签名，并把它绑定到当前 Project、调用者和精确请求正文。这证明：

> 该 Relay 为这次请求返回了这一份快照派生结果。

它不证明相关性天然正确、关系文档内容真实、结果已经穷尽所有可能或相关路径，
也不证明项目应该按照该结果行动。已返回 hop 的 exact Edge、完整坐标集合、binding、连续性和
请求预算仍由 closed result contract 与 SDK 验证。

`cf` 在验证签名结果后，会根据其中的稳定身份另外派生未签名但规范化的 `read_commands`，
供调用者读取 Project View 对象、Document 和 Meeting 的当前权威内容。这些命令读取的是执行时的
current state，可能已经比查询快照更新；它们不是签名结果的一部分，也不是精确重放查询快照。

最终装入 Agent 窗口的上下文，应来自这些 canonical 对象和关系证据，而不是只使用向量预览或分数。

## 8. 一个前端与后端环境的例子

假设 Project 中有同一个授权 Issue，以及两组真实关系：

```text
Edge F = {
  授权 Issue,
  前端 Role,
  Desktop Work,
  前端交互 Document
}

Edge B = {
  授权 Issue,
  后端 Role,
  Relay Work,
  后端授权 Document
}
```

两条 Edge 各自绑定 Context Document，解释对应坐标为什么需要共同理解。

对于同一个问题“召回与用户控制体验现在是怎么设计的？”：

- problem-only 查询应先发现项目中整体最相关的入口；
- 前端 Role / Desktop Work 环境应提高前端对象和关系说明成为 root 或路径材料的机会；
- 后端 Role / Relay Work 环境应提高后端对象和关系说明的机会；
- 两次结果仍可以共享授权 Issue、共同 Requirement 或跨端约束，因为它们来自同一 Project；
- 每条路径都必须沿 `Edge F` 或 `Edge B` 的真实完整集合，而不能根据“前端”“后端”文本临时造边。

期望不是得到两份互不相干的答案，而是得到**不同但相关**的项目上下文：共同问题仍然可见，
当前责任和工作环境决定哪些关系更值得优先展开。

如果必须从 `Desktop Work` 确定出发，应把它同时或单独作为 initial Coordinate；如果只是希望它影响
优先级，则把它作为 context Coordinate。

## 9. Agent 应如何使用这项能力

一个典型使用流程是：

1. 明确当前自然语言问题；
2. 从已经验证的 current Role、Work 或其他项目对象中选择本次上下文环境；
3. 如果已知必须从某个对象开始，另外提供 initial Coordinate；
4. 执行查询，检查 neutral 与 conditioned evidence、coverage 和停止原因；
5. 沿返回路径对稳定对象执行 canonical readback；
6. 用对象当前内容和 Context Documents 组装实际工作窗口；
7. 如果发现项目缺少真实关系，使用普通领域操作显式创建或修订 Document / Edge；
8. 不把本次 retrieval path、分数或模型判断直接写成项目事实。

Managed Agent 的 Harness 不应仅凭进程身份猜测当前 Role 或 Work。上层调用者应从 current Project
状态取得并明确传入合适的环境坐标；错误或过时的环境也不能因此获得额外权限。

## 10. 不改写 Project 规范事实与关系

图语义查询对 Project 规范状态是派生读取。它不会：

- 创建、更新或删除 Project Context Edge；
- 创建、修订或 tombstone Project Document；
- 修改 Project View、Meeting、Role、Assignment、Work 或 Commitment；
- 持久化问题、query vector 或 retrieval path；
- 把相似度升级为关系、事实、责任或权限；
- 把某个 Role / Agent 变成上下文所有者。

Embedding 与 semantic generation 是可删除、可重建的派生索引，不是新的 Project 事实源。
Provider admission、限流配额和运行指标可以更新派生运营状态；这些记录不包含问题正文，也不构成
Project View、Document、Context、Meeting 或 retrieval path 的持久写回。

权限验证独立于相关性评分。Community membership、调用者身份、来源可见性、query gate、生命周期
和 currentness 在 Provider 出域、候选召回和结果释放的相应边界继续生效。环境坐标、图相邻、
相似命中和 Relay 签名都不能扩大读写、Runtime、Sandbox、Secret 或外部系统权限。

## 11. 当前实现与资格边界

当前代码已经具备以下机制：

- problem-only、explicit initial 和 context lens 三类输入；
- 每个 context Coordinate 独立产生 conditioned evidence；
- problem 主导、environment 有界的候选评分；
- neutral root 保留；
- 沿真实无向 Hyperedge 的多跳遍历；
- 完整 Edge / binding / source / semantic provenance；
- Relay-signed exact request binding；
- `cf` 验证与 canonical readback 导航；
- 查询不改写任何 Project 关系。

但“机制已实现”不等于“相关性目标已经完成资格化”。现有验收已经证明后端 Role / Work 环境可以
提升后端 Work 和对应 Edge，也证明前端环境能够产生 conditioned gain；它尚未证明前端与后端等
所有具有区分度的环境都能在默认 root / path 预算内稳定返回人类预期的不同路径。

因此，当前准确结论是：

> 统一图上的上下文环境 lens 已经能够影响召回和排序；“不同环境稳定得到语义正确的不同路径”
> 仍是需要继续校准和验收的产品目标。

这项能力还需要 Provider、语义索引、Community index/query gate 和问题数据出境确认；相关性、
资源隔离、长期运行和生产部署仍在资格化中。不能把它描述为生产就绪，也不能把 environment gain
解释为事实置信度、因果证明或项目优先级。

## 12. 非目标

这项设计不试图：

- 为每个 Agent、Role 或 Work 建立私有知识图；
- 保证更换环境后结果必然不同、互斥、唯一或完整；
- 让 context Coordinate 成为 ACL、硬过滤器或自动 root；
- 用向量相似度自动发现并保存 Project Context Edge；
- 把 Hyperedge 拆成若干隐含二元关系；
- 判断 Context Document 的内容天然正确、充分、无冲突或未过期；
- 用 Relay 签名为语义相关性背书；
- 把 retrieval path 当成新的长期 Agent Memory；
- 取代 Human / Agent 对 Project Context 的显式维护。

## 13. 由此得到的设计原则

1. **Project 拥有上下文，Agent 只按需读取。** 连续性不依赖某个窗口、Session 或 Runtime。
2. **外置不等于私有化。** 有限窗口需要检索，不需要人为拆分项目知识。
3. **环境属于查询，不属于身份。** Role / Work 是观察位置，不是上下文所有者。
4. **问题始终主导。** 环境只提供可解释、有界的边际影响。
5. **不同路径来自真实差异。** 不为制造差异而伪造关系或牺牲共同问题语义。
6. **语义选择路径，图结构约束路径。** 只沿真实、完整、无向 Hyperedge 寻路。
7. **路径必须可追溯。** 每一步都保留 Coordinate、Edge、Document、Revision 和来源依据。
8. **派生读取必须回到规范事实。** 签名结果和分数不替代 canonical readback。
9. **检索绝不隐式写回。** 只有显式、授权的普通领域操作才能改变 Project。
10. **相关性不产生权限。** 环境、相似度、相邻关系与签名都不能扩大授权。

上下文环境感知的图语义检索最终解决的是：

> 当 Human 或 Agent 只能在有限窗口中工作时，如何根据当前 Role、Work 和问题，从 Project 共同持有的
> 上下文图中选择一条有依据的阅读路线，获得适合当前行动的上下文，同时不把 Project 拆成彼此漂移的
> 私有记忆空间。

## 继续阅读

- [Carryforth 核心模型](../core-model.md)
- [核心设计：Role Continuity](role-continuity.md)
- [核心设计：先有坐标，后有上下文](coordinate-and-context.md)
- [核心设计：Meeting](meeting.md)
- [Project Context 领域规范](../../stage/project-context/project-context.md)
- [Project Context 图语义化基础规范](../../stage/semantic/project-context-graph-semantic-foundation-spec.md)
- [Project Context 图语义检索实现计划](../../stage/semantic/project-context-graph-semantic-query-implementation-plan.md)
- [Project Context Desktop 图语义查询资格记录](../../stage/semantic/desktop/project-context-semantic-query-desktop-qualification.md)
- [语义 pgvector 运维](../../semantic-pgvector-operations.md)
- [当前状态与能力边界](../current-status.md)
