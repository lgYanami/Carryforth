# Project Context 统一语义检索引擎 TODO

> 状态：Skill修正与Coordinate类型过滤实现已完成；真实Agent验收待执行；第二阶段统一可靠性
> 运行时主体（R0–R6）已落地，但代码审计确认 RFX-01..RFX-07 七项与冻结合同的偏差——**主体已
> 实现、correctness 修复中**（F0 红色基线已建立；F1 已交付：RFX-01/RFX-02 关闭；F2 已交付：
> RFX-03 关闭，unsigned result 验证前移、permit 按值消费进单一同步 signer、complete-path 迁入
> 同一形状；剩余红色 rfx04/rfx05→F3；见
> [正确性修复计划](fix/project-context-unified-semantic-reliability-runtime-correctness-fix-plan.md)
> 与[统一可靠性运行时资格记录](project-context-unified-semantic-reliability-runtime-qualification.md)
> §10/§11 F1/F2 交付记录）
>
> 日期：2026-08-18

## Coordinate 起点检索的目标意图与上下文信号

### 已确认观察

`coordinate-search` v1把整段自然语言编码成一个向量，并在所有active-edge、eligible、current
Coordinates的overview中执行direct cosine top-K。一次真实验收中，包含完整问题、泛化Role环境和目标
Work定位要求的宽query未在top 8召回目标Work；保持同一generation与快照、改用聚焦的目标职责query后，
该Work变为rank 1。

当前没有证据表明candidate scope、fixed-point score或DESC排序实现错误。问题在于当前合同无法区分：

- 起点定位的主意图；
- 当前Role与其他上下文环境形成的soft lens；
- 完整问题、后续遍历目标和最终输出要求；
- 自然语言中的对象类型或lifecycle是否属于硬约束。

因此，“排序实现按合同正确”不等于“当前排序目标足以稳定选择起点”。Score只是归一化的相似度信号，
不是置信概率；`truncated=true`只表示存在第K+1个eligible候选。

### 已完成的即时修正

`search-project-context` Skill已收紧起点query：目标Coordinate或责任位置是主信号；当前Role只提供一句
相关责任lens；至多再加入一个必要区分事实。完整problem、最终输出格式和后续Edge/path目标保留在Agent
任务状态，不再机械复制进起点query。已有明确相关Coordinate时仍必须直接起步并跳过全图搜索。

### 已确认的后续方案

当前不继续设计 target/context 多通道融合。Carryforth 已从“服务端一次性理解上下文环境”转为Agent自主
渐进检索：Agent负责理解Role、Work、任务与Meeting环境，查询引擎只负责有界候选召回。

服务端已为返回Coordinate的语义操作增加closed类型过滤，使Agent可以只查`work`、只查`document`、
查询类型组合或省略过滤保持全量。过滤在top-K前执行，不进入Provider文本，也不改变基础相似度；
unfiltered v1与filtered v2保持独立可发现的兼容surface。完整方案与当前交付状态见
[Coordinate语义检索结构过滤方案](../fix/project-context-coordinate-type-filter-design.md)。

### 后续验证

在修改服务端合同前，先用版本化、非敏感标注集比较：原宽query、收窄后的`target + short Role lens`和
target-only query。至少记录Recall@1/3/8、MRR、平均候选观察数、跨类型干扰和跨Role依赖保留情况。

下一步验收修订后的Skill与显式Coordinate类型过滤。只有两者组合后，符合Skill合同的未见query仍不能
把可接受起点召回Top 8，才重新讨论多通道、服务端意图分类或二阶段rerank。不得通过临时调整余弦权重、
静默改写v1 query bytes或加入图邻域anchor掩盖问题。

