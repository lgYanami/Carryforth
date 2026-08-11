# Project Context Desktop 图语义查询资格报告

> 状态：D6 资格验证进行中；本地隔离门、Desktop light / dark 视觉证据和 local single-pod
> 真实 Provider 链路 smoke 已通过，但 known-negative 仍产生候选，真实 relevance / floor 质量校准
> 未通过；production LB / multi-pod qualification 尚无部署证据
>
> 日期：2026-08-11
>
> 基线提交：`507790180 feat(desktop): add project context semantic paths`
>
> 实现计划：
> [Project Context 图语义查询 Desktop 实现计划](./project-context-semantic-query-desktop-implementation-plan.md)
>
> 上游计划：
> [Project Context 图语义检索实现计划](../project-context-graph-semantic-query-implementation-plan.md)

## 1. 当前结论

本报告只记录 D6 的可复现资格证据，不把代码级合同测试替代真实 Provider、目标 PostgreSQL、实际
负载均衡 fleet 或首个灰度 Community 的运行证据。

截至当前记录：

- PostgreSQL 17、pgvector 0.8.5、2048 维向量与 SQLx round-trip 的隔离探针通过；
- 0057 → 0058、ledger-less fresh schema、desired-schema parity 与 index-disable 原子关闭通过；
- pgvector exact cosine 的排序和 fixed-point score 与进程内参考结果一致；
- revoke / ban 与 semantic result release 使用同一 Community lock 的线性化顺序通过真实 PostgreSQL测试；
- Relay 的 capability、raw parser、NIP-98 exact body、Provider boundary、traversal、response cap、postflight
  与 content-free observability 定向合同通过；
- 已新增隔离的 synthetic exact-kernel qualification runner，完成 2,000 / 10,000 source、default / hard-cap
  channel与recall profile的 EXPLAIN、p50 / p95 / p99、短并发soak、statement cancellation、temp spill、
  transaction age与vacuum观察；目标部署的canonical SQL、真实Community规模、冻结SLO和多Pod / Provider
  soak仍须形成独立证据；
- Desktop light / dark语义路径截图已生成、复核并通过hash distinctness；
- local single-pod 使用 Desktop 已配置身份完成 9 项真实 Volcengine 查询：请求链路 9 / 9 成功、
  四类返回分数均无 floor 违规，且同一 problem 的两个 context environment 产生不同 root / path 集合；
- known-negative 仍返回 6 个 roots / 12 条 paths，因此只能证明真实 Provider 交互和 floor 执行，不能
  宣称 relevance 质量或 provisional floor 已完成校准；
- local feature-off 已关闭 query gate、撤销 fleet assertion并停止 NIP-11 capability；disable 后的
  Native semantic query闭合为 `unsupported`，Provider physical gate与interactive admission摘要均不变，
  ordinary canonical read仍成功；
- production LB inventory、multi-pod fleet / Provider contention与长soak没有部署证据，不能由本地
  single-pod canary替代。

因此，本文件当前不能作为 production-ready 或扩大灰度的批准。

## 2. 本地隔离环境

所有数据库验证都使用临时、运行后销毁的隔离容器，没有读取或修改当前灰度 Community、Provider
admission、fleet attestation 或 query gate：

```text
PostgreSQL: 17.10
pgvector:   0.8.5
image:      pgvector/pgvector@sha256:d2ef61f42ef767baa5a1475393303cc235bcd92febd9d7014eddb48b41f3bad0
dimensions: 2048
metric:     cosine
```

验证输出没有保存 API key、problem、title、summary、query vector 或完整 source identity。

## 3. 已执行证据

### 3.1 PostgreSQL 与 schema

```bash
. ./bin/activate-hermit
CARGO_INCREMENTAL=0 ./scripts/test-semantic-pgvector.sh
CARGO_INCREMENTAL=0 ./scripts/test-semantic-migrations.sh
```

结果：

