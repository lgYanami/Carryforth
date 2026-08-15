# Project Context 统一语义计算资格记录

> 状态：U0–U7 已通过；第一阶段统一语义计算资格完成；真实Provider canary外部阻断
>
> 日期：2026-08-16
>
> 实现计划：
> [Project Context 统一语义计算实现计划](project-context-unified-semantic-computation-implementation-plan.md)

## 1. 阶段状态

| 阶段 | 状态 | 结论 |
| --- | --- | --- |
| U0 设计与差分门 | 通过 | 历史 v1 oracle 与独立 Phase 1 gate 可在同一工作树运行 |
| U1 共同 input/fence/vector | 通过 | 三种 closed input 与两类旧 wrapper 已委托共同类型；exact generation 只由 writer DB ticket 绑定 |
| U2 共同 Provider encoder | 通过 | Coordinate 与 graph 兼容 adapter 委托同一 bounded batch primitive；无 retry/fallback/额外调用 |
| U3 one-hop tagged family | 通过 | 两个 variant 只切换到 closed explicit-source facade；原有同一 exact SQL 与全部 policy 保持不变 |
| U4 whole-graph Coordinate | 通过 | Coordinate v1 接入共同 scorer；同 snapshot 精确差分与10k性能门通过 |
| U5 bounded complete path | 通过 | Q0/Qi closed bundle、root/relation/target scorer与path retention零行为迁移通过 |
| U6 默认切换与 legacy 收口 | 通过 | 四个operation默认Migrated；新fleet digest拒绝旧profile；rollback源保留到2026-09-16 |
| U7 最终资格与文档关闭 | 通过（1项外部阻断） | deterministic、service DB、target-scale、全量单元与文档关闭；真实Provider配置缺失 |

## 2. U0 证据

U0 保留了冻结的历史 manifest 及其 SHA-256：

~~~text
e7b18cdba9c40fa941a6a70fd8beb2629ecc4232dcc5d94316edbaf4fdae097e
~~~

历史 runner 只复核 `e8f26d6e65..ab395ff6f` 的冻结交付，不会把后续 Phase 1 生产代码误判成
历史交付漂移；原有 production freeze allowlist 没有扩大。

新增的 Phase 1 differential fixture SHA-256 为：

~~~text
52a062ad800ae0f8503fc9748a4be45e487fb964b551977c41666ccc3591ab19
~~~

它冻结：

- 四个逻辑 operation / 三个 surface；
- 当前 ordered input/vector bundle；
- legacy/new 必须消费同一次 Provider encoding 与同一 RR snapshot；
- 每个 operation 的 typed normalized result 与 closed error comparator；
- production 默认 route 仍是 legacy，request 内 mismatch 不 fallback。

本阶段实际通过：

~~~text
SEMANTIC_COMPATIBILITY_ENFORCE_FREEZE_DIFF=1 \
  ./scripts/check-semantic-retrieval-compatibility-baseline.sh manifest-only
./scripts/check-semantic-retrieval-computation.sh deterministic
./scripts/check-semantic-retrieval-computation.sh all
cargo fmt --all -- --check
git diff --check
~~~

完整 Phase 1 gate 已覆盖 `buzz-semantic-query`、`buzz-db` 与 `buzz-relay` 的相关定向测试和 production
`cargo check`。U0 没有修改 wire、Event kind、SDK、CLI、schema、migration、semantic index 或 canonical
Project Context 图。

## 3. 资格边界

### 3.1 U1 实现结论

U1 已交付以下 production 类型边界：

- `SemanticQueryInput` / `SemanticQueryInputBundle` 统一承载 Coordinate、Q0、Qi 的 exact UTF-8、input
  digest、operation contract digest 与有序 batch identity；各 closed builder 仍生成原有字节，不允许跨合同
  重新解释；
- `SemanticModelSpaceFences` 只描述 generation contract/model/dimensions 可比较空间，不携带 operation
  template，也不伪造 active generation identity；
- `ProviderEncodedSemanticInput` / bundle 绑定 Provider 输出与 exact input/model space，但不携带 generation
  UUID；
- `SemanticGenerationKey` 使用 host-derived `CommunityId + generation UUID`，由 writer DB ticket 创建
  `GenerationBoundQueryVector`；相同 generation UUID 出现在不同 Community 时仍 fail closed；
