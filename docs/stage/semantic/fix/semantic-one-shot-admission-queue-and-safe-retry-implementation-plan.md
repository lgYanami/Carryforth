# Project Context 语义 One-shot 有界排队与安全重试实施方案

> 状态：设计完成，未执行；本文仅记录候选方案，尚未产生代码、配置、migration 或运行时行为变更
>
> 日期：2026-08-15
>
> 适用范围：Project Context `coordinate-search` 与两种 one-hop semantic search；两者共享
> `SemanticOneShotExecution`、进程并发许可和 Community / Provider 分布式 rate gate
>
> 当前产品边界：Carryforth 当前只资格化本地源码构建与单 Relay；本文保持数据库 gate 的多实例安全性，
> 但不据此宣称多 Pod 公平性、负载均衡或生产资格
>
> 数据边界：本方案不新增 SQL migration，不改写 embedding、active generation、Project Context、
> Project View、Document、Event 或成员数据，不改变成功结果 Event 的 kind、schema、签名和 request binding

## 0. 已确认结论

本方案冻结以下产品与工程决定：

1. 当前现场的 `busy` 不是图路径、召回或 canonical 数据错误，也不是上游 Provider 429；
2. 两条 one-hop 请求约相差 17 ms 发起，先到请求预留了同一 `interactive_query` lane，后到请求在
   Provider 输入构建前被本地分布式 admission gate 拒绝；
3. 当前“所有语义命令都是 non-retried one-shot”虽然保护了费用、NIP-98 exact-body 和出域预算，
   但与多个 Managed Agent / Role 并行检索的真实调用方式不匹配；
4. 正常短时竞争必须首先由 Relay 有界排队吸收，不能要求每个 Agent 自行猜测 sleep；
5. 不采用无限队列，不把持续过载伪装成成功，也不通过降低 Provider 间隔规避真实速率合同；
6. CLI 只允许对 Relay 明确证明为 **Provider 出域前失败**的 admission busy 自动重试一次；
7. Provider 429、Provider transport、响应体中断、timeout、503、conflict 和 verification failure 均不自动重试；
8. 自动重试必须生成新的 UUIDv4 `request_id`、新的 NIP-98 Event 和新的 exact authenticated body；
9. 一次逻辑语义命令即使发生安全重试，仍最多产生一次真实 Provider 请求；
10. Skill 不再自行循环重试；CLI 拥有唯一的自动重试策略，Agent 不能通过同义改写绕过预算；
11. Queue、Provider gate、deadline、currentness、授权和结果签名继续失败关闭；
12. Full-path `semantic-query` 不在本阶段改造范围内，也不能成为 coordinate / one-hop 失败后的 fallback。

一句话目标：

> 将短时并发从“立即 429”改为“Relay 内有界等待并重新准入”；只有等待仍未成功且可以证明尚未出域时，
> CLI 才以新身份绑定重试一次，从而在不重复 Provider 调用的前提下消除正常并发造成的随机失败。

## 1. 问题复盘

### 1.1 现场证据

现场同一秒内存在两条相关 `/query`：

```text
request A starts
        │
        ├── 约 17 ms
        │
request B starts

request B -> HTTP 429 busy
request A -> HTTP 200, one signed result
```

Relay 进程指标同时显示：

```text
one-hop success                = 7
one-hop busy                   = 1
one-hop Provider input count   = 7
```

Provider input histogram在 `SemanticOneShotExecution::prepare` 成功后、Provider transport 前才递增。
`success + busy = 8`，但 Provider input count 仍为 7，因此失败请求没有执行 Provider 调用。

现场没有配置 `BUZZ_SEMANTIC_GRAPH_QUERY_MAX_IN_FLIGHT`，生效值是默认 8；故两条相关请求也不足以耗尽
process semaphore。剩余的实际失败点是 `reserve_semantic_graph_query_egress` 返回的 Provider admission
`Busy`。

### 1.2 当前失败链

当前 one-shot preparation 顺序是：

```text
try_acquire process permit
        │
        ├── no permit ───────────────────────────────> Busy
        │
        ▼
read authorized semantic ticket
        │
        ▼
reserve distributed Provider slot
        │
        ├── same-lane / physical-gate conflict ─────> Busy
        │
        ▼
sleep reserved wait
        │
        ▼
final egress confirmation
        │
        ▼
Provider call exactly once
```

数据库 gate 默认 Provider 请求间隔为 1,000 ms，并为 workload lane 维护两倍间隔的下一次 admission。
第一条 interactive query 已经预留槽位时，紧随其后的同 lane 请求满足：

```text
physical gate is not idle
AND lane_next > physical_next
```

当前实现立即返回 `SemanticProviderReservation::Busy`。这是一种正确的无等待过载保护，却不适合合法的
两 Role 并行检索。

### 1.3 为什么只加 CLI 通用重试不正确

现有 public `busy` 可能来自：

- process admission；
- distributed Provider admission；
- HTTP principal quota / admission；
- 上游 Provider 429。

前三者发生在 Provider 出域前；最后一种已经执行了一次 Provider transport。若直接把 one-hop 命令接入
CLI 现有 `with_retry_body`：

