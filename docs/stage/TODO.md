# Stage TODO

## 独立架构事项：统一 Project Context 语义检索引擎

> 状态：概念规范、兼容基线与统一语义计算实现设计已冻结；U0–U2已交付，待进入U3
>
> 更新日期：2026-08-16
>
> 概念规范：
> [Project Context 统一语义检索引擎规范](semantic/ unified-engine/project-context-unified-semantic-retrieval-engine-spec.md)
>
> 兼容基线：
> [Project Context 语义检索兼容基线记录](semantic/ unified-engine/project-context-semantic-retrieval-compatibility-baseline.md)
>
> 第一阶段实现设计：
> [Project Context 统一语义计算实现计划](semantic/ unified-engine/project-context-unified-semantic-computation-implementation-plan.md)
>
> 当前进展：四个逻辑operation、三个公开surface的deterministic与真实数据库兼容基线已冻结；真实
> Provider统一canary因缺少受支持配置未运行。统一语义计算的零行为迁移设计已通过代码、currentness、
> lifecycle与兼容性复核；历史v1 oracle和独立Phase 1差分/受保护surface门已同时闭合。共同input、
> model-space fence、Provider-bound result与writer-DB generation-bound vector已经交付；Coordinate与graph
> adapter也已委托同一个bounded Provider batch primitive。下一步进入U3 one-hop tagged family共同scorer迁移。

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
