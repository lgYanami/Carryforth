# Project Context 统一可靠性运行时实现计划

> 状态：R0、R1 已交付；R2–R6 待实施
>
> 日期：2026-08-16
>
> 代码基线：`feat/semantic-engine`，`4364deae89`
>
> 上位规范：
> [Project Context 统一语义检索引擎规范](project-context-unified-semantic-retrieval-engine-spec.md)
>
> 兼容基线：
> [Project Context 语义检索兼容基线记录](project-context-semantic-retrieval-compatibility-baseline.md)
>
> 第一阶段：
> [Project Context 统一语义计算实现计划](project-context-unified-semantic-computation-implementation-plan.md)、
> [Project Context 统一语义计算资格记录](project-context-unified-semantic-computation-qualification.md)
>
> 本阶段范围：统一交互式语义查询使用的 deadline/cancellation 上下文、Provider 单次 attempt、typed
> failure、有限 retry/backoff、request-local vector 复用、Provider circuit 与 release/signing 安全原语；
> 保持 traversal、总预算、snapshot 恢复范围和公开合同由各 closed operation coordinator 所有

## 0. 已确认决策

1. 第一阶段交付的是共享语义计算基座，不是统一执行四个公开 operation 的万能 Query Engine。
2. 第二阶段不把 traversal、frontier、hop、beam、root、path packing 或部分结果策略移入共同运行时。
3. 每个 operation coordinator 从自己的总预算创建provider-start/work/snapshot-close/absolute deadline
   windows；共同层只接收并遵守这些窗口，不决定完整路径或 one-shot 的预算与尾部保留。
4. 完整路径每个 root attempt 将 Q0 与保留的 Qi 作为一个有序 bundle 调用 Provider 一次；后续所有 hop
   复用同一个 vector bundle 和同一个 Stage C repeatable-read snapshot，不逐 hop 重新向量化。
5. 共同 exact scorer 不拥有事务生命周期，也不得在 SQL 失败后透明切换 snapshot；事务恢复范围由
   operation coordinator 决定。
6. 不新增大型 `ReliabilityProfile` 作为第二份 operation 策略来源。Coordinate、one-hop 与完整路径自身
   已经拥有 deadline、snapshot、retry、partial-result 与 release 规则。
7. 共同层只提供最小 `SemanticExecutionContext`、Provider 可靠性执行器、typed failure 和可组合安全原语。
8. Provider retry、operation restart 和 CLI retry 不能同时成为 retry owner。CLI 保持一次 HTTP 请求；
   Provider 物理 attempt 由一个 request-level ledger 统一计数，operation 只决定是否重建或重启自己的工作。
9. query vector 只允许在同一个逻辑请求内临时持有；不写入 PostgreSQL、Redis、日志或持久缓存，也不跨请求
   复用。
10. one-shot 与完整路径的 snapshot、release 和公开错误合同保持独立：
    - Coordinate 和两个 one-hop 继续对实际 RR observation 执行 exact-snapshot release；
    - 完整路径继续保持现有 `expected_snapshot: None` release 合同；
    - 完整路径现有 generation/context churn 重做行为不得机械复制给 one-shot。
11. 新 queue、Community/caller 公平、operation priority、Provider/DB/traversal 容量和后台/交互调度属于
    第三阶段资源治理。本阶段只让现有等待和资源取得路径受同一 deadline/cancellation 约束。
12. Phase 2 先做零策略迁移，再分别启用 retry、vector reuse 和 circuit；不得让架构接线与新可靠性行为
    同时上线。
13. 三个公开 surface、Event kind `40912/40913/40914`、capability、gate、SDK、CLI、request/result DTO、
    ranking、coverage、response cap 与公开错误映射保持不变。
14. 所有正常 fleet 实例必须运行相同 reliability route 与编译期策略 digest；不允许在同一可路由 fleet 中
    混跑 legacy 和 migrated 策略。
15. 本阶段不以成功宣称第三阶段资源治理、跨 Pod 全局公平、Provider fleet-wide 并发控制或 production SLO
    已完成。

## 1. 目标

Phase 1 已经统一：

- closed semantic input 与有序 input bundle；
- 单次 Provider batch encoding；
- Community/generation-bound query vector；
- current-head exact query-to-source scoring；
- 四个 operation 进入共同计算原语的 typed adapter。

Phase 2 要解决的不是语义计算重复，而是交互式查询执行可靠性仍分散：

- `SemanticOneShotExecution` 已经为 Coordinate 与两个 one-hop operation 共享 process admission、ticket、
  Provider reservation/wait、egress confirm、hard deadline 和 exact release；
- 完整路径在 `semantic_graph_query.rs` 中维护另一套 ticket、context observation、reservation、confirm、
  Provider attempt 和 churn retry；
- Provider transport 无内部 retry 或 circuit，并把连接前失败与请求交付后 outcome unknown 压缩为较粗错误；
- DB error 无法直接区分授权拒绝、snapshot/currentness、只读 transient 与 reservation commit outcome unknown；
- 当前取消主要依赖 future timeout/drop，没有统一的 caller disconnect、shutdown 和 terminal latch；
- one-shot release permit 通过后被丢弃，再由 surface 单独签名；完整路径已经使用单次 permit 同步签名；
- 指标无法统一区分 logical request、physical Provider attempt、retry disposition 和取消后的迟到工作。

本阶段完成后的目标边界是：

~~~text
closed operation coordinator
  owns request validation, total deadline, input rebuild,
       RR topology, traversal, restart scope, result and public errors
          |
          v
shared interactive reliability primitives
  SemanticExecutionContext
  Provider reliability executor
  typed internal failures
  request-local physical-attempt ledger
  cancellation/deadline helpers
  release-permit synchronous-finalize helper
          |
          v
shared semantic computation substrate
  SemanticInputEncoder::encode_once
  GenerationBoundQueryVector / bundle
  current-head exact scoring inside caller-owned RR
~~~

成功标准不是“所有 operation 使用同一个状态机类型”，而是：

1. 相同的 Provider egress 可靠性、安全重试和 circuit 规则只实现一次；
2. 相同的 deadline/cancellation 语义贯穿每个 operation 已有的执行阶段；
3. DB/Provider failure 能在内部准确分类，但公开错误逐字段保持兼容；
4. operation coordinator 仍明确拥有事务、恢复范围和最终结果；
5. 任何 retry 都有唯一 owner、统一物理 attempt 上限和可审计的资源归还证据。

## 2. 精确架构边界

### 2.1 共享语义计算基座

以下是 Phase 1 已交付并必须保持纯净的共同计算边界：

~~~text
SemanticQueryInputBundle
  -> SemanticInputEncoder::encode_semantic_inputs
  -> ProviderEncodedSemanticInputBundle
  -> GenerationBoundQueryVector / bundle
  -> SemanticGraphReadTx exact-scoring methods
~~~

约束：

- `SemanticInputEncoder` 一次接收一个有界 bundle，一次调用最多产生一个物理 Provider 请求；
- encoder 不拥有 retry、admission、generation、snapshot、scope、ranking 或 public error policy；
- Provider 输出只有经 fresh authorized DB ticket 绑定后才能成为 generation-bound vector；
- exact scorer 在调用者已经打开的 RR transaction 中执行；
- scorer 不打开、关闭或替换 transaction，也不透明 retry；
- operation-specific scope、fusion、floor、tie、K+1、coverage、omission 和 result projection 保持在 adapter；
- source-pair coherence 是独立的 current-source scorer，不强行塞入万能 query-vector scorer。

Phase 2 可以扩充错误分类或接收 deadline/cancellation hook，但不能把可靠性策略下沉到
`buzz-semantic-query` 的纯合同 trait 或 exact-scoring kernel。