- 旧 `EncodedCoordinateSearchQuery`、`EncodedSemanticQuery`、`SemanticCoordinateSearchVector` 与
  `SemanticExactQueryVector` 保持为兼容 wrapper。Coordinate 仍使用独立 template/digest，Q0/Qi 仍使用
  graph-query template/digest；
- Coordinate 与 graph operation-facing DB adapter 都会再次验证 closed input kind、operation contract、
  model space、tenant/generation 与 vector shape，授权与 currentness 没有移入通用数学类型。

U1 没有修改 Provider transport、调用批次、admission、retry、permit、RR snapshot、release、ranking、scope、
wire、Event kind、SDK、CLI、schema、migration、semantic index 或 canonical Project Context 图。

### 3.2 U1 执行证据

以下门在 U1 最终工作树通过：

~~~text
cargo test -p buzz-semantic-query --lib                         # 47 passed
cargo test -p buzz-semantic-query --test compatibility_baseline # 1 passed
cargo test -p buzz-semantic-query --test computation_differential # 2 passed
cargo test -p buzz-db --lib semantic_                            # 31 passed, 4 ignored
cargo test -p buzz-db --lib coordinate_search                    # 3 passed, 1 ignored
cargo test -p buzz-db --lib one_hop                              # 2 passed
cargo test -p buzz-relay --lib semantic_                         # 72 passed
cargo test -p buzz-relay --lib coordinate_search                 # 4 passed
cargo test -p buzz-relay --lib one_hop                           # 6 passed
cargo clippy -p buzz-semantic-query -p buzz-db -p buzz-relay --all-targets -- -D warnings
./scripts/check-semantic-retrieval-computation.sh all
git diff --check
~~~

历史 manifest 与 Phase 1 differential manifest 均未改写；aggregate computation gate 继续输出
`semantic retrieval computation gate passed (all)`。

### 3.3 尚未宣称的能力

U1 只完成共同 production types 与 DB generation binder；共同 Provider encoder 与共同 exact scorer 尚未
切换 production operation。它不代表可靠性 runtime、资源治理或 production SLO 已交付。

真实 Provider canary 仍缺少受支持的 `BUZZ_SEMANTIC_*` 配置，未在 U0–U3 运行；不得挪用 `LLM_*`。

## 4. U2 共同 Provider encoder

U2 新增 `SemanticInputEncoder`，其唯一输入/输出是一个已验证的 `SemanticQueryInputBundle` 与同序
`ProviderEncodedSemanticInputBundle`。生产 `VolcengineSemanticProvider` 在这个 primitive 内：

1. 在网络前复核 closed bundle 与 model space；
2. 精确按 bundle 顺序发送 UTF-8 texts；
3. 恰好发出一个 Provider batch；
4. 继续使用既有 response body cap、HTTP status 分类、model/count/index/dimension/finite/non-zero 验证；
5. 将完整 batch 一次性绑定到 input digest；任一成员失败即整批失败。

现有 `encode_coordinate_search` 与 `encode_queries` API 保留，并作为 thin compatibility adapter 委托共同
primitive。Coordinate、两个 one-hop variant 与完整路径的调用方、admission、deadline、retry 与 snapshot
编排没有改变；单请求失败不会自动 fallback 或再次请求 Provider。

新增 `ByteDeterministicSemanticInputEncoder` 只作为无 transport 的差分 seam：向量只由 exact Provider bytes
与 model space 决定，request/channel 仅保留为结果绑定。原 `DeterministicFakeQueryEncoder` 的 channel-aware
算法与历史 manifest 保持不变。

U2 新增/更新后的实际证据：

~~~text
cargo test -p buzz-semantic-query --lib                         # 48 passed
cargo test -p buzz-semantic-query --test compatibility_baseline # 1 passed
cargo test -p buzz-semantic-query --test computation_differential # 2 passed
cargo test -p buzz-relay --lib semantic_                         # 73 passed
cargo test -p buzz-relay --lib coordinate_search                 # 4 passed
cargo test -p buzz-relay --lib one_hop                           # 6 passed
cargo clippy -p buzz-semantic-query -p buzz-relay --all-targets -- -D warnings
./scripts/check-semantic-retrieval-computation.sh all
git diff --check
~~~

