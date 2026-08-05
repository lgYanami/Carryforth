# Project Context V0 领域规范

> 状态：首版概念规范
> 本文只定义 Project Context 的最小领域语义、关系、生命周期与查询语义，不定义事件
> kind、wire schema、数据库结构、事务、CLI、用户界面或迁移方案。

## 1. 文档目的

[项目视图定义与项目上下文关系](../project-view/project-view.md)已经把 Project View
定义为项目的一阶状态与稳定坐标，把 Project Context 定义为围绕这些坐标逐渐形成的
二阶语义。[项目文档与资源说明概念设计](../document/document.md)进一步提供了可稳定引用、
修订和按需读取的 Project Document。

在这些基础上，Project Context 首版不再尝试建立一套能够自动理解项目语义的上下文
治理系统，而只回答四个最小问题：

1. 多个项目坐标之间的一段上下文存在于哪里；
2. 哪些坐标共同构成它的准确作用范围；
3. 上下文内容以什么现有实体承载；
4. Human 或 Agent 如何找到并持续修正它。

此前完成的 [Project Context 最小核心语义实现计划](../document/project-context-core-semantics-implementation-plan.md)
只建立了 Agent 对 Project Document、Resource Guide 和按需读取方式的稳定认知，没有
创建 Project Context 领域实体。本文定义的 Edge 才是 Project Context 领域实现的起点。

本文固化当前已经形成共识的 Project Context Edge 模型。

## 2. 核心定义

Project Context 是：

> 连接同一 Project 内两个或多个坐标的一条无向边或超边；这组坐标之间的解释性语义
> 由关联在该 Edge 下的一份或多份 Project Document 承载。

