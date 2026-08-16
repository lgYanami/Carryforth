# Project Context 统一语义检索引擎 TODO

> 状态：Skill修正与Coordinate类型过滤实现已完成；真实Agent验收待执行；第二阶段统一可靠性运行时实现
> 计划已定稿并交付R0、R1、R2、R3、R4、R5，R6待实施
>
> 日期：2026-08-17

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
未交付，不宣称多Pod防惊群（第三阶段）。相关性验收结果只影响
Coordinate检索的排名合同，不阻塞可靠性阶段的零行为迁移步骤；若验收要求改变公开surface，必须按独立
版本化设计处理，不得混入可靠性迁移。
