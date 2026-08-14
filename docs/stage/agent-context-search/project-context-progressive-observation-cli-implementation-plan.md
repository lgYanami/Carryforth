# Agent Project Context 渐进观察与一跳语义选择分阶段实现计划

> 状态：实施中；Phase B0–B1及B3 storage-deny前置已交付；one-hop semantic仍feature-off；不构成production-ready声明
>
> 日期：2026-08-14
>
> 代码基线：`feat/agent-context-search` @ `596287200`（B0）；B1 结构观察在本计划内增量交付
>
> 已交付起点检索：
> [Agent Project Context 自然语言 Coordinate 起点检索分阶段实现计划](project-context-coordinate-search-implementation-plan.md)
>
> 起点检索资格：
> [Coordinate 起点检索资格记录](project-context-coordinate-search-qualification.md)
>
> 已交付有界路径查询：
> [Project Context 图语义检索分阶段实现计划](../semantic/project-context-graph-semantic-query-implementation-plan.md)
>
> 图领域规范：
> [Project Context V2 领域规范](../project-context/project-context.md)
>
> 独立架构事项：
> [统一 Project Context 语义检索引擎 TODO](../TODO.md)
>
> 历史废弃设计：
> [已废弃的 Project Context Agent 渐进检索实现设计](../meeting/context/project-context-progressive-retrieval-implementation-design.md)
>
> 本计划范围：Carryforth CLI 的原子结构观察；两个一跳语义选择 CLI；独立 closed request/result；
> current semantic index上的结构限域exact cosine；Relay签名virtual result；generic HTTP `/query`；
> SDK verifier；权限、currentness、资源门、feature-off灰度与资格
>
> 明确排除：Agent如何维护上下文环境、visited set、分支、循环、停止条件与最终回答；ACP/Project Space/
> base prompt调整；统一语义检索引擎重构；现有`coordinate-search`或`semantic-query`行为调整；自动遍历器、
> traversal session、路径DTO；Edge embedding；正文embedding；Desktop/Web UI

本文交付的边界是：

```text
选择起点：      coordinate-search（已交付）
轻量观察：      coordinate show
结构选边：      coordinate edges
语义选边：      coordinate edge-search
关系证据：      edge documents
完整成员：      edge coordinates
语义排成员候选：edge coordinate-search
```

CLI完成后，另立计划设计Agent如何把自己的Role、Work、Requirement、Issue、会议目的和当前任务写入query，
如何维护visited集合并形成`Coordinate → Edge → Coordinate`的渐进路径。本计划不得提前把这些策略固化进
CLI、Relay或ACP。

## 0. 已确认的产品与架构决策

### 0.1 产品操作边界

1. 已交付的`cf project-context coordinate-search`继续只负责全图自然语言起点候选发现；本计划不修改其
   wire、query template、排名、Provider、capability、结果或资格声明；
2. 已交付的`cf project-context semantic-query`继续作为有界多跳retrieval forest查询；它不再是Agent自查询
   起点入口，本计划不修改其Q0/Qi、root、floor、MMR、beam、retention或结果；
3. Coordinate取得Edge与Edge取得Coordinate是两个独立操作，不能让一个查询顺便返回下一层结构；
4. 新增两个一跳语义CLI：

   ```bash
   cf project-context coordinate edge-search <TYPE:UUID> --query <TEXT> [--limit 8]
   cf project-context edge coordinate-search <EDGE_KEY> --query <TEXT> [--limit 8]
   ```

5. `coordinate edge-search`只在输入Coordinate的active incident Edges范围内，以这些Edge绑定的current
   Context Documents做语义评分；返回Edge identities和排序后的Document候选identities/revisions/scores，
   绝不返回Edge Coordinates；
6. `edge coordinate-search`只在输入active Edge的完整成员集合内，以成员Coordinate的current semantic
   overview做语义评分；返回排序后的Coordinate候选identities/scores，绝不返回Edge Documents或canonical
   full-member structure DTO；当成员很少且全部可评分时，候选identity可能恰好覆盖全部成员，但这不替代
   `edge coordinates`的canonical结构观察；
7. 两个语义结果都是带canonical轻量观察的选择投影，不是完整结构读取。完整结构分别由
   `edge documents`和`edge coordinates`读取；单个Coordinate仍可由`coordinate show`复核；
8. 每个返回候选必须同时携带current canonical `title/name`、可用时的`description`、source-owned
   `summary`、lifecycle/status、typed revision/currentness provenance以及source-basis-bound读取descriptor。这里的
   `preview`就是语义result的一部分：Agent可以直接结合自己已知的Role、Work、Issue、会议目的和当前任务，
   使用这些字段排除语言相关但上下文不合适的对象，不能只拿identity和score继续盲目遍历；
9. 候选preview来自typed canonical source hydration，不能用embedding input、缓存文本或模型生成摘要伪造。
   完整Document正文、Project View完整对象、Meeting Board/speech与raw Event仍由返回的canonical read入口按需
   获取；只有需要完整内容或把内容作为事实/证据时才必须继续读取，而不是为了初步筛选候选就强制读取；
10. 现有`incident`保留为Human/兼容诊断命令，本计划不增强、不删除、不改变其现有输出。当前Agent提示仍会
   使用`incident`；只有后续独立prompt/traversal计划交付后，Agent默认流程才切换到这些原子CLI；
11. 结构读取进一步拆为：

    ```bash
    cf project-context coordinate show <TYPE:UUID>
    cf project-context coordinate edges <TYPE:UUID> [paging]
    cf project-context edge documents <EDGE_KEY> [paging]
    cf project-context edge coordinates <EDGE_KEY>
    ```

12. Coordinate、Edge、Document、path不被强塞进一个弱类型result。两个新语义操作可以共享一套严格tagged
    one-hop wire family，但每个variant拥有独立closed request/result与ranking contract；
13. 全仓统一语义检索引擎是`docs/stage/TODO.md`中的独立架构调整。本阶段不迁移现有两条生产查询，
    不引入动态query DSL，也不把临时helper宣称为通用engine。

### 0.2 排名与成本边界

1. 两个新操作每次只构造一个problem-only Q0输入，只允许一次Provider embedding调用；
2. 首版复用现有semantic graph冻结的Q0 canonical query-text与query compatibility fences，但不进入
   SemanticGraphQuery root/traversal orchestrator；
3. `coordinate edge-search`：

   ```text
   DocumentScore = direct_cosine(Q0, current relation Document overview)
   EdgeScore     = max(DocumentScore for that Edge)
   ```

4. `edge coordinate-search`：

   ```text
   CoordinateScore = direct_cosine(Q0, current Coordinate overview)
   ```

5. 不使用Qi/environment gain、anchor、neutral quota、kind weight、MMR、local coherence、relation/target/
   transition floor、harmonic score、beam、path retention或邻域传播；
6. 不累加或平均一条Edge的Document分数，避免绑定Document更多的Edge天然占优；
7. 首版无relevance floor。结果是有界top-K候选，不是“确实相关”的事实证明；全零分仍可稳定返回；
8. `limit`只控制返回Edge或Coordinate数量，默认8，范围`1..=32`；不暴露document limit、recall、模型、
   weight、floor、lifecycle、deadline、response bytes或vector参数；
9. 每条返回Edge最多携带固定3个ranked Documents；更多current scorable Documents通过
   `documents_truncated=true`表达，完整列表由`edge documents`读取；
10. CLI、SDK与Relay不得自动重试、redirect、fallback到其他语义接口或组合执行另一条one-hop query。

### 0.3 信任与持久化边界

1. CLI向选定Relay提交closed request，其中包含显式自然语言query、Project request identity和一个
   Coordinate或Edge scope identity；Relay只向Provider外发自然语言query及固定Q0 contract，绝不向Provider
   外发Coordinate、Edge key、Role、Work、Project identity、Document text、overview、拓扑或正文；
2. Community级自然语言query出域授权复用现有`semantic_graph_query_enabled`；不新增第二个Community列；
3. 新one-hop surface拥有独立deployment master和NIP-11 capability，默认关闭；
4. 两个CLI共享现有物理Provider admission/rate debt/query semaphore，不能通过新增surface扩大总吞吐；
5. query、canonical Provider input、vector和result不写入CLI/SDK/Relay持久存储、普通日志、数据库、URL或
   query cache；允许当前进程内存、stdout和调用它的当前Agent turn瞬时消费result；
6. signed virtual result只存在于当前HTTP response，禁止入库、普通REQ/COUNT/NIP-50/by-id/kindless/Redis/
   live fanout；
