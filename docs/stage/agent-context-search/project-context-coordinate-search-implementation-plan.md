# Agent Project Context 自然语言 Coordinate 起点检索分阶段实现计划

> 状态：Phase A0–A5代码已交付；Phase A6本地单Relay资格与回滚已完成；默认feature-off；
> 目标环境SLO与多Pod资格未完成，不构成production-ready声明
>
> 日期：2026-08-14
>
> 实现基线：`feat/agent-context-search` @ `7e53094c1`
>
> 现有语义路径查询：
> [Project Context 图语义检索分阶段实现计划](../semantic/project-context-graph-semantic-query-implementation-plan.md)
>
> 语义索引基础：
> [Project Context 图语义化基础分阶段开发计划](../semantic/project-context-graph-semantic-foundation-implementation-plan.md)
>
> 图领域规范：
> [Project Context V2 领域规范](../project-context/project-context.md)
>
> 历史废弃设计：
> [已废弃的 Project Context Agent 渐进检索实现设计](../meeting/context/project-context-progressive-retrieval-implementation-design.md)
>
> 当前资格记录：
> [Coordinate起点检索资格记录](project-context-coordinate-search-qualification.md)
>
> 本计划范围：Agent 使用自然语言从当前 Project Context 中召回有界、排序后的 Coordinate 起点候选；
> 独立 request/result 合同；Coordinate-only exact recall；Relay 签名 virtual result；HTTP `/query`；
> Carryforth CLI 与 ACP 稳定提示词接入；权限、currentness、资源门和灰度资格
>
> 明确排除：现有 `semantic-query` 路径算法调整、Edge/path 返回、Role/Work 推断、
> `coordinate show`、Coordinate→Edge、Edge→Document、Edge→Coordinate 等后续渐进遍历命令、
> Desktop/Web UI、正文 embedding、Edge embedding、邻域传播、rerank、ANN、查询结果持久化

本文只交付 Agent 自查询链的第一项能力：

> 当 Agent 不知道可靠图起点时，用一句自然语言取得若干当前、可遍历的 Coordinate 候选。

现有 `cf project-context semantic-query` 继续保留为完整语义路径查询，但不再作为 Agent
自查询的默认入口。现有语义路径计划中关于 ACP 默认使用 `semantic-query` 的位置说明，实施本计划后由本文
取代；现有路径 request/result、Desktop consumer、评分、遍历与资格边界不变。

上方历史渐进检索文档已经明确废弃，不得复活其中40910/40911 Coordinate Node、Node Meta、第二份
canonical summary owner或独立Node投影。本计划选择40913只作为response-local virtual result；
Coordinate title/summary继续由来源领域唯一拥有，query层只读取Foundation派生overview索引。

## 0. 已确认的产品决策

本计划冻结以下产品决定：

1. 新增独立的自然语言 Coordinate 检索，不包装、不调用、不裁剪现有路径型 `semantic-query`；
2. 新查询只返回 Coordinate 候选，不返回 Edge、Edge key、Context Document 或 path；
3. 查询只负责选择可能的遍历起点，不回答问题，也不决定最终上下文；
4. Agent 已经知道自己的 Role、Work、Requirement、Issue、会议目的和当前任务；系统不再把这些状态
   复制成新的图结构，也不在服务端推断 Agent 所属上下文；
5. Agent 可以自行把已知任务语义写入自然语言 query；首版不接收结构化 Role、Work、initial 或
   context Coordinates；
6. 每个候选必须是真实存在于当前 Project Context、且至少属于一条当前 active Edge 的 Coordinate；
7. 仅作为 Edge 关系证明的 Project Document 不是 Coordinate 候选；若同一 Document 本身也真实出现在
   某条 active Edge 的 Coordinate 集合中，则可作为 Document Coordinate 返回一次；
8. 排名只使用一次自然语言 query embedding 与当前 Coordinate overview embedding 的 direct cosine；
9. 不使用 context gain、anchor、neutral quota、root MMR、图邻域、Edge/Document 关系评分或路径 retention；
10. 结果是高召回的起点候选，不是事实、指令、授权、路径或“正确上下文”证明；Agent可以拒绝全部候选；
11. 第一版没有 caller 可调 score floor、类型权重、lifecycle、模型、Provider 或 vector 参数；
12. 默认返回8个候选，调用者只能通过 `limit` 调整数量，硬上限32；
13. 现有 semantic overview 索引、active generation、Provider admission、权限/currentness和Relay签名链继续复用；
14. 新能力拥有独立 wire、virtual Event kind、NIP-11 capability和deployment master；Community级自然语言
    query出域授权复用现有`semantic_graph_query_enabled` gate；
15. 新查询与现有路径查询共享 Provider 物理 rate/admission lane，不能通过新增入口绕过速率或并发限制；
16. 查询可能产生 Provider 成本，CLI与ACP均不得自动重放，也不得在每个Turn自动调用；
17. query 原文不进入普通日志、URL、持久缓存、Agent Context 或 Project Context 图事实；
18. 本计划不实现后续的渐进遍历命令；它们应在本能力交付后另立计划。

共享Community gate只表达“该Community允许semantic自然语言query出域”，不会决定具体surface是否广告。
路径查询和Coordinate search仍由各自deployment master、NIP-11 capability和handler readiness独立控制；
两者共同依赖`semantic_index_enabled`并共享物理Provider/fleet控制。本计划不新增第二个Community授权列。

一句话合同：

> `coordinate-search` 只把一段自然语言映射为当前图中的 Coordinate 候选集合；从哪个候选开始、如何沿
> `Coordinate → Edge → Coordinate` 前进，仍由 Agent 根据自己的已知任务和后续 canonical reads 决定。

## 1. 问题、目标与成功定义

### 1.1 为什么不能继续把路径查询作为 Agent 入口

现有图语义查询解决的是另一类问题：

```text
problem + optional initial/context Coordinates
  → Q0/Qi channels
  → root fusion / MMR
  → bounded Hyperedge traversal
  → retrieval forest
```

它会同时决定起点、关系文档、下一跳和最终保留路径。真实验收已经表明：conditioned score可以变化，
但共享词汇、共享Issue/Stage、root截断和路径retention仍可能让不同环境得到高度重叠的路径。

Agent 自遍历不需要把这些决定一次交给检索器。Agent只需要一个低成本入口，然后可以读取Coordinate摘要、
查看incident Edges、理解关系Document，再决定下一跳。因此新入口必须在root/path逻辑之前终止。

