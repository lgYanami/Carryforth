# Project Context 图语义查询高跳 Packing 与数据库资源安全修复方案

> 状态：核心修复已实施并完成单请求受控复验；query gate 保持关闭，资源阶梯资格待完成
>
> 日期：2026-08-12
>
> 适用范围：本地单 Relay、Project Context 图语义查询、Desktop 当前支持面
>
> 当前发布结论：BLOCKED。默认查询的 packing 500 已定位并修复，离线 1–6 hop
> 合同通过；但 disposable 资源并发/故障注入和长期 soak 尚未完成，不能据此恢复常开查询
>
> 数据边界：本方案不新增、修改或重放 migration 0057 / 0058，不删除或重建向量，
> 不改写 Project、Project View、Document、Meeting、Project Context、Event、成员、
> Community、Desktop keyring 或 managed Agent identity，不删除 Docker volume
>
> 关联方案：`../project-context-graph-semantic-query-implementation-plan.md`、
> `../fix/trusted-single-relay-fleet-policy-implementation-plan.md`

## 0. 已确认结论

本次有三条相关、但不能互相替代的故障链：

1. **功能合同故障**：默认 3-hop 查询完成 Provider 编码和图遍历后，产生的 completed forest
   未通过 response packer 的结构校验，以 HTTP 500 结束；Relay 指标将 4 次稳定复现都归类为
   `packing / invalid_packing_input`；
2. **资源隔离故障**：验收负载窗口内，512 MiB PostgreSQL cgroup 达到内存和 swap 上限，
   一个 backend 被 OOM killer 终止，PostgreSQL 进入 crash recovery；现有证据只支持聚合负载和
   资源档位不匹配，不能把某一条 Meeting 或 semantic SQL 冒充为已证明的唯一元凶；
3. **错误分类故障**：PostgreSQL recovery 期间，row-zero Community host lookup 因连接池/数据库
   不可用而失败，但 Relay 把 lookup failure 和真正 unmapped host 都压成同一个 404，Desktop
   短暂显示永久性的“不支持/Compatibility required”；数据库恢复后相同 Community 自动恢复。

三条故障之间的关系是：

```text
3-hop traversal
  └─ completed forest 违反 packer/result validator 合同
       └─ HTTP 500 invalid_packing_input             [确定性功能故障]

Desktop + 多 Agent + Meeting + semantic 等聚合负载
  └─ PostgreSQL 512 MiB cgroup OOM / crash recovery [资源故障]
       ├─ semantic / ordinary reads 暂时 503
       └─ host lookup dependency failure被错误压成 404 [错误分类故障]
```

Packing 500 发生在 Stage C read transaction 提交以后；它不是 PostgreSQL OOM 的直接 SQL
原因。OOM 会产生另外的 503/短暂 404，但不能解释在数据库健康时稳定复现的 3-hop packing 500。

Fleet Attestation 阻断已经通过 `trusted-single-relay` 修复，并非当前原因。本方案不得回退该策略。
`feat/context-desktop` 的 full-canvas 提交只涉及 Desktop UI、测试和文档，也不是上述后端故障的
直接来源。

## 1. 现场证据与预防性只读检查

### 1.1 Packing 证据

已知对照矩阵：

| 查询变化 | 结果 | 结论 |
|---|---|---|
| 默认预算，3 hops | 稳定 HTTP 500 | 默认路径存在确定性合同故障 |
| `max_paths=1`、其余保持 3 hops | HTTP 500 | 不是多 path 数量本身 |
| `max_response_bytes=100000` | HTTP 500 | 不是 128 KiB 上限 |
| `max_response_bytes=65536` | HTTP 500 | 降低 response cap 不能规避 |
| 同一查询仅改为 2 hops | 成功，返回 6 roots / 1 path | 故障首次在更深路径形状出现 |

当前合同的默认值为 3 hops，调用方允许 1–6 hops，服务端硬上限为 6。3-hop 失败不能通过把默认值
或硬上限降到 2 来“修复”。在结构原因关闭前，3–6 hops 全部视为未通过资格；这不表示每一条
4–6 hop 查询一定失败，而是没有证据证明它们对所有合法图形状都安全。

Relay 当前累计指标：

```text
buzz_semantic_graph_query_errors_total{stage="packing",code="invalid_packing_input"} 4
buzz_semantic_graph_query_errors_total{stage="packing",code="response_too_large"} 1
```

其中 `response_too_large=1` 来自预算边界测试，与 4 次默认路径 failure 是不同、预期可分类的结果。
未观察到本轮 packing 的 Signing、RelaySignerChanged、SizeEstimateDrift 或 Serialization 错误。

代码路径会在大小裁剪前调用 `validate_prepacking_input()`，再通过
`SemanticGraphQueryResult::validate_for_request()` 校验 completed forest。因此降低 response cap
不会改变 `InvalidInput`。现有 HTTP mapper 把 `InvalidInput(String)` 和 signing / serialization 等
错误统一压成 `semantic graph response packing failed`，具体 invariant 没有被安全地保存。

### 1.2 OOM 与连接预算证据

事故窗口已确认：

- PostgreSQL container memory limit：512 MiB；
- container swap limit：1 GiB 总 memory+swap，即额外 512 MiB swap；
- cgroup 记录 `oom=1`、`oom_kill=1`；
- PostgreSQL backend 被 signal 9 终止并进入 crash recovery；
- 被杀 backend 当时显示 Meeting bootstrap SQL，但当前相关表规模很小，不能由此认定该 SQL 是根因；
- crash recovery 的约 37 秒内，PostgreSQL 日志累计约 1,372 次连接失败（约 37 次/秒）；Relay
  同窗口还有约 103 个 `/query` 开始记录，并叠加 Meeting、semantic worker、reminder 和 readiness
  重试。这是明确的恢复风暴和故障放大器，不是最初 OOM 的证明原因；
- 数据库自动恢复后，Project View、Documents、Context、目标 Edge 和 81/81 semantic heads 仍完整。

现有 Relay 可能建立的 PostgreSQL pool 上限并未形成一个统一预算：

- main pool 默认 20；
- audit pool 5；
- search pool 没有显式配置，SQLx 0.9 默认上限为 10；
- semantic Stage C 使用 main pool，Relay 另有硬编码 `8` 的整个 query process semaphore；
- PostgreSQL `max_connections=100`，但容器只有 512 MiB。

无 read replica 时，同一 writer endpoint 的理论 pool ceiling 是 main 20 + audit 5 + search 10 = 35，
并非现有 main pool 注释声称的 25。若 `READ_DATABASE_URL` 误指回相同 endpoint，必须按解析后的 endpoint
合并预算，不能把 writer/read 两套 pool 当作独立容量。

这里的关键问题不是“100 个连接一定同时存在”，而是配置合同只按 PostgreSQL connection count
设计，没有按本地 cgroup memory、每 backend `work_mem`、复杂查询执行节点、并发后台 worker 和
普通读取保留容量。

