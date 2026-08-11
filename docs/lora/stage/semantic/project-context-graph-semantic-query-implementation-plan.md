# Project Context 图语义检索分阶段实现计划

> 状态：Phase 0–5 代码已交付但保持 feature-off；Phase 6 的 ACP 使用合同已交付，外部资格验证尚未完成；
> 当前不构成生产 ready 声明
>
> 日期：2026-08-11
>
> 代码基线：`feat/context-semantic` @ `98122b89e`
>
> 实现说明：本文所列查询代码位于上述基线之后的当前交付树。所有 Community 的查询 gate 与部署 master
> 默认关闭；只有完成本文 §18.3 的资格验证和显式启用顺序后，才允许发送问题与 overview。
>
> 基础规范：
> [Project Context 图语义化基础规范](./project-context-graph-semantic-foundation-spec.md)
>
> 已交付基础：
> [Project Context 图语义化基础分阶段开发计划](./project-context-graph-semantic-foundation-implementation-plan.md)、
> [Project Context 图语义化基础资格报告](./project-context-graph-semantic-foundation-qualification.md)
>
> 图领域规范：
> [Project Context V2 领域规范](../project-context/project-context.md)
>
> 计划范围：查询问题编码、可选初始 Coordinate、可选上下文 Coordinate、全局语义入口召回、
> problem-conditioned 环境增益、Context Document 关系评分、有界 Hyperedge 路径搜索、
> retrieval forest、权限与 currentness、HTTP `/query`、Relay 签名结果、Carryforth 与 ACP 接入、
> exact 检索灰度和后续 ANN 退出门
>
> 明确排除：正文 chunk、Runtime 自由文本、Edge summary / embedding、Coordinate 自有 embedding、
> LLM / cross-encoder rerank、Desktop、历史 Revision 检索、NIP-50 改造、自动修改 canonical summary、
> 独立图数据库、首版生产 ANN

## 0. 当前交付状态

`98122b89e` 是图语义 Foundation 的已提交基线。当前交付树已经在该边界之上实现 HTTP-first 图语义查询，
但能力仍由默认关闭的 deployment master、Community query gate、短期 fleet attestation 和 readiness 共同
fail closed。下文的“代码完成”只表示实现与本地定向测试已落地，不表示真实 Provider、目标 PostgreSQL
负载或实际负载均衡 fleet 已取得生产资格。

| 能力 | 代码交付 | 启用 / 资格状态 |
|---|---|---|
| Canonical Source overview、2048 维 embedding、generation / current head | 已交付 | 仍按 Foundation 的 per-Community gate 管理 |
| closed query / result / score / request-binding / fleet 合同 | 已交付 | feature-off |
| Q0 与逐个 context-conditioned query encoder | 已交付 | Community query gate 关闭时零查询出域 |
| deadline-aware Provider admission 与 final egress authorization | 已交付 | 顺序为 reserve → wait → READ COMMITTED 下先取得 Community shared writer fence 的 final confirmation（含 fleet）→ direct send；真实 Volcengine relevance / floor 资格未完成 |
| 0057 历史零向量 active-head repair / worker recovery | 已交付 | `repair-query-vectors` 只作用于当前 active generation；启用前按 §18.3 验证闭集统计并等待 worker 恢复 |
| writer DB current-head exact recall、结构角色与 current hydration | 已交付 | 目标数据量 `EXPLAIN`、p95/p99、并发与 soak 未完成 |
| root selection、Hyperedge traversal、retrieval forest 与 deterministic packing | 已交付 | 真实场景路径质量资格未完成 |
| HTTP `/query` strict extension 与 Relay 签名 virtual result kind `40912` | 已交付 | 默认不广告；实际 LB fleet attestation / 灰度未完成 |
| virtual kind ingest/store/ordinary read/search/fan-out deny seam | 已交付 | migration/readiness 通过后才可启用 |
| SDK result verifier 与 Carryforth `project-context semantic-query` | 已交付 | 只消费已广告的 HTTP capability |
| ACP Project Space 使用合同 | 已交付 | 只指导 Agent 显式调用，不自动查询或注入 Runtime |
| low-cardinality、content-free 查询 observability | 已交付 | 目标环境 dashboard / alert 门槛待灰度校准 |
| WebSocket semantic query | 未实施 | Phase 7 deferred；不得广告 WS capability |
| HNSW / ANN 生产访问路径 | 未实施 | Phase 8 deferred；exact 不能满足 SLO 时才触发 |

阶段状态分成“代码交付”和“外部资格”两个维度：

| Phase | 代码交付状态 | 尚未关闭的资格 / 运营门 |
|---|---|---|
| 0 | 完成，feature-off | 最终合并树全量回归仍需重跑 |
| 1 | 完成，feature-off | 真实 Volcengine 中英文 relevance 与四类 floor 尚未冻结 |
| 2 | 完成，feature-off | 目标 PostgreSQL `EXPLAIN`、SLO、并发 / vacuum 影响与 soak 尚未通过 |
| 3 | 完成，feature-off | 同 problem / 不同 environment 的真实路径评测尚未通过 |
| 4 | 完成，feature-off | 代表性高出度图、并发和长时间运行资格尚未通过 |
| 5 | 完成，HTTP-only、feature-off | 实际 LB inventory 的 fleet attestation 与首个灰度 Community 尚未通过 |
| 6 | ACP 合同代码完成 | Provider / DB / 安全 / 质量 / 运维资格整体待执行，不能声明 production ready |
| 7 | deferred | WebSocket raw preservation、binding 与 fleet parity 另行交付 |
| 8 | deferred | 仅在 exact 的目标规模 SLO 不达标时启动 ANN |

## 1. 目标与总体决策

### 1.1 要解决的问题

调用者通常只有自然语言问题，不一定知道关联的 Work、Issue、Requirement、Document、Meeting 或现有
Edge。要求调用者先提供一个准确 Coordinate，会让图查询无法成为真正的入口能力。

本计划的目标是：

> 在不复制、不切割 Project Context Graph 的前提下，由查询引擎根据自然语言问题和可选的上下文环境，
> 以有限成本发现全图入口，并沿真实 Hyperedge 形成一组当前、可解释、可继续验证的上下文路径。

### 1.2 总体执行链

```text
problem
  │
  ├── Q0: problem-only query vector
  │
  └── Qi: problem + one context Coordinate current overview
          （每个 context Coordinate 一个独立通道）
                    │
                    ▼
current graph semantic sources
├── Coordinate source candidates
└── active Context Document candidates
                    │
                    ▼
problem-dominant + bounded environment-gain ranking
                    │
                    ▼
roots
├── explicit initial Coordinate roots
├── problem-neutral semantic roots
└── environment-conditioned semantic roots
                    │
                    ▼
bounded Hyperedge traversal
Coordinate → (Edge, Context Document) → full Coordinate set → next Coordinate
                    │
                    ▼
SemanticRetrievalForest + coverage + provenance
```

### 1.3 核心实现决策

1. 核心查询是通用能力，不要求 Role，也不内建 Meeting 语义；
2. `problem` 必填，`initial_coordinates[]` 和 `context_coordinates[]` 均可选；
3. `initial_coordinates[]` 中通过 current graph + canonical source 验证的项是显式 root，不是全局召回范围；
4. `context_coordinates[]` 是软语义环境，不自动成为 root、ACL 或子图过滤器；
5. 每个上下文 Coordinate 与 `problem` 组成一个独立 query channel，不把所有环境拼成一段文本；
6. problem-only 相关性始终主导，环境只提供有界边际增益；
7. Coordinate 语义继续来自来源 overview，Edge 继续没有 summary 或 embedding；
8. Context Document 逐份提供关系语义，查询项是 `(edge_key, context_document_id)`；
9. Hyperedge 永远返回完整精确 Coordinate set，不拆成隐含二元边；
10. 首版只用 current overview 与 full-precision exact cosine scan；
11. 一次查询返回有界 retrieval forest 和诚实 coverage，不先引入分页；
12. Agent 是该查询的上层消费者，普通结构查询仍保持开放；
13. 不设计 V1/V2 并行 wire，Request/Result 不携带 `schema_version`；后续改动直接更新同一
    closed schema 与全部消费者。Contract digest 只用于完整性/可比较性 fence，不形成另一套版本
    dispatch。

## 2. 必须保持的领域边界

### 2.1 不创建新的图语义所有者

查询层不得创建：

- `CoordinateNode.summary`；
- Node embedding；
- Edge summary；
- Edge embedding；
- Role Context；
- Agent 私有图；
- 自动生成的二元关系；
- 持久化 retrieval path 作为新的项目事实。

来源对象仍是 title、summary 与正文的唯一 canonical owner。Foundation embedding 仍是可删除、可重建的
派生索引。

### 2.2 两类语义对象

查询层只消费两类语义对象：

```text
Coordinate candidate
└── Coordinate 指向的 Canonical Source overview

Relation candidate
└── active Edge 绑定的某一份 Project Document overview
```

同一个 Project Document 如果同时是 Document Coordinate 和 Context Document，只计算一次来源语义分数，
再映射成两个结构角色。不得因为角色重复而增加分数。

### 2.3 图外来源

Foundation 可以索引 Community 中尚未进入 Project Context Graph 的来源对象，但本计划实现的是“图语义
检索”，不是全 Project 来源搜索。因此：

- semantic-discovered result 只来自当前 active Coordinate 并集或 active Context Document binding；
- graph-external source 不作为结果 root 或 standalone result；
- graph-external `context_coordinate` 可以作为 query lens；
- graph-external `initial_coordinate` 返回 `not_in_graph` observation，不伪造 incident path；
- 未来普通 Project semantic search 另行设计。

## 3. 通用查询合同

### 3.1 Request DTO

```text
SemanticGraphQuery
├── request_id: UUID
├── project_id: UUID
├── problem: String
├── initial_coordinates: ProjectContextCoordinate[]
├── context_coordinates: ProjectContextCoordinate[]
├── lifecycle_filter
│   ├── all_current
│   ├── non_terminal
│   └── terminal_only
└── budget: SemanticGraphQueryBudget
```

字段语义：

- `project_id` 必须等于 host-derived Community 当前 Project View identity；
- `problem` 是唯一主查询；
- `initial_coordinates` 是显式遍历起点；
- `context_coordinates` 是相关性 lens；
- 同一个 Coordinate 可以同时出现在两组中，保留两个语义角色；
- 两组内部 canonical dedup 并按 Coordinate canonical order 排序；
- 输入排列不得改变最终查询向量集合、评分或路径；
- 客户端不能提交权重、threshold、provider、model、generation 或 raw vector。

Role、Work、Issue、Requirement、Document、Meeting 都只是 typed Coordinate。核心查询不要求当前 Role，
也不验证“这个 Role 是否属于某一个 Agent”。首版 ACP 只用稳定合同指导 Agent 将
verified current Role / Work 等选为普通 `context_coordinates`，不由 Harness 自动映射或调用。

Lifecycle selector 不删除调用者显式给出的 current/eligible `context_coordinates`；lens 反映查询
环境，而不是 result filter。其实际 lifecycle 仍记入 input observation。

### 3.2 输入边界

首版冻结以下服务端硬边界：

```text
MAX_QUERY_REQUEST_BYTES       = 64 KiB
MAX_PROBLEM_BYTES             = 16 KiB UTF-8
MAX_INITIAL_COORDINATES       = 16
MAX_CONTEXT_COORDINATES       = 8
MAX_QUERY_CHANNELS            = 9  // Q0 + 8 个 context channels
```

边界只用于请求资源控制，不反向限制 canonical summary 长度。所有文本拒绝 NUL；`problem.trim()` 必须非空。
未知字段、重复 `request_id` 形状错误、越界 UUID、未知 Coordinate subtype 和跨 Project identity 都 fail
closed。

Coordinate 数组的 hard count 在 canonical dedup **之前**应用；因此 9 个重复 context entries 仍是超界
请求，不能用 dedup 绕过 parse/memory 边界。通过 raw count 后才对 identity 去重用于评分。

首版不接受：

- `runtime_hints`；
- 完整 Agent Runtime Context；
- caller-provided embedding；
- caller-provided prompt template；
- caller-provided scoring weights；
- 正文或 chunk 文本。

### 3.3 Lifecycle selector

闭集定义为：

```text
all_current  = active + finalizing + terminal
non_terminal = active + finalizing
terminal_only = terminal
```

默认值是 `all_current`。Work `completed`、Issue `closed`、Requirement `satisfied`、Meeting `ended` 等业务
终态仍可参与查询。

Lifecycle selector 只筛选 **Coordinate structural role**。Active Context Document binding 始终可作为关系材料，
否则 `terminal_only` 会把解释终态 Work/Issue 关系的普通 Document 全部删掉。同一 Document
同时是 Coordinate 和 Context Document 时，它的 Coordinate 入口受 selector 约束，Context Document
入口仍可用。对 Edge 内 target Coordinates 继续应用 selector。

Explicit initial Coordinate 是另一个明确例外：只要它是 current graph member、来源当前可读且非
deleted/tombstone，就作为显式 root，不因 lifecycle selector 被静默丢弃。Selector 仍约束其自动候选名额
和后继 target Coordinates。Result observation 必须显示该 initial 的实际 lifecycle。

`tombstone` / `deleted` 先被 Foundation `eligibility` 排除，不能通过 lifecycle selector 重新进入。

## 4. Query Encoder 与外部出域

### 4.1 已确认的出域合同

对于显式启用图语义查询的 Community，已确认允许向火山引擎发送：

1. 用户本次提交的 `problem`；
2. context Coordinate 当前 overview 中已经获准出域的 type、title / name、optional summary；
3. 固定 query template 所需的静态标签。

首版仍禁止发送：

- Project View / Document / Meeting 正文；
- future content chunks；
- Agent 完整 Runtime Context；
- 自由文本 runtime hints；
- Context Document 正文；
- ACL、secret、auth tag 或私钥材料。

正文与 chunk 出域必须届时另行确认，不能由本计划推导授权。

### 4.2 独立 Query gate

新增 Community 字段：

```text
semantic_graph_query_enabled BOOLEAN NOT NULL DEFAULT FALSE
```

约束：

```sql
CHECK (NOT semantic_graph_query_enabled OR semantic_index_enabled)
```

两个 gate 语义不同：

- `semantic_index_enabled`：允许维护来源 overview embedding；
- `semantic_graph_query_enabled`：额外允许发送本次 `problem` 和 conditioned query text。

关闭 query gate：

- transaction 提交后立即停止公开 query，并停止签发新的 egress permit；等待中的 Provider slot reservation
  只是容量，不是授权，随后 confirmation 必须失败且零出域；
- 已经先完成 egress linearization 的单个 Provider batch 可以结束，但其结果必须在 Stage D 被丢弃；
- 不停止 Foundation worker；
- 不删除 source embeddings；
- 不影响 Project Context 普通结构读写；
- 不改变任何业务 revision。

现有 `Db::set_semantic_community_enabled(false)` 不能在 query gate 仍为 `true` 时只更新 index gate，否则会
撞上新 CHECK。0058 上线后冻结为：

```text
disable semantic index
→ 在同一个 Community row-lock transaction 中先置 query=false
→ 再置 index=false
```

直接 SQL 或旧 Admin 路径若只尝试关闭 index，DB CHECK 必须拒绝。Migration tests 要从 populated 0057 DB
覆盖 query on → index disable 的原子关闭与 rollback。

### 4.3 Query input 不是 Semantic Unit

不得伪造一个 `SemanticUnitIdentity` 存放问题。新增独立纯类型：

```text
SemanticQueryEncoderInput
├── request_id
├── channel_id
├── channel_kind
│   ├── problem
│   └── conditioned_context
├── query_contract_digest
├── text_digest
└── text
```

```text
EncodedSemanticQuery
├── request_id
├── channel_id
├── source_generation_contract_digest
├── embedding_space_fence
├── query_contract_digest
├── response_model
└── validated query vector
```

Foundation 现有 `SemanticModelContract.digest()` 包含 `input_contract_version = overview-v1`；query input 使用
另一套 template，因此不能把 source generation 的完整 `model_contract_digest` 冒充为 query
contract。本次拆成三个独立 fence：

```text
source_generation_contract_digest
  = Foundation generation 的完整 SemanticModelContract digest

embedding_space_fence
  = digest(provider, resolved_model, dimensions, distance_metric,
           normalization, provider_boundary)

query_contract_digest
  = digest(canonical serializer,
           problem template, conditioned-context template,
           Provider input limits)
```

Query capability 维护 closed compatibility allowlist：

```text
(source_generation_contract_digest,
 embedding_space_fence,
 query_contract_digest)
```

只有 allowlist 中的组合才可调用 Provider。Query vector 与 source vector 的可比较性由
`embedding_space_fence` 证明，不是由两者的 input contract 完全相同证明。
Allowlist 是 `buzz-semantic-query` 内的 closed code contract，不是 request 字段或 operator 可自由
拼接的 DB 字符串。扩展 allowlist 必须带新 query contract digest 与可比较性评测。

Query vector：

- 只存在于请求内存和短事务绑定参数中；
- 不写入 `semantic_units`、`semantic_embeddings`、Event store、FTS 或审计正文；
- 不返回客户端；
- 不记录原文或内容派生 digest 到普通日志 / metrics；
- 必须匹配 active generation 的 embedding-space fence；
- 必须同时携带并验证 source generation、embedding space 与 query contract 三个 fence；
- 必须 finite 且 cosine norm 非零。

### 4.4 固定 query templates

本计划中的“查询通道”不是网络连接、Project Channel 或 Graph Edge，而是一条独立的
**查询向量分支**：它拥有自己的 Provider input、query vector、exact recall 结果和候选相似度。
Q0 与每个 Qi 分别计算，之后才在评分层合并；任何一条分支都不会持久化为项目对象。下文优先使用
“查询向量分支”，保留 `query_channels_*` 仅作为既有 DTO / metric 字段名。

Problem-only 查询向量分支：

```text
Q0 = encode(
  semantic-graph-query.problem,
  problem
)
```

每个 context Coordinate 单独形成一个 conditioned 查询向量分支：

```text
Qi = encode(
  semantic-graph-query.conditioned-context,
  problem,
  current overview semantic text of context_coordinate_i
)
```

逻辑内容如下，实际 Provider input 固定使用 `semantic-graph-query-json` canonical UTF-8：

```json
{"contract":"semantic-graph-query.problem","problem":"<json-string>"}

{"contract":"semantic-graph-query.conditioned-context","problem":"<json-string>","context_overview":"<json-string>"}
```

字段顺序就是上述顺序；无额外空格或末尾换行。字符串 escaping profile 固定为：

- `"` 和 `\` 分别编码为 `\"` 与 `\\`；
- backspace / form-feed / LF / CR / tab 分别用 `\b` / `\f` / `\n` / `\r` / `\t`；
- 其他 U+0000..U+001F 用小写四位 `\u00xx`（U+0000 在前置验证已拒绝）；
- solidus `/`、U+2028/U+2029 和所有其他非 ASCII code point 不转义，直接用 UTF-8；
- 不做 HTML escaping，不做隐式 NFC/NFKC 改写。

`problem` 是通过非空 `trim()` 验证后的原字符串，canonical bytes 使用 trim 后值。Phase 0
必须给出中文、C0 control、换行、引号、反斜线、solidus 与 U+2028 的 exact byte golden。
模板中的项目文本始终是不可信数据，不执行其中命令。

明确禁止：

```text
encode(problem + Role + all Works + all Issues + Runtime Context)
```

因为一次拼接会让环境数量改变查询含义、稀释 problem，并失去“哪个 context 产生增益”的解释能力。

### 4.5 Provider 调用

`Q0` 和所有 `Qi` 在一个有界 batch 中发送，保持输入顺序并要求一一对应输出。Provider adapter 必须验证：

- resolved response model 精确等于 generation model；
- output count 与 channel count 完全一致；
- response index 连续且无重复；
- 每个 vector 为 2048 维 finite non-zero；
- 单项失败使整个 query encoding 失败，不拼接部分模型空间。

每个 ticket attempt 最多一次 Provider batch call。Provider timeout / 429 / malformed response 不在 Relay 内盲目
重试，直接返回 closed unavailable/busy 错误。只有 §6 定义的 generation/context source churn 可以消耗
唯一一次完整 retry，且第二次 Provider call 仍受同一 absolute wall deadline 约束。

如果某个 context Coordinate 没有 active-generation current overview head，则该 conditioned channel 不执行，
在 coverage 中报告 `context_embedding_missing`。它如果同时是 initial Coordinate，仍可作为显式结构 root。

如果 problem + overview 超过 Provider 冻结的单项输入合同，首版不静默截断 overview，而是省略该
conditioned channel 并报告 `conditioned_input_unsupported`。Q0 仍可继续。