### 1.2 本阶段目标

本阶段只实现：

```text
natural-language query
        │
        ▼
one independently versioned query embedding
        │
        ▼
current authorized indexed Coordinates in active Edges
        │
        ▼
direct cosine top-K
        │
        ▼
ranked ProjectContextCoordinate[]
```

### 1.3 成功定义

代码层成功必须同时满足：

- `cf project-context coordinate-search --query ...` 返回已验签的Coordinate-only结果；
- 对一次请求只构造一个Provider输入，只发送一次Provider embedding请求；
- 不进入semantic root选择、traversal semaphore、Edge materialization或path packing；
- binding-only Documents、graph-external来源和无active Edge的Coordinates不能占用top-K；
- Provider egress前验证当前权限与query/source fences，在RR snapshot中读取current heads，并在签名释放前
  重验caller、gates、fleet、generation、projection generation与Context revision；
- 现有 `semantic-query` wire、行为、测试与consumer不变；
- capability/gate关闭时零Provider egress；
- ACP只把该命令作为“没有可靠起点”时的显式候选发现工具，不自动调用。

质量层成功不等于“top-1永远正确”。本能力的主要离线指标是标注可接受起点集合的Recall@8；
Agent仍需在后续步骤检查候选并可拒绝全部结果。

## 2. 当前实现基线与复用边界

### 2.1 可直接复用的语义基础

当前Foundation已经提供：

- Project View、Project Document、Meeting current source overview；
- overview文本的`type + title + optional summary`合同；
- active semantic generation与current head；
- embedding model、dimension、normalization、space fence和source text digest；
- 当前来源变更后的job/rebuild/activate流程；
- exact cosine距离和固定点Score映射；
- canonical source adapter与current-head hydration。

新查询不得创建另一套Coordinate embedding，不得索引Agent私有状态，也不得读取正文构造query-time source。

### 2.2 可复用的权限与currentness链

现有语义查询已经具备：

- host-derived Community/Project绑定；
- current Human或合格managed Agent caller检查；
- ban、membership、Project View、Document、Meeting、Project Context与semantic readiness；
- Provider reservation、等待后的最终egress确认和共享Community writer fence；
- active generation、embedding-space与query contract fences；
- `REPEATABLE READ READ ONLY`查询snapshot；
- snapshot close后的release postflight；该postflight重验principal/gates/fleet/generation与Context结构身份，
  不把每个来源head变成持续current lease；
- Relay signer、caller、request、NIP-98 Event和exact body binding。

这些机制可以抽取或参数化后复用，但不能为了减少代码而让新查询调用现有graph traversal orchestration。

### 2.3 当前CLI能力与缺口

当前`cf project-context`已有：

- `semantic-query`：返回完整语义forest和read commands；
- `exact`：按完整无序Coordinate集合找Edge；
- `incident`：按一个Coordinate返回所有incident Edges及其完整Coordinates和绑定Documents；
- `contains-all`：按Coordinate子集过滤Edges；
- `attach` / `detach`：维护关系Document binding。

当前没有：

- 自然语言Coordinate-only搜索；
- 独立Coordinate摘要读取命令；
- 按Edge key拆分的渐进读取命令。

本计划只补第一个缺口。虽然`incident`当前已经一次返回后续遍历所需的大部分结构，但本计划不把它重构为
新的细粒度接口。

## 3. Closed Request合同

### 3.1 Rust DTO

在`buzz-semantic-query`中新增独立类型：

```text
ProjectContextCoordinateSearchQuery
├── request_id: UUIDv4
├── project_id: UUIDv4
├── query: String
└── limit: u8
```

建议冻结常量：

```text
DEFAULT_COORDINATE_SEARCH_LIMIT       = 8
MAX_COORDINATE_SEARCH_LIMIT           = 32
MAX_COORDINATE_SEARCH_QUERY_BYTES     = 16 KiB UTF-8
MAX_COORDINATE_SEARCH_REQUEST_BYTES   = 64 KiB
MAX_COORDINATE_SEARCH_RESPONSE_BYTES  = 64 KiB Event array
MAX_COORDINATE_SEARCH_WALL_TIME_MS    = 45_000
```

`limit`只是结果数量边界，不是DB自由资源预算。调用者不能提交 recall、deadline、response bytes、
Provider/model、score floor或query-vector参数。

### 3.2 Canonicalization

`validate_and_canonicalize()`必须：

1. 要求`request_id`为非nil UUIDv4；
2. 要求`project_id`为非nil UUIDv4；
3. 对`query`执行`str::trim()`；
4. trim后不能为空；
5. 拒绝NUL；
6. 按UTF-8 bytes检查16KiB上限，不能按字符数；
7. 不做Unicode normalization、翻译、lowercase或语言检测；
8. 要求`1 <= limit <= 32`；
9. 以`deny_unknown_fields`拒绝扩展字段；
10. 检查canonical request JSON不超过64KiB。

HTTP层必须先执行资源门再进入上述DTO canonicalization：

1. streaming读取exact body时以64KiB硬限拒绝超大`Content-Length`和chunked body；
2. raw exclusive extension必须保留为`RawValue`或等价原始slice，不能先转成会覆盖duplicate key的`Value`；
3. 对该raw slice直接使用Serde deserializer读取closed DTO，使duplicate known field和unknown field都失败；
4. request field order和无语义whitespace可以非canonical；NIP-98和request binding绑定调用者实际发送的exact bytes；
5. SDK/CLI builder始终输出唯一canonical field order，但Relay不把非canonical whitespace本身当成错误；
6. 只有Relay-signed result content要求与重新序列化后的canonical JSON逐字节相等。

Request不携带`schema_version`。以后若改变语义，直接更新closed schema、query contract digest和全部consumer，
不在同一endpoint中并行dispatch多个版本。

### 3.3 独立Provider query-text合同

新入口复用同一个embedding模型，但不复用`SemanticGraphQuery`的Q0/Qi request类型和contract digest。
新增独立模板标识：

```text
PROJECT_CONTEXT_COORDINATE_SEARCH_QUERY_CONTRACT =
  "carryforth.project-context-coordinate-search.query"
```

Provider输入固定为canonical UTF-8 JSON：

```json
{"contract":"carryforth.project-context-coordinate-search.query","query":"<escaped canonical query>"}
```

要求：

- 固定字段顺序；
- 复用现有lower-hex JSON control-character escaping实现；
- raw UTF-8，不做Unicode normalization；
- 建立独立domain-separated query contract digest与text digest；
- Debug实现只显示input类型和byte count，不显示query；
- 发送前再次验证contract digest、text digest和byte cap；
- 每个request只产生一个encoder input。