### 2.2 Closed operation coordinator

Operation coordinator 继续拥有：

- host/project/request validation；
- total wall-time budget 与 response/cleanup reserve；
- Coordinate、Q0、Qi 的构造和 context observation；
- 是否需要 RR、使用哪个 observation、何时 commit/rollback；
- one-hop scope 和完整路径 root/traversal 策略；
- snapshot 变化时重启短操作、重启完整 attempt 或 fail closed；
- result packing、canonical validation、response-size 校验；
- release request 的 exact-snapshot 或 current-authorization-only policy；
- Event kind、签名、HTTP error 与 CLI-visible error mapping。

完整路径的多跳不是多个独立 Provider query。当前正确的数据流必须保留：

~~~text
Stage A context RR -> close
  -> Q0/Qi single Provider batch
  -> Stage C RR opens
  -> root recall/matrix/coherence
  -> repeated relation/target/coherence scoring for traversal
  -> Stage C RR closes
  -> packing/release/sign
~~~

同一 Stage C RR 内某条 SQL 失败后不得从新 snapshot 继续剩余 traversal。共同层只能返回 typed failure；
是否重做完整 attempt 由完整路径 coordinator 决定。

### 2.3 共享交互式可靠性原语

Phase 2 新增的共同层只负责：

- 一个 operation 提供的 monotonic provider-start/work/snapshot-close/absolute deadline windows；
- caller disconnect、shutdown、deadline 和显式取消的统一 signal；
- logical request 内的物理 Provider attempt ledger；
- Provider circuit token、reservation、wait、final egress confirm 与单次 HTTP attempt；
- Provider retry/backoff 的唯一执行位置；
- request-local validated input/vector reuse metadata；
- typed、content-free failure 与 retry disposition；
- 低基数阶段指标；
- release permit 到同步签名之间“无 await、单次消费”的安全 helper。

共同层不拥有：

- total deadline 数值来源；
- operation budget、priority 或成本权重；
- input 重建和 context 重新观察；
- RR transaction、traversal 或 snapshot 恢复决定；
- public result、wire 或 error DTO；
- queue、公平性或容量策略。

### 2.4 第三阶段资源治理边界

以下事项不进入本计划：

- 新的 process、Provider、DB、traversal 或 hydration queue；
- queue count/bytes、per-Community 或 per-caller backlog；
- weighted scheduling、优先级、成本单位和 work-conserving borrowing；
- Provider fleet-wide rate/concurrency lease；
- PostgreSQL ordinary-work reserve 与 semantic DB session cap；
- background indexing 与 interactive query 的公平份额；
- adaptive concurrency、cross-request batching/cache 或 load shedding policy。

本阶段继续使用现有 semaphore、DB Provider reservation 和 traversal admission。它只保证任何现有 wait
与 future 都能被 deadline/cancel 终止，并且失败后资源按既有语义归还。

## 3. 当前执行基线

### 3.1 Provider attempt

| Operation | 单个 attempt 的输入 | 单个 attempt 的 Provider 调用 | 当前 operation retry |
| --- | --- | --- | --- |
| whole-graph Coordinate | 一个 Coordinate input | 一次 | 无 |
| Coordinate -> incident Edge | 一个 Q0 | 一次 | 无 |
| Edge -> member Coordinate | 一个 Q0 | 一次 | 无 |
| bounded complete path | Q0 + 0..N 个 Qi 的单个 bundle | 一次 | generation/context churn 最多重做一个完整 root attempt |

因此完整路径一个逻辑请求当前可能产生零次、一次或最多两次 Provider 调用；每个 traversal hop 不调用
Provider。Phase 2 的 attempt ledger 必须按物理调用计数，并覆盖 operation restart，防止 provider retry 与
完整路径 churn retry 乘法放大。

### 3.2 RR ownership

- Coordinate：Provider 后打开一个短 RR，执行全图 Coordinate scorer，commit，再 exact release；
- one-hop：Provider 后打开一个短 RR，验证 projection/context observation，执行 scoped search，commit，
  再 exact release；
- complete path Stage A：用短 RR 观察 conditioned context 后立即 commit；
- complete path Stage C：Provider 后打开一个 RR，从 root recall 到完整 traversal 结束一直持有，再 commit；
- `SemanticOneShotExecution` 不持有 RR；`SemanticGraphRootQuerySession` 持有完整路径 Stage C RR。

不存在四个 operation 共用的一条“ticket -> Provider -> traversal -> release”RR 生命周期，Phase 2 也不得
创造这种抽象。

### 3.3 Deadline 与取消

- Coordinate 与 one-hop 使用固定 one-shot hard deadline；
- 完整路径使用 caller budget 派生 work、snapshot-close 与 absolute deadline，并允许 traversal work deadline
  产生合法 `WallTimeExhausted` 部分结果；
- one-shot 当前共同 deadline 没有完整包住 release 后的所有 result build/sign/bridge serialization；
- 完整路径在 packing、postflight、signing 前后检查 absolute deadline；
- 当前没有统一 request cancellation token；timeout/drop 是主要停止手段。

Phase 2 必须保留总预算 owner，但让每个底层 await 接收同一 operation context 的剩余时间。不得为 retry
或每个 hop 重置 deadline。

### 3.4 Release 与签名

- Coordinate 与 one-hop：`confirm_release(expected_snapshot=Some(actual_rr_ticket))`；
- complete path：`confirm_release(expected_snapshot=None)`；
- complete path 已把 `SemanticGraphQueryReleasePermit` 同步消费到签名函数；
- one-shot 当前在 `Permitted(_permit)` 分支丢弃 permit，随后再由 surface 构造并签名 Event。

Phase 2 应统一“permit 只能同步消费到签名”的安全形状，但不能统一 `expected_snapshot` 参数或 result
builder。

### 3.5 当前错误分类缺口

现有公开错误用于兼容输出，不能直接作为服务器 retry 决策：

- `ProviderTransport` 没有区分 connect/pre-handoff 与 request/response outcome unknown；
- Provider 429、明确 5xx、永久 4xx 与无效响应在不同 surface 中有不同公开映射；
- `DbError::Sqlx` 没有区分只读 transient、pre-transaction failure 与 commit outcome unknown；
- `DbError::AccessDenied` 可能来自授权、generation/readiness 或其他 fail-closed检查；
- 公开 `retryable=true` 只表示 caller-facing 建议，不证明服务器可以安全 replay。

内部 failure taxonomy 必须先于任何新 retry 交付。

## 4. 目标内部模型

以下名称冻结责任，不要求实现逐字使用同名 Rust 类型。

### 4.1 `SemanticExecutionContext`

每个 closed operation 在通过外层 request validation 后创建一个 context：

~~~text
SemanticExecutionContext
  deadline_windows {
    provider_start_before
    work_deadline
    snapshot_close_deadline
    absolute_deadline
  }
  cancellation
  lifecycle_latch: Active | Finalizing | Cancelling | TimedOut | Completed
  logical_request_id (仅内存、不可作高基数metric label)
  provider_attempt_ledger
  operation_attempt_ledger
~~~

约束：

- 所有窗口都由closed operation根据现有预算产生；共同层只能读取，不能推导、重置或延长；
- `provider_start_before`决定是否还允许开始一个新物理Provider attempt；`work_deadline`、
  `snapshot_close_deadline`和`absolute_deadline`分别保留operation已有工作、关闭RR和最终收尾边界；
- R2零策略迁移时，one-shot可把没有独立语义的窗口设为同一现有45秒边界；启用R4 retry前，one-shot
  coordinator必须显式提供早于absolute deadline的`provider_start_before`，为RR/release/finalize保留有界尾部；
  共同runtime不得自行猜测该reserve；complete path继续使用已有work/close/absolute tails；