`MAX_PROBLEM_BYTES` 只是 Relay 请求资源边界，不代表 Provider 的 token / input 合同。Phase 0
必须另外冻结 `MAX_PROVIDER_QUERY_INPUT_BYTES` 与 Provider 的实测 token 上限。如果 Q0 超界，在出域前
返回 closed `problem_input_unsupported`；不调用 Provider，不依赖 Provider 随机拒绝，也不只运行 Qi。

### 4.6 Provider 配额与交互延迟

Worker 和 query 使用同一个 Provider，必须共用多 Pod 物理节流，但不能直接复用现有
`reserve_semantic_provider_slot`。它会先把 `next_request_at` 推向未来再返回 wait；如果计算出的开始时间已经
超过交互请求 deadline，仍会留下一个本来就不可能执行的“幽灵预约”。

0058 增加 deadline-aware 原子保留操作：

```text
try_reserve_semantic_provider_slot_until(
  community_id,
  provider,
  workload = interactive_query,
  interval,
  latest_start_at
)

row-lock physical provider gate
→ calculate candidate scheduled_at
→ if candidate scheduled_at > latest_start_at:
     return busy WITHOUT changing next_request_at or admission state
→ otherwise reserve exactly one batch slot and commit
```

Provider slot reservation **只是物理速率容量，不是出域授权**。它可以在同一短 transaction 中做一次早期
principal / gate / generation / context-head recheck 以减少无谓预约，但该观察不能跨越随后发生的 wait。
实际顺序冻结为：

```text
reserve one Provider slot（capacity only）
→ commit reservation
→ wait until scheduled_at
→ one short READ COMMITTED writer transaction first acquires the shared Community writer fence
→ after that lock wait, revalidate the short-lived HTTP fleet assertion with DB clock plus full
  principal / query gate / generation / graph revision / every conditioned context exact current head
→ commit one single-use egress permit
→ without any other await, hand off directly to the one Provider batch send
```

最后的短 writer transaction 才是 egress authorization linearization point。保留成功后，即使 caller 取消、
fleet assertion 过期 / 撤销，或最终授权重验失败，该 slot 也视为已消耗；不做事后“退还”，避免多 Pod
双重使用。这是可接受的有界容量浪费，但必须零出域。只有 `candidate scheduled_at > latest_start_at` 的
`busy` 路径回滚且零写入，不得把“最终未发请求”一概误报为 phantom reservation。

这里不能用“先开始普通 `REPEATABLE READ` transaction，再等待 advisory lock”替代。PostgreSQL 可能在执行
lock statement 时已经固定 RR snapshot；若排他 writer 正在持锁，reader 等它提交后仍可能从旧 snapshot
读取撤权前状态。最终确认固定为 `READ COMMITTED`，先等待与 canonical Community auth writers 相同 key 的
shared transaction advisory lock；该 lock statement 的 snapshot 不复用，后续检查才形成新 statement
snapshot。`communities`、generation、Context state、已存在的 context source 与 fleet assertion 再按固定顺序
取得 `FOR SHARE`，使已经观察的 query/index/currentness/fleet 状态稳定到 confirmation commit。

Query 另外增加：

- per-process bounded concurrency semaphore；
- per-principal / per-Community admission；
- interactive deadline；
- batch-only provider request；
- deadline 内拿不到 provider slot 时返回 `query_provider_busy`，不形成无界队列。

0058 还要给 interactive query 增加 Community/provider 作用域的独立 token bucket（或等价的持久化
workload lane），冻结 query 可消耗的最大 Provider 份额，为 worker / rebuild 保留最小进度。当前
`semantic_provider_rate_gates` 仍是最终物理节流门，workload lane 只做公平 admission，不允许任一方绕过
它。Phase 1 必须同时把 worker 从可以预约远期 slot 的旧 API 迁到同一 workload-aware scheduler；
不允许只改 query 一侧却让多个 background Pod 持续占住未来队列。Phase 1 必须用多 Pod 并发测试证明：
`busy` 不推进 gate；取消不重用 slot；query
与 worker 都有有界前进，不会互相永久饥饿。

## 5. Candidate 集合与结构角色

### 5.1 Current candidate 的完整资格

一个 overview 只有同时满足以下条件才可参与召回：

```text
Community query/index gate enabled
AND active generation pointer matches query ticket
AND generation lifecycle = active
AND source eligibility = eligible
AND source epoch + snapshot digest = current head fence
AND unit set state = active
AND unit kind = overview
AND unit-set extractor = generation extractor
AND embedding generation/source-contract/embedding-space/dimensions match
AND role-specific structural-entrypoint mask is non-empty
AND current source-family capability/readiness is available
AND source has a current graph structural role
```

Mask 的精确规则见 §8.2；source row 通过某一角色获得查询资格，不会让其他角色自动获得
traversal 资格。

首版没有一套新的 per-source ACL。当前权限边界是 Community-global current principal +
Project Context read decision，再加 Project View / Document / Meeting 各来源的当前 capability/readiness。
如未来出现 per-source ACL，它必须在 distance 前 push down；本计划不伪造当前不存在的授权模型。

不得仅依据：

- `semantic_sources.coverage_state = current`；
- `semantic_embeddings` 中存在一行；
- 旧 generation embedding；
- stale unit set；
- Project Context 中仍保留 Coordinate identity。

### 5.2 Graph structural roles

查询时从 canonical graph 动态映射：

```text
SourceStructuralRole
├── coordinate
│   └── source identity appears in an active Edge Coordinate set
└── context_document
    └── Project Document has an active Context Document binding
```

Candidate source 至少有一个角色才进入图语义召回。Document 可以同时拥有两个角色，但语义距离只计算一次。

`context_document` 角色还携带当前：

```text
(document_id, edge_key, binding revision)
```

现行领域约束是一份 Context Document 最多属于一条 active Edge。本计划不改变该基数；若以后领域规则放宽，
Query DTO 已用数组 / identity 表达，不能把单 Edge 假设写进 source embedding。

### 5.3 Exact recall 流程

首版每个查询向量分支都执行 current authorized candidate 集合上的 full-vector exact cosine scan：

```text
cosine_distance(q, source) = source.embedding <=> q.vector
cosine_similarity = 1 - cosine_distance
```

这里的 `full-vector` 指 query 与 source 都使用完整的 2048 维 `vector`，不是读取完整正文；`exact`
指先完成 Community、权限、current head、lifecycle 与 graph role 过滤，再对过滤后的每个候选逐一计算
余弦距离并排序取 Top-K。首版不使用 HNSW/IVFFlat 近似候选，也不使用 `halfvec` 降精度。

流程：

1. 为每个 channel 取有界 top-K source identity；
2. 对所有 channel 的 top-K 做 source identity union；
3. 加入显式 initial Coordinate 对应来源；
4. 对 union 中每个 source 计算 Q0 与每个 Qi 的完整 full-vector score matrix；
5. 在纯算法层计算 EnvironmentGain、root score 与 path score；
6. 通过 canonical source adapter 水合 title / summary / lifecycle，不返回 `semantic_units.semantic_text`。

这样 conditioned channel 可以引入 problem-only top-K 之外的候选，同时每个候选仍有 Q0 基础分，能够应用
problem relevance floor。

上述 top-K union **只用于选 roots**，不是 traversal 白名单。Stage C 每遇到一个新的 current /
eligible adjacent Context Document 或 Coordinate，都必须按需对 Q0 + 已执行 Qi 计算完整 score
matrix，并在当前 transaction 内按 `(source identity, source epoch, snapshot digest)` 缓存标量结果。
这些候选不需要曾出现在 root recall top-K 中。

### 5.4 首版不引入 FTS 融合

Foundation 当前没有 semantic text GIN 访问路径。本计划首版只交付 vector exact recall，避免同时引入中文
分词、lexical/vector 量纲融合和第二套质量变量。

稳定 ID、title exact match 或未来 lexical / RRF 如需加入，直接修改当前 closed ranking contract、更新
digest 并协调更新消费者；不保留并行 V1/V2 dispatch，也不得静默改变 `semantic-graph-score`。

## 6. Currentness 与数据库快照

### 6.1 两段式查询

Provider 网络调用不得占用长数据库事务。一次 query 分成：

```text
阶段 A：Query ticket
├── full Semantic Graph composite read decision
├── query/index gate
├── active generation + source/space/query contract fences
├── current context Coordinate heads
└── fixed query inputs

阶段 B：Encode（事务外）
├── reserve one Provider slot（capacity only）
├── wait until scheduled_at
├── one short READ COMMITTED writer transaction first acquires the shared Community writer fence
├── after the lock wait, recheck DB-clock fleet plus principal/query/generation/graph/context heads
├── acquire one single-use egress permit at commit
├── no other await before Provider handoff
└── Q0 + Qi one bounded batch

阶段 C：Writer DB REPEATABLE READ READ ONLY
├── DB 内再次执行 full Semantic Graph composite read decision
├── recheck query/index gate + all three contract fences
├── recheck context source epoch/snapshot
├── exact recall
├── structural role mapping
├── Hyperedge traversal
├── canonical title/summary hydration
└── result observation

阶段 D：Result release confirmation
├── READ COMMITTED 下先取得同一 shared Community writer fence
├── 同一短 transaction 重验 composite read decision + query/index gate + DB-clock fleet
├── commit one single-use result-release permit
└── permit 后不再 await，直接同步签名 virtual Event
```

`SemanticGraphReadDecision` 不是新 ACL，而是一个 fail-closed composite helper。它每次都验证 current
credential、principal / ban / managed-owner、Project identity、Project Context structural read，以及
Project View、Project Document、Meeting 三个 canonical source family 的当前 read/capability readiness。
不能因为当前 `project_context_structural_read_ready` 恰好检查了某些 prerequisite，就在 query 中隐式依赖
该实现细节；composite helper 与测试必须显式锁定完整集合。这样 graph-external context lens、Context
Document 和返回的不同 Coordinate family 都经过同一读权限合同，Edge / Binding 永远不替代来源权限。

Stage B 先原子保留一个满足 `latest_start_at` 的 Provider slot 并在事务外等待。reservation 可以带早期安全
重验以避免明显无效请求占用容量，但它不是授权。wait 完成后，用一个短 READ COMMITTED writer-DB
transaction 先取得 shared Community writer fence；只有 lock wait 返回后，才读取 DB-clock fleet assertion、
composite read decision、current principal、query/index gate、三个 contract fence、graph revision，以及每个
conditioned context source 的 exact epoch、snapshot digest、eligibility、source-family readiness 与 active
overview head。Community row、generation、Context state、已存在的 source row 与 fleet row 的 shared locks
使这些分步观察等价于一个稳定的 final confirmation 区间。任一 context 已更新、清空 summary、删除、
tombstone 或失权时，都不得发送 Stage A 缓存的旧 overview；按 §6.3 消耗共享的一次 retry，或在合同允许时
省略该 channel 并诚实报告 coverage。

最终短 transaction 的成功 commit 是一次 Provider batch 的 **egress authorization linearization point**，并
签发只可消费一次、只覆盖当前 request / generation / context-head set 的内存 permit。撤权或 query-disable
若先于该观察点提交，本次 Provider 调用必须为零，即使此前已经预约并等待过 slot；该 slot 可以浪费但不能
复用。若 permit 已先线性化，则它属于已经授权的 in-flight batch，之后提交的撤权不能宣称能撤回已经发生的
出域。Disable transaction 提交后不得再签发新 permit，Stage D 也必须阻止任何已在途结果返回。实现和测试
必须按这个数据库先后关系表述，不再使用无法实现的“只要网络调用尚未开始就能无条件追回”的 wall-clock
承诺。fleet check 是最终 confirmation transaction 的一部分；所有其他 waitable readiness 工作必须发生在
该 transaction **之前**。permit commit 与 Provider handoff 之间不得再执行任何其他 await，permit 不得复用、
排队或跨 retry 使用。

Stage D 重新执行同一个 composite helper，而不只是粗粒度 Context gate，并在同一 short final confirmation
中锁定 DB-clock fleet assertion。任何 current principal / ban、任一 source-family read/capability readiness、
query/index gate 或 fleet 失败，都丢弃已计算结果，不签名 virtual Event。Stage D 成功 commit 签发一个不可
复制的 result-release permit；permit 到签名之间不得出现任何 await。若撤权 writer 先取得排他 Community lock
并提交，Stage D 必须在共享 lock wait 后看到撤权并拒绝；若 release permit 先 commit，则随后撤权线性化在其后，
该次同步签名属于已授权的 in-flight release。

首版对“current”的精确合同是：路径与 source/head 事实是 **as-of Stage C repeatable-read
snapshot**，不宣称它们在 HTTP response 到达时仍是最新。结果携带 snapshot observation 与每个
source basis；调用者在使用事实前仍须执行 canonical read。Stage D 只是安全 postflight，不把一次
已完成的 snapshot 重写成另一个数据时点。

### 6.2 Generation 冲突

如果 encode 前后的 active generation、source generation contract、embedding-space fence 或 query
contract 变化：

- 丢弃所有 query vectors；
- 最多重新读取 ticket、重新 encode 一次；
- 第二次仍变化则返回 `semantic_generation_changed`；
- 绝不能用旧 query vector 搜索新 generation；
- 绝不能混合两个 generation 的 source scores。

### 6.3 Context Coordinate 冲突

如果 conditioned query 使用的 context Coordinate source epoch / snapshot digest 在 encode 后变化：

- 丢弃该请求的全部 conditioned query vectors；
- 与 generation 冲突共享一次完整 retry 预算；
- 不只替换 revision 后重放旧向量；
- 第二次仍 churn 则返回 `context_source_changed`。

同一检查也必须发生在 encode **之前**：Stage A ticket 与 Stage B egress permit 之间的 source churn 不得先把
旧文本发出、再依赖 Stage C 补救。Stage C 检查负责保证 query vector 与检索 snapshot 一致，Stage B 检查
负责保证出域文本在 egress linearization point 仍是可发送的 current source；两者不能互相替代。

### 6.4 Graph observation

Recall 与 traversal 在同一个 writer DB repeatable-read snapshot 中读取：

- `project_context_edge_state.context_revision`；
- active `project_context_edges`；
- normalized `project_context_edge_coordinates`；
- active `project_context_document_bindings`。

结果返回该 snapshot 的 `context_revision` 和 transaction observation time。查询不推进 Context
revision，也不创建新的 graph snapshot。

### 6.5 Source provenance

每个返回 source 至少携带：

```text
source identity
source lifecycle / status
typed source basis
source invalidation epoch
source snapshot digest
summary coverage
semantic generation
source generation contract digest
embedding-space fence
query contract digest
```

Meeting 不伪造统一整数 revision，继续使用 Foundation typed basis。

## 7. 确定性评分合同

### 7.1 Fixed-point Score

纯算法层不累计裸 `f32`。定义：

```text
SCORE_SCALE = 1_000_000
Score ∈ [0, SCORE_SCALE]
```

DB cosine distance 在入口转换成 fixed-point；乘法使用足够宽的整数中间值；所有除法采用统一 half-up rounding。

```text
round_div(n, d) = (n + floor(d / 2)) / d

mul_score(a, b) = round_div(a × b, SCORE_SCALE)

weighted_score([(weight_i, score_i)])
  = min(SCORE_SCALE, Σ mul_score(weight_i, score_i))

harmonic_score(a, b)
  = 0                                      if a = 0 or b = 0
  = round_div(2 × a × b, a + b)       otherwise
```

所有小数权重在实现中都是 `Score` 整数常量：

```text
W_CANDIDATE_PROBLEM       = 750_000
W_CANDIDATE_ENVIRONMENT   = 200_000
W_CANDIDATE_ANCHOR        =  50_000
W_DOCUMENT_PROBLEM        = 700_000
W_DOCUMENT_ENVIRONMENT    = 200_000
W_DOCUMENT_COHERENCE      = 100_000
W_SECOND_ENVIRONMENT      = 250_000
W_ROOT_RELEVANCE          = 850_000
W_ROOT_DIVERSITY          = 150_000
DISCOUNT_FACTOR           = 850_000
```

Context kind 权重同样固定为 `1_000_000 / 900_000 / 600_000 / 500_000`。除下文
`DocumentScoreWithoutLocal` 明确定义的有理数特例外，各公式都先对单项乘法做
`mul_score`，再求和；不允许实现自由选择“先合并再 round”。

pgvector 返回的 cosine distance 先由 DB 边界量化，这一步是 SQL 与 Rust 的共同权威形状：

```text
distance 必须 finite，否则该 candidate fail closed
similarity = clamp(1.0 - distance, -1.0, 1.0)
normalized = (similarity + 1.0) / 2.0
Score = floor(normalized × SCORE_SCALE + 0.5)
```

SQL 用 `double precision` 执行上述 clamp 和 `floor(... + 0.5)` 后返回 `bigint`；Rust 算法层
只接收已验证的整数 `Score`，不对同一 DB distance 再做第二次浮点量化。纯 Rust golden
使用固定 `distance` bit pattern 验证同一表达式。

给定完全相同的 validated query vector bytes、Stage C snapshot、ranking contract 和 budget，必须
产生 byte-stable score 与排序。不宣称两次外部 Provider 重新 encode 会返回 byte-identical
vector。Tie-break 固定使用 canonical source identity、EdgeKey、Document ID、Coordinate canonical
order。

`ranking_contract_digest` 以 domain `buzz.semantic-graph-ranking` 绑定当前公式、整数舍入、全部权重/
floor、root 配额/MMR、beam/fair-first-wave 和 tie-break；`budget_profile_digest` 以 domain
`buzz.semantic-graph-budget` 绑定 defaults、hard caps、counter 扣减时点与 response packing。两者是当前
contract 的完整性摘要，不是 wire 版本号或 dispatch key。

### 7.2 Normalized semantic score

`semantic-graph-score` 使用同一模型空间内的 cosine similarity：

```text
raw_similarity(q, X) = 1 - cosine_distance(q, X)

normalized_similarity(q, X)
  = clamp((raw_similarity + 1) / 2, 0, 1)
```

转换成 `Score` 后参与后续公式。Phase 0 必须用真实模型评测集冻结：

- `BASE_ENTRY_FLOOR`；
- `RELATION_FLOOR`；
- `TARGET_FLOOR`；
- `TRANSITION_FLOOR`；
- query overfetch 倍数。

这些阈值属于 ranking contract，不允许客户端提交。没有通过 absolute problem floor 的 semantic candidate，即使
conditioned rank 很高也不能成为自动 root；显式 initial root 是唯一例外。

### 7.3 ProblemScore 与 conditioned score

对候选来源 `X`：

```text
ProblemScore(X) = normalized_similarity(Q0, X)

ConditionedScore(context_i, X)
  = normalized_similarity(Qi, X)

RawGain(context_i, X)
  = max(0, ConditionedScore(context_i, X) - ProblemScore(X))
```

这一步只计算“加入某个环境后，相对于同一 problem 基线增加了多少相关性”，避免把 problem 本身的相关性
重复计入 EnvironmentGain。

### 7.4 Context kind 权重

`semantic-graph-score` context 内部权重：

```text
Project Work          1.00
Project Issue         0.90
Project Requirement   0.90
Project Role          0.60
其他 Coordinate       0.50
```

```text
WeightedGain(context_i, X)
  = RawGain(context_i, X) × ContextKindWeight(context_i)
```

这些权重只决定环境增益内部的相对强度。它们不代表权限、项目重要性或工作优先级。

### 7.5 `second_highest_gain`

对同一个候选 `X`：

1. 按 canonical context Coordinate identity 去重；
2. 同一 evidence 只保留最高 WeightedGain；
3. 按 `(gain DESC, coordinate canonical order ASC)` 排序；
4. `highest_gain` 是第一项，不存在时为 `0`；
5. `second_highest_gain` 是第二项，不存在时为 `0`。

它不是“第二名候选”，而是同一个候选的第二强独立环境证据。两个不同 Coordinate 即使 gain 数值相同，仍可
分别成为 first 与 second；同一 Coordinate 重复输入不能制造 second evidence。

### 7.6 `saturate`

首版按已确认的简单闭集定义：

```text
saturate(x) = clamp(x, 0, 1)

EnvironmentGain(X)
  = saturate(
      highest_gain(X)
      + 0.25 × second_highest_gain(X)
    )
```

第三及以后环境 evidence 不再增加分数，但保留在 explanation 中。这样：

- 最强环境主导；
- 第二强环境只提供有限支持；
- 环境数量不能无限推高候选；
- 增加更多 Work evidence 不会线性放大环境权重。

Golden example：

```text
WorkGain  = 0.30
IssueGain = 0.18
RoleGain  = 0.05

EnvironmentGain
  = clamp(0.30 + 0.25 × 0.18, 0, 1)
  = 0.345
```

### 7.7 入口分数

```text
CandidateScore(X) =
    0.75 × ProblemScore(X)
  + 0.20 × EnvironmentGain(X)
  + 0.05 × AnchorGain(X)
```

Anchor 只允许：

```text
1.00  X 就是某个 explicit initial Coordinate
0.50  X 与某个 initial Coordinate 同属一条 current exact Hyperedge
0.00  其他情况
```

