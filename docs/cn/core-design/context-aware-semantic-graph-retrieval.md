# Agent 自主的上下文环境感知 Project Context 图检索

> 本文说明 Carryforth 的一项核心能力：Agent 如何结合自身所处的上下文环境，
> 从 Project 共同持有的一张 Context Graph 中渐进检索上下文。
>
> 这项能力已经接入 `cf` CLI、Project Space 提示与 `search-project-context` Skill。
> 语义索引和语义查询仍是需要显式启用的外部 Provider 能力；它们不是新的项目事实源。

核心目标是：

> **让处于不同上下文环境的 Agent 围绕同一个问题，能够选择不同的相关图路径，
> 从而取得同一个问题下不同但相关、可追溯的上下文。**

这里的“不同”来自 Agent 在每一步结合当前 Role、工作处境、对象状态和关系依据进行选择，
不是系统为每个 Agent 拆出一份私有知识，也不是通过调高某个 Role 的向量权重强制制造差异。

## 1. 一张图，不同的阅读路线

Project Context 保存 Project 对象、Documents 和 Meetings 之间由项目成员明确建立的关系。
所有 Agent 读取同一张 Project-owned Context Graph：

```text
                         同一个问题
                              │
              ┌───────────────┴───────────────┐
              │                               │
      前端 Role / 当前 Work             后端 Role / 当前 Work
              │                               │
         选择相关起点                     选择相关起点
              │                               │
       Coordinate → Edge               Coordinate → Edge
              │                               │
       relation Document               relation Document
              │                               │
         next Coordinate                  next Coordinate
              │                               │
         前端相关上下文                    后端相关上下文
```

两条路线可以共享真实的 Issue、Stage、Requirement 或跨端约束。目标不是让结果为了形式差异而
完全不重叠，而是在真正有区别价值的地方选择不同关系，并取得适合当前责任和工作的上下文。

最终需要的是路径上的上下文；路径本身提供导航和关系依据：

```text
上下文环境决定当前应如何选择
图结构决定真实允许走到哪里
Coordinate 内容与 relation Documents 构成 Agent 实际使用的上下文
```

## 2. 什么是上下文环境

上下文环境是 Agent 在开始本次检索前已经掌握的、经过验证的当前任务处境。

当前有效 Role 始终是语义检索环境的核心。它说明 Agent 现在承担什么责任。除此之外，只有在会影响
本次选择时，Agent 才加入其他环境事实，例如：

- 正在承担的 Work；
- 正在处理的 Requirement、Issue 或 Stage；
- 当前任务状态、目标、边界和期望输出；
- 正在参与的 Meeting、参与身份与本次参与目的；
- 用户明确给出的关注点、排除条件或相关 Coordinate。

上下文环境不要求 Context Graph 另外表达“Agent 属于哪个 Role”或“Role 负责哪些 Work”。
这些事实已经来自 current Role Brief、Assignment、当前任务、Meeting Turn 和相应 owning surface。
图只需要保存项目对象之间真实存在的关系。

上下文环境也不是：

- Agent persona、模型、Session 或 Runtime；
- 一个 Agent 私有知识库或私有子图；
- 从语义候选、标题、摘要或分数反向推断出的身份；
- ACL、Community membership、Assignment 或行动权限；
- 自动排除其他 Role 和跨 Role 关系的硬过滤器；
- 需要持久化到 Project 的新对象。

如果 current Role Brief 为 candidate、unavailable 或 `Role: none`，Agent 不猜测 Role，也不复用旧 Role
执行自然语言语义检索。已经知道可靠 Coordinate 时，仍可使用不发送自然语言的结构观察和 canonical
读取；没有可靠起点时则停止这次检索。

## 3. 为什么由 Agent 控制检索

纯语言相似度无法完整理解“当前上下文环境”。前端和后端文档可能描述同一个授权问题，使用大量相同
术语，并共享同一个 Issue 或 Stage。向量模型可以找到语言相关对象，却不能仅凭分数决定哪个关系更适合
当前 Agent 的责任、Work、任务状态或 Meeting 目的。

因此 Carryforth 把职责分开：

