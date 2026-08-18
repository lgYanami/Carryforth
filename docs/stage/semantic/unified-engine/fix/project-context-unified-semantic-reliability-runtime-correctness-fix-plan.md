# Project Context 统一可靠性运行时正确性修复计划

> 状态：修复中——F0 已交付（状态修正 + 7 个 rfx 红色基线 + 机械门清单断言，RFX-06 红色证据
> 按计划记录为 F4 test-first 条件式）；F1 已交付（target-window admission、`TimedOut` 真实
> latch、`Finalizing` stage 所有权、one-shot eighths reserve、shutdown 订阅与 caller guard，
> RFX-01/RFX-02 关闭；runtime digest 随日期化 descriptor 轮换
> `2c898e16… → 36776253…`）；F2–F5 未开始。`just test-unit` 中 rfx03/rfx04/rfx05 的红色是
> 本计划的预期基线状态（分别等待 F2/F3）
>
> 日期：2026-08-18
>
> 审计代码基线：`feat/semantic-engine`，`1d8be46434cacd97995a57b4dae94fee9525cefc`
>
> 关联文档：
> [统一可靠性运行时实现计划](../project-context-unified-semantic-reliability-runtime-implementation-plan.md)、
> [统一可靠性运行时资格记录](../project-context-unified-semantic-reliability-runtime-qualification.md)、
> [统一语义检索引擎规范](../project-context-unified-semantic-retrieval-engine-spec.md)
>
> 本计划只修复 Phase 2 可靠性运行时的正确性、生命周期和资格证据；不交付第三阶段资源治理

## 0. 结论与处理原则

当前代码已经交付了共享 `SemanticExecutionContext`、Provider egress、typed failure、retry/backoff、
request-local vector reuse、process-local circuit 和 release-finalize 原语，四个逻辑 operation 也已经接入
主体路径。因此本次不回滚整套 Phase 2，也不重新设计查询引擎。

但是，代码审计确认部分关键实现与已冻结计划不一致，其中完整路径 deadline tail 已形成直接功能错误；
其余问题破坏了 lifecycle、circuit、Provider attempt 和 release permit 的安全合同。准确状态应为：

> Phase 2 主体实现和确定性测试框架已经落地，但 correctness 修复与完整资格尚未完成；修复关闭前，不能
> 声明统一可靠性运行时已按实现计划完整交付，也不能声明 production qualification 完成。

修复遵循以下原则：

1. 保留四个 closed operation、三个公开 surface 和现有 Event kind；
2. 不改变 query input、query vector、exact score、scope、ranking、coverage 或 path 语义；
3. 不改变 one-shot `expected_snapshot=Some(actual)` 与 complete-path `expected_snapshot=None` 的差异；
4. 不增加 Provider physical-attempt 上限，不以更多 retry 掩盖错误；
5. 不新增跨请求 vector cache，不写 PostgreSQL、Redis 或日志；
6. 不新增 queue、fairness、capacity、跨 Pod circuit 或 production SLO；
7. 先补能稳定复现问题的失败测试，再修改生产实现；
8. 公开 HTTP status、closed error、`retryable`、response body 与 CLI exit 保持兼容。

## 1. 已确认问题

| ID | 严重度 | 问题 | 直接影响 |
| --- | --- | --- | --- |
| RFX-01 | P0 | Deadline admission 检查任意较早窗口，且 R4 one-shot 仍没有内部收尾 reserve | 完整路径合法 partial 无法发布；one-shot retry可占满公开 hard deadline |
| RFX-02 | P1 | timeout 状态、`Finalizing` 与生产 cancellation 接线不符合 lifecycle 合同 | `TimedOut` 实际不可达；finalize 后仍可启动新 semantic stage；shutdown/disconnect 不能中断所有 await |
| RFX-03 | P1 | one-shot release permit 与 unsigned result/signing 顺序错误 | release 前没有完成结果验证，permit 没有被线性消费到签名 |
| RFX-04 | P1 | Circuit 拒绝缺 fresh auth，final revalidation 与 Provider handoff 有竞态 | 撤权调用方可能观察 circuit 状态；open circuit 后仍可能外发 |
| RFX-05 | P1 | physical Provider ledger 在实际外发前计数 | circuit/DB 拒绝的零外发请求也被记作物理调用并消耗预算 |
| RFX-06 | P1 | complete-path fresh-plan 与 release retry 未按冻结合同实现 | context 输入变化没有交回 outer coordinator；complete-path release transient 没有有界重验 |
| RFX-07 | P1 | 资格记录覆盖不足且部分测试固定了错误行为 | 现有绿色门无法证明 Phase 2 完成定义成立 |