- 它还会自动重试 502 / 503 / 504 和 network / body-transfer error；
- 它无法证明第一次请求是否已经进入 Provider；
- 它可能复用不允许 replay 的 NIP-98 exact body；
- 多 Agent 会同时退避、同时醒来，形成新的 thundering herd；
- 一次逻辑 hop 的 Provider 成本会从 1 变为不受语义预算明确约束的多次。

因此通用 HTTP retry 不能用于本方案。排队必须在共享容量所有者 Relay；自动重试必须由独立的、
Provider-before/after 可证明策略控制。

## 2. 目标与非目标

### 2.1 目标

- 两条相差毫秒级的正常 one-shot 请求在默认配置下都能完成，不再随机暴露 `busy`；
- 最多 8 条默认并发 one-shot 请求可在 Provider 健康且无持续外部过载时有界收敛；
- Relay 等待不持有数据库连接或事务；
- process、queue、distributed gate 和 Provider 调用数均有硬上限；
- Queue 等待计入现有 45 秒 one-shot wall-time；
- 等待后的请求必须重新验证 ticket、generation、Context currentness、routing trust 和授权；
- 自动重试最多一次，并且第一次尝试必须可证明没有 Provider egress；
- retry 后的成功结果严格绑定新的 request、NIP-98 Event 和 exact body；
- 旧 CLI + 新 Relay、新 CLI + 旧 Relay 均保持安全兼容；
- 通过无敏感内容的指标区分 queue、process、distributed gate 和 Provider rate limit；
- queue full、deadline 不足或持续过载时仍返回明确、可观测的 bounded failure。

### 2.2 非目标

- 不提供无限等待或“所有查询最终成功”的承诺；
- 不提高或绕过 Provider 配额；
- 不降低 `BUZZ_SEMANTIC_REQUEST_INTERVAL_MS`；
- 不把 `BUZZ_SEMANTIC_GRAPH_QUERY_MAX_IN_FLIGHT` 调大作为 rate-gate 修复；
- 不在数据库中建立持久 HTTP request queue；
- 不保证多 Pod 全局 FIFO 或无饥饿；
- 不自动重试 Provider 429、timeout、transport、503 或 malformed response；
- 不自动 fallback 到 full-path `semantic-query` 或另一种 one-hop operation；
- 不改变 semantic query text、embedding model、ranking、limit、scope 或 canonical hydration；
- 不改变 Agent Context Search 默认深度、分支、正文读取和逻辑 semantic-command 预算；
- 不修改 Desktop Project View 或 Project Context UI；
- 不宣称本方案完成生产、多 Relay 或容量 SLO 资格。

## 3. 必须保持的不变量

### 3.1 Provider 与预算不变量

1. 每个 HTTP attempt 最多一次 Provider egress；
2. 每个自动重试的逻辑 semantic command 最多一次 Provider egress；
3. 只有明确的 pre-egress admission failure 可以触发自动 retry；
4. Provider input 必须继续来自原 closed query encoder，不得本地改写或拆分；
5. retry 不能改变 query、scope、limit、Project 或 authenticated caller；
6. retry transport attempt 不增加 Skill 的逻辑 hop 数，但必须进入独立 attempt / retry 指标；
7. Agent 不得在 CLI 失败后再执行自己的隐藏 retry loop；
8. 不能通过同义 query 生成新的逻辑命令绕过预算。

### 3.2 身份、授权与 currentness 不变量

以下检查在排队或 retry 后都必须重新执行：

- Host-derived Community 绑定；
- caller NIP-98 与 exact body；
- HTTP admission、replay 和成员资格；
- Project Context read decision；
- deployment master 和 Community query gate；
- active semantic generation、model contract 和 source heads；
- Project Context revision / source epoch；
- routing trust / Fleet policy；
- Provider 出域前 final confirmation；
- snapshot commit 后 release confirmation；
- Relay signer、request binding 和 signed result verification。

第一次 attempt 的 NIP-98 Event 已经进入 replay 检查，因此 retry 严禁复用原 Event 或原 exact body。

### 3.3 数据库与取消不变量

- `Busy` / deferred observation 不写 gate 行；
- sleep 前必须结束 reservation transaction；
- 不允许持有 SQL transaction、connection 或 Community lock 进入 sleep；
- 一旦 future Provider slot 已成功提交，取消请求可以浪费该 slot，但不能“释放”它；
- 已提交 reservation 不能回滚时间线，否则其他 Relay 可能已基于新的 `next_request_at` 排程；
- cancellation 必须释放本地 pending permit 和 process permit；
- cancellation 后不得启动后台 retry task；
- generation、Context 或授权在等待期间变化时，必须返回原有 conflict / restricted / unavailable，
  不能继续使用旧 ticket 出域。

### 3.4 隐私与日志不变量

日志和 metric label 不得包含：

- query text；
- Coordinate、Edge、Document 或 Project object identity；
- Provider body、embedding 或 API key；
- NIP-98 exact body、签名或 private key；
- 高基数 request ID、完整 caller identity或用户可控标题。

允许的低基数 label 仅包括 `surface`、`stage`、`outcome` 和固定 error class。

## 4. 目标执行模型

### 4.1 总体流程

