# Project Context 图语义化基础分阶段开发计划

> 状态：基础实现已交付；本地资格验证通过，生产灰度待 Operator 执行
>
> 日期：2026-08-11
>
> 计划起始代码基线：`feat/context-semantic` @ `a66b376975`
>
> 规范基线：
> [Project Context 图语义化基础规范](project-context-graph-semantic-foundation-spec.md)
>
> 计划范围：pgvector 部署前置、Semantic Unit 与 extractor、派生索引、Project View / Document /
> Meeting 来源接入、异步 embedding worker、重建与 generation 运维、灰度验收
>
> 明确排除：公开语义查询 API、自然语言查询输入、初始 Coordinate 选择、相关性排序、FTS / Vector
> 融合、图路径搜索、Agent / CLI / Desktop 使用方式、首版正文切片

> 资格证据：
> [Project Context 图语义化基础资格报告](project-context-graph-semantic-foundation-qualification.md)

## 0. 实际交付状态

本计划已经在 `feat/context-semantic` 完成基础实现。各阶段当前状态如下：

| Phase | 状态 | 已交付结果 |
|---|---|---|
| 0 | 已完成 | PostgreSQL 17 + pgvector 0.8.5 固定镜像、Compose / Harness / Helm / CI 接入、2048 维 preflight 与部署 runbook |
| 1 | 已完成 | `buzz-semantic` 纯合同、`overview-v1` extractor、Markdown 可见文本规范化、2048 维模型合同与 deterministic fake encoder |
| 2 | 已完成 | additive `0057` schema、fresh schema 同步、Community capability gate、source / unit / embedding / head / job / rebuild / rate-gate 派生表 |
| 3 | 已完成 | Project View、Project Document、Meeting typed source adapters；来源事务内同步失效与 durable job coalescing |
| 4 | 已完成 | leased worker、Volcengine adapter、完整 unit-set CAS 激活、向量复用、共享 provider rate gate、metrics 与 readiness |
| 5 | 已完成 | `buzz-admin semantic` preflight、generation、enable / disable、durable rebuild、verify、activate / rollback、retry、retire、purge、GC |
| 6 | 本地资格已完成 | migration / fresh schema / pgvector / worker race / generation cutover 与 rollback 自动化通过；生产 Community 尚未启用 |

实际交付仍严格停在 foundation 边界：没有公开 semantic query、没有 ANN/HNSW 生产访问路径、没有图路径
检索、没有 Agent/CLI 查询入口，也没有把正文或 chunk 发送给外部 provider。首个生产灰度必须由 Operator 按
[部署与升级 runbook](../../../semantic-pgvector-operations.md) 对一个明确授权的 Community 执行，不能由构建或
Relay 启动自动触发。

## 1. 计划目的

本计划把已经确认的“图语义化基础”拆成可以独立开发、审查、验证和回滚的阶段。

最终要交付的不是一套直接回答问题的图检索器，而是一层可靠的机器语义基础：

```text
Canonical Project Sources
├── Project View object title + summary
├── Project Document title + summary
└── Meeting title + summary
        │
        ▼
versioned semantic extraction
        │
        ▼
PostgreSQL / pgvector derived index
├── source identity and typed source basis
├── overview Semantic Unit
├── model-versioned embedding
├── currentness / coverage state
└── rebuild and generation lifecycle
```

完成本计划后，系统应当具备：

1. 从三类 canonical source 确定性提取 `overview` Semantic Unit；
2. 异步生成、保存并原子激活当前 embedding；
3. 在来源变化、删除或失效后，立即阻止旧向量冒充当前内容；
4. 在 encoder 故障、worker backlog 或索引重建时继续正常写入 canonical source；
5. 删除全部派生数据后，只依赖 canonical source 完整重建；
6. 同时维护多个模型 generation，并安全切换或回滚；
7. 为后续通用语义查询和图路径检索提供稳定输入。

完成本计划**不表示**以下能力已经可用：

- 用户用自然语言直接搜索图；
- 系统自动选择初始 Node / Edge；
- 根据 problem、Role、Work 等环境产生不同路径；
- Agent 自动取得相关上下文；
- Desktop 展示语义命中；
- ANN 排名已经满足权限和召回要求。

这些能力必须在本基础交付并验收后单独设计。

## 2. 已确认的交付原则

### 2.1 Canonical Source 仍是唯一内容 owner

Project View、Project Document 和 Meeting 各自拥有 title、summary 与完整内容。

本计划不会把它们复制进：

- `ProjectContextCoordinate`；
- Project Context Edge / Binding / Meta；
- 新的 Coordinate Node；
- signed Project Context projection；
- Agent 私有上下文层。

Semantic Unit 和 embedding 是可删除、可重建的数据库派生状态，不成为第二份项目事实。

### 2.2 Edge 不生成 summary 或 embedding

Edge 仍只保存精确无向 Coordinate 集合和 Context Document binding。

一条 Edge 的关系语义来自它绑定的每一份 Project Document。Document 的 title / summary 生成自己的
Semantic Unit 和 embedding；查询时再通过当前 binding 映射回 Edge。

本计划不实现：

- `Edge.summary`；
- `Edge.embedding`；
- 多份 Context Document 的向量平均；
- Edge 方向、关系类型或静态相关性权重。

### 2.3 首版只交付 overview

首版每个符合资格的 source 只生成一个 `overview` unit：

```text
source type label
+ source-native title / name
+ optional source-owned summary
```

`content_chunk` 的身份空间从 schema 和类型设计上预留，但本计划不交付正文切片器、chunk embedding 或
chunk 查询。

### 2.4 pgvector 是数据库版本前置，语义能力默认关闭

从 semantic schema 发布版本开始，pgvector 作为 Buzz PostgreSQL 的部署前置能力。这样可以继续使用仓库
现有的单一 SQLx migration 与 `../../../schema/schema.sql`，不额外建立第二套可选 migrator。

同时必须区分：

```text
Database prerequisite: pgvector installed and schema-ready
Runtime capability:     disabled by default per Community
Worker/provider:        started only when explicitly configured and enabled
Public semantic query:  not delivered by this plan
```

因此：

- 官方本地、Harness、CI、self-hosted Compose 和 Helm quickstart 必须先具备 pgvector；
- 外部 PostgreSQL 必须在升级 semantic schema 前由 operator 完成 preflight；
- `semantic_index_enabled` 或等价 Community gate 默认 `false`；
- 未启用的 Community 不会把内容发送给外部 encoder；
- pgvector 存在不代表自动建立索引、自动回填或开放查询。

Community gate 只控制 worker、provider 和未来 query 的可用性，不停止 canonical source change capture。
Community 被 disable 后，currentness fence 仍随来源写入推进，dirty job 可以继续积压但不会被消费；重新 enable
前必须完成 reconcile、drain 和 generation verify。这样旧 head 不会因为一次 disable / re-enable 重新变成
current。

### 2.5 来源写入不等待 encoder

