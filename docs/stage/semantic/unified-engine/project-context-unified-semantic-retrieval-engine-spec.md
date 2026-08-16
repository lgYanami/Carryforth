# Project Context 统一语义检索引擎规范

> 状态：概念规范已确认；兼容基线与第一阶段统一语义计算已交付；第二阶段可靠性运行时实现计划已定稿、待交付
>
> 日期：2026-08-16
>
> 范围：Project Context 语义检索的统一语义计算、可靠性运行时、资源治理，以及公开
> closed operation 的边界
>
> 明确排除：crate、module、trait 与类型布局，SQL 与 migration，队列和调度算法，环境变量命名，
> 具体并发数、超时、重试次数、退避参数、熔断阈值，迁移步骤、灰度步骤和性能资格方案
>
> 关联文档：
> [Project Context 图语义化基础规范](../project-context-graph-semantic-foundation-spec.md)、
> [Project Context 图语义查询实现计划](../project-context-graph-semantic-query-implementation-plan.md)、
> [语义检索兼容基线交付计划](project-context-semantic-retrieval-compatibility-baseline-plan.md)、
> [统一语义计算实现计划](project-context-unified-semantic-computation-implementation-plan.md)、
> [统一语义计算资格记录](project-context-unified-semantic-computation-qualification.md)、
> [统一可靠性运行时实现计划](project-context-unified-semantic-reliability-runtime-implementation-plan.md)、
> [Stage TODO](../../TODO.md)

## 1. 文档目的

Project Context 已经提供多种语义检索操作：

1. 从全图中选择可能相关的 Coordinate 起点；
2. 从一个 Coordinate 选择相关的 incident Edge；
3. 从一个 Edge 选择相关的 member Coordinate；
4. 从自然语言问题生成有界的完整上下文路径。

这些操作的候选范围、排名策略、预算和返回结果不同，但都依赖相同的 semantic foundation、Provider、
current-head exact scoring、安全边界和运行资源。

本文确认一个统一架构目标：

> Project Context 只提供一套内部语义检索引擎。该引擎统一语义计算、可靠性运行时与资源治理；
> 面向调用者的 Coordinate、Edge 和路径操作仍是 closed、独立且可验证的产品合同。

本文定义的是目标状态和必须保持的边界，不定义如何修改现有代码达到该状态。具体实现、迁移和资格设计
必须另写文档。

## 2. 核心结论

统一引擎由三部分组成：

~~~text
Project Context Semantic Retrieval Engine
├── Unified Semantic Computation
│   ├── query encoding
│   ├── generation-bound query vector
│   ├── current semantic source observations
│   └── exact scorer
├── Unified Reliability Runtime
│   ├── admission and bounded waiting
│   ├── deadline and cancellation
│   ├── retry and backoff
│   ├── circuit breaking
│   ├── snapshot recovery
│   └── release-time verification
└── Unified Resource Governance
    ├── Provider capacity
    ├── database scoring capacity
    ├── traversal capacity
    ├── Community and caller fairness
    ├── overload protection
    └── metrics and operational evidence

Closed Operations
├── whole-graph Coordinate discovery
├── Coordinate to incident Edge search
├── Edge to member Coordinate search
└── bounded complete-path search
~~~

必须同时成立：

1. 同一份 canonical source 只属于一套 semantic foundation；
2. 相同的最终语义输入在兼容 generation 中使用同一套编码能力和 query-vector 合同；
3. 所有操作使用同一套 current-head exact scorer；
4. 排队、等待、重试、取消、熔断和快照恢复由统一运行时提供；
5. Provider、数据库和 traversal 资源由统一治理层协调；
6. 每个逻辑操作仍拥有 closed scope、ranking、budget 和 result 合同；已经共享 closed tagged family 的
   操作继续共享既有 wire、capability 和错误族；
7. 统一不得引入可由调用者自由组合的查询 DSL。

