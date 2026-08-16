# Project Context 语义检索兼容基线交付计划

> 状态：已执行并归档；B4在基线冻结时未运行，后续Provider encoding证据见Phase 1资格记录
>
> 日期：2026-08-15
>
> 代码基线：feat/semantic-engine，e8f26d6e65
>
> 上位规范：
> [Project Context 统一语义检索引擎规范](project-context-unified-semantic-retrieval-engine-spec.md)
>
> 范围：在统一语义计算开始前，审计并以可执行证据冻结四个逻辑 operation、三个公开 surface 的
> 当前兼容合同和运行画像
>
> 明确排除：统一引擎代码、query template 调整、权重或 floor 调整、新 retry/fairness/circuit 行为、
> 生产 SLO、schema 或 semantic index 迁移

## 0. 已确认的阶段决策

1. 兼容基线以现有实现审查为起点，但不能只交付审查文档；必须形成可重复执行的 characterization。
2. 本阶段不修改生产查询行为，不开始统一语义计算、可靠性运行时或资源治理。
3. 基线按四个逻辑 operation 记录，但尊重现有三个公开 surface；两个一跳 operation 继续共享 closed
   tagged wire family。
4. 永久基线使用合成 Project、固定 fake encoder vector 和确定性 snapshot，不依赖真实 Provider 漂移。
5. 真实 Provider 只做有界、授权、feature-off 可回滚的运行资格；真实 vector、原始请求和精确分数不进入
   仓库。
6. 必须区分“后续迁移必须保持的兼容合同”与“可靠性或资源治理阶段可以显式改变的当前运行画像”。
7. 当前已知缺陷或未完成资格单独记录为 known deviation；冻结现状不等于把缺陷永久产品化。
8. 任何 query-text、ranking、retry、snapshot 或错误表现变化都必须由后续阶段显式声明，不能借公共代码
   抽取静默发生。

## 1. 阶段目标

本阶段要建立统一语义检索引擎后续迁移使用的判断依据：

> 给定同一个有序合成语义输入 bundle、generation、对应的固定 query-vector bundle、verified snapshot
> 和 closed operation，
> 能够机械判断迁移前后的 scope、基础分数、排名、结果、安全边界及错误是否保持一致。

同时记录当前系统在真实执行中的：

- Provider input 与 attempt 形态；
- admission、Busy、deadline、取消和 retry 表现；
- snapshot/release 行为；
- Provider、数据库和 traversal 资源入口；
- 已有指标与可观测性缺口。

后一组是“当前运行画像”，用于下一阶段设计，不自动成为永久兼容合同。

## 2. 成功定义

阶段完成时必须具备：

1. 一张四个逻辑 operation、三个公开 surface 的 source-of-truth 矩阵；
2. 一套所有 operation 共用的非敏感合成语义图 fixture；
3. 固定 fake encoder 和固定向量下的 query、exact score、排序与结果 characterization；
4. SDK、Relay、数据库和 CLI 的可执行兼容门；
5. authorization、currentness、snapshot/release、deadline 和取消的基线矩阵；
6. 一个统一入口运行确定性基线并生成 content-free manifest；
7. 一次隔离数据库资格以及一次有界真实 Provider canary，或明确记录未运行原因；
8. 一份兼容基线记录，区分 protected contract、current profile、known deviation 和未关闭资格；
9. 后续实现能够让旧路径和新路径消费同一个有序 query-vector bundle 与同一个 snapshot，执行
   differential comparison。

本阶段完成不代表统一引擎已经实现，也不代表 production-ready。

## 3. 当前查询面盘点

### 3.1 四个逻辑 operation 与三个 surface

| 逻辑 operation | 公开 surface | 当前语义输入 | 当前执行形态 |
| --- | --- | --- | --- |
| 全图 Coordinate 起点检索 | Coordinate search，result kind 40913 | 独立 Coordinate query-text 合同 | one-shot |
| Coordinate 到 incident Edge | one-hop tagged family，result kind 40914 | 完整路径 Q0 | one-shot |
| Edge 到 member Coordinate | one-hop tagged family，result kind 40914 | 完整路径 Q0 | one-shot |
| 有界完整路径检索 | semantic graph query，result kind 40912 | Q0 及可选 Qi channels | multi-stage traversal |