这项相关性工作与统一可靠性运行时分开验收。第二阶段可靠性运行时已按
[统一可靠性运行时实现计划](project-context-unified-semantic-reliability-runtime-implementation-plan.md)
恢复实施：R0 characterization收口、R1 typed failure/执行上下文类型层、R2共享Provider
可靠性执行器零策略迁移与R3 deadline、cancellation与release-finalize已交付——四operation
（whole-graph Coordinate、one-hop两variant、complete-path每个root attempt）均接入同一
reservation/wait/egress/encode-once primitive，neutral判别经冻结映射表还原各surface公开
错误，production行为零差异；deadline windows与attempt ledger进入生产路径；stage准入/
run_stage统一仲裁deadline与cancellation（biased平局、future drop即mandatory cleanup），
shutdown/disconnect传播至全部等待边界，one-shot release permit同步消费到Event签名
（R0 known gap关闭），complete-path partial-result tails逐字保持，latch post-check拒绝
发送已取消的签名结果。R4安全retry、backoff与request-local vector复用已交付——runtime
独占closed retry policy（每item独立route flag、§4.5矩阵行、ledger预算探测、work窗口
full-fit、full-jitter backoff），coordinator运行机械循环逐attempt组装fresh plan；transport
handoff certainty在私有边界产生；one-shot read transient同ticket重开RR复用bound vector；
complete-path churn以content-free identity stash实现exact-compatible复用；release
confirmation仅unsigned/permit-less transient原地重试；一切declined/exhausted返回最后
typed failure走冻结公开映射。R5共享Provider circuit已交付：circuit由Provider持有
（`Arc`共享，四operation与每次retry同域，executor内接线、coordinator无法绕过），
failure-domain key为endpoint+model+config epoch的content-free digest；
`Closed/Open/HalfOpen`全转移bump epoch（late旧epoch成功不能关闭新circuit），
half-open为独占真实请求probe（持有者不观察由probe budget回收）；429走独立
throttle（Retry-After、cap 60s、只延长）不计入健康；健康集合=connect/明确5xx/
transport unknown/protocol-invalid response，input/4xx/DB/snapshot/cancel不计入；
refusal统一走既有Busy/Unavailable冻结映射（无新公开code）；shadow默认（spectator
token不能移动模拟状态），`BUZZ_SEMANTIC_PROVIDER_CIRCUIT_ENFORCE` 为isolated
single-Relay canary开关；fast gate在reservation前、wait后与final egress confirm后
各一次epoch-token无等待重验（最后一次紧邻Provider调用）；fleet-shared epoch/lease
未交付，不宣称多Pod防惊群（第三阶段）。R6资格、rollout与文档收口已交付，第二阶段
关闭：reliability contract进入编译fleet digest（`d9878ff2…`→`2c898e16…`，
characterization golden同步重钉），真实fake Provider fault matrix把attempt分类、
circuit行与retry决策三视图逐行钉住，cancellation/shutdown soak（240迭代×4
source×3形状）通过，gated真实Provider canary只断言content-free不变量（不存
query/vector/body），binding test把digest descriptor与编译常量互相绑定；资格门
运行记录、Phase 1 `2026-09-16`窗口依赖（computation差分oracle仍以legacy为参考）
与真实fleet切流模板见
[统一可靠性运行时资格记录](project-context-unified-semantic-reliability-runtime-qualification.md)，
未运行的disposable DB/真实Provider/真实fleet门逐项列明原因与复跑配方。可声明
“统一可靠性原语与Provider执行层已交付”；统一资源治理（bounded
queue/fairness/capacity、fleet-shared circuit）与production SLO属第三阶段。

**2026-08-18 修正**：上述R6收口声明被代码审计推翻——七项偏差（RFX-01..RFX-07）确认与冻结
合同不一致，其中RFX-01（complete-path合法partial被全局deadline门转为hard timeout）构成直接
功能错误。当前准确状态为“主体已实现、correctness 修复中”：F0已交付失败回归基线
（`reliability_fix_regressions` 7个rfx测试在当前代码上失败，`just test-unit` 因此保持红色直至
F1–F4逐项修复；三个确定性characterization门不受影响），修复设计、分阶段交付（F0–F5）与退出门
见[正确性修复计划](fix/project-context-unified-semantic-reliability-runtime-correctness-fix-plan.md)；
资格记录（§2门清单、§9红色基线）同步改写。correctness修复关闭前，不得声明Phase 2按实现计划
完整交付或具备部署资格。

**2026-08-18 F1 交付**：Deadline与lifecycle修复完成（修复计划§3.2）——target-window
admission（RFX-01关闭，complete-path合法partial tail可达）、`timeout()`真实CAS写入
`TimedOut`（RFX-02关闭）、`Finalizing`仅接受finalize stage的ownership规则、one-shot内部
eighths冻结reserve（公开45s合同不变）、relay shutdown订阅与caller disconnect guard进入
生产取消路径、runtime digest随日期化descriptor轮换（`2c898e16…→36776253…`，三处golden
显式重钉，资格记录§5/§10）。三个确定性门复跑绿色；剩余红色rfx03（F2）、rfx04/rfx05（F3）。

**2026-08-18 F2 交付**：Release-finalize线性化完成（修复计划§3.3/§2.3）——one-shot两surface的
unsigned result构造、canonical验证与Event builder（含response cap）全部前移到release确认之前；
`begin_release_signer`/`sign_released` guard按值消费permit（唯一构造点、无Clone、`#[must_use]`，
拒绝路径消费permit且绝不调用签名闭包，签名中cancel/deadline由§4.1 post-check丢弃已签名结果）；
complete-path bridge尾段删除手写四段finalize逻辑迁入同一helper；两类release policy（exact-snapshot
与current-authorization）保持。RFX-03三测转绿；digest轮换`36776253…→94b3912f…`（资格记录
§5第四行/§11）。三个确定性门复跑绿色；剩余红色rfx04/rfx05（F3）。
相关性验收结果只影响
Coordinate检索的排名合同，不阻塞可靠性阶段的零行为迁移步骤；若验收要求改变公开surface，必须按独立
版本化设计处理，不得混入可靠性迁移。