### 1.3 Transient 404 证据

`tenant::bind_community()` 已经强类型区分：

```text
BindError::UnmappedHost  // lookup 成功，确实无映射
BindError::Lookup(E)     // lookup 无法执行，数据库/连接池失败
```

但 HTTP `/query`、WebSocket handshake 和其他 row-zero call site 当前都把两者转换为同一个 generic
404。事故窗口内多次 404 延迟约等于 main pool 的 3 秒 acquire timeout；数据库恢复后相同
`localhost:3000` host 自动恢复，说明 Community row 并未消失。

外部响应仍不得回显原始 Host、Community ID 或数据库错误，但 dependency unavailable 不应被表达为
永久 unmapped。真正 unmapped host 保持 generic 404；lookup timeout/recovery/connection failure 应为
不含 tenant 信息的可重试 503。

### 1.4 2026-08-12 预防性只读快照

在不发 semantic query、不重启、不迁移、不修改 gate 的前提下执行了只读检查：

| 检查 | 当前结果 | 判定 |
|---|---:|---|
| Relay `/_readiness` | 200，meeting/semantic ready | 服务已恢复 |
| query handler | parser/handler ready，trusted single relay | capability 恢复 |
| PostgreSQL memory | 424.6 MiB / 512 MiB（82.92%） | **余量仍不足** |
| PostgreSQL PIDs | 27 | 高于轻量本地栈预期，需纳入预算 |
| main pool | size 19 / max 20，active 0，idle 19 | 无当前 query 压力，但连接驻留高 |
| cgroup OOM delta | `oom=1`、`oom_kill=1` | 已发生一次真实 OOM |
| packing failures | invalid input 4，未继续增加 | 验收停止后未再施压 |

后续只读样本中 PostgreSQL memory 曾回落到约 221 MiB / 512 MiB，说明事故后高水位会随 backend/cache
回收而明显波动；单个瞬时低值或高值都不能替代 warm-idle 与阶梯峰值测量。当前 readiness 为绿不等于
资源风险关闭：既然无 active main-pool query 时也观察过约 83% 水位，在完成资源档位、pool 预算和
Stage C admission 前，不恢复默认/高跳 Live 验收。

## 2. 已证实事实与尚未证实内容

### 2.1 已证实

- Provider、向量索引和主要 relevance 链可用；受控预算查询能返回 Relay-signed result 与 canonical
  `read_commands`；
- 默认 3-hop 的四次失败均属于 `invalid_packing_input`；
- failure 发生在 response packing 的 prepacking validation，而不是 response-size fitting；
- 2-hop 对照成功，3-hop 对照失败；
- PostgreSQL 发生过 cgroup OOM kill 和 crash recovery；
- transient 404 与数据库不可用窗口一致，Community 数据没有丢失；
- 当前 512 MiB 档位和分散 pool 上限没有足够安全余量。

### 2.2 尚未证实，实施和文档不得冒充结论

- 具体违反的是 path contiguity、重复 Edge/Coordinate、coverage、provenance、score、lifecycle 还是
  其他 invariant；
- 任何一条特定 semantic SQL 是 OOM 的唯一原因；
- Meeting bootstrap SQL 是内存元凶；它只能被称为 OOM victim 当时正在执行的语句；
- 单条 traversal 在隔离条件下就会耗尽 512 MiB；
- 将 memory limit 单独提高即可解决全部问题；
- 4、5、6 hops 一定失败或一定成功；
- 当前代码具备生产、多 Pod、LB、长期 soak 或正式发布资格。

## 3. 目标与非目标

### 3.1 目标

- 找到并修复 3-hop forest 的具体合同违例，不降低默认或硬跳数上限；
- 让 1–6 hop 合法 forest 都能确定性 pack、validate、sign 和 round-trip；
- 让非法 forest 返回低基数、稳定、脱敏的内部 reason code；
- 外部保持不泄露 graph topology/content 的稳定错误 subtype；
- 为 Stage C DB traversal 增加独立、可配置、可测量的并发 admission；
- 明确 main/audit/search/semantic 的总连接预算，并保留普通 host/Project/Meeting 读取容量；
- 建立受支持的本地 semantic PostgreSQL memory/settings profile；
- 将 row-zero truly-unmapped 与 dependency-unavailable 正确映射为 404 / retryable 503；
- 在 scratch 环境完成故障注入、内存阶梯和 1–6 hop 资格后再恢复 Live；
- 修复和验收过程中保持 canonical 与 semantic generation 数据不变。

### 3.2 非目标

- 不通过把默认 `max_hops_per_path` 改为 2 来隐藏 bug；
- 不把服务端硬上限从 6 降低；
- 不用无界重试或扩大 response cap 掩盖 invalid forest；
- 不把整个 semantic request 永久限制为并发 1；
- 不让一个 traversal semaphore 覆盖 Provider wait、Provider HTTP 或 packing 全生命周期；
- 不自动 query-enable，不自动发 Provider canary；
- 不恢复 Fleet TTL；
- 不改变向量 dimensions、Provider、ranking、lifecycle 或 canonical read contract，除非确定的 root cause
  证明现有合同本身有错误且经过独立设计评审；
- 不交付生产、多实例或正式 release 资格。

## 4. 必须保持的不变量

修复不得弱化：

1. NIP-98 exact request/body/Host binding；
2. Community、caller、成员和 read authorization；
3. query/index/Project Context gate；
4. active generation、current heads 和 source currentness；
5. Provider 出域 acknowledgement、admission 和 Stage B final recheck；
6. Stage C repeatable-read snapshot；
7. lifecycle filter、budget、beam、materialization 和 deadline caps；
8. Stage D current principal/query/index/source-family readiness recheck；
9. stable Relay signer、request binding 和 signed response binding；
10. `persisted_virtual_events=0`；结果 Event 继续只存在于 response，不落 canonical event store；
11. external error 不包含 problem、overview、向量、完整 Coordinate/Edge/path、API key 或私钥；
12. `trusted-single-relay` 只省略 Fleet proof 的现有语义。

## 5. Packing 合同修复设计

### 5.1 先关闭可观测性缺口

不能继续把所有 `InvalidState(String)` 只计为 `invalid_packing_input`。在
`buzz-semantic-query` 定义稳定、低基数的 result invariant reason，例如：

```text
non_canonical_query
prepack_coverage_state
root_discovery_or_shell
input_observation_partition
query_channel_binding
path_identity_or_ordinal
path_start_entrypoint
path_not_coordinate_contiguous
path_repeated_edge
path_repeated_coordinate
hyperedge_identity_or_binding
provenance_mismatch
score_mismatch
lifecycle_mismatch
budget_exceeded
unknown_contract_violation
```