## 3. 统一的对象语义基础

Project Context 的可检索对象语义继续来自 canonical source：

- Project View object；
- Project Document；
- Meeting。

对象的 overview、semantic unit 和 embedding 是可重建的派生索引。Coordinate 只引用 canonical source，
不拥有第二份 embedding；Edge 只表达精确 Hyperedge 关系，也不拥有 embedding。

因此：

- Coordinate 检索使用其来源对象的 current semantic observation；
- Coordinate 到 Edge 的语义选择使用 Edge 绑定的 relation Documents；
- Edge 到 Coordinate 的语义选择使用成员 Coordinate 所指来源对象的 current semantic observation；
- 完整路径检索组合相同来源向量、真实 Hyperedge 和关系 Document，不生成或改写项目关系。

统一引擎不得为同一个来源对象按公开操作重复建立多份语义索引，也不得为了统一查询而改变 canonical
Project Context 图。

## 4. 统一语义计算

### 4.1 一套 query encoding 能力

所有语义操作共享同一套文本向量化能力。引擎接收经过验证的语义文本输入，并在同一个 active semantic
generation、模型和 embedding space 中生成 query vector。

操作名称、wire extension、Event kind、scope、limit、权重、floor、重试参数和其他执行元数据不得作为
无关文本发送给 Provider。它们属于本地查询合同，而不是用户问题的语义。

如果两个操作最终使用完全相同的语义文本、模型和 generation，它们应得到同一 query vector。不能仅因
一个调用来自 Coordinate search、另一个来自 Edge search，就人为构造不同的向量语义。

### 4.2 Q0、Qi 与普通 query

Q0、Qi 和普通自然语言 query 是同一编码能力的不同输入：

- Q0 只表达当前问题；
- Qi 表达当前问题以及与本次检索相关的上下文环境；
- 普通 query 表达调用者希望定位的对象或上下文。

Qi 与 Q0 得到不同向量，是因为 Qi 确实包含额外的上下文环境语义，而不是因为它属于不同公开操作。
一个请求可以需要一个或多个语义输入，但每个输入都进入同一验证、编码和 generation 绑定流程。

具体如何组织 problem 与 context environment 的文本，是后续实现设计与兼容迁移事项；本规范只要求：

1. 只有会影响语义的内容进入编码输入；
2. 操作合同标识不伪装成语义内容；
3. 实际编码字节可被完整绑定和审计；
4. 不同输入不会因共享引擎而被误认为同一向量。

### 4.3 统一的 generation-bound query vector

query vector 的共同含义必须包括：

- active semantic generation；
- embedding model 与 dimensions；
- embedding-space compatibility；
- 实际编码输入的 digest；
- 已验证、有限且可用于 exact scoring 的 embedding。

query vector 不公开携带“只能用于某个 CLI”的人为身份。一个 closed operation 能否使用该向量，由它的
scope、ranking 和安全合同决定，而不是由 Provider 文本中的 operation marker 决定。

向量只有在 generation、模型空间和实际输入仍兼容时才可复用。任何不兼容变化都必须使旧向量失效。

### 4.4 一套 exact scorer

所有操作共享一套 current-head exact scorer。它负责在同一可信快照内：

- 只观察授权 Project 中 eligible、current、兼容 active generation 的语义来源；
- 验证 query vector 与 source embedding 属于同一模型空间；
- 计算确定性的 exact similarity；
- 生成统一的固定点基础分数；
- 对相同输入和快照保持稳定排序基础。

exact scorer 只提供“query 与一个 current semantic source 的直接相似度”。它不替公开操作决定：

- 候选 scope；
- Edge 如何按 relation Documents 聚合；
- 是否使用 context gain、anchor、coherence 或 diversity；
- root、beam、path retention 和 response packing；
- operation-specific floor、limit 和 omission。

这些仍是 closed operation 的独立策略。

### 4.5 统一不等于同一排名公式