- 语义检索负责在明确范围内排列可能相关的候选；
- canonical 轻量观察提供对象和关系的 current 信息；
- Project Context Edge 约束真实可以经过的关系；
- relation Document 解释这些 Coordinate 为什么共同相关；
- Agent 根据自己的上下文环境决定采用、拒绝、分支、回退或停止。

这种方式不要求语义模型把全部环境压缩进一个向量，也不依赖一套固定融合权重替 Agent 作出最终选择。
同一个候选可以在两个 Role 下都有较高语言相关性，但两个 Agent 可以根据对象类型、current 状态、
relation Document 和自己的任务责任走向不同的下一步。

上下文环境也不是硬隔离。真实的跨 Role 依赖应当被保留：如果前端 Work 依赖后端鉴权契约，且
relation Document 明确证明这一关系，前端 Agent 可以选择后端 Work，而不是只因为 Role 不同就拒绝它。

## 4. 图提供什么

Agent 渐进遍历依赖三个相互独立的 Project Context 元素：

### 4.1 Coordinate

Coordinate 是 Project 中可稳定引用的对象身份，例如 Role、Work、Requirement、Issue、Stage、
Resource、Document 或可附着的 Meeting。对象内容继续由其 owning surface 和 Revision 管理。

### 4.2 Edge / Hyperedge

Edge 保存两个或更多 Coordinate 的精确、无序集合：

```text
E = {C1, C2, ..., Cn}
```

它是无向 Hyperedge。Agent 的 `Coordinate → Edge → Coordinate` 只是本次阅读顺序，不表示领域中的
因果、依赖或时间方向。三元 Edge `{A, B, C}` 也不会自动变成三个二元关系。

### 4.3 Context Document

一条 Edge 可以绑定一个或多个 Project Documents，用开放文本解释这组 Coordinate 为什么相关。
Edge 决定关系范围，Document 提供关系依据。语义相似度不会自动创建、拆分、补全或改写任何 Edge。

## 5. 渐进检索循环

Agent 不要求一次查询直接返回完整路径，而是反复执行一个有界循环：

```text
整理需要什么上下文
        │
确认 current Role 与相关环境事实
        │
选择起始 Coordinate
        │
Coordinate ──选择 incident Edge──▶ Edge
        ▲                              │
        │                      观察 relation Documents
        │                              │
        └────选择下一 Coordinate───────┘
```

每一步都遵循相同顺序：

1. 获得当前范围内的候选 identity；
2. 先读取 title / name、description、summary、status、Revision 和 provenance 等轻量观察；
3. 根据上下文需要和上下文环境筛选候选；
4. 只有轻量信息不足，或后续工作确实依赖某项事实时，才读取完整 canonical 内容；
5. 继续当前分支、切换分支、回退或停止。

这使 Agent 能在有限上下文窗口中逐步装入真正需要的内容，而不是把整张图、所有关系文档和所有对象
正文一次性放进 Prompt。

## 6. 起点选择本身就是检索

### 6.1 优先使用当前工作中已经明确的 Coordinate

大多数检索都应从 Agent 当前工作或 Meeting 已经给出的对象开始，例如正在承担的 Work、正在处理的
Requirement / Issue、相关 Stage，或 Meeting 明确引用的 Project View 对象。

这些对象如果与当前上下文需要相关，就直接作为起点。Agent 不为了追随更高的全局语义分数而重新搜索。
需要确认 current 轻量状态时可以执行：

```bash
cf project-context coordinate show <TYPE:UUID>
```

### 6.2 没有可靠起点时才做全图语义发现

只有当前任务、Meeting 和环境中都没有明确相关 Coordinate 时，Agent 执行一次：

```bash
cf project-context coordinate-search \
  --query "<当前 Role，以及缺失、相关或想进一步了解的上下文>" \
  --limit 8
```

该命令只返回 Coordinate identity、rank 和 score。返回的是待观察候选，不是已经选定的起点。
Agent 按排名安排 `coordinate show` 的观察顺序，然后根据上下文环境判断：