HTTP fake Provider 直接断言了：Coordinate 为一次单输入调用；one-hop 为一次 Q0 调用；完整路径 Q0+Qi 为一次
有序 batch 调用；Provider 乱序 datum 仍按 index 恢复，exact text 与 input digest 没有变化。

## 5. U3 one-hop tagged family

U3 按 `Edge → member Coordinate`、`Coordinate → incident Edge` 的顺序，将两个 one-hop variant 接入
`SemanticExactExplicitSourceScope` 与 `score_explicit_source_scope_exact`。这个 facade 只接受 DB 结构读取已经
解析出的两种 closed source set，并分别保持 4096 Coordinate / 2048 relation binding 的既有上限。

审查确认迁移前两个 variant 已经调用完整路径共用的 `query_exact_source_scores`，因此本阶段没有第二份 SQL、
没有 score 算法切换，也没有必要制造无实际差异的 runtime route。facade 继续固定：

- `AllCurrent` direct Q0 exact score；
- 无 floor、coherence、context gain、Edge grouping 或 public projection；
- Relay 不可构造 scope，只有 DB operation method 能选择 closed variant；
- Incident Edge 的 Document grouping/max-score、Edge canonical tie、preview/coverage/omission/truncation仍在
  `scoped_search`；
- Edge Coordinate 的完整 Hyperedge、membership proof、Coordinate tie、preview/coverage/omission/truncation
  仍在 `scoped_search`。

U3 最终证据：

~~~text
cargo test -p buzz-db --lib one_hop                              # 2 passed
cargo test -p buzz-db --lib semantic_                            # 31 passed, 4 ignored
cargo test -p buzz-relay --lib one_hop                           # 6 passed
cargo test -p buzz-semantic-query --test compatibility_baseline # 1 passed
cargo test -p buzz-semantic-query --test computation_differential # 2 passed
cargo clippy -p buzz-db -p buzz-relay --all-targets -- -D warnings
./scripts/check-semantic-retrieval-computation.sh all
git diff --check
~~~

其中 disposable-pgvector one-hop fixture 继续验证两个 variant 的 direct score、完整 scope、canonical hydration
与结果形状；40914 Relay/SDK closed tagged family 回归继续通过。U3 没有修改 Provider、snapshot/release、wire、
Event kind、capability、schema 或 migration。

## 6. U4 whole-graph Coordinate

U4 新增 `GlobalGraphCoordinates` closed scope，并让 Coordinate v1 保持独立 query template/digest 与兼容
wrapper后可进入共同 `GenerationBoundQueryVector` 和 scorer facade。共享内核只保留一份：

- authorized reader/project、active generation与model-space验证；
- current source head、active unit set、overview unit与exact-current embedding join；
- exact cosine、finite检查与fixed-point `Score`量化。

graph recall与global Coordinate使用由上述相同静态片段组成的两个closed physical plan。前者保留graph role
evidence、raw-distance per-channel rank与late hydration；后者只物化active Edge中去重的Coordinate并按
`Score DESC, ProjectContextCoordinate::Ord ASC`先取K+1。Relay与caller不能选择SQL、scope或ordering。

同一disposable pgvector RR事务中的legacy/shared差分覆盖：active Edge、multi-Edge去重、relation-only
Document排除、Document兼任Coordinate、terminal、missing head、K+1，以及raw distance不同但fixed score
相同的canonical tie。limit 4与5的typed batch均精确相等。旧Coordinate SQL只供acceptance与profile
rollback窗口使用，不存在请求内fallback或第二次Provider调用。

当前compiled route matrix按`edge-member/coordinate-incident/whole-coordinate/full-path`依次为
`migrated/migrated/legacy/legacy`。profile字节纳入fleet runtime digest；runtime contract已显式升级为
`semantic-query-http-runtime-20260816-u4`，digest为
`9601b1014e85e16d0eaa8db6146e168653353e489646478380234fc4f56565c8`。因此U4 production Coordinate
仍返回legacy路径；migrated scorer只由同snapshot资格入口执行，普通请求不能选择compare或自动fallback。

10k目标规模最终测量：

~~~text
target indexed Coordinates:       10,000
active Coordinates missing head:   1,000
deleted-edge indexed distractors:  5,000
graph-external distractors:         5,000
dimensions:                         2,048
limit / observed:                  32 / 33