统一 query encoding 与 exact scorer，不要求四种操作采用同一个最终排名公式。

例如：

- 全图 Coordinate 起点检索可以直接按 Coordinate source similarity 排序；
- Coordinate 到 Edge 可以先评分 relation Documents，再按 Edge 的 closed aggregation 合同排序；
- Edge 到 Coordinate 可以只在完整 Edge 成员集合中排序；
- 完整路径查询可以在 direct similarity 之外使用上下文视角、关系一致性和有界遍历策略。

引擎共享事实一致的基础分数；操作负责把基础分数解释成自己的有限结果。

## 5. 公开操作保持 closed、独立

统一引擎不是一个公开的万能 semantic-query 接口。以下四个逻辑操作继续保持 closed 边界：

| 公开操作 | 候选范围 | 主要返回 | 明确不混入 |
| --- | --- | --- | --- |
| 全图 Coordinate 起点检索 | active graph 中 eligible Coordinates | Coordinate 候选 | Edge、路径 |
| Coordinate 到 incident Edge | 指定 Coordinate 的 incident Edges，以绑定 Documents 提供关系语义 | Edge 候选及匹配的关系 Document 轻量观察 | Edge 成员 Coordinates、完整路径 |
| Edge 到 member Coordinate | 指定完整 Edge 的成员 Coordinates | Coordinate 候选及其轻量观察 | relation Documents、其他 Edges、完整路径 |
| 有界完整路径检索 | verified graph 上的 roots、relations、targets 与 traversal state | roots、paths、provenance 与 coverage | 任意调用者查询计划 |

每个逻辑操作继续独立冻结：

- request variant 与输入上限；
- scope 和 eligibility；
- ranking、floor、budget 与 truncation；
- result variant 与 canonical verifier；
- snapshot 和 release-time currentness 语义；
- 响应大小。

wire 边界不要求与逻辑操作一一对应。已经发布的两个一跳操作继续共享一个 closed tagged request/result
family、Event kind、wire extension、capability 和错误族；统一引擎不得以“独立”为由拆分它，也不得把
任意 scope 混入该 family。全图 Coordinate、one-hop family 和完整路径这三个既有公开 surface 均保持
兼容。

调用者不得提交任意 SQL、filter、vector、模型、动态权重、动态 floor、重试次数、优先级或执行计划。

## 6. 统一可靠性运行时

### 6.1 统一请求生命周期

每个语义请求都进入同一类受控生命周期：

~~~text
validate
  -> authorize and admit
  -> bounded queue
  -> confirm outbound permission
  -> encode semantic input
  -> acquire verified snapshot
  -> exact score
  -> optional operation-specific traversal or projection
  -> release-time verification
  -> validate and sign closed result
~~~

不同操作可以跳过不需要的阶段，例如一跳查询不需要 traversal；但不能绕过共同的授权、admission、
generation、deadline、取消和 release-time verification。

### 6.2 绝对 deadline

deadline 覆盖请求的完整生命周期，而不只是 Provider HTTP 调用。它至少约束：

- 等待 admission 和资源；
- Provider encoding；
- 数据库快照和 exact scoring；
- operation-specific traversal；
- canonical hydration 和 result projection；
- release-time verification 与结果签名。

进入队列不能无限延长请求寿命。任何阶段都不得在 deadline 到期后继续产生新的外发调用、数据库工作或
可见结果。

deadline 窗口的所有权固定为：每个 closed operation 从自己的总预算派生
provider-start/work/snapshot-close/absolute deadline 窗口，包括完整路径既有的
work/snapshot-close/absolute 尾段保留；共享可靠性运行时只接收并遵守这些窗口，不得推导、重置或
延长它们。retry 与 operation restart 不得重置 deadline；在启用任何 Provider retry 前，one-shot
operation 必须显式提供早于 absolute deadline 的 provider-start 窗口，为 RR、release 和收尾保留有界
尾部。共享运行时不拥有完整路径的总预算、traversal 或部分结果策略。