基线不得先假设“相同自然语言当前已经跨四个 operation 得到相同向量”。Coordinate search 使用独立
query template；一跳和完整路径 Q0 使用另一份模板。这是当前事实，未来是否迁移由统一语义计算设计决定。

### 3.2 当前两类执行生命周期

当前 Coordinate search 与 one-hop family 已共享 one-shot 安全外壳；完整路径查询拥有独立的多阶段
orchestration、traversal 和 packing 生命周期。

必须分别记录：

- one-shot 的 snapshot-bound release 行为；
- 完整路径的现有 release 语义；
- one-shot 当前无内部 retry 的表现；
- 完整路径在特定 generation/context churn 下的当前 attempt 行为；
- 三个 surface 的 admission、Provider 和 deadline 入口。

这些差异不得在基线阶段被“统一”掉。

### 3.3 现有证据

可复用的现有证据包括：

- buzz-semantic-query 中 Coordinate、one-hop、完整路径的 pure contract 和 ranking tests；
- buzz-sdk 中三个 surface 的 request/result binding、Event、tag 和 signer verification tests；
- buzz-db 中 Coordinate exact SQL、one-hop scoped search、完整路径 exact scorer 与 release-lock tests；
- buzz-relay 中 Provider adapter、one-shot、完整路径 orchestration、traversal 和 packing tests；
- carryforth-cli 中三个 surface 的 exact filter、NIP-98、no-redirect、no-client-retry 和输出验证；
- Coordinate 10k exact SQL qualification；
- one-hop disposable pgvector 与真实 Provider canary；
- 完整路径 synthetic exact-kernel、真实 Provider 和 Desktop D6 资格证据。

既有证据可以纳入基线，但不能用零散历史报告代替统一、可重复的 characterization runner。

## 4. 基线的三类事实

### 4.1 Protected contract

以下内容默认要求零行为保持，除非后续有独立迁移设计：

- request/result closed DTO 或 tagged variant；
- Event kind、wire extension、capability 和 request-result binding；
- authorization、host-derived Community、Project 与 caller 边界；
- query validation、输入上限和 canonical query bytes；
- generation、model、dimension 和 embedding-space compatibility；
- candidate eligibility、current-head、scope 和 topology completeness；
- fixed-vector exact score、stable tie 与 deterministic ordering；
- operation-specific floor、limit、budget、omission 和 truncation；
- result projection、canonical verifier、response cap；
- snapshot 与 release-time currentness 语义；
- feature-off、gate-off 和 capability-off 的 pre-Provider fail closed。
- 返回给调用者的 HTTP status、closed error code、retryable 字段、body shape 和 CLI exit category。

### 4.2 Current runtime profile

以下内容必须被准确记录和测试，但允许在可靠性运行时或资源治理阶段有意改变：

- 立即 Busy 还是有界等待；
- 当前 Provider attempt 和 batch count；
- 当前 operation 是否内部 retry；
- deadline 分配、失败发生阶段与阶段覆盖；
- disconnect/cancellation 的传播范围；
- semaphore、连接和 traversal admission；
- 当前 latency、吞吐和 saturation；
- 当前指标聚合和负载反馈。

可靠性阶段可以改变一次请求在返回前是否等待、重试或恢复，但不能借此静默改变最终返回的公开
status/code/retryable/body shape 与 CLI exit 映射；公开错误变化需要独立版本化迁移。

后续改变这些行为时，必须更新 profile、资格和版本说明，但不应被误判成 wire 兼容破坏。

### 4.3 Known deviation

若审计发现现有实现与已发布文档、安全边界或测试不一致：

1. 不在本阶段顺手修复；
2. 不把错误行为写成新的产品规范；
3. 以 known deviation 记录触发条件、影响和证据；
4. 由独立修复或迁移计划决定预期；
5. 修复后更新基线版本。

## 5. 两条证据轨

### 5.1 可提交的确定性基线

仓库永久保存：

- 合成 fixture；
- 固定 fake encoder 输出；
- canonical query input 与 digest；
- generation/model/dimension 等兼容元数据；
- fixed-point exact score 和 stable ordering；
- normalized result；
- closed error category；
- Provider attempt/batch count；
- snapshot/release 结果；
- fixture、manifest 和 baseline version 的 hash。