实现不得通过解析英文错误字符串来决定 reason。应把 validator 内部错误改成 typed violation；现有
`validate()` / `validate_for_request()` 如需保持 API 兼容，可以把 typed error 映射回当前
`SemanticGraphQueryError`，同时为 Relay 提供一个返回 typed reason 的显式验证入口。新增 public API
必须有 doc comments。

外部响应建议保持 HTTP 500，并使用稳定、不泄露内容的 subtype：

```text
internal:semantic_graph_query:result_contract_violation
```

内部日志和 metric 只能记录：

- invariant reason；
- stage=`traversal_output|packing_input|packing_output`；
- caller budget 的 hop/path profile；
- observed root/path/max-hop **计数**；
- request correlation 的随机 run ID（不是 graph ID）。

禁止记录 problem、context overview、完整 Coordinate、Edge、path、向量、Provider body、API key 或
signed result body。reason enum 与 metric label 必须有穷尽映射测试，避免高基数字符串标签。

### 5.2 Traversal 与 packer 复用同一结构 validator

把 forest 的纯结构合同抽成无数据库、无网络、无签名 side effect 的 validator：

```text
validate_completed_forest_shape(query, roots, paths, coverage, observations)
```

执行两次：

1. Stage C read transaction 提交后、`SemanticGraphTraversalOutcome` 交给 bridge 前；
2. packer 接收完整 input 后，连同 request binding/input observations 做完整验证。

第一次让 traversal producer 在自己的边界暴露错误，第二次保持 packer 对任意未来调用者失败关闭。
两者必须共享实现，不能复制一套规则后再次漂移。结构验证在 read transaction commit 之后执行，
不为纯 CPU validation 延长 PostgreSQL snapshot 生命周期。

### 5.3 离线复现而不是继续查询 Live 数据

从验收中只提取**形状**，创建完全合成、无业务内容的 deterministic fixture：

- 至少一条 3-hop linear path；
- branch convergence；
- 同一 Hyperedge 多 Coordinate；
- 不同 Context Document 绑定同一 Edge；
- cycle candidate 被正确省略；
- path endpoint 重合但 provenance 不同；
- lifecycle active/finalizing/terminal/deleted 组合；
- explicit initial 与 semantic root 混合；
- response truncation 丢整条 path、整段 summary，不能制造 dangling path。

先让 fixture 精确重现当前 reason，再修 producer 或 validator：

- 如果 traversal 产生不连续、重复或 provenance 错误，修 traversal state transition；
- 如果 producer 输出符合设计而 validator 拒绝，则必须先证明 spec/签名/read-command 不变量仍成立，
  再修改 validator；
- 禁止简单删除 validator 检查；
- 禁止在 packer 中静默丢弃任意 invalid path 并把结果伪装成成功；如产品决定允许 omission，必须
  有 typed omission reason、coverage accounting 和签名后的可验证证据。

在 typed reason 可用后按以下顺序排查，但不得把顺序写成已证明根因：

1. 第 3 hop 首次触发的完整 path 约束：contiguity、entrypoint、重复 Edge/Coordinate、Hyperedge
   membership；当前 `FrontierPathState::append_hop` 只覆盖部分重复约束，完整 path 首次在 packer 校验；
2. 第 3 hop 新 hydrate source 的 preview 合同（非空 title、禁止 NUL 等）；
3. path 停止后的 coverage/completion/budget accounting；
4. binding/provenance/score；这些已有较多逐 hop 复核，但仍需 mutation test 证明。

### 5.4 1–6 hop 预防矩阵

必须覆盖：

| 类别 | 断言 |
|---|---|
| valid linear 1..6 hops | pack、validate、sign、deserialize 全通过 |
| 7 hops | request budget 在 traversal/Provider 前拒绝 |
| cycle | 不生成重复 Coordinate/Edge path；coverage reason 正确 |
| branch convergence | provenance-distinct path 稳定排序、去重合同正确 |
| repeated Edge | deterministic typed rejection |
| repeated Coordinate | deterministic typed rejection |
| broken contiguity | deterministic typed rejection |
| wrong entrypoint | deterministic typed rejection |
| wrong score/provenance | deterministic typed rejection |
| deleted/tombstone target | 不作为 continued target 返回 |
| response cap | whole path/summary omission；coverage 计数一致 |
| shuffled valid input | packed bytes/result deterministic |
| signed round-trip | exact request binding、Relay signer 与 read_commands 保持 |

增加 bounded property tests：生成有上限的合法 forest，要求
`valid forest => pack + validate`；对单一 invariant 做 mutation，要求
`invalid forest => exact typed reason`。property test 必须固定 seed/最大 case 数，不得访问 Provider 或
Live DB。

## 6. PostgreSQL 资源档位与连接预算

### 6.1 受支持的本地 semantic profile

当前 512 MiB profile 已由真实 OOM 证明不合格。实施时先在 scratch 环境冻结一个受支持的 local
semantic profile；候选起点为：

```text
container memory limit          2 GiB
shared_buffers                  256 MiB
work_mem                        4 MiB
maintenance_work_mem            128 MiB
effective_cache_size            1536 MiB
max_connections                 40
```

这些值是待 qualification 的候选，不得仅凭经验直接标记为完成。最终冻结值必须由 §10 的 1→2→4→8
阶梯、后台 Desktop/Meeting/Agent 流量和峰值证据决定，并同步 root `docker-compose.yml` 与受支持的
`deploy/local/compose.yml`。若最终值不同，在实施记录中说明测量依据。
`effective_cache_size` 只是 planner hint，不是预分配内存；资源证据必须来自 cgroup/PostgreSQL 实测，
不能把该设置值计为实际占用或安全余量。

完成阈值在开始 Live 前冻结，至少要求：

- warm idle memory 不超过 container limit 的 60%；
- qualification 峰值不超过 75%，同时保留至少 256 MiB absolute headroom；
- `memory.events` 的 `oom`、`oom_kill`、`high`、`max` 在测量窗口无增量；
- PostgreSQL 不进入 crash recovery；
- pool acquire timeout 为 0；
- 取消/超时后没有遗留长 transaction 或持续增长的 backend memory。

如果 2 GiB 候选无法满足门槛，应降低 pool/traversal 并发或提高受支持档位；不得放宽门槛解释失败。

### 6.2 Pool 总预算必须显式配置

新增并校验非敏感配置，例如：

```text
BUZZ_DB_MAIN_MAX_CONNECTIONS / MIN_CONNECTIONS / ACQUIRE_TIMEOUT
BUZZ_DB_READ_MAX_CONNECTIONS / MIN_CONNECTIONS / ACQUIRE_TIMEOUT
BUZZ_DB_CONTROL_MAX_CONNECTIONS / MIN_CONNECTIONS / ACQUIRE_TIMEOUT
BUZZ_DB_AUDIT_MAX_CONNECTIONS / MIN_CONNECTIONS / ACQUIRE_TIMEOUT
BUZZ_DB_SEARCH_MAX_CONNECTIONS / MIN_CONNECTIONS / ACQUIRE_TIMEOUT
BUZZ_DB_SERVER_CONNECTION_RESERVE
```