- `semantic preflight` 返回 `ready=true`；
- `vector` / `halfvec`、cosine distance、2048 维 SQLx bind/decode 全部通过；
- 0057 populated upgrade、0058 additive migration、fresh `schema/schema.sql` 与 semantic schema drift 检查通过；
- `events_kind_not_semantic_graph_query_result` 为 VALID，virtual result Event 没有获得持久化资格；
- `semantic_query_upgrade_is_additive_and_index_disable_is_atomic` 通过；
- `semantic_pipeline_activates_only_a_complete_fenced_set` 通过。

### 3.2 Exact correctness 与权限线性化

在独立 PostgreSQL 容器中设置 `BUZZ_TEST_SEMANTIC_DATABASE_URL` 后执行：

```bash
CARGO_INCREMENTAL=0 cargo test -p buzz-db --lib \
  semantic_query::tests::final_confirmation_lock_orders_writer_first_and_permit_first_revocations \
  -- --nocapture

CARGO_INCREMENTAL=0 cargo test -p buzz-db --lib \
  semantic_query::tests::pgvector_exact_order_and_score_match_bruteforce_reference \
  -- --nocapture
```

结果：2 / 2 通过。

该权限测试证明两种线性化顺序：

- canonical writer 先持有锁并提交 ban 时，final confirmation 必须等待并观察到 ban；
- release permit 先取得 shared lock 时，ban writer 必须等待 permit transaction 释放。

它是数据库线性化合同证据，不单独证明真实 Relay 请求在 ban-first 场景下的 Provider 网络计数为零；后者
仍需在真实 canary 中同时观察 Provider egress 和 result release。

Exact correctness 测试证明小型 fixture 上 PostgreSQL `<=>` 排序与 fixed-point score 匹配进程内 brute-force
参考；它不是目标规模性能 benchmark。

### 3.3 Relay 安全与完整性合同

```bash
CARGO_INCREMENTAL=0 cargo test -p buzz-relay --lib semantic_ -- --nocapture
```

结果：47 / 47 通过，包含：

- NIP-11 capability 要求所有 readiness fence，并在 fleet check失败时保持 fail closed；
- semantic raw filter 只接受 closed envelope，拒绝混合、unknown field 与普通 kind `40912` 查询；
- NIP-98绑定 exact body、host、request 和 traversal identity；
- Provider query 只接受单个有界 batch，绑定 model / dimensions / contract 三重 fence；
- worker adapter 在 transport 前拒绝 `content_chunk`；
- oversized Content-Length、chunked response 与错误响应 body 均受硬上限约束；
- deadline保留固定 response tail；
- Hyperedge完整性、traversal预算、response原子 omission、postflight signer变化和最终 response cap 均
  fail closed；
- metric label 和错误码保持闭集、低基数且不含内容。

### 3.4 DB query / fleet 合同

```bash
CARGO_INCREMENTAL=0 cargo test -p buzz-db --lib semantic_query::tests
CARGO_INCREMENTAL=0 cargo test -p buzz-db --lib semantic_fleet::tests
```

结果：17 / 17 与 2 / 2 通过，包含 distance前 materialized roles / eligible集合、query三重fence、
generation与graph revision预留锁、current-epoch coverage、bodyless hydration、完整Hyperedge、snapshot-bound
traversal、closed fleet identity以及使用数据库时钟和shared Community lock的fleet readiness。§3.2所列两个
需要 PostgreSQL的测试另以显式隔离数据库环境运行；本节的无环境批次不能替代那份真实DB证据。

### 3.5 Desktop light / dark 视觉证据

Playwright使用Desktop E2E mock bridge渲染真实Project Context UI，在语义结果激活后验证root、route Edge与
terminal Coordinate标记，执行Fit paths并等待动画结束后截图：

| Theme | 路径 | SHA-256 |
|---|---|---|
| light | `desktop/test-results/semantic-d6/project-context-semantic-light.png` | `fe9634ab7f81e06dc27dd0e690a8633bbd04cbac76e038e2b17685fc6950103b` |
| dark | `desktop/test-results/semantic-d6/project-context-semantic-dark.png` | `bd505532b29de929a15980796a9811463e0aa246664739a2c88e5ba3e59ab8ea` |