7. 语义索引只参与排名，不能替代canonical Project Context topology或source currentness验证；
8. 所有命令都是read-only；本计划不新增mutation，也不改变`attach`/`detach`。

一句话合同：

> 一跳语义查询只在调用者明确给定的结构范围内选择候选；它不创建结构、不返回下一层完整结构、不形成路径，
> 也不替Agent理解其上下文环境。

## 1. 当前实现、可复用基础与缺口

### 1.1 当前CLI

当前`project-context`已有：

```text
coordinate-search
semantic-query
exact
incident
contains-all
attach
detach
```

当前`incident`一次返回incident Edges、每条完整Coordinates以及全部Context Documents。这对Human诊断有用，
但不符合Agent逐步决定“先选Edge，再选Coordinate”的操作边界。

### 1.2 可直接复用的语义基础

现有Foundation和query实现已经提供：

- Project View、Project Document、Meeting current source overview；
- active semantic generation、model contract、dimension、embedding-space与query fences；
- source change后的job/rebuild/activate/current-head流程；
- host-derived Community/Project、caller、membership/ban和Project Context read authorization；
- Provider reservation、wait、Community writer fence下的最终egress confirm；
- 共享物理Provider client、rate debt与query semaphore；
- `REPEATABLE READ READ ONLY` semantic snapshot；
- current-head exact cosine与fixed-point `Score(0..=1_000_000)`；
- exact hit的typed canonical hydration、title/description/summary/lifecycle/status与source provenance recheck；
- Coordinate的incident Edge/Binding候选加载；
- EdgeKey到完整current Hyperedge及member sources的加载；
- generation/context/permission release postflight；
- exact-body NIP-98、request binding和Relay-signed virtual result verifier。

因此本阶段不创建第二份embedding、不修改extractor、不重建semantic index，也不新增ANN。

### 1.3 不能直接复用的路径策略

现有relation/target traversal ranking不能直接作为新CLI结果：

- relation ranking加入entered Coordinate↔Document coherence、environment gain和`RELATION_FLOOR`；
- target ranking依赖已选relation Document，加入Document↔target coherence、transition score和floors；
- traversal结果还经过beam、visited/path、endpoint retention与response packing。

新一跳CLI必须只复用候选加载、current-head和exact cosine kernel，新增窄的scoped ranking方法，不调用上述策略。

### 1.4 需要补齐的能力

| 需求 | 新CLI | 结果边界 |
|---|---|---|
| 轻量看Coordinate | `coordinate show` | 一个canonical source observation |
| 列出Coordinate的Edge | `coordinate edges` | EdgeKey + binding count，不含Docs/Coords |
| 在Coordinate邻域语义排Edge | `coordinate edge-search` | ranked Edge + ranked canonical Document observations，不含Coords |
| 查看一条Edge的关系证据 | `edge documents` | canonical Document metadata/summary，不含Coords |
| 查看一条Edge完整成员 | `edge coordinates` | complete Coordinates，不含Docs |
| 在一条Edge内语义排Coordinate | `edge coordinate-search` | ranked Coordinate + canonical lightweight observations，不含Docs |

候选preview已经足以进行第一轮上下文筛选；只有候选保留后需要完整内容或证据时，才继续复用：

```bash
cf project-view get-object <type> <uuid>
cf documents get <uuid> --revision <revision> [--content-only]
cf meetings show --meeting <uuid>
cf meetings board get --meeting <uuid>
cf meetings history --meeting <uuid> --limit <N>
cf resources guide <resource_uuid> [--revision N] [--content-only]
```

## 2. CLI合同

### 2.1 命令树

保留flat兼容命令`coordinate-search`，新增两个nested read groups：

```text
cf project-context coordinate show <TYPE:UUID>
cf project-context coordinate edges <TYPE:UUID> [--limit 32] [continuation]
cf project-context coordinate edge-search <TYPE:UUID> --query <TEXT> [--limit 8]

cf project-context edge documents <EDGE_KEY> [--document <UUID> | --limit 32 [continuation]]
cf project-context edge coordinates <EDGE_KEY>
cf project-context edge coordinate-search <EDGE_KEY> --query <TEXT> [--limit 8]
```

`--format compact`仍是全局参数。所有Coordinate token复用现有parser：

```text
project_profile / goal / role / plan / stage / requirement / issue / work / resource / document / meeting
```

EdgeKey严格为64字符小写hex。不得新增title lookup、模糊名称、别名或第二套Coordinate parser。

### 2.2 `coordinate show`

只返回目标Coordinate的current canonical轻量source observation：

```json
{
  "project_id": "uuid-v4",
  "snapshot": {
    "context_meta_event_id": "hex64",
    "context_revision": 42,
    "projection_generation": 9
  },
  "coordinate": {
    "coordinate": {
      "coordinate_type": "project_view_object",
      "object_type": "work",
      "object_id": "uuid-v4"
    },
    "state": "active",
    "title": "Authorization UI",
    "summary": "Client-side authorization checks",
    "object_revision": 7
  }
}
```

目标必须至少属于一条current active Edge；graph-external或已detach返回`not_found`。该命令不返回Edge、
Context Documents、semantic score、正文或preview。

`coordinate show`与`edge coordinates`复用同一个新closed `ProjectContextCoordinateObservation`输出合同；不修改
现有`incident/exact/contains-all`的legacy `CoordinateOutput`：

| source/state | 必有字段 | 条件字段 | 禁止字段 |
|---|---|---|---|
| Project View active | `coordinate,state,title,object_revision,updated_at,updated_by,read_command` | `description,summary,status`按11类现有v3 typed值存在时输出 | document/meeting fields |
| Document active | `coordinate,state,title,document_revision,updated_at,updated_by,fetch_command` | canonical Document metadata `summary`存在时输出 | object/meeting fields |
| Meeting active/terminal | `coordinate,state,title,status,meeting_fetch` | `description,summary,updated_at`存在时输出 | object/document revisions |
| tombstoned | `coordinate,state`及该source已验证revision/time/actor | 无content字段 | fetch/preview |
| unavailable | `coordinate,state,unavailable_reason` | 无 | revision/content/fetch |

所有不存在的条件字段必须省略，不输出`null`。`meeting_fetch`严格为现有typed
`{metadata,board,speech}`命令。Project View `read_command`固定为
`cf project-view get-object <type> <uuid>`；Document `fetch_command`固定pin observed revision。Document
Coordinate的summary来自唯一Project Document metadata owner，不能由embedding text补齐。11类Project View
`status`各自沿用现有v3 typed serializer，并为每一类固定golden。

### 2.3 `coordinate edges`

返回当前Coordinate的Edge identities，不返回Docs或Coordinates：

```json
{
  "project_id": "uuid-v4",
  "snapshot": {
    "context_meta_event_id": "hex64",
    "context_revision": 42,
    "projection_generation": 9
  },
  "coordinate": { "coordinate_type": "project_view_object", "object_type": "role", "object_id": "uuid-v4" },
  "edges": [
    { "edge_key": "hex64", "binding_document_count": 4 }
  ],
  "page": {
    "limit": 32,
    "next_after_edge_key": null,
    "truncated": false
  }
}
```

`--limit`默认32、范围`1..=32`。分页按EdgeKey升序，cursor exclusive。continuation必须携带上一页
完整snapshot tuple：

```text
--after-edge <EDGE_KEY>
--expected-context-meta-event-id <EVENT_ID>
--expected-context-revision <REVISION>
--expected-projection-generation <GENERATION>
```

四个参数全有或全无。`truncated=true`时`next_after_edge_key`必须等于本页最后一个EdgeKey；
`truncated=false`时必须为`null`。任一snapshot字段变化返回non-retryable conflict，不自动从第一页开始；
授权通过后先比较snapshot tuple，再检查cursor是否仍属于scope，因此snapshot mismatch优先于cursor `not_found`。
设`remaining`为同一snapshot中cursor之后的Edge数，必须满足：

```text
edges.len() == min(page.limit, remaining)
page.truncated == (remaining > page.limit)
page.truncated == true  => edges非空且next_after_edge_key == edges.last.edge_key
page.truncated == false => next_after_edge_key == null
```

### 2.4 `edge documents`

只返回指定current active Edge的Context Documents：

```json
{
  "project_id": "uuid-v4",
  "snapshot": { "context_meta_event_id": "hex64", "context_revision": 42, "projection_generation": 9 },
  "edge_key": "hex64",
  "documents": [
    {
      "document_id": "uuid-v4",
      "state": "active",
      "title": "Authorization relation evidence",
      "summary": "...",
      "document_revision": 7,
      "fetch_command": "cf documents get <uuid> --revision 7 --content-only"
    }
  ],
  "page": { "limit": 32, "next_after_document_id": null, "truncated": false }
}
```