```text
authenticated semantic request
        │
        ▼
try acquire bounded pending permit
        │
        ├── full ──> pre-egress busy + safe retry hint
        │
        ▼
wait for shared process permit (bounded)
        │
        ├── admission deadline ──> pre-egress busy + safe retry hint
        │
        ▼
read current authorized ticket
        │
        ▼
distributed reservation loop
        │
        ├── deferred ──> close transaction ─> sleep + jitter ─> recheck
        │
        ├── admission deadline ──> pre-egress busy + safe retry hint
        │
        └── reserved
                │
                ▼
          wait reserved start time
                │
                ▼
          final egress confirmation
                │
                ▼
          Provider exactly once
                │
                ▼
          snapshot rank / commit / release confirm / sign
```

### 4.2 两层本地容量

新增一个 one-shot pending semaphore；它与现有 process semaphore职责不同：

| 层 | 默认 | 含义 |
|---|---:|---|
| pending | 16 | executing + waiting one-shot HTTP requests 总上限 |
| process | 8 | 已进入 semantic preparation / execution 的共享工作上限 |

pending permit 在 one-shot handler 进入 preparation 时以 `try_acquire_owned` 获取：

- 获取失败立即返回 pre-egress busy；
- 获取成功后一直持有到 request 完成或取消；
- 它只限制 Coordinate / one-hop，不能被 full-path query 绕过后错误计入 one-shot queue；
- process permit 继续与 full-path semantic query 共享，避免 DB / memory 工作并发被拆成两个独立上限。

等待 process permit 时使用 Tokio semaphore 的有界 async acquire，不再使用当前的立即
`try_acquire_owned`。等待上限由 one-shot admission deadline 控制。

### 4.3 Admission deadline

新增配置：

```text
CARRYFORTH_SEMANTIC_ONE_SHOT_MAX_PENDING=16
CARRYFORTH_SEMANTIC_ONE_SHOT_MAX_ADMISSION_WAIT_MS=10000
```

校验范围：

```text
MAX_PENDING:            1..=64
MAX_ADMISSION_WAIT_MS:  100..=30000
```

`MAX_PENDING` 未设置时按
`min(64, max(16, 2 * BUZZ_SEMANTIC_GRAPH_QUERY_MAX_IN_FLIGHT))` 推导；标准默认 process=8 时结果为 16。
显式值可以小于 shared process max，从而有意限制 one-shot surface，但 status 和启动日志必须明确报告
两个独立上限，不能让 operator 将 pending capacity 误认为 process capacity。

每条请求计算：

```text
absolute_deadline = started_at + 45s
admission_deadline = min(
    started_at + configured_max_admission_wait,
    absolute_deadline - 5s execution reserve
)
```

5 秒是 Provider / DB / release 的最小剩余执行预算，不替代 Provider 自身 request timeout。若配置或时钟
使 execution reserve 无法满足，请求在出域前返回 busy，而不是占用一个必然超时的 Provider slot。

### 4.4 Distributed reservation 状态

将：

```rust
SemanticProviderReservation::Busy
```

扩展为包含无内容时间提示的状态，例如：

```rust
pub enum SemanticProviderReservation {
    Reserved { wait: Duration },
    Busy { retry_after: Duration },
}
```

`retry_after` 使用数据库 `clock_timestamp()` 和已锁定 gate 行计算，不能使用调用者时钟推断。

当前两个 Busy 条件分别给出：

- physical gate 未空闲且当前 lane 不能预留下一槽：最早重试时间是当前 `physical_next`；
- `scheduled_at` 已晚于 caller admission deadline：最早重试时间是 `scheduled_at`。

Busy transaction 继续 rollback 且零写入。Background worker 可以使用时间提示改善 defer 调度，但本阶段
不改变 worker 的 durable job budget 和 poison policy。

### 4.5 Relay reservation loop

`SemanticOneShotExecution::prepare` 在取得 process permit 和 authorized ticket 后执行：

1. 调用 `reserve_semantic_graph_query_egress`；
2. `Reserved`：退出 loop，按既有流程 sleep 到预留时间并 final confirm；
3. `Busy { retry_after }`：
   - 若 `now + retry_after` 超过 admission deadline，返回 admission busy；
   - 否则记录 deferral，结束 DB transaction 后 sleep；
   - 在不早于 `retry_after` 的基础上增加 `0..=100 ms` full jitter；
   - 重新调用 reservation，数据库在同一事务内重新检查 ticket / Context / gate；
4. 任一 DB、authorization、generation、routing 或 deadline 错误按原类别返回；
5. 一旦 `Reserved` 提交，不再进入 reservation retry loop。

跨 Relay 的严格 FIFO 不作为承诺。数据库 advisory / row lock 继续保证每次 reservation 原子；jitter 只
减少同时唤醒，不参与授权和速率正确性。

## 5. 安全自动重试合同

### 5.1 为什么需要 capability

现有 one-hop closed error 允许：

```json
{
  "code": "busy",
  "message": "One-hop semantic search is busy",
  "retryable": true,
  "retry_after_seconds": 1
}
```

旧合同中的 `retry_after_seconds` 可能来自 Provider 429，不能让新 CLI 在连接旧 Relay 时将它误解为
“第一次 attempt 未出域”。因此增加一个 additive NIP-11 capability：

```text
carryforth-semantic-one-shot-admission-retry-v1
```

该 capability 表示：