独立contract使未来可以单独评估Coordinate入口，不会因路径查询Q0/Qi模板调整而静默改变Agent起点召回。

## 4. Candidate资格与排序合同

### 4.1 Candidate必须同时属于三个集合

```text
current authorized canonical sources
∩ active semantic-generation exact heads
∩ current active Project Context Coordinate roles
```

第三个集合的精确定义是：来源能映射为一个`ProjectContextCoordinate`，且该Coordinate至少出现在当前
Project Context revision的一条active Edge完整Coordinate集合中。

因此以下对象必须排除：

- 只作为Edge binding Document的Project Document；
- 尚未进入任何active Edge的Project View/Document/Meeting来源；
- graph-external semantic source；
- deleted/tombstoned/unauthorized/non-current source head；
- inactive generation、错误embedding space或零/损坏vector；
- stale Project Context projection中的Coordinate角色。

Document同时拥有`is_coordinate=true`与`is_context_document=true`时，只以Coordinate角色返回一次，
不会因为双重结构角色增加score。

### 4.2 过滤必须发生在top-K之前

不能复用现有“Coordinate或Context Document”的全局source recall后再在Rust中过滤，因为高分
binding-only Document会占用`limit`，导致真实Coordinate少召回。

SQL逻辑必须先形成Coordinate-only eligible CTE，再计算distance和limit：

```sql
WITH coordinate_roles AS MATERIALIZED (
  -- current active Edge coordinate occurrences only
  SELECT DISTINCT canonical_source_identity, canonical_coordinate_identity
  ...
  WHERE is_coordinate = TRUE
),
eligible AS MATERIALIZED (
  SELECT ...
  FROM current_semantic_heads head
  JOIN coordinate_roles role USING (canonical_source_identity)
  WHERE head.generation_id = $active_generation
    AND head.model_contract_digest = $expected_model_contract
    AND vector_dims/head model/response model/norm/current predicates
    AND canonical/current/authorization predicates
),
ranked AS (
  SELECT ..., exact_cosine_score
  FROM eligible
  ORDER BY distance ASC, canonical_coordinate_identity ASC
  LIMIT $limit_plus_one
)
SELECT ... FROM ranked;
```

实际实现沿用当前pgvector参数化SQL和typed row mapper，不能拼接vector/query字符串。

### 4.3 Direct ranking

唯一ranking signal：

```text
score = existing fixed-point cosine(query_vector, coordinate_overview_vector)
```

排序：

1. `score`降序；
2. 同分按`ProjectContextCoordinate` canonical order升序；
3. rank从1连续编号；
4. SQL在distance前完成Coordinate身份去重；
5. 读取`limit + 1`个唯一Coordinate以证明`truncated`。

明确禁止调用或复制：

- `candidate_score`；
- `environment_gain`；
- `context_kind_weight`；
- `root_diversity_priority`；
- `select_automatic_roots`；
- Edge relation/target/transition score；
- traversal/beam/path score/retention。

### 4.4 v1不提供semantic abstention

第一版不引入未经Coordinate-search标注集校准的新score floor。只要存在eligible Coordinates，服务端就返回
最多`limit`个最近候选；不存在eligible/indexed Coordinate时返回空数组。

因此：

- `coordinates=[]`不等同于“自然语言没有答案”，只表示当前snapshot无可返回候选；
- 非空结果也不证明候选正确；
- score只用于相对排序，不是置信度或授权；
- Agent必须能在读取当前Coordinate信息后拒绝全部候选。

以后若增加abstention，必须有独立标注集、错误成本和版本化query contract，不能复用路径查询现有floor或让
caller自由传threshold。

## 5. Closed Result合同

### 5.1 Signed wire DTO

在`buzz-semantic-query`中新增：

```text
ProjectContextCoordinateSearchResult
├── request_id: UUIDv4
├── project_id: UUIDv4
├── request_binding_digest: Digest32
├── observations
│   ├── semantic_generation_id: UUIDv4
│   ├── embedding_space_fence: Digest32
│   ├── query_contract_digest: Digest32
│   ├── projection_generation: u64
│   ├── project_context_revision: u64
│   └── snapshot_observed_at: Timestamp
├── coordinates[]
│   ├── rank: u8
│   ├── coordinate: ProjectContextCoordinate
│   └── score: Score
└── truncated: bool
```

第一版不返回coverage计数，也不要求当前图的每个Coordinate都已有queryable semantic head。缺失、building、
failed、错误generation/model/dimension或current-head不一致的来源不进入本次eligible集合；其余当前已索引
Coordinates仍可正常排序返回。若eligible indexed集合为空则返回空数组。

因此，调用者不能根据某个Coordinate未出现或`coordinates=[]`推断“不相关”或“不存在”；结果只陈述本次
snapshot中可参与排名的候选，不陈述未索引集合的语义。

`truncated=true`只表示同一snapshot中存在第`limit + 1`个eligible Coordinate，不表示结果正确、答案完整或
还有可分页的continuation。第一版没有cursor或pagination。

### 5.2 Result invariants

`validate()`和`validate_for_request()`至少验证：

- request/project/generation identity合法且与request一致；
- observations fences非零、revision/generation合法；
- `coordinates.len() <= request.limit`；
- rank精确为`1..=len`；
- Coordinate canonical合法、属于同一project适用边界且不重复；
- Score在`0..=1_000_000`范围；
- score非递增，同分项按Coordinate canonical order；
- `truncated=true`时返回长度必须等于request limit；返回长度小于limit时必须为false；
- response canonical JSON/Event-array不超过64KiB；
- 不出现unknown fields。

SDK不能独立证明数据库事实，但必须严格证明Relay签名、closed content、request binding与结构不变量；
Relay负责只在通过DB snapshot与closed postflight后签名。

### 5.3 CLI输出

命令：

```bash
cf project-context coordinate-search \
  --query "与后端授权预检和无泄露错误相关的工作" \
  --limit 8
```

CLI JSON保持signed result的closed DTO，不增加本地Edge/path投影。普通结果项仅有：

```json
{
  "rank": 1,
  "coordinate": {
    "coordinate_type": "project_view_object",
    "object_type": "work",
    "object_id": "..."
  },
  "score": 812345
}
```

`--format compact`只改变JSON whitespace。CLI不得添加`read_commands`，避免把第一阶段起点发现重新扩成
隐式批量source读取。

