# Project Context 图语义化基础规范

> 状态：概念规范，设计已确认，待单独编写实现设计
>
> 日期：2026-08-10
>
> 范围：来源语义、Coordinate 与 Edge 的语义边界、Semantic Unit、PostgreSQL / pgvector
> 派生索引、版本与生命周期、权限和可重建性
>
> 明确排除：语义查询输入、可选初始坐标、召回与排序公式、FTS / Vector 融合、图路径搜索、
> Agent 使用方式、CLI / Desktop、具体 embedding 模型与供应商、SQL migration 和 worker 实现
>
> 关联文档：
> [Project Context V2 领域规范](../project-context/project-context.md)、
> [Project View 来源对象摘要实现设计](../project-context/summary/project-view-summary-implementation-design.md)、
> [Meeting 来源摘要实现设计](../project-context/summary/meeting-summary-implementation-design.md)、
> [Project Document 领域设计](../document/document.md)

## 1. 文档目的

Project Context 已经提供一张由稳定 Coordinate、精确无向 Hyperedge 和 Context Document binding
组成的共享项目上下文图。Project View 对象、Project Document 和 Meeting 也已经各自在自己的来源领域中
保存并提供可选 `summary`。

这些能力解决了两个问题：

1. 项目内容由什么稳定对象承载；
2. Human 或 Agent 如何先阅读标题与摘要，再决定是否加载完整内容。

但纯文本不会自动产生不依赖 Agent 的语义检索能力。PostgreSQL FTS 可以匹配词项，却不能单独理解不同
表述之间的语义相近。要让系统通过自然语言找到可能相关的 Coordinate 或关系内容，需要由 semantic
encoder 把来源文本转换成 embedding，并由 PostgreSQL / pgvector 保存和检索这些向量。

本文只定义这层“图语义基础”：

> 哪些 canonical 内容提供语义，如何把它们转化为可重建的机器检索表示，以及这种表示如何与现有
> Project Context 图保持一致而不成为第二份项目事实。

本文不定义用户或 Agent 如何提出查询，也不定义如何从命中结果构造上下文路径。这些属于后续独立的
语义检索规范。

[Project Context V2 领域规范](../project-context/project-context.md)在首版中明确排除了 semantic search、
vector index 和自动上下文编译器。本文是在来源摘要与图结构稳定后的下一阶段扩展，只增加可重建的派生
检索基础，不修改 Coordinate、Edge、Context Document 或权限的既有领域不变量。

此前的
[Project Context Agent 渐进检索设计规范](../meeting/context/project-context-progressive-retrieval-spec.md)
当前状态是“待重写、部分核心结论已经失效、不得作为实现依据”；其中的
`CoordinateNode.summary` 不会被本文恢复。与它配套的
[渐进检索实现设计](../meeting/context/project-context-progressive-retrieval-implementation-design.md)
已经整体废弃，本文也不会恢复其中的 Node Head、Node Meta、Node Catalog 或独立 Node revision。

## 2. 核心结论

总体结构固定为：

```text
Canonical Sources
├── Project View objects
├── Project Documents
└── Meetings
        │
        │ source-owned title / summary / content
        ▼
Semantic Extraction
├── overview
└── content chunks（未来可选）
        │
        │ semantic encoder
        ▼
PostgreSQL / pgvector derived index
├── source identity and typed source basis
├── semantic unit identity and digest
├── embedding model identity
└── embedding

Canonical Project Context
├── Coordinate identity ────── resolves to Canonical Source
├── exact unordered Hyperedge
└── Context Document binding ─ resolves to Project Document semantic units
```

必须同时成立：

1. 语义内容属于 Canonical Source；
2. embedding 属于可重建的数据库检索索引；
3. Coordinate 不保存或拥有 embedding；
4. Edge 不保存 summary、embedding、方向、关系类型或静态语义权重；
5. Edge 的关系语义由它绑定的每一份 Context Document 分别提供；
6. chunk 只是来源对象内部的检索单位，不成为 Coordinate、Edge 或新的图节点；
7. pgvector 与现有 canonical 数据可以位于同一个 PostgreSQL 中，但两者的事实地位不同。

## 3. 术语与责任边界

### 3.1 Canonical Source

Canonical Source 是 Coordinate 所指向、拥有当前项目内容的来源领域实体：

- Project View object；
- Project Document；
- Meeting。

来源对象拥有：

- 稳定身份；
- 来源原生的 title / name；
- 可选 `summary`；
- 完整 canonical content；
- 生命周期；
- 来源领域自己的 typed currentness observation、签名投影与权限。