Canonical source mutation 只能同步完成轻量的派生 currentness fence 和 durable dirty job / outbox 写入。

来源事务中禁止执行：

- Markdown chunking；
- tokenizer；
- embedding provider 请求；
- vector 生成；
- ANN 索引维护工作流；
- 大范围重建。

encoder 超时、provider 不可用、worker 停止或 job backlog 都不能反向把 Project View、Document 或 Meeting
写入报告为失败。

### 2.6 已确认的首个模型与数据出域边界

首个 production-like model profile 已确认如下：

```text
provider:                        Volcengine Ark（外部 provider）
request model:                   doubao-embedding-vision-251215
vector dimensions:               2048
distance metric:                 cosine
client-side normalization:       none
encoder input contract version:  overview-v1
provider execution boundary:     Buzz → Volcengine HTTPS API
```

2026-08-11 已使用开发环境配置对 `/api/plan/v3/embeddings` 做最小真实探测：固定版本模型可以直接请求，
`dimensions = 2048` 返回恰好 2048 个有限浮点值。API key、endpoint credential和原始vector不得写入本文、
日志、fixture或generation metadata。production配置应使用semantic专属secret/config；本地spike可以读取现有
开发环境配置，但不能把通用LLM配置变成长期隐式依赖。

显式启用某个Community的semantic capability，同时表示该Community授权把以下`overview-v1`输入发送给
Volcengine：

```text
source type label
+ source-native title / name
+ optional source-owned summary
```

授权不包括 description、Document body、Meeting Board / Speech、Project Context topology、Role / Work lens、
source UUID或未来`content_chunk`。正文切片的数据出域必须在后续chunk设计中重新显式确认；在此之前，
external provider adapter必须拒绝`ContentChunk`，不能因为schema预留了unit kind就发送正文。

本授权不替代provider retention / logging / deletion条款的运维审查。该审查由Phase 0形成记录，不再要求
产品方提供模型参数。

### 2.7 currentness 必须 fail closed

异步索引可以暂时缺失，但不能返回旧内容冒充当前内容。

来源变化时必须先推进 semantic currentness fence，并使所有旧 source-generation head 失去 current
资格；随后 worker 才异步生成新值。未来查询必须同时验证：

- semantic head 指向的 typed source basis；
- canonical source 当前 observation；
- 当前 lifecycle；
- 当前调用者权限。

### 2.8 首版只使用 writer PostgreSQL

semantic source observation、worker、verify、cutover、rollback 以及后续查询的首版实现全部使用 writer
database pool。

现有 `READ_DATABASE_URL` replica 可能滞后于 source invalidation。没有额外 WAL freshness fence 前，不得让
semantic currentness 或权限安全依赖 read replica。

### 2.9 不扩张现有 NIP-50

当前 `buzz-search` 面向 event FTS，Project View、Project Document 和 Project Context 私有投影被有意排除。

本计划不把这些 projection kinds 塞进普通 NIP-50，也不公开 raw embedding。后续语义查询必须建立独立、
鉴权且 Project-scoped 的查询合同。

## 3. 当前实现基线与缺口

### 3.1 已有能力

| 能力 | 当前状态 | 本计划如何复用 |
|---|---|---|
| Project View summary | 九类对象均由来源保存 optional summary | 作为 overview 输入 |
| Project Document summary | 当前 Document revision 保存 title / summary / body | overview 首版只取 title / summary |
| Meeting summary | `meeting_sessions.summary` 已存在 | 与 canonical Meeting title 一起作为 overview 输入 |
| Project Context V2 | Coordinate、精确 Hyperedge、Context Document binding 已稳定 | 只在未来查询时解析来源和 binding |
| PostgreSQL | canonical source 与 graph 已在同库 | 同库增加 pgvector 派生索引 |
| Durable worker 模式 | push queue、Meeting outbox 等已有 lease / retry 模式 | 复用 claim fence、`SKIP LOCKED`、backoff 与 recovery |
| Generation 运维模式 | Project View / Document 已有 migration、reprojection 与 readiness 经验 | 复用显式 status / verify / cutover / rollback 思路 |

### 3.2 当前硬缺口

1. `../../../docker-compose.yml`、`../../../docker-compose.harness.yml` 和 self-hosted Compose 当前使用普通
   `postgres:17-alpine`，不自带 pgvector；
2. Helm quickstart 与外部 PostgreSQL 尚无 pgvector capability contract 和 preflight；
3. workspace 没有 pgvector Rust adapter，也没有经过 SQLx 0.9 验证的 vector round-trip；
4. 没有统一 `CanonicalSemanticSourceObservation`；
5. 没有 versioned overview extractor；
6. 没有 semantic source currentness fence、unit set、embedding generation 或 job 表；
7. 没有覆盖三类 source 全部写路径的 invalidation / enqueue；
8. 没有 embedding worker、provider contract、coverage metrics 或 rebuild 工具；
9. 没有 semantic query，因此也没有已经证明安全的 ANN access path。

### 3.3 Migration 基线

当前最新 migration 是 `0056_meeting_summary.sql`。

下一份 semantic schema migration 当前预计为 additive `0057_*`，但必须在 Phase 0 的 pgvector 部署前置
完成后才能合入。并行开发可能继续占用 migration 编号，因此实施合并前必须重新确认下一个可用编号，不能
只依赖本计划中的预留值。实现时还必须同步：

- `migrations/*.sql`；
- `../../../schema/schema.sql`；
- `../../../crates/buzz-db/src/migration.rs` 的 migration 数量与逐版断言；
- fresh schema / brownfield upgrade drift tests；
- tenant table 的 `community_id` leading-key lint。

不得修改已经发布 migration 的内容或 checksum。

## 4. 阶段总览

```text
Phase 0  部署可行性、模型信任边界、实现设计冻结
   │
   ├───────────────┐
   ▼               ▼
Phase 1          Phase 2
纯语义合同        pgvector 派生 schema（capability off）
   └───────┬───────┘
           ▼
Phase 3  三类 source adapter + 原子失效 / dirty capture
           ▼
Phase 4  leased worker + provider + shadow indexing
           ▼
Phase 5  rebuild / verify / generation cutover / rollback
           ▼
Phase 6  集成验收、灰度发布、基础交付完成
```

阶段依赖规则：

- Phase 0 是硬门槛；
- Phase 1 与 Phase 2 可在 Phase 0 决策冻结后并行；
- Phase 3 必须同时依赖 Phase 1 和 Phase 2；
- Phase 4 必须在 source currentness 已可证明后开始；
- Phase 5 必须复用已经稳定的 worker 与 generation 合同；
- Phase 6 只在完整 rebuild、故障恢复和 rollback 验证通过后开始。

任何阶段都不得用“后续查询会过滤”掩盖本阶段的 currentness、Community 隔离或来源身份缺口。

## 5. Phase 0：部署前置与实现合同冻结

### 5.1 目标

证明 Buzz 的所有受支持 PostgreSQL 部署面都能可靠使用 pgvector，并冻结 extractor、model、provider、
currentness 与容量的实现合同。