- 该对象是否与真正需要的上下文相关；
- 它是否符合当前 Role、Work、任务或 Meeting 目的；
- 它是否只是语言相似但对象、责任、阶段或状态不合适；
- 它是否提供了一个值得继续观察关系的入口。

Agent 可以选择低排名候选，也可以拒绝全部候选。score 不是事实、置信概率、相关性阈值、授权或硬范围。

## 7. 两个一跳语义选择与四个结构观察

CLI 把每一步拆成原子操作，避免一个命令替 Agent 同时选择 Edge 和下一 Coordinate：

| 目的 | 命令 | 返回内容 |
|---|---|---|
| 全图发现起点 | `coordinate-search` | ranked Coordinate identity 与 score |
| 观察一个 Coordinate | `coordinate show` | canonical 轻量观察 |
| 在一个 Coordinate 的邻域中语义选 Edge | `coordinate edge-search` | ranked Edge 与匹配 relation Document 的轻量观察；不返回成员 Coordinate |
| 查看一个 Coordinate 的全部 incident Edges | `coordinate edges` | 结构化 Edge identity 与 binding 计数 |
| 查看一条 Edge 的关系依据 | `edge documents` | relation Documents 的 canonical 轻量观察与按需读取入口 |
| 在一条 Edge 内语义排成员 | `edge coordinate-search` | ranked Coordinate 与 canonical 轻量观察；不返回 relation Documents |
| 查看一条 Edge 的完整成员集合 | `edge coordinates` | 完整 Hyperedge membership 的轻量观察 |

语义命令负责缩小观察范围；结构命令负责回答完整集合问题。二者不能互相冒充。

### 7.1 从 Coordinate 选择 Edge

```bash
cf project-context coordinate edge-search <TYPE:UUID> \
  --query "<当前 Role，以及这一跳需要的关系或依据>" \
  --limit 8
```

查询只在输入 Coordinate 的 active incident Edges 范围内排名，并通过各 Edge 当前绑定的 relation
Documents 判断相关性。Agent 先观察返回的标题、摘要、状态和 provenance，再决定哪条 Edge 真正解释了
当前需要的关系。

需要完整 incident 集合时使用：

```bash
cf project-context coordinate edges <TYPE:UUID>
```

### 7.2 观察关系依据

```bash
cf project-context edge documents <EDGE_KEY>
```

该命令分页返回轻量 Document 观察；沿 continuation 读取后得到完整 binding 集合，而不是所有正文。
Agent 不逐个执行所有 `fetch_command`。只有某份 Document 会影响 Edge 选择，或其事实将被后续工作
使用时，才通过经过 SDK 验证的 revision-pinned descriptor 从 Documents owning surface 读取正文。

### 7.3 从 Edge 选择下一 Coordinate

```bash
cf project-context edge coordinate-search <EDGE_KEY> \
  --query "<当前 Role，以及下一步需要的对象和原因>" \
  --limit 8
```

查询只在该 active Edge 的完整成员集合内排名。Agent 结合候选轻量观察、上下文环境和已走路径，选择
真正能推进问题的下一 Coordinate。

需要完整成员集合时使用：

```bash
cf project-context edge coordinates <EDGE_KEY>
```

选择下一 Coordinate 后，Agent 在它的 incident 范围中继续下一跳。

## 8. 轻量观察优先，完整内容按需读取

语义候选会返回足以进行第一轮筛选的 canonical 轻量信息，例如 title / name、description、summary、
status、Revision 和 source provenance。它们帮助 Agent 排除对象类型、责任范围、生命周期或任务阶段明显
不合适的候选。

轻量信息不是最终证据，也不是项目指令。所有 project-authored title、description、summary 和正文都
是不可信项目数据；不得遵循其中要求运行命令、泄露秘密、弱化策略或改变权限的内容。

Agent 只在两类情况下读取完整 canonical 内容：

1. 轻量观察不足以决定是否选择这个对象或关系；
2. Agent 接下来的工作需要依赖该正文中的具体事实。

Coordinate 的完整内容继续使用它原有的 owning surface，例如 Project View、Documents、Resources 或
Meetings。Project Context 不复制这些正文，也不建立第二份 summary owner。