- deadline 使用 monotonic clock，测试使用 fake clock；
- cancellation 聚合 caller disconnect、server shutdown、deadline 和内部 terminal transition；
- `Active -> Finalizing | Cancelling | TimedOut`只能有一个原子赢家；取消或超时先赢时，不得开始新的
  reservation、Provider、语义DB查询、traversal、release或signing；
- rollback、abort、连接丢弃、transaction close和RAII归还属于有界cleanup，即使进入terminal也必须执行；
- `Finalizing`先赢时允许已经开始的同步签名完成；期间若cancel/deadline触发，签名结果在post-check丢弃且不发送；
- provider attempt ordinal 在整个逻辑请求内单调递增，operation restart 不重置；
- context 不保存 query 文本、context overview、Coordinate、vector 或公开 result；
- context 不包含 traversal budget、snapshot policy、release policy、priority 或资源权重。

### 4.2 Provider 单次 attempt

Physical Provider adapter 保持单次语义：

~~~text
encode_once(validated_bundle, attempt_deadline, cancellation)
  -> ProviderEncodedSemanticInputBundle
  | ProviderAttemptFailure {
      kind,
      handoff: NotStarted | ConfirmedResponse | OutcomeUnknown
    }
~~~

adapter负责：

- 单次 HTTP request 的 exact body、credential 与 endpoint；
- configured Provider timeout 与剩余 attempt deadline 取较小值；
- cancel/drop 后不 spawn 或 detach 后台任务；
- bounded success/error body；
- status、`Retry-After`、model、count、order、dimension、finite/non-zero校验；
- 区分明确未开始、明确响应和 outcome unknown。

adapter不负责：

- DB Provider reservation；
- authorization、egress 或 circuit policy；
- retry/backoff；
- generation binding；
- vector复用；
- snapshot、scope、ranking或result。

### 4.3 Provider可靠性执行器

共享执行器对每个物理 Provider attempt 执行：

~~~text
fresh operation attempt plan
  -> circuit fast gate / half-open lease
     -> if rejected: fresh auth/gate check, then caller-visible circuit outcome
  -> existing DB Provider reservation
  -> cancel/deadline-aware wait
  -> circuit epoch/lease revalidation
     -> if stale/rejected: fresh auth/gate check, then caller-visible circuit outcome
  -> final auth/currentness/routing confirmation
  -> final no-wait circuit epoch/lease revalidation
  -> encode_once
  -> typed outcome
~~~

operation attempt plan 只能由 closed coordinator 通过内部`fresh_plan(attempt_ordinal)`回调提供，包括：

- fresh authorized ticket；
- validated input bundle；
- current context egress expectations；
- host-derived Community、caller 与 relay signer；
- operation现有deployment gate和routing trust。

每次物理Provider attempt，包括backoff后的retry，都必须重新调用`fresh_plan`。共同执行器不得直接复用上一
attempt的plan。如果complete-path fresh observation使Q0/Qi输入发生变化，回调必须把控制权交还outer
coordinator重建root attempt，而不是在Provider执行器内继续retry。
这种情况使用`ReturnToOperationForInputRebuild`；输入未变时，Provider retry loop仍只有共同执行器一个owner。

retry时不得复用：

- 旧 authorization ticket；
- 旧 Provider reservation；
- 旧 egress permit；
- 旧 routing assertion；
- 旧 context observation，除非 operation 重验并构造完全相同输入。

authorization/gate的fresh检查必须先于caller可观察的circuit结果，避免向无权caller泄露Provider健康状态。
circuit token在reservation前取得，half-open lease必须排他；若fast gate已open，先做一次无等待fresh
auth/gate确认再返回，不创建reservation。reservation wait后对epoch/lease做无等待重验；若token已失效，
同样先fresh确认caller仍有权观察，再返回circuit结果。token仍有效时执行最终egress confirmation；由于该
DB确认本身是await点，返回后必须紧邻`encode_once`再做一次无等待epoch/lease重验。若此时token已失效，
丢弃未消费的egress permit并返回circuit结果，Provider delta必须为0。若wait或final confirm期间epoch变化，
已commit reservation仍可能被消耗，但open circuit下的新请求不得系统性地先消费reservation或继续外发。

Provider reservation 一旦成功 commit 就是已消费的 rate capacity，任何失败、取消或后续授权变化都不得
refund。若 reservation commit outcome unknown，在没有 idempotency/reconciliation 设计前不得自动重放。

### 4.4 Internal failure taxonomy

内部 failure 至少区分：

~~~text
ContractInvalid
AuthorizationDenied
PolicyDisabled
FleetUnavailable
AdmissionBusy
DeadlineExceeded
Cancelled

ProviderConnectNotStarted
ProviderRateLimited { valid_retry_after }
ProviderRetryableResponse { status_class }
ProviderRejected
ProviderOutcomeUnknown
ProviderProtocolInvalid

DbReadSnapshotTransient { phase, sqlstate_class }
DbReadSnapshotCloseUnknown
DbSnapshotChanged
DbAuthorizationDenied
DbInvariantViolation
ProviderReservationCommitOutcomeUnknown
ReleaseConfirmationTransient { sqlstate_class }
ReleaseConfirmationOutcomeUnknown

ResultInvalid
ResponseTooLarge
SigningFailed
~~~

要求：

- 分类在最接近事实来源的位置产生，不通过字符串或公开HTTP code反推；
- DB transient只接受按effect phase和SQLSTATE冻结的closed allowlist；不在allowlist的`Sqlx`错误一律terminal；
- Provider reservation commit unknown永远terminal；read-only RR故障只有在旧transaction已显式关闭或连接被
  丢弃后才可交回operation；release outcome unknown只有在未签名且从未收到permit时才可重验；
- DB授权、gate、generation/currentness结果应尽可能由同一线性化 observation 返回；
- nested provider/DB错误不得把query、正文、vector、endpoint credential或完整identity写入Display/Debug；
- 每个 surface 的adapter继续映射为原有HTTP status、closed code、`retryable`、body和CLI exit；
- operation restart期间若fresh授权变为denied，最终以授权拒绝为准，不能返回更早的transient错误。

### 4.5 Retry disposition

共同可靠性层只接受closed disposition：

~~~text
Terminal
RetryProviderWithFreshPlan
ReturnToOperationForInputRebuild
ReturnToOperationForSnapshotRestart
RetryReleaseConfirmation
~~~

不提供caller可配置的次数、曲线、jitter、priority或fallback。

安全重试矩阵：

| Failure | Phase 2 disposition | 必要条件 |
| --- | --- | --- |
| Provider connect明确未开始 | executor通过callback取得fresh plan后retry | fresh circuit token + reservation + egress confirm + remaining window |
| Provider 429 | executor通过callback取得fresh plan后retry | 合法Retry-After完整落入operation提供的窗口 |
| Provider明确5xx | executor通过callback取得fresh plan后retry | bounded Provider retry ledger + fresh circuit/reservation/confirm |
| Provider handoff后timeout/断流 | Terminal | 无Provider idempotency key，outcome unknown |
| Provider永久4xx | Terminal | 保持surface公开映射 |
| model/count/order/dimension/vector无效 | Terminal | protocol/contract failure，可计入circuit |
| DB只读typed transient/close unknown | 返回operation | closed SQLSTATE/phase allowlist；必须关闭或丢弃旧RR |
| reservation commit outcome unknown | Terminal | 没有幂等reservation/reconciliation前不重放 |
| generation/context变化 | 返回operation | operation重验输入；禁止共享层擅自重建 |
| one-shot exact release snapshot changed | 返回operation | 可选择丢弃unsigned结果并重做短operation |
| release `Denied` | Terminal/Restricted | 授权拒绝优先，永不retry |
| release `FleetUnavailable` | Terminal/Unavailable | 不重跑已完成operation |
| release DB transient/outcome unknown | 同阶段有界重验 | 仅未签名、未收到permit；使用operation原有release policy |
| input/result/signing | Terminal | 永不retry |
| cancel/deadline | Terminal | 迟到结果不得签名或返回 |

