# Stage TODO

## 独立架构事项：统一 Project Context 语义检索引擎

> 状态：兼容基线与第一阶段统一语义计算已交付；第二阶段统一可靠性运行时实现计划已定稿、R0–R4 已交付，R5–R6 待实施
>
> 更新日期：2026-08-17
>
> 概念规范：
> [Project Context 统一语义检索引擎规范](semantic/unified-engine/project-context-unified-semantic-retrieval-engine-spec.md)
>
> 兼容基线：
> [Project Context 语义检索兼容基线记录](semantic/unified-engine/project-context-semantic-retrieval-compatibility-baseline.md)
>
> 第一阶段实现设计：
> [Project Context 统一语义计算实现计划](semantic/unified-engine/project-context-unified-semantic-computation-implementation-plan.md)
>
> 第二阶段实现设计：
> [Project Context 统一可靠性运行时实现计划](semantic/unified-engine/project-context-unified-semantic-reliability-runtime-implementation-plan.md)
>
> 当前进展：四个逻辑operation、三个公开surface的deterministic与真实数据库兼容基线已冻结；真实
> Provider统一canary因缺少受支持配置未运行。统一语义计算的零行为迁移设计已通过代码、currentness、
> lifecycle与兼容性复核；历史v1 oracle和独立Phase 1差分/受保护surface门已同时闭合。共同input、
> model-space fence、Provider-bound result与writer-DB generation-bound vector已经交付；Coordinate与graph
> adapter也已委托同一个bounded Provider batch primitive；两个one-hop variant现通过closed explicit-source
> facade调用原本已经共享的exact SQL。whole-graph Coordinate现也通过共同授权/current-head/distance/
> fixed-score静态kernel执行，并保留独立模板、canonical tie和K+1；同snapshot差分与10k资格已通过。
> bounded complete path现以专属closed Q0/Qi bundle驱动共同root/relation/target scorer，并保持原有
> traversal、packing、retry与release语义；同快照root差分和路径差分已通过。U6已把四个operation切换为
> 同一compiled migrated profile，并通过新fleet runtime digest拒绝旧/新profile混跑。legacy Coordinate SQL与
> graph adapter只保留到2026-09-16的profile rollback窗口。U7 deterministic、disposable pgvector、
> target-scale、feature/gate/fleet与全量单元资格已经关闭。后续已把完整`LLM_*`三元组接入同一Provider
> 配置边界，并以真实Provider完成Coordinate输入和Q0/Qi bundle canary。第一阶段完成。第二阶段统一可靠性
> 运行时实现计划已定稿并完成R0收口：可靠性characterization manifest（四operation的Provider attempts、RR
> 生命周期、release参数、deadline形状、现有retry，完整路径每hop零Provider调用，one-shot permit丢弃冻结为
> known gap）、`just semantic-retrieval-reliability` gate与Phase 2 protected-surface allowlist已交付；目录
> 迁移后的检查脚本与文档死链已同步修复，三个gate实际运行通过。上位规范已明确共同层只接收operation
> deadline窗口、bounded queue承诺移至第三阶段。R1 typed failure与执行上下文已零行为交付：
> `semantic_query_runtime.rs` execution-context类型层（cancellation/latch/deadline windows/attempt
> ledger/Provider handoff/failure taxonomy与closed retry disposition）与 `buzz-db` SQLSTATE分类
> 已交付。R2共享Provider可靠性执行器零策略迁移已完成：四operation（whole-graph Coordinate、
> one-hop两variant、complete-path每个root attempt）接入同一reservation/wait/egress/
> encode-once primitive，neutral判别经冻结映射表还原各surface公开错误，production行为零差异；
> deadline windows与attempt ledger进入生产路径，三个gate实际运行通过。R3
> deadline、cancellation与release-finalize已交付：stage准入/run_stage统一仲裁（cancellation
> biased赢得平局、mid-flight future drop即mandatory cleanup）、shutdown/disconnect传播、
> one-shot release permit同步消费到Event签名（R0 known gap关闭）、complete-path
> partial-result tails逐字保持、latch post-check拒绝发送已取消的签名结果。R4
> 安全retry、backoff与request-local vector复用已交付：runtime独占closed retry
> policy（每item独立route flag、§4.5矩阵行、ledger预算探测、work窗口
> full-fit、full-jitter backoff经run_stage与cancellation竞速），coordinator
> 运行机械循环组装fresh plan（§4.3 fresh ticket/reservation逐attempt重建）；
> transport handoff certainty在私有边界产生（connect失败pre-handoff可重试）；
> one-shot read transient同ticket重开RR并复用bound vector（单restart预算）；
> complete-path churn接入同一ledger并以content-free identity stash实现
> exact-compatible vector复用（不跨generation/input、不持久化）；release
> confirmation仅unsigned/permit-less transient原地重试（上限2次，
> Denied/FleetUnavailable不重试）；declined/exhausted一律返回最后typed
> failure走冻结公开映射。下一步实施R5共享Provider circuit；统一资源治理
> 仍排在其后。