shared sequential:                 p50 285ms / p95 412ms
same-run legacy reference:         p50 311ms / p95 411ms
shared EXPLAIN:                     plan 40.942ms / execute 286.716ms
legacy EXPLAIN:                     plan 33.327ms / execute 290.538ms
temp read/write blocks:             0 / 0 (both plans)
4-client 8s soak:                   101 completed / 0 errors / p50 310ms
~~~

最终执行证据：

~~~text
just semantic-migration-test
just coordinate-search-qualification
cargo test -p buzz-db --lib                         # 179 passed, 227 ignored
cargo clippy -p buzz-db -p buzz-relay --all-targets -- -D warnings
just semantic-retrieval-computation
git diff --check
~~~

aggregate gate继续通过49个`buzz-semantic-query`测试、31个相关DB测试、73个Relay语义测试及三个surface
定向回归。Coordinate的40913 wire/capability/gate、Provider单调用、one-shot RR/release、public result和错误
合同未变；one-hop与完整路径回归未发生policy漂移。真实Provider canary仍因缺少受支持的
`BUZZ_SEMANTIC_*`配置未运行。

## 7. U5 bounded complete path

U5 将完整路径的有序Q0/Qi结果收口为`SemanticGraphQueryVectorBundle`。该类型只接受：Q0首项、其后按
canonical Coordinate严格排序的Qi、graph query contract digest，以及同一request、Community/generation、
model space和Provider batch。traversal channel binding还会在DB侧逐项验证`channel_id`对应的Qi Coordinate，
不能交换Q0或两个context分支。

迁移保留两条closed adapter：

- compiled `Legacy`继续执行历史graph encoder与`SemanticExactQueryVector` binder，再无损转换为bundle；
- `Migrated`直接执行共同`SemanticInputEncoder`并由writer-DB ticket绑定共同Provider bundle。

两条adapter之后共用同一个授权/current-head/exact-score SQL和既有policy owner。普通请求仍由compiled
`migrated/migrated/legacy/legacy` profile选择legacy完整路径adapter；不存在请求字段、自动fallback、双Provider
调用或第二个snapshot。fleet runtime contract/digest因此仍保持U4值。

差分证据分为三层：

1. pure binder：同一Provider Q0+Qi结果经历史wrapper和共同bundle后逐字段相等，并拒绝错误Q0/Qi绑定；
2. disposable pgvector：同一RR snapshot中历史exact入口与共同graph scorer的root score rows完全相等；共同
   bundle继续完成relation Document与完整Hyperedge target排名，保留原coherence、relation/target/transition
   floor；
3. traversal/packing：compatibility/common bundle在同一2-hop synthetic backend得到完全相同的roots、paths、
   materialization、retention、truncation与completion；既有1–6 hop pack/sign/SDK验证以及40912回归继续通过。

最终执行证据：

~~~text
just semantic-migration-test
just semantic-retrieval-computation
cargo clippy -p buzz-semantic-query -p buzz-db -p buzz-relay --all-targets -- -D warnings
cargo check -p buzz-semantic-query -p buzz-db -p buzz-relay
git diff --check
~~~

aggregate gate通过49个`buzz-semantic-query`测试、32个相关DB测试（4 ignored）和74个Relay语义测试；
service-backed disposable pgvector fixture额外实际执行了完整路径root、relation和target scorer。完整路径现有
generation/context churn retry、Provider reservation/final confirm、traversal permit、唯一RR snapshot、budget、
path identity、coverage/packing与`expected_snapshot: None` release语义均未修改。真实Provider canary仍因缺少
受支持的`BUZZ_SEMANTIC_*`配置未运行。

## 8. U6 默认切换与 legacy 收口

U6把compiled route profile从`migrated/migrated/legacy/legacy`切换为四项全`migrated`。全图Coordinate和
bounded complete path因此默认消费U4/U5已经差分通过的共同计算路径；40912/40913/40914、三个HTTP extension、
capability、Community gate、CLI、SDK、ranking、budget、result和错误合同均未改。runtime contract升级为
`semantic-query-http-runtime-20260816-u6`，compiled digest为：