### 1.1 RFX-01：阶段 deadline 被错误地全局化

当前 `SemanticDeadlineWindows::expired_window()` 从 `ProviderStart`、`Work`、`SnapshotClose`、`Absolute`
中返回最早过期项；`SemanticExecutionContext::admit_stage()` 不接收目标阶段，直接使用这个全局检查。

完整路径本来允许：

```text
Work deadline 到达
  -> traversal 停止并形成 WallTimeExhausted partial
  -> 在 SnapshotClose window 内 commit RR
  -> 在 Absolute window 内 packing / release / sign
```

当前 postflight 调用 `admit_stage()` 和 `run_stage(Absolute, ...)` 时，已经过期的 Work window 会阻断后续
tail，合法部分结果最终变成 hard timeout。相关位置：

- `crates/buzz-relay/src/semantic_query_runtime.rs`：`expired_window()`、`admit_stage()`；
- `crates/buzz-relay/src/semantic_graph_traversal.rs`：`WallTimeExhausted` 与 snapshot close；
- `crates/buzz-relay/src/api/bridge.rs`：complete-path postflight/release。

此外，one-shot 在 R4 已启用 retry 后仍通过 `for_one_shot_hard_deadline()` 把 `ProviderStart`、`Work`、
`SnapshotClose`、`Absolute` 全设为同一个 45 秒边界。原计划只允许 R2 零策略迁移暂时全等；R4 前必须由
one-shot coordinator 明确留下 Provider 后的 RR、release、finalize 收尾 reserve。当前实现没有满足这一点。

### 1.2 RFX-02：lifecycle 与 cancellation 只完成了部分接线

当前 `timeout()` 先执行 `cancel()`，原子状态实际写入 `Cancelling`，再只把返回值重标为 `TimedOut`；
`LIFECYCLE_TIMED_OUT` 没有真实写入路径。

同时：

- `forbids_new_semantic_work()` 把 `Finalizing` 当作允许新工作；
- `admit_stage()` 不检查 lifecycle latch；
- `CallerDisconnected` 只出现在测试中，没有生产接线；
- server shutdown 主要在阶段入口轮询布尔值，不能唤醒正在 Provider wait、Provider encode 或 DB await 的
  请求；
- soak 测试直接调用 `context.cancel()`，没有验证真实 request/shutdown signal。

### 1.3 RFX-03：one-shot release permit 没有线性消费到签名

Coordinate 和 one-hop 当前顺序是：

```text
confirm release -> permit / Finalizing
  -> 构造并验证 result
  -> build Event
  -> sign
  -> finalize_completed(permit)，丢弃 permit
```

冻结合同要求：

```text
构造并验证 unsigned result
  -> confirm release
  -> permit / Finalizing
  -> permit 作为线性能力 move 进同步 signer
  -> post-check
  -> emit
```

当前控制流虽然没有中间 `await`，但 permit 与签名没有类型级绑定；结果验证失败也可能发生在 release permit
已经发放之后。完整路径已经把 permit 传入 signing helper，one-shot 必须收敛到同一安全形状，但仍保留各自
的 release request 和结果类型。

### 1.4 RFX-04：circuit 与 Provider handoff 之间仍有两个 TOCTOU

第一处是可观察错误优先级：

- fast circuit gate 拒绝时直接返回 `AdmissionBusy`；
- reservation wait 后 token stale/open 时同样直接返回 `AdmissionBusy`；
- 两条分支都没有执行计划要求的 fresh authorization/gate recheck。

如果调用方在等待期间被撤权，同时 circuit 打开，调用方可能看到 Provider Busy，而不是授权拒绝，从而
观察到本不应暴露的 Provider 健康状态。