`initial_coordinates` 无论分数都作为显式 root；AnchorGain 只影响其他 semantic candidates 的有限排序，不把
搜索限制在初始分量。

固定不变量：

```text
ProblemWeight > EnvironmentWeight + AnchorWeight
```

### 7.8 Relation Document 与目标 Coordinate 分数

从 Coordinate `U` 查看 incident Edge 上某一份 Context Document `D`：

```text
LocalPathCoherence(D, U)
  = normalized_similarity(D.current_embedding, U.current_embedding)

DocumentScore(D | U) =
    0.70 × ProblemScore(D)
  + 0.20 × EnvironmentGain(D)
  + 0.10 × LocalPathCoherence(D, U)
```

如果 `U` 是没有 current embedding 的 explicit initial root，`LocalPathCoherence` 为不可观测，
不是零。此时按已知分量重新归一化：

```text
DocumentScoreWithoutLocal(D) =
    round_div(
      7 × ProblemScore(D) + 2 × EnvironmentGain(D),
      9
    )
```

这是评分合同中唯一的有理数重归一特例，只在分子求和后执行一次 `round_div`，
不先把 `7/9` 与 `2/9` 近似成两个 `Score` 权重。这只用于没有 source embedding 的显式
结构起点；自动 semantic root 不能绕过 embedding 资格。

对同一 Edge 中可能继续到达的 Coordinate `V`：

```text
RelationDocumentCoherence(D, V)
  = normalized_similarity(D.current_embedding, V.current_embedding)

TargetCoordinateScore(V | D) =
    0.70 × ProblemScore(V)
  + 0.20 × EnvironmentGain(V)
  + 0.10 × RelationDocumentCoherence(D, V)
```

对于直接由全局 relation-document hit 形成的 root，不需要伪造 entered-from Coordinate：

```text
RelationRootScore(D) = CandidateScore(D)

RelationRootTransitionScore(D, V) =
  harmonic_mean(RelationRootScore(D), TargetCoordinateScore(V | D))
```

`RelationRootScore(D)` 必须通过 `RELATION_FLOOR`，`TargetCoordinateScore` 必须通过
`TARGET_FLOOR`，最后 `RelationRootTransitionScore` 还必须通过 `TRANSITION_FLOOR`。

Local coherence 始终只是小幅辅助，不能替代 problem。

### 7.9 `harmonic_mean`

对非负分数 `a`、`b`：

```text
harmonic_mean(a, b) =
  0                         if a = 0 or b = 0
  2 × a × b / (a + b)       otherwise
```

```text
TransitionScore(D, V) =
  harmonic_mean(
    DocumentScore(D | U),
    TargetCoordinateScore(V | D)
  )
```

Golden examples：

```text
harmonic_mean(0.80, 0.80) = 0.80
harmonic_mean(0.90, 0.20) ≈ 0.327273
harmonic_mean(0.90, 0.00) = 0
```

调和平均使关系文档与目标 Coordinate 任一边很低时，整条跳转被短板拉低。不得添加 epsilon 让零分关系
材料被高分 Node “救活”。

### 7.10 PathScore

避免路径越长因为累加项越多而天然更高：

`RootScore` 的 closed 定义为：

```text
automatic Coordinate root       = CandidateScore(root source)
explicit initial with embedding = CandidateScore(root source)
Context Document root            = RelationRootScore(root document)
explicit initial without embedding = None
```

同一 source root 有多个 eligible structural entrypoints 时仍只有一个 `RootScore`；不按
entrypoint 复制或累加 source score。

```text
DISCOUNT = 0.85

WeightedPathQuality =
  (RootScore + Σ DISCOUNT^(hop-1) × TransitionScore(hop))
  / (1 + Σ DISCOUNT^(hop-1))

PathScore = max(
  0,
  WeightedPathQuality
    - hop_count × HOP_PENALTY
)
```

实现不直接执行上述展示用浮点公式，而是等价的 fixed-point 形状：

```text
root_weight = SCORE_SCALE if RootScore exists, otherwise 0

numerator =
    (RootScore exists ? RootScore × root_weight : 0)
  + Σ transition_score_i × discount_weight_i

denominator = root_weight + Σ discount_weight_i

weighted_path_quality =
  None                              if denominator = 0
  round_div(numerator, denominator) otherwise

path_score =
  None if weighted_path_quality is None
  max(0, weighted_path_quality - min(SCORE_SCALE,
      hop_count × HOP_PENALTY)) otherwise
```

`DISCOUNT_WEIGHT(1) = SCORE_SCALE`，`DISCOUNT_WEIGHT(n+1) =
mul_score(DISCOUNT_WEIGHT(n), 850_000)`；每一跳的权重依次基于上一个已 round 的整数权重，
禁止用平台 `powf`。`HOP_PENALTY`、floor 和所有权重均进入
`semantic-graph-score`。路径内已经禁止重复 Coordinate / Edge，首版不再叠加未定义的
`repetition_penalty`。

乘法和累加使用足够宽的无符号整数，仅在最后除法时 round 一次。

Explicit initial root 可以缺少 semantic embedding：

- 零跳 root 的 `semantic_score = None`；
- 第一条 qualifying transition 出现后，PathScore 只对已存在的 transition 项按上述权重
  重新归一化，不把缺失 RootScore 当作零；
- 返回值显式记录 `semantic_provenance = None` 和 `semantic_score = None`；
- 它仍可根据 canonical graph 扩展，但不伪造自动 semantic relevance。

### 7.11 Score explanation

每个 source / path 返回足以精确重算的结构化分量：

```text
ScoreExplanation
├── problem_score
├── conditioned_evidence[]
│   ├── context_coordinate
│   ├── context_kind_weight
│   ├── conditioned_score
│   ├── raw_gain
│   └── weighted_gain
├── highest_gain
├── second_highest_gain
├── environment_gain
├── anchor_gain
├── local_coherence?
├── document_score?
├── target_coordinate_score?
├── transition_score?
├── penalties[]
└── final_score
```

查询层不使用 LLM 生成“为什么相关”的自然语言理由。Score 不表示事实置信度、因果强度、权限或项目优先级。

## 8. Root 选择

### 8.1 Root 来源

```text
RootDiscoveryChannel
├── explicit_initial
├── problem_neutral
└── context_conditioned { context_coordinate }

RootStructuralRole
├── coordinate
└── context_document { edge_key, document_id }
```

Query lane 与 graph structural role 是两条正交轴，不得塞进同一个 `origin` enum。首版在查询
开始时就选完所有有界 roots，不再增加一个会丢失原始 discovery channel 的
`global_restart` origin。

### 8.2 候选池

```text
RootPool =
    explicit initial roots
  ∪ top-K(Q0)
  ∪ top-K(each Qi)
  ∪ Context Document structural projections of those source hits
```

semantic candidate 必须满足 `ProblemScore >= BASE_ENTRY_FLOOR`。EnvironmentGain 不能把与 problem 不相关的
对象单独推入结果。

Source 资格与 structural entrypoint 资格必须分开计算。对每个 source 先形成：

```text
EligibleStructuralEntrypoints(X)
├── coordinate { Coordinate }
│   └── current graph member
│       AND (lifecycle selector accepts source
│            OR Coordinate 是 explicit initial)
└── context_document { edge_key, document_id }
    └── current active binding
        AND ProblemScore(X) >= BASE_ENTRY_FLOOR
        AND RelationRootScore(X) >= RELATION_FLOOR
```

Automatic Coordinate entrypoint 另外要求 `ProblemScore >= BASE_ENTRY_FLOOR`。Explicit initial 例外只放宽
它自己的 Coordinate entrypoint，不自动放宽同 source 的 Context Document entrypoint。一个 automatic
source 至少有一个 eligible entrypoint 才能进入 root 竞争。

因此，Document 即使同时是 Coordinate 与 Context Document，也不能依靠其中一个角色绕过
另一个角色的 lifecycle 或 relation floor。DB 可在一行中聚合 source，但必须同时返回
role-specific eligibility mask；Result 和 frontier 只保留 mask 中的 entrypoints。

### 8.3 中立配额

显式 initial roots 不占自动语义 root budget；它们受独立的 `MAX_INITIAL_COORDINATES` 约束。自动语义 root
budget 至少 `50%` 保留给 problem-neutral Q0 candidates，剩余名额由 conditioned Coordinate 与
relation-document candidates 竞争。最终 root 总数上限为：

```text
accepted_initial_roots + max_semantic_roots
```

精确分配算法：

```text
S = requested max_semantic_roots
neutral_reserved = ceil(S / 2)

1. explicit in-graph roots 全部接受，不计入 S
2. 从 qualifying Q0-discovered pool 选 min(neutral_reserved, available) 个；其第一个必须是纯
   `ProblemScore` 最高者
3. 剩余 S - selected 名额在全部未选 qualifying automatic candidates 中竞争
4. 中立候选不足时可回填 conditioned 候选；中立候选充足时不得侵占保留位
```

同一 source 同时由 Q0/Qi 或 Document 双结构角色发现时只占一个 automatic root 名额；root 保留
全部 discovery evidence，但只保留通过上述 mask 的 structural entrypoints。

这保证：

- 不同环境可以改变近似候选的排序；
- 最强 problem-only root 不会被 Role / Work tunnel 挤掉；
- 不同环境没有真实边际增益时，可以得到相同路径；
- 系统不强制制造 Agent 差异。

### 8.4 确定性 root diversity

首版只在 automatic root 贪心选择时使用一个冻结的 MMR 变体；不在 frontier 内叠加另一套
“高度重复”启发式。

```text
RootRedundancy(X, selected)
  = max normalized_similarity(X, Y)
        for Y in selected automatic roots with current embeddings
  = 0 when no comparable selected root exists

RootSelectionPriority(X)
  = 0.85 × CandidateScore(X)
  + 0.15 × (1 - RootRedundancy(X, selected))
```

中立保留 lane 的第一个 root 按 `(ProblemScore DESC, canonical identity ASC)` pin，之后用
`0.85 × ProblemScore + 0.15 × (1 - redundancy)` 选其余中立位。剩余混合位才使用上述
`RootSelectionPriority` 与 `CandidateScore`。这保证 EnvironmentGain 不会在“中立保留 lane”内挤掉最强
Q0 root。

每个配额步骤都重新计算 priority，最终按 `(priority DESC, relevance score DESC, canonical
identity ASC)` 选择。Explicit roots 不参与 redundancy baseline，因为它们可能没有 embedding。低于
absolute relevance floor 的内容始终不可因 diversity 入选。

## 9. Hyperedge 路径搜索

### 9.1 Coordinate root

```text
Coordinate U
→ current incident Hyperedges
→ 每条 Edge 的每份 Context Document D 分别评分
→ 只保留通过 RELATION_FLOOR 的 (Edge, D)
→ 返回该 Edge 完整 Coordinate set
→ 对其他 Coordinate V 分别评分并依次应用 TARGET_FLOOR / TRANSITION_FLOOR
→ 生成 U --(Edge,D)--> V 后继 PathState
```

### 9.2 Relation Document root

```text
matched Context Document D
→ 验证 current active binding
→ 得到 exact Hyperedge E
→ 返回 E 的完整 Coordinate set
→ 分别评分每个 Coordinate V
→ 生成 D --E--> V 后继 PathState
```

### 9.3 Edge 多文档规则

一条 Edge 有多份 Context Documents 时，搜索选项是：

```text
(edge_key, document_id_1)
(edge_key, document_id_2)
...
```

明确禁止：

```text
EdgeEmbedding = average(document embeddings)
EdgeScore = sum(document scores)
EdgeScore = max(document scores) stored as Edge property
```

选中某份 Document 只表示“这份关系材料值得沿用”，不表示它概括整条 Edge，也不表示所有成员都相关。

### 9.4 Hyperedge 完整性

每个 hop 必须返回：

```text
edge_key
complete canonical coordinates[]
edge last_context_revision + source_change_id
all current Context Document binding observations[]
selected context_document_id + exact binding provenance
```

首版 server contract 固定 `MAX_HYPEREDGE_IDENTITY_BYTES = 64 KiB`，按不含 preview/summary 的
canonical hop edge identity JSON 字节计算，并纳入 budget profile digest。只有该固定边界被超过时才使用
`hyperedge_too_large`。一条本身不超过此边界的 path 仅因当前 request/packing 剩余字节不足而未返回时，
记录 `response_bytes` exhaustion，不伪称 Edge 过大。两种情况都绝不能只返回部分 Coordinates
或把 `{A,B,C}` 投影成 `{A,B}`。

Lifecycle/readiness 过滤只决定哪些 source 可作为 semantic candidate / continued target，不修剪 Edge identity。
一个 terminal、tombstoned 或当前不可水合的 Coordinate 仍然以 typed identity 出现在
`complete_coordinates[]`，只是不作为本次可继续 semantic target；图结构不因读取权限被缩边。

### 9.5 Best-first / beam frontier

只有 §8.2 的 eligible `structural_entrypoint` 生成 seed。一个 source root 可有多个 seed，但
source score 不因 seed 数量重复累加。内部状态冻结为：

```text
PathState
├── root_id / structural_entrypoint
├── ordered hops
├── current_coordinate?
├── visited_coordinates / visited_edges
├── root_score? / path_score?
└── successor_accumulator(max = beam_width)

ExpansionContinuation
├── coordinate_incident { path_state, after_relation_rank? }
└── edge_targets {
      path_state, entered_from_coordinate?, edge, document,
      after_target_rank?
    }
```

`after_*_rank` 是 Stage C snapshot 内部使用的 `(score DESC, canonical tie-break)` keyset，
不是 public pagination cursor。每个 exact rank 请求 `slice + 1` 条逻辑结果以判断是否仍有剩余工作。
禁止按 canonical key 先截断再评分。

扩展按以下逻辑次序进行：

1. Coordinate seed/path 从 `coordinate_incident` 开始，对全部 authorized current incident
   relation Documents 做 exact rank，先应用 `RELATION_FLOOR`，再按
   `(DocumentScore DESC, edge_key, document_id)` 流式物化；
2. 每个入选 `(Edge,D)` 形成 `edge_targets` continuation，读取完整 Hyperedge identity；
3. Relation-document seed 不伪造 entered-from Coordinate，直接以
   `(None, bound Edge, root Document)` 形成 `edge_targets`；此时 hop 的 `document_score`
   等于 `RelationRootScore(D)`；
4. `edge_targets` 对全部 authorized current target Coordinates 做 exact rank，依次应用
   `TARGET_FLOOR` / `TRANSITION_FLOOR`，按
   `(TransitionScore DESC, Coordinate canonical order)` 流式物化；
5. 每个通过的 target 形成候选后继，按
   `(PathScore DESC, full provenance canonical order)` 进入当前逻辑 PathState 的有界
   top-`beam_width` accumulator；该 PathState 的 relation/target frontier 封口或被全局预算截止后，
   accumulator 才会把保留的 successor PathStates 发布到全局队列。

因此 `beam_width` 的唯一口径是“每个逻辑 PathState 扩展后最多保留多少个 successor”，
不再同时定义一个相互冲突的 per-root/per-depth beam。观测到第 `beam_width + 1` 个可用
successor 时才记录 `beam_width` exhaustion；刚好有 `beam_width` 个不算截断。

为了保证断开分量有一次观测机会，先按
`(root canonical identity, structural_entrypoint)` 执行 deterministic first wave。每个 seed 获得
一个 expansion quantum，只对 `expanded_coordinates / incident_edges / relation_options /
target_options` 四个物化维度使用：

```text
slice = ceil(remaining_dimension_budget / remaining_first_wave_seeds)
```

`beam_width`、`max_paths`、response bytes 和 hop limit 不做 first-wave slice。一个 seed 用不完 slice
时余量自然留给后续 seed；一个 seed 用完 slice 但仍有 ranked remainder 时，必须保留
`ExpansionContinuation`，不得丢弃剩余候选。所有 seed 都获得一个 quantum 后，continuation 与已封口
state 的 successors 才进入全局 priority queue。

全局队列的 `SelectionPriority` 是 owning prefix 的 `PathScore`。缺 embedding 的 zero-hop explicit
seed 在 first wave 之后仍无 PathScore 时，scheduling priority 专用值为 `0`，再按 root / entrypoint /
continuation key 破平；这个值不写入 result，也不伪造 semantic score。一旦形成第一条 qualifying
transition，后续状态正常使用 PathScore。

Exact DB 为排名可能扫描整个 authorized incident set；请求 budget 限制的是返回/水合/入队的
逻辑工作量，不伪称 SQL `LIMIT` 能限制 distance 扫描量。DB 工作继续由 statement timeout、
scan rows 与 Phase 2 性能门控束。

其余确定性不变量：

- 同一路径不重复 Coordinate 或 Edge；
- 同一 endpoint Coordinate 最多保留 `2` 条不同 provenance；provenance identity 是
  `(root structural entrypoint, ordered [(edge_key, selected_document_id,
  continued_to_coordinate), ...])`，不只比较最后一跳；按 PathScore 优先；
- Q0/Qi 对同一 source 的距离结果按 source basis 复用；`TargetCoordinateScore(V|D)` 按
  `(D basis, V basis)` 复用；`DocumentScore(D|U)` 必须按 `(U basis, D basis)`、
  `TransitionScore` 必须按 `(U,E,D,V)` 区分；
- cache 复用不二次消耗 materialization counter，但不会让新 PathState 绕过
  `expanded_coordinates` 计数；
- 所有 tie-break 均使用 canonical identity。

Result 中的 `SemanticPath` 只是已停止的 leaf，或因全局 budget/wall deadline 被截止的
当前 prefix；不返回每个中间 PathState。`max_paths` 是最终保留的 top-N path 上限，
不是 DB scan 上限。中间 state 由 root/entrypoint/hop/beam 与全局 materialization budgets 共同限定。
Zero-hop seed 的终止状态由 Root DTO 的 `seed_outcomes[]` 表达，不再丢失。

## 10. Budget 与终止语义

### 10.1 Request budget

```text
SemanticGraphQueryBudget
├── max_recall_per_channel
├── max_semantic_roots
├── max_hops_per_path
├── beam_width
├── max_expanded_coordinates
├── max_incident_edges_materialized
├── max_relation_options_materialized
├── max_target_options_materialized
├── max_paths
├── max_wall_time_ms
└── max_response_bytes
```

调用者只能请求不超过 server hard cap 的值，不能设置 provider variants、weights、threshold、work_mem 或
pgvector GUC。

初始默认与 hard cap：

| Budget | Default | Hard cap |
|---|---:|---:|
| recall / channel | 64 | 256 |
| automatic semantic roots | 6 | 16 |
| hops / path | 3 | 6 |
| beam width | 8 | 32 |
| expanded Coordinates | 64 | 512 |
| materialized incident Edges | 96 | 1024 |
| materialized `(U,E,D)` relation options | 128 | 2048 |
| materialized `(U,E,D,V)` target options | 192 | 4096 |
| paths | 12 | 64 |
| wall time | 10 s | 30 s |
| final serialized response | 128 KiB | 256 KiB |

各 counter 的 closed 口径与截断检测为：

| Dimension | 计数口径 | 何时算 exhausted |
|---|---|---|
| `recall_per_channel` | 每个 channel 返回的 unique eligible source | exact rank 请求 `K+1`，实际存在第 `K+1` 项 |
| `semantic_roots` | selected automatic source roots；explicit initial 不计 | 仍有 qualifying automatic root 被抑制 |
| `hops_per_path` | 已物化的完整 hops | prefix 达上限后不再查看后继，按合同保守标记未检查的 hop frontier |
| `beam_width` | 每个逻辑 PathState 保留的 successors | 观测到第 `B+1` 个 qualifying successor |
| `expanded_coordinates` | 按完整 path provenance 去重的 `coordinate_incident` 首次开始；continuation 不重复计 | 仍有 PathState 需要开始 incident rank |
| `incident_edges_materialized` | Stage C 已水合完整 identity 的 `edge_key` 全局去重；relation seed 的 bound Edge 也计 | 仍有 selected Edge 需要首次水合 |
| `relation_options_materialized` | unique `(entered_from_coordinate?, edge_key, document_id)`；relation seed 使用 `None` | 仍有通过 relation floor 的 option 被抑制 |
| `target_options_materialized` | unique `(entered_from_coordinate?, edge_key, document_id, target_coordinate)` | 仍有通过 target/transition floor 的 option 被抑制 |
| `paths` | 搜索结束后参与 top-N 的 leaf/truncated prefix | 存在第 `N+1` 条可返回 path |
| `response_bytes` | 最终序列化 Event array | summary/root/path 因字节预算被省略 |

除 `hops_per_path` 明确采用“达上限即不再探测并保守报告”的特例外，“计数刚好等于
cap”不算 exhausted；必须按上表观测到被抑制的逻辑工作。
Source/score/Edge cache hit 不重复消耗对应 unique materialization counter，但新 provenance 的
Coordinate PathState 仍单独消耗 `expanded_coordinates`。