### 5.4 明确禁止的返回内容

Wire和CLI都不得返回：

- Edge、Edge key或完整Coordinate set；
- Context Document identity或binding；
- path、root、hop、terminal；
- title、description、summary、正文或preview；
- source Markdown或embedding/vector；
- Role/Agent ownership推断；
- canonical read command；
- context/anchor/fusion/MMR/path score explanation。

## 6. HTTP、Virtual Event与SDK信任边界

### 6.1 独占`/query` filter extension

继续使用generic `POST /query`，不增加endpoint-specific HTTP API。新exact body只允许一个filter：

```json
[
  {
    "kinds": [40913],
    "authors": ["<expected-relay-pubkey>"],
    "#p": ["<authenticated-caller-pubkey>"],
    "limit": 1,
    "carryforth_project_context_coordinate_search": {
      "request_id": "...",
      "project_id": "...",
      "query": "...",
      "limit": 8
    }
  }
]
```

Filter只能有上述五个keys。新extension与现有semantic extension、普通filter、COUNT、NIP-50或多个filter
混用时必须拒绝。Relay必须在普通Filter反序列化丢弃unknown fields之前，从raw JSON严格识别该extension。

### 6.2 新virtual result kind

在当前registry确认未占用后冻结：

```text
KIND_PROJECT_CONTEXT_COORDINATE_SEARCH_RESULT = 40913
```

它是Relay-signed、response-only、non-persistent virtual Event：

- client永远不能提交；
- 不进入events表；
- 不进入普通REQ、COUNT、NIP-50、by-id、subscription或fan-out；
- 不进入search、import、replay、cache或Relay之间同步；
- 只作为当前authenticated `/query` response中的唯一Event。

建议marker：

```text
t = carryforth-project-context-coordinate-search-result
```

Tags精确复用现有结果binding形状：

```text
p               = authenticated caller
request_id      = request UUID
request_binding = exact authenticated transcript digest
t               = fixed result marker
```

不允许额外tag、不同顺序或uppercase digest。

### 6.3 SDK模块

新增`buzz_sdk::semantic_coordinate_search`，不要把第二种content schema塞入
`buzz_sdk::semantic_graph`：

- `build_project_context_coordinate_search_http_request`；
- `ProjectContextCoordinateSearchHttpQueryRequest { request, exact_body }`；
- `ProjectContextCoordinateSearchHttpRequestObservation`；
- `build_project_context_coordinate_search_result`；
- `parse_project_context_coordinate_search_result`。

Builder只序列化一次exact body；该bytes同时用于HTTP body、NIP-98 payload hash和request binding。
任何consumer都不得对request重新序列化后声称仍是同一authenticated transcript。
Coordinate-search使用独立的request-binding hash domain，不能与40912 graph result共享marker后只靠内容类型区分。

Parser验证：

- Event Schnorr signature；
- expected Relay signer；
- kind 40913；
- exact tag sequence；
- canonical closed JSON content；
- caller、Project、request id与request内容；
- NIP-98 auth Event id；
- exact authenticated body；
- request binding；
- result invariants与64KiB Event-array cap。

### 6.4 普通读取fail closed

新增migration `0059_project_context_coordinate_search.sql`：

- 独立VALID约束`events.kind <> 40913`；
- readiness检查新constraint、VALID状态和parser/runtime contract；
- `schema/schema.sql`保持fresh-install parity。

不新增Community gate column；新查询复用0058的`semantic_graph_query_enabled`与
`CHECK (NOT semantic_graph_query_enabled OR semantic_index_enabled)`。0059 readiness只新增40913 virtual-kind
存储保护和新runtime/parser能力检查。

同时扩展：

- `buzz-core` virtual-kind分类和compile-time assertions；
- `buzz-core/src/filter.rs`中的普通Filter/kind检查；
- ingest/event insert/import拒绝；
- kindless/by-id DB predicates；
- ordinaryREQ/COUNT/NIP-50拒绝与最终delivery gate；
- search强制排除；
- pubsub/fan-out不发布；
- semantic readiness persisted virtual event计数覆盖40912与40913。

不能删除或弱化0058对40912的独立保护。

## 7. Relay执行链

### 7.1 固定顺序

```text
authenticated POST /query raw body
  │
  ├── resolve host-derived Community / expected Relay signer
  ├── authenticate NIP-98 exact body
  ├── identify exclusive coordinate-search extension
  ├── cheap authorization before detailed schema errors
  ├── parse + canonicalize request
  ├── verify request.project_id == verified current Project identity
  │
  ├── Stage A: capture coordinate-search ticket
  │   ├── caller/membership/ban
  │   ├── Project View / Context / semantic readiness
  │   ├── shared semantic_graph_query Community gate
  │   ├── active generation / embedding space
  │   └── current projection generation + Context revision
  │
  ├── reserve shared semantic Provider admission
  ├── wait without holding DB transaction
  ├── final egress recheck under Community writer fence
  ├── build one independent coordinate-search query input
  ├── one Provider embedding call; no retry
  │
  ├── open REPEATABLE READ READ ONLY exact-recall snapshot
  ├── verify ticket/current generation/source heads
  ├── coordinate-only prefilter + cosine top-(limit+1)
  ├── typed current canonical hydration
  ├── pack Coordinate-only result + truncated observation
  ├── close snapshot
  │
  ├── release postflight: caller/gates/fleet/generation/projection/context revision
  ├── derive request binding from exact HTTP transcript
  ├── sign virtual kind 40913 with expected Relay key
  └── return exactly one Event array
```

### 7.2 Snapshot currentness合同

结果只表示Relay在`observations.snapshot_observed_at`对应的RR snapshot中观察到的候选排序，不是到达Agent时
仍然current的lease：

- Stage A与egress confirmation验证当时的source state/generation/query fences；
- exact recall与typed hydration在同一个RR snapshot中完成；
- snapshot内任一selected candidate无法满足canonical mapping时整次查询返回`conflict`或`unavailable`，
  不跳过后签发一个缩短的partial列表；
- release postflight重验caller、Community gate、fleet、semantic generation、Project projection generation和
  Project Context revision；
- release postflight不逐个重验returned source head；title/summary在不推进Context revision的情况下仍可能于
  snapshot后变化；
- 因此Relay签名证明snapshot、request binding和发布时授权，不证明score或source内容在响应到达时仍current；
- Agent依赖候选前必须通过后续canonical read读取当前来源，发现不存在、失权或内容变化时舍弃旧候选。