本阶段不创建 production semantic table，不修改 source 写入，不启动 worker。

### 5.2 工作包 A：pgvector 部署前置

目标部署已确认允许安装pgvector。该产品决策解除“数据库是否允许extension”的设计阻断，但不能替代每个
实际环境的installed extension / vector type preflight，也不能代表Buzz运行账号拥有`CREATE EXTENSION`
权限。

需要覆盖：

- local `../../../docker-compose.yml`；
- integration `../../../docker-compose.harness.yml`；
- self-hosted `../../../deploy/compose/compose.yml`；
- CI migration / schema drift PostgreSQL；
- Helm quickstart PostgreSQL subchart；
- external managed PostgreSQL；
- writer 与未来 replica 的 extension 兼容性。

交付内容：

1. 选择并固定 PostgreSQL 17 + pgvector 的镜像版本与 digest；
2. 明确 external DB 的受支持版本、extension 安装权限与升级顺序；
3. 增加只读 preflight，至少验证：
   - `pg_available_extensions` 能发现 `vector`；
   - extension 已经安装；
   - `vector` type 和基本运算可用；
   - writer 使用的版本与配置满足合同；
4. 对 external managed PostgreSQL，要求 operator 先通过数据库提供商的特权通道安装 extension；Buzz
   preflight 不声称能够通过只读 catalog 证明当前账号拥有 `CREATE EXTENSION` 权限；
5. 对已经存在的 PostgreSQL volume 执行原位镜像切换与回滚演练，覆盖 PG major / minor、data directory、
   UID / GID、locale、extension package版本和已有数据卷启停；
6. 形成“先准备数据库、再执行 semantic schema migration”的 runbook；
7. 首次 semantic schema rollout 明确把 Helm `migrate.autoMigrate` 暂时设为 `false`，先运行 preflight，
   再由一次显式 `buzz-admin migrate` 执行 migration，验证完成后才恢复常规 deployment设置。

`0057` 不负责安装 pgvector。它只依赖并校验已经存在的 extension / vector type。仓库现有 SQLx migrator的
advisory lock仍然保留；首次显式migrate是扩展前置的发布门，不是否定现有多pod migration序列化机制。

建议提供 operator 命令：

```text
buzz-admin semantic preflight
```

具体命令名称在实现设计中冻结。

### 5.3 工作包 B：Rust / SQLx spike

最小 spike 必须证明：

- workspace 当前 SQLx 0.9 与选定 pgvector Rust adapter 兼容；
- vector 可安全 bind / decode；
- 维度不匹配、NaN、Infinity 和非法值会 fail closed；
- exact distance query 可运行；
- 多model不同维度的物理隔离方案已经比较并固定，包括column typmod、表 / 分区边界、generation过滤和
  未来partial / expression ANN index的可行性；
- HNSW / IVFFlat 是否可用只做可行性验证，不在本阶段承诺 production index；
- migration 与 `../../../schema/schema.sql` fresh apply 都能识别 vector type。

2048维超过pgvector当前`vector` HNSW / IVFFlat索引的2000维上限，但没有超过普通`vector`存储上限，
也没有超过`halfvec` ANN索引的4000维上限。因此首个model的物理方向固定为：

1. 完整provider输出保存在single-precision `vector` 中，并以generation contract校验
   `vector_dims(embedding) = 2048`；
2. exact cosine distance和最终rerank使用完整`vector`；
3. 后续需要ANN时，为该generation建立`embedding::halfvec(2048)`的partial / expression HNSW索引，
   operator class使用`halfvec_cosine_ops`；
4. ANN只产生扩大后的候选集，最终顺序由完整`vector` cosine rerank；
5. generation隔离必须保证不同dimension或model不会命中同一表达式索引。

Phase 0 spike必须实测cast、index build、candidate recall和full-vector rerank。若实测结果不满足召回要求，
只能保持exact scan或建立新的model generation，不能静默把2048维截断到2000维。

### 5.4 工作包 C：模型与信任边界

首个model profile已由第2.6节冻结。实现仍须把以下字段作为closed generation contract持久化和验证：

```text
model id / version
vector dimensions
distance metric
normalization rule
encoder input contract version
provider execution boundary
```

实现与运维仍须完成：

- 对Volcengine输入日志、保留期限和删除方式形成审查记录；
- 确认per-Community enable只授权第2.6节的overview输入；
- 把model返回值的resolved model id与generation contract核对，不允许alias静默漂移；
- timeout、rate limit、批处理和最大输入覆盖策略；
- 测试使用的 deterministic fake encoder。

在没有实测provider配额前，首次shadow运行采用保守、可配置的初值：

```text
enabled communities:          1
semantic worker instances:    1
request items per batch:      1
max in-flight requests:       2
sustained request rate:       1 request / second
burst:                        2 requests
request timeout:              30 seconds
```

429必须尊重`Retry-After`并走durable retry；5xx和网络错误使用有界指数退避。不得把这些初值编码成provider
协议常量。只有在灰度指标证明错误率、延迟、费用和队列恢复均可接受后，operator才能显式提高配额。

### 5.5 工作包 D：容量与成本基线

至少统计：

- 每个 Community 的 Project View / Document / Meeting source 数量；
- title-only 与 summary-present overview 的比例；
- 当前 summary 大小分布；
- 单 model 的向量存储、job、unit set 和 generation 开销；
- 全量 backfill 的 provider 请求量、成本与预计耗时；
- 多 generation 并存和 rollback window 的额外磁盘。

### 5.6 阶段交付物

- semantic foundation implementation design；
- pgvector deployment / upgrade runbook；
- SQLx vector spike 与测试；
- model / provider trust-boundary decision；
- capacity report；
- Phase 1—6 使用的 frozen terminology 与 version contracts。

### 5.7 退出门槛

- 所有官方 PostgreSQL 路径以及受支持矩阵内、已由operator安装extension的external DB都能使用
  `vector`；
- extension 缺失时，preflight给出明确、可操作的安装指引，不虚假判断账号安装权限；
- 已有持久卷经过pgvector镜像原位启动与回滚演练；
- 未启用 semantic capability 时，现有 Buzz 行为不变；
- fake encoder 可完全离线运行测试；
- 真实 provider 未经明确授权不会收到任何 Project 文本；
- 已决定 vector schema 如何进入单一 migration / desired schema；
- 已对首个 model profile、维度和输入覆盖达成结论。

未满足任一项时，不得合入 semantic schema migration。

## 6. Phase 1：纯语义合同与 overview extractor

### 6.1 目标

建立不依赖数据库和网络 provider 的纯语义核心，使三类 source 对同一输入产生稳定、可测试、可版本化的
Semantic Unit。

推荐新增独立 crate：

```text
crates/buzz-semantic
```

最终名称由 Phase 0 implementation design 固定。

### 6.2 类型合同

至少定义：