### 6.3 有界等待与负载反馈

可靠性运行时先让现有等待和资源取得路径——admission、Provider reservation wait、数据库取得与
traversal permit——全部受同一 operation deadline 与 cancellation 约束；下游已经无法满足请求时，尽早
返回稳定、低基数、content-free 的负载错误。

新的 bounded queue、队列容量治理与负载反馈 admission 不属于可靠性运行时阶段的承诺，完整移至统一
资源治理阶段交付。等待不承诺所有请求最终成功，也不得削弱 feature gate、authorization 或
release-time fence。

### 6.4 分类重试与 backoff

重试必须由统一运行时按失败类别决定，并同时满足：

- 总次数和总时间有界；
- 使用同一个绝对 deadline；
- 采用受控 backoff；
- 不突破 Provider、数据库或 Community 配额；
- 不把同一逻辑请求伪装成无限多个新请求；
- 可取消；
- 有完整但不泄露 query 的指标。

可以考虑重试的类别包括短暂网络故障、明确可恢复的 Provider 服务失败、可恢复数据库冲突，以及符合
operation snapshot 合同的短暂 currentness 变化。

以下类别不得自动重试：

- authorization、membership、ban、feature gate 或 outbound permission 被拒绝；
- 输入、DTO、签名、模型响应或向量无效；
- canonical result 或 response-size 验证失败；
- caller 取消或 deadline 到期；
- 已知不会因等待而恢复的 capability 与合同不兼容。

具体重试次数、退避曲线和 jitter 不在本规范中决定。

### 6.5 query vector 复用与 snapshot 恢复

当失败只发生在 Provider 之后的 snapshot、topology 或 release 阶段，而且 generation、模型空间和实际
编码输入仍完全相同时，统一运行时可以复用已验证 query vector，避免无意义地再次外发同一文本。

若 generation、模型、dimensions、embedding-space compatibility 或实际输入发生变化，必须重新编码或
fail closed，不能把旧向量带入新语义空间。

snapshot 恢复必须保持每个 operation 自己的快照语义。统一运行时不得把不同 revision、projection
generation 或 source currentness observation 拼成一个结果，也不得在“统一”时偷偷收紧或放宽已发布
operation 的 release 合同。

### 6.6 取消

caller 取消、连接关闭、shutdown 和 deadline 必须能传播到：

- queue wait；
- Provider 请求；
- 数据库查询和事务；
- traversal；
- hydration；
- retry/backoff wait。

取消后的迟到结果不得被签名或返回，资源必须及时归还。后台 semantic indexing 的 durable job 语义与
交互式查询取消语义保持分离。

### 6.7 circuit breaker

统一运行时对共享 Provider 和必要下游维护统一的健康判断。持续、同类、可归因于下游的失败可以触发
circuit breaker，使新请求在外发前快速失败或等待恢复窗口。

circuit breaker 必须：

- 作用于正确的共享故障域；
- 避免某个 operation 单独制造无限探测；
- 支持受控恢复探测；
- 不把授权失败、用户输入错误或结果为空计为 Provider 故障；
- 输出低基数状态和恢复指标；
- 不泄露 query、项目内容或凭据。

具体状态机和阈值由后续实现设计决定。

### 6.8 release-time fail closed

所有成功结果在释放前必须再次验证其共同安全条件仍成立，包括 caller、Project、feature gate、Provider
outbound permission、generation compatibility 和必要的 currentness。

不同 operation 可以有不同的 snapshot release 约束；统一运行时提供共同验证能力，但不能把它们强制
改成同一个 release policy。

## 7. 统一资源治理

### 7.1 独立资源维度

统一治理至少区分：

1. Provider encoding；
2. 数据库 exact scoring；
3. 图 traversal；
4. canonical hydration 与 result projection；
5. queue 长度、等待时间和请求内存。