1. Relay 已实现本文的 bounded one-shot queue；
2. one-hop `busy + retry_after_seconds` 只用于 Provider-before admission failure；
3. Provider 429 仍可返回 `busy`，但不得携带自动 retry hint；
4. Coordinate-search HTTP 429 在该 capability 下也保证发生在 Provider egress 前；
5. 成功 response contract 没有变化。

Relay 只有在 one-shot runtime、base surface capability 和新策略都有效时才广告它。它不是新的 enable
开关，不可绕过各 surface deployment master 或 Community gate。

### 5.2 兼容矩阵

| Client | Relay | 行为 |
|---|---|---|
| 旧 | 旧 | 当前 no-wait / no-retry 行为 |
| 旧 | 新 | Relay queue 生效；Client 忽略新增 capability，不自动 retry |
| 新 | 旧 | capability 缺失，保持旧的 no-retry 行为 |
| 新 | 新 | Relay queue；仅 safe admission busy 自动 retry 一次 |

不增加 one-hop error JSON 字段，因此旧 strict parser 不会因 unknown field 失败。NIP-11 capability 是自动
重试语义的版本 fence。

### 5.3 Relay busy 分类

内部错误必须区分：

```text
AdmissionBusy {
    retry_after_seconds,
    provider_egress_started = false
}

ProviderRateLimited {
    provider_egress_started = true,
    upstream_retry_after_seconds
}
```

public 映射：

| 内部原因 | HTTP | body | CLI 自动 retry |
|---|---:|---|---:|
| pending full | 429 | busy + bounded retry hint | 是，最多一次 |
| process wait exhausted | 429 | busy + bounded retry hint | 是，最多一次 |
| distributed admission exhausted | 429 | busy + bounded retry hint | 是，最多一次 |
| pre-semantic HTTP quota | 429 | capability 下保留 bounded hint | 是，最多一次 |
| Provider 429 | 429 | busy，无 safe hint | 否 |
| timeout / unavailable | 504 / 503 | 现有 closed error | 否 |

Provider 的原始 header、body、配额名和内部错误不得透传。`retryable=true` 继续表示 Human 可以重新评估后
发起新请求；只有 capability + bounded hint 同时满足才表示 CLI 可以自动重试。

### 5.4 CLI retry state machine

不能把语义命令接入 `with_retry_body`。新增独立、只接受 canonical logical request spec 的 helper：

```text
attempt 1
  ├── success ---------------------------------> verify and return
  ├── safe admission busy + capability --------> wait hint + jitter
  │                                                │
  │                                                ▼
  │                                             attempt 2
  │                                                ├── success -> verify and return
  │                                                └── any error -> return
  └── any other error --------------------------> return
```

固定策略：

```text
maximum attempts:          2
maximum automatic retries: 1
logical command deadline:  60 seconds
jitter after retry hint:   0..=250 ms
```

若 hint + jitter 后不足以在 logical deadline 内开始第二次 attempt，则不 retry，直接返回最后一个 busy。
CLI 不截短为更早时间后偷偷立即重试。

每次 attempt：

- 从 immutable logical spec 新建 UUIDv4 `request_id`；
- 重新 validate / canonicalize request；
- 重新构建 exclusive filter；
- 重新序列化 exact body；
- 重新签署 NIP-98 Event；
- 执行现有 `*_once` transport；
- 只使用该 attempt 返回的 request / auth event / exact body 验签成功 Event。

第二次 attempt 不重复 Project identity read；Relay 会重新执行服务端 currentness 与授权检查。若 capability、
query gate、权限或 snapshot 在等待期间变化，第二次 attempt 按当前状态失败关闭。

### 5.5 Coordinate 与 one-hop 的差异

共同部分：

- 两者都进入 shared bounded queue；
- 两者都使用新 request ID / NIP-98 body retry；
- 两者都最多一次 automatic retry；
- 两者都保持一次逻辑命令最多一次 Provider egress。

现有 Coordinate-search 将 Provider errors 映射为 503，而其 429 只来自 pre-egress admission；在新 capability
存在时，新 CLI 可将 Coordinate 429 识别为 safe admission busy。One-hop 必须依赖 closed error 中的
`retry_after_seconds`，且 Provider 429 不再填充该 safe hint。

### 5.6 Skill 预算语义

更新 Managed Agent Skill：

- “一次 semantic command”仍计为一个逻辑调用；
- CLI 内部 admission attempt 由 CLI 计数，不由 Agent 再发命令；
- failed logical commands 仍计入 Skill 实际成本；
- Agent 不得因为 CLI 已 retry 就重新改写同义 query；
- CLI 最终返回 busy / timeout / transport / unavailable 后，Skill 停止该分支或按现有 frontier 回退，
  不能再次调用同一 semantic command；
- 不允许 fallback 到 full-path `semantic-query`。

将当前“所有 busy 都不得自动 retry”修改为：

> CLI 可以按 advertised admission-retry capability 对明确的 pre-egress busy 执行一次内建重试；Agent 本身
> 不得增加重试。其他错误继续 non-retried。

## 6. 配置设计

### 6.1 新配置

在 Relay Config 增加：

```text
CARRYFORTH_SEMANTIC_ONE_SHOT_MAX_PENDING=16
CARRYFORTH_SEMANTIC_ONE_SHOT_MAX_ADMISSION_WAIT_MS=10000
```

这两个值必须：