- `SemanticSourceIdentity`；
- Project View / Document / Meeting 的 closed source family / subtype；
- typed source basis；
- `CanonicalSemanticSourceObservation`；
- source-native lifecycle / status filter metadata；
- `SemanticUnitKind::{Overview, ContentChunk}`；
- `SemanticUnitIdentity`；
- source snapshot digest；
- semantic text digest；
- extractor version；
- model contract；
- coverage / failure state；
- embedding value validation；
- encoder trait 与 deterministic fake implementation。

`ContentChunk` 只保留类型与身份兼容性，不实现实际 chunk extractor。

### 6.3 Overview extractor

首版 extractor 严格只接收：

```text
source type label
+ title / name
+ summary, when present
```

不得自动加入：

- description、purpose、Board、Document body；
- Role、Work、当前 query 或 caller 环境；
- 相邻 Coordinate / Context Document；
- status、priority、assignee、revision；
- Project Context topology；
- UI fallback 文本。

### 6.4 Markdown 规范化

实现 versioned、确定性的 visible-text extractor：

- 不执行 Markdown、HTML、链接或命令；
- 相同输入和 extractor version 产生相同 bytes / digest；
- 规范化规则不依赖 Desktop theme、viewport 或 locale；
- 行为实质变化必须提升 extractor version；
- 不把规范化结果写回 canonical summary。

### 6.5 输入覆盖

summary 不增加 semantic 专属长度上限。

如果 active model 无法完整处理 overview：

- 不得静默截断后标记为完整；
- 记录 canonical semantic text digest 与实际 coverage；
- 当前策略不能完整覆盖时进入 `unsupported` / `failed`；
- 未来采用分段、池化等策略时必须提升 extractor 或 encoder input contract version。

### 6.6 阶段测试

- 九类 Project View object 的 table-driven fixtures；
- Project Document active / deleted、summary present / missing；
- Meeting active / ended、summary present / missing；
- Work completed、Issue closed、Requirement satisfied仍是eligible current source；
- Markdown、多语言、代码标识和恶意命令文本；
- 相同输入的 digest determinism；
- source扫描或集合枚举顺序变化，不改变每个source自己的unit identity和digest；
- 相同 semantic text 的两个 source 保留不同 source identity；
- 非语义 status / priority 变化不改变 semantic text digest；
- 维度、NaN、Infinity 和 model-contract mismatch 拒绝；
- 超模型输入不被静默截断。

### 6.7 退出门槛

- overview extractor 有 golden fixtures；
- extractor 不读取数据库、网络或 Project Context graph；
- fake encoder 能稳定驱动后续 DB / worker 测试；
- typed source basis 不伪造跨领域统一 revision；
- `summary = None` 明确形成 title-only coverage，而不是“不相关”；
- content chunk 没有被偷渡进首版交付。

## 7. Phase 2：pgvector 派生 schema，默认 capability-off

### 7.1 目标

以 additive migration 建立语义派生状态和 durable work queue，但不回填、不启动 worker、不激活 generation，
也不改变任何现有 source / graph 行为。

### 7.2 逻辑 schema

具体表名和列在 implementation design 中冻结，逻辑上至少需要：

```text
Semantic model / index generation
├── Community scope
├── extractor contract
├── model contract
├── lifecycle: building / ready / active / rollback-ready / retired / failed
└── activation metadata

Semantic source currentness
├── source identity
├── eligibility
├── typed current basis / snapshot digest
├── invalidation epoch
└── coverage state

Semantic unit set
├── source basis
├── extractor version
├── complete unit count
└── staging / active / retired state

Semantic unit
├── unit kind / key / ordinal / path
├── semantic text digest
├── summary coverage
└── extraction provenance

Semantic embedding
├── semantic unit
├── model generation
├── vector
└── indexed metadata

Source-generation head
└── atomic pointer to one complete unit set with complete embeddings

Durable semantic job
├── latest desired source basis / invalidation epoch
├── claim id / lease
├── attempt / next attempt
└── bounded failure metadata
```

Semantic Unit 与 embedding 必须分层：同一个 unit set 可以被多个 model generation 复用，不能因为模型 B
重建就复制或改写 source-level unit identity。

Phase 0冻结的多维物理隔离方案必须在这里落地。即使production ANN index延后，Phase 2也必须证明不同
dimension的vector不能被同一次distance运算或错误generation query混用，未来每generation的partial /
expression index具有明确、无破坏性的DDL入口。

### 7.3 Schema 不变量

- 所有 Community 数据表以 `community_id` 作为 leading key；
- source family / subtype / unit kind / lifecycle 使用 closed values；
- raw embedding 不进入通用 `events` 表或 Nostr projection；
- current head 只能指向完整 unit set；
- staging set 不可被普通 current read 观察；
- model dimension、metric 与 normalization 由 generation contract 约束；
- capability 默认关闭；
- 删除所有 semantic rows 不影响 canonical source 或 graph；
- migration 不做全量 backfill；
- Relay startup 不做 generation cutover。

### 7.4 ANN index 边界

首版 schema 可以存储并做小规模 exact vector 运算，但不在普通 migration 中创建 production ANN index。

原因：

1. model generation 可能拥有不同维度；
2. `CREATE INDEX CONCURRENTLY` 不适合普通事务 migration；
3. ANN 候选形成前的 Community / ACL 过滤尚待查询规范固定；
4. 过早建立 HNSW / IVFFlat 会把查询假设写进基础 schema。

production ANN access path 由后续查询设计和 operator DDL workflow共同决定。

### 7.5 Readiness

新增 live catalog probe，检查：

- pgvector extension / type；
- semantic 关键表、列、约束和必要 B-tree index；
- per-Community enable 状态；
- active generation 与 model contract；
- worker / provider readiness。

Community 未启用时，整体 Buzz deployment 仍可正常工作；Community 一旦启用，则 semantic schema、active
generation 和 worker/provider readiness 必须 fail closed。

### 7.6 Migration 验证

建议新增专门脚本，而不是只依赖偏向 Project View / Document 的现有 migration test：

```text
scripts/test-semantic-migrations.sh
```

至少覆盖：

- empty DB fresh SQLx migration；
- populated `0056 → 0057` upgrade；
- 两个并发 migrator；
- `../../../schema/schema.sql` ledger-less fresh apply；
- migration / desired schema no-drift；
- extension missing；
- migration账号不负责安装extension，缺失时提供确定失败；
- tenant-key lint；
- capability default-off；
- semantic rows 全删后 canonical state不变。

### 7.7 退出门槛

- Phase 0 所有部署面通过 vector preflight；
- `0057` 与 fresh schema 完全等价；
- schema 默认不启动任何 semantic 行为；
- 没有 active generation、没有自动 backfill；
- 当前 Project View / Document / Meeting / Project Context tests 不受影响；
- 没有 Edge embedding 或第二份 source summary；
- ANN 索引没有被提前固化。
- 多model / 多dimension的存储和安全查询方案已通过数据库集成测试。

## 8. Phase 3：Canonical source adapters 与变更捕获

### 8.1 目标