一个 hop 的物化次序固定为：

```text
admit relation option
→ hydrate/cache complete Edge identity
→ admit target option
→ atomically append the complete hop to a successor PathState
```

每步只在对应的 new unique key 存在预算时才 commit counter；任一后续步不能继续时，可以诚实保留
已发生的 inspection/materialization 计数，但不得向 Result 追加部分 hop。相同 key 的 cache reuse
不扣第二次预算。

`max_response_bytes` 计算整个序列化 Event array，而不是只计算 result content；同时必须小于运行时
`BUZZ_MAX_FRAME_BYTES` 并保留外层 Event、tags、signature 与 JSON escaping 开销。以上是首轮实现值，
Phase 0 / 6 通过真实 fixture 调整时直接更新当前 budget profile 和 digest，并协调更新所有消费者；
不保留并行 profile 版本，也不能静默改变同一 wire 的含义。

`max_wall_time_ms` 使用 monotonic clock，起点是 host/auth 与 closed request parse 成功之后、读取
Stage A ticket 之前；终点是 deterministic packing、Stage D security postflight 和 virtual Event 签名完成。
它包含 Provider slot wait、encode、最多一次完整 retry、Stage C DB work 与 hydration。所有阶段共享一个
absolute deadline，不在每个阶段重置计时器。

为了让 `wall_time_exhausted` 仍能安全返回一个完成 postflight 的签名结果，server budget profile 还冻结一个
调用者不可修改的 `response_tail_reserve_ms`。Phase 0 用 hard-cap fixture 的 packing + composite Stage D +
签名 p99 加安全余量确定它；不得凭常量猜测后静默修改。执行时：

```text
absolute_deadline = started_at + effective max_wall_time
work_deadline     = absolute_deadline - response_tail_reserve
```

Provider slot、encode、retry、Stage C statement timeout 与 traversal 都以 `work_deadline` 的剩余时间和各自
server cap 的最小值为界。到达 `work_deadline` 才形成可签名的
`completion_reason=wall_time_exhausted`，并立即进入 deterministic packing → Stage D → signing 的保留尾段。
Stage D 使用 absolute deadline 的剩余时间，不得因为搜索已超时而跳过或降级。

若未能在 `absolute_deadline` 前完成完整 packing、Stage D 和签名，整次请求返回 closed
`query_deadline_exceeded`，不返回 virtual Event。换言之，`wall_time_exhausted` 是“有安全尾段的搜索停止”，
不是“绝对时限已经过去以后继续做安全工作”。

Query result 不静默截断 canonical summary。若某一份完整 summary 无法放入当前 response budget，可以保留
source identity / title 并省略该 summary，同时报告 `summary_omitted_for_response_budget`；调用者再通过
canonical source read 获取。Edge 的完整 Coordinate identity 不允许采用同样的字段省略策略：无法完整放入
时必须整体停止 / 删除该 path，并报告 `hyperedge_too_large` 或 response truncation coverage。

### 10.2 成功终止、降级与错误

```text
BranchStopReason
├── frontier_exhausted
├── below_relevance_threshold
├── cycle_or_duplicate
├── max_hops_reached
├── hyperedge_too_large
├── global_budget_exhausted
└── wall_time_exhausted
```

```text
CompletionReason
├── frontier_exhausted
├── budget_exhausted
└── wall_time_exhausted
```

`budget_exhausted` 另返回按固定 enum order 排序的 `exhausted_dimensions[]`；多个边界同时到达时
不需要猜一个“主预算”。`frontier_exhausted` 只表示当前有界 frontier 已处理完，不表示
“找到了全部相关上下文”或“足够回答问题”。

`exhausted_dimensions[]` 的 closed order 是：

```text
recall_per_channel
semantic_roots
hops_per_path
beam_width
expanded_coordinates
incident_edges_materialized
relation_options_materialized
target_options_materialized
paths
response_bytes
```

Wall deadline 用 `completion_reason=wall_time_exhausted` 单独表达，不再同时塞入 budget enum。

Branch 同时观测到多个停止条件时使用固定优先级：

```text
wall_time_exhausted
> global_budget_exhausted
> hyperedge_too_large
> max_hops_reached
> cycle_or_duplicate
> below_relevance_threshold
> frontier_exhausted
```

`frontier_exhausted` 表示没有 current authorized outgoing option；`below_relevance_threshold`
表示存在可评分 option 但全部未过 floor；`cycle_or_duplicate` 表示至少有 qualifying option，
但全部会重复该 path 的 Coordinate/Edge。有任一 successor 继续时，当前 prefix 不作为
stopped leaf 返回；被抑制的其他分支只进入 bounded coverage 计数。

Completion 的 closed 映射是：

```text
work_deadline reached
  => wall_time_exhausted
else any exhausted_dimensions item actually suppressed eligible work,
     or hops_per_path deliberately left a frontier uninspected
  => budget_exhausted
else
  => frontier_exhausted
```

因此除 `hops_per_path` 的保守特例外，cap 刚好被填满不会自动使 Completion 降级。Root 的 zero-hop seed 使用同一 stop
reason 和优先级写入 `seed_outcomes[]`；结果不再把“已完整检查但无后继”与“从未扩展”混同。

`relation_embedding_missing`、`target_embedding_missing`、`index_coverage_partial`、
`summary_omitted_for_response_budget` 记入 `coverage.degraded_mode_counts`，不与 completion reason
争夺一个字段。

以下是整次请求错误，不签名一个伪装成功的 result：

```text
semantic_generation_changed
context_source_changed
problem_input_unsupported
query_encoder_unavailable
query_provider_busy
authorization_changed
query_disabled
query_deadline_exceeded
response_too_large
```

### 10.3 确定性 response packing

响应组装顺序固定：

1. 先按 coverage 的 hard sample cap 预留 Event envelope、observations、input observations 和最大 coverage
   空间，避免“记录省略又使 coverage 自己超界”的循环；
2. 预留全部 accepted explicit initial root 的无-summary shell；连这些必需 shell 都无法表达时，整请求
   `response_too_large`；
3. automatic roots 按 `(semantic score DESC, root canonical identity)` 打包；`semantic_score=None`
   只可出现在 explicit initial，它们已在第 2 步按 canonical identity 排序；
4. 只对已打包 root 按 `(path score DESC, path_id ASC)` 打包 path；root 被省略时，所有引用它的
   paths 同时省略，绝不返回悬空 `root_id`；
5. source title、identity、semantic provenance 和 score explanation 是已返回 semantic 条目的必需 metadata；
   summary 只有整份可放入时才附带，否则保留条目并
   写 `summary_omitted_reason=response_budget`；
6. 一条 Hyperedge hop 的完整 Coordinate/Document identity 是原子单元；放不下时丢弃整条 path，
   不截断 Edge；
7. 省略只更新预留的固定计数/有界 sample slot，最后重算 serialized Event array bytes；不允许无界
   追加 omission 列表。

如果连必需 envelope/observation/coverage 也无法在
`min(max_response_bytes, BUZZ_MAX_FRAME_BYTES - worst_case_event_overhead)` 内表达，整次请求返回
`response_too_large`。其余超出只形成诚实 partial result，不与“整请求 fail closed”混为一种行为。

### 10.4 首版不分页

首版返回一个有界 forest，不定义 continuation cursor。调用者可以：

- 调整 problem；
- 调整 context Coordinates；
- 请求更高但仍受限的 budget；
- 使用普通 `exact / incident / contains-all` 查询继续探索；
- 按结果中的 source identity 读取 canonical full content。

## 11. Result DTO

### 11.1 顶层结果

```text
SemanticGraphQueryResult
├── request_id
├── project_id
├── request_binding_digest
├── observations
│   ├── semantic_generation_id
│   ├── source_generation_contract_digest
│   ├── embedding_space_fence
│   ├── query_contract_digest
│   ├── ranking_contract_digest
│   ├── budget_profile_digest
│   ├── extractor_version
│   ├── project_context_revision
│   └── snapshot_observed_at
├── input_observations
│   ├── accepted_initial_coordinates[]
│   ├── initial_not_in_graph[]
│   ├── omitted_initial_coordinates[]
│   ├── accepted_context_coordinates[]
│   └── omitted_context_coordinates[]
├── roots[]
├── paths[]
├── coverage
├── completion_reason
└── exhausted_dimensions[]
```

结果不包含：

- problem 明文；
- raw query / source embedding；
- `semantic_units.semantic_text`；
- canonical full body；
- LLM rationale；
- 未授权来源的数量或 identity。

`request_binding_digest` 绑定本次经过认证的精确请求，而不是只重复 caller-controlled UUID。HTTP 计算：

```text
SHA-256(
  "buzz-semantic-http-request\0"
  || host-derived Community identity
  || authenticated caller pubkey
  || NIP-98 auth Event id
  || SHA-256(exact authenticated POST body bytes)
)
```

NIP-98 auth Event id 为每次请求提供不可预测的 request-specific salt，避免仅对 problem 做可离线枚举的稳定
digest。仅允许开发环境 `X-Pubkey` 时，用固定 zero auth Event id，但 exact body 内仍含 request UUID；该路径
不得用于生产。binding 只进入本次签名结果与 verifier 内存，不进入日志、metrics、Event store 或 audit 正文。
Phase 7 的 WebSocket binding 必须届时单独冻结为同等强度的 authenticated connection/request transcript
合同，不能复用 HTTP 公式假装有 NIP-98 Event。

`accepted_initial_coordinates[]` 不是 Coordinate 字符串数组，而是 closed observation：

```text
AcceptedInitialCoordinateObservation
├── coordinate
├── current graph membership observation
├── canonical source basis
└── semantic_state
    ├── current { generation, unit_key, snapshot_digest }
    ├── missing
    ├── building
    ├── failed
    └── unsupported
```

只有 current graph member 才进入该数组并成为 root。Graph-external initial 只进入
`initial_not_in_graph[]`。缺 embedding 不阻止 in-graph explicit root，但必须通过 `semantic_state` 诚实表达。

Active Edge 可以按领域合同继续保留已经 deleted、tombstoned 或暂时无法水合的 Coordinate identity。
因此“在图中”不等于“存在可作为 root 的 current canonical source”。这类输入进入独立 closed observation：

```text
OmittedInitialCoordinateObservation
├── coordinate
├── current graph membership observation
└── reason
    ├── source_not_found
    ├── source_deleted
    ├── source_tombstoned
    └── source_ineligible
```

Initial 的互斥映射固定为：不在 current graph → `initial_not_in_graph[]`；在图中且 canonical source
current/readable/eligible → `accepted_initial_coordinates[]`，即使 embedding missing；在图中但 canonical
source 属于上述不可用状态 → `omitted_initial_coordinates[]`。Source-family 或 principal/readiness 失败仍是
整请求错误，不放进 omitted observation。

Context input 也使用 closed observation：

```text
AcceptedContextCoordinateObservation
├── coordinate
├── canonical source basis / lifecycle
└── semantic head provenance used to build Qi

OmittedContextCoordinateObservation
├── coordinate
└── reason
    ├── source_not_found
    ├── source_ineligible
    ├── semantic_head_missing
    ├── semantic_head_building
    ├── semantic_head_failed
    └── conditioned_input_unsupported
```

Authorization/readiness 失败是整请求错误，不以 `omitted` 暴露对方是否存在。

### 11.2 Root

所有 root / relation Document / continued Coordinate 共用：

```text
SemanticSourcePreview
├── canonical title
├── summary?
└── summary_omitted_reason?
    └── response_budget
```

`summary=None` 表示 source 本来没有 summary；`summary_omitted_reason=response_budget` 表示当前
snapshot 有 summary，但 virtual Event 未复制它。两者不得混为同一个 null。

结构 provenance 使用现有 Edge / Binding 行中的精确字段，不把三个不同 revision 压成一个含混值：

```text
ProjectContextEdgeProvenance
├── last_context_revision
└── source_change_id

ProjectContextBindingProvenance
├── binding_context_revision
├── source_change_id
└── projection_event_id
```

它们分别映射 `project_context_edges.{last_context_revision,current_source_change_id}` 与
`project_context_document_bindings.{binding_context_revision,current_source_change_id,current_projection_event_id}`；
顶层 `observations.project_context_revision` 仍是 Stage C snapshot 的 global catalog observation，三者不能互相
冒充。

```text
SemanticRoot
├── root_id
├── discovery_channels[]
├── structural_entrypoints[]
│   ├── coordinate { Coordinate }
│   └── context_document
│       ├── edge_key
│       ├── document_id
│       ├── edge_provenance: ProjectContextEdgeProvenance
│       └── binding_provenance: ProjectContextBindingProvenance
├── source identity
├── preview: SemanticSourcePreview
├── lifecycle / status
├── canonical source provenance
├── semantic provenance?
├── semantic score?
├── score explanation?
└── seed_outcomes[]
    ├── structural_entrypoint
    ├── produced_path_count
    └── zero_hop_stop_reason?
```

同一 Document source 如果同时是 Coordinate 和 Context Document，只占一个 automatic source-root 名额，
但 `structural_entrypoints[]` 只保留通过 §8.2 role-specific mask 的真实入口，并分别生成路径。
`seed_outcomes[]` 与 entrypoints 一一对应；产生任一 path 时 `zero_hop_stop_reason=None`，否则按
§10.2 写入精确 stop reason。`produced_path_count` 是应用 `max_paths` 和 response packing 之前的搜索产出数；
后续省略由 Coverage 的 retained/returned 计数表达。Explicit initial 缺 embedding 时，
canonical provenance 仍必须存在，semantic provenance/score/explanation 为 `None`。

`root_id` 是 domain-separated SHA-256，输入为 project identity + source identity + 排序后全部
structural entrypoint identity（Coordinate 或 edge_key/document_id，不含其 revision provenance）；不包含 score
或随机数。`path_id` 是 root ID + 完整有序 hop provenance 的
domain-separated SHA-256。Wire 统一用小写 hex，不使用进程内自增 ID。

### 11.3 Path 与 hop

```text
SemanticPath
├── path_id
├── root_id
├── hops[]
├── terminal_coordinate
├── path_score
├── path_score_explanation
└── branch_stop_reason
```

```text
PathScoreExplanation
├── root_score?
├── transition_scores[]
├── discount_weights[]
├── weighted_path_quality
├── hop_penalty
└── final_score
```

```text
SemanticHyperedgeHop
├── ordinal
├── entered_from_coordinate?
├── edge
│   ├── edge_key
│   ├── complete_coordinates[]
│   ├── provenance: ProjectContextEdgeProvenance
│   └── current_context_document_bindings[]
│       ├── document_id
│       └── provenance: ProjectContextBindingProvenance
├── selected_relation_document
│   ├── document_id
│   ├── binding_provenance: ProjectContextBindingProvenance
│   ├── preview: SemanticSourcePreview
│   ├── canonical + semantic provenance
│   ├── document_score
│   └── score_explanation
├── continued_to_coordinate
│   ├── Coordinate
│   ├── preview: SemanticSourcePreview
│   ├── actual lifecycle
│   ├── canonical + semantic provenance
│   ├── target_score
│   └── score_explanation
└── transition_score
```

`selected_relation_document.binding_provenance` 必须与
`edge.current_context_document_bindings[]` 中同一 `document_id` 的 observation 完全相等。完整 binding
数组按 Document UUID 排序；`path_id` 的 hop provenance 输入包含 Edge provenance、完整 binding observations
以及被选 binding，不能只包含 `edge_key` 和 Document ID。这样 source update、Document detach/reattach 或
同一 Edge 的其他 binding 变化都不会被误认为同一可复核 path。

Coordinate-entered hop 的 Document explanation 包含 `local_coherence`；relation-document seed 的首跳把
`document_score = RelationRootScore(D)` 并在 explanation 中标明 `score_role=relation_root`。Target explanation
包含 relation-document coherence。连同 Root explanation 与 `PathScoreExplanation`，返回整数必须能按
§7 精确重算，不需要 raw vector。

路径中的 `entered_from → Edge → continued_to` 只是检索导航，不表示 Edge 有方向、因果、顺序或二元语义。

### 11.4 Coverage

```text
SemanticGraphQueryCoverage
├── authorized_graph_sources
├── current_indexed_graph_sources
├── title_only_sources
├── embedding_coverage
│   ├── current
│   ├── missing
│   ├── building
│   ├── failed
│   ├── unsupported
│   └── non_queryable_zero_vector
├── query_channels_requested
├── query_channels_executed
├── omitted_context_channel_counts_by_reason
├── neutral_candidates_considered
├── conditioned_candidates_considered
├── roots_selected
├── roots_returned
├── expanded_coordinates
├── incident_edges_materialized
├── relation_options_materialized
├── target_options_materialized
├── paths_generated
├── paths_retained
├── paths_returned
├── omitted_for_response_budget
│   ├── automatic_roots
│   ├── paths
│   └── summaries
├── truncation_counts_by_dimension
├── truncation_samples[]  // max 32
│   ├── root_id
│   ├── path_id?
│   ├── structural_entrypoint
│   └── dimension
└── degraded_mode_counts
    ├── relation_embedding_missing
    ├── target_embedding_missing
    ├── index_coverage_partial
    ├── summary_omitted_for_response_budget
    └── hyperedge_too_large
```

Coverage 只统计调用者当前有权知道的范围。`summary=None`、embedding missing 或 provider unavailable 不等于
“不相关”。除 `truncation_samples[]` 外 coverage 全部是固定字段的整数/闭集 map；sample
按 `(dimension enum order, root_id, path_id, structural_entrypoint)` 排序后只保留前 32 条，总数仍由
`truncation_counts_by_dimension` 完整表达。因此 coverage 本身始终有界。

`roots_selected` 是搜索前入选数，`roots_returned` 是 response packing 后真正序列化数。
`paths_generated` 是搜索封口后可返回的 leaf/truncated-prefix 数，`paths_retained` 是应用
`max_paths` top-N 后数，`paths_returned` 是字节打包后数。三者必须满足
`generated >= retained >= returned`，且任一 returned path 的 root 必须计入 `roots_returned`。

两个核心计数的口径必须固定：

```text
authorized_graph_sources
  = Stage C snapshot 中通过 current principal/source-family readiness、属于当前图，
    且至少具有一个真实结构角色的 canonical source identity 去重集合；lifecycle
    selector 适用于普通来源，explicit initial 保留其既定 lifecycle 例外

current_indexed_graph_sources
  = authorized_graph_sources 中同时具有该 active generation
    exact epoch + snapshot digest head、active unit set 和可查询 non-zero
    overview embedding 的 source identity 去重集合
```

这里的 `authorized_graph_sources` 是 **pre-score structural/read eligible** 集合，不应用
`BASE_ENTRY_FLOOR` 或 `RELATION_FLOOR`。语义 floor 只决定某个结构角色能否成为 root / path entrypoint；
coverage 衡量的则是调用者当前有权探索的图来源中，有多少已经具备目标 generation 的 current embedding。
因此 coverage 不依赖 query vector，也不会把“与本次 problem 分数较低”误报为“未覆盖”。同一 Document 同时
具有 Coordinate 与 Context Document 角色时仍按 canonical source identity 只计一次。

`semantic_sources.coverage_state` 是 Foundation 聚合状态，不是 per-generation currentness，不得用它代替
active-generation exact head。差集分类互斥且使用固定优先级：

```text
non_queryable_zero_vector
> failed
> building
> unsupported
> missing
```

一个 source 只进入第一个命中的类别；`current` 精确等于
`current_indexed_graph_sources`，全部分类之和必须等于 `authorized_graph_sources`。状态来自目标 active
generation 的 exact head/job/unit-set 观测，不用全局 `coverage_state` 代替。

对每个 `authorized_graph_sources` identity，判定必须绑定当前 source invalidation epoch 与目标 active
generation，不能只做集合相减后猜测原因：

```text
current
  = exact current-basis head + active matching unit set + overview embedding
    + matching model/dimensions + non-zero norm

non_queryable_zero_vector
  = exact current-basis head/unit/embedding 其余 fence 均匹配，但 vector_norm = 0

failed
  = exact target-generation job 的 desired_invalidation_epoch 等于 current epoch
    且 state = poison
    OR state = succeeded 但不存在与其 current basis 一致的完整可查询 head

building
  = exact target-generation job 的 desired_invalidation_epoch 等于 current epoch
    且 state ∈ {pending, claimed, retry}

unsupported
  = current canonical/structural source 存在，但 Foundation current observation 通过 closed
    compatibility reason 明确判定该 source 不能为目标 generation 形成 overview；不得由“没有 job”推断

missing
  = source current/eligible/supported，但没有 exact current-basis queryable head，且不存在上述
    current-epoch job / zero / integrity-failure observation
```

Stale epoch 的 job、其他 generation 的 `succeeded/poison`、旧 unit set 和旧 head 对该分类都没有贡献。
`title_only_sources` 只统计 `current` 中 `summary_coverage=title_only` 的 source，是正交子计数，不再形成第七个
互斥状态。实现必须给上述每一分支以及 stale-job 对照写 DB golden。