~~~text
e49d7ae9e69a2818a9ce9c061443a4441d332c86a3f8b46824b147a5da716f40
~~~

fleet回归明确证明旧U4 digest `9601b101…`形成的inventory即使内部同质，也不能通过U6 binary的compiled-runtime
验证；因此旧/新profile不能在普通attested fleet中混跑。此次本地资格没有部署或修改任何真实fleet/gate，不能
冒充production cutover。

legacy Coordinate SQL与完整路径compatibility adapter保留为profile rollback源，删除日期为`2026-09-16`。
它们不受request、动态配置或失败路径选择；rollback仍要求gate/capability off、drain、整fleet同profile部署与
重新attestation，不是即时flag或单pod回退。

默认切流后的执行证据：

~~~text
just semantic-migration-test
just semantic-retrieval-computation
just coordinate-search-qualification
just semantic-query-qualification
cargo check -p buzz-db -p buzz-relay
~~~

结果：

- disposable migration/service矩阵全部通过，包含compiled Coordinate route、one-hop和完整路径
  root/relation/target的同snapshot证据；
- aggregate继续通过49个pure contract、32个DB（4 ignored）和74个Relay语义测试；
- 10k Coordinate shared EXPLAIN 290.774ms、legacy同运行参考266.037ms，均无temp spill；4-client 8秒soak完成
  98次、0 error；
- graph exact kernel在10k×4 channel的EXPLAIN为381.208ms，在10k×9 hard-cap为822.103ms；statement
  cancellation无残留session，soak无失败事务；
- 以上为本地合成测量，未冻结生产SLO。真实Provider canary仍留给U7，并且只接受受支持的
  `BUZZ_SEMANTIC_*`配置。

## 9. U7 最终资格与阶段关闭

U7在最终四项全`Migrated` compiled profile上重新执行了分层资格：

~~~text
just semantic-retrieval-computation
just semantic-test
just coordinate-search-qualification
just semantic-query-qualification
cargo clippy -p buzz-semantic-query -p buzz-db -p buzz-relay --all-targets -- -D warnings
just test-unit
just ci
~~~

结果摘要：

- compatibility v1与Phase 1 differential两个tracked manifest/hash均保持原值，四operation/三surface的
  deterministic gate通过；
- `semantic-test`验证PostgreSQL 17.10、pgvector 0.8.5、fresh/upgrade schema、Coordinate/one-hop/完整路径
  scorer与fleet release矩阵；
- 两个target-scale runner均通过，详细content-free数值记录在§8；它们是本地测量而非生产SLO；
- feature/process master/Community gate/capability/fleet关闭路径继续在Provider前fail closed；旧U4 runtime
  inventory不能通过U6 compiled runtime验证；
- `just test-unit`的28组全仓单元套件全部通过；strict Clippy、format、diff与最终`just ci`通过；
- 三个公开surface的Event kind、extension、capability、CLI/SDK、query text、ranking、budget、result、错误与
  release语义未发生未批准变化。

真实Provider canary未运行。资格前只检查配置是否存在，不读取或输出值；当前进程与`.env`都没有
`BUZZ_SEMANTIC_API_KEY`、`BUZZ_SEMANTIC_BASE_URL`或`BUZZ_SEMANTIC_REQUEST_MODEL`。按照计划，这是一项明确
外部阻断，不得由`LLM_API_KEY`、`LLM_BASE_URL`或`LLM_MODEL`替代。整个U7没有打开Community semantic gate，
Provider egress为0。

## 10. 最终结论与后续边界

第一阶段统一语义计算资格通过：四个逻辑operation共同使用closed input/vector、一次bounded Provider encoding
能力与current-head exact scorer，并继续保持各自closed scope、policy和public result。legacy Coordinate SQL与
完整路径compatibility adapter只作为profile rollback源保留至`2026-09-16`；若使用，必须按gate off、drain、
整fleet部署与重新attestation流程执行，不能动态fallback。

上位spec、历史baseline、实现计划与Stage TODO已同步。中英文current-status仍准确描述公开能力为实验性、
受门控且非production-ready，因此没有为了内部重构改写产品状态。下一阶段是**单独设计统一可靠性运行时**；
retry、backoff、circuit、snapshot recovery与统一错误生命周期尚未交付。统一资源治理与跨operation production
容量资格仍排在其后。