要求：

- search pool 不再依赖 SQLx 隐式默认值；
- writer-main/read-main/control/audit/search 的 idle timeout 和 lifetime 同样显式配置；
- control pool 是连接同一 writer endpoint 的独立小池，专供 row-zero host resolution、短 health/readiness
  probe 和其他在 tenant binding 前必须完成的控制面读取；它不能被 semantic traversal 借用；
- audit/search 关闭时不分配 pool；
- 按解析并脱敏后的数据库 endpoint 分组计算预算；同 endpoint 的 writer/read/search 必须相加；
- Relay 启动时打印各 pool ceiling 与总预算，不打印 DATABASE_URL；
- 对每个 canonical endpoint 分别校验：
  `writer_main + read_main + control + audit + search + server_reserve <= endpoint max_connections`，
  其中只累计实际指向该 endpoint 的 pool；
- `READ_DATABASE_URL` 存在时，`Db::new` 会创建一套独立 read pool，不能把它视作 writer-main 的别名
  而漏算；如果 read URL 指回 writer（含等价 URL/别名），writer group 必须同时计入两套 ceiling；
- 如果 read endpoint 真正独立，分别向 writer/read 读取并验证其 `max_connections` 和各自 reserve；
- 无法证明 endpoint 独立时采用保守合并，不能因主机别名而少算；当前 local profile 将
  `localhost`、`127.0.0.1`、`::1` 与默认端口规范化到同一组。未来非本地 alias 如需声明独立，必须
  通过显式、受校验的 endpoint-group 配置/服务器身份完成，不依赖 DNS 字符串猜测；
- `server_reserve` 只是保留给 migration/operator/故障恢复的 PostgreSQL 全局余量，不能被描述成对 Relay
  请求的可执行优先级；row-zero 的可执行保留由独立 control pool 保证；
- semantic traversal 上限还必须满足
  `traversal_in_flight <= main_max - ordinary_main_reserve`，从 main pool 中为 Project/Document/Meeting 等
  普通业务读取留下明确槽位；这只约束 semantic 对 main 的占用，不能替代 control pool；
- 配置非法时 Relay 启动失败，而不是运行中等 pool timeout。
- 为各 pool 设置非敏感 `application_name`，使 `pg_stat_activity` 和事故日志可区分 main/audit/search，
  不再依靠 PID 猜测来源。

本地无 replica 候选可以从 writer-main 12、read-main 0、control 2、audit 2、search 2、server reserve 4、
ordinary main reserve 4 开始测量；配置独立 replica 时再为 read-main 冻结单独 ceiling。这不是未经资格
即可冻结的产品默认，最终值由普通负载和 semantic 阶梯共同决定。

必须增加资源隔离回归：人为占满 semantic 可用的 main-pool 槽位时，mapped host resolution 和 readiness
仍从 control pool 在冻结时限内完成；占满 main 不能再把 configured host 变成 3 秒 timeout/404。
如果 control pool 自身不可用，按 §7 的 dependency 分类失败关闭，不能回落 main pool。
配置测试还必须覆盖：无 read URL、独立 read endpoint、字节相同 read/writer URL，以及经默认端口、
大小写/IPv6/解析规范化后实际相同的 endpoint；任何一种同端点别名都不能绕过 pool 总预算。

### 6.3 Stage C 专用 traversal admission

保留现有整个 query process 上限，用于限制 Provider/query 总数；另增加只覆盖 Stage C expensive DB
snapshot/traversal 的 semaphore：

```text
BUZZ_SEMANTIC_GRAPH_TRAVERSAL_MAX_IN_FLIGHT
```

要求：

- permit 在 Provider 编码完成、`begin_generation_bound_read()` 之前取得；
- permit 只持有到 read transaction commit，不覆盖 packing、postflight 或 signing；
- 不使用无界等待；只允许一个受 deadline 约束的短等待，失败返回稳定、可重试的
  `busy:semantic_graph_query:traversal_busy`；
- saturation 不创建 DB transaction，不增加 pool waiter；
- 增加 `in_flight / limit / busy_total / wait_seconds / transaction_seconds` 指标；
- caller 取消、deadline、DB error 和 panic-safe drop 都释放 permit；
- 不能长期硬编码为 1。

并发 1 只用于修复后的第一条 smoke。随后依次测 2、4、8，冻结满足 memory、pool 和普通功能 SLO 的
最高安全值。若 4 安全而 8 不安全，产品默认可定为 4；若 8 安全，保持现有吞吐。结果必须来自测量，
不能因为本次事故直接猜测。

Provider 已经发生在 Stage C 之前；如果 traversal saturation 在 Provider 返回后出现，可能浪费一次
Provider 调用。实现可在 Provider 前做非授权性的 capacity hint，但真正 permit 仍只能在进入 Stage C
前原子取得；不得为了避免浪费重新把 traversal permit 覆盖整个 Provider wait。

### 6.4 预防性 local capacity check

增加只读脚本/Just recipe，例如：

```text
scripts/check-local-semantic-capacity.sh
just semantic-local-capacity-check
```

脚本仅允许 local Docker context，验证：

- PostgreSQL container 是本仓库拥有的实例；
- memory/swap limit 达到受支持 profile；
- PostgreSQL major、pgvector、settings 与冻结合同一致；
- writer/read/control/audit/search pool 按 canonical endpoint 汇总后，与各 endpoint 的
  `max_connections`、server reserve 和 traversal limit 一致；
- `memory.events` OOM counters 可读取并形成 baseline；
- migration 0057/0058 与 semantic schema ready；
- 不输出 DATABASE_URL、password、Provider key 或业务内容。

该检查不能自动调大资源、重建容器、执行 query-enable 或发 Provider 请求。

### 6.5 数据库故障短路与恢复风暴抑制

OOM 发生后的 1,372 次失败连接说明仅增加 memory 仍不足。实现一个共享、短时的 database
dependency circuit/backoff：

- circuit key 由 canonical database endpoint digest + pool role 组成；digest 输入必须先剥离 userinfo、
  password、query、fragment 和其他 secret，只保留足以区分故障域的规范化非敏感坐标；不能用一个
  全局布尔把 writer、read replica、search、audit 和 control 混为同一故障域；
- pool-local acquire timeout 只打开对应 role 的 circuit；connection refused/recovery 等明确 endpoint-wide
  故障可以影响同 endpoint 的各 role，但不能传播到不同 read-replica endpoint；
- 首次连接拒绝、recovery、acquire timeout 后进入有上限的指数退避并加入 jitter；
- background Meeting、semantic worker、reminder 和 lifecycle task 使用共享 dependency 状态，不能各自
  以固定短周期同时重连；