这些资源的成本不同，不能只用一个全局 semaphore 代表全部负载。一个只需一次 exact score 的一跳查询
不应无条件占用 traversal 容量；完整路径查询也不能通过拆分阶段绕过 Provider 或数据库限制。

### 7.2 统一配置面与 closed operation profile

部署者应从统一配置面治理总容量和保护策略。每个 closed operation 可以拥有受控的资源 profile，用于
描述它会使用哪些资源、允许的最大工作量和服务等级。

operation profile 由系统定义，不由 caller 动态提交。统一配置不得演变成允许外部指定任意并发、优先级、
重试或查询预算的接口。

### 7.3 公平性

资源调度需要同时考虑：

- 全局服务容量；
- Community 公平性；
- caller 公平性；
- operation 成本差异；
- 交互式查询与后台 indexing 的相互影响。

目标不是保证所有请求获得相同延迟，而是防止：

- 一个 Community 占满全局 Provider 或数据库容量；
- 一个 caller 通过并发请求饿死同 Community 的其他 caller；
- 高成本完整路径查询持续阻塞低成本一跳查询；
- 后台 indexing 饿死交互式查询；
- 交互式查询永久阻止 semantic index 收敛。

公平性不改变 authorization，也不向 caller 暴露其他租户的负载信息。

### 7.4 负载保护

引擎应在最早可判定的阶段拒绝不可能完成或必然越界的请求。例如，已知 capability、gate、authorization、
scope 或资源预算不满足时，不应先调用 Provider 再失败。

当下游退化时，统一治理层应限制新的昂贵工作、保护已接受请求的有界完成，并避免重试放大故障。

任何降级都必须保持 closed result 和 fail-closed 安全边界。系统不能通过返回未经 currentness 验证的
缓存结果、部分 Hyperedge 或未签名内容来换取可用性。

## 8. 可观测性与隐私

统一引擎应为所有 operation 提供同口径、可聚合的运行证据。至少覆盖：

- 请求进入、排队、拒绝、取消、成功和失败；
- queue wait、各阶段 latency 与 end-to-end latency；
- Provider attempt、成功、失败、限流、retry 与 circuit 状态；
- 数据库 exact-scoring rows、事务时间、timeout 与资源异常；
- snapshot conflict、恢复与 release-time rejection；
- traversal 工作量、预算耗尽和结果截断；
- 各资源池的 capacity、in-flight 与 saturation；
- Community 和 caller 公平调度是否生效。

指标、日志和错误必须 content-free、低基数。不得记录：

- 自然语言 query；
- context environment 或 overview 正文；
- Provider authorization；
- embedding 或 vector；
- title、summary、Document 正文；
- private key、NIP-98 payload 或完整身份；
- 无界 Project、Community、caller、Coordinate、Edge 或 request 标识标签。

需要关联一次请求内部阶段时，应使用有界、不可反推出项目内容的诊断方式，并遵守既有隐私与日志合同。

## 9. 兼容性与迁移边界

本文描述目标架构，不表示当前四种操作已经完全迁移到统一引擎，也不表示它们已经获得本文列出的全部
可靠性和公平性能力。

在单独实现设计、迁移和资格完成前：

- 现有 wire、Event kind、SDK 和 CLI 合同保持不变；
- 现有 query text、ranking、floor、budget 和 result 语义保持不变；
- 现有 feature gate、fleet attestation 和 release-time snapshot 合同保持不变；
- 不得把架构统一伪装成无行为变化的小重构；
- 不得宣称统一引擎 production-ready。

如果现有编码输入包含 operation-specific contract marker 或 JSON 字段，而目标设计要将其从 Provider
语义文本中移除，这属于 query-text contract 迁移。它可能改变 embedding 和排名，必须单独版本化、评测、
灰度和回滚，不能在抽取公共代码时悄然发生。