`--document <UUID>`用于精确读取一条已知binding，不能与`--limit`或continuation参数组合。列表模式的
`--limit`默认32、范围`1..=32`；按Document UUID升序，cursor exclusive：

```text
--after-document <DOCUMENT_UUID>
--expected-context-meta-event-id <EVENT_ID>
--expected-context-revision <REVISION>
--expected-projection-generation <GENERATION>
```

四个continuation参数全有或全无，并绑定上一页完整Context snapshot。active Document的fetch command必须
pin observed revision；metadata unavailable时`fetch_command`省略，不能生成unverified current read。

列表模式必须输出`page`；`truncated=true`时`next_after_document_id`等于本页最后一个Document UUID，
`truncated=false`时为`null`。`--document`精确模式保持同一envelope与`documents`数组，数组必须恰好一项且
省略`page`；binding不存在为`not_found`。snapshot mismatch同样先于cursor/binding `not_found`。

`ContextDocumentObservation`是closed DTO：

| observation | 必有字段 | 条件字段 | 禁止字段 |
|---|---|---|---|
| active | `document_id,state,title,document_revision,updated_at,updated_by,fetch_command` | canonical metadata `summary`存在时输出 | `unavailable_reason` |
| unavailable | `document_id,state,unavailable_reason` | 无 | title/summary/revision/time/actor/fetch |

`state`只允许`active | unavailable`；`unavailable_reason`首版只允许`metadata_unavailable`，所有不存在字段省略而
非`null`。若active Context binding指向verified tombstoned/deleted Document，整次读取返回
`verification_failed`，不能把损坏关系降级成普通tombstone observation。

列表模式设`remaining`为同一snapshot中cursor之后的active binding数，必须满足：

```text
documents.len() == min(page.limit, remaining)
page.truncated == (remaining > page.limit)
page.truncated == true  => documents非空且next_after_document_id == documents.last.document_id
page.truncated == false => next_after_document_id == null
```

### 2.5 `edge coordinates`

只返回指定current active Edge的完整canonical Coordinate set及轻量source observations：

```json
{
  "project_id": "uuid-v4",
  "snapshot": { "context_meta_event_id": "hex64", "context_revision": 42, "projection_generation": 9 },
  "edge_key": "hex64",
  "coordinates": [
    {
      "coordinate": { "coordinate_type": "project_view_object", "object_type": "work", "object_id": "uuid-v4" },
      "state": "active",
      "title": "Authorization UI",
      "summary": "..."
    }
  ]
}
```

Hyperedge成员集合是原子事实，不分页、不partial serialize。若完整identity或最终输出超过硬限，整次失败。

### 2.6 `coordinate edge-search`

```bash
cf project-context coordinate edge-search role:<uuid-v4> \
  --query "授权预检、客户端错误展示和泄露防护" \
  --limit 8
```

CLI只打印SDK验签后的one-hop result `incident_edges` variant。每个ranked Document已含可直接用于上下文
筛选的canonical preview、currentness provenance与pinned `fetch_command`；CLI不另造unsigned wrapper、不输出
query、不在本地重新排名。

### 2.7 `edge coordinate-search`

```bash
cf project-context edge coordinate-search <hex64-edge-key> \
  --query "接下来需要处理的客户端实现工作" \
  --limit 8
```

CLI只打印SDK验签后的one-hop result `edge_coordinates` variant。每个ranked Coordinate已含可直接用于上下文
筛选的canonical preview、currentness provenance与source-owned读取入口。输入Edge必须current active；不存在
或已detach返回`not_found`，请求期间变化返回`conflict`。

## 3. One-hop closed request合同

### 3.1 Tagged request family

在`buzz-semantic-query`新增：

```text
ProjectContextOneHopSemanticQuery
├── request_id: UUIDv4
├── project_id: UUIDv4
├── query: String
├── limit: u8
└── scope: OneHopSemanticScope
    ├── incident_edges { coordinate: ProjectContextCoordinate }
    └── edge_coordinates { edge_key: EdgeKey }
```

JSON使用closed internally-tagged enum，`deny_unknown_fields`；两个variant不能同时出现，不能省略scope。
CLI两个命令只是该family的typed adapters，不提供调用者可提交的动态filter或plan。

### 3.2 Validation与资源门

冻结：

```text
DEFAULT_ONE_HOP_LIMIT                   = 8
MAX_ONE_HOP_LIMIT                       = 32
MAX_ONE_HOP_QUERY_BYTES                 = 16 KiB UTF-8
MAX_ONE_HOP_INNER_REQUEST_BYTES         = 64 KiB canonical inner request
MAX_ONE_HOP_EXACT_HTTP_BODY_BYTES       = 64 KiB raw authenticated filter array
MAX_ONE_HOP_PROVIDER_INPUT_BYTES        = 64 KiB
MAX_ONE_HOP_RESPONSE_BYTES              = 512 KiB Event array
MAX_ONE_HOP_WALL_TIME_MS                = 45_000
MAX_MATCHED_DOCUMENTS_PER_EDGE          = 3
MAX_INCIDENT_EDGES_MATERIALIZED         = 1_024
MAX_RELATION_DOCUMENTS_MATERIALIZED     = 2_048
MAX_EDGE_COORDINATES_MATERIALIZED       = 4_096
MAX_HYPEREDGE_IDENTITY_BYTES            = 64 KiB
```

Relay在generic bridge parse前以extension key的content-free byte scan识别该surface，并先对完整raw authenticated
filter-array body执行64KiB门；之后strict parse exclusive filter，再对canonical inner request独立执行64KiB门。
SDK builder也必须对即将发送与NIP-98 hash的同一份exact body bytes执行相同上限。request/parser必须：

- UUID为非nil v4；
- query执行`trim()`，trim后非空，拒绝NUL，按UTF-8 bytes检查16KiB；
- 不做Unicode normalization、翻译、lowercase或语言检测；
- limit为`1..=32`；
- Coordinate/EdgeKey canonical并与host-derived Project匹配；
- duplicate known fields与unknown fields拒绝；
- raw field order/whitespace可非canonical；SDK builder始终输出唯一canonical bytes；
- canonical Provider input与最终result分别检查自己的byte cap。

`Debug`、error和metrics只记录operation、bytes、counts、低基数状态，不记录query、scope identities、vector、
title或summary。

### 3.3 Q0 query-text合同

两个variant把`query`映射为现有semantic graph冻结的problem-only Q0 canonical input：

```json
{"contract":"semantic-graph-query.problem","problem":"<escaped canonical query>"}
```

实现应从`query_text.rs`暴露/复用一个纯Q0 builder，不构造完整`SemanticGraphQuery`，不生成Qi。结果观察同时
绑定现有`query_contract_digest`和variant-specific `ranking_contract_digest`。

本阶段不把已发布的40913 Coordinate-search template迁移到Q0，也不把Q0迁移到40913；统一四种query-text
属于独立引擎TODO和相关性兼容迁移。

### 3.4 Closed error合同

one-hop HTTP错误固定为closed
`{code,message,retryable,retry_after_seconds?}`；unknown/duplicate字段拒绝。`message`不得包含query、scope
identity、Project text、Provider body或完整caller identity。code只允许：

```text
invalid_input / unsupported / restricted / not_found / busy / conflict / timeout
scope_too_large / hyperedge_too_large / response_too_large
unavailable / verification_failed / internal
```

唯一HTTP/CLI映射冻结为：

| HTTP code | status | `CliError` | exit | retryable |
|---|---:|---|---:|---|
| `invalid_input` | 400 | `Usage` | 1 | false |
| `unsupported` | 400 | `Usage` | 1 | false |
| `restricted` | 403 | `Auth` | 3 | false |
| `not_found` | 404 | `NotFound` | 1 | false |
| `busy` | 429 | `Unavailable` | 2 | true |
| `conflict` | 409 | `Conflict` | 5 | false |
| `timeout` | 504 | `Unavailable` | 2 | true |
| `scope_too_large` / `hyperedge_too_large` / `response_too_large` | 413 | `Usage` | 1 | false |
| `unavailable` | 503 | `Unavailable` | 2 | true |
| `verification_failed` / `internal` | 500 | `Other` | 4 | false |

retryable code可带`1..=3600`的`retry_after_seconds`，但CLI仍不得自动retry。所有映射输出content-free固定文案，不把Relay
body原样打印。`not_found`只允许在粗粒度授权通过后用于不存在、已detach或不属于scope的对象。