- readiness 先执行一次短 PostgreSQL probe；probe 失败时直接把 DB-dependent components 标为
  unavailable，不再同一轮并发启动多组 feature SQL；
- 正常业务请求不在 Relay 内无界自动重放写操作；客户端只对明确可重试的读/探测做有界退避；
- dependency 恢复后采用受控 ramp-up，避免所有 worker 在同一毫秒恢复；
- 增加 `db_dependency_open / reconnect_attempts / reconnect_rejected / recovery_duration` 低基数指标；
- fault test 冻结恢复窗口的最大连接尝试率，候选门槛为全 Relay 每秒不超过 5 次；最终值在实现前
  明确写入测试，不能继续接受事故中的约 37 次/秒。

该 circuit 只抑制依赖不可用时的重连；不能缓存或伪造 Community、权限、Project/Document/Context
结果，也不能在数据库不可用时把 stale 数据标成 current。

## 7. Row-zero 404/503 与 Desktop 恢复语义

### 7.1 Relay mapping

对所有 **tenant-scoped door** 统一 `bind_community()` error mapping：

```text
BindError::UnmappedHost  -> 404 relay: no community is configured for this host
BindError::Lookup(transient dependency failure)
                         -> 503 unavailable:relay:community_lookup
BindError::Lookup(permanent contract/programming failure)
                         -> 500 internal:relay:community_lookup
```

不能把全部 `DbError` 都标成 retryable。新增闭集 `HostLookupFailureClass`（或等价类型）：

- `PoolTimedOut`、`PoolClosed`、connection-class SQLSTATE、recovery/shutdown、resource exhaustion 等短暂
  dependency failure 才映射 503；
- undefined table/column、schema contract、decode、type mismatch 和其他 programming/invariant failure
  映射 generic 500；
- 未识别错误默认 500，不能乐观标成可重试。

503/500 均不回显 host、Community、SQL、pool state 或内部 error。内部只记录低基数 reason，例如
`pool_acquire_timeout | database_recovery | connection_unavailable | schema_contract |
decode_contract | unknown_internal`。

该区分不会提供默认 tenant 或放宽 row-zero：Lookup failure 仍然在任何 tenant-scoped read/auth 之前
失败关闭。安全测试需要证明任意 host 在 DB unavailable 时都得到同一 503 body；真正未知 host 在 DB
健康时仍得到同一 generic 404。

应覆盖 WebSocket handshake、`POST /query`、`/events`、`/count`、workflow/webhook、git/media 等所有
tenant-scoped row-zero 入口，避免只有 semantic endpoint 修好而 Desktop 其他请求仍误报 404。

NIP-11 是明确的例外：它当前在 WebSocket row-zero binding 前提供 base document，并对 host-scoped
icon/capability 做 best-effort 观察。不能把 NIP-11 笼统改成 tenant-scoped 503。改造为一次 host
observation并复用，避免同一个文档为多个 extension 重复 bind/query；base NIP-11 继续 200、继续不泄露
host 是否映射。同时增加 closed、无 tenant 内容的观察状态，例如：

```text
buzz_supported_extensions_status = observed | temporarily_unavailable
```

`buzz_` 前缀是现有 NIP-11/wire 兼容命名空间，不是需要做产品文案去 Buzz 的用户可见品牌。

- mapped 或真正 unmapped 且全部观察成功：`observed`，extension absence 才能解释为 unsupported；
- lookup/recovery/control-pool unavailable 或任一动态 capability 查询无法完成：
  `temporarily_unavailable`，不把 omission 解释为永久 unsupported；
- 未识别 schema/programming failure 对外同样只暴露 unavailable observation，内部记 typed 500-class
  metric；base document仍不返回 host/SQL；
- Desktop、Tauri 和 `cf` 对 `temporarily_unavailable` 返回 retryable unavailable；只有 `observed` 状态下
  缺 extension 才返回 unsupported；
- 对缺少该新字段的 legacy Relay 保持现有兼容行为，不能把 unknown 自动授权为 supported。

### 7.2 Desktop 状态

Desktop/Tauri 必须把三类状态分开：

```text
unsupported   = Relay 已成功响应且 capability/schema 明确不支持
unavailable   = 503、连接失败、timeout、数据库恢复；可重试
ready         = 当前验证成功
```

`unavailable` 时：

- UI 显示临时不可用/正在重新连接，不显示永久 “Project Context is not supported”；
- 保留最近已验证的画布仅作 stale presentation，并明确标记非 current；
- 禁止在 stale 状态执行 semantic query 或 mutation；
- 使用 capped exponential backoff + jitter；恢复成功后自动重新验证；
- Community/identity/workspace token 变化继续使旧结果失效；
- 不形成请求风暴。

### 7.3 回归矩阵

- mapped host + DB healthy => 原有 200/auth 行为；
- unmapped host + DB healthy => generic 404；
- mapped/unmapped host + pool acquire timeout => byte-identical retryable 503；
- mapped/unmapped host + schema/decode failure => byte-identical generic 500；
- PostgreSQL recovery => 503，不出现 configured-host 404；
- NIP-11 dependency outage => base document 200 + `temporarily_unavailable`，不伪造 capability；
- NIP-11 healthy true absence => `observed`，可以判定 unsupported；
- dependency 恢复 => 自动回到 200，无 Community 重建；
- Desktop unavailable => reconnecting/stale，不进入 unsupported；
- capability 确实撤销 => unsupported；
- 反复 503 => backoff 有界，无 tight loop。

## 8. 文件改动矩阵