第二处是最终外发线性化：共享 executor 完成最后一次 circuit token revalidation 后返回 coordinator；
coordinator 随后才构造输入、记录指标并调用 `encode_once`。其他请求可以在这段间隙推进 circuit epoch，
当前请求仍会向 Provider 外发。

### 1.5 RFX-05：attempt ledger 的名字与计数事实不一致

`begin_provider_attempt()` 当前发生在 circuit fast gate、DB reservation、wait、final egress confirmation 和
真实 Provider handoff 之前。因此以下情况都会增加所谓 physical attempt：

- circuit 已 open；
- reservation Busy/Unavailable；
- context/currentness 变化；
- final egress confirmation 拒绝；
- deadline/cancel 在 Provider 前发生。

这会让指标与 retry cap 表达“admission attempt”，而不是计划冻结的“真实物理 Provider 调用”。

### 1.6 RFX-06：两条 retry 边界没有闭合

完整路径 Provider retry 会重新观察 context 并重建 Q0/Qi，但没有对旧、新 ordered input bundle identity
执行比较。输入变化时仍在同一个 root attempt 内继续 retry，没有返回
`ReturnToOperationForInputRebuild` 并消费 operation restart ledger。

此外，release confirmation 的 bounded typed retry 只接入 one-shot；complete-path postflight 对 release DB
错误只执行一次并直接映射为 503。两类 operation 可以共享 confirmation retry 原语，但必须继续使用不同的
`expected_snapshot` 参数和公开错误映射。

### 1.7 RFX-07：当前资格门不足以关闭上述问题

当前 deterministic reliability gate 已通过，但缺少以下关键行为门：

- `work expired -> snapshot close -> partial release/sign`；
- timeout 后实际 latch state 为 `TimedOut`；
- `Finalizing` 后 generic stage admission 被拒绝；
- revoked authorization 与 circuit open/stale 的组合优先级；
- final circuit revalidation 与真实 Provider handoff 的竞态；
- 零 Provider 外发时 physical attempt delta 为零；
- permit 必须由 signer 按值消费；
- 真实 caller disconnect/server shutdown 中断所有 await；
- complete-path input changed 与 release transient retry。

资格记录还明确列出 disposable pgvector、migration、完整 integration、真实 Provider 与真实 fleet 未运行；
`just ci` 和计划列出的最终 `just semantic-test` 也没有完成记录。

R4 还把多个 retry 行作为同一个最终编译 profile 启用，没有计划要求的逐项 canary/cutover 证据。修复不新增
caller-selectable flag；应通过acceptance-only closed profile或逐行故障注入，分别证明connect、429、5xx、
snapshot recovery、vector reuse和release retry，再对最终组合profile做一次整体资格。

## 2. 修复设计

### 2.1 阶段感知的 deadline admission

删除“任意较早窗口过期就拒绝所有后续阶段”的执行语义。`expired_window()` 可以保留用于诊断，但不能再
作为 generic admission gate。

共同 context 提供目标阶段明确的接口，例如：

```text
admit_stage(target_window, expiry_disposition)
run_stage(target_window, expiry_disposition, future)
```

`expiry_disposition` 只允许 closed 内部值：

- `Terminal`：该窗口过期使整个请求终止；
- `Cutoff`：停止当前工作，但保留 operation 已经预留的后续 cleanup/response tail。

Operation coordinator 继续拥有含义：

- one-shot 对外公开的 45 秒 hard deadline保持不变，但内部必须使用明确且digest-bound的
  `provider_start_before < work < snapshot_close < absolute`；准确 reserve 数值在F0作为closed常量冻结，
  由one-shot coordinator提供，不能由共同runtime按剩余时间猜测；
- complete-path Provider/start、root 计算不能越过 Work；
- traversal Work cutoff 可以形成合法 partial，不把整个 context 标成 terminal；
- RR commit/rollback 使用 `SnapshotClose`；
- packing、release、同步签名和 response post-check 使用 `Absolute`；
- cleanup 不走 semantic stage admission，任何状态下都必须完成。

`ProviderStart` 只控制能否开始新的物理 Provider attempt，不能阻断已经合法进入的后续非 Provider tail。

### 2.2 修正 lifecycle 状态机并接入真实 cancellation

`SemanticLifecycleLatch` 必须直接执行以下 CAS：