release阶段只允许重做“confirmation”本身，不能重做评分、hydration、traversal或packing。最终公开结果优先级
固定为：已经赢得lifecycle latch的cancel/deadline；fresh `Denied`；one-shot最终
`SnapshotChanged`；`FleetUnavailable`；最后一个closed DB release transient。各surface继续通过兼容基线中
冻结的HTTP/CLI projector输出，不引入共同公开错误。retry耗尽时投影最后一个typed failure，不把它改写成
snapshot conflict；若重验时变为授权拒绝，则授权拒绝覆盖此前transient。

首个启用retry的版本建议：

- Provider transport retry budget在每个逻辑请求内最多1次，并跨operation/root restart共享；
- one-shot物理Provider attempt硬上限为2；complete path保留最多2个root attempt，物理Provider attempt硬上限
  为3，从而允许“一次安全Provider retry + 一次既有churn root restart”但禁止乘法放大到4次；
- 所有backoff使用operation提供的同一组deadline windows，不能吃掉work/close/finalize保留窗口；
- full-jitter参数由server-owned编译期/受控配置定义并进入runtime digest；
- `Retry-After`无法完整落入剩余执行和收尾窗口时不提前请求；
- backoff时不持有RR transaction或traversal permit；
- CLI继续只执行一次HTTP请求，不新增client retry。

除物理Provider ledger外，context还维护closed、request-level计数器：

| Counter | Phase 2硬上限 |
| --- | --- |
| Provider transport retry | 1 |
| one-shot operation/snapshot restart | 1（总operation attempt最多2） |
| complete-path root restart | 1（总root attempt最多2） |
| release confirmation retry | 1（总confirmation attempt最多2） |

这些上限彼此不嵌套生成新预算，并全部受物理Provider attempt硬上限和同一deadline windows约束。complete path
不另设“snapshot-only继续遍历”计数；Stage C失败只能走既有完整root restart。计数与启用矩阵进入compiled
runtime digest。任何计数耗尽都投影最后一个typed failure，不允许tight DB/release loop。

实现必须使用同一个`SemanticExecutionContext`中的独立单调计数器和显式状态转换，不能把Provider retry、
input rebuild与operation restart写成各自重新计数的嵌套递归：

~~~text
Provider failure
  -> consume provider_retry ledger
  -> fresh_plan
     -> input unchanged: next physical Provider attempt
     -> input changed: consume operation/root restart ledger
                       -> return to coordinator for rebuild
~~~

`ReturnToOperationForInputRebuild`不重置已经消费的Provider、physical-attempt或release ledger。即使input变化
发生在每次fresh plan期间，one-shot仍最多一次restart，complete path仍最多一次root restart；任何下一步还要
同时通过对应ledger、物理Provider硬上限和deadline window。

### 4.6 Request-local vector复用

query vector复用不是cache surface。它由operation暂存已有的
`GenerationBoundQueryVector`/bundle，并在fresh ticket下重验：

~~~text
Community
generation_id
source generation contract
model
dimensions
embedding-space fence
ordered channel kinds
encoding contract digests
exact input digests
operation-approved context identity
~~~

全部相同才允许在新RR中复用。任何一项变化都必须重新encode或fail closed。

边界：

- Provider成功、短RR打开/评分失败时，Coordinate或one-hop可由coordinator重开短RR并复用vector；
- 完整路径Stage C失败时不能从失败hop换snapshot继续；coordinator只能重启完整Stage C/attempt；
- conditioned context重新观察后若任何Qi输入变化，整个有序bundle必须重新encode；
- 复用只发生在原逻辑请求和原absolute deadline内；
- 不跨请求、pod或process共享；不写数据库、Redis、文件、日志或metrics。

### 4.7 Provider circuit

Phase 2 circuit只保护共享Provider物理故障域：

~~~text
provider endpoint identity + config epoch + request model
~~~

不按operation、Community、caller或generation分别建立breaker。

规则：

- authorization/gate检查先于caller可观察的circuit-open结果，避免健康状态泄露；
- connect、明确5xx、持续transport失败和protocol-invalid可计入健康失败；
- 429进入独立throttle/cooldown，不计入健康失败率；
- auth、input、empty result、DB、snapshot、cancel不计入Provider circuit；
- `Closed -> Open -> HalfOpen` 使用epoch fencing，旧epoch迟到成功不能关闭新circuit；
- half-open只允许有界真实请求作为探针，不发送合成query；
- circuit-open映射既有Busy/Unavailable，不新增公开code；
- circuit状态和指标不包含query、Community、caller或项目内容。

首版可先使用process-local shadow和isolated single-Relay canary。若没有fleet-shared epoch/lease，资格记录必须
明确不能宣称防止多Pod half-open惊群；共享Provider fleet-wide容量与lease仍由第三阶段设计。

### 4.8 Release与同步finalize

共同helper只冻结以下顺序：

~~~text
operation-built and validated unsigned result
  -> operation-specific DB release confirmation
  -> single-use release permit
  -> atomic Active -> Finalizing arbitration
  -> no await
  -> synchronous sign/finalize consuming permit
  -> deadline/terminal post-check
  -> emit response
~~~

helper不决定：

- `expected_snapshot=Some`还是`None`；
- request/result binding；
- Event kind/tags/content；
- response packing和byte cap；
- surface公开错误。

如果deadline/cancel在release前触发，不得调用release或sign。取得permit后，finalizer与cancel/deadline通过
lifecycle latch竞争：

- `Cancelling/TimedOut`先从`Active`转换成功：丢弃permit，不开始签名；
- `Finalizing`先转换成功：允许已开始的同步签名完成，期间不再开始其他semantic work；
- 若cancel/deadline在同步签名期间到达，post-check丢弃已签结果且不发送response；
- mandatory rollback/abort/RAII cleanup不受“禁止新semantic work”限制；
- 同步签名失败则terminal，不重试operation，也不生成第二个Event。

release permit产生后不得等待外部资源，也不能复制、缓存或跨task传递。该线性化合同替代不可实现的
“墙钟超时后任何CPU签名指令都不会继续”要求。

release permit是被move消费的线性能力，不是由通用cleanup回收的RAII资源。`Finalizing`赢家必须先把permit
从共享状态移出并同步传入签名函数，之后才能drop finalizer guard或执行其余cleanup；cancel/timeout赢家则只
能drop未消费permit且不得签名。transaction、连接与semaphore的rollback/RAII归还必须与已move的permit分离，
不能在`Finalizing`赢家路径中把permit误判为“未消费”并提前drop。

## 5. Operation接线设计

### 5.1 Whole-graph Coordinate discovery

保留：

- Coordinate-specific canonical input bytes和contract digest；
- type filter在scoring/K+1前应用；
- direct cosine、no floor、Coordinate canonical tie和result shape；
- Provider后generation-compatible read admission；
- 对实际RR ticket执行exact release。

接入：

- operation从固定one-shot预算创建deadline windows与cancellation；
- Provider阶段使用共同可靠性执行器；
- 短RR失败由Coordinate coordinator决定是否以同一vector重开；
- unsigned result在release前完成validation；
- exact release permit同步消费到40913 Event签名。

### 5.2 Coordinate -> incident Edge

保留：