| 文件/区域 | 计划改动 |
|---|---|
| `crates/buzz-semantic-query/src/result.rs` 或新 `validation.rs` | typed invariant reason、共享 forest validator、1–6 hop/property tests |
| `crates/buzz-relay/src/semantic_graph_traversal.rs` | traversal 输出边界验证、Stage C permit、3–6 hop fixtures |
| `crates/buzz-relay/src/semantic_graph_response.rs` | typed packing input error、低基数 metric、完整 pack/sign round-trip tests |
| `crates/buzz-relay/src/semantic_graph_observability.rs` | reason/in-flight/busy/transaction 指标，禁止高基数字段 |
| `crates/buzz-relay/src/api/bridge.rs` | stable external subtype、row-zero typed mapper、HTTP admission tests |
| `crates/buzz-relay/src/tenant.rs` | 保持 typed BindError，新增 transient/permanent lookup 分类与 tests/doc |
| `crates/buzz-relay/src/nip11.rs` | 单次 best-effort host observation、extension observation status、客户端合同测试 |
| `crates/buzz-relay/src/router.rs` | WS/HTTP row-zero 404/503、readiness/fault tests |
| `crates/buzz-relay/src/state.rs` | 独立 traversal semaphore；现有 whole-query semaphore 继续保留 |
| `crates/buzz-relay/src/config.rs` / `main.rs` | traversal/pool config、总预算验证、显式 search/audit pool ceiling |
| Relay readiness 与 background worker 启动路径 | dependency circuit、错峰退避、单 probe 短路 |
| `crates/buzz-db/src/lib.rs` | main/control pool config 接线与 pool metrics；不改 schema/migration |
| `crates/carryforth-cli/src/commands/project_view_snapshot.rs` 与 NIP-11 consumers | 解析 observation status；semantic 缺能力时区分 unavailable/unsupported |
| `crates/carryforth-cli/src/commands/{project_context,documents,meetings}.rs` | 统一复用 capability observation，避免各命令自行误判 omission |
| `crates/carryforth-cli/TESTING.md` | dependency outage、legacy Relay 与恢复验收说明 |
| `desktop/src-tauri/src/commands/project_context/**` | dependency unavailable 与 unsupported 分型 |
| `desktop/src/features/project-context/**` | reconnecting/stale UI、backoff、恢复重验 |
| `docker-compose.yml`、`deploy/local/compose.yml` | 经 qualification 冻结的 semantic PostgreSQL profile |
| `.env.example` | 非敏感 pool/traversal 配置说明，真实 `.env` 不入提交 |
| `scripts/check-local-semantic-capacity.sh` | 只读 local preflight |
| 新 disposable resource qualification script | scratch PG、fault injection、阶梯测量与清理 |
| `Justfile` / CI | contract gate；Docker resource qualification 保持显式、隔离执行 |
| `docs/semantic-pgvector-operations.md` | 支持档位、preflight、恢复/回滚 runbook |

实施前需通过 `rg` 再枚举全部 `bind_community()` call site，不能把上表当作穷尽路径清单。

## 9. 分阶段实施

### Phase B0：冻结事故证据与关闭 Live 压力

- 保留脱敏时间线、metric 和 cgroup counter；
- 不复制 problem、overview、向量、业务 ID 或完整 path 到 tracked 文档；
- 不继续 Live 3–6 hop/default 查询；
- 若存在非验收调用方仍可能触发 semantic query，先由 operator 显式 `query-disable`；本方案不自动
  修改 gate。

### Phase B1：Typed reason 与离线复现

- 引入低基数 invariant reason；
- 保持外部内容不泄露；
- 创建合成 3-hop reproduction；
- 确认具体 reason 后再决定 producer 或 validator 修复点。

退出门：无需 Live DB/Provider 即可稳定 red test，且失败 reason 精确、无业务内容。

### Phase B2：修复 forest producer/validator 合同

- 修复已证明的具体状态转换或 validator 合同；
- 复用 traversal/packer validator；
- 完成 1–6 hop、cycle、convergence、truncation、signing/property tests。

退出门：原 reproduction 变绿；mutation tests 仍以精确 reason 失败；默认/硬上限不降低。

### Phase B3：资源 profile、pool budget 与 Stage C admission

- 显式配置 main/audit/search pool；
- 增加 Stage C traversal semaphore 和 metrics；
- 资格化并冻结 local semantic PostgreSQL profile；
- 增加只读 capacity check。

退出门：scratch 1→2→4→8 阶梯确定安全默认；无 OOM、pool timeout 或普通读取退化。

### Phase B4：Row-zero dependency 分类和 Desktop 恢复态

- 所有 row-zero call site 正确映射 404/503；
- NIP-11 base document 保持 pre-bind/best-effort，并输出 namespaced observation status；
- 枚举并迁移 `carryforth-cli` 中所有直接读取 `supported_extensions` 的 consumer；至少 semantic-query
  必须把 `temporarily_unavailable` 映射为 retryable unavailable，而不是 false capability/unsupported；
- Desktop 分离 unsupported/unavailable/stale；
- PostgreSQL stop/recovery fault injection 验证自动恢复且无请求风暴。

### Phase B5：Disposable 集成与 soak

- 自建 scratch pgvector/PostgreSQL；
- 使用合成 canonical graph 和虚构文本；
- 验证 1–6 hop、并发阶梯、取消/deadline、OOM/recovery；
- 完成后只删除本次拥有的 container/anonymous test volume；绝不指向 Live volume。

### Phase B6：受控 Live 恢复

只有 §10 全部通过后执行 §11 的逐级恢复。每一级失败立即停止，并保持/恢复 query-disable。

## 10. 自动化测试与预防门禁

### 10.1 无基础设施单测

```bash
. ./bin/activate-hermit
cargo test --locked -p buzz-semantic-query --lib
cargo test --locked -p buzz-relay --lib semantic_graph_response::tests
cargo test --locked -p buzz-relay --lib semantic_graph_traversal::tests
cargo test --locked -p buzz-relay --lib tenant::tests
cargo test --locked -p carryforth-cli --lib
cargo clippy --locked -p buzz-semantic-query -p buzz-relay -p carryforth-cli --all-targets -- -D warnings
cargo fmt --all -- --check
```

测试必须包含 1–6 hop valid matrix 和逐 invariant mutation；不能只有 1-hop packer fixture。
CLI 单测必须覆盖：observed+present、observed+absent、temporarily unavailable、缺字段 legacy Relay、
malformed/unknown status；semantic、Project View、Documents 和 Meetings 不得对同一 NIP-11 状态产生
互相矛盾的永久/临时判断。

### 10.2 Desktop 回归

```bash
cd desktop
pnpm test
pnpm check
pnpm build:e2e
pnpm exec playwright test --project=smoke tests/e2e/project-context.spec.ts
```

新增 E2E：503 unavailable、stale canvas、backoff、恢复；确实缺 capability 才显示 unsupported。

### 10.3 Disposable PostgreSQL qualification

新脚本必须自行创建唯一命名、ownership label 的 scratch container/database，默认拒绝 remote Docker
context，所有 SQL 先验证固定 disposable marker。它至少覆盖：

- migration 0057/0058 fresh/upgrade；
- synthetic 1–6 hop traversal；
- concurrency 1→2→4→8；
- Desktop/Meeting/ordinary query 背景负载；
- cancellation/deadline/connection release；
- PostgreSQL stop/recovery 与 row-zero 503；
- cgroup memory/pids/pool/transaction metrics；
- owned cleanup；cleanup failure 使 qualification 非零。

脚本不得接受或继承 Live `DATABASE_URL`，不得挂载 `postgres-data`，不得执行 `down -v`。

### 10.4 仓库级门禁

```bash
git diff --check
./scripts/check-carryforth-current-product-surface.sh
./scripts/check-open-source-release-surface.sh
just ci
```

Docker resource qualification 可以保持显式/manual gate，但完成记录必须附命令、profile、峰值、并发级别
和 counter delta；不能以普通 unit tests 替代。