`summary` 是由 Human 或 Agent 维护的项目内容。它回答“这个对象包含什么、处理什么问题时值得加载”，
但仍只是非权威的检索提示，不能替代完整内容或作为授权依据。

### 3.2 Coordinate

Coordinate 是对 Canonical Source 的 Project-scoped 稳定引用。它只承担身份与图连接作用，不复制来源的
title、summary、正文或 embedding。

查询或展示层可以通过 Coordinate 解析来源对象并水合当前 title / summary。这种观察值不是 Coordinate
拥有的第二份内容。

Coordinate identity、规范化、判等和 EdgeKey 均不包含：

- title；
- summary；
- content；
- embedding；
- embedding model；
- source revision。

### 3.3 Project Context Edge

Edge 继续完全遵循 [Project Context V2 领域规范](../project-context/project-context.md)：

```text
ProjectContextEdge
├── project
├── exact normalized Coordinate set
└── one or more Context Document bindings
```

Edge 只表达“这组精确坐标共享上下文”这一结构事实。它没有独立正文，也不生成一份聚合语义。

### 3.4 Semantic Unit

Semantic Unit 是 semantic extractor 从一个 Canonical Source 的某一当前版本中确定性提取出的机器检索
单位。

它是内部索引单位，不是：

- Project View object；
- Project Document；
- Meeting；
- Project Context Node；
- Coordinate；
- Edge；
- Nostr 项目事实事件。

删除所有 Semantic Unit 和 embedding 后，系统必须能够从当前 Canonical Sources 重新生成它们。

### 3.5 Embedding

Embedding 是指定模型对一个 Semantic Unit 文本的派生数值表示：

```text
embedding
  = encode(
      semantic text,
      extractor version,
      embedding model and version
    )
```

embedding 不是 Human 或 Agent 编写的项目内容，也不是来源对象的一部分。它不进入来源对象 command、
signed projection、CLI 普通对象 DTO 或 Desktop 对象模型。

## 4. Coordinate 的语义来源

### 4.1 来源对象直接拥有语义内容

Coordinate 的语义内容直接来自它所指向的 Canonical Source，而不是来自 Project Context 自建的 Node
summary：

| Coordinate family | Canonical semantic source |
|---|---|
| Project View object | 对象自己的类型、title / name、`summary` |
| Project Document | Document 自己的 title、`summary` |
| Meeting | Meeting 自己的 title、`summary` |

Project Context 不复制这些字段，也不负责其生成、修正或生命周期。

每类来源通过自己的 verified current read model 提供等价观察：

```text
CanonicalSemanticSourceObservation
├── community / project
├── source family / subtype / stable id
├── current lifecycle
├── typed source basis
├── source-native title / name
├── optional source-owned summary
├── content read capability
└── source snapshot digest
```

extractor 不得把 CLI 展示文本、Project Context preview fallback、未验证 JSON 或旧客户端缓存当作 canonical
来源。`typed source basis` 由各来源领域定义；本规范不强迫 Project View、Document 和 Meeting 共享一种整数
revision，也不为 Meeting 反向创造新的 summary revision。

### 4.2 Overview Semantic Unit

每个 active、可坐标化的来源对象逻辑上可以产生一个 `overview` unit：

```text
OverviewSemanticUnit
├── source identity
├── source type / subtype
├── source basis
├── semantic title
├── optional source-owned summary
├── extractor version
└── semantic text digest
```

首版 semantic text 的输入范围固定为：

```text
source type label
+ source-native title / name
+ source-owned summary, when present
```

不自动加入：

- description、purpose、Board 或其他业务正文作为 summary fallback；
- 相邻 Coordinate 的 title / summary；
- Edge 上的 Context Document；
- Role、当前 Work、当前查询或调用者环境；
- revision、作者、更新时间、Assignment 等易变 metadata；
- 系统根据图结构猜测出的关系描述。

这些边界保证同一个来源对象只有一份与调用者无关的基础语义表示。不同问题或不同 Agent 环境如何使用它，
属于查询时行为。

### 4.3 Markdown 与规范文本

来源 `summary` 可以是 Markdown。semantic extractor 应从 canonical Markdown 中提取可见文本用于编码，
但不能把解析后的文本写回来源对象或冒充新的 canonical summary。

规范提取至少满足：

- 保留标题、段落、列表项和代码标识的可见文本；
- 不执行链接、HTML、工具命令或文本中的任何指令；
- 不因渲染主题、Desktop 样式或视口宽度改变结果；
- 相同 canonical 输入与 extractor version 产生相同 semantic text digest；
- extractor 行为发生实质变化时必须提升 extractor version 并重建索引。