## 4. One-hop closed result合同

### 4.1 公共observations

```text
ProjectContextOneHopSemanticResult
├── request_id
├── project_id
├── request_binding_digest
├── observations
│   ├── semantic_generation_id
│   ├── source_generation_contract_digest
│   ├── embedding_space_fence
│   ├── query_contract_digest
│   ├── ranking_contract_digest
│   ├── projection_generation
│   ├── project_context_revision
│   └── snapshot_observed_at
└── selection: OneHopSemanticSelection
```

结果是Relay-signed RR snapshot observation，不声明response到达时持续current。所有score使用现有
`Score(0..=1_000_000)`。

### 4.2 Canonical candidate observation与semantic result preview

每个returned Document/Coordinate必须携带同一RR snapshot内typed hydration得到的
`CanonicalCandidateObservation`。该对象不是另一次读取的提示，而是one-hop semantic result自身必须返回、
SDK必须验签的候选信息：

```text
CanonicalCandidateObservation
├── source_basis                         # typed Project View/Document/Meeting currentness
├── source_invalidation_epoch
├── source_snapshot_digest
├── lifecycle
├── source_status?
├── preview
│   ├── title
│   ├── description?
│   └── summary?
└── canonical_read
    ├── project_view { command, expected_object_revision }
    ├── document { fetch_command, expected_document_revision }
    └── meeting { metadata, board, speech, expected_create_event_id, expected_end_event_id? }
```

规则：

- `preview`是语义结果的必有候选观察；Agent可以立即读取它完成第一轮上下文筛选，不需要先执行
  `canonical_read`；
- 对CLI消费者而言，这个typed canonical `preview`就是本命令唯一的semantic candidate preview；不得再并列
  生成一份LLM摘要、缓存excerpt或embedding-text preview；
- `preview`只能复制source-owned canonical字段；不存在的`description/summary/status`省略，不输出`null`；
- `description`来自typed Project View或Meeting current observation；Project Document无独立description时省略；
- `summary`必须完整复制；不得截断、LLM改写或用embedding input补齐；若完整signed result超过512KiB，整次
  `response_too_large`，调用者可显式降低`--limit`；
- `canonical_read`由identity与typed basis确定，SDK必须逐字符重算并验证。Document command必须pin
  `document_revision`；Project View/Meeting命令不支持revision flag时，descriptor中的expected basis是readback
  check，不得伪装成命令本身已pin；
- preview用于候选筛除和决定是否继续读取；score只解释语言相关性，preview则让Agent判断候选是否属于当前
  Role、Work、Issue、会议目的或任务上下文。二者都不能单独证明权限、方向、因果或完整source事实；
- `canonical_read`不是使用preview的前置条件，而是读取完整正文/对象、核对变化或引用证据的后续入口；
- result不内联Document完整body、Project View完整object、Meeting Board/speech或raw Event，也不返回Provider
  input/vector。内部用于embedding的overview若与canonical preview不同，不得冒充canonical字段或作为第二份
  summary owner。

排名完成后只hydrate最终returned candidates，但hydration仍在同一RR transaction内，并重新验证source identity、
current basis、invalidation epoch、snapshot digest与semantic head。任一returned candidate不能得到完整closed
observation时，整次`verification_failed`；不得降级为identity-only、用缓存preview代替，或偷偷换入下一名。

### 4.3 `incident_edges` result variant

```json
{
  "request_id": "uuid-v4",
  "project_id": "uuid-v4",
  "request_binding_digest": "hex64",
  "observations": {
    "semantic_generation_id": "uuid-v4",
    "source_generation_contract_digest": "hex64",
    "embedding_space_fence": "hex64",
    "query_contract_digest": "hex64",
    "ranking_contract_digest": "hex64",
    "projection_generation": 9,
    "project_context_revision": 42,
    "snapshot_observed_at": "2026-08-14T00:00:00Z"
  },
  "selection": {
    "selection_type": "incident_edges",
    "coordinate": { "coordinate_type": "project_view_object", "object_type": "role", "object_id": "uuid-v4" },
    "edges": [
      {
        "rank": 1,
        "edge_key": "hex64",
        "score": 863300,
        "ranked_documents": [
          {
            "rank": 1,
            "document_id": "uuid-v4",
            "document_revision": 7,
            "score": 863300,
            "canonical_observation": {
              "source_basis": {
                "family": "project_document",
                "basis": { "document_revision": 7, "source_change_id": "hex64" }
              },
              "source_invalidation_epoch": 12,
              "source_snapshot_digest": "hex64",
              "lifecycle": "active",
              "source_status": "active",
              "preview": {
                "title": "Authorization relationship",
                "summary": "Explains how the client and service split authorization checks."
              },
              "canonical_read": {
                "read_type": "document",
                "fetch_command": "cf documents get <uuid> --revision 7 --content-only",
                "expected_document_revision": 7
              }
            }
          }
        ],
        "binding_document_count": 4,
        "scorable_document_count": 1,
        "documents_truncated": false
      }
    ],
    "coverage": {
      "active_incident_edges": 2,
      "active_relation_bindings": 5,
      "scorable_relation_bindings": 1,
      "scorable_edges": 1,
      "title_only_scorable_bindings": 0,
      "omitted_relation_bindings": {
        "source_not_found": 0,
        "source_tombstoned_or_deleted": 0,
        "source_ineligible_or_unreadable": 1,
        "semantic_head_missing": 1,
        "semantic_head_building": 1,
        "semantic_head_failed_or_unsupported": 1,
        "non_queryable_zero_vector": 0
      }
    },
    "truncated": false
  }
}
```

验证不变量：

- `edges.len() == min(request.limit, scorable_edges)`，rank从1连续；
- Edge按`score DESC, edge_key ASC`；
- 每个Edge的`ranked_documents.len() == min(3, scorable_document_count)`；
- Document按`score DESC, document_id ASC`，rank从1连续且Document ID唯一；
- `edge.score == ranked_documents[0].score`；
- `binding_document_count >= scorable_document_count >= ranked_documents.len()`；
- `documents_truncated == (scorable_document_count > 3)`；
- `selection.truncated == (scorable_edges > request.limit)`，仅表示同一snapshot存在第`limit+1`条
  scorable Edge；
- `active_relation_bindings`按唯一`(edge_key, document_id)`计数；
- `scorable_relation_bindings + sum(omitted_relation_bindings) == active_relation_bindings`；
- `scorable_edges`是至少拥有一个scorable binding的唯一Edge数；
- 缺embedding计coverage，不计truncation；
- 每个ranked Document必须有完整`canonical_observation`；其Project Document basis revision、显式
  `document_revision`和pinned `fetch_command`三者一致；
- 结果绝不含`coordinates`、Document body、raw Event、内部embedding overview或binding content。

### 4.4 `edge_coordinates` result variant

```json
{
  "request_id": "uuid-v4",
  "project_id": "uuid-v4",
  "request_binding_digest": "hex64",
  "observations": { "...": "same closed observations" },
  "selection": {
    "selection_type": "edge_coordinates",
    "edge_key": "hex64",
    "ranked_coordinates": [
      {
        "rank": 1,
        "coordinate": { "coordinate_type": "project_view_object", "object_type": "work", "object_id": "uuid-v4" },
        "score": 841230,
        "canonical_observation": {
          "source_basis": {
            "family": "project_view",
            "basis": { "schema_version": 3, "object_revision": 7, "source_change_id": "hex64" }
          },
          "source_invalidation_epoch": 22,
          "source_snapshot_digest": "hex64",
          "lifecycle": "active",
          "source_status": "active",
          "preview": {
            "title": "Authorization UI",
            "description": "Implement client-side authorization checks and failure presentation.",
            "summary": "Frontend behavior for authorization and disclosure-safe errors."
          },
          "canonical_read": {
            "read_type": "project_view",
            "command": "cf project-view get-object work <uuid>",
            "expected_object_revision": 7
          }
        }
      }
    ],
    "coverage": {
      "edge_coordinate_count": 5,
      "scorable_coordinates": 1,
      "title_only_scorable_coordinates": 0,
      "omitted_coordinates": {
        "source_not_found": 0,
        "source_tombstoned_or_deleted": 0,
        "source_ineligible_or_unreadable": 3,
        "semantic_head_missing": 1,
        "semantic_head_building": 0,
        "semantic_head_failed_or_unsupported": 0,
        "non_queryable_zero_vector": 0
      }
    },
    "truncated": false
  }
}
```

验证不变量：