```text
Active -> Finalizing
Active -> Cancelling
Active -> TimedOut
```

不得通过“写入 Cancelling、返回 TimedOut”模拟 deadline。`admit_stage()` 只允许 `Active`；`Finalizing`、
`Cancelling`、`TimedOut`、`Completed` 均拒绝任何新的 Provider、semantic DB、traversal、release 或 signing
stage。已经取得 finalizer guard 的同步签名不再调用 generic `admit_stage()`。

生产 cancellation 接线要求：

1. HTTP request adapter 持有 request-drop/caller-disconnect guard；
2. Relay shutdown 使用可订阅、可唤醒的 signal，而不是只轮询 `AtomicBool`；
3. Provider reservation wait、backoff、Provider encode、DB await、traversal 和 release confirmation 都与同一
   context cancellation future 竞争；
4. 取消后不 spawn 或 detach 补偿工作；transaction 和 permit 依靠显式 close 或 RAII 归零；
5. cancel 在 `Finalizing` 期间到达时只设置 discard，不能启动第二条 terminal/finalize 路径。

### 2.3 统一 release-finalize 的线性 helper

为两类 operation 使用同一内部安全形状：

```text
ValidatedUnsignedResult<T>
  -> operation-specific confirm_release(...)
  -> SemanticGraphQueryReleasePermit
  -> begin_finalize(permit) -> FinalizerGuard<T>
  -> sign_released(guard, relay_key)
  -> deadline/cancel post-check
  -> SignedResult<T> | discard
```

要求：

- unsigned result、request binding、response cap 和 canonical validation 全部在 release 前完成；
- release permit 不实现 `Clone`，不能存入共享状态或跨 task 传递；
- signer 必须按值接收 permit/guard，成功或失败都消耗它；
- cancel/timeout 先赢时只 drop 未消费 permit，绝不调用 signer；
- `Finalizing` 先赢时只允许一次同步 signer；
- signing 期间 cancel/timeout 只使 post-check 丢弃 signed Event；
- one-shot 保持 exact-snapshot release，complete-path 保持 current-authorization release；
- Event kind、builder、size cap 和公开错误仍由 closed surface 所有。

### 2.4 把 circuit fence 与 Provider handoff 合并为一个线性点

共享 Provider executor 不再返回一个可被 coordinator 长时间持有的 `ProviderEgressAdmission` 后再由外层
调用 Provider。它接收两个由closed coordinator构造的trait-free callback：

- `reauthorize_without_reservation`：只在caller将观察circuit拒绝时，通过fresh DB writer-fence重验当前
  principal、gate、generation/context expectation与routing trust；不创建reservation、不编码、不执行查询；
- `lazy_encode`：只在最终handoff线性化成功后才构造并poll一次Provider future。

共同executor在内部完成：

```text
fresh plan / authorization
  -> reserve non-counting physical-attempt budget token
  -> circuit fast gate
     -> refused: fresh auth/gate recheck -> caller-visible Busy
  -> DB reservation / wait
  -> token revalidation
     -> stale: fresh auth/gate recheck -> caller-visible Busy
  -> final egress confirmation
  -> circuit authorize_handoff(circuit token + budget token)
  -> invoke lazy Provider closure exactly once
  -> observe outcome against the same handoff token
```

`authorize_handoff` 必须定义清楚的线性化语义：在同一个同步临界区内验证 epoch/half-open lease 并产生一次性
`ProviderHandoffPermit`，同时接收已经预留但尚未计数的budget token。permit 产生后，该 attempt 已经被
circuit接纳；之后其他请求打开circuit不追溯撤销这次已接纳调用。permit 产生前token失效则budget token
被drop且Provider delta为零。budget已耗尽时不得进入circuit handoff；此前取得的half-open probe lease必须由
circuit token的Drop归还，不能留下占用。

fast-open 和 wait-stale 分支需要一个无 Provider 副作用的 fresh DB authorization/gate check。该检查复用
host-derived Community、caller、ticket 与 routing trust，不能依赖客户端字段，也不能创建 reservation。
它通过上述closed callback留在operation授权边界内，共同runtime只编排结果优先级：fresh
authorization/gate拒绝或无法证明当前授权时，返回现有授权/不可用错误；只有fresh检查明确允许后才返回
caller-visible circuit Busy。