建立三类 source 的统一 verified observation，以及能够覆盖所有 canonical 写入路径的原子 currentness
失效和 durable dirty capture。

本阶段仍可使用 fake encoder；重点是证明“观察了什么”和“旧值何时失效”。

### 8.2 统一 adapter 合同

建议由 `buzz-db` 提供内部 typed API：

```text
observe_current(source_identity)
  -> CanonicalSemanticSourceObservation

scan_current_sources(community, family, cursor, limit)
  -> page<CanonicalSemanticSourceObservation>
```

adapter 必须：

- 直接读取并验证 canonical PostgreSQL state；
- 复用领域 parser / validator；
- 不消费 CLI DTO、Desktop fallback 或 Context preview；
- 不把 best-effort signed discovery projection 当作 currentness owner；
- 在非法 canonical row 上 fail closed；
- 返回 typed source basis、snapshot digest、lifecycle 和 eligibility。

lifecycle / status必须与semantic text分离：它们供后续query过滤候选，不自动进入overview embedding，也不能
把completed / closed等业务终态归类为删除。

### 8.3 Project View adapter

覆盖九类 current v3 Project View object。

建议 basis 至少包含：

```text
schema version
+ object type / source type
+ object revision
+ source change provenance
```

`project_revision` 可以作为 provenance，但不能因为其他 object 的无关更新而强迫当前 object 重编码。

需要把 title / name 访问器收敛到 Project View domain，与现有统一 `summary()` accessor 对称，避免 semantic
层再复制一份类型 match。

资格边界：

- tombstone / `deleted_at` 为 ineligible；
- Role inactive、Work done、Issue closed 等业务状态仍是当前 canonical object，不能被误当删除；
- 这些业务状态作为source-native filter metadata返回，为后续`include terminal`类查询条件提供基础；
- status-only 更新如果 semantic text digest 未变，可以复用向量。

### 8.4 Project Document adapter

从 current Document pointer 与 current revision row 重建 verified Document snapshot。

建议 basis 至少包含：

```text
document revision
+ current source change provenance
```

Document catalog revision 可以作为观察 provenance，但不能因其他 Document 更新而强迫本 Document重编码。

资格边界：

- active revision 可索引；
- source-native deleted / tombstone lifecycle为ineligible；
- 合法active Document不会仅因为正文为空而失去overview资格；
- overview 只取 title / summary；
- exact Markdown body read capability为未来 chunk 保留，但本阶段不切片。

### 8.5 Meeting adapter

Meeting 必须联合 canonical Meeting session 与 canonical channel / create metadata取得 title 和 summary。

不得使用 kind `39000` 作为 currentness basis，因为它是 canonical commit 后的 best-effort discovery projection。

Meeting 没有 summary revision，本计划也不增加业务 revision。typed basis 应由现有 Meeting identity / lifecycle
证据加 semantic snapshot digest组成。

资格边界：

- ended Meeting 仍是可读 canonical source，不能被误当 tombstone；
- active / finalizing / ended 的索引资格按 Meeting source visibility合同判断；
- 是否已经属于 Project Context graph、当前是否可 attach，由后续查询通过 canonical graph / resolver判断，
  不能写进 embedding；
- summary SET / CLEAR、正常 End、abort / revocation 等会改变 source observation或eligibility。

### 8.6 原子 currentness 与 durable dirty capture

实现设计优先采用 canonical table trigger，或证明所有 canonical mutation transaction 都调用同一 typed helper。

无论实现形态如何，必须覆盖：

- Project View initialize、ordinary create / update / delete、continuity，以及任何会修改 canonical business
  body或object revision的typed repair / operator路径；
- Document create、update、delete、reprojection噪声排除；
- Meeting create、summary SET / CLEAR、V0 / V1 / V2 End、abort、revocation / recovery；
- meeting-backed `channels.name` 的当前通用update路径；
- 会改变Meeting source visibility / eligibility的channel delete或visibility update路径；
- 未来新增的canonical title repair路径。

Project View 的普通 reprojection 与 `restore_project_view_v3_membership_snapshot` 只恢复派生 projection / NIP-43
membership wire，不修改 canonical business body、object revision、source basis 或 semantic snapshot digest。
它们不得推进 semantic currentness fence，也不得产生 dirty job。若未来某个 recovery / repair 开始修改
canonical object事实，则必须按实际受影响对象进入前述变更捕获，而不能继续沿用纯投影豁免。

每次相关 source mutation 的同一事务只做：

1. 更新 semantic source currentness fence / invalidation epoch；
2. 使旧 source-generation head失去 current资格；
3. 对所有需要保持 current 的 generation upsert / coalesce 最新 dirty job。

不得在这个事务内调用 encoder。

同一 source 的重复或快速连续变化可以压缩成一个“最新 desired basis”job；被更新超越的 worker结果必须在
最终 CAS 时丢弃。

per-Community gate关闭时仍执行轻量currentness fence和dirty capture，只是不启动provider / worker /
query。re-enable必须先使所有旧head保持ineligible，运行full reconcile，处理最新desired basis并verify；
完成前不得恢复semantic query资格。

### 8.7 Graph 与 ACL 边界

- Edge attach / detach 不触发 source embedding；
- Context Document binding 只在未来查询时映射 Document semantic units；
- source scan覆盖所有符合来源资格的对象，不以“已经属于某条Edge”为索引前提；
- graph membership 不复制进 semantic source registry作为权威值；
- Member removal、ban 或 caller ACL 变化不批量重算向量；
- 后续查询在召回前和返回前实时验证 current ACL；
- dirty capture不成为权限边界。

### 8.8 阶段测试

- 三类 source create / update / delete 产生或合并 job；
- source事务 rollback 不留下 fence / job；
- duplicate / replay 不制造多个当前 basis；
- summary修改后旧 head立即不可 current；
- Project View projection-only reprojection 不排队；
- `restore_project_view_v3_membership_snapshot` 不改变source basis / digest，且产生零semantic job；
- 修改canonical business body / object revision的typed Project View repair会捕获受影响对象；
- Document projection-only reprojection 不排队；
- Meeting summary SET / CLEAR、不同协议 End 与 revocation均被捕获；
- Meeting channel rename、delete和visibility变化均更新或失效source observation；
- Edge bind / unbind 的 encoding job数为零；
- source scan按 Community / family / stable id keyset有界且可恢复；
- 从空 semantic状态扫描得到完整、确定的 eligible source集合；
- cross-Community identity碰撞不串数据。

### 8.9 退出门槛

- 三类 source adapter 均只依赖 canonical state；
- 所有 canonical写路径有自动化覆盖证据；
- 旧向量在新向量生成前已经失去 current资格；
- source mutation没有同步模型请求；
- graph attach / detach不重编码；
- ACL没有被错误固化进 embedding；
- scan可作为漏通知后的最终一致性修复面。

## 9. Phase 4：异步 worker 与 shadow indexing

### 9.1 目标