- 使用严格整数 parser；
- 拒绝 0、负数、空白、overflow 和范围外值；
- 未显式配置 pending 时使用 4.3 节定义的 shared-process-derived 默认值；
- 只在 Coordinate 或 one-hop surface 被启用时参与 runtime readiness；
- 输出到 `/_status` 时只报告数值，不报告 Provider identity 或 caller 数据；
- 记录在 `.env.example` 和 semantic operations 文档；
- 不由 `start.sh` 自动改写用户真实 `.env`。

### 6.2 不修改的配置

以下配置语义保持不变：

```text
BUZZ_SEMANTIC_GRAPH_QUERY_MAX_IN_FLIGHT
BUZZ_SEMANTIC_GRAPH_TRAVERSAL_MAX_IN_FLIGHT
BUZZ_SEMANTIC_REQUEST_INTERVAL_MS
BUZZ_SEMANTIC_REQUEST_TIMEOUT_SECS
CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE
CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE
```

尤其不能通过缩短 `BUZZ_SEMANTIC_REQUEST_INTERVAL_MS` 让并发测试“看起来通过”。Provider rate gate
间隔必须继续由部署方按真实 Provider 合同配置。

### 6.3 Readiness

Queue 初始化失败、配置非法或 semaphore capacity 不可构造时 Relay 启动失败。Queue 满是运行时容量状态，
不撤销 NIP-11 capability，也不让全局 readiness 变为 503；持续 queue saturation 通过 metric / alert 表达。

## 7. 类型与模块设计

### 7.1 Relay 类型

建议新增 closed 类型：

```rust
enum SemanticOneShotSurface {
    CoordinateSearch,
    OneHopSemanticSearch,
}

enum SemanticOneShotError {
    // existing variants...
    AdmissionBusy {
        retry_after_seconds: Option<u64>,
    },
}
```

Provider 429 不进入 `SemanticOneShotError`；Coordinate / one-hop adapter 分别保留 Provider error 分类，避免
把 pre-egress 与 post-egress busy 再次压平。

`SemanticOneShotExecution` 持有：

- pending permit；
- process permit；
- absolute deadline；
- 其余现有 ticket、reader、signer、routing trust 和 Provider。

permit 字段只负责 RAII，不暴露 public API。

### 7.2 AppState

在 `AppState::new` 中构造 one-shot pending semaphore。它是 AppState-owned，不是 module-level singleton，
因此 Community / Relay 重启不会复用旧 pending 状态，也不触及 Desktop 的 community cache reset 合同。

### 7.3 CLI 类型

CLI 必须保留结构化 admission busy，不可先压成普通 `Unavailable(String)` 再靠字符串判断。建议增加：

```rust
CliError::SemanticAdmissionBusy {
    surface: SemanticSurface,
    retry_after_seconds: Option<u64>,
    automatic_retry_safe: bool,
}
```

要求：

- `Display` 是固定 content-free 文案；
- exit code 继续为 2；
- JSON stderr 的 `retryable` 继续为 true；
- 最终失败时可输出 bounded `retry_after_seconds`；
- 不输出 query、scope、request ID 或 Provider 原因；
- 只有 dedicated semantic retry helper 消费 `automatic_retry_safe`。

## 8. Wire 与能力兼容

### 8.1 不变项

- HTTP 路径仍为 `POST /query`；
- NIP-98 exact payload 要求不变；
- Coordinate request / result schema 不变；
- one-hop request / result schema 不变；
- kind 40913 / 40914 不变；
- Relay result signer 和 request binding 不变；
- one-hop closed error JSON 字段集合不变；
- 不增加 redirect、fallback 或 background retry。

### 8.2 Additive capability

只新增 NIP-11 extension：

```text
carryforth-semantic-one-shot-admission-retry-v1
```

CLI identity observation增加一个布尔值，但不能用 capability 代替 Coordinate / one-hop 各自的 base
capability。执行某个 surface 必须同时满足：

```text
base surface capability == true
```

只有自动 retry 额外要求：

```text
admission retry capability == true
```

### 8.3 Capability 撤销

若 rolling change、配置或 runtime 使新语义不再可证明，Relay 必须先停止广告新 capability。正在等待的
旧请求仍按其进入时的 server deadline 执行，但 retry attempt 会重新读取当前能力 / 当前服务状态并失败关闭。

## 9. 公平性、过载与多实例边界

### 9.1 Background / interactive 公平性

现有 physical gate 与两倍 lane interval 保持不变。Interactive waiter 只在 gate 允许时真正预留，不提前
占用未来连续槽位；因此 background index 仍有机会取得中间槽位。

本方案不能将：

```text
same-lane busy -> reserve lane_next immediately
```

因为这会让一批 interactive requests 预占未来时间线，破坏现有 workload fairness。正确方式是观察
`retry_after`、结束事务、等待后重新竞争。

### 9.2 持续过载

以下情况仍允许出现 public busy：

- pending 已达 16；
- process permits 被长期操作占用；
- background / root / one-shot 总需求超过 Provider throughput；
- admission wait 达 10 秒；
- 剩余 absolute deadline 不足 5 秒；
- Provider 自身 429。

前五种可以携带 safe retry hint；Provider 429 不携带 automatic hint。一次 CLI retry 后仍 busy 时必须返回，
不能继续指数循环。

### 9.3 多 Relay