本文其他位置的“currentness”均按上述边界理解，不得扩张成response-arrival current声明。

### 7.3 不进入图遍历资源

实现必须能通过测试证明没有调用：

- `select_automatic_roots`；
- root MMR；
- incident Edge expansion；
- traversal semaphore；
- relation/target/transition ranking；
- beam/frontier；
- path packing。

Coordinate search仍占用共享Provider admission/rate gate和writer exact-query连接资源；它不是免费旁路。

### 7.4 Deadline与取消

第一版使用一个45秒absolute monotonic deadline：

- reservation等待和Provider call消费同一deadline；
- 为snapshot close、postflight、signing保留固定tail；
- deadline后不自动重放；
- client disconnect可取消本地等待，但不得导致第二次Provider请求；
- Provider已经收到query后无法远程追回；Relay只保证revocation-first在Provider egress前被final recheck阻断，
  以及release-first失败时不签发结果。

## 8. Capability、Gate与运营控制

### 8.1 独立capability

NIP-11新增：

```text
carryforth-project-context-coordinate-search-http
```

只在以下全部成立时广告：

- `CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE=true`；
- stable Relay signer；
- Project View v3与Project Context read readiness；
- semantic foundation与active generation ready；
- current Community `semantic_graph_query_enabled=true`；
- coordinate-search parser/handler runtime digest ready；
- 当前fleet policy满足；
- Provider与database readiness满足。

现有`buzz-project-context-semantic-query-http`继续独立广告。一个能力开启或关闭不得暗中改变另一个能力。

### 8.2 共享Community出域gate与物理限流

继续使用现有`buzz-admin semantic`命令：

```text
query-readiness
query-enable --acknowledge-problem-egress
query-disable
```

`query-enable`继续设置共享`semantic_graph_query_enabled`。它的deployment precondition调整为“至少一个已编译
semantic query surface master为true”，并在输出中列出当前可启用surface；保留既有
`--acknowledge-problem-egress`兼容参数，不机械改名。Enable必须在DB transaction中重复检查Foundation、
active generation、Project Context和fleet attestation。`query-disable`立即关闭两类新请求并使后续
egress/release recheck失败，但不停止semantic indexing。

只启用Coordinate search时，path master保持false、Coordinate master为true；共享Community gate开启后只有
Coordinate capability会被广告。只关闭Coordinate surface时关闭其deployment master；紧急停止全部自然语言
query egress时使用现有`query-disable`。

Provider admission、rate-limit debt、fleet attestation和物理query workload lane继续共享，避免通过两个
capability获得双倍Provider吞吐。runtime contract digest需包含两种parser/handler和两个virtual kinds；
部署新版本会使旧fleet attestation自然失效并要求重新attest。

在attested-fleet模式中，rollout必须先部署同构runtime，再重新attest，最后广告新capability。runtime digest
变化到重新attest完成之间，现有40912 semantic-query也会按共享fleet合同暂时fail closed；运营记录必须把
这个预期窗口写明，不能让新Pod先广告而请求随机落到不识别40913的旧Pod。

### 8.3 默认feature-off

共享`semantic_graph_query_enabled`沿用0058的默认false，新的deployment master默认false，NIP-11默认不广告。
代码合并不授权真实query外发。

启用顺序：

```text
Foundation ready
→ deploy homogeneous runtime
→ verify migration/readiness/virtual-kind guards
→ fleet attest（需要时）
→ set deployment master
→ semantic query-readiness
→ explicit semantic query-enable + existing egress acknowledgement
→ observe NIP-11 capability
→ authorized canary
```

回滚顺序：

```text
semantic query-disable（紧急关闭全部semantic query egress）
→ verify NIP-11 capability absent
→ revoke fleet if deployment-wide semantic HTTP rollback
→ leave Foundation index intact
→ verify ordinary Project Context reads continue
```

## 9. Error与重试合同

继续使用闭合错误类别：

| code | 典型原因 | retryable | 自动重试 |
|---|---|---:|---:|
| `invalid_input` | query空白/NUL/超限、limit非法、closed schema失败 | false | 禁止 |
| `unsupported` | capability未广告、runtime/gate未支持 | false | 禁止 |
| `restricted` | caller无权、ban、Project/Community不匹配 | false | 禁止 |
| `busy` | Provider admission/rate gate繁忙 | true | 禁止 |
| `conflict` | Project/Context/generation在请求期间变化 | true | 禁止 |
| `timeout` | 45秒deadline或Provider timeout | true | 禁止 |
| `too_large` | request/success/error body cap | false | 禁止 |
| `unavailable` | Foundation、Provider、DB、fleet或readiness不可用 | true | 禁止 |
| `verification_failed` | malformed/wrong-signer/wrong-binding virtual result | false | 禁止 |
| `internal` | signing/serialization/impossible state | false | 禁止 |

`retryable=true`只允许Human/Agent在重新评估后显式发起新request；每次重试必须生成新的request_id、
NIP-98 Event和exact body。CLI transport设置`retry=false`。

## 10. Carryforth CLI与ACP接入

### 10.1 CLI命令

`ProjectContextCmd`新增：

```rust
#[command(name = "coordinate-search")]
CoordinateSearch {
    #[arg(long)]
    query: String,
    #[arg(long, default_value_t = 8, value_parser = 1..=32)]
    limit: u8,
}
```

CLI执行：

1. 从configured Relay URL和NIP-11解析canonical Relay self；
2. 从verified Project View v3解析当前Project identity；
3. 要求新capability；
4. 本地生成UUIDv4 request_id；
5. 使用SDK builder得到canonical request和exact body；
6. 生成observed NIP-98 header/event id；
7. 单次`POST /query`；
8. success body上限64KiB，error body上限16KiB，redirect禁用；
9. 严格要求单一canonical Event；
10. SDK parse/verify后输出Result；
11. 任何错误都不自动重放。

CLI Debug/error不得包含query原文。`--help`需明确“candidate starting Coordinates, not paths or answers”。

### 10.2 ACP稳定提示词

修改：

- `crates/buzz-acp/src/project_space.rs`；
- `crates/buzz-acp/src/base_prompt.md`；
- 对应contract version/hash与tests。

稳定合同改为：

```text
如果任务已明确可靠Coordinate，跳过coordinate-search。
如果只有自然语言问题且没有可靠起点，显式调用一次coordinate-search。
结果只是起点候选；不要把rank/score当作事实或授权。
结合当前已知Role/Work/Issue/会议目的，自行选择候选；必要时读取当前canonical source。
候选都不合理时可以拒绝全部候选，不得为满足结果而强行遍历。
不要在每个Turn自动查询，不要自动重试，不要把结果持久化为Agent Context或图事实。
不要把现有semantic-query作为默认Agent自查询入口。
```