最小模型为：

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
```

其中：

- Edge 只表达“这些坐标共享上下文”这一结构事实；
- Project Document 才是上下文语义的内容实体；
- Edge 自身没有正文、方向、关系类型、维护者或过期时间；
- 所有坐标、Edge 和 Context Document 必须属于同一个 Project；
- 当前 Buzz 接入中，一个 Community 仍等同于一个 Project。

## 3. 坐标

### 3.1 首版坐标类型

首版只允许两类坐标：

1. Project View 对象；
2. Project Document。

Project View 对象坐标复用现有对象类型与稳定 `object_id`。这包括 Project Profile、Goal、
Role、Plan、Stage、Requirement、Issue、Work 和 Resource 等现有 Project View 对象，
不把它们复制成 Context 专用对象。

Project Document 坐标复用稳定 `document_id`，不使用标题、当前 Revision 或某一次 Nostr
event ID 作为身份。

### 3.2 坐标身份

坐标引用必须具有稳定、Project-scoped 的身份。对象内容、状态、标题或当前 Revision
变化，不改变已经进入 Edge 的坐标身份。

同一个坐标在集合中只能出现一次。坐标集合不考虑输入顺序，并在判等和查询前按坐标
类型与稳定 ID 规范化。

### 3.3 坐标与 Context Document 是不同结构角色

Document 可以作为 Edge 的一个坐标，也可以作为 Edge 的上下文内容载体。这是两种独立
的结构角色：

- 出现在 `coordinates` 中，表示该 Document 是上下文所解释的对象之一；
- 出现在 `context_documents` 中，表示该 Document 承载这条 Edge 的解释性内容。

把一份 Document 关联为 Context Document，不会自动把它加入 Edge 的坐标集合；反之
亦然。

## 4. Edge 的身份、唯一性与无向性

### 4.1 精确坐标集合定义一条 Edge

在同一个 Project 内，同一组精确坐标只能存在一条 Project Context Edge：

```text
{A, B} == {B, A}
{A, B} != {A, B, C}
```

因此，Edge 的领域唯一键是：

```text
Project + normalized exact coordinate set
```

协议或存储是否额外分配内部标识属于实现设计；内部标识不得允许同一个 Project 中出现
两条精确坐标集合相同的 Edge，也不改变本规范中的判等与查询语义。

### 4.2 一组坐标只有一条 Edge，但可以有多份 Document

同一组坐标的不同上下文主题不通过重复 Edge 表达，而通过同一 Edge 下的多份 Context
Document 表达。例如 `{Requirement A, Requirement B}` 可以同时关联：

- 一份解释历史原因的 Document；
- 一份记录兼容性注意事项的 Document；
- 一份说明后续影响的 Document。

这些内容仍属于同一条 `{A, B}` Edge。

### 4.3 不同坐标集合必须分开

`{A, B}` 与 `{A, B, C}` 的解释范围不同，必须是两条独立 Edge。超边不会被系统自动
拆成多条二元边：

```text
Edge {A, B, C}
```

不隐含存在 `{A, B}`、`{A, C}` 或 `{B, C}`。

### 4.4 Edge 没有方向

Edge 不区分 source 与 target。A 对 B 的影响、B 对 A 的影响，以及涉及三个或更多坐标
时的整体解释，都记录在同一条 Edge 的 Context Document 中。

系统不因为正文采用某种叙述顺序而推导 Edge 方向。

## 5. Context Document

### 5.1 复用普通 Project Document

Context Document 不是新的文档类型。普通 Project Document 被关联到一条 Context Edge
后，就在该关系中承担 Context Document 的角色，并继续完整复用现有 Document 领域的：

- 稳定 `document_id`；
- Current Revision 与历史 Revision；
- Markdown 正文；
- 作者、变更者和规范时间；
- 并发更新；
- tombstone 与删除保护；
- Community / Project 权限。

Edge 关联稳定 `document_id`，读取时取得该 Document 当前可用的 Revision；历史内容仍
通过 Document 自身的 Revision 机制追溯。首版不为 Edge 再建立一套内容版本。

### 5.2 一份 Document 最多属于一条 Edge

一份 Project Document 可以不属于任何 Context Edge；一旦作为 Context Document 关联，
它最多只能属于一条 Edge。

这里的“属于”只指 Document 出现在 Edge 的 `context_documents` 中。它不限制同一份
Document：

- 成为任意 active Project View 对象的 Context Reference 目标；
- 作为 Document 坐标出现在其他 Context Edge 中；
- 继续作为 Resource Guide 或其他现有 Document Reference 的目标。

这些是彼此独立的结构角色。

如需把同一主题解释给另一组坐标，应根据真实语义：

- 修正原 Edge 的准确坐标范围；或
- 为另一条 Edge 创建独立 Document；

不得通过让同一份 Context Document 同时属于多条 Edge 来模糊其作用范围。

### 5.3 Document 承载解释性语义

Context Document 适合记录：

- 为什么这些坐标会共同出现或被这样安排；
- 它们之间存在的特殊依赖或约束；
- 一个坐标变化可能对其他坐标产生什么影响；
- 适用边界、例外、兼容条件和注意事项；
- 实际工作中发现、验证或修正的跨坐标经验。

它不应复制 Project View 已经直接表达的当前状态、归属、执行关系或责任信息。若一项
内容已经能够成为 Requirement、Issue、Work、Role、Decision 或其他明确领域状态，应
写入对应领域，而不是只留在 Context Document 中。

## 6. 系统与维护者的责任边界

### 6.1 Human / Agent 维护语义

项目上下文的语义由实际承担 Project Role、执行 Work 和理解项目的 Human / Agent 维护。
维护责任跟随真实工作与 Role 承担关系，不在 Edge 上增加 `maintainer`、`owner` 或专用
Assignment 字段。

Human / Agent 在工作中：

1. 发现或创造跨坐标的解释性语义；
2. 查询准确坐标集合是否已有 Edge；
3. 已有合适 Context Document 时更新它；
4. 需要独立内容边界时在同一 Edge 下增加 Document；
5. 没有 Edge 时，以第一份 Context Document 建立它；
6. 实践证明内容错误时直接修正文档。

### 6.2 系统只维护结构事实

系统负责：

- 保存和查询 Edge、坐标集合与 Document 关联；
- 保证同 Project、精确集合唯一性和 Document 单 Edge 归属；
- 复用现有身份、Revision、tombstone、权限与删除保护；
- 记录由 Human / Agent 明确提交的变化。

系统不负责：

- 判断项目还缺少什么上下文；
- 自动判断内容是否过期、冲突、错误、完整或可信；
- 自动产生 Gap；
- 根据对象变化自动改写 Context Document；
- 从聊天、工作状态或文档正文中推断新 Edge；
- 指定哪个 Role 永久拥有某条 Edge。

只有 Human / Agent 通过实际工作发现并明确写回后，这些语义变化才成为项目上下文的一
部分。

## 7. 生命周期

### 7.1 Edge 随 Document 关联存在

Edge 没有独立的空生命周期：

- 向一个尚不存在的精确坐标集合关联第一份 Context Document 时，自动建立 Edge；
- 向已有 Edge 关联 Document 时，只增加该 Edge 的 Document 成员；
- 从 Edge 移除 Document，不删除该 Document；
- 最后一份 Context Document 被移除后，空 Edge 同时消失。

因此，当前领域状态中不存在 `0` 份 Document 的 Edge，也不需要由调用者先创建一条空
Edge 再填充内容。

### 7.2 删除保护

作为 Context Document 关联在 Edge 下的 active Document 不能直接删除。调用者必须先
解除它与 Edge 的关联；如果它还是 Resource Guide 或其他现有活跃引用的目标，也必须
继续满足 Document 领域已有的删除保护。

解除关联只改变 Context 结构，不删除 Document。解除后，它重新成为一份未关联该 Edge
的普通 Project Document，可以继续使用、重新关联或在满足所有删除条件后进入
tombstone。

### 7.3 坐标进入 tombstone 后 Edge 保留

任一坐标进入 tombstone，不自动移除或改写包含它的 Edge，也不自动删除 Context
Document。Edge 继续保留其稳定坐标引用，使成员仍能理解历史对象与现存对象之间的关系。

查询结果应明确呈现坐标当前已经 tombstoned，而不能把 Edge 静默隐藏、改写成较小坐标
集合或级联删除。

### 7.4 不发生隐式级联

- 修改 Context Document 不改变 Edge 坐标集合；
- 修改坐标对象不自动修改 Context Document；
- 解除全部 Document 关联使 Edge 消失时，不删除这些 Document；
- 重叠 Edge 之间不传播 Document 或内容；
- `{A, B, C}` 的变化不自动产生、更新或删除 `{A, B}`；
- 改变上下文作用范围，应显式解除原关联并关联到新的精确坐标集合。

## 8. 查询语义

所有查询都发生在当前 Project 内，以规范化后的坐标集合进行集合运算，并继续经过现有
权限检查。

### 8.1 `exact({A,B})`

取得坐标集合精确等于 `{A,B}` 的唯一 Edge：

```text
exact({A, B})
  → {A, B}