实现可恢复、多实例安全的 embedding worker，为显式启用的 Community 建立 overview embedding，但仍不向
Agent、Desktop 或普通查询开放结果。

### 9.2 Worker 生命周期

复用仓库现有 durable worker模式：

```text
claim latest due job
→ claim_id + lease_until fence
→ read current canonical source observation
→ extract complete unit set
→ reuse an exact embedding, or reserve Provider capacity and wait outside DB transaction
→ after wait, atomically revalidate Community gate + exact generation contract/lifecycle
  + exact claim/lease/current epoch + exact eligible source basis/snapshot
→ commit one non-reusable egress permit and immediately hand off to Provider
→ stage unit set + embeddings
→ re-read source currentness
→ CAS activate complete source-generation head
→ fenced complete / retry / discard
```

必须具备：

- `FOR UPDATE SKIP LOCKED` 或等价多 worker claim；
- lease expiry recovery；
- bounded batch；
- attempts / next-attempt backoff；
- poison job状态；
- graceful shutdown；
- stale claim不能 complete新claim；
- source改变时旧结果不能激活；
- Provider slot reservation只代表容量，不代表出域授权；
- slot等待期间提交的capability关闭或canonical title / summary变化必须让最终短writer
  `REPEATABLE READ`重验失败并保持零出域；已预约slot允许浪费，但不得复用；
- 最终permit commit后除同步metrics外必须直接调用Provider，不得插入另一个await窗口；
- retry不重复创建多个 current set。

最终事务的并发合同是严格的 writer-first / permit-first 二选一：Foundation disable 与 permit 竞争同一
Community row；Project View / Document 的 canonical writer 在 source trigger 中更新
`semantic_sources`，Meeting 的 Session summary / End、Channel name / delete 与 runtime phase writer 也全部经同一
trigger 更新该 row；trigger随后才coalesce job。permit按Community、generation、source、job顺序持有共享锁到
commit。若writer先持锁并在permit的`REPEATABLE READ` snapshot建立后提交，PostgreSQL返回`40001`而不是让
旧版本取得row lock，DB adapter把它归一为闭集`Unavailable`；若permit先持锁，writer等待permit commit，该
Provider batch按permit-first线性化。自动化并发回归会先确认permit确实阻塞在Community/source writer上，再
提交writer，并要求最终只返回`Unavailable`。

### 9.3 原子 unit-set 激活

即使首版只有 overview，也必须按“完整 source unit set”建立激活边界，为未来 chunks 保持兼容。

禁止观察：

- 一半新 unit、一半旧 unit；
- source basis A 的 unit + basis B 的 embedding；
- model contract不匹配的向量；
- staging set作为 current；
- 维度错误、NaN、Infinity 的向量。

### 9.4 Digest 复用

当 source revision推进但 semantic text digest、extractor version和 model contract不变时，worker可以复用
已有 embedding，并把新 source basis原子激活。

复用必须以 digest和完整合同为依据，不能只比较 title或近似文本。

### 9.5 Provider 实现

- deterministic fake provider始终用于单元和集成测试；
- production provider由 Phase 0 frozen contract实现；
- input不写入普通日志或 error metadata；
- error只保存 bounded、去敏后的分类信息；
- timeout、rate limit、batch和重试明确；
- provider关闭或故障时source写入继续成功；
- 未启用 Community的内容不能发送给provider。
- Community gate、generation、claim / lease / epoch、source basis / snapshot / eligibility必须在
  Provider等待之后的同一个最终transaction中全部通过，不能依赖等待前的observation或reservation。

### 9.6 Metrics 与可观测性

至少提供：

- enabled Community数；
- current / missing / building / stale / failed / ineligible source数；
- summary-missing overview数；
- queue depth、oldest job age；
- claim、success、retry、poison、lease recovery；
- provider latency / error class / rate-limit；
- CAS discard / superseded jobs；
- generation coverage；
- unit / embedding行数与估算磁盘；
- retired / stale / orphan staging数量与最老保留时间；
- semantic schema / worker / provider readiness。

不得在 metrics label中放 source title、summary、Document内容、UUID高基数或raw error文本。

### 9.7 阶段测试

- worker crash后lease recovery；
- 两个worker竞争同一job只有一方激活；
- encode中途source改变导致旧CAS失败；
- 重试和poison job有界；
- provider outage不影响canonical写入；
- status-only source更新复用embedding；
- Work completed、Issue closed、Requirement satisfied仍保持current semantic head；
- summary clear产生新的title-only overview；
- source delete / tombstone立即不可current；
- Meeting ended仍按source合同保持eligible；
- mixed dimensions / model contracts不能串用；
- raw source文本不进入日志；
- worker只使用writer pool。

Worker还必须安全清理失败或被supersede的staging set。清理只能在lease / claim fence证明没有活跃writer，
且目标不被任何current或rollback-ready head引用后执行。

### 9.8 退出门槛

- shadow indexing在测试Community稳定运行；
- worker多副本与重启不破坏current head；
- 所有失败均落入可观察coverage状态；
- encoder故障不会阻塞source写入；
- 没有public semantic结果面；
- 仍然没有Edge embedding或自动图推断。
- superseded staging不会无界增长，并有可审计的retention / GC证据。

## 10. Phase 5：Backfill、重建与 generation 运维

### 10.1 目标

提供显式、可暂停、可恢复、可验证的 operator能力，为现有source补齐embedding，并安全构建、切换或回滚
model generation。

这些能力进入 `buzz-admin` 或等价operator surface，不进入Agent-first `carryforth-cli` / `cf`。

### 10.2 Operator 能力

建议命令族：

```text
semantic preflight
semantic status
semantic generation create
semantic rebuild
semantic verify
semantic activate
semantic rollback
semantic retry-failed
semantic retire / purge
semantic gc
```

具体命令名称和参数由implementation design冻结。

### 10.3 Backfill / rebuild

重建流程必须：

- 只通过三类 canonical adapter读取；
- 按 Community、source family和stable id做keyset分页；
- 使用durable checkpoint；
- 幂等enqueue，不直接同步编码整张表；
- 可以中断、恢复和取消；
- 与并发source写入共同收敛到最新basis；
- 不在migration或Relay startup中执行；
- 不创建、修改或删除canonical source / Edge；
- 不推进任何业务revision。

### 10.4 Retention 与物理清理

立即失去current资格和物理删除是两个不同阶段。implementation design必须冻结派生数据retention policy，
并提供：

- source删除或tombstone后的unit set / embedding清理；
- 被新basis覆盖的retired set清理；
- abandoned staging set清理；
- retired generation清理；
- completed / poison job与bounded error metadata清理；
- 按source、Community或generation的可审计operator purge；
- provider侧删除义务与本地purge的对应记录。

GC不得删除：

- current head引用的set；
- active worker仍持有有效claim的staging set；
- rollback-ready generation观察窗口仍需要的embedding；
- 尚未通过retention cutoff和引用检查的数据。

GC与worker、rebuild、generation cutover并发时必须使用明确fence，不能只按时间盲删。