在后续`coordinate show`命令交付前，Agent可按Coordinate类型使用已有canonical owner read surface，
或显式使用当前`incident`读取图结构；本计划不新增批量full-body读取。

基础命令表加入`coordinate-search`。`semantic-query`命令仍存在，但稳定Project Space不再指导Agent用它完成
默认起点发现；只有Human明确要求完整语义路径查询时才可显式使用。

## 11. 代码影响面与文件拆分

### 11.1 新增文件

建议新增：

- `crates/buzz-semantic-query/src/coordinate_search.rs`
  - request/result/observations、constants、canonicalization与invariants；
- `crates/buzz-semantic-query/src/coordinate_search_query_text.rs`
  - 独立query template、digest与encoder input；
- `crates/buzz-sdk/src/semantic_coordinate_search.rs`
  - exact-body builder、virtual Event builder/parser；
- `crates/buzz-db/src/semantic_coordinate_search.rs`
  - ticket adapter、Coordinate-only exact recall、current indexed eligibility与typed rows；
- `crates/buzz-relay/src/semantic_coordinate_search.rs`
  - deadline-aware orchestration；
- `migrations/0059_project_context_coordinate_search.sql`。

避免继续扩大已经很大的`buzz-db/src/semantic_query.rs`与Relay bridge。共享权限/fleet/currentness helper如需抽取，
应形成窄的private API并保留现有semantic graph回归，不复制一套容易漂移的安全逻辑。

### 11.2 修改文件

预期修改：

- `crates/buzz-semantic-query/src/lib.rs`：导出新closed合同；
- `crates/buzz-core/src/kind.rs`、`filter.rs`：40913、virtual-kind分类和普通Filter拒绝；
- `crates/buzz-sdk/src/lib.rs`：导出新SDK模块；
- `crates/buzz-db/src/lib.rs`、migration/readiness/schema drift；
- `schema/schema.sql`：0059 fresh-install parity；
- `crates/buzz-db/src/event.rs`与所有ordinary read guards；
- `crates/buzz-search/src/query.rs`：virtual kind排除；
- `crates/buzz-relay/src/api/bridge.rs`：只增加raw dispatch seam；
- `crates/buzz-relay/src/nip11.rs`、config/status：独立capability；
- `crates/buzz-relay/src/handlers/req.rs`及COUNT/search/fan-out gates；
- `crates/buzz-admin/src/semantic.rs`：readiness/enable/disable；
- `crates/carryforth-cli/src/lib.rs`、`commands/project_context.rs`、client transport；
- `crates/carryforth-cli/TESTING.md`；
- `crates/buzz-acp/src/project_space.rs`、`base_prompt.md`。

### 11.3 明确不修改

- `SemanticGraphQuery`和`SemanticGraphQueryResult`字段；
- current path ranking/floors/root/traversal/retention；
- Desktop Tauri/React/semantic overlay；
- Project Context Edge/Coordinate领域模型；
- semantic source extractor或embedding schema；
- full source read commands；
- graph mutation/attach/detach授权。

## 12. 分阶段实施顺序

### Phase A0：冻结合同与影响面（已交付）

交付：

- request/result/query-text常量与closed JSON fixtures；
- kind 40913、extension、NIP-11 capability和gate命名；
- error matrix、limit/body/deadline常量；
- 本计划与现有semantic plan的ACP supersession链接。

退出门：

- 纯合同tests先红后绿；
- 现有SemanticGraphQuery fixtures字节不变；
- 无实现代码能够在feature-off时广告新capability。

### Phase A1：Pure contract、Core kind与SDK（已交付）

交付：

- `buzz-semantic-query` Coordinate-search request/result/query-text；
- kind 40913 closed classification；
- SDK exact-body builder和Relay-signed result verifier；
- virtual Event ingest/read denial pure gates。

退出门：

- canonicalization、digest、sorting、truncated和unknown-field矩阵通过；
- wrong signer/caller/project/request/NIP-98/body/binding/tag/content均fail closed；
- kind 40912现有SDK tests不回归。

### Phase A2：Schema、DB ticket与Coordinate-only recall（已交付）

交付：

- migration 0059与schema parity；
- shared Community egress gate与Coordinate-specific runtime readiness；
- Coordinate-only eligible CTE；
- exact top-(limit+1)、typed hydration、current indexed eligibility；
- 复用egress/release fences的窄private API。

退出门：

- binding-only Document即使score最高也不占用top-K；
- Document双角色只返回一次；
- graph-external/无active Edge/stale head被排除；
- context revision/generation/auth race fail closed；
- migration upgrade/fresh/drift/rollback-readiness tests通过。

### Phase A3：Relay orchestration、HTTP与NIP-11（已交付）

交付：

- one-input Provider flow；
- raw exclusive filter parser；
- 45秒deadline与body caps；
- signed 40913 response；
- independent capability/master + shared Community egress gate；
- shared admission/fleet/rate control；
- low-cardinality content-free metrics。

退出门：

- 一次成功或失败请求的Provider call count最多1；
- capability/gate off时Provider call count为0；
- 不取得traversal semaphore、不materialize Edge/path；
- malformed/mixed/ordinary 40913 filter fail closed；
- no redirect/no retry/body cap/timeout tests通过。

### Phase A4：Admin与Carryforth CLI（已交付）

交付：

- readiness/enable/disable；
- `cf project-context coordinate-search`；
- NIP-11/project/relay identity解析；
- exact-body one-shot transport；
- JSON/compact/help/closed error输出；
- `TESTING.md`更新。

退出门：

- CLI输出无Edge/path/title/summary/read_commands；
- capability缺失时不发HTTP；
- 429/503/timeout不自动重放；
- wrong Relay result不输出候选；
- query原文不出现在Debug/error/test logs。

### Phase A5：ACP入口替换（已交付）

交付：

- Project Space contract version/hash更新；
- base prompt命令表更新；
- 默认起点发现从`semantic-query`切到`coordinate-search`；
- not-every-turn、candidate-not-fact、canonical reread与reject-all规则。

退出门：

- 稳定prompt不再指导Agent把`semantic-query`作为默认入口；
- 已知Coordinate时明确跳过search；
- no automatic retry/query/persist规则有contract tests；
- Role/Work不会由Harness暗中注入request。