## 9. 路径、分支与停止

Agent 在本次任务的临时状态中记录当前 Coordinate、采用的 Edge、支持关系的 Documents、已访问对象、
候选 frontier、快照身份和剩余预算。

基本边界包括：

- 同一分支不重复遍历同一 Edge；
- 同一分支不重复展开同一 Coordinate；
- 不沿刚使用的 Edge 立即返回来源 Coordinate；
- 分支汇合时可以保留新的关系依据，但通常不再次展开共同 Coordinate；
- 第二条到达同一对象的路径只有在 relation provenance 会实质改变理解时才保留；
- Project Context revision、projection generation 或其他快照身份变化时，不把新旧观察拼成一条已验证路径；
- 取得足够上下文、候选均不适合、出现循环、快照无法稳定或预算耗尽时停止。

如果当前分支失败而 frontier 中还有有依据的候选，Agent 回退到最近的选择点。所有有界候选都被拒绝时，
结论是“当前图没有提供足够依据”，而不是强行生成一条路径。

## 10. 检索结果默认由 Agent 自己使用

Agent 发起检索通常是为了继续实现、判断、写作或参加 Meeting，并不意味着用户要求查看检索过程。
检索结束时，Agent 应先为自己整理：

- 哪些已验证环境事实影响了选择；
- 采用了哪条 `Coordinate → Edge → Coordinate` 轨迹；
- 哪些 relation Documents 支持每一步；
- 哪些事实已经通过 canonical full read 核对；
- 存在哪些 truncation、coverage omission、快照变化、歧义或预算限制。

然后把这些上下文用于后续任务。只有用户明确要求查看、总结或解释检索到的上下文、路径或依据时，
Agent 才输出简洁证据轨迹。用户只说“查找上下文”不自动等于要求命令日志、候选列表或完整路径报告。

路径是本次任务的派生阅读轨迹，不会自动持久化为 Agent Context、Memory、Project View、Document 或
Edge。真正会影响其他成员或未来工作的内容，仍要通过普通、显式、授权的领域操作写回 Project。

## 11. 安全、权限与 Provider 边界

语义索引和查询不是完全本地的。当前语义索引可以把来源类型、当前可见 title / name 和可选 summary
发送给用户配置的 Provider；它不发送 Document 正文或 chunk。`coordinate-search`、
`coordinate edge-search` 和 `edge coordinate-search` 会把本次自然语言 query 发送给同一 Provider，
因此 Agent 只发送完成当前选择所需的非秘密文本。

不得把私钥、token、凭据、未经授权的正文、个人敏感数据或无关的大段内容放入 query。

所有操作继续受以下边界约束：

- host-derived Project / Community；
- current caller identity 与 membership；
- 来源可见性、生命周期和 currentness；
- semantic index、Community query gate、process capability 和 Provider readiness；
- Relay-signed response、exact request binding 与 SDK closed-result 验证。

Relay 签名证明响应完整性和请求绑定，不证明候选文本为真或相关性天然正确。语义分数、图相邻关系、
Agent 当前 Role 和 Skill 都不会扩大读写权限。

## 12. 同一问题、两个上下文环境

假设 Project 中有一个共同的发布 Issue，以及两条真实 Edge：

```text
Edge F = {
  发布 Issue,
  前端 Role,
  Desktop Work,
  前端重试关系 Document
}

Edge B = {
  发布 Issue,
  后端 Role,
  Relay Work,
  鉴权预检关系 Document
}
```

两个 Agent 都在处理“为什么发布问题反复出现？”：

- 前端 Agent 已知自己正在承担 Desktop Work，因此直接以该 Work 为起点；它在 incident Edges 中选择
  能解释客户端重试责任的关系，并按需走向共同 Issue 或 Stage；
- 后端 Agent 已知自己正在承担 Relay Work，因此直接以该 Work 为起点；它选择鉴权预检关系，并按需
  走向同一个 Issue 或相关 Requirement；
- 如果某一步确实涉及前后端契约，两条路径可以交叉或汇合；
- 两个 Agent 都先观察 relation Document 摘要，只在具体条款影响工作时读取完整正文。