### 2.5 让 physical-attempt ledger 在真实 handoff 时计数

Provider budget 分成两个概念：

- admission/operation restart 继续由现有 operation ledger 有界；
- physical Provider attempt 只在 `ProviderHandoffPermit` 被消费、lazy Provider closure 即将调用时增加。

在fast circuit gate前预留一个不可复制的 `ProviderAttemptBudgetToken` 防止并发越界；预留本身不增加
physical counter。只有budget token与circuit token共同被handoff permit消费、lazy Provider closure即将调用
时才增加physical counter。circuit、DB、deadline、cancel或budget exhaustion在handoff前拒绝时，相关token
drop且physical delta为零；half-open probe lease也必须随circuit token归还。

现有总上限保持：one-shot 最多 2 次、complete-path 最多 3 次；本修复不提高上限。

### 2.6 收口 complete-path retry

每个 root attempt 保存 ordered input bundle 的 exact identity：channel kind、contract digest、input digest 和
顺序。fresh plan 后：

- identity 完全相同：可按现有 Provider retry ledger 发起下一物理 attempt；
- context 或 Q0/Qi identity 变化：返回 typed `ReturnToOperationForInputRebuild`，由 outer coordinator 消费
  operation restart 并重建 root attempt；
- authorization、generation/model incompatibility、deadline/cancel：按既有终止语义失败；
- 旧 ticket、reservation、circuit token、egress permit 不得复用。

Provider retry 与 operation restart 继续共享同一个 request context 和总 physical-attempt cap，不能形成嵌套
预算。

release confirmation 抽出共享的 bounded helper，但只重试 closed、明确“没有产生 permit/没有未知副作用”
的 DB transient。以下永不 retry：

- `Denied`、`SnapshotChanged`、`FleetUnavailable`；
- commit/outcome unknown；
- 已取得 permit；
- deadline、cancel；
- result/signing failure。

one-shot 与 complete-path 均最多执行 2 次 release confirmation；complete-path 仍传
`expected_snapshot=None`，不能因复用 helper 被收紧。

## 3. 分阶段交付

### F0：修正状态与建立失败回归

1. 将 Phase 2 资格记录和 TODO 状态改为“主体已实现、correctness 修复中”；
2. 为 RFX-01 至 RFX-06 增加会在当前代码上失败的最小行为测试；
3. 为 RFX-07 增加可执行的gate inventory/status assertion，准确列出已运行、未运行和条件式资格；
4. 保留当前 runtime digest、manifest 和 binary 作为诊断/rollback 基线；
5. 不在 F0 修改公开合同或生产行为。

退出门：RFX-01至RFX-06各有单一、可重复、内容无关的失败行为证据；RFX-07由机械gate清单证明，
不以源码检索或人工声称代替。

### F1：Deadline 与 lifecycle

1. 改为 target-window admission；
2. 区分 terminal deadline 与合法 cutoff；
3. 冻结one-shot的内部provider/work/close/finalize reserve，同时保持公开45秒合同；
4. 让 `TimedOut` 成为真实 latch 状态；
5. 禁止 `Finalizing` 启动 generic stage；
6. 接通真实 request/shutdown cancellation；
7. 验证完整路径 partial tail。

退出门：`WallTimeExhausted` partial 可以在 Work 结束后、Absolute 结束前完成 commit/release/sign；Absolute
结束后不得签名或返回成功。

### F2：Release-finalize 线性化

1. one-shot 在 release 前构造并验证 unsigned result；
2. 引入按值消费 permit 的 signer/finalizer guard；
3. complete-path 迁入同一安全 helper 形状；
4. 保留两类 snapshot/release policy；
5. 覆盖 cancel/finalize/signing failure 竞态。

退出门：所有成功 Event 都能从类型和测试证明由单一 release permit 同步授权；permit 不可复制、丢失或在
签名后才补交。

### F3：Circuit、handoff 与 physical ledger

1. 增加 circuit outcome 前的 fresh auth/gate check；
2. 将 final circuit fence 与 lazy Provider closure 合并；
3. 引入一次性 handoff/budget token；
4. 让 physical counter 只在真实调用前增加；
5. 保持 process-local circuit 和现有公开 Busy 映射。

