# Stage TODO

## 独立架构事项：统一 Project Context 语义检索引擎

> 状态：兼容基线、第一阶段统一语义计算已交付；第二阶段统一可靠性运行时主体（R0–R6）已落地，
> 但 RFX-01..RFX-07 七项 correctness 偏差确认——**主体已实现、correctness 修复中**（F0 红色
> 基线已建立；F1 已交付：target-window admission、`TimedOut` 真实 latch、`Finalizing` stage
> 所有权、one-shot eighths reserve、shutdown 订阅与 caller guard，RFX-01/RFX-02 关闭；F2 已
> 交付：unsigned result 验证前移、permit 按值消费进单一同步 signer guard、complete-path 迁入
> 同一形状，RFX-03 关闭；F3 已交付：统一 `execute_provider_attempt` 执行器、circuit 拒绝经
> fresh-authorization 复核后调用方可见、最终 fence 与预算消费合并进单一同步 handoff 点、
> non-counting 预算 token 只计真实 handoff，RFX-04/RFX-05 关闭、rfx 红色基线清零；runtime
> digest 随日期化 descriptor 三轮轮换 `2c898e16…→36776253…→94b3912f…→745ca584…`；F4（RFX-06
> test-first）与 F5 未开始）；第三阶段统一资源治理未启动
>
> 更新日期：2026-08-18
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
> failure走冻结公开映射。R5共享Provider circuit已交付：circuit由Provider
> 实例持有（Arc共享，四operation与每次retry同属一个故障域），gate/重验
> 全部接线在共享executor内（reservation前fast gate、wait后与final egress
> confirm后各一次epoch-token无等待重验，最后一次紧邻Provider调用），
> coordinator无法绕过；failure-domain key为endpoint+model+config epoch的
> content-free SHA-256 digest（config epoch每次构造+1，重配即新domain）；
> Closed/Open/HalfOpen全转移bump epoch（late旧epoch成功不能关闭新circuit），
> half-open为独占真实请求probe、持有者不观察由probe budget回收为Open；
> 429走独立throttle（Retry-After、cap 60s）不计入健康，健康集合=connect/
> 明确5xx/transport unknown/protocol-invalid response；refusal统一走既有
> Busy冻结映射（无新公开code）；shadow默认（spectator token不移动模拟
> 状态），`BUZZ_SEMANTIC_PROVIDER_CIRCUIT_ENFORCE`为isolated single-Relay
> canary开关；process-local circuit不宣称多Pod防惊群（fleet-shared
> epoch/lease属第三阶段）。R6资格、rollout与文档收口已交付，第二阶段关闭：
> reliability contract（route/retry矩阵/attempt caps/backoff/circuit/
> vector-reuse/release）进入编译fleet digest（`2c898e16…`），真实fake
> Provider fault matrix把attempt分类、circuit行与retry决策三视图逐行钉住，
> cancellation/shutdown soak（240迭代×4 source×3形状）通过，gated真实
> Provider canary只断言content-free不变量，buzz-relay binding test把
> descriptor与编译常量互相绑定；资格门运行记录、digest切流表（含Phase 1
> `2026-09-16`窗口依赖与真实fleet切流模板）见
> [统一可靠性运行时资格记录](semantic/unified-engine/project-context-unified-semantic-reliability-runtime-qualification.md)，
> 未运行的disposable DB/真实Provider/真实fleet门逐项列明原因与复跑配方。
> 可声明"统一可靠性原语与Provider执行层已交付"；统一资源治理（bounded
> queue/fairness/capacity、fleet-shared circuit状态）与production SLO属第三阶段。
>
> **2026-08-18 修正**：R6收口时的上述声明被代码审计推翻：七项偏差（RFX-01..RFX-07）与冻结
> 合同不一致，其中RFX-01（complete-path合法partial被全局deadline admission门转为hard
> timeout，`WallTimeExhausted`部分结果在生产路径不可达）构成直接功能错误；RFX-02..RFX-06涉及
> lifecycle状态、release permit顺序、circuit/授权优先级、physical attempt计数与complete-path
> retry边界，RFX-07为资格证据缺口。当前准确状态为"主体已实现、correctness 修复中"：F0已交付
> 失败回归基线（`reliability_fix_regressions` 7个rfx测试故意红色，`just test-unit` 因此红色直至
> F1–F4；三个确定性characterization门保持绿色），修复设计与退出门见
> [正确性修复计划](semantic/unified-engine/fix/project-context-unified-semantic-reliability-runtime-correctness-fix-plan.md)，
> 资格记录§2/§9同步改写。correctness修复关闭前不得声明Phase 2完整交付或具备部署资格。
>
> **2026-08-18 F1 交付**：Deadline与lifecycle修复落地——target-window admission（RFX-01关闭，
> 合法partial tail可达）、`timeout()`真实CAS写入`TimedOut`（RFX-02关闭）、`Finalizing`
> stage所有权（generic仅从`Active`准入）、one-shot内部窗口eighths冻结reserve（公开45s合同
> 不变）、relay shutdown订阅与caller disconnect guard接入生产取消、runtime digest随日期化
> descriptor轮换（`2c898e16…→36776253…`，资格记录§5第三行，三处golden显式重钉）。剩余红色：
> rfx03（F2）、rfx04/rfx05（F3）。
>
> **2026-08-18 F2 交付**：Release-finalize线性化落地（修复计划§3.3/§2.3）——one-shot两surface的
> request binding、结果构造、canonical验证与unsigned Event builder全部前移到release确认之前
> （RFX-03关闭：contract/size失败不再消耗permit或锁上`Finalizing`）；`begin_release_signer`/
> `sign_released`单一同步signer guard按值消费permit（拒绝路径不进签名闭包，签名中取消由
> post-check丢弃已签名结果）；complete-path bridge尾段迁入同一helper形状；两类release policy
> 保持；digest轮换`36776253…→94b3912f…`（资格记录§5第四行/§11）。剩余红色：rfx04/rfx05（F3）。
>
> **2026-08-18 F3 交付**：Circuit、handoff与physical ledger修复落地（修复计划§3.4/§2.4、§2.5）
> ——统一执行器`execute_provider_attempt`承接one-shot与complete-path两surface的Provider
> attempt（RFX-04关闭：circuit拒绝——fast-gate与wait-stale两处——先经调用方fresh-authorization
> 复核闭包（closed coordinator重读writer-fence ticket）才以Busy对调用方可见，复核自身失败按
> 既有冻结映射出栈；最终circuit fence与physical预算消费合并进单一同步handoff点，无await间隔，
> lazy encode仅在handoff之后构造，outcome观测绑定handoff permit）；non-counting
> `ProviderAttemptBudgetToken`在circuit gate之前保留、只在真实handoff消费、pre-handoff任何
> 拒绝Drop退还（physical delta零、transport-retry token退还），caps不变（RFX-05关闭）。
> rfx04/rfx05转绿，**F0红色基线清零**（`reliability_fix_regressions` 14/14绿）；half-open
> probe lease归还仍由既有R5 probe-budget timeout兜底（Copy token无Drop路径，如实记录）。
> digest轮换`94b3912f…→745ca584…`（资格记录§5第五行/§12）。三个确定性门复跑绿色；
> `buzz-relay --lib semantic_` 128绿、全量972绿（8个环境性DB失败）、`just test-unit` exit 0。
> 剩余：F4（RFX-06，DB依赖路径test-first）、F5（资格收口）。

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