两份hash不同，且两张图已经人工复核。此证据证明light / dark主题下的Desktop语义overlay视觉状态，不证明
真实Relay响应、Provider relevance、权限线性化、production LB或multi-pod运行资格。当前只有两张主题图；
不能将其表述为semantic-only / semantic+selection四种独立截图均已完成。

Carryforth集成分支在合入本提交后重新构建并运行同一两条E2E，产物均为`1131 × 951`，人工复核通过且
hash distinct：light为`221ee902d6c472c4f68ee404e4be3a45a15f4f544bf84db87f63cf2a5251cc3c`，dark为
`bd505532b29de929a15980796a9811463e0aa246664739a2c88e5ba3e59ab8ea`。上表继续保留基线提交
`507790180`的原始资格证据；这组复验只证明Carryforth集成没有破坏D6视觉状态，不替代本报告列出的
Provider、目标PostgreSQL、production LB或multi-pod阻断项。

## 4. Performance / soak 审计

仓库原先只有兼容性、migration与3-vector correctness工具，没有可重复的exact-query性能runner。本次新增：

```text
scripts/qualify-semantic-exact-query.sh
scripts/semantic-exact-query-qualification-setup.sql
scripts/semantic-exact-query-qualification-explain.sql
scripts/semantic-exact-query-qualification-pgbench.sql
```

执行：

```bash
. ./bin/activate-hermit
scripts/qualify-semantic-exact-query.sh
```

Runner固定使用PG17.10 + pgvector 0.8.5的临时容器、确定性2048维合成向量和独立schema；结束后销毁
容器，不连接canonical数据库。它拒绝query budget常量漂移，输出content-free `qualification.json`与完整
JSON EXPLAIN。每个source另有只用于benchmark采样的`scale_ordinal`：eligible source与distractor会共同进入
同一pre-gate窗口，随后distractor只能由Community、generation、current-head、authorization或eligibility
predicate排除，不能再被source identity或scale条件提前排除。默认本地规模：

| Profile | 可评分sources | fixture distractors | channels | recall / channel | iterations |
|---|---:|---:|---:|---:|---:|
| medium default | 2,000 | 5,000 | 4 | 64 | 20 |
| target default | 10,000 | 5,000 | 4 | 64 | 15 |
| target hard-cap | 10,000 | 5,000 | 9 | 256 | 10 |

其中9 channels与256 recall是合同hard cap；4 channels是明确的本地default代表profile，不表示请求合同
默认携带3个context。10,000 sources是可通过环境变量提高的本地qualification target，不冒充真实生产
Community容量。medium窗口实际纳入2,000个distractor；target窗口纳入全部5,000个distractor。

### 4.1 本地结果

本轮content-free产物：

```text
test-results/semantic-exact-query-qualification/d6-distractor-fix-20260811/qualification.json
SHA-256: a9946dfc6f323ca9ce8178a276e3d70ddbf2fe88d3bcfca7d757672e5ba73925
```

Predicate与distance行数：

| Profile | pre-gate | rejected by gate | eligible | distance rows | predicate proof |
|---|---:|---:|---:|---:|---|
| medium default | 4,000 | 2,000 | 2,000 | 8,000 | 通过 |
| target default | 15,000 | 5,000 | 10,000 | 40,000 | 通过 |
| target hard-cap | 15,000 | 5,000 | 10,000 | 90,000 | 通过 |
| target hard-cap forced spill | 15,000 | 5,000 | 10,000 | 90,000 | 通过 |

每个target profile的5,000个rejected rows按五类predicate各1,000个；medium profile各400个。Runner机械
断言每个distractor恰好只失败一个predicate、五类均非空、pre-gate等于eligible加rejected，并且distance
严格等于eligible乘channels。任一行数或predicate partition不一致都会使runner失败。

本轮latency结果：

| Profile | distance rows | EXPLAIN execution | p50 | p95 | p99 |
|---|---:|---:|---:|---:|---:|
| medium default | 8,000 | 73.960 ms | 84.553 ms | 101.421 ms | 101.954 ms |
| target default | 40,000 | 393.779 ms | 359.099 ms | 569.337 ms | 569.337 ms |
| target hard-cap | 90,000 | 955.215 ms | 810.945 ms | 1,063.150 ms | 1,063.150 ms |