## 12. 权限与隐私

### 12.1 授权顺序

一次公开 query 必须按顺序：

1. host-derived Community、NIP-98 / NIP-42 credential；
2. coarse detect semantic extension；
3. 在读取详细 Coordinate、index coverage 或调用 Provider 前执行 Semantic Graph composite read decision；
4. 检查 query/index Community gates 和 readiness；
5. 解析、规范化、验证 closed request；
6. DB 内验证 current principal、ban、Project identity 和三个 canonical source family 的 read readiness；
7. 原子保留 deadline 内的 Provider slot；该 reservation 只是容量，不是授权；
8. 等待 slot 后，用一个短 READ COMMITTED writer transaction 先取得 shared Community writer fence；
9. lock wait 完成后同时按 DB clock 重验短期 fleet assertion，并重验完整 principal、composite read
   decision、query/index gate、contract fences、graph revision 与全部 context exact heads，在 commit 取得
   single-use egress permit；
10. egress permit 之后不再执行其他 await，直接调用 Provider；
11. repeatable-read transaction 内再次执行完整 composite read decision；
12. candidate 形成前应用 Community / Project、source-family readiness、lifecycle 与 graph role；
13. 返回前用短 READ COMMITTED transaction 先取得同一 shared Community writer fence，再在同一 transaction
    重验完整 composite read decision、query/index gate 与 DB-clock fleet；
14. release confirmation commit 取得 single-use result-release permit，之后不再 await，直接同步签名。

未授权调用不得触发 Provider 网络请求，也不得观察 generation、coverage、Coordinate 是否存在或候选数量。

### 12.2 Context 不扩大权限

- Role / Work / Issue 等 context Coordinate 只影响相关性；
- initial Coordinate 不授予图或来源读取权；
- Edge membership 不授予 Document / Meeting / Project View 读取权；
- Context Document binding 不授予 Document 读取权；
- semantic gate 不是 ACL；
- include terminal 不恢复 tombstone、deleted 或失权来源。

### 12.3 召回前过滤

不能先执行跨 Community ANN / exact，再隐藏未授权结果。首版的 current principal 是
Community-global，没有虚构 per-source ACL。候选 CTE 必须先绑定：

- host-derived Community；
- current Project；
- current principal；
- active generation；
- current source head；
- current Project View / Document / Meeting source-family capability/readiness；
- lifecycle selector；
- active graph structural role。

然后才执行 distance order。未来如有 per-source ACL，也必须在该 materialized eligible set 中应用。
返回内容是 as-of Stage C snapshot；Stage D 不重写该数据时点，但必须重验 current composite security gate，
包括结果涉及的全部 source-family read/capability readiness。任何失败都丢弃整个结果，不能只删除某些
source 后继续签名。

### 12.4 日志与观测

禁止日志或 metrics label 包含：

- problem；
- conditioned query text；
- title / summary；
- query vector；
- source embedding；
- context Coordinate 列表；
- source ID 等高基数敏感标签。

允许记录：

- request count / duration；
- byte length 与 channel count；
- content-free error code；
- budget profile；
- candidate / path 数量；
- stop reason；
- Provider status class；
- generation ID 仅限受控 debug / operator surface，不作 metrics label。

## 13. PostgreSQL exact 访问路径

### 13.1 Writer DB only

首版 query ticket、context head、exact recall、graph traversal、canonical hydration 和 currentness recheck 全部使用
writer pool。Read replica 在 source invalidation WAL 落后时可能返回已失效 head，不得参与首版语义查询。

### 13.2 Query ticket API

新增：

```text
Db::semantic_graph_query_ticket(...)
├── Community / Project identity
├── index/query gates
├── active generation
├── source generation contract + digest
├── embedding-space fence
├── compatible query contract + digest
├── extractor version
├── Project Context read observation
└── query readiness
```

DB API 只接受 host/auth 导出的 Community，不信任 payload 自由指定 tenant。

### 13.3 Exact candidate SQL 约束

内部 eligible CTE 至少 JOIN：

```text
communities
→ semantic_index_generations(active pointer + lifecycle)
→ semantic_source_generation_heads
→ semantic_sources(exact epoch + snapshot digest + eligibility)
→ semantic_unit_sets(active + extractor)
→ semantic_units(overview)
→ semantic_embeddings(exact generation + model + dimensions)
→ graph structural role CTE
```

Graph role CTE 必须先按 canonical source identity 聚合，再 JOIN semantic head。同一 Document 同时是
Coordinate 与 Context Document，也只产生一行 source embedding，同时保留两个结构角色与全部
binding metadata。不能在 embedding JOIN 后再用 `DISTINCT` 补救，那会先破坏 top-K 与 coverage。

首版 exact recall 使用一个 batched statement，一次处理最多 9 个 query channels，而不是重复构建
9 次 current/ACL/graph CTE。SQL shape 固定为：

```sql
WITH requested_reader(pubkey) AS (
  VALUES ($current_reader_pubkey::bytea)
),
authorized_reader AS MATERIALIZED (
  -- Inline the exact direct-member / managed-owner / current-ban predicate
  -- currently owned by Db::community_global_authorized_pubkeys, but execute
  -- it on this same transaction connection and snapshot.
  SELECT requested_reader.pubkey
  FROM requested_reader
  LEFT JOIN users AS actor
    ON actor.community_id = $host_community_id
   AND actor.pubkey = requested_reader.pubkey
  WHERE current_community_global_principal_predicate
),
authorized_project AS MATERIALIZED (
  SELECT community.id AS community_id,
         community.semantic_active_generation_id AS generation_id
  FROM communities AS community CROSS JOIN authorized_reader
  WHERE community.id = $host_community_id
    AND community.semantic_index_enabled
    AND community.semantic_graph_query_enabled
    AND community.semantic_active_generation_id = $expected_generation_id
),
graph_roles AS MATERIALIZED (
  -- UNION ALL current Coordinate identities and active Document bindings,
  -- then GROUP BY canonical source identity. Aggregate structural roles and
  -- binding tuples here so one source is represented exactly once.
  SELECT source_family, source_subtype, source_id,
         bool_or(is_coordinate) AS is_coordinate,
         bool_or(is_context_document) AS is_context_document,
         array_agg(DISTINCT binding_tuple ORDER BY binding_tuple)
           FILTER (WHERE binding_tuple IS NOT NULL) AS bindings
  FROM current_authorized_graph_source_roles
  GROUP BY source_family, source_subtype, source_id
),
explicit_initial_sources(source_family, source_subtype, source_id)
AS MATERIALIZED (
  SELECT * FROM unnest($initial_source_families,
                       $initial_source_subtypes,
                       $initial_source_ids)
),
active_generation AS MATERIALIZED (
  SELECT generation.*
  FROM authorized_project AS project
  JOIN semantic_index_generations AS generation
    ON generation.community_id = project.community_id
   AND generation.generation_id = project.generation_id
  WHERE generation.lifecycle = 'active'
    AND generation.model_contract_digest = $source_generation_contract_digest
    AND generation.dimensions = $expected_dimensions
),
eligible AS MATERIALIZED (
  SELECT source.source_family, source.source_subtype, source.source_id,
         source.invalidation_epoch, source.snapshot_digest,
         source.source_basis, source.lifecycle_class, source.source_status,
         unit_set.unit_set_id, unit.unit_key, unit.summary_coverage,
         embedding.embedding,
         (role.is_coordinate AND
           (lifecycle_selector_accepts($lifecycle_selector,
                                       source.lifecycle_class)
            OR initial.source_id IS NOT NULL))
           AS coordinate_entry_eligible,
         role.is_context_document AS context_document_entry_structural,
         role.bindings
  FROM active_generation AS generation
  JOIN semantic_source_generation_heads AS head
    ON head.community_id = generation.community_id
   AND head.generation_id = generation.generation_id
  JOIN semantic_sources AS source
    ON (source.community_id, source.source_family, source.source_subtype,
        source.source_id, source.invalidation_epoch, source.snapshot_digest)
     = (head.community_id, head.source_family, head.source_subtype,
        head.source_id, head.source_invalidation_epoch,
        head.source_snapshot_digest)
  JOIN semantic_unit_sets AS unit_set
    ON unit_set.community_id = head.community_id
   AND unit_set.unit_set_id = head.unit_set_id
   AND unit_set.state = 'active'
   AND unit_set.extractor_version = generation.extractor_version
  JOIN semantic_units AS unit
    ON unit.community_id = unit_set.community_id
   AND unit.unit_set_id = unit_set.unit_set_id
   AND unit.unit_kind = 'overview'
  JOIN semantic_embeddings AS embedding
    ON embedding.community_id = unit.community_id
   AND embedding.unit_set_id = unit.unit_set_id
   AND embedding.unit_key = unit.unit_key
   AND embedding.generation_id = generation.generation_id
   AND embedding.model_contract_digest =
       generation.model_contract_digest
   AND embedding.dimensions = generation.dimensions
   AND embedding.response_model = generation.model
  JOIN graph_roles AS role
    USING (source_family, source_subtype, source_id)
  LEFT JOIN explicit_initial_sources AS initial
    USING (source_family, source_subtype, source_id)
  WHERE source.eligibility = 'eligible'
    AND (((role.is_coordinate
           AND (lifecycle_selector_accepts($lifecycle_selector,
                                           source.lifecycle_class)
                OR initial.source_id IS NOT NULL)))
         OR role.is_context_document)
    AND source_family_is_currently_ready(source.source_family)
    AND vector_norm(embedding.embedding) > 0
),
query_vectors(channel_id, query_vector) AS MATERIALIZED (
  SELECT * FROM unnest($channel_ids, $query_vectors)
),
distances AS (
  SELECT eligible.*, query_vectors.channel_id,
         eligible.embedding <=> query_vectors.query_vector AS distance
  FROM eligible CROSS JOIN query_vectors
),
ranked AS (
  SELECT distances.*,
         row_number() OVER (
           PARTITION BY channel_id
           ORDER BY distance ASC, source_family ASC,
                    source_subtype ASC, source_id ASC, unit_key ASC
         ) AS channel_rank
  FROM distances
)
SELECT /* no raw vector */ ...
FROM ranked
WHERE channel_rank <= $recall_per_channel;
```

`explicit_initial_sources` 中的三个数组由 Relay 已 closed-parse 并 canonicalize 的 Coordinate 生成，
不是 caller 可注入的 SQL fragment。`context_document_entry_structural` 只表示 binding 结构可用；在 distance
量化出 `ProblemScore/CandidateScore` 后仍必须应用 §8.2 的 `BASE_ENTRY_FLOOR` 与
`RELATION_FLOOR`，再形成最终 entrypoint mask。不得把上述两个 boolean 重新合并成一个 source-level
`eligible` 并向 Result 暴露全部 roles。

上述 `current_community_global_principal_predicate` 是文档中对现有
`Db::community_global_authorized_pubkeys` SQL 的缩写，不是要新建一个可被 caller 伪造的 boolean
参数或未定义 SQL function。实现时把该单一权威 predicate 抽为同一 Rust SQL fragment/helper，在
ticket、Stage C 和 Stage D 复用。具体权限/readiness predicate 可由 typed DB helper 生成等价 SQL，但必须保持上述 materialization
边界。`EXPLAIN (ANALYZE, BUFFERS, SETTINGS)` 验收必须证明 distance 节点只消费 `eligible`
行，且 Community B / old generation / stale head 不进入 distance input。

0057 的外键只把 embedding 的 generation、dimensions 与 `model_contract_digest` 绑定到 generation；
`semantic_embeddings.response_model` 只有非空约束，并不由该外键证明等于
`semantic_index_generations.model`。因此 current eligible JOIN、worker activation 与 reusable embedding
路径都必须显式验证二者相等；不能只依赖 Provider adapter 当时做过校验。

再计算：

```sql
embedding <=> $query_vector::vector
```

排序 tie-break：

```text
distance ASC,
source_family ASC,
source_subtype ASC,
source_id ASC,
unit_key ASC
```

不要在生产 exact 路径用 `SET enable_indexscan=off`，因为它也会关闭 tenant / current-head B-tree。该设置只用于
benchmark exact reference。

### 13.4 Query API 拆分

新增 `crates/buzz-db/src/semantic_query.rs`，不要继续把查询读逻辑堆入已超过三千行、以写入/worker 为主的
`semantic.rs`。

建议 API：

```text
semantic_graph_query_ticket
load_current_context_overviews
begin_semantic_graph_read
recall_current_graph_sources_exact
score_candidate_matrix_exact
score_current_source_pairs_exact
resolve_current_source_structural_roles
load_incident_hyperedge_frontier
rank_incident_relation_options_exact
rank_edge_target_options_exact
hydrate_current_semantic_sources
semantic_graph_coverage
```

`load_current_context_overviews` 可以向 Relay 内部 orchestration 返回已经 exact-head fence 验证的
Foundation overview semantic text，以构造 Qi；它不得进入 public DTO、日志或 metrics。Stage C 必须再验证
该 context source epoch/digest，使编码文本与当前 head 精确一致。“semantic text 不越过 internal DB
API”指它不越过 Relay 内部 query orchestration 边界，不是禁止该受控 internal method 返回文本。

`score_current_source_pairs_exact` 是 transaction-bound scalar API：输入有界 source identity pairs，对两端
重新 JOIN 同一 active generation 的 exact current heads，只返回 cosine distance 与两端 provenance，不返回
raw vector。它服务 RootRedundancy、`LocalPathCoherence(D,U)` 和
`RelationDocumentCoherence(D,V)`。

`rank_incident_relation_options_exact` 与 `rank_edge_target_options_exact` 不能先按 canonical key 截断再让
Rust 评分。它们在同一 materialized eligible/snapshot 内计算 Q0/Qi 距离、source-pair coherence 和
`semantic-graph-score` 的 fixed-point 排序键，然后才 `ORDER BY ... LIMIT remaining_budget`。
Phase 0 的纯 Rust 实现是权威 reference；DB SQL 对乘法/除法/round/tie-break 必须通过同一 golden 逐项
等价测试。客户端不能传入 SQL 权重或 floor。

DB 层返回 internal source hit / scalar 与完整 provenance，不返回 public DTO，不执行 Provider 文本拼接。

### 13.5 Zero vector

Cosine distance对零向量没有有效方向。Phase 1 必须补齐：

- Query vector non-zero norm validation；
- active source embedding non-zero norm validation；
- worker activation 拒绝 cosine zero vector；
- exact query fail closed 处理遗留 zero row；
- ANN 阶段验证 `halfvec(2048)` cast 后仍非零。

0058 给 `semantic_embeddings` 增加 `CHECK (vector_norm(embedding) > 0) NOT VALID`：它立即阻止新零向量，
但不会让历史零行导致 migration 整体失败。Query-readiness 不得把 `NOT VALID` 当作已清理证据。
`vector_norm(vector)` 是 pgvector 0.8.5 对 full-precision `vector` 的函数；`l2_norm` 在该版本只接受
`halfvec` / `sparsevec`，不得用于本阶段的 full-vector SQL。

0058 同时定义已有 active generation 的升级路径：

1. query-readiness 扫描 active-generation exact heads，发现 full vector 零范数即拒绝 query-enable；
2. Admin `semantic repair-query-vectors` 在单事务中锁定 Community 的 active generation，只删除 exact
   current zero-vector head，并以 head 已记录的同一 source invalidation epoch 幂等 create / requeue 对应
   source-generation job；它不调用 canonical source mutation、不推进 epoch，也不触碰 rollback-ready 或
   其他 generation；
3. 历史零 embedding 在 head 删除后已不能参与查询。Worker 重建时只允许在主键冲突且旧 embedding
   `vector_norm=0` 时用已验证的 non-zero Provider 结果替换该派生行；已有 non-zero embedding 仍保持不可覆盖。
   完整 activation CAS 成功后才恢复 current query head；
4. coverage 把该差集报告为 `non_queryable_zero_vector`；
5. 被 repair 选中的旧零行通常由上述 worker activation 原位修复；没有 current head 引用的其他历史零行由
   后续 GC 清理，任何零行都不作为 current head 可用性的证据。

Repair 返回 content-free 闭集统计，并强制满足：

```text
current_heads_scanned
  = queryable_current_heads
  + zero_vector_current_heads
  + other_nonqueryable_current_heads

heads_invalidated
  = zero_vector_current_heads
  = jobs_created + jobs_requeued
```

重复执行时，已经删除 head 的 source 不再成为 victim，不会再次推进 job 或 source epoch。

这需要更新 Foundation 中现有用全零向量构造的测试 fixture，不能把“行数完整”误当成“cosine 可查询”。

## 14. HTTP `/query` 与 Relay 虚拟结果 Event

### 14.1 HTTP-first

首版复用现有 authenticated `POST /query` bridge，不新建业务专用 HTTP endpoint，也不复用 NIP-50
`search`。

原因：

- `/query` 已有 NIP-98 body binding 与 host-derived Community；
- Carryforth 已有 authenticated query client；
- raw filter extension 可以表达 closed DTO；
- NIP-50 不能表达初始 Coordinate、context Coordinate、预算和路径；
- 当前 Relay 明确拒绝 Project Context / Project Document NIP-50 search。

### 14.2 Strict raw filter extension

建议 wire：

```json
{
  "kinds": [40912],
  "authors": ["<relay-self>"],
  "#p": ["<caller-pubkey>"],
  "limit": 1,
  "buzz_project_context_semantic": {
    "request_id": "<uuid>",
    "project_id": "<uuid>",
    "problem": "...",
    "initial_coordinates": [],
    "context_coordinates": [],
    "lifecycle_filter": "all_current",
    "budget": {}
  }
}
```

Outer filter 只允许上述标准字段与一个 extension。必须：

- 单 filter；
- 单 kind；
- 单 author，等于 current Relay signer；
- 单 `p`，等于 authenticated caller；
- `limit = 1`；
- 禁止 ids、since、until、search 和普通/custom extension 混合；
- unknown inner / outer field 拒绝。

`api/bridge.rs` 必须在两阶段解析得到 `raw_filters` 之后、进入普通 Project Context / event
filter 分类之前识别这个 exclusive extension。`40912` 不加入
`is_project_context_protocol_kind`，否则 virtual response 会被误当成 canonical graph projection。

### 14.3 Virtual result Event

新增 relay-only virtual result kind，当前建议保留 `40912`：

```text
kind: 40912
author: current Relay signer
tags:
  p = caller pubkey
  request_id = <request UUID>
  request_binding = <request binding digest hex>
  t = buzz-project-context-semantic-result
content: closed SemanticGraphQueryResult JSON
```

`q` 是 Nostr 的 quote Event / address 标准 tag，不能拿 UUID request identity 复用；`x` 也已有标准
SHA-256 tag 语义。这里使用两个 feature-owned 多字符 tag，SDK 按 raw exact tag 验证，不把它们解析成
Nostr quote 或通用 content hash。

它只在本次 response 中生成：

- 不写 Event store；
- 不写 search_tsv；
- 不进入 Redis / pubsub；
- 不 fan-out；
- 不允许客户端提交；
- 不允许普通 REQ、COUNT、by-id 或 NIP-50 读取；
- 不成为 Project Context canonical projection；
- 不使用 40910 / 40911，避免与废弃 Node 设计产生歧义。

Kind registry 实现一个独立 `is_semantic_graph_virtual_result_kind`，并同步 `ALL_KINDS` 与
`is_relay_only_kind`。它不仅依赖面向可存储 Event 的 `P_GATED_KINDS`：那条路径存在 ids exemption，
不是 virtual response 的完整保护。

0058 在 `events` 增加并验证数据库级 `CHECK (kind <> 40912)`，constraint 名固定且纳入 schema readiness。
若 populated database 意外已有该 kind，migration/readiness 必须 fail closed 并要求 operator quarantine / purge；
不能先广告 capability。该约束与 `is_relay_only_kind` 分别保护直接数据库写和 Relay ingest，二者都不能省略。

普通读取也必须独立防御：

- `handlers/req.rs` 对显式包含 40912、但没有合法 exclusive raw extension 的请求返回 unsupported；
- kindless、`ids`、NIP-50 和其他无法在 filter 层证明排除 40912 的查询，在 DB predicate 中强制
  `kind <> 40912`，并在最终 delivery result gate 再丢弃该 kind；
- WS / HTTP COUNT 的 fast path 与 fallback 都应用相同排除，不能返回零伪装“支持 semantic query”；
- live fan-out、Redis/pubsub ingress 与 search materialization 也设置 virtual-kind deny gate；
- startup/readiness 验证 constraint 已 VALID 且持久化行数为零。

因此“by-id 查不到”不是依赖“正常情况下不会存进去”的假设。即使 importer、旧备份或手工 SQL 破坏前一层，
普通读取仍不得暴露带 canonical summary 的伪/旧 virtual result。