### Phase A6：资格、灰度与回滚（本地单Relay已完成；生产资格未关闭）

交付：

- labeled start-coordinate evaluation；
- exact SQL target-scale EXPLAIN和并发测量；
- one authorized non-sensitive canary；
- capability/gate-on和完整gate-off zero-egress证据；
- rollout/rollback记录。

退出门：

- 关键标注场景top-8至少包含一个可接受起点；
- overall Recall@8、各Coordinate类型/语言cohort、重复稳定性有版本化报告；
- cross-Role overlap只作为候选质量指标记录，不伪装成hard scope；
- Provider、DB和HTTP p95/p99门槛在目标环境冻结并通过；
- enable前后、revoke-first、release-first、rollback时序证据完整；
- 未完成上述门时保持feature-off，不宣称production ready。

当前实证状态：

- 4个非敏感中英文前/后端case均在top-1命中预登记的可接受Role/Work集合；
- 前后端top-8交集为6，Jaccard为0.6，证明入口能定向首位，同时诚实保留候选重叠成本；
- PostgreSQL 17.10、pgvector 0.8.5、10k active indexed Coordinates的production SQL测量已完成；
- 本地授权Human、真实Provider、真实Relay签名/SDK验证链4/4成功；
- gate关闭后capability撤下，CLI fail closed，Provider调用计数保持4→4；
- `just test-unit`、`just test`与`just ci`最终全部通过；
- 目标部署SLO尚未冻结，多Pod/load-balancer soak与更大版本化标注集尚未执行，因此继续feature-off。

## 13. 测试矩阵

### 13.1 Pure contract tests

- query空白、全空白、NUL、中文UTF-8边界、16KiB±1；
- limit 0/1/8/32/33；
- UUID nil/v1/v4、project mismatch；
- unknown field、duplicate known JSON key；
- noncanonical request field order/whitespace可接受且request binding覆盖exact bytes；
- noncanonical signed result content必须拒绝；
- exact query-text escaping、raw Unicode、digest稳定性；
- one request只产生一个encoder input；
- rank连续、score顺序、同分canonical tie-break、Coordinate去重；
- truncated与request limit/返回长度组合；
- result count超过request limit；
- response size cap；
- forbidden Edge/path/summary字段无法反序列化。

### 13.2 DB/integration tests

- active Coordinate source进入eligible；
- binding-only Document score最高仍不进入distance候选；
- Document Coordinate + binding双角色只返回一次；
- graph-external source排除；
- Coordinate从最后一条active Edge detach后排除；
- terminal但仍在active Edge且canonical eligible的Coordinate可返回；
- tombstone/deleted/unauthorized/stale head排除；
- wrong generation/space/model/dimension/zero vector排除；
- SQL在Coordinate role gate之后计算distance；
- limit+1精确产生truncated；
- query向量不落库；
- Project Context revision在Provider前变化、Provider后read前变化、snapshot后release前变化；
- membership/ban/gate/fleet在各阶段变化；
- migration 0059 upgrade/fresh schema/drift/readiness；
- events表直接插入40913被DB constraint拒绝。

### 13.3 Relay/security tests

- raw filter exact keys和值；
- mixed semantic/coordinate-search extension拒绝；
- 0/2 filters、wrong kind/author/p、limit、extra key拒绝；
- ordinary REQ/COUNT/NIP-50/by-id/kindless对40913 fail closed；
- client submit、import、fan-out、subscription均拒绝40913；
- pre-parse授权顺序不产生schema oracle；
- exact NIP-98 `u/method/payload/nonce`与event id；
- reserialization/change-one-byte导致binding失败；
- wrong signer/caller/project/request id/tag order/content canonicality；
- success 64KiB、error 16KiB Content-Length/chunked双重cap；
- redirect/connect/429/503/timeout/body-transfer都最多一次Provider/HTTP尝试；
- deployment master/shared Community gate/NIP-11 off时零Provider egress；
- Provider call后revoke不签result；
- shared admission证明coordinate-search不能绕过semantic query rate debt；
- content-free metrics不含query、Coordinate UUID、title或summary。

### 13.4 Algorithm behavior tests

- direct cosine顺序与brute-force reference一致；
- 不调用root candidate score、context gain或MMR；
- 不取得traversal semaphore；
- 不执行Edge/Document/target SQL；
- 前端/后端语义相近时可同时返回，结果不声称hard Role scope；
- 同query、同generation/context revision重复调用排序稳定；
- 改变query可改变score/order但不改变Coordinate identity规则；
- active-edge Coordinate缺少current queryable head时只排除该项，其余eligible candidates仍可返回；
- empty graph/index返回空候选且不伪造“无答案”；
- 缺失Coordinate和空数组都不能被解释为“不相关”或“不存在”。

### 13.5 CLI/ACP tests

- Clap command/help/default/hard cap；
- capability/project identity/Relay self解析；
- JSON和compact仅whitespace不同；
- Result只含closed DTO；
- no read_commands/title/summary/Edge/path；
- HTTP只发送一次；
- query不进入Debug/error；
- Project Space contract version/hash变化；
- prompt包含coordinate-search、not-every-turn、candidate-not-fact、reject-all；
- prompt不再把semantic-query作为默认Agent自查询入口；
- Agent已知精确Coordinate时跳过search；
- 没有Role/Work hidden request injection。

### 13.6 真实资格矩阵

建立版本化、非敏感标注集，每个case包含：

```text
case_id
language/cohort
natural-language query
acceptable_start_coordinates[]
unacceptable-but-similar coordinates[]
graph/context/semantic generation fixture identity
```

至少覆盖：

- 中英文与混合语言；
- 前端/后端共享术语但不同可接受起点；
- 同一Issue/Stage连接的多Work；
- Requirement、Work、Issue、Role、Resource、Document、Meeting；
- 多个都合理的起点；
- 没有强相关对象但仍会返回nearest candidates的case；
- summary缺失/title-only；
- high-degree与多Island图；
- source更新、detach、tombstone、generation切换。

主要报告：Recall@1/3/8、MRR、候选重复率、类型分布、相似Role重叠、index readiness、latency和Provider成本。
Recall@8是入口能力主指标；precision和cross-Role overlap用于理解Agent后续筛选成本，不把它们误写成权限失败。

## 14. 可观测性、隐私与成本

### 14.1 允许记录的low-cardinality指标