所有EXPLAIN均证明materialized `pre_gate`与`rejected_by_gate`实际执行，只有materialized `eligible`进入
distance。4 MB work_mem下hard-cap观察到1,424 temp read blocks与2,091 temp written blocks；64 kB强制
spill profile观察到102,990 / 3,908 blocks，证明spill证据链可工作。所有读查询WAL records为0。

8秒、4 clients / 4 jobs的target-default短soak：

| 场景 | samples | p50 | p95 | p99 | measured TPS | max txn age | failures | sampled CPU peak |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| baseline | 81 | 393.905 ms | 478.638 ms | 615.313 ms | 10.125 | 385 ms | 0 | 404.75% |
| concurrent VACUUM ANALYZE | 73 | 412.438 ms | 737.714 ms | 777.570 ms | 9.125 | 459 ms | 0 | 404.86% |

VACUUM耗时146 ms；结束后synthetic表为15,000 live / 0 dead tuples、`vacuum_count=1`。1 ms
`statement_timeout`按预期取消target hard-cap query，返回非零状态，取消后与soak后都没有semantic query或
idle-in-transaction session残留。

这些数值只描述当前开发机上的synthetic exact distance kernel。样本较小，baseline与vacuum场景的差异
只能视为本轮观察值，不能外推为稳定的vacuum性能影响。

### 4.2 尚未关闭的目标环境门

Runner没有复制完整canonical graph roles / heads / hydration join，也没有部署LB、多个Relay Pod、worker或
Provider。因此以下门仍阻断production-ready：

- 在目标writer PG与实际Community数据上对生产`EXACT_SOURCE_SCORES_SQL`执行同样的
  `EXPLAIN (ANALYZE, BUFFERS, WAL, SETTINGS)`；
- 由Operator冻结数字SLO，再判断上述p95 / p99、CPU、buffers、spill和transaction age是否通过；
- 代表性高出度graph roles、default / hard-cap完整Stage C、statement cancellation与vacuum影响；
- worker + interactive query、多Pod admission、Provider contention和更长时间soak；
- 真实负载下的429、Provider wait、错误闭集、DB资源与恢复结果。

目标环境启用前必须补齐或以部署侧等价工具执行以下门：

| 门 | 最小证据 |
|---|---|
| Predicate before distance | 生产SQL的EXPLAIN证明distance节点只消费materialized `eligible` rows |
| Default / hard-cap latency | p50 / p95 / p99、扫描行数、CPU、buffers、temp spill |
| Deadline | query / statement timeout取消结果、无超时后签名、无长事务残留 |
| Concurrency | worker + interactive query、公平 admission、多 Pod并发无永久饥饿 |
| Vacuum | 最大 snapshot transaction age、dead tuple / vacuum delay前后对照 |
| Soak | 持续时间、请求量、错误闭集、429、Provider wait、DB资源与恢复结果 |

不得用本报告中的3-vector correctness或synthetic runner宣称目标Community与多Pod门已经通过。

## 5. Security / revoke 待完成的真实门

代码级合同已经覆盖 capability、authorization fence、release fence和内容边界。本轮 local single-pod
已机械覆盖下列第1、6项的 feature-off / canonical-read证据，并检查了第7项中的资格产物内容边界；
实际部署的权限撤销、运行日志审计、UI清理和多Pod边界仍须继续验证：

1. NIP-11 不广告时 Desktop不调用 semantic Native command，Relay Provider query egress为零；
2. ban / membership revoke先于 Relay final egress线性化时，Provider query egress为零；
3. revoke先于 result release线性化时，不返回签名 result；
4. 已取得 permit后到达的 revoke等待 permit完成，记录明确的线性化顺序；
5. capability off、403、trusted identity变化或 Community switch后 Desktop立即清除 active overlay；
6. canonical Inspector继续读取 verified source，不使用 semantic preview作为 fallback；
7. 日志和资格产物不出现 problem、overview、vector、API key或完整 source identity。