- ranked Coordinates全部属于该snapshot中的完整active Edge；
- `ranked_coordinates.len() == min(request.limit, scorable_coordinates)`，rank从1连续、Coordinate唯一；
- 排序为`score DESC, canonical Coordinate Ord ASC`；
- `edge_coordinate_count >= scorable_coordinates >= ranked_coordinates.len()`；
- `scorable_coordinates + sum(omitted_coordinates) == edge_coordinate_count`；
- `truncated == (scorable_coordinates > request.limit)`，仅表示同一snapshot存在第`limit+1`个scorable member；
- 起点Coordinate也可能被返回；循环/visited过滤属于后续Agent策略；
- 每个ranked Coordinate必须有与identity、typed source basis和semantic head一致的完整
  `canonical_observation`；
- 结果绝不含canonical full-member structure DTO、Documents、完整source body/raw Event、内部embedding
  text或next-hop建议。

coverage用于解释候选池，不能冒充结构DTO。若Edge很小且全部member可评分，`ranked_coordinates`可能恰好
列出全部member identities；这仍不携带完整source observations，也不替代`edge coordinates`的canonical
full-member读取。

### 4.5 Empty、coverage与oversize语义

- 输入Coordinate不属于任何current active Edge时，三个Coordinate结构/语义命令统一返回`not_found`；
- 有Edge但无current scorable relation Document：成功返回空Edges，coverage说明原因；
- active Edge不存在：`edge coordinate-search`为`not_found`；
- active Edge成员均不可评分：成功返回空Coordinates；
- 空结果不能解释为“不相关”“对象不存在”或“无权限”；
- 任一scope候选超过物化硬限：`scope_too_large`整体失败，不打分任意前缀；
- 完整Hyperedge identity超过硬限：`hyperedge_too_large`整体失败；
- result超过512KiB：`response_too_large`整体失败，不静默删减candidate/coverage或canonical preview；
- `truncated=false`只表示所有scorable候选已返回，不表示所有canonical候选可评分或答案正确。

每个active relation binding或Edge member必须按下列优先级恰好进入一个互斥bucket：

```text
source_not_found
→ source_tombstoned_or_deleted
→ source_ineligible_or_unreadable
→ semantic_head_missing
→ semantic_head_building
→ semantic_head_failed_or_unsupported
→ non_queryable_zero_vector
→ scorable
```

`title_only_scorable_*`是`scorable_*`的子集，不是额外bucket。还必须满足：

```text
title_only_scorable_bindings <= scorable_relation_bindings
scorable_edges <= active_incident_edges
title_only_scorable_coordinates <= scorable_coordinates
```

每条返回Edge的`binding_document_count`与`scorable_document_count`来自同一次完整Edge分组，不能由packed
Document slice反推。

## 5. Canonical scope、DB ranking与currentness

### 5.1 Ticket与事务顺序

两种operation统一遵循：

```text
host/caller粗鉴权
  → closed request validation
  → process query permit
  → Provider readiness
  → semantic query ticket（generation + Project Context observations）
  → shared Provider reservation
  → wait
  → Community writer fence下最终egress confirm
  → exactly one Q0 Provider call
  → validate model/dimension/order/finiteness/norm/fences
  → REPEATABLE READ READ ONLY
  → compare initial ticket vs RR ticket projection_generation + project_context_revision
  → canonical scope load + current-head exact scoring
  → commit snapshot
  → release postflight
  → signed virtual Event
```

reservation不代表授权。membership/ban、Project read、Community gate、deployment master、fleet、generation或
routing在wait期间变化时，必须零Provider egress。

Provider后发生generation或Context topology observation变化时，fail closed；不得再次调用Provider，不自动
重放query。source state/head首次在Provider后的RR snapshot中观察，因此RR开始前的source churn可由该snapshot
吸收，不伪称为conflict。

`begin_semantic_graph_read`仍先执行现有generation contract检查。返回`SemanticGraphReadTx`后、任何scope/source
load前，one-hop orchestrator必须比较初始egress ticket与`read_tx.ticket()`的
`projection_generation + project_context_revision`；任一不等立即rollback并返回`conflict`。这专门关闭
Provider egress至RR之间的topology窗口，不改变40912/40913合同。

### 5.2 Coordinate → Edge DB合同

在`SemanticGraphReadTx`新增窄的scoped方法：

1. 验证输入Coordinate canonical且属于至少一条current active Edge；
2. 完整枚举有界的`(edge_key, active binding Document, binding/edge provenance)`；
3. 若Edge或relation refs超过硬限，返回scope-too-large，不取任意前缀；
4. 去重Document semantic sources，加载current states/heads；
5. 只用Q0 direct cosine评分，不计算entered↔Document coherence或relation floor；
6. 将每个score重新join并验证exact current binding provenance；
7. 先对全部scorable Documents评分，再按Edge分组；
8. 每Edge内部stable sort，EdgeScore取最佳Document；
9. 最后执行Edge K+1和每Edge固定3 Documents packing；
10. 对packed Documents在同一RR内执行typed canonical hydration，补齐preview/provenance/pinned read descriptor；
11. 生成完整coverage，不把missing/building/failed混成“无关系”。

不得先取全局top Documents再分Edge，否则一条多Document Edge会挤掉其他Edge。

### 5.3 Edge → Coordinate DB合同

1. 以EdgeKey读取一个current active完整Hyperedge，验证Edge/binding/topology provenance；
2. 完整成员数或identity bytes超限时整体失败；
3. 固定`LifecycleFilter::AllCurrent`，terminal member与其他current member一起参与；不暴露lifecycle参数；
4. 将每个member映射为其真实semantic source；relation-only Document不得混入；
5. 加载每个member的current state/head；
6. 只用Q0 direct cosine评分，不要求selected relation Document，不计算D↔V coherence、target或transition floor；
7. stable sort后执行Coordinate K+1；
8. 对packed Coordinates在同一RR内执行typed canonical hydration，补齐title/name、可用description、summary、
   lifecycle/status、provenance与read descriptor；
9. 输出rank/score/canonical lightweight observation与coverage，不输出完整Edge。

### 5.4 Snapshot与release语义

- 所有候选枚举、current head、distance、grouping和coverage来自同一RR snapshot；
- topology真相来自canonical active Edge/Binding表，不从semantic source roles反推Edge；
- semantic generation、projection generation和Context revision必须在release postflight保持；
- release复用现有principal/gate/fleet/generation/projection generation/Context revision fence；首版不新增
  source-head arrival-time recheck；
- RR前source churn由当前snapshot吸收，RR后source变化不改写已签snapshot，但结果不得称为arrival-time current；
- 结果仍只称`snapshot_observed_at`，不是持续授权lease或arrival-time current证明；
- 后续canonical read若发现revision/head已变，应重新开始当前一跳，不能静默读取新内容解释旧score。

## 6. HTTP、SDK、virtual Event与能力广告

### 6.1 一个严格tagged one-hop wire family

两条CLI共享一个新generic `/query` extension：

```text
carryforth_project_context_one_hop_semantic_search
```

共享一个response-only virtual Event kind：

```text
40914 = KIND_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_RESULT
```

共享marker：

```text
carryforth-project-context-one-hop-semantic-search-result
```

这是两个已冻结variant的closed family，不是统一检索engine或开放DSL。40912 semantic forest、40913 global
Coordinate search与40914 one-hop selection三种schema互不兼容，不得混用。

`scope`固定使用`scope_type`作为tag。SDK canonical exact-body golden的filter key顺序与形状为：

```json
[
  {
    "kinds": [40914],
    "authors": ["relay-pubkey-hex64"],
    "#p": ["caller-pubkey-hex64"],
    "limit": 1,
    "carryforth_project_context_one_hop_semantic_search": {
      "request_id": "uuid-v4",
      "project_id": "uuid-v4",
      "query": "natural language",
      "limit": 8,
      "scope": {
        "scope_type": "incident_edges",
        "coordinate": {
          "coordinate_type": "project_view_object",
          "object_type": "role",
          "object_id": "uuid-v4"
        }
      }
    }
  }
]
```

另一variant的`scope`只能是`{"scope_type":"edge_coordinates","edge_key":"hex64"}`。Relay-signed result
Event的tag顺序和值严格为：

```text
["p", <authenticated-caller-hex64>]
["request_id", <request-uuid-v4>]
["request_binding", <digest-hex64>]
["t", "carryforth-project-context-one-hop-semantic-search-result"]
```

不允许额外、重复或重排tag；Event content是closed result的canonical JSON。

### 6.2 Exact HTTP与NIP-98