### 10.5 Generation 切换

标准流程：

```text
create generation B
→ shadow build B
→ verify contract and coverage
→ mark B ready
→ atomic active pointer A → B
→ keep A rollback-ready during observation window
→ retire / purge A only after explicit approval
```

为了允许真实rollback，A在观察窗口内必须继续接收source invalidation和jobs。若A已落后，则rollback命令必须
拒绝，要求先catch up并verify，不能把旧generation直接恢复成active。

### 10.6 验证内容

`semantic verify` 至少检查：

- eligible source集合与semantic currentness集合一致；
- 每个current head对应完整unit set；
- source basis / snapshot digest仍current；
- generation model / extractor contract一致；
- vector维度和数值合法；
- current / missing / failed coverage可解释；
- 没有跨Community引用；
- Document Coordinate与Context Document角色复用同一Document units；
- Edge attach / detach没有复制或删除Document embedding；
- raw embedding没有public projection。
- stale / deleted / abandoned derived state符合retention policy且没有悬空引用。

### 10.7 阶段测试

- 清空全部派生数据后全量重建；
- rebuild中断后resume不重复、不漏source；
- backfill与source并发修改只激活最新basis；
- generation A serving时完整构建B；
- A → B原子切换；
- rollback window内A/B同时保持current；
- B → A rollback；
- coverage不足、contract mismatch或stale generation拒绝activate / rollback；
- purge只删除derived state；
- source purge、staging GC、poison job cleanup不触碰current / rollback-ready数据；
- GC与worker claim / generation cutover并发时不误删；
- model切换不推进Project View / Document / Meeting / Context revision。

### 10.8 退出门槛

- operator可以从空索引恢复完整coverage；
- rebuild不要求停写canonical source；
- generation切换和rollback均有自动化证据；
- 旧generation不会在未验证时被恢复；
- 失败可以disable semantic capability而不影响Project Context和source读取；
- stale、deleted、abandoned staging和retired generation有有界、可审计的物理清理机制；
- 已形成值班、provider故障、queue堆积、磁盘和rollback runbook。

## 11. Phase 6：集成验收与灰度发布

### 11.1 目标

在不开放产品语义查询的前提下，证明整个基础层能够在真实部署中持续保持currentness、coverage、隔离和
可重建性。

### 11.2 发布顺序

建议严格按以下顺序：

1. 发布带pgvector的官方PostgreSQL镜像和external DB升级说明；
2. 在feature off状态完成preflight；
3. 显式运行semantic schema migration；
4. 发布包含pure semantic core、adapters和worker的Relay，Community gate仍关闭；
5. 为单个测试Community创建generation；
6. 使用fake或批准的provider进行shadow backfill；
7. verify并观察queue / coverage / source write latency；
8. 激活测试Community generation，但仍不开放public query；
9. 逐步扩大Community范围；
10. 形成foundation qualification report并冻结后续query的可依赖合同。

首个灰度Community不在文档中硬编码UUID。operator在执行时选择或创建一个专用非生产Community，且必须：

- 明确启用`semantic_index_enabled`；
- 同时包含有代表性的Project View、Document和Meeting source；
- 内容owner知道title / summary会按第2.6节发送给Volcengine；
- 初始只启用这一个Community；
- 不因为数据库中“第一个”或“最小UUID”而自动选择；
- qualification report记录实际Community identity、source数量、开始/结束时间和关闭方式，但不记录内容。

首次灰度使用第5.4节的保守provider初值。测试期间根据queue oldest age、429、provider latency和成本按需
调高，不需要产品方预先提供固定quota；任何调高都必须是显式配置变更并进入qualification report。

### 11.3 内部诊断

可以提供operator-only exact vector probe，用来证明vector存储、距离合同和source provenance正确。

该probe不是产品query，必须：

- 只对显式Community和operator运行；
- 不通过Nostr / CLI普通项目读取开放；
- 不定义最终top-K、ranking或path语义；
- 不绕过source currentness和Community隔离；
- 只返回身份、provenance和诊断distance，不输出raw embedding；
- 不作为未来query兼容承诺。

### 11.4 灰度指标

至少观察：

- source write p50 / p95与feature off基线；
- job enqueue开销；
- queue oldest age和积压恢复速度；
- current coverage和failed比例；
- provider吞吐、rate limit、错误率和成本；
- CAS superseded比例；
- rebuild耗时；
- generation双写期间磁盘增长；
- Relay restart / rolling deployment期间worker恢复；
- PostgreSQL CPU、IO、WAL和存储；
- zero cross-Community visibility violation。

### 11.5 回滚

故障处置分成互斥分支，不能把“停worker”和“generation rollback”机械串成同一条顺序：

```text
Provider / worker故障
→ disable query/provider for one Community
→ currentness capture继续、dirty jobs积压
→ 修复后reconcile + drain + verify
→ 再enable

Bad active generation, worker仍健康
→ verify rollback-ready generation仍current
→ 必要时catch up
→ atomic generation rollback
→ 再决定是否停止worker/provider

Semantic schema / DB故障
→ disable semantic capability
→ 不尝试generation rollback
→ 修复schema后full reconcile + verify
→ 再选择active generation和enable
```

回滚不得：

- down-migrate canonical source；
- 删除Project Context graph；
- 修改source summary；
- 自动drop pgvector extension；
- 自动重放旧embedding为current；
- 让旧binary读取包含未知canonical字段。

### 11.6 最终验收场景

1. 创建带summary的Work，异步得到current overview embedding；
2. 创建无summary的Issue，得到title-only overview和明确coverage；
3. 修改Work summary后，旧head立即失效，新head最终激活；
4. 只改status且semantic text不变时复用向量；
5. Work completed、Issue closed、Requirement satisfied后仍有current语义，并保留可过滤lifecycle metadata；
6. 删除Document后旧embedding不能current；
7. Meeting summary SET / CLEAR / End均正确推进semantic observation；
8. ended Meeting不被误判为tombstone；
9. Edge绑定三份Context Document时只存在三份Document语义，不存在第四份Edge语义；
10. detach Context Document不删除它作为普通Document的embedding；
11. provider停机时source写入继续成功，coverage进入missing / failed；
12. worker在source并发更新后不能激活旧basis；若更新在Provider slot等待期间先提交，旧title / summary还必须
    保持零Provider出域；
13. Community A不能观察Community B的索引状态或向量；
14. 清空semantic派生数据后能完整重建；
15. generation切换和rollback不修改任何业务revision；
16. capability关闭后Project View、Document、Meeting和Project Context仍正常工作。

### 11.7 退出门槛

- 所有官方和external DB部署路径有可执行runbook；
- enabled Community的每个eligible source处于current或明确的非current状态；
- currentness、Community隔离和原子head有并发测试证据；
- source写入不依赖encoder可用性；
- rebuild、cutover和rollback真实演练通过；
- metrics、alert与故障处置完成；
- no public query、no Edge embedding、no chunk交付边界保持；
- 形成签字确认的foundation qualification report。