确定性基线必须在无网络、无真实 Provider、无生产数据库的环境中可重复运行。

### 5.2 真实数据库与 Provider 资格

真实运行证据用于证明执行链确实可工作，并观察当前 runtime profile。仓库只提交脱敏摘要：

- run identity 与代码 commit；
- adapter、model contract、dimensions 和 generation identity；
- semantic input digest，不含原始输入；
- Provider attempt/batch delta；
- operation 成功数与 closed error category；
- scope/result invariant 是否通过；
- 范围化 latency 和资源指标；
- feature enable/disable 与 rollback 结果。

以下内容不得成为永久 golden 或提交到仓库：

- 真实 query 和 context overview；
- Provider HTTP body、headers、URL、request ID 和错误正文；
- 实际 embedding/vector；
- 真实 similarity float、精确 fixed-point score 和精确排名；
- title、summary、Document 正文、Coordinate/Edge identity；
- private key、NIP-98 payload 或完整 caller identity；
- 单次精确 latency、吞吐、rate-window、jitter 或熔断时序。

真实 Provider 的结果只能在同一次冻结 generation 的运行内做对照；Provider 漂移不能自动被判为代码兼容
回归。

## 6. 合成基线 fixture

### 6.1 Fixture 目标

建立一份非敏感、确定性、可由四个逻辑 operation 共用的 Project Context fixture，至少包含：

- 多种 Project View Coordinate；
- Project Document 与 Meeting Coordinate；
- 多条完整、无向 Hyperedge；
- 每条测试 Edge 的多份 relation Documents；
- 共享 Coordinate 和共享 relation Document；
- 相同 exact score 的 stable-tie 候选；
- active、inactive、detached、terminal 和 tombstoned 状态；
- current、missing、building、failed、stale 和 zero-vector source head；
- 同一 Coordinate 出现在多条 Edge 的去重情况；
- 可完成路径、循环候选、预算截断和 endpoint 重合路径。

fixture 只为覆盖合同边界，不模拟真实项目语义质量。

### 6.2 固定语义输入

为四个 operation 冻结合成输入：

- Coordinate search 的当前 canonical encoder input；
- one-hop Q0 的当前 canonical encoder input；
- 完整路径 Q0；
- 至少一个带 context overview 的 Qi；
- Unicode、转义和边界长度输入；
- validation error 输入。

永久 golden 记录精确 UTF-8 bytes 与 digest。真实 Provider canary 只记录 digest。

### 6.3 固定 fake encoder

fake encoder 对每个合成输入返回已命名、有限、非零、维度固定的向量，使 fixture 能机械产生：

- 不同基础分数；
- 相同分数 tie；
- floor 上下边界；
- context gain；
- 多 channel candidate matrix；
- K+1 truncation；
- one-hop Edge aggregation；
- 路径 root、relation、target 和 retention 差异。

fake vector 是测试输入，不代表真实模型质量。

## 7. 确定性 characterization 矩阵

### 7.1 Query encoding 与 Provider adapter

每个逻辑输入锁定：

- validation；
- canonical bytes 和 digest；
- query contract/ranking digest；
- generation/model/dimensions binding；
- Provider batch 输入数量与顺序；
- model、count、dimension、finite、nonzero 等响应验证；
- 当前 attempt 数量；
- Debug、error 和 metrics 不包含输入或 vector。

### 7.2 Scope、exact score 与 ranking

固定向量和固定 snapshot 下锁定：

- eligible/current 过滤发生在 distance 之前；
- exact cosine 到 fixed-point score；
- stable tie；
- active-edge Coordinate 去重；
- Coordinate-only 全图 scope；
- incident Edge relation-Document scope 和 Edge aggregation；
- complete Edge member Coordinate scope；
- Q0/Qi candidate matrix、root fusion、relation/target/coherence/floor；
- traversal cycle、beam、global budget、stop precedence 和 path retention；
- omission、coverage、truncation 和 response cap。

### 7.3 Closed result 与三个 surface

锁定：