SDK verifier 必须验证 Relay signer、kind、`p`、`request_id`、`request_binding`、tag exactness、content closed schema、project /
request identity、按原 authenticated request 重算的 request binding 和 result size。只校验 request UUID 不足以
证明该签名结果回答了当前 problem / contexts / lifecycle selector / budget；同一 UUID 的旧结果必须被拒绝。
continued target 必须携带实际 lifecycle 并按原 request 的 selector 校验；每个 accepted explicit initial 的
typed source basis 与 Current/Missing/Building/Failed/Unsupported head 状态，也必须和唯一 explicit root 的
canonical / semantic provenance 一一对应。

只有成功（包括诚实 partial coverage）才返回 virtual Event。整请求错误使用现有 HTTP bridge
的 content-free closed error envelope，不伪造 `roots=[]` 的成功结果：

```text
400  invalid closed request / problem_input_unsupported
401/403  authentication or Project Context read denied
409  semantic_generation_changed / context_source_changed
413  response_too_large
429  query_provider_busy / admission denied
503  query disabled, incompatible contract, encoder/provider unavailable
504  query_deadline_exceeded
```

客户端不可通过错误形状观察未授权 generation、coverage 或 source 是否存在。

### 14.4 Capability 与 fleet fence

NIP-11 按 transport 使用两个互不冒充的 capability，不增加 schema version：

```text
buzz-project-context-semantic-query-http
buzz-project-context-semantic-query-ws
```

Phase 5 只可能广告 `...-http`；Phase 7 完成 WebSocket parity 后才额外广告 `...-ws`。一个不带 transport
的 `buzz-project-context-semantic-query` 不得广告，因为它无法表达“HTTP 已交付但 WS REQ 会丢 raw
extension”的真实状态。

只有同时满足以下条件才按 host 广告：

- Project Context structural read ready；
- semantic schema ready；
- index/query Community gates 开启；
- active generation lifecycle = active；
- source generation / embedding-space / query contract compatibility allowlist 匹配；
- Provider config 与 embedding-space fence 精确匹配；
- 对应 transport 的本 Pod runtime code 与 fail-closed raw parser 已就绪；
- deployment-wide fleet attestation 证明所有仍在负载均衡中的 Relay Pod 都运行同一 query runtime digest；
- stable Relay signer 存在。

Community DB gate 不能证明 fleet 已完成滚动升级。实现增加默认关闭的 deployment master
`BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE`，并让每个 Pod 在现有 protected readiness detail 中报告 closed
`semantic_query_runtime_digest`、HTTP parser/handler readiness 与 master state。部署控制面或 operator runbook
必须枚举所有当前可路由 Pod，生成包含 deployment identity、排序后的 instance identities、共同 runtime digest
和短有效期的 fleet attestation；`buzz-admin semantic query-readiness/query-enable` 只接受与当前部署 inventory
一致的 attestation。单个 NIP-11 响应或单个 Pod 的 `/ready` 不能充当 fleet 证明。

安全 rollout 必须先把“识别 semantic raw extension、显式拒绝普通 40912、普通读取排除 virtual kind”的
fail-closed seam 部署到所有 Pod，并保持 deployment master 与 Community query gate 关闭；完成 fleet
attestation 后才可打开 master 和 Community gate。未来代码回滚顺序相反：先 query-disable 并撤销 fleet
attestation，再从负载均衡中滚回旧 Pod。不得在 query gate 开启时进行不理解该 extension 的 mixed-fleet
rollout。

0058 使用 Community-scoped 的 `semantic_graph_http_fleet_attestations` 保存短期 operator assertion。该表不是
service discovery：DB 和任意单 Pod 都不能自称知道 LB 后面的完整实例集合。部署控制面必须先枚举**所有当前可
路由实例**，从 health-only / protected `/_status` 读取每个实例的 closed `deployment_id`、`instance_id`、
`runtime_digest`、`parser_ready`、`handler_ready`，按 `instance_id` 排序、去重后形成 strict JSON：

```json
{
  "transport": "http",
  "deployment_id": "buzz-prod-cn-1",
  "instances": [
    {
      "instance_id": "relay-0",
      "runtime_digest": "<64-character lowercase digest from /_status>",
      "http_ready": true
    }
  ]
}
```

所有实例必须 `http_ready=true`，且 runtime digest 与执行 `buzz-admin` 的相同 compiled contract 完全一致。
operator 以 `semantic fleet-attest --inventory ... --acknowledge-current-routing-inventory` 写入，TTL 只能为
30..=900 秒；需要在过期前由控制面重新枚举并刷新。`semantic fleet-check` 验证 deployment / 可选本实例 / digest /
expiry，`semantic fleet-revoke` 立即失权。revoke 或 expiry 不依赖后台清理：NIP-11、请求首次执行和最终签名前的
检查都会立即 fail closed；Provider 分布式预约的 wait 结束后，最终短 READ COMMITTED writer transaction
必须先取得 shared Community writer fence，再以 DB clock 锁定并验证 fleet、完成全量出域重验。成功 commit
后不再做其他 await，直接 hand off Provider send。fleet 或最终授权失败时已预约 slot 可以浪费，但必须零出域。
Stage D 使用同一顺序把 fleet 合入 result-release confirmation，成功 permit 后不再 await，直接同步签名。
每个 Relay 在 deployment master
为 true 时必须配置相同 `BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID` 和各自唯一的
`BUZZ_SEMANTIC_GRAPH_QUERY_INSTANCE_ID`。

NIP-11 表达 host-scoped 持久能力，不包含当前 semaphore 是否有空位、Provider slot 是否瞬时忙或
某个请求的 deadline。瞬时过载返回 `query_provider_busy`，不使 capability 抖动。

因为明确不做 V1/V2 并行协议，未来 closed schema 修改时 Relay / SDK / Carryforth / ACP 必须协调
更新；当前 capability 名不承担版本协商。旧客户端不保证能与修改后的 strict schema 混用。

### 14.5 WebSocket 延后

当前 `ClientMessage::Req` 只保存 `nostr::Filter`，未知 raw extension 会丢失。Phase 7 才修改：

- `protocol.rs` 保留 raw filter JSON；
- `connection.rs` / `handlers/req.rs` 识别 exclusive semantic query；
- 一次性返回 `EVENT + EOSE`；
- 不登记 live subscription；
- 与 HTTP 使用同一个 request/result golden；
- 只有完成独立 WS request-binding transcript、fleet attestation 与 parity 后才广告
  `buzz-project-context-semantic-query-ws`。

## 15. Carryforth 与 ACP

### 15.1 Carryforth CLI

新增：

```text
cf project-context semantic-query
  --problem <text>
  [--initial-coordinate <typed-token>]...
  [--context-coordinate <typed-token>]...
  [--lifecycle all-current|non-terminal|terminal-only]
  [bounded budget flags]
```

CLI：

- 从 NIP-11 取得 Relay self 与 capability；
- 读取当前 Project identity；
- 复用现有 typed Coordinate parser / canonicalizer；
- 生成 request UUID；
- 通过新增的 `CarryforthClient::semantic_query_once()` 发送 strict extension；
- 验证 virtual result Event；
- 输出 closed JSON / compact JSON；
- 为 Coordinate、Context Document 生成现有 canonical read 命令；
- 不在客户端重算 score、召回或路径。

不能直接复用现有 `CarryforthClient::query()`：它通过 `with_retry_body()` 对 timeout/body/decode/request
错误以及 HTTP 429/502/503/504 最多重放三次，而一次已到达 Relay 的 semantic query 可能已经产生
Provider 成本。它的默认 30 秒 total timeout 也与服务端 30 秒 hard wall cap 相同，可能在 Relay 正准备
返回时先超时并重放。

`semantic_query_once()` 的合同固定为：

```text
exact filter array bytes 只序列化一次
→ 只签名一次 NIP-98 Event
→ 保存该 auth Event id、exact body digest 与 expected request_binding_digest
→ 使用独立 45 秒 total timeout 发送恰好一次 HTTP request
→ 返回 response bytes + 本地 binding observation 给 SDK verifier
```

该方法不得调用 `with_retry_body()`，不得因 429/502/503/504、timeout、response body loss 或 decode failure
自动重发。连接失败也直接返回 content-free transport error；用户下一次显式调用生成新的 request UUID 与
NIP-98 Event。这样 Relay 内部因 generation/context churn 允许的一次完整 retry 仍是唯一可能增加 Provider
batch 次数的自动机制。45 秒客户端 timeout 为服务端 30 秒 absolute deadline、响应传输和本地验证留出固定
余量；它不扩大服务端预算。

Capability 解析的实际落点是
`crates/carryforth-cli/src/commands/project_view_snapshot.rs::ProjectViewIdentity`；必须在这里增加
semantic-query capability 字段与 NIP-11 fixture，不只在 CLI subcommand 内手工检查字符串。

建议 exit：

```text
0 success（包括 honest partial coverage）
1 input / closed schema / unsupported budget
2 network / provider transport unavailable
3 auth / restricted
4 capability / integrity / generation churn / other
5 reserved for existing write conflict；query 不复用
```

### 15.2 ACP 适配

核心查询无 Agent schema。首版 ACP Harness 不自动收集 Role/Work、不自动调用 query，也不把
结果自动注入 Runtime。本阶段只更新 `PROJECT_SPACE_SECTION` 与 contract version，指导 Agent 自己将
已验证且对当前问题有用的 Coordinate 作为 CLI `context_coordinates`。Role 缺失或环境为空时，
problem-only query 仍完全合法。

稳定 Project Space contract 增加：

- 对只有自然语言问题、没有可靠起点的任务，可先调用 semantic query；
- verified Role / Work 只作为 context Coordinates，不是 ACL 或 hard filter；
- explicit task Coordinate 同时可作为 initial Coordinate；
- semantic result 是候选路径，不是事实证据；
- 使用事实前仍读取 canonical full content；
- ordinary exact / incident / contains-all 始终可用；
- 不要求每 Turn 自动查询；
- 不把 retrieval result 自动持久化成 Agent Context 或新图。

如未来要由 Harness 自动调用，必须另行设计 `relay.rs` / `role_brief.rs` / `pool.rs` / `queue.rs`
中的调用时机、取消、预算和结果注入；不能把这些行为归因于一段提示词。

同 problem、不同 Agent environment 可以产生不同路径；环境没有真实增益时相同路径完全合法。

## 16. 实施阶段

### Phase 0：冻结合同、评测集与纯数学

> 当前状态：代码交付完成，保持 feature-off。closed DTO、canonical query text、fixed-point score、
> request binding、root / frontier 与结果结构校验已经落地；最终合并树仍须执行 §17.9 的回归命令。

#### 交付

1. 新建 `crates/buzz-semantic-query`；
2. closed request / result / budget / coverage / path DTO；
3. `Score` fixed-point、DB distance 量化、整数权重与 `7/9` 有理数特例；
4. query template canonical serializer；
5. `second_highest_gain`、`saturate`、`harmonic_mean`；
6. Candidate / Document / Target / Transition / Path score；
7. deterministic root pool、role-specific entrypoint mask、formal frontier state/counter 和 tie-break；
8. deterministic fake query encoder；
9. deterministic controlled fixture 与可接 fake encoder 的评测 seam；真实中英文 domain dataset 归入
   Phase 6 资格验证；
10. 冻结 query serializer / DTO / 算法形状，给出 provisional floor，并冻结
    `HOP_PENALTY`、counter 扣减点、stop precedence 和 coverage sample cap；
11. 冻结 HTTP request-binding digest 与 exact-body golden；
12. 用 hard-cap fixture 冻结 `response_tail_reserve_ms`，验证 work/absolute 两层 deadline；
13. 冻结当前 `semantic-graph-query`、`semantic-graph-score` 和 budget profile 的 digest 计算方法。

#### 退出门

- 所有 pure algorithms 不依赖 DB、Relay、HTTP、ACP 或 Tauri；
- 输入 permutation 不改变结果；
- score explanation 能逐项重算；
- problem dominance 不变量通过 property test；
- 不得绕过 query gate / Provider adapter 发送真实项目数据；
- query template 不发送正文或 Runtime hint；
- request/result golden fixtures closed parse；
- 同 request UUID、不同 problem/context/budget 的 request binding 必须不同；
- `wall_time_exhausted` 总有足够尾段完成 packing + composite Stage D + signing；
- 文件和 public API 符合 Rust doc / no-unwrap 规则。

### Phase 1：Query gate、Provider 与 schema readiness

> 当前状态：代码交付完成，保持 feature-off。0058、独立 query gate、共享 Provider、deadline-aware
> admission、fleet fence 与 Admin 控制面已经落地；真实 Volcengine relevance / floor 资格尚未执行，
> 因而本阶段的环境依赖退出门仍未关闭。

#### 改动

```text
migrations/0058_project_context_semantic_query.sql
schema/schema.sql
crates/buzz-db/src/migration.rs
crates/buzz-db/src/semantic.rs
crates/buzz-db/src/semantic_fleet.rs                new
crates/buzz-admin/src/semantic.rs
crates/buzz-relay/src/semantic_provider.rs       new
crates/buzz-relay/src/semantic_fleet.rs          new
crates/buzz-relay/src/semantic_runtime.rs
crates/buzz-relay/src/config.rs
crates/buzz-relay/src/state.rs
```

#### 交付

1. `semantic_graph_query_enabled` additive gate 与约束；
2. `events.kind <> 40912` VALID DB constraint 与 virtual-kind storage readiness；
3. Admin `semantic query-enable / query-disable / status`；
4. 从 worker 私有 `VolcengineEncoder` 抽取 shared provider client；
5. worker adapter 继续只接受 Overview Unit；
6. query adapter 只接受 `SemanticQueryEncoderInput`；
7. Provider batch encode、non-zero vector validation；
8. `semantic_query_provider_admission` Community/provider workload lane + deadline-aware physical slot reserve +
   wait 后独立、shared Community writer fence 线性化的 final egress confirmation；
9. semantic query readiness probe 与默认关闭的 HTTP deployment master；
10. migration / fresh schema parity；
11. `[外部资格，未完成]` 在不开放外部 route 的前提下，运行受控 Volcengine pre-enable shadow
    qualification，冻结 `BASE_ENTRY_FLOOR` / `RELATION_FLOOR` / `TARGET_FLOOR` /
    `TRANSITION_FLOOR`。

Migration 只新增 `0058_project_context_semantic_query.sql`，同步 `schema/schema.sql`，并把
`buzz-db/src/migration.rs` 的 embedded migration count / contract assertion 从 57 更新到 58；不回改 0057
checksum。

#### 退出门

- query gate 关闭时 Provider query 网络调用为零；
- query gate 不能在 index gate 关闭时开启；
- virtual kind DB constraint 已 VALID，历史/当前持久化行数为零；
- Foundation index disable 在同一事务先清 query gate；populated 0057 升级与 rollback 回归通过；
- disable query 不停止 worker；
- worker 不能借 query adapter 发送 arbitrary text；
- query adapter 不能接收 Semantic Unit body / chunk；
- worker/query 并发不永久饥饿；
- deadline 超限的 reservation 不推进 physical gate，取消不产生 slot 双用；
- reservation 不是授权；wait 后先取得 shared Community writer fence、且含 fleet 的 final writer
  confirmation 失败时零出域，slot 保持 consumed；
- final confirmation commit 与 Provider handoff 之间没有其他 await；
- 四类 floor 已通过授权的真实 Provider 空间评测冻结并进入 ranking contract digest；
- migration 从 0057 populated DB 与 fresh schema 均通过；
- rollback 只需 disable query，不 down-migrate Foundation。

### Phase 2：Writer DB current-head exact recall

> 当前状态：代码交付完成，保持 feature-off。writer DB exact recall、currentness / ACL / graph-role
> predicate、hydration、coverage、traversal read 与 postflight 已落地；目标 PostgreSQL 的 `EXPLAIN`、
> p50/p95/p99、并发、vacuum 影响与 soak 仍是启用前阻断项。

#### 改动

```text
crates/buzz-db/src/semantic_query.rs             new
crates/buzz-db/src/lib.rs
crates/buzz-db/src/semantic_query.rs::tests
```

#### 交付

1. query ticket；
2. transaction-bound current principal recheck；
3. context Coordinate current overview load；
4. graph structural role CTE；
5. 单个 materialized eligible set 上的 batched per-channel full-vector exact top-K；
6. union candidate full score matrix；
7. current canonical source hydration；
8. authorized coverage；
9. transaction-bound current source-pair scalar distance API；
10. exact vs brute-force correctness harness；
11. `[目标环境资格，未完成]` `EXPLAIN (ANALYZE, BUFFERS, WAL, SETTINGS)` baseline。

#### 退出门

- Community / Project filter 在 distance 前生效；
- stale / building / failed / old generation rows绝不命中；
- `semantic_embeddings.response_model` 与 generation model 不相等时绝不命中；
- source update commit 后旧 head立即消失；
- Document detach 后不再映射旧 Edge；
- terminal selector正确，deleted / tombstone不可恢复；
- exact top-K 与进程内 brute-force 在 float tolerance 内 100%一致；
- raw vector 不越过 DB API；internal overview text 只能进入 Relay query orchestration，不进 public DTO/日志；
- writer pool only；
- Document 双结构角色不重复 candidate/count/distance；
- coverage 对 exact current-epoch job 的 pending/claimed/retry/poison/succeeded-without-head 采用 §11.4
  固定互斥映射，stale epoch 或其他 generation job 不影响计数；
- `EXPLAIN` 证明 distance 只消费 materialized eligible rows；
- 在对外 route 可启用前，default 与 hard-cap budget、代表性/高出度 Community、多 Pod 并发下的
  p50/p95/p99、CPU、buffers、temp spill、DB scan rows、statement cancellation、最大 repeatable-read
  transaction age 与 vacuum 影响都已记录并通过冻结 SLO；
- 每条 Stage C statement 设置 `SET LOCAL statement_timeout`、`lock_timeout` 和 transaction idle guard，取当前
  剩余 work deadline 与 server cap 的最小值；Stage D 使用 absolute deadline 的剩余时间；未达标时先下调
  budget profile 或进入 ANN 评估，
  不允许带着未验证的 30s writer transaction 开放路由。

### Phase 3：Environment-conditioned ranking 与 roots

> 当前状态：代码交付完成，保持 feature-off。Q0 / Qi orchestration、bounded environment gain、neutral
> pin、MMR、explicit initial observation 与 deterministic root selection 已落地；真实 Provider 空间中的
> “同 problem / 不同 environment”质量资格仍未完成。

#### 交付

1. Q0 + Qi Provider orchestration；
2. generation / context-source retry；
3. Candidate score matrix 转 fixed-point；
4. EnvironmentGain / second-highest explanation；
5. explicit initial observations；
6. neutral 50% quota；
7. conditioned / relation root selection；
8. 冻结的 root-only MMR 与 neutral-lane pin/backfill；
9. role-specific eligible structural entrypoint mask；
10. query-level closed/bounded coverage / degraded modes。

#### 退出门

- problem-only query 无 context 时正常工作；
- context Coordinate 不必在图中也可作为 lens；
- graph-external initial 不产生伪路径；
- in-graph 但 source missing/deleted/tombstoned/ineligible 的 initial 只进入精确 omitted observation；
- 8 个重复 evidence 与一个 evidence结果相同；raw 第 9 项输入在 request 边界被拒绝；
- 同 problem + 不同真实环境在 controlled fixture 中重排 roots；
- 双方保留最强 neutral root；
- 无环境增益时结果相同；
- context missing 时 problem-only 降级且 coverage 诚实；
- generation / context churn 最多一次 retry。
- `terminal_only` 下双角色 Document 不能借 context-document 资格恢复 non-terminal Coordinate entrypoint；
- Coordinate role 入选不能让未过 `RELATION_FLOOR` 的 context-document entrypoint 启动。

### Phase 4：Hyperedge retrieval forest

> 当前状态：代码交付完成，保持 feature-off。完整 Hyperedge hydration、逐文档 relation option、
> fair first wave、global frontier、cycle / budget / stop semantics 与 forest provenance 已落地；代表性
> 高出度 Community 的并发和长时间资格仍未完成。

#### 交付

1. incident Edge frontier DB read；
2. `(edge_key, document_id)` 独立 relation option；
3. complete Hyperedge hydration；
4. DocumentScore、TargetScore、harmonic transition；
5. formal `PathState / ExpansionContinuation` 与 per-state beam；
6. cycle suppression、provenance dedup 与精确 materialization counters；
7. 可恢复 ranked remainder 的 fair first wave + global best-first frontier；
8. bounded forest、root seed outcomes、branch/completion/degraded/error taxonomy；
9. source / Edge / Binding provenance 与可重算 hop/path explanation。

#### 退出门