## 6. Rollback 门

本地已经证明 schema additive、Foundation/query gate可分离以及 index disable会原子清 query gate，并已按
以下顺序完成一次 local single-pod content-free演练。实际部署仍须重复相同流程：

```bash
buzz-admin semantic query-disable
buzz-admin semantic fleet-revoke
buzz-admin semantic query-readiness
```

随后验证：

- NIP-11停止广告 `buzz-project-context-semantic-query-http`；
- 新 Desktop query不发送 problem；
- Provider query egress停止；
- ordinary Project Context与canonical source reads继续可用；
- Foundation worker和active generation不因 query rollback被删除或回退；
- 关闭 deployment master后，旧 Pod不能在 query gate开启时重新进入负载均衡。

本轮 local演练已证明前五项；deployment master、实际负载均衡与旧Pod移除不在本地环境内，仍未完成。

## 7. D6 完成清单

| 资格项 | 当前状态 | 证据归属 |
|---|---|---|
| Desktop / Native / TS / E2E矩阵 | 通过：fmt / clippy / typecheck / lint；semantic model 6 / 6；Playwright 2 / 2 | Desktop质量门 |
| light / dark语义路径截图 | 通过，2张且hash distinct | 本报告 §3.5 |
| semantic-only / semantic+selection独立视觉基线 | 未要求则不阻断；如作为发布门则待补 | Desktop截图 |
| PostgreSQL / pgvector / migration | 通过 | 本报告 §3.1 |
| exact correctness | 通过，小型fixture | 本报告 §3.2 |
| permission / release DB线性化 | 通过，代码级 | 本报告 §3.2 |
| Relay semantic安全合同 | 通过，47项 | 本报告 §3.3 |
| 真实 Volcengine relevance与四类floor | 部分完成：9 / 9真实请求成功、返回分数0违规；known-negative仍返回6 roots / 12 paths，质量未通过 | 本报告 §8 |
| synthetic EXPLAIN / latency / cancellation / vacuum | 通过，本地10k target | 本报告 §4.1 |
| 目标PG canonical EXPLAIN /冻结SLO / vacuum | 阻断，尚无目标证据 | 本报告 §4.2 |
| 多Pod / Provider并发与长soak | 阻断，尚无部署证据 | Deployment qualification |
| local single-pod fleet attestation | 通过：受控窗口1 routed / 1 attested | 本报告 §8.1 |
| local single-pod首个Community query smoke | 通过：Desktop native 9 / 9返回200 | 本报告 §8.2 |
| source / revision stale smoke | 待完成 | Desktop + runtime canary |
| local feature-off / rollback演练 | 通过（local single-pod）：gate / capability off、fleet revoked、zero Provider reservation、canonical read继续、Foundation继续 | 本报告 §8.5 |
| production LB inventory / multi-pod qualification | 阻断，尚无部署证据 | Deployment qualification |

### 7.1 已合并的 local content-free 记录

以下记录来自实际D6输出，没有用代码测试或synthetic benchmark推导：

```text
Local single-pod runtime canary
├── Community: 现有本地开发 Community（身份不写入报告）
├── active generation / model / dimensions: active / configured Volcengine model / 2048
├── eligible sources / ready heads / failed jobs: 80 / 80 / 0
├── routed Relay instances / attested instances: 1 / 1
├── query readiness / NIP-11 capability: 受控窗口 ready / HTTP on、WS off；窗口后 HTTP off
└── canary query count / status / content-free latency: 9 / 9 HTTP 200；2,419..7,748 ms，median 4,541 ms

Real Volcengine relevance
├── English problem-only: 6 roots / 12 paths；6,510 ms
├── Chinese problem-only: 6 roots / 12 paths；3,588 ms
├── explicit initial Coordinate: accepted 1 / not-in-graph 0 / omitted 0；7 roots / 12 paths
├── same problem with different context Coordinates: accepted 1 / 1；root Jaccard 0.333；path Jaccard 0.143
├── terminal lifecycle filter: automatic Coordinate roots terminal 1；continued targets terminal 7；violations 0
└── observed base / relation / target / transition floors: 54 / 119 / 109 / 109 observations；all violations 0

Local feature-off / rollback
├── query-disable result: query_enabled=false；semantic_index_continues=true
├── NIP-11 capability after disable: HTTP semantic capability off
├── Provider query egress after disable: Native query闭合unsupported；physical gate / interactive admission摘要均不变
├── fleet revoke / readiness result: revoked=true
├── ordinary Project Context / canonical reads: disable后Incident read success / 1 Edge / 2,050 ms
└── final local gate / fleet state: query gate off / fleet revoked / Foundation index ready
```