- request必须是exclusive single filter，`kinds=[40914]`且只含一个one-hop extension；
- 与40912、40913、另一普通filter、额外kind或多个extension混合全部拒绝；
- HTTP层先按完整raw exact body bytes限流，再deserialize exclusive filter与closed inner DTO；
- NIP-98绑定POST method、canonical Relay `/query` URL、exact body payload hash和auth Event ID；
- request binding domain覆盖exact body、NIP-98 auth Event ID、caller、Relay、host-derived Project、request ID和
  scope variant；
- Community/Project只能从host-derived identity获得；payload Project不匹配立即拒绝；
- Relay先验证认证与粗粒度Project Context read权限，再暴露scope-specific `not_found`，避免authorization oracle；
- success只返回一个Relay-signed Event；error body与success body各自有streaming cap；
- client/server不follow redirect、不自动retry。

### 6.3 Signed result verifier

SDK parser必须逐项验证：

- kind 40914、Relay signer、caller `p` tag、Project、request ID、result marker；
- exact NIP-98 Event、exact-body request binding和scope variant；
- canonical content bytes、unknown/duplicate fields、response size；
- generation/query/ranking/embedding fences；
- request/result coordinate或edge identity一致；
- variant-specific ordering、rank、counts、score、coverage与禁止字段；
- Document revision、EdgeKey、Coordinate形状和UUID/hex bounds；
- 每个returned candidate的typed source basis、canonical provenance、lifecycle/status、preview字段与read descriptor；
- title/description/summary只能出现在`canonical_observation.preview`，read command只能出现在匹配source family的
  `canonical_read`；
- 结果中没有query、完整body/object/Board/speech、raw source Event、内部embedding overview、Provider input或vector；
  canonical candidate `preview`则是必有并被exact验证的result字段。

CLI只能打印SDK verifier通过后的closed DTO，不能信任raw HTTP JSON。

### 6.4 Capability、master与fleet

新增：

```text
NIP-11: carryforth-project-context-one-hop-semantic-search-http
ENV:    CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE=false
```

capability只有在下列全部ready时广告：

- deployment master true；
- existing Community semantic query gate与semantic index ready；
- Provider、stable Relay signer、Project/Document/Meeting read dependencies ready；
- writer DB ticket/release路径ready；
- attested fleet全部支持40914 parser/handler/result/verifier/runtime digest。

新parser/handler/wire/result-kind/ranking contracts必须进入fleet runtime descriptor。mixed fleet时fail closed。部署顺序：

```text
全fleet部署feature-off兼容代码
→ migration/schema readiness
→ 新runtime attestation
→ 单Community短期开master/capability
→ canary
→ disable + revoke
→ 资格通过后再灰度
```

## 7. Virtual kind存储deny与migration

新增migration `0060_project_context_one_hop_semantic_search.sql`并同步`schema/schema.sql`。upgrade沿用0059的在线
模式，先`ADD CONSTRAINT ... NOT VALID`，再独立`VALIDATE CONSTRAINT`；fresh schema直接声明validated约束，
不得在单条upgrade DDL中对`events`做阻塞式全表验证：

```text
events_kind_not_project_context_one_hop_semantic_search_result CHECK(kind <> 40914)
```

同步：

- `buzz-core` kind定义、Relay-only/response-only classifier；
- DB upgrade/fresh schema/readiness/drift；
- client event submit/direct insert拒绝；
- ordinary REQ、COUNT、NIP-50、kindless、ids-only、HTTP/WS与Redis fanout拒绝；
- existing 40912/40913 nonleak回归。

40914持久化计数必须恒为0。

## 8. 结构读取的snapshot、source与输出合同

### 8.1 Snapshot tuple

结构分页统一使用：

```text
context_meta_event_id + context_revision + projection_generation
```

三者来自同一verified Context meta double-read。continuation中任一不匹配为non-retryable `conflict`；cursor
不存在或不属于scope为`not_found`；非法参数为`usage`。不能把两个snapshot的pages拼成一个邻域。
`context_meta_event_id`必须是canonical小写hex64，`context_revision`为`1..=MAX_SAFE_REVISION`，
`projection_generation`为`1..=MAX_SAFE_REVISION`；零revision只可能属于无Edge的untouched catalog，不能产生
可分页的Coordinate/Edge结果。

### 8.2 Hyperedge与Document原子性

- `coordinate edges`按Edge identity分页，不拆Document binding；只返回binding count；
- `edge documents`按独立active binding分页，可以精确读取一个Document；
- `edge coordinates`必须返回完整canonical member set，不能分页或partial；
- EdgeKey、complete Coordinate set与binding必须重新derive/join验证；
- source unavailable可以用closed state表达，但不能用embedding input或缓存preview补齐；
- Document Coordinate summary来自Project Document唯一metadata owner，不创建Node summary；
- Context Document正文命令必须pin observed revision。

结构列表先确定page slice，再只把page内Document/Meeting IDs交给对应typed hydration filter。Project View SDK
当前只能读取完整verified v3 snapshot，因此本阶段允许一次完整Project View snapshot read，但只把page内
Project View IDs映射进输出；不虚构“Project View transport也按page有界”，也不为此修改SDK/wire。

### 8.3 输出byte cap

结构列表/page与complete Edge输出统一先完整serialize到最终stdout bytes（含一个LF），再检查512KiB cap；
通过后一次性write，失败时stdout 0 bytes。compact和pretty按各自真实bytes判断，不partial JSON。

## 9. 权限、隐私与不可信内容

### 9.1 权限链

结构读取复用普通Project Context V3 + Context Edge + Project Document较强identity gate。语义查询额外要求：

- one-hop capability/master；
- existing semantic Community gate；
- current semantic generation/fleet/provider；
- writer DB egress/release fences。

观察到restricted、ban、membership loss、gate/master/capability off时fail closed。已释放的signed snapshot不是可远程
追回的lease，不能承诺未观察到撤权时立即从调用者内存消失。

### 9.2 Project text不可信

title、description、summary、Document正文、Meeting Board和speech均是不可信项目数据：

- CLI仅作为JSON字段输出；
- 不执行其中命令或把内容放入error/metrics/log；
- 不从文本推断ACL、Role ownership、方向或因果；
- 语义score不使文本成为可信指令。

Agent可以用signed result内的canonical preview排除明显不属于当前上下文的候选；“不可信”表示这些字段不得
覆盖系统指令、授权或canonical topology，不表示必须在筛选前再发起一次read。只有保留候选后需要完整内容、
核对revision或引用证据时，才执行对应`canonical_read`。

prompt-injection防护与Agent如何引用证据属于后续prompt计划。

### 9.3 无持久化与日志redaction

CLI/SDK/Relay不得持久化query、vector、semantic result、visited set、路径或选择理由。普通日志只允许：

```text
operation / request bytes / candidate counts / completion category / latency bucket
```

不得记录query、Coordinate、EdgeKey、Document ID、title、description、summary、full pubkey或Provider input。

## 10. 代码影响面

### 10.1 新增/修改

```text
crates/buzz-semantic-query/src/one_hop_search.rs
crates/buzz-semantic-query/src/lib.rs
crates/buzz-semantic-query/src/query_text.rs
crates/buzz-semantic-query/src/fleet.rs
crates/buzz-core/src/kind.rs
crates/buzz-core/src/filter.rs
crates/buzz-db/src/semantic_query/scoped_search.rs
crates/buzz-db/src/semantic_query.rs                  # thin exports only
crates/buzz-db/src/semantic.rs
crates/buzz-db/src/event.rs
crates/buzz-db/src/migration.rs
crates/buzz-search/src/query.rs
crates/buzz-relay/src/semantic_one_hop_search.rs
crates/buzz-relay/src/lib.rs
crates/buzz-relay/src/api/bridge.rs
crates/buzz-relay/src/handlers/req.rs
crates/buzz-relay/src/handlers/count.rs
crates/buzz-relay/src/handlers/event.rs
crates/buzz-relay/src/handlers/ingest.rs
crates/buzz-relay/src/config.rs
crates/buzz-relay/src/main.rs
crates/buzz-relay/src/nip11.rs
crates/buzz-relay/src/router.rs
crates/buzz-relay/src/semantic_fleet.rs
crates/buzz-sdk/src/semantic_one_hop_search.rs
crates/buzz-sdk/src/lib.rs
crates/carryforth-cli/src/lib.rs
crates/carryforth-cli/src/client.rs
crates/carryforth-cli/src/commands/mod.rs
crates/carryforth-cli/src/commands/project_context.rs
crates/carryforth-cli/src/commands/project_context_observation.rs
crates/carryforth-cli/src/commands/project_view_snapshot.rs
crates/carryforth-cli/TESTING.md
crates/buzz-admin/src/semantic.rs
migrations/0060_project_context_one_hop_semantic_search.sql
schema/schema.sql
relevant readiness/drift/qualification scripts
```