退出门：revoked caller 永远先得到授权拒绝；token 在 handoff 前 stale/open 时 Provider delta 为零；handoff
线性化后恰好一次调用并恰好一次closed outcome handling。只有上位合同允许的Provider结果进入一次
health/throttle observation；deadline与cancel继续不计入circuit健康失败。

### F4：Retry 与恢复边界

1. complete-path fresh input identity 比较；
2. input changed 回到 outer coordinator；
3. complete-path 接入 bounded release-confirmation retry；
4. 保证 Provider、operation、release 三类 ledger 不嵌套放大；
5. 保留所有 frozen public error mapping。

退出门：one-shot 不超过 2 次、complete-path 不超过 3 次 physical Provider call；每个 traversal hop仍为零
Provider call；release outcome unknown 永不重放。

### F5：资格与文档收口

1. 运行 deterministic、disposable DB、migration、unit 和 CI 门；
2. 使用受控真实 Provider canary 验证 attempt/circuit/cancel，不保存 query/vector/body；
3. 对每个retry/recovery行分别完成closed故障门，再验证最终组合profile；
4. 更新 runtime digest、manifest、qualification 和 TODO；
5. 记录每个真实 fleet old/new digest transition 与 binary rollback；
6. 分开记录“correctness implementation closed”和“deployment qualification closed”；
7. 未具备真实 Provider/fleet 环境时，只能关闭前者，状态必须保持“实现完成、部署资格未完成”。

退出门：correctness门全部通过后可声明“Phase 2 correctness implementation已交付”；真实Provider/fleet门
也通过后，才可恢复无修饰的“Phase 2 已完整交付/完成部署资格”表述。

## 4. 必须新增的测试

### 4.1 Deadline 与 partial

- fake clock 推进到 Work 之后、SnapshotClose 之前：RR commit 成功；
- 推进到 SnapshotClose 之后：commit/rollback 按 frozen error 失败，不能跨 snapshot；
- `WallTimeExhausted` partial 在 Absolute 之前完成 release/sign；
- Absolute 到期先赢时零签名、零 response；
- one-shot不产生partial，且任何新Provider attempt不得在`provider_start_before`之后开始；
- one-shot在Work之后仍能使用closed reserve完成短RR、release和同步finalize，但绝不越过公开45秒Absolute。

### 4.2 Lifecycle 与 cancellation

- `timeout()` 后 `state()==TimedOut`；
- `Finalizing` 后所有 generic stage admission 失败；
- finalizer guard 自身仍可完成一次同步签名；
- caller disconnect、server shutdown、deadline 分别中断 Provider wait、backoff、encode、DB await、traversal、
  release；
- 每个取消点 transaction、semaphore、probe lease 和 permit 归零，无 detached work。

### 4.3 Release 与签名

- unsigned validation 失败时 release call count 为零；
- release denial/snapshot change/fleet unavailable 时 sign count 为零；
- permit 只能 move 一次，不能 Clone 或二次消费；
- Finalizing 期间 cancel 导致 signed Event 被丢弃；
- one-shot exact release、complete-path permissive release 保持原样；
- complete-path release no-effect transient 最多重验一次，outcome unknown 不重试。

### 4.4 Circuit 与 attempt

- fast-open + authorization revoked：返回授权错误，reservation/provider delta 为零；
- wait-token stale + authorization revoked：同上，且已消费 reservation 不 refund；
- final confirmation 阻塞期间 circuit epoch 变化：handoff 前拒绝，Provider delta 为零；
- physical budget已耗尽时Provider delta为零、half-open probe lease归还；
- handoff 已线性化后 circuit 打开：该 attempt 恰好一次，不产生第二次外发；
- open/Busy/DB reject/deadline-before-handoff 均不增加 physical count；
- 500/429/connect/成功分别只产生一次匹配的 circuit observation；
- half-open 仍只有一个 probe。

### 4.5 Retry 与差分