### 背景

Project Context 正在形成四类语义检索面：

1. 已交付的自然语言 Coordinate 起点检索；
2. 已交付的多跳图语义路径检索；
3. 已交付的 `Coordinate → incident Edge` 语义检索，以 Edge 绑定的关系 Documents 作为候选证据；
4. 已交付的 `Edge → member Coordinate` 语义检索，在完整 Hyperedge 成员范围内排序Coordinate候选；
   “下一步”与循环防护仍由后续Agent遍历策略决定。

它们的候选范围与结果合同不同，但都可能复用以下基础设施：

- 自然语言 query validation、canonical template与一次性Provider encoding；
- Community/Project/caller authorization、provider admission与rate/concurrency fence；
- active semantic generation、current source head、embedding model/dimension验证；
- exact vector scoring、fixed-point score与stable tie ordering；
- verified Project Context topology observation与canonical source join；
- request/result binding、release-time authorization/currentness fence；
- coverage、omission、truncation、timeout与content-free error分类；
- Provider、数据库、Relay、SDK和CLI的共同测试seam。

如果为每个查询各自复制这些流程，后续修复安全边界、调整embedding模型或扩展查询形态时容易产生行为漂移。
但把四种操作粗暴合并成一个动态“万能查询DSL”，又会削弱closed DTO、资源上限和fail-closed验证。

### 待解决的架构问题

单独设计一个typed Project Context semantic retrieval engine，明确区分：

```text
共同检索内核
  query encoding
  authorization / admission
  generation + current-head validation
  exact scoring
  canonical graph/source verification
  release fence / coverage

查询专属策略
  global Coordinate candidate scope
  incident relation-Document scope + Edge grouping
  complete-Edge member Coordinate scope
  multi-hop traversal / path retention
```

设计必须回答：

1. 共同内核落在哪个crate/module，如何避免继续扩大现有大型Relay/DB文件；
2. scope resolver、scorer、grouping/projector与result verifier采用哪些closed Rust types；
3. 哪些query template、score解释和budget可以共享，哪些必须按operation独立冻结；
4. global source snapshot、topology-scoped snapshot与multi-hop traversal如何共享同一generation/currentness合同；
5. 两个已交付的一跳语义查询如何迁入统一引擎，同时保持各自的closed scope/result variant及既有共享
   tagged wire family；
6. 如何迁移现有三个wire surface和四个逻辑operation，而不改变已发布的结果、权限与灰度语义；
7. 如何建立跨operation的Provider call-count、current-head、authorization、release-race、资源上限和性能回归矩阵。

### 范围边界

- 这是独立架构调整，不作为当前Agent渐进上下文查询CLI设计的隐藏前置重构；
- 当前阶段不得借此修改已发布的Coordinate search或semantic graph query权重、floor、路径策略或wire合同；
- 不因“统一”而让调用者提交任意SQL/filter/weight/query-plan；公开面继续使用closed operation DTO；
- 不把Edge、Coordinate、Document或path结果强制塞进同一个弱类型result；
- 不在没有迁移与回归证据时一次性替换现有生产查询路径。

### 独立交付入口

后续应新建立项与实现计划，至少包含：

1. 现有三个wire surface、四个逻辑operation与重复设施的只读审计；
2. typed engine API和crate依赖设计；
3. 两个已交付一跳语义操作的零行为变化迁移对照；
4. 现有查询零行为变化的迁移方案；
5. security/currentness/rollout review；
6. target-scale benchmark与跨operation资格门。

概念规范的确认不代表已经决定具体trait、crate、wire合并方式或迁移顺序。