- kind 40912、40913、40914 的 relay-only/response-only 边界；
- exclusive closed HTTP filter；
- one-hop 两个 variant 的隔离与共享 family；
- exact request body 与 NIP-98 binding；
- Relay signer、caller、Project、request binding 和 canonical content verification；
- ordinary REQ/COUNT/search/submit 不泄露 virtual result；
- CLI 参数、exit category 和稳定 JSON 字段语义。

不以 pretty formatting、JSON key order、时间戳、签名或完整 help 文本作为 golden。

### 7.4 Authorization、currentness 与 race

对三个 surface 建立同口径矩阵：

- nonmember、ban、owner/member revocation；
- Project/Community mismatch；
- feature master、Community gate、capability、fleet 和 provider unavailable；
- Provider 前 revocation 为零 egress；
- Provider 后、snapshot 前变化；
- snapshot 内 source/Edge/current-head 变化；
- release 前 revocation或generation变化；
- detached/tombstone/source revision变化；
- 结果不跨 revision、projection generation 或 semantic generation 拼接。

### 7.5 Deadline、取消与当前 retry

记录：

- queue/admission wait 是否消费 deadline；
- Provider、数据库、traversal、hydration 和 release 当前受哪个 deadline 约束；
- caller disconnect、explicit cancel 和 shutdown 的当前传播；
- 取消后是否仍产生 Provider/DB/traversal 工作；
- one-shot 当前 attempt 行为；
- 完整路径当前可恢复 churn 的 attempt 行为；
- timeout、busy、cancelled、unavailable 和 internal 当前在什么阶段触发及经过多少次尝试。

各失败最终映射到的公开 status/code/retryable/body shape 与 CLI exit category 按 protected contract
锁定。本阶段只记录现状，不新增 retry、backoff 或 circuit breaker。

### 7.6 Resource 入口画像

记录但不改变：

- Provider admission 与 rate-debt 入口；
- exact scoring 的数据库连接和事务入口；
- traversal semaphore；
- hydration/packing 成本；
- background indexing 与 interactive query 的共享资源；
- 当前 metrics 与缺失指标。

本阶段不承诺公平性，也不设置新的并发或 SLO。

## 8. Differential seam

确定性 runner 必须为后续迁移提供 differential seam：

~~~text
same synthetic request
  -> one ordered canonical semantic-input bundle
  -> one corresponding fixed query-vector bundle
  -> one verified snapshot
  -> legacy operation path
  -> migrated operation path
  -> normalized result and closed-error comparison
~~~

单输入 operation 的 bundle 长度为1；完整路径的 bundle 包含有序 Q0 与实际启用的 Qi channels。迁移
对照不得让 legacy 与 migrated path 各自再次调用 Provider，否则 Provider 漂移、重试和外发次数会污染
语义计算差异。

normalized comparison 必须保留 scope、score、rank、omission、coverage、snapshot 和 result content，
只移除随机 request ID、签名、创建时间和运行耗时等非语义字段。

本阶段只建立 seam 和 legacy baseline；migrated path 由后续统一语义计算阶段接入。

## 9. 交付物

### 9.1 文档

新增：

- 本计划；
- Project Context 语义检索兼容基线记录。

基线记录至少包含：

- operation/surface source-of-truth 矩阵；
- protected contract；
- current runtime profile；
- known deviations；
- deterministic manifest hash；
- real DB/Provider 资格摘要；
- 未关闭资格和下一阶段允许变化清单。

### 9.2 可执行门

计划新增一个统一入口，例如：

~~~text
just semantic-retrieval-compatibility-baseline
~~~

该入口应运行：

- buzz-semantic-query deterministic characterization；
- buzz-core/SDK virtual-kind 和 wire verification；
- buzz-db fixed-vector exact-scoring 与 snapshot tests；
- buzz-relay fake-Provider orchestration/race tests；
- carryforth-cli command、binding 和 output tests；
- manifest 结构与 hash gate。

服务型、真实数据库和真实 Provider 资格使用显式独立入口，不能隐藏在默认 unit gate 中。

### 9.3 资格 runner 与产物

计划新增统一资格 runner。它可以复用现有：

- semantic exact-query qualification；
- Coordinate exact SQL qualification；
- one-hop disposable pgvector fixture；
- 三个 surface 的 SDK、CLI、运维入口和历史 canary 步骤。