现有`client.rs`已有两份相似semantic one-shot路径；本阶段允许抽一个仅限HTTP/NIP-98/no-redirect/bounded-body的
内部generic helper，前提是40912/40913 exact-body tests逐字节不变。不得借机抽全仓semantic engine。

### 10.2 明确不修改

```text
semantic source extractor / overview contract
semantic embedding tables / worker jobs
Project Context canonical Edge model
existing 40912/40913 request/result schemas
existing semantic-query ranking/traversal
desktop/
web/
admin-web/
crates/buzz-acp/**
```

如果实现要求修改现有查询结果或统一engine，必须停止并回到`docs/stage/TODO.md`独立设计。

## 11. 分阶段实施

### Phase B0：冻结CLI、wire与goldens

交付：

- nested command tree和结构/语义结果禁止字段；
- tagged one-hop request/result；
- canonical candidate preview/provenance/read descriptor closed DTO；
- 40914/extension/marker/capability/master命名；
- Q0与两个ranking digests；
- resource/error闭集；
- 现有40912/40913/incident行为goldens。

退出门：closed DTO pure tests先通过；没有动态DSL；没有ACP/prompt diff；统一engine仍只在TODO。

### Phase B1：原子结构观察CLI

交付：

- `coordinate show/edges`；
- `edge documents/coordinates`；
- full snapshot continuation；
- pinned Document reads；
- atomic stdout serializer。

退出门：零Provider调用；结构命令不跨层返回；existing incident/exact/contains-all/attach/detach不回归。

### Phase B2：One-hop pure contract与SDK

交付：

- request/result validators、redacted Debug；
- candidate preview/provenance/read descriptor exact validator；
- Q0 builder复用；
- ranking/coverage digests；
- exact HTTP builder/request binding；
- signed result Event builder/verifier；
- success/error byte caps。

退出门：恶意variant、unknown/duplicate fields、wrong identity/signer/body/kind全部fail closed。

### Phase B3：40914 storage deny与readiness

交付：kind classifier、migration、fresh schema、readiness/drift、ordinary query/ingest/fanout deny。

安全实施顺序：B3必须先于B2公开SDK Event builder落地，避免阶段提交期间出现“能够构造40914、但普通
ingest/storage尚未fail closed”的窗口；该依赖顺序不改变B2/B3各自产品范围。

退出门：40914 persistence count 0；40912/40913全部nonleak回归继续通过。

### Phase B4：DB scoped exact ranking

交付：

- incident relation Docs direct Q0 scoring + Edge grouping；
- complete Edge members direct Q0 scoring；
- current-head/provenance verification；
- packed candidates的same-RR typed canonical hydration；
- K+1、coverage、hard caps；
- target-scale EXPLAIN与statement timeout。

退出门：不调用coherence/floor/transition/path代码；相同fixture纯公式可重算；scope外候选为0。

### Phase B5：Relay execution与feature-off capability

交付：ticket、permit、shared Provider reservation、one-call encoding、RR、release、signed Event、HTTP dispatch、
NIP-11/fleet/config/status/admin readiness。

退出门：master/gate/fleet/auth拒绝路径Provider egress 0；success/error/churn均至多一次；mixed fleet不广告。

### Phase B6：Carryforth CLI接线

交付：两个semantic CLI、capability preflight、one-shot no-retry transport、SDK parse、JSON/compact打印。

退出门：CLI不本地排名、不回显query；candidate canonical observation/read descriptor必须来自验签result，
不得在CLI用未验证数据补齐；variant字段严格隔离。

### Phase B7：回归、资格与回滚

交付：scoped/full tests、真实授权Provider canary、结构/语义 paired fixture、性能证据、feature-off rollback、资格记录。

退出门：目标环境SLO与multi-pod未完成前保持feature-off，不宣称production-ready。

## 12. 测试矩阵

### 12.1 CLI parse与结构输出

- 11类Coordinate token、EdgeKey、UUIDv4、limit/cursor边界；
- nested help且mutation不包装成Agent read；
- `coordinate edges`绝不含Documents/Coordinates；
- `edge documents`绝不含Coordinates；
- `edge coordinates`绝不含Documents；
- Document active pinned command，unavailable无command；
- 首次CLI invocation输出的snapshot/cursor原样输入第二次invocation，两页无重叠且union完整；
- revision/generation相同但meta Event ID替换时continuation conflict，且Document/Meeting/PV hydration调用为0；
- complete Hyperedge、multi-Document、shared Coordinate、detach race；
- pretty/compact各自按最终UTF-8 bytes加唯一LF测试512KiB刚好/超1边界；
- 注入writer断言刚好边界只发生一次完整write，超1时write调用0次且buffer为空。

### 12.2 Request/query-text pure tests

- trim/blank/NUL/中文UTF-8 16KiB；canonical inner与完整raw exact HTTP body分别测试64KiB门；
- duplicate/unknown fields、variant混合/缺失；
- UUID/Project/Coordinate/EdgeKey/limit；
- Q0 escaping、Unicode、digest和exact Provider input；
- Debug/error/log无query/scope identity；
- 一次request只产生一个encoder input。

### 12.3 Incident Edge ranking

- Coordinate只枚举active incident Edges；
- relation-only Documents作为评分候选但不变成Coordinate；
- detached/inactive/other Edge Documents排除；
- 同Edge多Docs、同分tie、最佳Doc决定EdgeScore；
- 先全量评分再分组，不能global Doc top-K后分组；
- fixed3 Docs、documents_truncated、Edge K/K+1；
- missing/building/failed/title-only/zero-vector coverage；
- 同一binding满足多个表象条件时按冻结优先级唯一分类，coverage等式严格成立；
- 无floor，全零分稳定返回；
- Document title/summary/revision/provenance与pinned fetch command逐字段golden；
- 两个语言高度相似、但分别证明前端/后端关系的Document可同时被召回；result内的canonical title/summary/status
  足以让调用者区分候选，测试不得先调用`documents get`才完成判断；
- returned Document hydration失败时整次verification failure且不换入下一名；
- 每个returned Document都含完整canonical preview/provenance/read descriptor，且result绝不含Edge Coordinates或
  Document body；
- 1024/2048硬限前后与scope-too-large。

### 12.4 Edge Coordinate ranking

- EdgeKey读取一个current active完整Hyperedge；
- 只评分complete members，不混relation-only Doc或其他Edge Coordinates；
- Project View/Document/Meeting members；
- direct cosine、stable tie、K/K+1；
- missing/building/failed/title-only/zero-vector coverage；
- 同一member满足多个表象条件时按冻结优先级唯一分类，coverage等式严格成立；
- 起点Coordinate可再次出现；
- Project View/Document/Meeting的title、可用description、summary、lifecycle/status、basis与read descriptor
  variant goldens；
- 两个词汇高度重叠、但Role/Work上下文不同的Coordinate可同时被召回；result内的canonical preview保持各自
  source语义，不被score或embedding overview改写，测试无需额外source read即可做候选初筛；
- returned Coordinate hydration失败时整次verification failure且不换入下一名；
- 每个returned Coordinate都含完整canonical preview/provenance/read descriptor，且result绝不含Documents、
  canonical full-member structure DTO或完整source body；
- nonexistent/detached/tombstoned/churn；
- 4096 members/64KiB identity hard caps。

### 12.5 Auth、Provider与race

- capability/master/Community gate/index/provider/fleet/signer矩阵；
- nonmember、ban、wrong caller/project/relay、auth ordering；
- reservation后revoke/master-off/gate-off/generation change零egress；
- success、429、503、timeout、disconnect、redirect均Provider call<=1；
- Provider wrong count/model/dimension/order/nonfinite/norm/oversize；
- generation/topology在egress前、Provider后、RR后、release前变化；
- source churn在RR前被当前snapshot吸收，RR后变化不伪造arrival-time current且canonical reread可要求重启；
- permit/cancellation/timeout释放，transaction rollback，无后台retry。

### 12.6 HTTP/SDK/wire