这些槽位只用于local single-pod证据。即使全部填写并通过，也不能把production LB inventory、multi-pod
admission / Provider contention或长soak标为完成。

只有所有阻断项都有可审计证据后，才可以把本报告状态改为“D6通过”；在此之前保持单 Community、可立即
`query-disable` 的灰度边界。

## 8. 真实 Volcengine relevance / canary

### 8.1 身份边界与受控启用

本轮通过 Desktop / Tauri 的正常身份边界读取已配置身份：
`resolve_persisted_identity` → `AppState::signing_keys`。只验证调用者 `signable=true` 且为当前
Community member；没有导出、打印或写入私钥，也没有复用 Relay signer、修改 membership 或把凭据放进
环境变量。资格报告不保存完整公钥或其他调用身份。

用户已明确允许本轮把非敏感合成 problem 发送给配置的 Volcengine Provider。启用前的 content-free
状态如下：

| 观测 | 结果 |
|---|---:|
| active Project Context Edges | 21 |
| current Context Document bindings | 24 |
| eligible sources / complete current generation heads | 80 / 80 |
| failed semantic jobs | 0 |
| vector dimensions | 2048 |
| `database_ready` / `base_enable_ready` / `active_generation_ready` | true / true / true |
| `non_queryable_current_heads` | 0 |
| routed / attested Relay instances | 1 / 1 |

只在 matrix 执行窗口内显式执行 `query-enable --acknowledge-problem-egress`；窗口内 NIP-11 只广告 HTTP
semantic capability，不广告 WS capability。整个过程使用相同的 Native validation、workspace capture、
NIP-98 exact-body signing、Relay handler、SDK result verification 与 response parser，不是直接调用 Provider
或绕过 Desktop 的测试客户端。

### 8.2 Content-free 查询矩阵

评测只使用非敏感合成问题。下表不保存 problem、title、summary、vector、API key、完整 Coordinate / source
identity 或原始响应；所有 9 项返回 HTTP 200，completion reason 均为 `budget_exhausted`：

| Case | latency | roots / paths | channels requested / executed | 关键观测 |
|---|---:|---:|---:|---|
| English problem-only | 6,510 ms | 6 / 12 | 1 / 1 | lifecycle违规0；environment gain 0 |
| Chinese problem-only | 3,588 ms | 6 / 12 | 1 / 1 | lifecycle违规0；environment gain 0 |
| explicit initial Coordinate | 7,748 ms | 7 / 12 | 1 / 1 | accepted 1；not-in-graph 0；omitted 0 |
| context environment：Work | 4,541 ms | 6 / 12 | 2 / 2 | accepted context 1；max gain 249,815 |
| context environment：disconnected Requirement | 5,021 ms | 6 / 12 | 2 / 2 | accepted context 1；max gain 111,666 |
| non-terminal | 3,532 ms | 6 / 12 | 1 / 1 | automatic Coordinate roots active 4；targets active 12；违规0 |
| terminal-only | 2,419 ms | 6 / 4 | 1 / 1 | automatic Coordinate roots terminal 1；targets terminal 7；违规0 |
| missing context Coordinate | 3,757 ms | 6 / 12 | 2 / 1 | accepted 0；omitted 1 |
| known-negative | 4,955 ms | 6 / 12 | 1 / 1 | 未被拒绝，形成明确的质量失败证据 |