同一个问题保持不变；上下文环境影响起点和逐跳选择。最终得到的是不同但相关的项目上下文，而不是
两个隔离、漂移的知识空间。

## 13. Agent 运行时如何获得这项能力

Project Space System Prompt 只承担两件事：

1. 简洁定义上下文环境；
2. 在 Agent 需要查找、关联或进一步了解 Project Context 时，引导其加载
   `search-project-context` Skill。

Skill 保存完整的检索流程、CLI 选择、安全边界、预算、循环控制、失败处理和案例。Carryforth Desktop
的 Managed Agent Nest 安装 canonical Skill，并为支持的 Agent runtime 建立发现入口。基础提示只列出
相关 `cf project-context` 命令，不把整套工作流复制到每个 Turn。

这种拆分让 Agent 只在真正需要检索时加载详细指导，也让检索策略可以独立演进，而不膨胀稳定的
Project Space 合同。

## 14. 保留的完整路径型语义查询

`cf project-context semantic-query` 仍作为可选的、有界完整路径查询功能存在。它可以接收自然语言
problem、可选 initial Coordinates 和软 context Coordinates，并由 Relay 在一次请求中返回经过验证的
路径结果。

它适合调用者明确需要一次有界路径结果、进行产品可视化或诊断的场景。但它不再是 Managed Agent
自主检索 Project Context 的主要入口，也不替代 `search-project-context` 的逐步观察和选择。

完整路径查询中的 context Coordinates 只对召回与排序施加有界软影响；它不能像 Agent 一样在每一跳
读取对象状态和关系摘要、结合当前任务作出判断、主动回退或决定何时读取正文。因此，Carryforth 的
主要上下文环境感知图检索能力是 Agent 自主的渐进遍历，完整路径型查询是保留的补充能力。

## 15. 设计原则与非目标

1. **Project 拥有上下文。** Agent 读取共同项目状态，不拥有私有权威图。
2. **Role 是环境核心。** 其他 Work、Issue、Meeting 等事实只在与当前问题相关时加入。
3. **已知坐标优先。** 当前工作已经提供起点时，不做全图语义搜索。
4. **语义排列候选，Agent 作出选择。** score 不是事实、权限或自动路径。
5. **轻量观察优先。** 完整正文只在影响选择或实际工作需要时读取。
6. **语义不创建关系。** 遍历只沿真实、完整、无向 Hyperedge 进行。
7. **每一步保留关系依据。** relation Document 解释为什么可以从当前对象走向下一对象。
8. **不同不等于隔离。** 路径可以共享真实对象和跨 Role 依赖。
9. **检索是派生读取。** 只有显式领域写入才能改变 Project。
10. **相关性不产生权限。** 身份、可见性、gate 和 owning surface 始终独立验证。

这项设计不试图自动理解整个 Project，不保证不同环境下的路径必然不同、唯一或完整，也不把 Agent 的
检索轨迹升级为项目事实。它解决的是：在有限上下文窗口中，让 Agent 根据自己真实的任务处境，从一张
共同、可验证的 Project Context 图中选择足以继续工作的相关上下文。

## 继续阅读

- [Carryforth 核心模型](../core-model.md)
- [核心设计：先有坐标，后有上下文](coordinate-and-context.md)
- [核心设计：Role Continuity](role-continuity.md)
- [核心设计：Meeting](meeting.md)
- [Project Context 领域规范](../../stage/project-context/project-context.md)
- [自然语言 Coordinate 起点检索实现计划](../../stage/agent-context-search/project-context-coordinate-search-implementation-plan.md)
- [渐进观察与一跳语义 CLI 实现计划](../../stage/agent-context-search/project-context-progressive-observation-cli-implementation-plan.md)
- [`search-project-context` Runtime Skill](../../../desktop/src-tauri/src/managed_agents/search_project_context_skill.md)
- [Project Context 图语义检索实现计划](../../stage/semantic/project-context-graph-semantic-query-implementation-plan.md)
- [语义 pgvector 运维](../../semantic-pgvector-operations.md)
- [当前状态与能力边界](../current-status.md)