每个 Relay 有自己的 pending queue；数据库 gate 仍是 physical Provider capacity 的唯一分布式权威。
因此多实例下不会因为本地 queue 引入并发 Provider 越界，但本文不保证：

- 跨 Pod FIFO；
- 每个 Pod 等比例份额；
- 网络分区后的公平性；
- rolling mixed-version 自动 retry 行为一致。

additive NIP-11 fence 防止新 CLI 对旧 Relay 自动 retry，但完整多实例资格仍属于独立工作。

## 10. Observability

### 10.1 Relay metrics

新增低基数指标：

```text
carryforth_semantic_one_shot_pending
carryforth_semantic_one_shot_pending_limit
carryforth_semantic_one_shot_process_wait_seconds{surface}
carryforth_semantic_one_shot_admission_wait_seconds{surface}
carryforth_semantic_one_shot_admission_total{surface,outcome}
carryforth_semantic_one_shot_reservation_deferrals_total{surface}
carryforth_semantic_one_shot_provider_egress_total{surface,outcome}
```

`outcome` 仅允许固定集合，例如：

```text
admitted / queue_full / process_deadline / gate_deadline / cancelled
success / rate_limited / transport / rejected / invalid_response / timeout
```

现有 surface-specific request / duration / Provider-input metrics继续保留。新 provider egress counter 用于直接
证明 automatic retry 没有产生第二次出域，不能仅依赖 HTTP request count 推断。

### 10.2 内容安全日志

仅在 terminal busy、异常长 wait 和 Provider failure 打结构化日志：

```text
surface
stage
waited_ms
deferral_count
retry_hint_seconds
```

不记录 query、scope、candidate、Provider body 或高基数 request identity。

### 10.3 运行判断

默认验收期关注：

- `queue_full` 应为 0；
- 两 Role 并行时 `reservation_deferrals_total` 可以增加；
- deferral 后两个请求均 success；
- provider egress count 与逻辑成功 / Provider error 对齐；
- automatic retry 不造成同一逻辑命令两次 provider egress；
- p95 admission wait 低于配置上限；
- sustained saturation 必须可见，不能只在 Agent 文本中出现。

## 11. 文件改动矩阵

| 文件 | 计划改动 |
|---|---|
| `../../../../crates/buzz-db/src/semantic.rs` | Provider reservation Busy 增加 DB-clock retry hint；保持零写入与 gate 公平性 |
| `../../../../crates/buzz-db/src/semantic_query.rs` | query egress reservation传递 typed Busy hint；补 currentness / rollback tests |
| `../../../../crates/buzz-relay/src/config.rs` | 解析 pending / admission-wait 配置及范围、交叉校验 |
| `../../../../crates/buzz-relay/src/state.rs` | AppState-owned one-shot pending semaphore |
| `../../../../crates/buzz-relay/src/semantic_one_shot.rs` | pending/process bounded wait、distributed reservation loop、deadline、metrics |
| `../../../../crates/buzz-relay/src/semantic_coordinate_search.rs` | typed admission busy 与 Provider error 边界 |
| `../../../../crates/buzz-relay/src/semantic_one_hop_search.rs` | admission busy safe hint；Provider 429 无 automatic hint |
| `../../../../crates/buzz-relay/src/api/bridge.rs` | closed error映射、pre-semantic quota hint、surface metrics |
| `../../../../crates/buzz-relay/src/nip11.rs` | additive admission-retry-v1 capability |
| `../../../../crates/carryforth-cli/src/commands/project_view_snapshot.rs` | 观察新 capability，不替代 base capability |
| `../../../../crates/carryforth-cli/src/error.rs` | 保留结构化 semantic admission busy 与 bounded hint |
| `../../../../crates/carryforth-cli/src/client.rs` | `_once` 保持单次 transport；新增 dedicated safe retry helper / attempt deadline |
| `../../../../crates/carryforth-cli/src/commands/project_context.rs` | Coordinate logical spec 每次 attempt 生成新 request identity |
| `../../../../crates/carryforth-cli/src/commands/project_context_one_hop.rs` | one-hop logical spec 每次 attempt 生成新 request identity |
| `../../../../crates/carryforth-cli/TESTING.md` | queue / retry / error / attempt 合同与命令验收 |
| `../../../../desktop/src-tauri/src/managed_agents/search_project_context_skill.md` | 允许 CLI 内建一次 safe retry；继续禁止 Agent retry / fallback |
| `../../agent-context-search/skill-prompt/skill-prompt.md` | 同步 reviewed source prompt |
| `../../agent-context-search/search-project-context-skill-delivery-plan.md` | 更新 Skill 验收合同和预算口径 |
| `../../agent-context-search/project-context-coordinate-search-implementation-plan.md` | 更新 no-retry 与 429 合同历史说明 |
| `../../agent-context-search/project-context-progressive-observation-cli-implementation-plan.md` | 更新 one-hop no-retry、capability 和 qualification 合同 |
| `../../agent-context-search/project-context-coordinate-search-qualification.md` | 增加 burst / egress-count 证据 |
| `../../agent-context-search/project-context-progressive-observation-cli-qualification.md` | 增加 safe retry / mixed-version 证据 |
| `../../../../.env.example` | 记录两个新配置、默认值和边界 |
| `../../../semantic-pgvector-operations.md` | queue saturation、指标、调优与 rollback runbook |
| 本文档 | 逐 Phase 记录提交、review、验证结果和偏离 |