具体 Markdown AST、空白归一化和代码块处理规则由实现设计固定。

### 4.4 缺失 summary

`summary = None` 是合法状态。此时 overview 只基于来源类型与 title / name 生成，并显式记录
`summary_missing = true` 或等价覆盖状态。

缺失 summary 不表示：

- 来源内容为空；
- 来源与任何问题无关；
- 索引损坏；
- 系统可以自动用 description 或正文替代 summary。

语义检索必须能够在结果覆盖信息中区分“有完整 overview 输入”和“只有 title 的降级输入”。具体如何调整
排序属于后续检索规范。

### 4.5 Graph membership 与来源语义分离

semantic index 可以覆盖所有当前可成为 Coordinate、仍有可读 canonical content 的来源对象，而不要求该
对象已经出现在某条 Project Context Edge 中。这里的“当前”表示它仍是来源领域的 current object，不表示
其业务 status 必须处于进行中。

是否属于当前图必须通过当前 active Edge 的 Coordinate 并集判断：

```text
graph membership
  = exists active Project Context Edge containing Coordinate
```

`graph_membership` 不成为语义索引中的第二份权威事实。实现可以缓存它，但查询前必须以当前 canonical
Project Context 状态验证。

因此，图外来源对象可以被后续语义查询发现并作为独立内容候选，但不能被描述成当前图节点，也不能从它
声称存在 incident Edge。

### 4.6 业务终态不是 tombstone

Project View 的业务终态与删除 tombstone 是两套独立语义：

```text
Requirement: satisfied / withdrawn
Issue:       resolved / closed
Work:        completed / cancelled
Plan/Stage:  completed / cancelled
```

这些 status 不删除对象。对象仍然保留完整 title、summary、正文、关系和 current source basis，因此继续
产生 current Semantic Unit 和 embedding。

semantic source observation 必须保留 source-native lifecycle / status，供后续查询决定是否：

- 同时检索进行中与已结束对象；
- 只检索进行中对象；
- 只检索 completed / closed 等历史上下文。

lifecycle / status 是结构化过滤 metadata，不自动拼进 overview semantic text，也不改变来源内容所有权。
从进行中变为业务终态时，如果 title / summary 没有变化，可以按第 8.5 节复用原 embedding并只更新当前
source basis和lifecycle observation。

只有显式 Delete 才产生真正的 Project View tombstone。当前 tombstone 是 bodyless 删除标记，不等同于
“项目事项已经完成”。

## 5. Edge 的关系语义

### 5.1 Edge 不生成单一 embedding

一条 Edge 可以绑定多份 Context Document，每份 Document 可能分别解释：

- 历史原因；
- 技术约束；
- 兼容边界；
- 决策结果；
- 后续影响。

系统不得把这些 Document 的 title、summary、正文或 embedding：

- 拼接成一份 canonical Edge summary；
- 平均成一个 Edge embedding；
- 累加成一个静态 Edge relevance；
- 用来推导 Edge 方向或关系类型。

否则会抹平同一 Edge 上多份解释材料的独立语义，也会让绑定 Document 更多的 Edge 获得虚假的静态优势。

### 5.2 每份 Context Document 独立提供关系语义

Edge 的机器可检索关系语义是一个集合：

```text
EdgeSemanticSurface(E)
  = {
      semantic units of D
      for each active Context Document D bound to E
    }
```

其中每个 Document 的 overview 和未来 content chunks 都继续属于该 Project Document 的派生索引，
不复制为 Edge-owned unit。

按当前 Project Context V2 语义，一份 Project Document 作为 Context Document 时最多属于一条 active
Edge。它仍可同时作为 Document Coordinate 出现在其他 Edge 中，这两个结构角色互不影响。

### 5.3 Document 的多重结构角色复用同一语义索引

同一 Project Document 可能同时承担：

1. 普通 Project Document；
2. Document Coordinate；
3. 某条 Edge 的 Context Document。

这些角色引用同一组来源语义单元：

```text
Project Document D
└── Semantic Units(D)
    ├── overview
    └── content chunks（未来）

Document Coordinate(D) ───────────────┐
Context Document binding(E, D) ───────┴── resolves to Semantic Units(D)
```

不得为三个角色分别复制 summary 或生成三组内容相同的 embedding。

### 5.4 Binding 只提供结构映射