当前仓库没有可直接重复运行的三-surface真实 Provider canary runner；此前部分临时 native seam 已删除。
因此统一、可回滚的 canary seam 是本阶段新增的资格基础设施，不得把历史报告描述成现成 runner。

本地产物写入 ignored 的 test-results 目录。仓库只提交 content-free qualification record 和必要的合成
fixture/golden。

### 9.4 预期代码影响

允许修改：

- 既有测试模块和 test-only helper；
- 新的合成 fixture、manifest 生成器和 differential test seam；
- Justfile 与 scripts 中的资格入口；
- 基线计划与资格记录。

原则上不修改：

- production query builder；
- Provider production adapter；
- DB production SQL；
- Relay production orchestration；
- public DTO、Event kind、wire、capability、gate；
- schema、migration、semantic source extractor 和 worker。

若缺少可观察 attempt、cancel 或 release race 的测试 seam，只能增加 cfg(test) 或 acceptance-only seam，并
单独证明 production build、公开 API 和运行行为未改变。acceptance-only seam 必须在 default-feature
production build 中不编译、不导出且不可被生产请求触达。

## 10. 分阶段交付

### Phase B0：只读盘点

- 完成四 operation、三 surface、两生命周期矩阵；
- 标记 protected contract、current profile、known deviation；
- 映射现有 test、qualification 与缺口；
- 记录当前 commit、toolchain、schema/fleet/query/ranking digest。

退出门：矩阵无未知 owner，两个 one-hop variant 的共享边界记录准确。

### Phase B1：合成 fixture 与 manifest

- 建立共用 Project Context fixture；
- 建立固定 fake encoder vectors；
- 冻结 canonical input bytes/digest；
- 定义 normalized result 和 manifest schema；
- 生成首份 deterministic baseline hash。

退出门：同一 commit 连续运行结果一致，不含时间、签名和随机身份噪声。

### Phase B2：Pure、wire 与 CLI characterization

- 补齐四 operation pure contract；
- 补齐三个 surface 的 SDK/wire/virtual-kind矩阵；
- 锁定 Provider batch/attempt；
- 锁定 CLI 参数、binding、错误类别和稳定字段语义。

退出门：无网络、无服务的确定性门全部通过。

### Phase B3：数据库、Relay 与 race characterization

- 在 disposable pgvector 中运行同一 fixture；
- 锁定 exact score、scope、ranking、snapshot 和 release；
- 补齐 revocation/currentness/deadline/cancel矩阵；
- 记录三 surface 当前 retry 与资源入口画像；
- 建立 future migrated path 使用的 differential seam。

退出门：固定向量结果与 pure expectation 一致，所有差异均被解释或列为 known deviation。

### Phase B4：真实 DB 与 Provider canary

- 使用同一冻结 fixture 和同一 active generation；
- 分别运行四个逻辑 operation；
- 记录 Provider attempt、闭合结果与范围化 latency；
- feature-off 后再次调用，确认 preflight fail closed 且 Provider delta 为零；
- 回滚所有临时 gate、fleet 和进程状态。

退出门：canary 成功，或以明确 external/infra blocker 记录为未完成；不得用失败的真实 canary否定已经通过
的 deterministic compatibility。

### Phase B5：记录、审查与冻结

- 发布兼容基线记录；
- 固定 fixture、manifest 和 baseline hash；
- 复核无生产行为、schema、wire 或 gate 默认值变化；
- 执行 production-path diff allowlist、default-feature build、public API/wire/digest 和 manifest/record
  聚合门；
- 复核未记录真实或非合成 query、真实 Provider vector、真实项目内容、身份或凭据；合成 query 与固定
  fake vectors 只允许出现在明确标记的 tracked fixture/golden 中；
- 明确下一阶段哪些行为必须零变化，哪些 current profile 可以显式迁移。

退出门：代码、文档、安全和资格审查通过，才可进入“统一语义计算”实现设计。

## 11. 质量门

计划执行至少包括：

~~~bash
. ./bin/activate-hermit

cargo test -p buzz-semantic-query --lib
cargo test -p buzz-core --lib
cargo test -p buzz-sdk --lib
cargo test -p buzz-db --lib
cargo test -p buzz-relay --lib
cargo test -p carryforth-cli