- 二元、三元和大 Hyperedge 均保持 exact set；
- 多 Context Documents 不聚合 Edge 分数；
- 增加无关 Document 不提升 Edge；
- RelationScore 低时高分 Node不能穿越；
- cycle 不无限扩展；
- 高出度遵守所有预算；
- disconnected relevant component 可经并列 semantic roots / fair first wave 发现；
- 每个返回 hop 可在同一 snapshot 中验证 Edge + Binding；
- Edge / Binding provenance 精确映射现有 DB revision、source-change 与 binding projection Event 字段，
  detach/reattach 后 path identity 必须变化；
- oversized Hyperedge 整体停止，绝不截断。
- first-wave slice 用尽但仍有候选时保留 continuation，回流预算可再使用；
- 除 hop limit 保守特例外，cap 刚好填满不报 exhaustion，只有实际抑制合格工作才报；
- 缺 embedding explicit root 与无 target relation root 都返回可审计 zero-hop seed outcome。

### Phase 5：HTTP、SDK 与 Carryforth

> 当前状态：HTTP-only 代码交付完成，保持 feature-off。strict `/query` extension、kind `40912` virtual
> response、普通读写 deny seam、SDK verifier、Carryforth one-shot client、NIP-11 HTTP capability 与 fleet
> fence 已落地；实际 LB inventory attestation 和首个灰度 Community 尚未通过。WebSocket 不属于本阶段。

#### 改动

```text
crates/buzz-core/src/kind.rs
crates/buzz-core/src/filter.rs
crates/buzz-sdk/src/semantic_graph.rs             new
crates/buzz-sdk/src/lib.rs
crates/buzz-relay/src/api/bridge.rs
crates/buzz-relay/src/semantic_graph_query.rs     new
crates/buzz-relay/src/semantic_graph_traversal.rs new
crates/buzz-relay/src/semantic_graph_response.rs  new
crates/buzz-relay/src/semantic_graph_observability.rs new
crates/buzz-relay/src/{router.rs,nip11.rs,state.rs}
crates/buzz-relay/src/handlers/{event.rs,req.rs,count.rs}
crates/buzz-search/src/query.rs
crates/carryforth-cli/src/lib.rs
crates/carryforth-cli/src/client.rs
crates/carryforth-cli/src/commands/project_context.rs
crates/carryforth-cli/src/commands/project_view_snapshot.rs
crates/carryforth-cli/TESTING.md
```

#### 退出门

- strict single-filter extension；
- virtual Event exact signature/tag/content/request-binding verification；
- Carryforth semantic HTTP 调用只签名/发送一次，不进入通用 read retry；SDK verifier 获得同一次发送的
  exact body bytes、NIP-98 Event id 与本地 expected request binding；
- dedicated client total timeout 固定大于 server hard deadline，服务端到达 30 秒边界不会触发自动重放；
- kind 40912 无 ingest / store / kindless/by-id/COUNT/NIP-50/fan-out 路径；
- 未授权检查早于 Provider 与 index existence lookup；
- deterministic packing 不超 cap；summary 只整份省略、Hyperedge path 只整条省略；必需 envelope
  仍超 cap 时才整请求 `response_too_large`；
- CLI problem-only、initial、context、terminal filter golden；
- NIP-11 只广告 HTTP-specific capability，且同时满足 host readiness、local transport master 与 fleet
  attestation；
- mixed-fleet fixture 中旧/disabled Pod 至少 fail closed，绝不把 semantic filter 当普通空查询成功返回；
- work deadline 可返回已签名 `wall_time_exhausted`，absolute deadline 超限只返回无 Event 的
  `query_deadline_exceeded`；
- ordinary Project Context queries无行为变化。

### Phase 6：ACP、质量资格与灰度

> 当前状态：ACP Project Space 合同与 contract version bump 已交付，查询 metrics / content-redaction
> 代码已落地；真实 Volcengine relevance / floor、PostgreSQL `EXPLAIN` / SLO、并发 / soak、实际 LB fleet
> inventory 和首个 Community 灰度均未取得资格。当前不能声明 production ready。
>
> 下列交付项中 1–4 的代码已完成，8 的书面 runbook 已在 §18.3 给出但尚未演练；5–7 的真实数据集 / 质量
> 与安全资格、9 的 soak / SLO 记录仍待执行。

#### 交付

1. Project Space stable contract 更新与 version bump；
2. Agent-facing CLI examples；
3. provider / DB / graph / response 分段 metrics；
4. query content-redaction tests；
5. 中英文 domain golden dataset；
6. 同 problem / 不同环境 path evaluation；
7. adversarial summary / prompt injection tests；
8. 单 Community operator enable / disable runbook；
9. exact-query soak 与 SLO 记录。

#### 灰度退出门

- Foundation active generation coverage达到 operator 门槛；
- query egress 明确开启且可单独关闭；
- zero unauthorized / stale / cross-Community hit；
- problem 与 source text 不进入日志；
- semantic result 不被 Agent当成 canonical fact；
- exact query 在目标 Community 规模下满足冻结 SLO；
- budget exhaustion / partial coverage 可观察；
- disable query 后普通图与来源读写完全正常；
- 至少一个真实场景证明环境改变由 `conditioned_evidence` 可解释；
- 至少一个场景证明不同环境正确返回相同路径。

### Phase 7：WebSocket parity（可后置）

> 当前状态：deferred，未实施，也不得广告
> `buzz-project-context-semantic-query-ws`。

只有完成 raw REQ preservation、one-shot lifecycle、独立 authenticated WS request binding、HTTP/WS golden
parity、auth、取消与 WS fleet attestation 测试后，才广告
`buzz-project-context-semantic-query-ws`。Phase 5 HTTP 交付不依赖本阶段。

### Phase 8：ANN（按规模触发，不是首发门槛）

> 当前状态：deferred，未实施。首发继续使用 full-vector exact；只有 Phase 6 的目标规模数据证明 exact
> 无法满足冻结 SLO 时才启动本阶段。

只有 exact p95 / CPU / concurrency 在目标 Community 明确不能满足 SLO 时进入。

2048 维约束：

- `vector` 可保存并做 full-precision exact；
- pgvector 0.8.5 的 `vector` HNSW 上限是 2000 维；
- 2048 维 ANN 必须使用 `halfvec(2048)` expression HNSW；
- ANN 结果必须用原始 full `vector` 重新排序。

建议表达式：

```sql
CREATE INDEX ... USING hnsw
  ((embedding::halfvec(2048)) halfvec_cosine_ops)
  WITH (m = 16, ef_construction = 64);
```

不能在当前跨 Community / generation / stale rows 的 `semantic_embeddings` 总表直接建立一个 global HNSW。
后续应增加纯派生 current ANN query projection，最小逻辑 shape 为：

```text
semantic_ann_current_candidates
├── community_id
├── generation_id
├── acl_cohort_id
├── source_family / subtype / id / unit_key
├── source invalidation epoch / snapshot digest
├── lifecycle / status filter metadata
├── full vector
└── primary key starts with
    (community_id, generation_id, acl_cohort_id, source identity, unit_key)
```

上述只是行逻辑 shape，**复合主键前缀不会让一个 HNSW index 按 tenant 物理隔离**。生产 DDL 必须使用
partitioned parent，并为每个启用 ANN 的 `(community_id, generation_id, acl_cohort_id)` 建立独立 physical
leaf；HNSW 只建在 leaf 上。不得创建承接多个 Community / generation / cohort 的 default ANN partition，
也不得在 parent 或共享 leaf 上建立 global HNSW。Server 先从 host、active generation 和 closed ACL cohort
解析唯一 leaf，SQL 仍以三项 equality predicate 驱动 PostgreSQL partition pruning；leaf/table identity 只能
来自 DB catalog 与 typed identifier，不能由 caller 拼接。`EXPLAIN` 必须证明 distance 节点只打开一个目标
leaf，加入其他 Community 的任意噪声行不得改变访问 plan 或结果。

增加 derived leaf lifecycle metadata：

```text
semantic_ann_leaves
├── community_id / generation_id / acl_cohort_id
├── lifecycle: building / ready / active / rollback-ready / retired / failed
├── expected eligible-role digest / count
├── materialized digest / count
└── built / verified / activated observations
```

当前 Community-global read model 可以使用一个固定 `community-read` ACL cohort。只有在一个物理
leaf 内 ACL 同质时才可使用 ANN。如未来出现 per-source/principal ACL，且无法映射到稳定物理
cohort，该请求必须回退 exact；不能用一个 global HNSW 再 `WHERE source_id IN (...)`宣称安全。

Projection 中的“query-eligible”指 current source/head/unit、canonical source-family readiness metadata 与
current graph structural role；它不包括 request-time query gate。Query-disable 可以保留不可访问的 derived
leaf，以便独立回滚，但 gate 关闭时任何 ANN statement 都不得执行。

后续 ANN 实现必须：

- 按达到规模阈值的 Community + generation + ACL cohort 建立上述物理 leaf；
- 只包含 current query-eligible rows；
- source head INSERT/replace 时同事务 upsert ANN row，head 失效/DELETE 时同事务删除；
- Coordinate 取得/失去 active graph membership，以及 Context Document Binding attach/detach 时，在同一个
  graph write transaction 中 upsert/delete 对应 ANN structural-role row；失去最后一个角色必须同步删除，
  不能等待异步 reconcile；
- graph role 新增若因 leaf building 暂时无法物化，必须记录 durable reconcile 并让该 slice 回退 exact；
- shadow generation leaf 完整后切换；
- cutover 前以 `(current exact head + eligible structural role + ACL cohort)` 为基准的 ANN projection parity =
  100%，不能只比较 semantic head 行数；
- 用 `embedding::halfvec(2048) <=> query::halfvec(2048)` 做 server-bounded overfetch；
- 用原始 full `vector <=> query` 对 overfetch 集重排，然后在同一 current snapshot 重新 JOIN 完整 eligible
  predicate：Community、Project、current principal/ACL cohort、active generation、exact epoch/digest head、
  active unit set、non-zero vector、source-family readiness、lifecycle selector 与 graph structural role；只重验
  exact head 不足以阻止已 detach 的 stale ANN row；
- exact 路径继续作为 recall correctness baseline 与 fallback。

Lifecycle/source-family 过滤在 HNSW 内可能是 scan-after-filter。所有 ANN GUC 只能由 server budget profile 以
`SET LOCAL` 设置：`hnsw.iterative_scan`、`hnsw.ef_search`、`hnsw.max_scan_tuples` 和
`hnsw.scan_mem_multiplier`。调用者不能自由传入。高选择性 filter 如果无法通过 overfetch 满足 recall /
underfill 门，自动回退 exact。

ANN 退出门：

```text
full-rerank recall@K >= 0.98
underfill <= 0.1% when exact has K results
zero stale / foreign / unauthorized hits
100% current-head + eligible-role / ANN projection parity before cutover
high-selectivity lifecycle/family slices pass recall and underfill gates
cross-Community noise does not change a Community leaf plan or result
EXPLAIN proves the expected halfvec HNSW leaf is used
attach/detach and Coordinate membership churn never exposes stale structural roles
measurable p95 benefit over exact
one-step per-Community rollback to exact
```