语义检索命中一份 Context Document 后，可以通过当前 active binding 映射到真实 Edge。这个运行时映射
可以概念性表示为：

```text
(edge_key, context_document_id, matched_semantic_unit)
```

它不是新的持久领域对象，也不改变以下事实：

- Edge identity 仍由精确 Coordinate 集合定义；
- Context Document identity 仍是稳定 `document_id`；
- `{A, B, C}` 不隐含 `{A, B}`、`{A, C}` 或 `{B, C}`；
- 查询时从某一 Coordinate 进入 Edge 不会使 Edge 获得领域方向；
- Document 的语义相似度不证明 Edge 或 Coordinate 之间存在因果关系。

## 6. Semantic Unit 模型

### 6.1 最小逻辑身份

一个 Semantic Unit 的逻辑身份至少由以下部分确定：

```text
SemanticUnitIdentity
├── community / project scope
├── source family
├── source subtype, when applicable
├── stable source id
├── source semantic basis
├── source snapshot digest
├── unit kind
├── unit key
└── extractor version
```

来源 semantic basis 必须能够证明 unit 来自哪个 canonical source observation。不同来源领域可以使用不同
revision / event 组合，不要求伪造一个跨 Project View、Document、Meeting 的全局 source revision。

`source snapshot digest` 是对本次 extractor 实际读取的 canonical 来源字段做 domain-separated hash；
`semantic text digest` 则是对某一个 unit 实际送入 encoder 的规范文本做 hash。前者证明完整提取集合的来源，
后者支持 unit 级复用。两者都不能替代稳定 source identity 或来源领域自己的 currentness token。

### 6.2 Unit kind

本规范预留两类 unit：

```text
SemanticUnitKind
├── overview
└── content_chunk
```

`overview` 是本阶段的基础能力。`content_chunk` 是后续能力，但其身份与生命周期必须从一开始兼容，避免
未来把 chunk 塞进 Coordinate 或另建一套无法统一检索的索引。

### 6.3 Overview unit

每个来源 observation、extractor version 只有一个 overview unit：

```text
unit_kind = overview
unit_key  = overview
```

overview 使用第 4.2 节规定的来源类型、title / name 和可选 summary。

### 6.4 Content chunk unit

content chunk 是来源完整内容中的一个派生片段：

```text
ContentChunkSemanticUnit
├── source identity and basis
├── unit_kind = content_chunk
├── deterministic chunk key
├── ordinal
├── optional structural path
├── extractor version
└── semantic text digest
```

它必须满足：

- 可回到原始来源对象；
- 可说明来源内的位置或结构路径；
- 不获得独立 Project identity；
- 不进入 Coordinate set；
- 不成为 Edge 成员；
- 不拥有 Project Context binding；
- 不自动生成新的 canonical summary；
- extractor 版本不变时，对同一 canonical 输入确定性切片。

Document 适合按 Markdown heading、段落和代码块等结构边界切片；Project View 对象更适合按其结构化字段或
语义段落切片。具体策略属于后续实现设计，不由本规范固定为统一 token 窗口。

### 6.5 Source-level 聚合边界

一个长 Document 可能产生大量 chunk，一个短 Work 可能只有 overview。Semantic Unit 数量不是来源对象的
重要性，也不能自然变成来源或 Edge 的相关度。

后续检索必须先保留 unit 命中，再按 source identity 聚合，防止“chunk 更多”自动获得更高排名。具体
聚合公式属于后续检索规范。

## 7. PostgreSQL / pgvector 派生索引

### 7.1 使用同一个 PostgreSQL

Buzz 可以在保存 canonical graph 与来源投影的同一个 PostgreSQL 中启用 pgvector。本文不要求独立向量
数据库或新的图数据库。

逻辑索引记录至少包含：

```text
SemanticEmbeddingIndexRecord
├── SemanticUnitIdentity
├── semantic text digest
├── summary coverage
├── embedding model id / version
├── vector dimensions
├── distance metric / normalization contract
├── embedding
├── indexed_at
└── index lifecycle state
```

具体表名、列类型、HNSW / IVFFlat、分区和索引参数属于实现设计。

### 7.2 统一索引而不是分散进各来源结构

embedding 不加入：

- Project View object body；
- Project Document revision / head；
- Meeting metadata / State / End；
- `ProjectContextCoordinate`；
- Edge / Binding / Meta projection。

采用统一索引的原因是：