- one-hop Q0字节；
- incident relation Document scope、Edge=max Document、preview/coverage/omission；
- 与另一个one-hop operation共享40914 tagged family；
- RR projection generation/context revision必须匹配pre-Provider observation；
- exact release和现有one-hop公开错误族。

接入：

- 使用同一个one-shot execution context和Provider可靠性执行器；
- scoped search不内嵌retry；
- snapshot变化返回one-hop coordinator；
- release permit同步消费到40914对应variant签名。

### 5.3 Edge -> member Coordinate

保留：

- complete Edge membership、可选closed Coordinate type filter；
- filter-before-score/K+1、filtered coverage和preview/read descriptor；
- 与IncidentEdges variant的字段隔离；
- one-hop snapshot/release/error合同。

可靠性接线与5.2相同，不新增第三套one-hop runtime。

### 5.4 Bounded complete path

保留：

- Stage A context observation；
- 每个root attempt一次Q0/Qi Provider batch；
- Stage C单一长RR；
- root recall/matrix/MMR、relation/target/coherence、frontier和packing；
- work/snapshot-close/absolute deadline与`WallTimeExhausted`合法部分结果；
- generation/context churn最多第二个完整root attempt；
- `expected_snapshot=None` release。

接入：

- outer coordinator创建现有`QueryDeadlines`并把absolute/work cutoff映射到共同context；
- Stage A和input rebuild继续由complete-path owner执行；
- 每个root attempt的Provider阶段使用共同可靠性执行器；
- 现有churn retry消费独立root-attempt ledger；Provider transport retry budget跨两个root attempt共享，二者共同
  受complete-path物理Provider attempt上限3约束，不能相乘到4次；
- Provider成功后vector bundle由root/traversal session继续持有；
- Stage C DB失败只返回typed failure，不能由scorer或runtime更换snapshot；
- traversal、partial result、packing和current-authorization release继续由complete-path coordinator负责；
- 已有release permit同步签名路径作为one-shot迁移的安全oracle。

## 6. 兼容与安全边界

### 6.1 Protected公开合同

Phase 2默认保持：

- 三个exclusive query extension和virtual result kind；
- NIP-98 exact-body、host-derived Community/caller和request binding；
- capability、deployment master、Community gate、fleet trust与stable signer要求；
- canonical input bytes、Provider batch shape与generation binding；
- candidate scope、ranking、floor、tie、budget、coverage、truncation和completion；
- snapshot observations、one-shot exact release与complete-path current release；
- result content/tags/signature verifier和response cap；
- HTTP status、closed code、`retryable`、error body和CLI exit category；
- feature-off/gate-off/capability-off/pre-auth failure时零Provider egress；
- CLI no-redirect、no-auto-replay和一次HTTP请求。

可靠性有意变化只允许改变：

- 一个逻辑请求内部是否发生安全Provider retry；
- 现有transient failure是否在原deadline内恢复成功；
- Provider attempt count和backoff latency；
- exact-compatible snapshot失败后是否由operation重启；
- caller disconnect/shutdown的传播速度；
- circuit-open时是否更早使用既有Busy/Unavailable返回。

任何公开错误字段或snapshot/release语义变化都需要独立版本化设计，不得混入本计划。

### 6.2 授权与currentness

- 每个物理Provider retry都必须重新取得fresh DB ticket、circuit token、reservation和最终egress confirmation；
- snapshot/operation restart只执行其closed operation要求的fresh auth/currentness/snapshot fence；release retry只
  重做operation-specific release confirmation，不申请未消费的Provider egress permit；
- process-local circuit不能替代host/project/auth/gate检查；
- reservation wait后的permit不能由reservation本身充当授权；
- release前继续在DB writer fence下重验principal、gate、readiness和fleet；
- fresh授权拒绝优先于先前的transient Provider/DB failure；
- 不返回旧缓存result，不放宽current-head，不允许部分Hyperedge或未签名降级。

### 6.3 资源线性化

- 现有admission wait/backoff期间不持有RR transaction或traversal permit；
- Provider reservation commit后rate capacity已消费且不refund；
- 每个物理attempt必须使用新的reservation和egress confirmation；
- Provider future、DB future和retry sleep都不得spawn/detach；
- cancel/deadline赢得lifecycle latch后不得发放新permit或开始新semantic work；已进入`Finalizing`的同步工作
  可完成但结果必须post-check，mandatory rollback/abort/close仍必须执行；
- transaction、semaphore permit和attempt guard必须通过RAII/显式close在所有路径归零；
- 本阶段不改变现有semaphore容量、取得顺序或DB pool预算，除非单独资源治理计划批准。

### 6.4 隐私

日志、错误、metrics、circuit key和资格产物不得记录：

- query、context overview、title、summary或正文；
- embedding或query vector；
- API key、NIP-98 body或private key；
- raw Community/caller/request/Coordinate/Document identity；
- Provider response body、完整endpoint URL或credential-bearing header。

允许的低基数标签：

~~~text
operation
surface
stage
failure_class
retry_disposition
attempt_ordinal_bucket
cancellation_source
circuit_state
outcome
~~~

## 7. 可观测性

至少提供：

- logical requests entered/succeeded/failed/cancelled；
- physical Provider attempts、成功、429、5xx、connect failure、outcome unknown与protocol-invalid；
- retry count、reason、backoff范围和remaining-deadline拒绝；
- vector reused/reencoded/reuse-rejected；
- snapshot restart requested/completed/exhausted；
- release permitted/denied/snapshot-changed/fleet-unavailable；
- per-stage latency与end-to-end latency；
- cancellation/timeout赢得latch后新增semantic work的违规计数，以及`Finalizing`期间cancel导致result丢弃计数；
- transaction/permit/attempt guard leak测试指标；
- circuit state transition、half-open probe和epoch；
- compiled reliability runtime contract digest。

必须区分：

~~~text
logical request count
physical Provider attempt count
operation attempt count
snapshot attempt count
~~~

否则无法发现retry放大或把完整路径第二root attempt误算为第二个用户请求。

## 8. 实现阶段

### R0：规范与characterization收口

> 当前状态：已交付（2026-08-16）。可靠性characterization manifest
> `semantic_retrieval_reliability_characterization_v1.json`（SHA-256
> `028d18d30b5ebe165858dadb62aa8d3e80df68b8c5521074686f9c66ff47fb18`）与
> `just semantic-retrieval-reliability` gate 已落地；三个gate（compatibility、computation、
> reliability）在R0工作树上全部实际运行通过。历史v1 oracle与Phase 1 differential两个
> tracked manifest/hash保持原值，未回写。

目标：在写生产runtime前固定正确边界。

交付：

1. 更新上位spec第6、10节：共同层接收operation deadline，不拥有完整路径总预算或traversal；
2. 将新bounded queue承诺完整移到第三阶段资源治理；
3. 明确Phase 1是共享计算基座，不是统一operation engine；
4. 扩充兼容manifest：四operation的Provider attempts、RR数量/生命周期、release参数、deadline和现有retry；
5. 增加“完整路径每hop零Provider调用”的characterization；
6. 冻结one-shot permit当前形状作为known gap，不把它误写为目标合同；
7. 同步目录迁移后的检查脚本与TODO链接，证明compatibility/computation gate读取当前canonical文档；
8. 新建Phase 2 protected-surface和生产diff allowlist。

退出门：文档、manifest和当前代码事实一致；不修改生产行为。

### R1：typed failure与执行上下文