同样，新增等待、重试、snapshot 恢复或公平调度会改变延迟、Provider attempt 和失败表现，也必须经过
独立兼容性与容量验证。

## 10. 设计与实施顺序

三个目标按照依赖关系依次交付：

~~~text
兼容基线
  -> 统一语义计算
  -> 统一可靠性运行时
  -> 统一资源治理
  -> 跨操作集成资格
~~~

这个顺序约束独立实现设计和迁移，不代表确认了具体代码结构、调度算法或参数。

### 10.1 先冻结兼容基线

在迁移任何查询前，先为四个逻辑 operation 记录可重复比较的现状，包括：

- 实际 query 输入与 query vector；
- 候选、基础分数、最终排名和结果；
- Provider attempt；
- snapshot 与 release-time 行为；
- deadline、取消和错误分类；
- 资源上限及现有负载行为。

兼容基线不是统一引擎的第四个目标，而是区分“零行为架构迁移”和“有意改变产品行为”的前提。

### 10.2 第一阶段：统一语义计算

先统一 query encoding、generation-bound query vector 和 current-head exact scorer，再让四个逻辑 operation
以各自的 closed scope、ranking、budget 和 result 使用这些共同原语。

这一阶段首先追求零行为变化，不同时调整 query template、权重、floor、路径策略或公开合同。需要改变
Provider 实际输入或排名语义的事项，必须在共同计算原语稳定后作为显式迁移独立评测。

语义计算必须先完成，因为后续运行时只有在明确向量兼容性、确定性评分和 snapshot 边界后，才能判断：

- 哪些失败可以安全重试；
- 哪些 snapshot 恢复可以复用 query vector；
- 哪些变化必须重新编码或 fail closed。

交付状态：第一阶段已经完成，交付物是**共享语义计算基座**，不是统一执行四个公开 operation 的万能
Query Engine。四个逻辑operation现在共同使用closed semantic input、Provider encoding
primitive、Community/generation-bound query vector以及current-head exact scorer；traversal、总预算、
snapshot 恢复范围与 release 策略仍由各 closed operation 独立拥有；公开surface、scope、
ranking、budget、result、error、snapshot与release合同保持兼容。该结论不包含新的retry、queue、circuit、
fairness或production SLO；这些仍属于后续阶段。

### 10.3 第二阶段：统一可靠性运行时

共同计算原语稳定后，再统一 absolute deadline、cancellation、typed failure 分类、retry、backoff、
circuit breaker、snapshot recovery 和 release-time verification 等可靠性原语。

边界与第一阶段同构：共同层只提供最小的执行上下文、Provider 可靠性执行器、typed failure 和可组合
安全原语；它接收 operation 提供的 deadline 窗口，不拥有完整路径的总预算、traversal、frontier、hop、
beam、root、path packing 或部分结果策略，也不新增第二份 operation 策略来源。新的 bounded queue 与
负载反馈 admission 属于第三阶段资源治理。

应先让现有 operation 接入共同生命周期并保持现有行为，再逐项启用新的重试和恢复能力。这样可以
分别验证“接入共同运行时是否正确”和“新增可靠性策略是否正确”，避免两类变化互相掩盖。

可靠性运行时必须先于资源治理，是因为公平排队和容量归还依赖统一的阶段边界、deadline、取消传播及
资源生命周期。

实现设计与交付状态由
[统一可靠性运行时实现计划](project-context-unified-semantic-reliability-runtime-implementation-plan.md)
单独维护。

### 10.4 第三阶段：统一资源治理

在统一运行时能够识别并控制各执行阶段后，再统一 Provider、数据库 exact scoring、traversal、
hydration 和 queue 资源治理，并加入 Community、caller、operation 与后台 indexing 之间的公平性、
过载保护、统一配置和指标。