- complete-path fresh Q0/Qi 相同：允许同 root attempt Provider retry；
- fresh Q0/Qi 变化：返回 outer rebuild，旧 bundle 不外发；
- Provider retry + operation restart 组合不超过 3 次；
- 同一 fixed vector/snapshot 下，修复前后四个 operation 的 normalized result、Event、公开错误保持兼容；
- Coordinate filters、one-hop coverage、full-path partial/omission 不变。

## 5. 质量门与资格边界

最低门：

```bash
just semantic-retrieval-compatibility-baseline
just semantic-retrieval-computation
just semantic-retrieval-reliability
just semantic-test
just test-unit
just ci
```

并单独记录：

```bash
just semantic-pgvector-test
just semantic-migration-test
cargo test -p buzz-relay --lib -- --ignored real_provider
```

要求：

- 普通 unit 绿色不能替代 disposable pgvector/migration；
- ignored Provider test 没有实际运行时不能写“真实 Provider 通过”；
- 没有真实 fleet 时不能写“fleet rollout/rollback 已演练”；
- 测试产物不得记录真实 query、embedding、API key、Authorization、Project 内容或身份；
- wall-clock 延迟不做永久 golden；使用 fake clock、attempt count、closed outcome 和资源归零作为机械门。

## 6. Rollout 与 rollback

本修复会改变 reliability runtime contract，必须更新 compiled runtime digest。发布遵循：

```text
feature/query gate off
  -> drain in-flight requests
  -> 部署同一修复 binary 到完整可路由 fleet
  -> 重新 attestation / advertisement
  -> deterministic + DB + Provider canary
  -> 短窗开启
```

禁止同一正常 fleet 混跑修复前后 digest。任一以下信号触发立即 gate-off 和 binary rollback：

- 合法 partial 仍变 hard timeout；
- auth 后置于 circuit outcome；
- Provider physical count 超 cap或零外发却增加；
- release permit 未消费、重复消费或迟到签名；
- snapshot/release policy变化；
- 公开错误、Event、排名或结果发生未批准差异；
- cancellation 后仍有 Provider/semantic DB/traversal 新工作或资源泄漏。

回滚使用当前 `1d8be4643` binary 只能作为临时恢复旧行为的手段，不能把本计划记录的已知错误重新标记为
合格。修复后的 legacy 删除窗口从最终目标 digest 完成真实切流时单独开始，不继承 Phase 1
`2026-09-16` 窗口。

## 7. 完成定义

### 7.1 Correctness implementation关闭

同时满足以下条件，才可以声明“Phase 2 correctness implementation已交付”：

1. stage admission 只检查目标 window，complete-path partial tail 可达，one-shot有明确且有界的内部收尾reserve；
2. `TimedOut`、`Cancelling`、`Finalizing` 与 `Completed` 状态和实际行为一致；
3. caller disconnect、shutdown、deadline 能中断全部 waitable semantic stage；
4. `Finalizing` 后只允许一次同步 signer和mandatory cleanup；
5. unsigned result 在 release 前完成验证；
6. 每个成功 Event 的 signer按值消费一个 release permit；
7. circuit outcome 前 fresh auth，handoff 具有单一线性化点；
8. physical attempt 只统计真实 Provider invocation；
9. complete-path input变化返回 outer rebuild，retry不发生乘法放大；
10. 两类 release policy保持，安全 transient retry有界，outcome unknown不重放；
11. 四个 operation 的 wire、结果、公开错误、ranking和snapshot合同通过差分；
12. deterministic、semantic DB、migration、unit与CI门通过；
13. qualification、实现计划、Stage TODO和current status改为一致事实；
14. 第三阶段 queue、fairness、capacity与跨Pod治理仍保持未交付。

### 7.2 Deployment qualification关闭

在7.1之外，还必须满足：

1. 受控真实Provider canary通过，且不保存query/vector/body；
2. 目标fleet完成同质old/new digest切流、gate/drain/re-attest与binary rollback演练；
3. rollback窗口owner、起止日期和保留/删除结论已经记录；
4. 未出现attempt放大、late sign、授权/circuit优先级、snapshot或公开结果差异。

只有7.1与7.2都关闭，才能使用无修饰的“Phase 2 已完整交付”或“具备部署资格”。如果环境不足以执行
7.2，必须停在“correctness implementation已交付、deployment qualification未完成”，不能写
production-ready。