- exact-body NIP-98 method/url/payload/event ID；
- 两个scope canonical inner JSON、exact outer filter key order与四个result tags逐字节golden；
- request binding覆盖variant和scope；
- exclusive filter，混40912/40913/ordinary/两个filter拒绝；
- wrong kind/marker/signer/caller/request/project/body/result variant；
- canonical result bytes、unknown/duplicate/oversize；
- rank/order/count/score/coverage/truncation恶意变体；
- query/body/raw Event/内部embedding overview字段注入拒绝，同时缺失canonical candidate preview也拒绝；
- title/description/summary移到`canonical_observation.preview`之外、read command与source family/basis不匹配时拒绝；
- preview中的NUL/oversize/unknown字段与伪造revision/digest/invalidation epoch拒绝；
- 512KiB边界保留完整summary，超限整次失败且不partial write或省略summary。

### 12.7 Virtual-kind nonleak

- client submit、WS EVENT、DB direct insert；
- ordinary kind filter、kindless、ids、REQ、COUNT、NIP-50、HTTP/WS；
- Redis/pubsub/live fanout；
- upgrade/fresh schema/readiness/drift；
- 40912/40913/40914分别保持response-only。

### 12.8 Regression与scope guard

- existing coordinate-search request/result/call-count/hash不变；
- existing semantic-query forest/ranking/Desktop consumer不变；
- incident/exact/contains-all/attach/detach不变；
- source extractor/index worker/rebuild不变；
- `crates/buzz-acp/**`无diff；
- 资格报告不得声称Agent已会防循环、选分支或生成不同路径。

## 13. 性能、灰度与质量门

### 13.1 性能资格

分别测：

- incident scope 1/32/1024 Edges与1/3/2048 relation Docs；
- Edge 2/32/4096 Coordinates；
- exact cosine EXPLAIN rows、buffers、temp spill、statement timeout；
- 4-client并发、共享Provider lane公平性、transaction age与vacuum；
- response packing、512KiB cap、cancellation；
- single-pod canary与production-like multi-pod routing。

未冻结目标SLO或未完成multi-pod时只能记录measurement，不得称ready。

### 13.2 Feature-off canary与rollback

canary顺序：

1. master false，NIP-11 capability absent，语义CLI零egress；
2. isolated Community启用existing semantic gate；
3. attested fleet ready后短期开one-hop master；
4. 运行两种operation的有界、无敏感文本fixture；
5. 使用一组语言高度相似、但Role/Work/Issue上下文不同的paired fixture，验证两边候选都携带可区分的canonical
   preview，且Agent无需额外read即可在结果层识别明显不合适的候选；
6. 验证score/coverage/canonical preview/provenance/read descriptors；
7. master off、fleet revoke、capability absent；
8. 再次调用语义CLI fail closed且Provider counter不增；
9. `coordinate show/edges`与`edge documents/coordinates`继续成功。

### 13.3 质量命令

实现阶段至少运行：

```bash
. ./bin/activate-hermit

cargo fmt --all -- --check
cargo clippy -p buzz-semantic-query -p buzz-core -p buzz-db -p buzz-sdk -p buzz-relay -p carryforth-cli --all-targets -- -D warnings
cargo test -p buzz-semantic-query --lib
cargo test -p buzz-core --lib
cargo test -p buzz-db --lib semantic_
cargo test -p buzz-sdk --lib semantic_
cargo test -p buzz-relay --lib semantic_
cargo test -p carryforth-cli project_context

just test-unit
just test
just ci
git diff --check
```

服务、Provider、target PostgreSQL或multi-pod未运行的资格项必须明确报告，不能以unit test代替。

## 14. 风险与禁止反例

### 14.1 主要风险

1. **把选择投影误当完整结构**：字段必须使用`ranked_documents`/`ranked_coordinates`和coverage/counts；
2. **复用路径ranking造成旧问题回流**：严禁coherence/floor/transition/retention；
3. **先截Doc再分Edge**：会让多Document Edge挤占结果；
4. **topology由semantic roles反推**：必须canonical join；
5. **source变化但Context revision不变**：只声明RR snapshot，后续canonical read发现变化时重新选择；
6. **新增surface绕过Provider限流**：必须共享物理admission/rate debt；
7. **40914泄漏入普通Nostr面**：全路径deny和migration不可省略；
8. **借局部复用启动统一engine迁移**：独立TODO未确认前不得改现有queries；
9. **CLI数量增加后Agent误用**：提示词设计留后续，但CLI help必须精确说明选择与观察边界。

### 14.2 禁止反例

不得：

- 在`coordinate edge-search`返回Edge Coordinates；
- 在`edge coordinate-search`返回Edge Documents或canonical full-member structure DTO；
- 在语义result省略returned candidate的canonical preview/provenance/read descriptor；
- 在语义result内联完整body/object/Board/speech、raw Event、内部embedding overview或Provider input；
- 在结构read返回semantic score或自动推荐；
- 使用Edge embedding或把Document分数求和；
- 复用RELATION/TARGET/TRANSITION floor、Qi、MMR或path代码；
- 对oversize scope取任意前缀后声称top-K；
- Provider失败后调用第二语义surface或自动重试；
- 从Coordinate/Edge payload推导Community；
- 让40914进入数据库、REQ、COUNT、NIP-50或fanout；
- 自动执行结果中的Document read；
- 在CLI/SDK/Relay持久保存或缓存query、vector、result或visited state；
- 修改ACP prompt宣称Agent已经会渐进遍历；
- 把本阶段helper命名或发布为统一semantic engine。

## 15. 最终验收标准

只有全部成立，才可称本阶段代码交付完成：

1. `coordinate show/edges`和`edge documents/coordinates`严格原子分层；
2. `coordinate edge-search`只返回ranked Edge，以及各Edge内ranked Document的identity/revision/score、canonical
   title/summary/status/currentness provenance和pinned read descriptor；它不返回Edge Coordinates；
3. `edge coordinate-search`只返回ranked Coordinate的identity/score，以及canonical title、可用description、
   summary、lifecycle/status/currentness provenance和typed read descriptor；它不返回Edge Documents或canonical
   full-member DTO；
4. 两个semantic variant各自只在调用者指定的一跳canonical scope内检索；
5. direct Q0 cosine可重算，无Qi/coherence/floor/MMR/path policy；
6. EdgeScore严格等于最佳Document score；
7. 一次显式调用最多一次Provider egress，无retry/redirect/fallback；
8. result coverage/truncation/empty/oversize语义closed且SDK exact验证；
9. generation/topology/auth release fences fail closed；source ranking与coverage严格来自一个RR snapshot且不冒充
   arrival-time current；
10. 40914 Relay-only、nonpersistent、nonqueryable，40912/40913不回归；
11. one-hop capability/master默认off，mixed fleet不广告；
12. source index/extractor/worker无需新表或rebuild；唯一schema变化是40914 storage deny；
13. existing coordinate-search、semantic-query和incident行为不变；
14. query、Provider input与vector不进入日志、缓存、URL、数据库或result；canonical candidate preview必须存在于
    signed result并只在当前调用者消费路径中使用，内部semantic overview不得冒充canonical preview；
15. Agent可以仅凭semantic result中的canonical preview完成候选初筛；完整正文/对象/raw Event仍按需通过typed
    read descriptor读取，结构read pin Document revision且不生成第二份summary owner；
16. scoped/full tests和feature-off rollback通过；
17. `crates/buzz-acp/**`无diff；
18. target SLO/multi-pod未通过时仍明确feature-off/non-production-ready；
19. 资格记录不宣称Agent traversal、循环防护或不同上下文路径已经交付；
20. 统一语义检索引擎仍作为独立TODO，不在本阶段暗中完成或部分迁移。

完成后的准确声明只能是：

> Carryforth已经提供原子结构观察和两个结构限域的一跳语义选择CLI；Agent如何组合这些CLI形成
> context-conditioned traversal仍未设计和交付。

## 16. 后续独立设计入口

### 16.1 Agent traversal/prompt计划

下一计划才讨论：

- Agent如何表示当前Role、Work、Issue、Requirement与会议目的；
- 何时使用global coordinate-search，何时已有可靠起点直接跳过；
- 如何措辞两个one-hop query；
- 如何检查不可信summary并读取pinned evidence；
- visited Coordinate/Edge/incidence集合与循环防护；
- branch/depth/call/full-content预算；
- backtrack、stop、uncertainty与证据轨迹；
- 不同上下文合理分叉和共享Issue/Stage自然汇合；
- ACP/base prompt调整与paired-context E2E。

### 16.2 统一语义检索引擎

四类语义surface的共同kernel、typed planner、迁移与扩展能力继续由
[Stage TODO](../TODO.md)单独立项。本计划只复用已存在的基础设施和一个closed one-hop family，不修改现有
生产查询语义。