明确不修改：

- migrations / `schema/schema.sql`；
- semantic embedding / generation 表；
- ranking contract digest；
- Project Context canonical schema；
- Project View / Document / Meeting 数据；
- Desktop Project View UI；
- full-path semantic-query retry 合同。

## 12. 分阶段实施

每一阶段完成后必须先 review 代码和文档，确认没有偏离本方案，再独立提交并继续下一阶段。

### Phase Q0：冻结合同与原因观测

交付：

- 增加 typed surface / admission / provider failure 分类；
- 增加 provider egress、pending、wait 和 terminal cause metrics；
- 冻结“一次逻辑命令最多一次 Provider egress”测试 helper；
- 此阶段保持当前 no-wait / no-retry 外部行为。

退出门：

- 可以从 metric 明确区分 process、distributed admission 和 Provider 429；
- 日志与 metrics 不含 query / scope / identity；
- 当前全部 semantic tests 无行为回归；
- review 确认没有借观测改动放宽出域。

### Phase Q1：数据库 retry hint

交付：

- `SemanticProviderReservation::Busy { retry_after }`；
- DB-clock earliest retry 计算；
- background worker兼容新 enum；
- 单实例、双 DB handle、same-lane、cross-lane 和 deadline tests。

退出门：

- Busy 零写入；
- Reserved 仍只消费一个 physical slot；
- cross-lane fairness 不变；
- retry hint 永不早于 gate 允许时间；
- 无 migration / schema drift。

### Phase Q2：Relay bounded queue

交付：

- Config 与 AppState pending semaphore；
- process async wait；
- reservation deferred loop；
- admission / absolute deadline；
- Coordinate 与 one-hop 统一使用；
- additive NIP-11 capability先保持不广告。

退出门：

- 2 路和 8 路 burst 在 fake Provider 下成功；
- Provider start 间隔不小于配置 interval；
- queue full / wait exhausted 返回 bounded busy；
- cancellation 释放 permits，无 detached retry；
- wait 期间 DB active connection / transaction 数不随 sleep 保持；
- generation / permission / query gate 变化仍失败关闭。

### Phase Q3：Capability 与 CLI 安全重试

交付：

- 广告 `carryforth-semantic-one-shot-admission-retry-v1`；
- one-hop safe hint 与 Provider 429 hint语义分离；
- CLI structured admission busy；
- Coordinate / one-hop dedicated max-two-attempt helper；
- 每次 attempt 新 request ID / NIP-98 / exact body；
- 60 秒 logical deadline；
- mixed-version tests。

退出门：

- old/new compatibility matrix全部通过；
- capability 缺失时新 CLI 零自动 retry；
- Provider 429 / timeout / transport / 503 调用计数严格为 1；
- safe admission busy + success 的 Provider 调用计数严格为 1；
- retry result只接受第二 attempt 的完整 binding；
- generic `with_retry_body` 未被 semantic commands 调用。

### Phase Q4：Skill、文档与资格验收

交付：

- 更新真实 Managed Agent Skill 与 reviewed prompt；
- 更新 CLI testing、两份 implementation plan 和 qualification；
- 更新 `.env.example` 与 operations runbook；
- 执行本地真实双 Role 并发验收；
- 记录指标、日志和 Provider egress count 证据。

退出门：

- Skill 不再把 Relay 已吸收的短时竞争报告为检索失败；
- Agent 本身没有第二层 retry loop；
- 两个 Role 可取得不同但相关的上下文路径；
- 不调用 full-path fallback；
- 不增加 semantic logical-command / branch / body-read 预算；
- Live 测试无 canonical mutation；
- 本文档记录最终提交和所有未运行检查。

## 13. 测试矩阵

### 13.1 DB 单元 / 集成测试

- idle gate立即 Reserved，wait=0；
- same-lane 在 physical busy 时返回精确 retry hint；
- cross-lane 仍可取得公平的下一槽；
- scheduled time 超过 admission deadline 返回 Busy hint且零写入；
- 两个 DB handles并发时只有一个相同 physical slot 被提交；
- Busy rollback 后 gate 行与 `updated_at` 不变；
- interval 100 ms、1 s、60 s边界；
- database clock偏离 caller clock不造成提前 hint；
- background worker收到新 Busy variant 后正确 defer且不 poison。

### 13.2 Relay queue 测试

- Coordinate + Coordinate 同时发起，两者成功；
- one-hop + one-hop 同时发起，两者成功；
- Coordinate + one-hop 同时发起，两者成功且共享 physical gate；
- 默认 8 路 burst全部在 admission budget内取得 slot；
- 第 17 条 pending request立即 bounded busy；
- process permits 被占用时 one-shot等待，不立即失败；
- admission wait超过 10 秒时 Provider call count=0；
- 取消 pending waiter后 queue gauge / permit恢复；
- 取消已提交 reservation后 slot保持消费但无 Provider call；
- wait 时 query-disable、permission revoke、generation / Context变化分别失败关闭；
- final release failure不签名；
- Provider 429只调用一次且不返回 automatic hint；
- Provider timeout / body oversize / invalid vector不 retry。

### 13.3 CLI retry 测试