```

它不返回 `{A,B,C}`。输入顺序不影响结果：`exact({B,A})` 与 `exact({A,B})` 等价。

### 8.2 `incident(A)`

取得所有包含坐标 `A` 的 Edge：

```text
incident(A)
  → {A, B}
  → {A, C}
  → {A, B, C}
```

概念上，`incident(A)` 等价于 `contains-all({A})`，但作为最常用的单坐标邻接查询保留
独立名称。

### 8.3 `contains-all({A,B})`

取得坐标集合包含查询集合全部坐标的 Edge：

```text
contains-all({A, B})
  → {A, B}
  → {A, B, C}
  → {A, B, D, E}
```

它不返回只包含部分查询坐标的 `{A,C}` 或 `{B,C}`。

形式化表示：

```text
exact(Q)        = { E | E.coordinates = Q }
incident(A)     = { E | A ∈ E.coordinates }
contains-all(Q) = { E | Q ⊆ E.coordinates }
```

### 8.4 查询只发现坐标与轻量 Document 信息

Context 查询不默认返回所有 Document 正文。结果应首先提供 Edge 坐标、坐标生命周期和
关联 Document 的轻量元数据；Human / Agent 再按需读取需要的正文。

坐标 tombstone 不使 Edge 自动退出上述查询结果。查询类型只表达集合匹配，不表达方向、
依赖、重要性或语义相关度排序。

## 9. Agent 发现与交付

Project Context 不在每一 turn 自动注入 Agent Context，也不要求把所有 Edge 或 Document
正文放入 Role Brief。

稳定的 system contract 只需要告诉 Agent：

- Project Context 以跨坐标 Edge 存在；
- 可以通过项目提供的查询能力发现相关 Edge；
- Context Document 正文按需读取；
- 工作实质发现、创造或纠正跨坐标语义时，应显式写回。

Agent 根据当前 Role、Work 和已经取得的 Project View 坐标，自主选择 `exact`、
`incident` 或 `contains-all` 查询。系统提供可发现性和读取能力，但不替 Agent 决定哪条
上下文在当前推理中必然相关。

## 10. 权限

Project Context 首版不建立独立 ACL，也不因为 Role、Edge 或 Document 关联自动授予
权限。

- Project 与 Community 边界复用现有模型；
- Project View 坐标的可见性复用 Project View 权限；
- Context Document 的读取、创建、更新与删除复用 Project Document 权限；
- Edge 的关联与解除操作组合使用上述现有授权边界；
- Context 查询不得借由 Edge 暴露调用者原本无权取得的坐标或 Document 内容；
- 读取 Context Document 不提升 Agent Runtime、Sandbox 或外部系统权限。

Edge 不保存维护者 ACL、Role 专属 ACL 或文档覆盖权限。

## 11. 坐标类型扩展

`CoordinateRef` 应采用可扩展的带类型引用，而不是把首版两种坐标硬编码成不可演进的
二选一表结构。

未来增加一种坐标类型时，只需为该类型定义：

1. Project-scoped 的稳定身份；
2. 规范化与判等规则；
3. 当前状态与 tombstone 的解析方式；
4. 现有权限检查；
5. 面向 Human / Agent 的轻量显示信息。

Edge 的无向集合、精确集合唯一性、Document 承载内容、生命周期以及 `exact`、
`incident`、`contains-all` 查询语义不需要随坐标类型改变。

首版不支持任意外部 URL、聊天消息、Runtime、文件路径或 Edge 自身作为坐标。外部资产
应先通过现有 Resource 或 Document 取得项目内稳定身份；其他类型等真实需求出现后再
扩展。

## 12. 示例

### 12.1 两个 Requirement 的上下文

```text
Edge {Requirement A, Requirement B}
├── Document：为什么两项需求必须共同交付
└── Document：它们的兼容性与回滚注意事项
```

两份 Document 属于同一条 Edge，而不是建立两条 `{A,B}` Edge。

### 12.2 两个 Requirement 与一个 Resource 的上下文

```text
Edge {Requirement A, Requirement B, Resource R}
└── Document：两项需求共同依赖该资源时的使用边界
```

它与 `{Requirement A, Requirement B}` 是不同 Edge：

```text
exact({A, B})          只取得 {A, B}
exact({A, B, R})       只取得 {A, B, R}
contains-all({A, B})   同时取得二者
incident(R)            取得所有包含 R 的 Edge
```

### 12.3 tombstone 坐标

Requirement A 进入 tombstone 后：

```text
Edge {Requirement A [tombstoned], Resource R}
└── Context Document 保留
```

系统不把它改写为单坐标 `{Resource R}`，因为这会改变原上下文的准确范围与历史含义。

## 13. 首版范围与非目标

### 13.1 首版范围

首版包括：

- Project View 对象与 Project Document 两类坐标；
- 两个或多个坐标构成的无向 Edge / Hyperedge；
- 同 Project、精确坐标集合唯一；
- 一条 Edge 下关联一份或多份 Project Document；
- 一份 Context Document 最多属于一条 Edge；
- 第一份 Document 建 Edge、最后一份移除时 Edge 消失；
- Context Document 删除保护；
- 坐标 tombstone 后 Edge 保留；
- `exact`、`incident` 和 `contains-all` 查询；
- 正文按需读取和 Human / Agent 显式维护；
- 完全复用现有权限与 Document Revision。

### 13.2 首版非目标

首版不定义或不承诺：

- 有向 Edge、关系类型或自定义关系 schema；
- 同一精确坐标集合下的多条 Edge；
- 一份 Context Document 同时属于多条 Edge；
- 单坐标 Context Edge；
- Edge 正文、独立语义 Revision、Edge maintainer 或 Edge ACL；
- 自动 Gap、完整性、新鲜度、冲突、可信度或重要性判断；
- 自动摘要、语义搜索、向量检索或 Context Compiler；
- 每 turn 自动注入 Edge 或 Document 正文；
- 对象状态变化触发 Context 自动更新；
- Context Edge 与现有 Context Reference 之间的自动转换、投影或同步；
- 跨 Project Edge；
- event kind、wire、存储、索引、事务、CLI 或 UI 设计。

## 14. 领域不变量与验证清单

后续实现至少必须保持：

1. 一条 Edge 必须且只能属于一个 Project。
2. 一条 Edge 至少包含两个不同坐标。
3. 一条 Edge 至少关联一份 active Context Document。
4. Edge 的坐标是无序、去重的集合。
5. 同一 Project 内，规范化后的同一精确坐标集合只能有一条 Edge。
6. `{A,B}` 与 `{A,B,C}` 是不同 Edge，且超边不隐式生成二元边。
7. Edge 没有方向，所有方向上的解释保存在同一组 Context Document 中。
8. 所有坐标和 Context Document 必须与 Edge 属于同一 Project。
9. Context Document 使用普通 Project Document，不复制正文或 Revision 模型。
10. 一份 Project Document 最多作为 Context Document 属于一条 Edge；这不限制它承担
    Context Reference 目标、Resource Guide 或 Edge 坐标等其他角色。
11. 第一份 Context Document 与 Edge 原子建立，当前状态中不存在空 Edge。
12. 移除最后一份 Context Document 时 Edge 消失，但 Document 保留。
13. 仍关联 Edge 的 Context Document 不能进入 tombstone。
14. 坐标进入 tombstone 不删除、缩小或改写 Edge。
15. 修改坐标、Document 或重叠 Edge 不产生隐式级联。
16. `exact` 只匹配相等集合，`incident` 匹配包含单坐标的集合，`contains-all` 匹配超集。
17. Context 查询和关联不授予任何新权限。
18. Context 正文不默认进入每一 turn 的 Agent Context。
19. 系统不从内容或状态自动推断 Gap、过期、冲突或新 Edge。
20. Project Context 只承载解释性二阶语义，不替代明确领域状态。

## 15. 与现有 Context Reference 的关系

现有 Project View Context Reference 与本文 Project Context Edge 是两种结构和语义均
不同的关系：

| | Context Reference | Context Edge |
|---|---|---|
| 结构 | Project View 对象拥有的单向引用 | 精确、无序的坐标集合形成的 Edge / Hyperedge |
| 范围 | 一个来源对象指向一个 Resource 或 Document | 两个或多个 Project View / Document 坐标 |
| 语义 | 目标资产或内容与来源对象相关 | Context Document 解释整组坐标之间的二阶语义 |
| 内容 | 引用本身不承载正文 | 一份或多份普通 Project Document 承载正文 |

二者可以独立共存。同一份 Document 可以同时作为 Context Reference 的目标和一条 Edge
的 Context Document，这表示它在两个不同结构角色中被使用，不构成冲突，也不违反
Context Document 的单 Edge 归属约束。

首版不在二者之间建立过渡或联动规则：

- 不把 Context Reference 自动转换成 Context Edge；
- 不为 Context Edge 的每个坐标自动创建指向 Context Document 的 Context Reference；
- 不要求二者同步、互相继承、互相替代或共同去重；
- 不修改现有 Context Reference 的 schema、生命周期和行为。

Human / Agent 继续通过 Context Reference 发现与单个 Project View 对象相关的 Resource
或 Document，通过 Context Edge 查询多个坐标之间的解释性上下文。

## 16. 当前结论

Project Context V0 可以收敛为一个很小的领域内核：

```text
稳定项目坐标的精确无向集合
        +
一份或多份普通 Project Document
        =
一条可查询、可持续修正的 Project Context Edge
```

系统保存结构和变化，Human / Agent 在真实工作中维护语义。项目不需要把全部上下文持续
塞入 Agent 的每一 turn，也不需要让系统假装理解何时缺失、过期或冲突；Agent 只在需要
时沿稳定坐标发现、读取并写回上下文。