## 11. Live 恢复顺序与停止条件

### 11.1 恢复前置条件

以下全部满足才能恢复：

1. 合成 3-hop reproduction 已找到具体 reason 并修复；
2. 1–6 hop offline matrix 全绿；
3. scratch resource/fault/soak 全绿；
4. local capacity check 通过；
5. PostgreSQL OOM counter baseline 已记录，支持 profile 已应用；
6. migration ledger/checksum、canonical/semantic counts 已只读记录；
7. Provider/API key 未进入日志或证据；
8. operator 明确批准一次受控、非敏感 Live 查询窗口。

### 11.2 阶梯

每一级只在前一级通过后继续：

1. 单请求，1 hop、1 path、短 wall time；
2. 单请求，2 hops；
3. 单请求，原失败形状的 3 hops；
4. 单请求，4、5、6 hops 的 bounded 合成问题；
5. 默认 profile；
6. traversal concurrency 2；
7. concurrency 4；
8. 只有前述资源门满足时才测 8；
9. 加入普通 Desktop、Meeting 和多 Agent 背景读取 soak。

每级验证 Relay signature、exact request binding、canonical `read_commands` 回读和数据 counts。

### 11.3 任一命中即停止

- packing 500 或任何新增 `invalid_packing_input`；
- PostgreSQL `oom/oom_kill/high/max` counter 增量；
- crash recovery；
- configured host 404；
- pool acquire timeout；
- memory 超过冻结峰值门；
- 普通 Project View/Documents/Context/Meeting 失败或明显退化；
- deadline 后 transaction/permit 未释放；
- canonical、generation、heads、Fleet 或 migration 出现未授权变化。

停止后先 `query-disable`，保存脱敏指标，再诊断；不得通过增大 hop/timeout/memory 或清数据继续试。

## 12. 数据安全与破坏性边界

禁止：

- 修改 migration 0057 / 0058 或 ledger checksum；
- 新建一个只为本 bug 绕过现有数据的 migration；
- `DROP`、`TRUNCATE`、业务 `DELETE`；
- purge/rebuild/GC active semantic generation；
- 删除 embeddings/heads/jobs；
- `docker compose down -v`；
- 删除或重置 PostgreSQL、Redis、MinIO volume；
- reset Desktop app state、keyring、Community 或 Agent identity；
- 用 Live 主库做 OOM、kill、fault injection；
- 把 query 失败解释成需要重建数据。

如应用新的 container resource limit，需要以现有 named volume 原位、非破坏性地 recreate PostgreSQL
container：先完成备份/ledger/count readback，明确 target container/volume，禁止 `-v`；recreate 后再次
校验 ledger、extension、counts 和 canonical read。该操作属于后续实施/验收步骤，不由本方案编写阶段
自动执行。

Live 前后至少固定比较：

- migration count/status 与 0057/0058 hash；
- Community/canonical host；
- Events、Project View objects/revision；
- Project Documents/catalog revision；
- Meeting sessions；
- Project Context revision/Edge count；
- active semantic generation；
- embeddings/current heads/jobs；
- `persisted_virtual_events=0`；
- Fleet rows逐字段不变（trusted mode 不读取不等于可删除）。

允许变化仅限：

- 显式 operator query-enable/query-disable；
- 运行指标；
- Provider admission 的派生限流时间戳；
- 不持久化的 signed response；
- disposable scratch DB/container 内本次测试拥有的数据。

## 13. 回滚

如果 packing 修复或 resource profile 有回归：

1. `query-disable`，先停止新的 Provider/graph query；
2. 停止受控验收流量；
3. 回滚代码/config 到最后已知稳定版本；
4. 如需重启 Relay/PostgreSQL，保留 named volume，绝不 reset；
5. 校验 migration/canonical/semantic counts；
6. 保持 query gate 关闭，直到失败 phase 重新通过。

不允许的回滚：

- 把默认/硬上限降到 2 hops 后宣称功能修复；
- 删除 packer validator；
- 把 invalid path 静默省略且不计 coverage；
- 只调大 PostgreSQL memory 而不做 pool/admission qualification；
- 把 dependency 503 改回 configured-host 404；
- 清空或重建用户数据以恢复 readiness。

## 14. 完成标准

只有以下全部 PASS 才关闭本 bug：

| Gate | 完成条件 |
|---|---|
| Packing root cause | 精确 typed invariant 已定位；不再只有 generic string |
| Hop contract | 合法 1–6 hop 全部 pack/validate/sign/round-trip |
| Invalid forest | 每类 mutation 给稳定 reason；无静默 omission |
| Default profile | 原失败 3-hop 与默认 profile 均成功 |
| Resource profile | scratch 和 Live 阶梯均满足冻结 memory/pool 门 |
| OOM | 无新增 oom/oom_kill/crash recovery |
| Admission | 1→2→4→8 有测量；冻结值不是永久硬编码 1 |
| Ordinary surfaces | Project/Document/Context/Meeting/host lookup 持续可用 |
| Row-zero | true unmapped=404；dependency unavailable=retryable 503 |
| NIP-11/CLI | base doc 保持可用；temporary observation 不再被 `cf` 误判成 unsupported |
| Desktop | unavailable/reconnecting 不再显示 unsupported；恢复自动重验 |
| Cancellation | 无遗留 transaction、permit 或 pool waiter |
| Data | migration/canonical/generation/heads/Fleet 不变 |
| Security | auth/currentness/Provider/signing 门禁和脱敏日志无回退 |
| Quality | 定向、Desktop、disposable qualification、`just ci` 全通过 |

任一子问题单独修复都不能解除整体 BLOCKED：packing 变绿但资源门失败，或增加 memory 后 500 仍在，
都保持 BLOCKED。

完成本方案仍只代表 Carryforth 当前 local single-Relay 支持面关闭这两个阻断；不自动获得生产、LB、
多 Pod、rolling upgrade、长期 relevance/soak 或正式 release 资格。

## 15. 实施记录

### 15.1 已实施的代码与配置

- completed forest 校验已改为闭集 typed invariant；外部仍是无内容 generic 500，内部只记录
  reason、root/path count、observed/budget hop，不记录 problem、overview、ID、path、vector 或密钥；
- traversal producer 与 packer 复用 request-aware validator；`FrontierPathState` 在 append 时校验
  entrypoint、contiguity、Edge membership 和 cycle；
- 离线真实 `TraversalEngine -> completed forest -> pack -> sign -> SDK verify` 已覆盖 1–6 hops；
- pool ceiling 已显式化，新增独立 control pool；当前本地预算为 writer main 12、control 2、
  audit 2、search 2、server reserve 4，启动时按 endpoint fail closed 校验；
- Stage C traversal 新增 Provider 后、repeatable-read 前的 fail-fast gate，当前默认 2；permit 只覆盖
  snapshot/traversal transaction，不覆盖 Provider wait 或 response packing；