`terminal-only` 的 6 个 roots 中另有 5 个 active Project Document relation roots。lifecycle selector按合同约束
automatic Coordinate roots与continued Coordinate targets，不把 current relation Document 的 active
lifecycle错误地当成 Coordinate lifecycle；因此该 case 的 selector违规数仍为0。

端到端 latency 范围为 2,419..7,748 ms，median 为 4,541 ms。样本只有9个，且没有冻结交互SLO，不能据此
宣称 latency qualification通过。

### 8.3 不同 environment 的路径差异

两个 environment 使用同一个 problem，分别提供一个 current-head Work 与一个不和该 Work 共享 active Edge
的 current-head Requirement。两者均生成并执行 Q0 + Qi，且各自 `accepted_context_coordinates=1`：

| 集合 | Work | Requirement | intersection | union | Jaccard | 各自独有 |
|---|---:|---:|---:|---:|---:|---:|
| roots | 6 | 6 | 3 | 9 | 0.333 | 3 / 3 |
| paths | 12 | 12 | 3 | 21 | 0.143 | 9 / 9 |

两组还共享3个 `problem_neutral` roots。该结果证明 conditioned environment 会改变候选 root与path集合，同时
保留一部分 problem-neutral结果；它不证明变化后的路径在业务语义上正确。当前开发 Community 没有
environment-specific expected-path labels，因此不能把集合差异本身当成 relevance 通过。

### 8.4 四类 floor 与 known-negative 结论

当前 ranking contract 的 provisional floor 与本次所有返回观测如下：

| 分数角色 | floor | observations | observed min | violations |
|---|---:|---:|---:|---:|
| automatic root ProblemScore | 550,000 | 54 | 618,452 | 0 |
| relation | 500,000 | 119 | 501,743 | 0 |
| target | 500,000 | 109 | 502,274 | 0 |
| transition | 500,000 | 109 | 508,733 | 0 |

这证明真实 Provider空间中返回结果严格执行了四类 absolute floor，但 known-negative 仍返回 6 个 roots与
12 条 paths。换句话说，threshold enforcement通过，quality calibration没有通过；不能因为违规数为0就把
四类 floor 标为 qualified。

当前 Community 是既有本地开发数据，没有 relevance labels、positive / negative source pairs 或预期路径
标注。下一步必须创建明确非敏感、可重放的 labeled Community fixture，覆盖实现计划 §17.8 的两组环境、
不相关环境、纯 Context Document 命中、多文档 Edge、title-only、terminal source 与 disconnected relevant
component；然后基于中英文正例召回、known-negative误召回和完整分数分布调校权重 / floor并重跑。

### 8.5 Canonical read 与回滚状态

在 query-enable 窗口内，从已验签结果对应的 explicit initial Coordinate 执行 ordinary shipping
Project Context `Incident` read，返回 HTTP 200、1条 Edge，端到端 1,614 ms；读取结果正文被丢弃。这证明
semantic结果没有替代 canonical source read，也没有让Inspector依赖 semantic preview。

matrix完成后执行 `query-disable` 与 `fleet-revoke`，当前状态为：

| 回滚观测 | 结果 |
|---|---:|
| `query_enabled` | false |
| fleet assertion | revoked |
| NIP-11 HTTP semantic capability | off |
| `semantic_index_continues` | true |
| Foundation index readiness | true |

随后使用相同的正常 Desktop 身份和 shipping command 执行 gate-off 回归：

| Gate-off回归 | 结果 |
|---|---:|
| ordinary canonical `Incident` read | success；1条 Edge；2,050 ms |
| Native semantic query | fail closed；`unsupported`；295 ms |
| Provider physical gate摘要 before / after | 相同 |
| interactive admission摘要 before / after | 相同 |

因此 disable 后的 semantic query 在 Provider slot reservation前停止，零新增预约、零 Provider egress；
ordinary Project Context read仍可用，Foundation index仍ready。本项只证明 local single-pod feature-off，
不替代 production LB中旧Pod移除、multi-pod fleet变化或deployment master关闭演练。