1. 一个来源可以有一个 overview 和任意数量的 chunk；
2. 一个来源可以同时存在多个 embedding model 版本；
3. Project View、Document、Meeting 可以在一次全局语义候选查询中使用同一向量索引；
4. 模型或 extractor 升级不需要修改 canonical object；
5. 索引失败、重试或重建不会阻断来源写入；
6. embedding 不需要通过 Nostr 发送给 Agent、Desktop 或普通项目读者。

统一索引表仍然只是 PostgreSQL 内部派生状态，不是新的项目领域层。

### 7.3 多模型与向量维度

索引身份必须包含 embedding model。系统不得假定所有历史和未来模型拥有相同维度、归一化方式或距离
度量。

模型合同至少包含：

```text
EmbeddingModelContract
├── model id
├── model version
├── vector dimensions
├── distance metric
├── normalization rule
└── encoder input contract version
```

同一个 Semantic Unit 可以在模型切换期间并行拥有多条 embedding。只有查询层选定的 active model / index
generation 参与对应查询；不同模型的原始距离分数不得直接比较。

模型未知、维度不匹配、距离合同不匹配或包含 NaN / Infinity 的向量不得进入 current index generation。

### 7.4 输入覆盖与模型上限

来源 summary 没有为 embedding 设置新的领域长度上限。semantic index 不得为了适配某个模型，反向收紧
Project View、Document 或 Meeting 的 canonical summary validator。

如果 overview 或未来 chunk 超过 active model 的输入上限：

- 不得静默截断后仍把结果标记为完整 current unit；
- 必须记录完整 canonical semantic text digest 与实际编码覆盖；
- 可以由 versioned extractor 定义确定性的分段、池化或其他完整覆盖策略；
- 如果当前实现无法完整编码，应进入明确的 missing / failed / unsupported coverage，而不是伪造完整向量；
- 更换覆盖策略必须提升 extractor 或 encoder input contract version 并重建索引。

具体 token 计数、分段与池化算法属于实现设计。

### 7.5 FTS 的位置

PostgreSQL FTS 可以从相同 Semantic Unit 文本建立词项索引，用于保留对象名、项目术语和代码标识等精确
命中。FTS 与 vector 的候选融合属于后续语义检索规范。

FTS 不替代 semantic encoder；纯 `tsvector` 匹配不能单独满足本文所说的语义近似检索。

## 8. 索引生命周期

### 8.1 来源写入不等待 embedding

Canonical Source 的 create / update / delete 必须先按来源领域自己的事务语义完成。它不得因为：

- encoder 不可用；
- 模型超时；
- pgvector 索引暂不可写；
- chunk extractor 失败；
- semantic worker backlog；

而被回滚或报告为来源写入失败。

embedding 通过来源提交后的异步索引流程生成。具体 outbox、job、worker 和重试机制属于实现设计。

### 8.2 当前来源变化立即使旧索引失去 current 资格

异步生成不能允许已被修改、清除或删除的旧文本继续冒充当前语义。

来源变化后，旧 embedding 必须通过下列任一等价机制立即失去 current eligibility：

- 来源事务同步写入 invalidation / current-basis fence；
- 查询时强制把索引的 source basis 与当前 canonical source basis 比较；
- 其他能够证明旧行不会作为 current hit 返回的 fail-closed 机制。

新的 embedding 尚未准备好时，该来源的状态是 `missing`、`building` 或 `stale/ineligible`，而不是继续把旧
向量静默当作当前值。

### 8.3 生成与提交

标准索引流程为：

```text
1. 观察一个当前 Canonical Source basis
2. 读取允许进入语义输入的 canonical 字段
3. 通过 extractor 生成完整 Semantic Unit set
4. 计算每个 unit 的 semantic text digest
5. 生成或复用指定模型的 embedding
6. 再次验证 source basis 仍是当前值
7. 原子激活这一 source / model / extractor 对应的完整 unit set
```

第 6 步失败时，旧任务必须丢弃或重新排队，不能覆盖更新后的来源索引。

### 8.4 同一来源版本的原子激活

一个来源版本包含 overview 以及未来可能存在的多个 chunks。系统不得向查询暴露：

- 新 overview + 旧 chunks；
- 旧 overview + 新 chunks；
- 同一 extractor 输出的一半 chunks；
- 同一 active model 下混合两个 source basis 的 unit set。

索引可以分批计算，但只有完整集合通过 source-basis CAS 后才能作为 current set 激活。

### 8.5 内容未变时允许复用向量

来源 revision 可能因为 status、priority 或其他未进入 semantic text 的字段变化而推进。若同时满足：

- semantic text digest 完全相同；
- extractor version 相同；
- embedding model contract 相同；