- root 与 `deploy/local` PostgreSQL profile 调整为 2 GiB、40 connections、256 MiB
  shared buffers、4 MiB work memory，并提供 `just semantic-local-capacity-check`；
- row-zero host lookup 使用 control pool；unmapped=404、transient dependency=retryable 503、
  permanent contract/decode=generic 500；NIP-11、`cf`、Tauri、Desktop 增加
  `temporarily_unavailable`/reconnecting 语义和有界退避；
- readiness 在 DB probe 失败时不再并发执行全部依赖 DB 的 feature checks。

### 15.2 2026-08-12 受控诊断与根因

在 operator 显式 `query-enable`、唯一一次默认查询、`query-disable` 的受控窗口中，新的 typed
reason 将旧 generic `invalid_packing_input` 精确定位为：

```text
budget_expanded_coordinates
roots=6, paths=12, max_observed_hops=3, max_hops_budget=3
```

根因是 `advance_incident()` 只在 per-work quantum 为 0 时检查全局
`max_expanded_coordinates`。前一个 work 已消耗第 64 个全局 slot 后，后续 work 获得新的
`global_step()` quantum=1，可先把计数递增到 65；遍历随后正常结束，但签名前 request-aware
validator 正确拒绝超预算 forest。这是全局预算与公平调度 quantum 之间的 N+1 admission bug，
不是三跳合同、Provider、response byte cap、签名或 serialization 故障。

修复后，Coordinate expansion 使用单一 admission helper，按以下顺序原子判定：

1. 全局计数已达到 cap -> 记录 `ExpandedCoordinates` exhaustion 并停止；
2. 当前 quantum 为 0 -> defer；
3. 两者均允许 -> 同时递增全局计数并消费 quantum。

新增 producer 级回归用一条两跳合成链和 `max_expanded_coordinates=1` 证明：第一跳消耗额度后，
第二轮即使得到 fresh quantum 也不能产生 N+1 expansion；结果以合法 1-hop
`GlobalBudgetExhausted` forest 结束。辅助边界测试另覆盖 63 -> 64 -> 拒绝 65。

### 15.3 受控复验结果

相同非敏感默认 problem 在修复后成功：

- roots=6，paths=12；
- completion=`budget_exhausted`；
- exhausted dimensions 包含 semantic roots、hop、beam、expanded Coordinates 和 paths；
- 本次实际返回路径最深 2 hops，因为 64-coordinate 全局预算先耗尽；这不是把服务端 1–6 hop
  合同降到 2，离线真实 traversal/pack/sign 矩阵仍覆盖到 6；
- 14 条 canonical `read_commands` 全部回读成功；
- 查询后 `query-disable` 成功，active Community query gate 计数为 0。

PostgreSQL container 使用原 `buzz-postgres-data` named volume 非破坏性 recreate；此前先生成并验证
逻辑备份。recreate 后：

- Memory=2 GiB，`OOMKilled=false`，restart count=0；
- 本次受控请求后约 176 MiB / 2 GiB；
- 新 container cgroup `high/max/oom/oom_kill` 均为 0；
- readiness 200，普通 Project View canonical read 成功；
- Community=50、Events=3447、Project View objects=36、Documents=30、Context Edges=23；
- active generation=1、heads=81、succeeded jobs=81、queued/claimed/poison=0；
- persisted virtual events=0；
- migration 0057/0058 文件 SHA-256 仍分别为
  `ed4483984abc53496ef4658ab118b3a58a614773dd7f364cf2859631807cb59e` 与
  `ee15144b372c05437e34dd773438c6170bb381ab6b04ebda5e9708a73f34c755`。

### 15.4 已通过的离线门

- `buzz-semantic-query` unit tests；
- Relay response packer 1–6 hop matrix；
- Relay synthetic TraversalEngine 1–6 hop matrix；
- expanded-coordinate N+1 producer regression；
- `carryforth-cli` 全量 unit tests；
- Desktop frontend unit/check；
- Tauri focused tests；
- root 与 Tauri changed-surface Clippy；
- source asset inventory、Rust fmt、diff check；
- Relay / `cf` / `buzz-admin` build；
- `RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 just ci` 完整通过，包括 Desktop/Web build、
  Tauri 全量测试（1728 passed、14 ignored，另 3 个音频集成测试通过）和 Mobile
  测试（570 passed、1 skipped）。默认并行 Tauri 测试曾在既有 `relay_admission`
  虚拟时间用例上发生调度停滞；相同完整测试集单线程无失败；
- `scripts/test-semantic-migrations.sh` 在自有 disposable pgvector 容器中通过 migration upgrade、
  semantic pipeline、Fleet policy Stage B/D、desired-schema 和 ledger-less fresh-schema gates；
- 现有 content-free exact-kernel qualification 在无网络、无挂载、自有 disposable 容器中按
  concurrency 1 -> 2 -> 4 -> 8 分别完成 8 秒 baseline 与 concurrent VACUUM 测量：所有档位
  `failed_transactions=0`、statement cancellation 无遗留 session、post-soak 无 semantic query 或
  idle transaction，owned container/anonymous volume 已清理。该测量使用 10,000-source 合成
  kernel，不等同于完整 canonical graph/Provider/Desktop 负载资格。

### 15.5 仍未关闭的资格边界

以下未完成，因此整体仍为 BLOCKED，query gate 保持关闭：

- 以完整 canonical traversal SQL 与 2 GiB cgroup profile 执行的资源曲线、内存余量和默认值冻结；
- disposable DB outage/recovery fault injection、重连速率和长时 soak；
- Desktop + Meeting + 多 Agent 背景负载下的完整阶梯；
- production/LB/multi-Pod/policy-homogeneity 和长期 relevance 资格。

不得把本次单请求成功解释成上述资格已经完成，也不得在完成资源阶梯前常开 query gate。

### 15.6 2026-08-13 默认查询超时合同修正

Desktop 带两个 soft context Coordinates 的默认查询产生 Q0 + 2 个 conditioned channels，工作量高于
problem-only CLI smoke；旧默认 10 秒预算因此稳定到达 traversal deadline。进一步检查发现 traversal
虽能形成 `wall_time_exhausted` 的合法部分 forest，却用同一个已经到期的 deadline 提交 Stage C
read-only transaction，最终把可签名部分结果错误转换为 504。

当前合同调整为：默认和 hard cap 均为 180 秒；前 174 秒用于 Provider、Stage C 与 traversal，随后
5 秒只用于关闭 read-only snapshot，最后 1 秒用于 packing、Stage D 与签名。CLI 与 Desktop 的单次
HTTP transport envelope 为 195 秒，仍不自动重放 Provider 请求。该修复不改变 1–6 hop 合同、不降低
三跳默认值、不放宽 result validator，也不修改 migration、canonical 数据或 semantic index。