just semantic-retrieval-compatibility-baseline
just semantic-test
just semantic-query-qualification
just coordinate-search-qualification

cargo clippy \
  -p buzz-semantic-query \
  -p buzz-core \
  -p buzz-sdk \
  -p buzz-db \
  -p buzz-relay \
  -p carryforth-cli \
  --all-targets -- -D warnings

just test-unit
~~~

其中 semantic-retrieval-compatibility-baseline 聚合门必须实际执行并验证：

- deterministic manifest schema、内容和 hash；
- qualification record 的结构与必填矩阵；
- 隐私扫描，并只允许明确目录中的合成 query 与 fake vectors；
- 相对代码基线的 production-path diff allowlist；
- default-feature production build；
- public API、wire、fleet、query-contract 和 ranking digest 未发生非预期变化。

真实 Provider、disposable pgvector、长期服务和 target-scale 门是否运行，必须在基线记录中逐项说明。最终
仓库门仍按 AGENTS.md 要求执行 just ci；不能运行时必须明确原因。

## 12. 风险与控制

### 12.1 把 Provider 漂移误判为代码回归

控制：永久 golden 只使用固定 fake vector；真实 Provider 只在同一次 run 内比较并提交脱敏摘要。

### 12.2 把当前缺陷永久冻结

控制：protected contract、current profile 和 known deviation 分栏；已知缺陷必须走独立修复决策。

### 12.3 Characterization 本身改变生产代码

控制：production path 默认不可改；新增 seam 必须是 cfg(test) 或 acceptance-only。acceptance-only seam
在 default-feature production build 中不得编译、导出或可达，并通过 production-path diff allowlist、
public API/wire/digest 与 default-feature build 审查。

### 12.4 全文 snapshot 造成脆弱测试

控制：只锁定稳定字段语义、canonical bytes/digest、normalized result 和 closed category；不锁定格式噪声。

### 12.5 真实证据泄露项目数据

控制：只使用非敏感 fixture，普通日志 content-free，真实产物忽略；隐私扫描显式允许 tracked fixture
中的合成 query 与 fake vectors，拒绝其他 query、Provider vector、项目内容、身份和凭据。

### 12.6 零行为基线被性能数据阻塞

控制：精确 latency 和吞吐只记录，不作为永久兼容 golden；目标 SLO 留给资源治理资格。

## 13. 非目标

本阶段不：

- 实现统一 query encoding、query vector 或 exact scorer；
- 删除现有 Coordinate query template 或 contract marker；
- 修改 Q0/Qi 语义；
- 调整 score、weight、floor、budget、MMR、beam、path retention；
- 增加 queue、retry、backoff、circuit breaker 或 snapshot recovery；
- 增加 Community/caller fairness 或新的资源池；
- 合并三个 surface 或拆分 one-hop shared family；
- 修改 CLI、SDK 或 Desktop 产品能力；
- 修改 schema、migration、semantic index 或 canonical Project Context；
- 冻结真实 Provider vector、绝对分数、绝对排名或精确性能；
- 宣称 production-ready。

## 14. 完成定义

只有以下条件全部满足，兼容基线才算冻结：

1. 四个逻辑 operation、三个公开 surface 和两个执行生命周期均有 owner 与证据；
2. protected contract、current runtime profile 和 known deviation 已明确分离；
3. 合成 fixture、fake vectors、normalized results 和 manifest 可重复；
4. query bytes/digest、fixed-vector score、scope、ranking、result 与 closed errors 有可执行门；
5. authorization、currentness、snapshot/release、deadline、取消和当前 attempt 行为有证据；
6. differential seam 能让未来 legacy/migrated path 使用同一有序 vector bundle 与 snapshot；
7. 真实数据库和 Provider 资格已执行或明确记录未执行原因；
8. 仓库未保存真实 query、vector、项目内容、凭据或无界身份；
9. 没有生产查询、schema、wire、capability、gate 默认值或 CLI 行为变化；
10. 基线记录、manifest hash、审查结论和未关闭资格已发布；
11. 后续变化规则明确：零行为迁移必须通过基线；有意变化必须独立设计、版本化和重新资格。

满足这些条件后，才进入统一语义计算的实现设计。