系统可以复用已有向量，并把 current source basis 更新到新 observation；不要求调用 encoder 生成相同向量。

复用不能只依赖标题相同或近似文本，必须依赖规范 semantic text digest。

### 8.6 业务终态、删除 tombstone 与失权

completed、closed、resolved、satisfied、cancelled、withdrawn、ended 等 source-native 业务终态不等于
删除。只要来源仍提供当前可读的 canonical object：

- 对应 unit 继续保持查询资格；
- 当前 embedding继续存在或按 semantic text digest复用；
- lifecycle / status作为结构化候选 metadata保存；
- 是否包含这些对象由后续查询条件决定，基础层不预先排除。

显式 Delete 产生的 bodyless tombstone或来源失去当前读权限后：

- 对应 unit 必须立即失去查询资格；
- 旧 embedding不得继续影响返回候选或泄露其存在；
- Project Context Edge可以按自己的领域规则继续保留Coordinate identity；
- bodyless tombstone不自动保留最后的title、summary或embedding作为当前内容；
- 后台物理清理可以异步完成，但读取资格必须fail closed。

Project Context仍展示bodyless tombstone的Coordinate identity，不表示semantic index仍拥有该对象的历史
内容。如果未来要求按删除前内容检索真正的tombstone，必须由来源领域提供可重建、权限明确的历史
snapshot，并单独设计历史语义检索；不能让旧embedding成为最后内容的唯一副本或冒充current source。

### 8.7 Edge attach / detach

Edge attach / detach 只改变 Project Context 结构：

- Coordinate 的图成员资格；
- Context Document binding；
- Edge / Context revision。

它不改变来源文本，因此不重新生成 Project View、Document 或 Meeting embedding。

attach 后通过新的 canonical 结构映射复用已存在的来源语义单元；detach 后删除该结构映射，但不删除作为
普通来源对象仍然有效的 Document embedding。

### 8.8 模型升级

embedding model 升级遵循：

```text
build new model generation
→ validate coverage
→ switch active generation
→ retire old generation
```

该过程：

- 不修改来源对象；
- 不推进 Project View / Document / Meeting revision；
- 不推进 Project Context revision；
- 不改变 Coordinate 或 Edge identity；
- 允许在 cutover 前同时保存新旧模型索引。

## 9. 一致性与 Provenance

### 9.1 不伪造跨领域全局 Revision

Project View、Document、Meeting 和 Project Context 各自拥有不同的 canonical revision / event 观察。
semantic index 不创建一个冒充所有来源事实的全局业务 revision。

每条 current embedding 必须至少能追溯：

```text
SemanticProvenance
├── project / community
├── source family / subtype / stable id
├── typed source basis
├── semantic text digest
├── extractor version
├── embedding model contract
├── index generation
└── indexed_at
```

Context Document 被作为关系语义使用时，还必须在查询时验证当前 binding 与 Project Context observation；
这项 binding provenance 不写入 Document embedding 本身。

### 9.2 Index generation 不是业务 Revision

index generation 只表示一组派生索引使用相同的 extractor / model / activation policy。它不能：

- 成为 Project View revision；
- 成为 Document catalog revision；
- 成为 Meeting state revision；
- 成为 Project Context revision；
- 被 Agent 当作项目事实版本引用。

### 9.3 原始来源始终可验证

embedding 命中只表示模型认为某个 Semantic Unit 与查询近似。它不证明：

- summary 正确；
- 来源当前事实正确；
- Edge 表达因果或方向；
- Context Document 覆盖了整条 Edge 的全部含义；
- 相邻 Coordinate 必然与问题相关。

任何需要作为事实使用的内容仍应回到当前 canonical source 和当前 binding 验证。具体读取流程属于后续检索
规范。

## 10. 权限、安全与隔离

### 10.1 embedding 继承来源敏感度

embedding 是受保护来源文本的派生表示。即使它不是可直接阅读的正文，也不能视为公开、无敏感度的数字。

必须满足：

- 索引按 Community / Project 严格隔离；
- 未授权调用者不能通过向量查询获知候选、数量、距离或存在性；
- 来源当前不可读时，其 embedding 不参与该调用者的召回；
- Project Context binding 不授予新的来源读取权限；
- Role、Work 或 Agent 身份不扩张 ACL；
- raw embedding 不通过普通 Nostr 查询、CLI 对象读取或 Desktop DTO 暴露。

### 10.2 权限检查不能只在结果展示时执行