> 当前状态：已交付（2026-08-16）。`crates/buzz-relay/src/semantic_query_runtime.rs`
> 落地 §4 typed execution-context layer（15 unit tests）：
> `SemanticCancellation`/`Handle` first-wins聚合、`SemanticLifecycleLatch`
> 单CAS仲裁（Finalizing先赢则post-check丢弃）、`SemanticDeadlineWindows`
> （`provider_start_before ≤ work ≤ snapshot_close ≤ absolute`，含R2零策略
> one-shot全等窗口构造）、`SemanticAttemptLedger`（one-shot物理2 /
> complete-path物理3、transport retry token跨operation restart共享、
> operation attempts 2、release confirmation 2）、
> `ProviderHandoffCertainty`+`ProviderAttemptFailure`（当前
> `ProviderTransport`保守映射为OutcomeUnknown）、`SemanticReliabilityFailure`
> 全变体与closed retry disposition矩阵、content-free
> `failure_class`标签、`SemanticExecutionContext`聚合。
> `crates/buzz-db/src/error.rs` 落地 `SemanticDbEffectPhase`、
> `SemanticDbSqlstateClass`（冻结SQLSTATE allowlist）、
> `SemanticDbFailureKind` 与 `DbError::semantic_failure_kind`（3 unit
> tests），经 `buzz_db` 根re-export。
> 对计划的一处偏离：交付项1的fake clock未引入——deadline windows以
> `Instant` 为合同、测试用确定性显式Instant构造，无任何sleep；clock
> 抽象推迟到R2执行器真实需要注入时再设计。
> 零行为验证：模块未接线（`#![allow(dead_code)]`，R2起逐operation接入），
> 本工作树三个gate（compatibility、computation、reliability `all`，含
> freeze-diff）+ `cargo clippy -D warnings` + `cargo fmt` 全部实际运行通过。

目标：建立可靠性判断所需类型，不启用retry。

交付：

1. `SemanticExecutionContext`、operation-provided deadline windows、fake clock、cancellation与lifecycle latch；
2. request-level Provider、operation/snapshot与release attempt ledgers；
3. Provider handoff certainty与typed attempt failure；
4. DB authorization/currentness/read-transient/commit-unknown分类；
5. content-free Debug/Display和统一指标schema；
6. 所有新类型default path保持单attempt、no-backoff、no-circuit行为。

退出门：三个surface的Provider bytes、attempt数、RR数、result/error逐项与baseline一致。

### R2：共享Provider可靠性执行器，零策略迁移

迁移顺序：

1. whole-graph Coordinate；
2. one-hop tagged family的两个variant；
3. bounded complete path的每个root attempt。

每步：

- 使用同一reservation/wait/egress/encode-once primitive；
- 不启用新retry、backoff或circuit拒绝；
- request开始时pin legacy或migrated route，处理中不fallback；
- acceptance compare只能test/acceptance build使用同一输入和已注入Provider outcome，不允许真实流量双发；
- 运行兼容manifest、operation differential和公开错误golden。

退出门：四operation均接入共同Provider执行器，production行为为零差异。

### R3：deadline、cancellation与release-finalize

目标：让一次operation的现有deadline覆盖所有底层等待和收尾，并正确处理取消。

交付：

1. bridge创建/传播disconnect与shutdown cancellation；
2. Provider wait/call、DB acquire/query、traversal、hydration、retry sleep全部使用同一context；
3. one-shot result build/sign/serialization纳入现有absolute deadline检查；
4. one-shot exact release permit同步消费到Event签名；
5. complete-path现有partial-result与deadline tails逐字保持；
6. 每个await边界的cancel/fault injection与资源归零断言。

退出门：cancel/deadline赢得latch后零新增外发/语义DB/traversal/signing阶段；mandatory cleanup完成；若
`Finalizing`先赢，允许同步签名完成但cancelled/timed-out结果不得发送。所有公开timeout/error保持兼容。

### R4：安全retry、backoff与request-local vector复用

启用顺序：

1. Provider明确pre-handoff connect failure；
2. Provider 429且完整Retry-After可落入剩余窗口；
3. Provider明确retryable 5xx；
4. one-shot typed只读DB/snapshot recovery；
5. complete-path将现有churn retry接入同一attempt ledger；
6. exact-compatible vector reuse；
7. typed release-confirmation DB transient/outcome-unknown重试，仅限未签名、未收到permit且operation-specific
   auth/snapshot policy重新确认；`Denied`和`FleetUnavailable`不进入该retry。

每项独立route、单独failure matrix和canary；不一次启用全部策略。

退出门：Provider、operation/snapshot与release ledger均不超过closed上限；不存在nested retry或tight loop；fresh
auth优先；vector不跨generation/input；snapshot不拼接；公开错误保持兼容。

### R5：共享Provider circuit

交付：

1. content-free Provider failure-domain key和config epoch；
2. `Closed/Open/HalfOpen` state、single-probe与epoch fence；
3. 429 throttle与health failure分离；
4. shadow metrics和故障注入；
5. process-local isolated canary；
6. capability/gate/auth先于caller-observable circuit outcome；
7. fast gate/half-open lease在Provider reservation前取得，wait后及final egress confirm后各以epoch token
   无等待重验，最后一次重验必须紧邻Provider调用；
8. fleet-wide限制写入资格记录，未有shared state时不宣称多Pod防惊群。

退出门：四operation共享同一个Provider故障域；一个operation不能绕过open circuit或制造独立probe风暴。

### R6：资格、rollout与文档收口

交付：

1. deterministic fault matrix；
2. disposable DB currentness/release race；
3. fake Provider retry/circuit/attempt证据；
4. gated真实Provider短canary，不保存query/vector/body；
5. cancellation与shutdown soak；
6. fleet runtime digest与同质性验证；
7. qualification记录、上位spec、TODO、README/current-status同步；
8. 记录Phase 1 computation窗口的既有owner、日期、当前状态，以及Phase 2正常路径和差分oracle是否仍依赖
   Phase 1 legacy computation的证据；R6不替代Phase 1在`2026-09-16`到期时执行的独立删除/延期change；
9. 为每次真实fleet old→new reliability digest切流记录精确digest、时间、owner、rollback binary与演练结果，
   并为最终目标reliability digest建立独立legacy删除窗口；
10. 明确第三阶段仍未交付的queue/fairness/capacity事项。

完成R6后可声明“统一可靠性原语与Provider执行层已交付”，不能声明统一资源治理或production SLO完成。

## 9. 测试矩阵

### 9.1 Pure与fake-clock

- deadline从operation注入且retry/hop不重置；
- provider-start/work/snapshot-close/absolute窗口保持operation提供值，runtime不自行推导tail；
- remaining window不足时不开始backoff/attempt；
- cancellation与terminal latch幂等；
- attempt ordinal跨operation restart递增；
- Provider、operation/snapshot和release counters分别耗尽且无法形成tight loop；
- nested retry无法超过one-shot 2 / complete-path 3的物理Provider上限；
- circuit transition、epoch、single half-open probe；
- 429 cooldown不污染health failure；
- content-free Debug/Display/metrics labels；
- vector reuse key对每个fence/input变化都拒绝。

### 9.2 Provider adapter

- pre-connect failure、429、5xx、permanent 4xx、timeout、response-read断流分类；
- request可能已交付时必须是OutcomeUnknown且不重试；
- success/error body cap；
- model/count/order/dimension/nonfinite/zero验证；
- configured timeout与remaining attempt deadline取较小值；
- cancel/drop后无detached HTTP work；
- retry每次都重新调用`fresh_plan`并发生fresh circuit/reservation/egress confirm；
- fresh plan发现auth撤销时Provider delta为0；发现complete-path Q0/Qi输入变化时返回coordinator重建，旧bundle
  不进入下一物理attempt；
- reservation commit unknown不自动重放。

### 9.3 DB与snapshot