- capability/gate/readiness状态；
- request success/error category；
- wall/provider/DB/postflight latency histogram；
- requested/returned count；
- truncated布尔；
- Provider admission wait/busy/timeout；
- result byte bucket；
- zero-result count；
- virtual-kind rejection count。

### 14.2 禁止记录

- query原文或可逆query摘要；
- query embedding/vector；
- title、summary、正文、preview；
- Coordinate UUID、Edge key、Document id；
- exact HTTP body、NIP-98 Authorization header、private key；
- signed result content；
- per-Community高基数query label。

request_id只能用于短期请求关联，不能和problem原文组成持久日志。若需要重复性诊断，使用预先登记的
非敏感case_id，而不是对生产query做离线导出。

本能力的Provider egress只包含固定Coordinate-search模板和自然语言query；不外发Role字段、Agent身份、
Coordinate、overview、title、summary、正文、Edge或拓扑。Provider当然会收到query原文，这是该显式功能的
必要边界；capability/gate关闭、caller无权或final egress recheck失败时必须在Provider之前停止。

### 14.3 成本边界

每个请求固定：

- 1个Provider query item；
- 1次exact Coordinate-only recall；
- 最多32个返回项；
- 无Edge/path traversal；
- 无自动retry；
- 无分页/cursor；
- 无结果cache。

不能因为比路径查询便宜就取消共享Provider rate gate。

## 15. 风险与禁止反例

### 15.1 主要风险

1. **nearest不等于正确**：相似Role/Work仍可能同时出现；这是候选发现的预期边界；
2. **Agent过度信任rank**：必须由prompt和后续canonical read约束；
3. **先recall后过滤**：binding Documents会占满top-K，必须SQL前置Coordinate role；
4. **共享gate误解**：Community gate有意授权两类semantic query出域；具体surface由各自deployment master和
   NIP-11 capability控制，紧急`query-disable`会同时关闭两者；
5. **限流旁路**：独立capability不能获得独立Provider吞吐；
6. **virtual kind泄漏**：40913必须像40912一样在所有ordinary路径fail closed；
7. **snapshot被误称current lease**：结果只表示签发snapshot，后续使用前仍需canonical reread；
8. **prompt提前扩张**：本计划不能暗中实现Role结构化过滤或自动多轮遍历；
9. **文件继续膨胀**：DB/Relay/bridge必须拆模块，不能把实现全部追加到现有大文件；
10. **false no-match**：v1没有校准floor，不得根据低score自动宣称无相关Coordinate。

### 15.2 禁止实现

- 调用`semantic-query`再删除Edges/paths；
- 从现有semantic roots提取Coordinates；
- 用Context Document root冒充Coordinate；
- 先对Coordinates+Documents做top-K后再过滤；
- 把Agent Role/Work自动注入query或图；
- 把Coordinate incident neighborhood当作隐藏加分；
- 让caller传score weight/floor/model/vector；
- 在CLI里自动读取所有候选正文；
- 自动对429/timeout重试；
- 将40913持久化以便稍后读取；
- 在能力关闭时仍发送query给Provider；
- 用Relay签名声称项目文本真实或安全；
- 把返回候选自动写入Agent memory、Project Document或新Edge。

## 16. 质量命令与交付证据

实现阶段至少运行与记录：

```bash
. ./bin/activate-hermit

cargo test -p buzz-semantic-query
cargo test -p buzz-core
cargo test -p buzz-sdk semantic_coordinate_search
cargo test -p buzz-db --lib semantic_coordinate_search
cargo test -p buzz-relay --lib semantic_coordinate_search
cargo test -p carryforth-cli coordinate_search
cargo test -p buzz-acp project_space

just test-unit
just test
just ci
```

Tauri不在root Cargo workspace，但本计划不修改Desktop，因此无需Desktop截图或Playwright。若实现意外触及Desktop，
必须停止并重新确认范围，不能把它顺带纳入。

资格报告至少回填：

```text
commit / migration / runtime digest
feature master / Community gate / NIP-11 before-after
virtual kind ordinary-read denial matrix
Provider call count / no-retry proof
DB exact-query EXPLAIN / buffers / p50-p95-p99
labeled-set version / Recall@1-3-8 / MRR / cohort breakdown
authorized canary content-free result counts
rollback gate-off / capability-off / zero-egress proof
all commands run / skipped / failed
```

## 17. 最终验收标准

只有以下全部成立，才可称代码交付完成：

1. 新request/result与现有SemanticGraphQuery完全独立；
2. `coordinate-search`只返回ranked Coordinates与content-free envelope metadata；
3. 所有候选都来自当前active Edge的Coordinate角色；
4. binding-only Documents、graph-external来源和stale heads不会占用top-K；
5. 排名是一个query vector对Coordinate overview的direct cosine；
6. 没有root、context gain、MMR、Edge或path逻辑；
7. Provider egress与snapshot有current source fences，release重验caller/gates/fleet/generation/Context结构，
   且结果明确只声明snapshot；
8. exact-body/NIP-98/request binding/Relay signer由SDK完整验证；
9. virtual kind 40913不可能进入普通Nostr存储和读取面；
10. 新capability与deployment master默认关闭并可独立撤下；共享Community gate默认关闭，
    `query-disable`可紧急停止两类semantic query egress；
11. 新入口与现有路径查询共享物理rate/fleet安全边界；
12. CLI单次请求、不自动重试、不泄露query；
13. ACP不再把路径查询作为默认Agent自查询入口；
14. Agent prompt明确候选不是事实，允许拒绝全部候选；
15. 现有`semantic-query`、Desktop和Project Context领域回归全绿；
16. schema/migration/readiness/fresh-install parity通过；
17. scoped tests、`just test-unit`、`just test`与`just ci`结果有记录。

只有Phase A6真实资格、显式enable和rollback证据也完成，才可进一步声明某个Community可启用。
在此之前，代码完成仍必须描述为：

> Coordinate-only Agent start search implemented, feature-off, not production-ready.

## 18. 后续计划边界

本计划完成后，Agent拥有“用自然语言选择起点”的能力。后续应分别设计并交付：

1. 查看一个Coordinate的current metadata与summary，但不默认加载完整source；
2. 获取一个Coordinate的incident Edge identities；
3. 查看一条Edge绑定的Document metadata/summary；
4. 查看一条Edge的完整Coordinate set；
5. ACP中`Coordinate → Edge → Coordinate`的有界、自主、去循环遍历指导。

这些能力不得反向扩张本计划的result。特别是Coordinate search不能因为后续遍历方便而提前返回Edge、Document、
summary或path。