资源治理的需求和资源维度应在第一阶段开始前就冻结，避免共同计算与运行时接口无法承载治理信息；但其
实际调度与策略实现排在第三阶段。否则容易先为每个 operation 再造一套 semaphore、queue 和限流逻辑，
随后又需要重新拆除。

### 10.5 最后进行跨操作集成资格

三个目标分别完成后，必须在同一运行环境中验证：

- 四个逻辑 operation 的行为与 closed 边界；
- Provider、数据库和 traversal 的共享容量；
- Community 与 caller 公平性；
- retry、circuit、snapshot conflict、取消和 deadline 的组合行为；
- 后台 indexing 与交互式查询互不无界饥饿；
- 过载和故障恢复不会突破 authorization、currentness 或 release fence。

任何单阶段通过都不能替代最终的跨操作容量与故障资格。

## 11. 非目标

统一引擎不负责：

- 把多个公开操作合并成一个 DTO、Event kind 或 CLI；
- 建立任意 semantic query DSL；
- 允许 caller 选择模型、向量、SQL、filter、权重、floor、priority 或 retry policy；
- 让所有操作采用同一最终排名公式；
- 为 Edge 建立独立 embedding；
- 为 Coordinate 复制来源 embedding；
- 改变 Project Context Hyperedge、Document binding 或 canonical source；
- 推断 Agent 当前 Role、Work、权限或上下文环境；
- 自动生成、删除或改写 Project Context 关系；
- 以近似向量检索替代 exact scorer 的既有正确性合同；
- 在本规范中确定具体性能目标、参数、各阶段内部迁移步骤或发布顺序。

## 12. 后续实现设计必须回答的问题

后续独立实现设计至少需要决定：

1. 如何划分内部组件和依赖，同时保持公开 operation 独立；
2. 如何让现有 Coordinate scorer 与其他 exact-scoring 路径共享同一内核；
3. 如何迁移 query encoding 而不混淆 query-text 兼容性；
4. 如何表达 operation-specific scope、ranking、budget、projection 和 release policy；
5. 如何建立统一 queue、retry、backoff、cancellation 和 circuit breaker；
6. 如何在 Provider、数据库、traversal、hydration 之间分配容量；
7. 如何实现 Community、caller、operation 和后台 indexing 的公平性；
8. 如何迁移现有完整路径查询而不改变其多阶段 snapshot 与 traversal 语义；
9. 如何验证零越权、无跨快照拼接、无重复 Provider 放大和无资源饥饿；
10. 如何进行灰度、回滚、容量测试和 production qualification。

这些问题在本文中只有约束，没有具体答案。

## 13. 规范级验收条件

未来实现只有同时满足以下条件，才能声称符合本规范：

1. 四种逻辑 operation 的 closed scope/result 合同没有被万能接口取代，两个一跳 operation 既有的共享
   tagged wire family 也没有被拆分；
2. canonical source 没有因 operation 不同而产生重复语义索引；
3. 相同最终语义输入在兼容 generation 中使用同一编码和向量合同；
4. operation 元数据不作为无关语义文本改变 query vector；
5. 所有 operation 使用同一 current-head exact-scoring 基础；
6. operation-specific scope、ranking、budget 和结果投影仍独立可验证；
7. queue、deadline、retry、backoff、cancellation、circuit 和 snapshot recovery 有统一责任方；
8. 重试不会绕过 authorization、gate、deadline 或资源限制，也不会无限放大 Provider 调用；
9. Provider、数据库、traversal 和 hydration 的容量可分别治理；
10. Community、caller、operation 与后台 indexing 不会无界互相饥饿；
11. release-time 验证保持 fail closed，且不改变各 operation 已冻结的 snapshot 语义；
12. 指标足以解释排队、重试、熔断、冲突和资源压力，同时不泄露 query 或项目内容；
13. 独立实现设计、迁移计划、回归证据和目标规模资格均已完成。

在这些条件完成前，只能说统一语义检索引擎的概念规范已经确认。