- auth、membership、ban、gate、generation、readiness、fleet outcome准确分类；
- DB transient按effect phase与closed SQLSTATE allowlist分类，未列举`Sqlx`错误不retry；
- reservation wait后revocation导致零Provider egress；
- RR只读transient关闭transaction后才返回recovery hint；
- exact scorer内部不retry；
- one-shot新RR结果不混入旧RR；
- complete-pathStage C错误不能从失败hop换snapshot继续；
- complete-path context/input变化强制reencode；
- one-shot exact release与complete-path `expected_snapshot=None`均保持；
- release `Denied`、`SnapshotChanged`、`FleetUnavailable`、DB transient和outcome unknown逐项命中closed
  disposition、次数上限与最终公开错误优先级；
- release denied后无签名；
- permit同步消费且不能复制/二次使用。

### 9.4 Operation differential

四operation分别断言：

- exact Provider input bytes和bundle顺序；
- Provider attempt、RR transaction和release调用数量；
- fixed-vector score/ranking/result；
- type filter、scope、coverage、omission、truncation和path不变；
- HTTP status/code/retryable/body与CLI exit不变；
- success Event kind/tags/content/request binding/signature不变；
- feature/gate/capability/auth失败时Provider delta为0。

完整路径额外断言：

- 单个root attempt只有一个Provider batch；
- traversal每hop Provider delta为0；
- Stage C始终是单一RR；
- current churn最多创建第二个root attempt；没有Provider transport retry时总物理attempt仍为2；
- 一次安全Provider retry与随后一次既有churn restart组合时允许最多第三个物理attempt；
- `WallTimeExhausted`仍是合法partial result；
- Provider retry与root retry不能产生第四个物理调用。

### 9.5 Cancellation与资源泄漏

在以下边界逐点取消：

~~~text
ticket
reservation
provider wait
egress confirm
provider send/read
RR open
exact score
relation/target rank
hydration
RR commit/rollback
packing
release
Active -> Finalizing arbitration
synchronous signing/post-check
~~~

每次断言：

- cancel/timeout先赢时不得开始签名且无response；
- `Finalizing`先赢时允许且只允许一次同步签名；期间到达cancel/timeout时post-check必须丢弃结果且无response；
- finalizer赢家把permit move并消费到签名后才drop guard；cancel/timeout赢家只drop未消费permit；通用cleanup
  不得再次取得或处理已move permit；
- DB transaction归零；
- process/traversal permit归还；
- 已commit rate reservation不refund；
- terminal后Provider/语义DB/traversal新增工作为0，且mandatory cleanup已完成；
- 无query/vector/content进入日志与资格产物。

### 9.6 Circuit与真实Provider

- 四operation共享同一process-local Provider circuit；
- auth失败不能探测circuit状态；
- circuit已open的新请求在reservation前拒绝；若wait期间撤权且circuit同时open，fresh auth拒绝优先且不泄露
  circuit状态；
- circuit token在final egress confirm阻塞期间变stale/open时，confirm返回后Provider delta为0；
- 一个operation触发open后其他operation不继续外发；
- half-open只有一个真实probe；
- 旧epoch迟到成功不关闭新open；
- 429只触发throttle；
- isolated真实Provider canary验证attempt总数、错误类与feature-off rollback；
- 不冻结真实vector、精确score、精确latency或Provider错误正文。

## 10. 文件与模块影响面

预计主要影响：

- `crates/buzz-relay/src/semantic_provider.rs`：单次attempt与handoff-aware failure；
- `crates/buzz-relay/src/semantic_one_shot.rs`：改为one-shot coordinator/共同原语client；
- `crates/buzz-relay/src/semantic_coordinate_search.rs`；
- `crates/buzz-relay/src/semantic_one_hop_search.rs`；
- `crates/buzz-relay/src/semantic_graph_query.rs`；
- `crates/buzz-relay/src/semantic_graph_traversal.rs`；
- `crates/buzz-relay/src/api/bridge.rs`；
- `crates/buzz-relay/src/state.rs`与`config.rs`：cancellation/circuit及runtime digest配置；
- 一个新的交互式可靠性模块，建议命名`semantic_query_runtime.rs`或等价名称；
- `crates/buzz-db/src/error.rs`与`semantic_query.rs`：typed failure和release/egress outcome；
- `crates/buzz-semantic-query/src/fleet.rs`：compiled reliability contract digest/route；
- compatibility、fault、qualification tests和本目录文档。

现有`crates/buzz-relay/src/semantic_runtime.rs`是durable后台embedding worker，不得改造成交互式query runtime，
也不得混合后台job retry与HTTP request cancellation语义。

默认不需要修改：

- semantic source extractor和worker job schema；
- Project Context canonical graph、Edge或Coordinate模型；
- source embedding表、pgvector索引和Phase 1 scoring SQL；
- SDK/CLI公开DTO与命令；
- Desktop/Web；
- migrations/schema。若后续为fleet-shared circuit引入持久状态，必须另行expand/rollback设计，不能在本计划
  中隐式加入。

## 11. 质量门

计划新增统一入口：

~~~bash
just semantic-retrieval-reliability
~~~

该入口至少执行：

- reliability contract/fake-clock tests；
- Provider attempt/failure/circuit tests；
- four-operation compatibility differential；
- cancellation/resource-leak fault matrix；
- DB currentness/release race；
- manifest和runtime digest verifier；
- privacy/content-free scan。

阶段性与最终门：

~~~bash
just semantic-retrieval-compatibility-baseline
just semantic-retrieval-computation
just semantic-retrieval-reliability
just semantic-test
just test-unit
just ci
~~~

带真实DB或Provider的资格可独立、明确运行；未运行必须在qualification中说明，不能由unit绿色替代。

## 12. Rollout与rollback

### 12.1 Route

每个operation在请求开始时由server-owned compiled route选择legacy或migrated reliability path。正常生产build
不包含caller-selectable route，也不包含acceptance compare。单请求一旦pin route：

- 不自动fallback；
- 不双发Provider；
- 不因内部failure切换implementation；
- retry继续使用同一路径和同一个request-level ledger。

Acceptance compare只能在`cfg(test)`或显式acceptance-only binary/feature中存在，默认production build不可达。

### 12.2 Fleet同质性

reliability route、retry类别、物理attempt上限、backoff合同、circuit合同和vector reuse合同进入compiled runtime
digest。正常attested fleet禁止混合reliability route/config/digest。

切换顺序：

1. deterministic/fault gates；
2. isolated trusted-single-Relay canary；
3. 关闭公开gate并drain；
4. 整fleet部署同一route/digest；
5. 重新attest并广告capability；
6. 小范围授权Community；
7. 扩大真实Provider短窗与故障资格；
8. 保留旧路径到rollback窗口结束。

### 12.3 Rollback

rollback必须：

1. 停止新admission；
2. 取消现有admission wait/backoff中的请求；
3. 让accepted请求在原absolute deadline内结束或取消；
4. gate off并drain；
5. 整fleet部署legacy route/digest；
6. 重新attest后再恢复capability。

不得请求内fallback、混跑fleet或通过增加attempt上限掩盖故障。process-local circuit可以随进程重启清空；
资格记录必须明确这不等于fleet-wide健康状态恢复。

### 12.4 强制abort与legacy删除门

出现以下任一信号必须立即停止扩张、gate off并执行12.3，而不是继续观察：

- 公开success/error/CLI differential不一致；
- Provider、operation/snapshot或release attempt超过compiled上限；
- 未授权/feature-off请求产生Provider egress，或circuit-open请求系统性先消费reservation；
- complete path跨RR继续、one-shot混合snapshot或release policy被改写；
- cancel/timeout仲裁后出现迟到response、资源泄漏或新的semantic work；
- fleet route/digest异构、privacy扫描失败、query/vector/content进入日志或资格产物。

必须分别管理两套不能混用的rollback生命周期：