官方约束参考：
[pgvector HNSW](https://github.com/pgvector/pgvector/blob/v0.8.5/README.md#hnsw)、
[half-precision indexing](https://github.com/pgvector/pgvector/blob/v0.8.5/README.md#half-precision-indexing)、
[filtering and iterative scans](https://github.com/pgvector/pgvector/blob/v0.8.5/README.md#filtering)。

## 17. 测试矩阵

### 17.1 纯数学 golden

- 空 evidence：highest / second / EnvironmentGain 均为 `0`；
- 单 evidence：second 为 `0`；
- 不同 Coordinate 同分时 second 等于 first；
- 同一 Coordinate 重复不能产生 second；
- pure helper property：任意 N 个相同 evidence 不比一个 evidence 产生更高增益；
- 第三个及以后 evidence 不改变 EnvironmentGain；
- `0.30 + 0.25 × 0.18 = 0.345`；
- saturate 保持 `[0,1]`；
- harmonic 对称、零吸收、不高于算术平均；
- `H(0.8,0.2)=0.32`；
- DB distance 到 `Score` 的 finite/clamp/floor-half-up golden 与 Rust reference 一致；
- `DocumentScoreWithoutLocal` 严格只执行一次 `round_div(7P+2E,9)`；
- fixed-point rounding 跨平台稳定；
- canonical query JSON 对中文/C0/LF/quote/backslash/solidus/U+2028 有 exact-byte golden；
- 同一 validated vector bytes + snapshot 重放 byte-stable，不要求重新 Provider encode 产生相同 bytes；
- explanation 可精确重算 final score。

### 17.2 Query channels 与环境差异

- Q0 不包含任何环境；
- 每个 context Coordinate 独立生成一个 Qi；
- 不产生 Role-only / Work-only query；
- 不把多个 context 拼成一个 input；
- Role 是普通 optional Coordinate；
- context Coordinate 顺序改变不改变 channel IDs / scores；
- raw 9 个 context Coordinates 因超过公开 wire cap 被拒绝；
- context 缺 current head 时不发送旧 overview；
- problem 相同、OSS Work 与 Backend Work 产生受控不同排序；
- irrelevant context 不改变 neutral top root；
- 多个弱 context 不压过强 problem match；
- 相同环境语义可产生相同路径，不强制差异。

### 17.3 Root 与断开分量

- problem-only 能从全图选择 root；
- accepted initial Coordinate 一定成为显式 root；
- initial 缺 embedding 时 root score/provenance 可空，仍可从 canonical incident Edge 扩展；
- initial 不限制全局 recall；
- initial 不在图中只返回 observation；
- initial 在图中但 canonical source 不可用时只返回 closed omitted reason，不伪造 root；
- context 在图外仍可作为 lens；
- 50% neutral quota；
- relation Document 可直接成为 root；
- 同一 Document 的 coordinate/context-document 双 entrypoint 都通过自身 mask 时均保留，且 source root
  只计一个名额；任一角色不通过时只移除该 entrypoint；
- 多个 semantic roots 可同时覆盖断开分量；
- fair first wave 不伪造分量间连接。
- initial 缺 embedding 且无合格 relation 时返回 zero-hop stop reason；
- first-wave 无预算的 seed 与已完整检查的 seed 结果不同。

### 17.4 Hyperedge

- `{A,B}` 正常扩展；
- `{A,B,C}` 每个 hop返回完整三 Coordinate；
- 不生成 `{A,B}`、`{A,C}` 隐含 Edge；
- 多文档分别评分；
- 文档数量不构成 Edge 静态优势；
- Document 双结构角色不重复 source score；
- detach 后 relation role消失；
- 低 DocumentScore 阻止穿越；
- cycle / dense / high fan-out 受预算控制；
- large Edge不截断。
- first-wave ranked remainder 在其他 seed 未用预算回流后能继续扩展；
- per-state beam 观测 `B+1` 才报 exhaustion，并按 score/provenance 稳定保留 top-B；
- relation-document seed 使用 `entered_from=None` 的 counter key，且不生成部分 hop；
- branch stop 竞争按固定优先级，root/hop/path explanation 可逐项重算。

### 17.5 DB / pgvector

- active generation + current head only；
- `coverage_state=current` 但目标 generation无 head时不得命中；
- source epoch / digest mismatch不得命中；
- unit set必须 active且 extractor匹配；
- wrong model / dimensions / zero / non-finite拒绝；
- exact top-K 与 brute-force 一致；
- Community A/B 相同 UUID 不碰撞；
- lifecycle三种 selector；
- `terminal_only` 仍保留 active Context Document relation role，但不把 non-terminal Coordinate 作为 target；
- explicit initial/context lens 不被 lifecycle selector 静默删除；
- source更新、删除、detach、generation cutover并发；
- graph snapshot内每条路径可复核；
- result 明确是 as-of Stage C snapshot，而不伪称 response-time current；
- query-enable readiness 拒绝 active zero heads，repair/rebuild 后 coverage 恢复；
- pairwise scalar API 对 wrong/stale head fail closed 且不返回 vector；
- ANN Phase 8：planner只打开一个 `(Community, generation, ACL cohort)` physical leaf，无 default/global HNSW；
- ANN Phase 8：attach/detach、Coordinate membership与source-head churn后，full rerank 的完整 eligible recheck
  不返回 stale structural role；
- ANN Phase 8：eligible-role parity未达100%或高选择性underfill时自动回退exact；
- writer DB only。

### 17.6 权限与隐私

- query gate关闭时零 Provider调用；
- Stage A 后、Stage B egress linearization 前提交的撤权/query-disable 导致零出域；
- Provider slot reservation 只表示容量；reserve → wait → final writer confirmation（含 fleet）的任一失败都
  零出域，已保留 slot 仍保持 consumed；
- writer-first race：ban/remove 的排他 Community writer fence 先持有并 commit 后，Stage B/Stage D 的
  READ COMMITTED shared-fence wait 返回，必须看到撤权并拒绝 permit；
- permit-first race：Stage B egress permit 或 Stage D release permit 先 commit 时，后来的 ban/remove writer
  必须等待 shared fence 释放，并明确线性化在该次已授权 in-flight egress/release 之后；
- contract test 明确 final confirmation 不是 `REPEATABLE READ` lock-wait snapshot，Community/fleet rows 使用
  `FOR SHARE` 稳定到 commit；
- egress permit 先线性化、随后撤权的 race 被归类为已授权 in-flight batch，permit 不能复用；
- Stage A 后 context summary update/clear、delete、tombstone 或 source-family 失权时，不发送旧 overview；
- credential / principal / ban / owner状态；
- 未授权不能观察 Coordinate / generation / coverage；
- source family readiness变化 fail closed；
- ACL 在 distance 前应用；
- final auth 撤权后不返回已算结果；
- Stage D source-family/read gate 失败时不签名 virtual Event；
- Context Document binding不授予Document权限；
- problem / query text / vector不入DB、日志、metrics、event store；
- 恶意 title / summary 只作为 encoder数据；
- query result不含正文或raw vector。

### 17.7 Relay / SDK / CLI

- mixed filter、unknown field、wrong kind / author / p / limit拒绝；
- virtual Event只返回一次；
- submit kind 40912拒绝；
- DB constraint 拒绝直接持久化 40912，kindless / ids / COUNT / ordinary REQ / NIP-50 即使面对注入 fixture
  也查不到 virtual result；
- `q`/`x` 标准 tag 不出现；`request_id` / `request_binding` custom tags、signer 与 content exact verifier；
  同 UUID 不同 exact request body 的旧签名结果拒绝；
- NIP-11 分别广告 HTTP / WS capability，HTTP-only 阶段绝不声称 WS 支持；
- mixed fleet 在 query-enable 前被 attestation 阻止；disabled/旧 handler 不返回空成功；
- CLI compact与JSON golden；
- CLI timeout/body-loss/429/502/503/504 均不自动重发 semantic request；
- Provider timeout / 429 / bad model / wrong dimensions；
- provider deadline busy 不推进 physical gate；reservation 成功后的取消或含 fleet 的 final authorization
  失败会消耗但不重用 slot，并且零 Provider send；
- final writer confirmation 与 Provider handoff 之间没有其他 await；
- Stage D 把 current auth、query/index、canonical read readiness 与 DB-clock fleet 合并在一次 final release
  confirmation 中；release permit 与 result signing 之间没有其他 await；
- work deadline 停止后仍完成 composite postflight/signing；absolute deadline 超限无签名 Event；
- response cap；summary 整份省略与 Hyperedge path 整条省略的 deterministic golden；
- ordinary `exact / incident / contains-all` 回归。

### 17.8 E2E golden 场景

同一问题：

```text
“为什么发布后这个问题持续复发？”
```

环境 A：

```text
Role / Work 偏开源运营、社区发布、贡献者协作
```

环境 B：

```text
Role / Work 偏后端、数据迁移、事务恢复
```

验收：

- 两者共享最强 problem-neutral root；
- A 提升社区反馈 / 发布历史相关 Context Document 路径；
- B 提升迁移 / 事务 / 服务故障相关路径；
- 差异能由 conditioned evidence 精确解释；
- 去掉 context 后恢复 Q0 中立排序；
- 不相关 context 不制造假差异；
- 关系相关但断开当前 initial component 的内容仍可由并列 semantic root 找到。

另需 golden：

- 只通过 Context Document 命中进入 Edge；
- 多文档 Edge 只有一份与问题相关；
- title-only overview；
- partial Foundation coverage；
- terminal Work / Issue / Meeting；
- context Coordinate 自身不在图中；
- graph generation / source churn。

### 17.9 当前代码证据与尚缺资格证据

以下是本轮实现过程中已经取得的定向代码级证据。它们证明相应模块的合同与纯逻辑测试可执行，但不替代
最终合并树的全量回归，也不替代真实 Provider、数据库负载和部署 fleet 的资格验证：

| 范围 | 已执行证据 | 结论与边界 |
|---|---|---|
| query contract / score / binding / result | `cargo test -p buzz-semantic-query --lib` | 本轮曾通过 24 项；之后增加了 fleet、redaction 与恶意 result 补强，最终合并树必须重跑 |
| DB exact recall / hydration / traversal contract | `cargo test -p buzz-db --lib semantic_query::tests` | 15 项定向测试通过；被 `#[ignore]` 的真实 pgvector 测试和目标环境 `EXPLAIN` 不计入该结论 |
| migration / 历史零向量 repair | `CARGO_INCREMENTAL=0 ./scripts/test-semantic-migrations.sh` | pgvector 0.8.5 一次性容器通过：0057→0058、`vector_norm(vector)` constraint、active-head repair 闭集、重复执行、job create / requeue、canonical epoch 不变、rollback-ready generation 隔离、worker non-zero 恢复、desired-schema 与 ledger-less fresh schema |
| Relay traversal | `cargo test -p buzz-relay --lib semantic_graph_traversal::tests` | 9 项通过；对应 all-target clippy 定向检查通过 |
| Relay response packing / signing fence | `cargo test -p buzz-relay --lib semantic_graph_response::tests` | 8 项通过；覆盖 atomic path/summary omission、postflight 与 final cap |
| ACP stable contract | `cargo test -p buzz-acp --lib` | 837 项通过；对应 clippy 定向检查通过 |

交付合并前至少重新执行：

```bash
. ./bin/activate-hermit
CARGO_INCREMENTAL=0 cargo fmt --all -- --check
CARGO_INCREMENTAL=0 cargo test -p buzz-semantic-query --lib
CARGO_INCREMENTAL=0 cargo test -p buzz-db --lib semantic_query::tests
CARGO_INCREMENTAL=0 cargo test -p buzz-relay --lib semantic_graph_query::tests
CARGO_INCREMENTAL=0 cargo test -p buzz-relay --lib semantic_graph_traversal::tests
CARGO_INCREMENTAL=0 cargo test -p buzz-relay --lib semantic_graph_response::tests
CARGO_INCREMENTAL=0 cargo test -p buzz-sdk --lib semantic_graph::tests
CARGO_INCREMENTAL=0 cargo test -p carryforth-cli --lib
CARGO_INCREMENTAL=0 cargo test -p buzz-acp --lib
CARGO_INCREMENTAL=0 ./scripts/test-semantic-migrations.sh
CARGO_INCREMENTAL=0 cargo clippy -p buzz-semantic-query -p buzz-db -p buzz-relay \
  -p buzz-sdk -p carryforth-cli -p buzz-acp --all-targets -- -D warnings
```

查询能力的真实环境资格证据仍为空，不得用上述 unit / contract test 代替：

- 授权数据上的真实 Volcengine 中英文 relevance、误召回和四类 floor 报告；
- 目标 PostgreSQL 17 + pgvector 0.8.5 上的 `EXPLAIN (ANALYZE, BUFFERS, WAL, SETTINGS)`、
  p50/p95/p99、CPU、buffers、temp spill、取消与 vacuum 影响；
- default / hard-cap、高出度与多 Pod 并发 / soak 报告；
- 部署控制面枚举的实际负载均衡 inventory、短期 attestation 和 expiry / revoke 演练；
- 首个灰度 Community 的权限、路径质量、partial coverage、disable 回归与 canonical-read 验证。

## 18. Metrics、运维与灰度

### 18.1 Metrics

```text
buzz_semantic_graph_queries_total{result}
buzz_semantic_graph_query_duration_seconds{stage}
buzz_semantic_graph_query_errors_total{stage,code}
buzz_semantic_graph_query_channels{result}
buzz_semantic_graph_query_candidates{role}
buzz_semantic_graph_query_result_items{kind}
buzz_semantic_graph_query_response_bytes
buzz_semantic_graph_query_paths{branch_stop_reason}
buzz_semantic_graph_query_zero_hop_stops{branch_stop_reason}
buzz_semantic_graph_query_completions_total{completion_reason}
buzz_semantic_graph_query_budget_exhausted_total{budget_kind}
buzz_semantic_graph_query_generation_retries_total
buzz_semantic_graph_query_provider_failures_total{code}
buzz_semantic_graph_query_partial_coverage_total{reason}
buzz_semantic_graph_query_degraded_total{reason}
buzz_semantic_graph_query_provider_wait_seconds
buzz_semantic_graph_query_provider_input_bytes
buzz_semantic_graph_query_db_distance_rows{stage}
buzz_semantic_graph_query_snapshot_transaction_seconds
```

所有 labels 使用闭集低基数值。

### 18.2 Operator surface

```text
buzz-admin semantic status
buzz-admin semantic query-enable --acknowledge-problem-egress
buzz-admin semantic query-disable
buzz-admin semantic query-readiness
buzz-admin semantic repair-query-vectors
buzz-admin semantic fleet-attest --inventory ... \
  --acknowledge-current-routing-inventory
buzz-admin semantic fleet-check
buzz-admin semantic fleet-revoke
```

`buzz-admin` 每次只作用于 `RELAY_URL` host 映射到的一个 Community；CLI 不接受 caller-provided
`--community`，避免跨 Community 误操作。

`query-enable` 前必须检查：

- Foundation schema / pgvector ready；
- Community index enabled；
- active generation verified；
- active-generation exact heads 无 zero/non-queryable vector；
- Project Context structural read ready；
- source/embedding-space/query compatibility allowlist 与 Provider config match；
- HTTP deployment master 已开启，所有当前可路由 Pod 的 runtime digest / fail-closed parser / handler
  readiness 经同一短期 fleet attestation 验证；
- virtual kind storage constraint 已 VALID 且持久化行数为零；
- explicit problem egress policy acknowledged；
- coverage 与 exact benchmark达到 operator 门槛。

### 18.3 Qualification / enable runbook

本节是安全启用顺序，不是当前生产 ready 证明。命令依赖部署已有的数据库、host mapping、Relay signer 与
secret manager 配置；运行记录只保存 content-free identity、digest、计数、延迟和 closed reason，不能把 API
key、`problem`、title、summary、query vector 或完整 source identity 写进文档和日志。

#### A. Migration：查询能力继续关闭

1. 保持 `BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=false`，并确认目标 Community 的
   `semantic_graph_query_enabled=false`；
2. 先备份并按现有变更流程应用 additive migration：

   ```bash
   buzz-admin migrate
   buzz-admin semantic preflight
   buzz-admin semantic query-readiness
   ```

3. 验证 migration 0058 / fresh `schema.sql` parity、`vector(2048)`、query admission/fleet 表、kind `40912`
   VALID storage constraint、零 persisted virtual Event；任何失败都停在 feature-off。

#### B. Foundation index / generation：先形成 current exact heads

如果目标 Community 尚无已验证 active generation，按 Foundation 流程执行：

```bash
buzz-admin semantic generation-create --volcengine
buzz-admin semantic enable
buzz-admin semantic rebuild --generation-id <generation-id>
buzz-admin semantic status
buzz-admin semantic verify --generation-id <generation-id>
buzz-admin semantic generation-ready --generation-id <generation-id>
buzz-admin semantic activate --generation-id <generation-id>
```

等待 worker 完成后再次运行 `status` / `verify`。不得跳过 generation coverage、non-zero / dimension / model
fence；Document 正文和 chunk 不属于本次出域授权。

如果 0057→0058 升级后的 `query-readiness` 报告 active-generation 历史零向量，保持 query gate 关闭并执行：

```bash
buzz-admin semantic repair-query-vectors
buzz-admin semantic status
buzz-admin semantic query-readiness
```

第一次命令输出必须满足 §13.5 的闭集等式，且
`canonical_source_epochs_advanced=0`、`other_generations_changed=0`。等待 Foundation worker 把
`jobs_scheduled` 全部完成并恢复 non-zero current heads；再次运行 repair 应为零 victim。若
`other_nonqueryable_current_heads>0`，该命令不会掩盖或修复其他完整性问题，operator 必须保持 query gate
关闭并单独诊断。

#### C. 资格报告：仍不打开 Community query gate

在任何外部 route 可用前，完成并归档以下 content-free 报告：

1. 授权数据上的真实 Volcengine 中英文 relevance、误召回、同 problem / 不同 environment 与四类 floor；
2. 目标 PostgreSQL exact query 的 `EXPLAIN`、default / hard-cap p50/p95/p99、CPU、buffers、spill、
   statement cancellation、transaction age 与 vacuum 影响；
3. 高出度、多 Pod 并发、partial coverage、权限撤销、deadline、disable 与 soak；
4. §17.9 最终合并树回归全部通过。

这一步尚未完成，因此当前交付不能进入生产 query-enable。

#### D. Homogeneous fleet：所有 Community query gate 仍关闭

1. 先把 fail-closed raw parser、virtual-kind deny seam、HTTP handler、SDK/CLI 对应 Relay 代码部署到所有
   可路由 Pod；
2. 给所有 Pod 配置相同的 `BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID`、各自唯一稳定的
   `BUZZ_SEMANTIC_GRAPH_QUERY_INSTANCE_ID`，在全部 Community query gate 仍关闭的条件下统一设置
   `BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true`；query gate 关闭，所以此时 NIP-11 仍不得广告能力，
   Provider query egress 仍为零；
3. 由部署控制面直接枚举**实际负载均衡当前可路由实例**，读取 protected status 中的 instance、deployment、
   runtime digest、parser / handler readiness，生成 §14.4 的排序、去重 strict inventory；不能从单 Pod
   推断 fleet，也不能沿用滚动升级前的 inventory；
4. 写入短期 assertion 并立即检查：

   ```bash
   buzz-admin semantic fleet-attest \
     --inventory <current-lb-inventory.json> \
     --expires-in-seconds 300 \
     --acknowledge-current-routing-inventory
   buzz-admin semantic fleet-check
   ```

TTL 只允许 30..=900 秒。控制面必须在过期前重新枚举和刷新；实例增删、digest 变化或路由变化后，旧 assertion
立即视为不可复用。

#### E. Readiness 与显式 query egress acknowledgement

对首个候选 Community 执行：

```bash
buzz-admin semantic query-readiness
buzz-admin semantic fleet-check
```

只有 `database_ready=true`、`base_enable_ready=true`、active generation / exact heads / Project Context / signer /
Provider / fleet / virtual-kind constraint 全部通过，且 C 步资格报告已批准，才执行唯一的显式出域开启：

```bash
buzz-admin semantic query-enable --acknowledge-problem-egress
```

该 flag 只确认允许本 Community 将 `problem` 与 current title/summary overview 发给配置的 Provider；不授权
正文、chunk、Runtime hint 或其他自由文本。开启后复查 `query-readiness`、fleet TTL 与 NIP-11：只能广告
`buzz-project-context-semantic-query-http`，不能广告 WS capability。

#### F. 单 Community 灰度

用 Carryforth problem-only、explicit initial、不同 context environment、terminal filter 和 canonical-read
回归逐项验证；观察 §18.1 的 closed metrics、权限、partial coverage、路径 explanation 与 ordinary Project
Context 无回归。首个 Community 未通过前不得扩大。只有 exact 的目标规模 SLO 明确不达标时，才评估
Phase 8 ANN。

### 18.4 Rollout 摘要

```text
migration 0058（query off）
→ Foundation generation / current heads ready
→ Provider relevance + DB SLO + security / soak qualification
→ homogeneous Relay rollout（all Community query gates off）
→ actual LB inventory + short-lived fleet attestation
→ query-readiness / fleet-check
→ explicit --acknowledge-problem-egress + query-enable
→ one-Community canary
→ qualified expansion
```

### 18.5 回滚

```text
semantic query incident
→ semantic query-disable
→ NIP-11停止广告
→ 停止problem出域和新query
→ semantic fleet-revoke
→ 关闭deployment master / 从负载均衡移除待回滚Pod
→ Foundation worker/source embedding继续正常
→ ordinary Project Context / source reads继续正常
```

回滚不删除 canonical data、不推进业务revision、不删除 active generation。必要时再独立 disable Foundation，但那
不是查询回滚的默认动作。代码 rollback 必须发生在上述 query-disable 与 fleet attestation 撤销之后，不能让
不理解 semantic raw extension 的旧 Pod 在 query gate 开启时重新进入负载均衡。

最小 operator 顺序：

```bash
buzz-admin semantic query-disable
buzz-admin semantic fleet-revoke
buzz-admin semantic query-readiness
```

随后由部署控制面关闭 `BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE` 并执行代码回滚。即使 assertion 已过期或
被撤销，仍应显式 `query-disable`，避免后续新 assertion 意外恢复 Community gate 的可执行条件。

## 19. 代码影响面

| 责任 | 主要落点 |
|---|---|
| Workspace / crate 接线 | 根 `Cargo.toml`、新 `crates/buzz-semantic-query/Cargo.toml`、各消费 crate manifest |
| 纯 query / score / forest / binding / fleet 合同 | 新 `crates/buzz-semantic-query/src/{contract.rs,query_text.rs,encoder.rs,score.rs,root.rs,frontier.rs,result.rs,binding.rs,fence.rs,fleet.rs}` |
| Foundation 共享 model/vector validation | `crates/buzz-semantic/src/{encoder.rs,model.rs}` |
| Query gate / migration | 新 `migrations/0058_project_context_semantic_query.sql`、`schema/schema.sql`、`crates/buzz-db/src/{migration.rs,semantic.rs}` |
| current-head exact recall / graph read tx | 新 `crates/buzz-db/src/semantic_query.rs`、`crates/buzz-db/src/lib.rs` |
| fleet attestation persistence / readiness | 新 `crates/buzz-db/src/semantic_fleet.rs`、新 `crates/buzz-relay/src/semantic_fleet.rs` |
| shared Volcengine provider | 新 `crates/buzz-relay/src/semantic_provider.rs` |
| query orchestration / ranking / traversal / response | 新 `crates/buzz-relay/src/{semantic_graph_query.rs,semantic_graph_traversal.rs,semantic_graph_response.rs}`、`state.rs` |
| low-cardinality content-free observability | 新 `crates/buzz-relay/src/semantic_graph_observability.rs` |
| raw HTTP `/query` extension / route | `crates/buzz-relay/src/{api/bridge.rs,router.rs}` |
| virtual kind registry / filter defense | `crates/buzz-core/src/{kind.rs,filter.rs}` |
| virtual kind storage / read / search / fan-out defense | `crates/buzz-db/src/event.rs`、`crates/buzz-search/src/query.rs`、`crates/buzz-relay/src/handlers/{event.rs,req.rs,count.rs}` |
| request builder / result verifier | 新 `crates/buzz-sdk/src/semantic_graph.rs`、`crates/buzz-sdk/src/lib.rs` |
| capability / protected status / startup fence | `crates/buzz-relay/src/{nip11.rs,router.rs,main.rs,config.rs,semantic_runtime.rs}` |
| Operator gate | `crates/buzz-admin/src/semantic.rs` |
| Agent-facing CLI | `crates/carryforth-cli/src/{lib.rs,client.rs,commands/project_context.rs,commands/project_view_snapshot.rs}` |
| ACP stable use contract / version fixtures | `crates/buzz-acp/src/{project_space.rs,base_prompt.md,lib.rs,pool.rs,queue.rs}` |
| Provider/config wiring | `crates/buzz-relay/src/{semantic_provider.rs,config.rs,main.rs,state.rs}`、`.env.example`、config tests |
| Migration / DB qualification helpers | `scripts/test-semantic-migrations.sh`、`scripts/test-semantic-pgvector.sh`、DB ignored pgvector test |
| 运维依据 | `docs/semantic-pgvector-operations.md`、本文 §18.3 |

`SemanticQueryEncoderInput` 和 query result types 属于新 query crate。`buzz-semantic` 只提取 source/query
共用的 model-space fence、Provider result 形状与 non-zero vector validation；不把 arbitrary query text 塞回现有
closed `SemanticEncoderInput`。Shared Provider 如放入 Relay `AppState`，初始化不得继续被
`semantic_worker.enabled` 间接控制；query runtime 与 worker 可独立开关。

## 20. 风险与收口

| 风险 | 收口 |
|---|---|
| 环境压过问题，形成 Role tunnel | problem权重大于其余总和、absolute floor、neutral quota |
| context越多得分越高 | 每项独立delta、identity去重、只取top two、second × 0.25 |
| conditioned query 混成不可解释长文本 | 每个context一个固定模板通道 |
| 初始Coordinate锁死搜索范围 | initial只作root/弱anchor，全局Q0始终执行 |
| 多文档Edge静态占优 | `(edge, document)`独立option，不聚合Edge分数 |
| 高分Node穿过无关关系材料 | relation floor + harmonic mean |
| Hyperedge被拆成二元关系 | 每hop必须携带完整exact Coordinate set |
| 长路径因累加更高 | discounted weighted average + hop penalty |
| generation切换混模型 | ticket + source/space/query 三 fence + 一次完整re-encode retry |
| source变更后使用旧conditioned query | context epoch/digest recheck |
| 未授权向量挤占top-K | tenant/ACL/current-head CTE先于distance |
| query问题泄漏 | 独立gate、auth-before-provider、无内容日志、不持久化 |
| query与worker争抢Provider | 单batch、共享物理gate、interactive admission/deadline |
| 把Provider slot误当出域授权 | reserve / wait 后执行含 fleet 的 final writer confirmation；失败浪费slot但零出域 |
| 2048维直接建错误HNSW | exact首发；后续halfvec expression + full rerank |
| 全局ANN跨tenant/stale污染 | per-Community/generation current ANN projection |
| missing embedding被解释成无关 | coverage / degraded mode，绝不作否定事实 |
| semantic hit被Agent当答案 | CLI/ACP明确要求canonical full read |

## 21. 明确禁止的反例

1. `embed(problem + Role + all Works + all Issues + runtime)`；
2. 分别 embed Role / Work 后简单与 problem vector相加，却不记录独立conditioned evidence；
3. 只召回Q0 top-K后再重排，导致conditioned channel无法引入新候选；
4. 多个EnvironmentGain直接求和；
5. 按不同数值而不是evidence identity定义second-highest；
6. Role作为必填、ACL、source filter或独立Role Context；
7. initial Coordinate作为subgraph hard filter；
8. Edge summary、Edge embedding或多Document平均向量；
9. Context Document分数求和形成Edge重要性；
10. `{A,B,C}`拆成多个二元Edge；
11. BFS / shortest path代替语义评分；
12. 图邻接自动传播相关性；
13. 用epsilon让零RelationScore产生可通过Transition；
14. 把query伪造成SemanticUnit并写入Foundation表；
15. 使用read replica读取current semantic head；
16. 直接查询`semantic_embeddings`而不验证head/generation/source fence；
17. 全局HNSW后再过滤Community / ACL；
18. halfvec ANN结果不做full-vector rerank；
19. 将problem、runtime或source text写入日志；
20. semantic结果直接作为事实或权限证据；
21. 为了展示不同Agent价值而强制生成不同路径。

## 22. 最终验收不变量

1. `problem` 是唯一主查询；
2. Role不是通用查询必填项；
3. initial Coordinate可选且不限制全局召回；
4. context Coordinate可选且只影响有界相关性；
5. 每个context Coordinate形成独立conditioned query channel；
6. query problem与context overview出域只发生在显式query-enabled Community；
7. Runtime自由文本和正文不出域；
8. Query vector不持久化、不返回、不记录内容日志；
9. problem贡献大于environment与anchor总和；
10. environment使用边际增益，不重复计算problem基础相关性；
11. second-highest是同一候选的第二强独立environment evidence；
12. EnvironmentGain只使用最强和0.25倍第二强证据并clamp；
13. 关系Document和目标Coordinate通过harmonic mean形成短板约束；
14. Query结果可以因真实环境不同而不同，也允许完全相同；
15. Coordinate 与 Context Document 分别使用 source-owned overview；
16. Edge没有summary、embedding或聚合score；
17. 每份Context Document独立评分；
18. Hyperedge始终完整，不拆二元关系；
19. 并列roots和fair first wave不伪造跨分量连接；
20. 首版exact query只读active generation/current heads；
21. ACL、tenant、lifecycle和graph role在distance前生效；
22. stale、old-generation、tombstone、deleted和失权来源不能命中；
23. terminal business objects默认仍可检索；
24. 一次查询不混合generation或source basis；
25. 结果携带可审计provenance、coverage和stop reason；
26. missing / partial不等于irrelevant；
27. semantic result不是canonical事实；
28. 普通Project Context查询继续开放；
29. disable query不影响Foundation或业务读写；
30. ANN不是首发前置，且不能以tenant/ACL/currentness为代价换性能；
31. Provider slot reservation只是容量；出域授权只在等待后的final writer confirmation线性化：
    `READ COMMITTED` 先取得shared Community writer fence，再锁定并检查auth/query/index/context/fleet；
    失败时slot可浪费但必须零出域。
32. Stage D使用相同shared Community writer fence顺序，把current auth/query/index/canonical readiness/fleet
    合为一次result-release confirmation；permit后无await并同步签名。普通RR transaction在等待advisory lock
    前形成的旧snapshot不构成这两个linearization point。