先进行全局 ANN 检索、再隐藏未授权结果，可能通过数量、排序、延迟或候选不足泄漏受限内容，并会让
未授权向量挤占合法候选。

后续查询实现必须在召回范围形成前应用 Project / Community 和来源权限边界，并在返回前再次验证当前
lifecycle 与权限。具体索引分区、过滤和授权查询方式属于实现设计。

### 10.3 Semantic text 是数据，不是指令

title、summary、Document 正文和 chunk 中的内容均是不可信项目数据。extractor、encoder 和索引 worker：

- 不执行其中的 shell、CLI、链接或工具指令；
- 不因文本声称拥有权限而改变 ACL；
- 不允许文本修改 model、project、source identity 或 index generation；
- 只执行固定的数据提取与编码合同。

若未来使用 LLM 或 cross-encoder 进行 rerank，必须另行定义只读权限和 prompt-injection 边界。

### 10.4 外部 encoder

如果 embedding 由外部服务生成，向该服务发送 Project 内容必须位于明确授权的信任边界内，并遵守项目的
数据保留、日志和删除政策。是否使用本地模型、受控服务或其他 encoder 属于实现设计，但不得因方便而
绕过来源可见性与 Community 隔离。

## 11. 索引覆盖状态

图语义基础必须把索引不完整作为正常、可观察状态，而不是伪装成“没有相关内容”。至少区分：

```text
SemanticIndexState
├── current       当前 source basis 的完整 unit set 已激活
├── missing       当前来源没有可用 embedding
├── building      正在为当前来源生成完整 unit set
├── stale         只有旧 source basis 的索引，不能冒充 current
├── failed        当前生成失败，可重试
└── ineligible    来源已显式删除、失权或不满足当前索引资格
```

业务终态对象只要仍是当前可读canonical source，就属于`current / missing / building / stale / failed`之一，
而不是因为status结束自动进入`ineligible`。

stale 索引可以为诊断、清理或重新生成暂时保留，但不得参与“当前项目内容”的候选召回。历史 Revision 的
语义检索若未来需要，必须另行建立显式历史查询合同，不能把 stale current index 偷换成历史检索。

索引覆盖至少能够统计：

- 可索引来源总数；
- current / missing / building / stale / failed 数量；
- summary missing 的 overview 数量；
- 各 active model / extractor generation 的覆盖；
- 未来 chunk extraction 的完整性。

这些指标是派生索引健康度，不是项目完成度或内容质量评分。

## 12. 非目标

本文不定义或引入：

- `SemanticGraphQuery`；
- `initial_coordinates[]` 的来源或语义；
- 自然语言问题如何编码；
- problem、Role、Work、Issue、Requirement 或 Runtime Context 如何组合；
- lexical / vector recall、RRF、cross-encoder 或 rerank；
- candidate score、Edge score、path score；
- 初始 Node / Edge 选择；
- BFS、beam search、best-first、restart 或 retrieval forest；
- Agent 渐进遍历 Prompt；
- Meeting 专用检索行为；
- 新的 Coordinate Node、Edge summary、Edge embedding 或 Role Context；
- embedding 自动修改 canonical summary；
- 独立图数据库或独立向量数据库；
- SQL schema、migration、worker、队列、CLI、ACP 或 Desktop 实现；
- 具体 embedding 模型、维度和供应商选择；
- 历史 Revision / pinned snapshot 的语义索引与检索；
- 首版正文 chunk 的实际交付。

后续语义检索设计只能消费本文定义的基础，不得反向改变来源摘要所有权、Coordinate identity 或 Edge
领域语义。

## 13. 必须保持的不变量

1. Canonical Source 是其 title、summary 和完整内容的唯一 canonical owner。
2. embedding 是可删除、可重建、模型版本化的 PostgreSQL 派生索引。
3. embedding 不进入来源对象、Coordinate、EdgeKey、Binding 或 signed project projection。
4. Coordinate 的 title / summary 变化不改变 Coordinate identity。
5. embedding model 或 extractor 升级不推进任何业务 revision。
6. 来源 summary 更新不推进 Project Context revision。
7. Edge attach / detach 不重新编码未变化的来源内容。
8. Edge 不拥有聚合 summary 或单一 embedding。
9. 同一 Edge 的多份 Context Document 分别提供语义，不被静态平均或累加。
10. 同一 Document 的普通来源、Document Coordinate 与 Context Document 角色复用同一组 Semantic Units。
11. 一份 Context Document 的关系语义只通过当前 canonical binding 映射到 Edge。
12. Hyperedge 始终保持完整精确 Coordinate set，不因语义检索拆成隐含二元边。
13. content chunk 始终映射回来源对象，不成为 Coordinate、Edge 或独立项目事实。
14. Semantic Unit 数量不代表来源或 Edge 的重要性。
15. 旧索引任务不能覆盖更新后的 source basis。
16. 同一来源版本的 overview / chunks 必须作为完整集合激活。
17. 业务终态不自动失去语义资格；显式删除、bodyless tombstone或失权来源的旧向量不能作为当前候选返回。
18. `summary = None` 和 `embedding missing` 均不表示来源不相关。
19. raw embedding 不对普通项目客户端公开。
20. 清空整个语义索引后，可以只依赖 canonical source 与当前 Project Context 结构完整重建。
21. 两个来源即使 semantic text 完全相同，也保留各自独立的来源身份与 provenance，不因 digest 相同而合并。
22. 维度、模型空间或数值合法性不符合 active model contract 的向量不能激活。