## 12. 代码责任面

以下是预计责任面，不在本计划中冻结所有文件名：

| 责任 | 预计位置 |
|---|---|
| 纯semantic类型、extractor、provider trait | 新`../../../crates/buzz-semantic` |
| pgvector schema、adapter、jobs、worker DB API | `../../../crates/buzz-db/src/semantic.rs`及子模块 |
| worker orchestration | `../../../crates/buzz-relay/src/semantic_runtime.rs` |
| provider / worker config | `buzz-relay` config、`../../../.env.example` |
| model generation与operator命令 | `../../../crates/buzz-admin/src/semantic.rs` |
| Project View source adapter | `buzz-db` + `buzz-project-view`纯title accessor |
| Document source adapter | `buzz-db` + `buzz-project-document`现有typed snapshot |
| Meeting source adapter | `buzz-db` Meeting / Channel canonical read |
| Migration | 当前预计`migrations/0057_*`，实施合并前重新确认下一个可用编号 |
| Fresh schema | `../../../schema/schema.sql` |
| Deployment | Compose、Harness、CI、Helm、deployment docs |
| Metrics / readiness | Relay metrics与DB catalog readiness probe |
| Migration / rebuild tests | 新semantic migration脚本和DB integration tests |

本计划不要求修改：

- Project Context Coordinate / Edge / Binding wire；
- Project View、Document或Meeting canonical summary ownership；
- `carryforth-cli` / `cf` Agent surface；
- ACP Prompt；
- Desktop、Web、Mobile；
- generic `buzz-search` NIP-50合同。

如果实施中需要修改这些边界，必须先回到规范层重新评审，而不能作为某个Phase的顺手改动。

## 13. 跨阶段测试策略

### 13.1 Pure unit / golden

- identity、digest、Markdown规范化；
- source family / subtype closed parsing；
- model contract和vector合法性；
- title-only coverage；
- extractor version变化；
- fake provider determinism。

### 13.2 Database integration

- fresh / brownfield migration；
- tenant isolation；
- source adapter verified reads；
- currentness trigger / helper；
- job claim、lease、retry、poison；
- staging / complete set / head CAS；
- multi-generation；
- rebuild、cutover、rollback；
- delete / tombstone / lifecycle。

### 13.3 Failure injection

- provider timeout、429、invalid vector、wrong dimension；
- worker crash before / after staging；
- DB commit uncertainty；
- lease expiry；
- source mutation during encode；
- duplicate jobs；
- extension unavailable；
- disk pressure；
- rolling deployment with feature off / on；
- backfill interrupted and resumed。

### 13.4 Security

- Community identity碰撞；
- disabled Community不会发送文本，但currentness capture继续推进；
- raw embedding不出现在events / ordinary DTO / logs；
- malicious Markdown只作为数据；
- source失权后未来查询无法使用旧向量；
- read replica lag不能绕过writer currentness。

### 13.5 最终质量门

每个Phase运行其定向测试；Phase 6前至少运行：

```bash
. ./bin/activate-hermit
just ci
just test
```

同时运行新增的pgvector migration、worker race、rebuild和generation集成套件。Desktop / Mobile / Web不在
本计划代码范围内；如果实际改动触及它们，则追加各自完整quality gate。

## 14. 风险与强制控制

| 风险 | 强制控制 |
|---|---|
| 部署DB没有pgvector | Phase 0先升级镜像与external DB preflight，0057不得提前合入 |
| pgvector镜像与已有volume不兼容 | Phase 0执行原位启停、UID / GID、locale和回滚演练 |
| external provider泄露Project内容 | per-Community显式enable、信任边界审批、去内容日志 |
| 旧embedding冒充current | canonical currentness fence + source-basis CAS + future query二次验证 |
| disable期间source变化后旧head复活 | disable仍推进fence；re-enable前full reconcile、drain和verify |
| source写入被模型故障阻塞 | source事务只做轻量fence/job，encoder完全异步 |
| 漏掉某条source写路径 | canonical table trigger或统一tx helper + full reconciler + exhaustive tests |
| Meeting没有summary revision | typed composite basis + semantic snapshot digest，不新增业务revision |
| business status被误当删除 | source-native lifecycle matrix；done/closed/ended不自动ineligible |
| read replica返回旧head | 首版只用writer pool |
| 多模型维度导致索引混用 | Phase 0冻结物理隔离方案，Phase 2验证generation-safe distance query |
| migration建立ANN时锁大表 | 普通migration不建production ANN，后续operator concurrent workflow |
| ANN先召回再鉴权泄漏 | ANN access path延后到query / ACL设计，不照搬generic FTS |
| chunk数量制造静态重要性 | 首版不交付chunk；未来按source聚合并单独设计 |
| rebuild覆盖并发新写 | latest desired basis、invalidation epoch、final CAS |
| generation回滚到陈旧索引 | rollback-ready generation持续dual-maintenance，verify后才能回切 |
| stale / deleted向量无限保留 | retention policy、引用fence、operator purge和可观测GC |

## 15. 本计划完成定义

只有同时满足以下条件，才能把图语义化基础标记为完成：

1. pgvector在所有受支持PostgreSQL部署面有明确、已验证的前置合同；
2. semantic capability按Community默认关闭且可独立启停；
3. 三类canonical source均有verified typed adapter；
4. 业务终态source继续语义化，并提供与embedding分离的lifecycle / status metadata；
5. overview extractor确定、版本化且不使用fallback正文；
6. source变化能立即使旧head失去current资格；
7. worker能够异步生成、验证并原子激活完整unit set；
8. encoder故障不阻断canonical source mutation；
9. current / missing / building / stale / failed / ineligible可观察；
10. 全量backfill、从空重建、generation cutover和rollback均通过；
11. Edge没有summary或embedding，Context Document复用Document语义；
12. raw embedding没有进入普通Nostr、CLI、Desktop或日志；
13. Community隔离、writer currentness和并发CAS有自动化证据；
14. canonical source、Coordinate、EdgeKey和业务revision没有被模型生命周期改写；
15. 首版没有偷渡content chunks；
16. public semantic query仍未开放，后续设计可以明确依赖本基础合同。

## 16. 后续开发顺序

本计划完成后，按以下顺序继续：

1. **通用图语义查询规范**：定义 `problem`、可选 `initial_coordinates[]`、Agent environment、权限、
   candidate、coverage和provenance；
2. **召回与排序实现设计**：lexical / vector、source聚合、模型选择、ACL-aware access path；
3. **语义路径检索设计**：全局入口、Context Document映射、Hyperedge展开、budget、restart、retrieval
   forest；
4. **Agent使用设计**：语义结果与canonical reads、exact / incident / contains-all的组合；
5. **Content chunk扩展**：Document / Project View / Meeting各自的切片策略、原子unit-set和source级聚合。

这些后续阶段不得反向改变本计划已经守住的内容所有权、Coordinate identity、Hyperedge语义或embedding的
派生性质。