1. **Phase 1 legacy computation source**：当前U6 compiled runtime digest为
   `e49d7ae9e69a2818a9ce9c061443a4441d332c86a3f8b46824b147a5da716f40`；legacy Coordinate SQL与完整路径
   compatibility adapter的既定删除日期是`2026-09-16`。该日期不是Phase 2 R0–R6必须全部完成的期限。
   到期前必须二选一：若Phase 1自身删除门已满足且Phase 2不再把这些实现作为差分oracle，则按独立change
   删除；若Phase 2仍依赖它们，则记录延期原因、owner和新的明确日期，不能静默永久保留。
2. **Phase 2 reliability transitions**：R2零策略接线、R4逐项启用retry/recovery、R5启用circuit都可能形成
   不同compiled digest。每次进入真实fleet的old→new切流都必须记录精确source/target digest、cutover时间、
   owner、可部署rollback binary与演练结果；不能用第一个R2窗口覆盖后续R4/R5变化。第一次source digest可以
   是U6的`e49d7ae9…`，但该U6 profile已经是四operation migrated computation，并不等于启用Phase 1 legacy
   scorer/adapter；后续source则是前一已部署reliability digest。
3. **Phase 2 legacy reliability deletion window**：只从计划声明的最终目标reliability digest完成最后一次真实
   fleet同质切流时开始，单独记录开始日期、结束日期与owner。若最终目标策略随后再变，新的digest transition
   必须重新记录，删除窗口也从新的最终cutover重新计算。该窗口保护的是“旧可靠性编排 + migrated
   computation”及对应可部署binary，不自动继承`2026-09-16`。

R2接线及后续retry/circuit合同会产生新的compiled runtime digest；进入真实fleet时必须gate/drain、整fleet同质
部署并重新attest。Phase 2 legacy reliability source至少保留到：最终目标digest下四operation整fleet同质迁移完成；
deterministic、disposable DB和真实Provider资格均通过；其独立rollback窗口结束；并完成一次整fleet
gate/drain/redeploy/re-attest的binary rollback演练。任一窗口到期仍有未关闭信号时必须显式延期，R6不得把
对应legacy source标为可删除。

删除任一legacy source都必须是单独change，重新跑适用的完整门并保留上一个可部署binary；删除后不再宣称
flag级回滚。Phase 2 differential默认应使用“旧可靠性编排 + migrated computation”作为oracle，避免为了
可靠性迁移无条件延长Phase 1旧计算实现。

### 12.5 当前目录迁移前置项

当前阶段文档目录已从带空格的`semantic/ unified-engine`迁移到`semantic/unified-engine`。R0必须先同步
compatibility/computation检查脚本与`docs/stage/TODO.md`中的硬编码路径，并让现有两个gate实际运行；不能用
绿色但未读取目标文件的脚本作为Phase 2基线证据。

已在R0交付：兼容基线脚本的QUALIFICATION路径与状态标记已指向当前canonical文档（修复前manifest-only
scope实际失败于missing compatibility baseline record），`docs/stage/TODO.md`残留带空格链接已修正；两个
既有gate在新脚本上以manifest-only与all scope实际运行通过。历史commit区间freeze-diff allowlist中的
带空格路径属于历史路径，保持原样。

## 13. 风险与禁止项

禁止：

- `if public_error.retryable { retry() }`；
- retry所有`reqwest`、`DbError::Sqlx`或`AccessDenied`；
- 在`SemanticInputEncoder`、exact scorer、CLI或operation内部再造独立Provider retry；
- 把一个hop当成新的Provider query；
- DB错误后从新RR继续剩余完整路径；
- retry时沿用旧ticket、reservation、egress permit或routing assertion；
- handoff outcome unknown或reservation commit unknown时盲目重放；
- 每次retry重置deadline；
- backoff时持有RR、lock或traversal permit；
- 将query vector写入PostgreSQL、Redis、文件、日志或跨请求cache；
- 用旧result、部分Hyperedge、放宽currentness或未签名内容降级；
- circuit在authorization前返回；
- 给每个operation建立独立Provider circuit；
- 将bounded queue、公平性、priority或容量重构偷渡进Phase 2；
- 把后台semantic worker的durable retry与交互式request cancellation合并；
- 用万能trait、动态map或caller-controlled DSL替代closed operation coordinator。

主要风险与控制：

| 风险 | 控制 |
| --- | --- |
| runtime接线改变公开错误 | exact HTTP/CLI golden + legacy/migrated differential |
| nested retry放大Provider调用 | 分离Provider/root ledger + one-shot max2 / complete-path max3硬门 |
| fresh-plan与operation rebuild形成递归预算 | 同一context独立单调ledger + 显式状态转换 + 不重置计数 |
| vector跨generation/input复用 | fresh ticket binder + exact ordered input digest |
| snapshot拼接 | scorer无retry + operation-owned RR/restart |
| permit cleanup早于签名或被二次处理 | finalizer赢家先move并消费permit + cleanup所有权分离 |
| 取消与签名竞态 | `Active -> Finalizing \| Cancelling/TimedOut`仲裁 + post-check丢弃 |
| circuit泄露健康状态 | auth/gate先行 + content-free公开映射 |
| Provider重复计费 | outcome unknown不retry + 每attempt新reservation |
| complete-path partial result回归 | fake clock work/close/tail资格 |
| Phase 2与资源治理混淆 | 无新queue/capacity/fairness的diff allowlist |

## 14. 完成定义

Phase 2完成必须同时满足：

1. 四个逻辑operation的Provider阶段都使用同一可靠性执行器；
2. `SemanticInputEncoder`和exact scorer仍保持单次计算、无策略的Phase 1边界；
3. operation coordinator继续拥有total deadline、RR、traversal、restart scope、partial result与release policy；
4. 全部await路径接收同一operation-provided deadline/cancellation；
5. Provider/DB failure有typed、content-free内部分类，公开错误逐字段兼容；
6. Provider retry只有一个owner，物理attempt有统一硬上限且不发生乘法放大；
7. unsafe/ambiguous Provider和DB outcome不自动replay；
8. request-local vector只在fresh ticket证明exact compatibility时复用，且从不持久化；
9. scorer不透明retry，完整路径不跨RR拼接；
10. one-shot与complete-path各自的snapshot/release/partial-result合同保持；
11. 所有成功Event都经过operation-specific release，并同步消费permit到签名；
12. cancel/deadline先赢时零新增外发、语义DB、traversal或签名阶段；finalize先赢时只允许同步完成并在迟到
    cancel/deadline下丢弃结果；mandatory cleanup完成且资源无泄漏；
13. 四operation共享一个Provider circuit故障域，且限制被准确记录；
14. compatibility、computation、reliability、semantic DB、unit和CI门通过；
15. qualification、spec、TODO和状态文档同步；
16. Phase 1 computation窗口有既有owner、日期、当前状态和Phase 2依赖证据；未到期时不得被误标为可删除，
    到期删除/延期由Phase 1独立change负责，不把Phase 2完成强行绑定到该日期；
17. 每个实际reliability digest transition均有old/new digest与rollback证据，最终目标digest的legacy删除窗口
    有独立owner、日期和保留结论；
18. 文档明确第三阶段queue、capacity、公平性和production SLO仍未交付。

达到这些条件后，可以声明：

> Project Context交互式语义检索已经共享可靠性执行上下文、Provider attempt/retry/circuit、typed failure、
> cancellation/deadline和release-finalize安全原语；四个closed operation仍独立拥有查询与遍历语义。

不能声明：

- 完整路径已经变成统一引擎自动多跳；
- 四个operation共享同一RR或同一release policy；
- query vector成为跨请求cache；
- bounded queue、Community/caller公平、统一资源容量或production SLO已经完成。