## 14. 验收场景

### 14.1 Coordinate overview

- 创建带 title / summary 的 Work 后，最终产生一个可追溯到该 Work 当前 revision 的 overview embedding；
- 创建不带 summary 的 Issue 后，可以产生 title-only overview，并明确标记 summary missing；
- 修改 Work summary 后，旧向量立即失去 current 资格，新向量异步激活；
- 只修改未进入 semantic text 的 status 时，允许基于相同 digest 复用 embedding；
- Work 进入 `completed`、Issue 进入 `closed`、Requirement 进入 `satisfied` 后仍保留 current overview；
- 这些业务终态作为 lifecycle / status metadata 可供后续查询选择，不需要重新定义为 tombstone；
- Work embedding 缺失不影响 Work create / update 成功。

### 14.2 Edge 与 Context Document

- 一条 Edge 绑定三份 Context Document 时，三份 Document 分别拥有 overview embedding；
- 系统不生成第四份“Edge embedding”；
- 删除其中一份 binding 不删除该 Document 作为普通来源对象的 embedding；
- 同一 Document 同时作为某条其他 Edge 的 Coordinate 时，仍复用同一来源 embedding；
- 通过 Context Document 命中映射 Edge 时，返回的是当前 exact Hyperedge，不生成二元关系。

### 14.3 Chunk 兼容性

- 一个 Document 可以拥有 overview 与多个 content chunks；
- chunk 命中始终能够定位回 Document 与来源内结构路径；
- chunk 不出现在 Project Context Coordinate 集合中；
- Document 某一章节变化时，未变化 chunk 可以按 digest 复用，完整新集合通过 source-basis CAS 后激活；
- 一个长 Document 不因 chunk 数量多而在基础层获得静态重要性。

### 14.4 模型与重建

- 同一 overview 可以并行生成 model A 与 model B 的 embedding；
- model B 建设期间，model A 的 current generation 继续保持一致；
- 切换 active model 不修改来源或 Project Context revision；
- 删除全部 semantic index 表内容后，可以从 canonical sources 重建相同 unit identity、digest，并在同一模型
  合同下重新生成有效向量；
- extractor 行为变化时通过新 extractor version 重建，不静默复用旧 digest。

### 14.5 权限与生命周期

- Community A 的向量查询不能观察 Community B 的候选、数量或距离；
- Work `completed`、Issue `closed`等业务终态仍可作为current候选；
- 来源被显式Delete并成为bodyless tombstone后，旧embedding不再作为current候选；
- 来源读权限被撤销后，即使向量物理行尚未清理，也不能参与该调用者召回；
- 恶意 summary 中的命令文本只作为数据编码，不触发工具调用或权限变化；
- 外部 encoder 不可用时，索引进入 missing / failed 状态，不影响 canonical source。

## 15. 后续文档边界

完成本文后，后续设计按顺序拆分：

1. **语义索引实现设计**：pgvector extension、表结构、source adapters、outbox / worker、模型合同、
   current-set activation、rebuild 与运维；
2. **通用图语义查询规范**：自然语言问题、可选 `initial_coordinates[]`、查询环境、候选类型、权限、覆盖与
   provenance；
3. **语义路径检索设计**：全局入口、Coordinate / Context Document 召回、图上展开、预算、重启与结果
   森林；
4. **Agent 使用设计**：Agent 如何把语义检索结果与 canonical reads、现有 exact / incident /
   contains-all 查询结合。

上述任何后续能力都不得把 embedding 提升为项目事实，也不得重新给 Coordinate、Edge 或 Role 建立第二份
上下文所有权。