- capability + safe busy + success：两次 HTTP、一个 Provider egress、成功；
- capability缺失 + 同一 busy：一次 HTTP、直接失败；
- safe busy无 hint：不自动 retry；
- hint晚于 logical deadline：不自动 retry；
- 第二次仍 busy：总 attempts=2，返回 retryable exit 2；
- Provider busy：不自动 retry；
- 503 / 504 / network send / body drop：不自动 retry；
- conflict / restricted / invalid / verification：不自动 retry；
- 两次 attempt 的 request IDs不同且都是 UUIDv4；
- NIP-98 Event IDs、payload hash 和 exact bodies不同；
- query / scope / limit / project / caller保持相同；
- 第一次 binding的 success Event不能作为第二次结果通过；
- generic retry helper调用计数为零。

### 13.4 Skill 回归

- Agent执行一条 CLI command，内部 safe retry后成功，不再额外调用同一命令；
- 最终 busy 时停止当前 branch或使用已有 frontier，不同义改写；
- timeout / unavailable仍不 retry；
- 不 fallback `semantic-query`；
- 逻辑 one-hop总预算、活跃 branch、depth 和 canonical body reads不变；
- 默认输出仍是后续任务上下文，不自动发布检索日志。

### 13.5 Mixed-version

- old CLI + new Relay：queue生效，未知 capability被忽略；
- new CLI + old Relay：没有 capability，no retry；
- 新 Relay关闭 base one-hop capability时不能只广告 retry capability；
- capability观察暂时失败时不能假定 safe retry；
- strict one-hop error parser继续接受原字段集并拒绝未知字段；
- capability撤销后下一 logical command不 retry。

### 13.6 建议质量门

```bash
just test-unit
just test
just desktop-tauri-check
just desktop-tauri-test
just desktop-check
```

按阶段还应运行 scoped Cargo tests；最终由 `just ci` 给出完整本地 PR gate。任何因环境、Provider 凭据或
服务状态未运行的检查都必须在提交记录和本文档中明确列出，不能写成通过。

## 14. Live 验收

在不 reset、不删除 volume、不重建 canonical 数据的前提下：

1. 记录 one-hop success / busy、Provider input、Provider egress、queue wait、deferral基线；
2. 使用两个不同 Role / Agent 同时从各自当前 Work 执行 one-hop检索；
3. 保持 problem相关但上下文环境不同；
4. 验证两条 HTTP command均成功；
5. 验证至少一条出现 admission deferral，且 public `busy` 为 0；
6. 验证 Provider start间隔满足当前 1,000 ms配置；
7. 验证每条逻辑 command只有一个 Provider egress；
8. 用 canonical read回读采用的 Coordinate / Edge / Document；
9. 验证两个 Role 可以选择不同路径，不要求完全不相交；
10. 使用 mock / test Relay触发 queue exhaustion，验证 CLI只 retry一次；
11. 使用 mock Provider 429，验证 CLI不 retry；
12. 对比前后 Project Context、Project View、Document、Event和semantic generation计数，确认无业务数据改写。

真实 Provider 验收不能通过并行压测制造费用；容量边界使用 fake Provider / integration harness 证明。

## 15. Rollout 与 rollback

### 15.1 Rollout

顺序必须是：

```text
DB / shared enum兼容代码
        ↓
Relay queue（暂不广告 retry capability）
        ↓
CLI capability-aware retry
        ↓
Relay广告 retry capability
        ↓
Skill合同更新
```

在单仓库本地交付中可以按 Phase 提交，但 capability 只能在 Relay 与 CLI 行为、tests全部存在后广告。

### 15.2 Runtime rollback

若 queue 导致异常等待或资源压力：

1. 将 `CARRYFORTH_SEMANTIC_ONE_SHOT_MAX_PENDING` 收紧到经本地 burst 验证的有界值；
2. 将 max admission wait收紧到最小合法值；
3. 先停止广告 retry capability；
4. 必要时关闭 Coordinate / one-hop deployment master；
5. Community query gate和Foundation index不需要删除。

不能通过设置 Provider interval 为 0、无限 pending 或关闭 currentness confirmation 进行应急恢复。

### 15.3 Code rollback

代码回滚先撤 capability广告和 Skill retry说明，再回滚 CLI helper和 Relay queue。数据库无 migration，
因此不需要 down migration；现有 gate行继续兼容旧代码。

## 16. 完成定义

只有同时满足以下条件才能把本文状态改为“已实施”：

1. Q0–Q4均有独立提交和阶段 review结论；
2. 两条毫秒级并发 one-hop不再产生 public busy；
3. 默认 8 路 burst资格测试通过；
4. queue full / sustained overload仍有界失败；
5. automatic retry仅在 capability + safe hint下发生且最多一次；
6. 每次 retry使用新 request ID、NIP-98 Event和exact body；
7. 一次逻辑 command最多一次 Provider egress有可执行证据；
8. Provider 429、timeout、transport、503、conflict和verification均零自动 retry；
9. currentness、authorization、query-disable和result binding回归通过；
10. Skill不自行 retry、不 fallback、不突破预算；
11. old/new兼容矩阵通过；
12. 无 migration、无 canonical mutation、无敏感日志；
13. scoped gates与最终质量门结果已记录；
14. 所有实现偏离、未完成多实例边界和未运行检查均明确记录。
