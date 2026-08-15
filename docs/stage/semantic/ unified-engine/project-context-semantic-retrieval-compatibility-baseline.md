# Project Context 语义检索兼容基线记录

> 状态：兼容基线已冻结；真实 Provider canary 未运行；作为历史 v1 oracle 保留
>
> 日期：2026-08-15
>
> 代码起点：feat/semantic-engine，e8f26d6e65
>
> 交付计划：
> [语义检索兼容基线交付计划](project-context-semantic-retrieval-compatibility-baseline-plan.md)
>
> 上位规范：
> [Project Context 统一语义检索引擎规范](project-context-unified-semantic-retrieval-engine-spec.md)

## 1. 当前结论

本记录冻结四个逻辑 operation / 三个公开 surface 的现有兼容边界，为后续统一语义计算提供
characterization oracle。

本轮已经完成：

- operation、surface、执行生命周期和 owner 盘点；
- 一份跨四个 operation 的非敏感合成 manifest；
- Coordinate 独立输入、one-hop/完整路径共享 Q0、完整路径 Qi 的精确 bytes/digest；
- 固定 16 维 fake vector bundle 的 digest；
- ranking、budget、fleet 和 query contract digest；
- direct score、environment、candidate、relation、target、transition 和 path score golden；
- manifest hash、结构检查和统一执行入口；
- pure、wire、SDK、Relay、CLI 与 default-feature production check；
- disposable PostgreSQL 17.10 / pgvector 0.8.5 的迁移、Coordinate 和 one-hop scoped search；
- Coordinate 10k 与完整路径 exact kernel 的本地 content-free 资格；
- production-path freeze diff、严格 Clippy、仓库 unit gate 与完整 `just ci`。

明确未完成：

- 本轮没有建立或执行新的三-surface真实 Provider canary runner。预检确认当前进程和 `.env` 均没有
  `BUZZ_SEMANTIC_API_KEY`、`BUZZ_SEMANTIC_BASE_URL`、`BUZZ_SEMANTIC_REQUEST_MODEL`；现有 `LLM_*`
  配置不属于受支持的 2048 维 embedding 合同，未被挪用；
- 三个 surface 在同一故障注入下的完整 disconnect/cancel 与 Provider/DB work-stop 对照仍是 known
  deviation；
- 多Pod、长期Provider故障、公平性和production SLO仍不在本阶段关闭。

真实 Provider 未运行是计划允许的外部资格阻断，不否定 deterministic 与真实数据库兼容基线。本文不是
production qualification。统一语义计算的迁移状态由独立实现计划与资格记录维护；本文只保留开始迁移前
已经冻结的历史 v1 oracle，不随新实现改写。

## 2. Source-of-truth 矩阵

| 逻辑 operation | 公开 surface | result kind | query input | 生命周期 | 主要 owner |
| --- | --- | ---: | --- | --- | --- |
| 全图 Coordinate 起点检索 | Coordinate search | 40913 | 独立 Coordinate query template，单输入 | one-shot | buzz-semantic-query coordinate_search、buzz-db semantic_coordinate_search、buzz-relay semantic_coordinate_search |
| Coordinate 到 incident Edge | one-hop tagged family | 40914 | semantic graph Q0，单输入 | one-shot | buzz-semantic-query one_hop_search、buzz-db semantic_query/scoped_search、buzz-relay semantic_one_hop_search |
| Edge 到 member Coordinate | one-hop tagged family | 40914 | semantic graph Q0，单输入 | one-shot | 与上一项共享 family 和执行 owner，以 tagged scope/result variant 隔离 |
| 有界完整路径 | semantic graph query | 40912 | Q0 加零到多个 Qi | multi-stage traversal | buzz-semantic-query contract/query_text/score/root/frontier/result、buzz-relay semantic_graph_query/traversal/response |

三个 surface 的 wire verification 分别由 buzz-sdk 的 semantic_coordinate_search、
semantic_one_hop_search 和 semantic_graph 模块承担；Carryforth CLI 最终都经过同一有界 semantic HTTP
one-shot transport helper，但保持独立 command/result parser。

## 3. 两类执行生命周期

### 3.1 One-shot

覆盖：

- 全图 Coordinate 起点检索；
- Coordinate 到 incident Edge；
- Edge 到 member Coordinate。

当前画像：

- 共用 semantic one-shot admission/ticket/release 安全外壳；
- 一次逻辑请求没有内部 retry；
- release 绑定 exact expected snapshot；
- 不取得 traversal semaphore；
- Provider、writer DB、authorization、generation 和 currentness fence 仍是必经边界。

### 3.2 Multi-stage traversal

覆盖完整路径查询。

当前画像：

- Q0 与已支持 Qi 作为一个有序 Provider batch；
- root selection、RR snapshot traversal、packing 和 response tail 分阶段执行；
- 特定 generation/context churn 最多重做一次 root attempt；
- release 保持完整路径现有 operation-specific snapshot 合同；
- 使用独立 traversal admission。

这些差异是 current runtime profile，不代表未来可靠性运行时必须永久保持相同等待或 attempt 数量。

## 4. Protected contract

后续零行为迁移默认必须保持：

- 三个 surface 的 Event kind、exclusive filter、capability 和 request-result binding；
- 两个 one-hop variant 共用 closed tagged family，不拆分也不混合结果；
- request validation、canonical input bytes、query/ranking/budget/fleet digest；
- host-derived Community、Project、caller authorization 与 feature gate；
- generation、model、dimensions、embedding-space/current-head compatibility；
- operation-specific candidate scope、fixed score、tie、floor、budget、coverage 和 truncation；
- result shape、canonical verifier、response cap；
- snapshot 与 release-time currentness；
- HTTP status、closed code、retryable、body shape 与 CLI exit category；
- feature-off、gate-off 和 capability-off 时 pre-Provider fail closed。

## 5. Current runtime profile

以下行为先记录，后续可以通过独立可靠性或资源治理设计显式改变：

- Busy 是立即返回还是等待；
- Provider attempt/batch count；
- deadline 在各阶段的分配；
- disconnect、cancel 和 shutdown 的传播；
- 当前内部 retry；
- Provider、DB、traversal 和 hydration admission；
- latency、吞吐、saturation 和 metrics 覆盖。

可以改变返回前如何等待、重试或恢复，但不能借 current profile 静默改变最终公开错误合同。

## 6. Deterministic manifest

Tracked manifest：

~~~text
crates/buzz-semantic-query/tests/fixtures/
  semantic_retrieval_compatibility_manifest.json
  semantic_retrieval_compatibility_manifest.sha256
~~~

SHA-256：

~~~text
e7b18cdba9c40fa941a6a70fd8beb2629ecc4232dcc5d94316edbaf4fdae097e
~~~

manifest 固定：

- 一个合成 Project；
- Role、Work、Issue 三个 Coordinate；
- 一条完整无向三成员 Hyperedge；
- Coordinate template 输入；
- one-hop 与完整路径共同 Q0；
- 一个 Work-conditioned Qi；
- deterministic-fake-v1，16 dimensions；
- 四个逻辑 operation 的 ordered input/vector bundle digest；
- result kind、surface、lifecycle 和 protected result boundary；
- current query/ranking/budget/fleet digests；
- fixed score goldens。

真实 embedding 或 Provider vector 没有写入 manifest。query_vector_digests 只是合成 fake vector 的
content-free SHA-256。

## 7. 已有代码证据

### 7.1 Pure contract

- crates/buzz-semantic-query/src/coordinate_search.rs
- crates/buzz-semantic-query/src/one_hop_search.rs
- crates/buzz-semantic-query/src/contract.rs
- crates/buzz-semantic-query/src/query_text.rs
- crates/buzz-semantic-query/src/score.rs
- crates/buzz-semantic-query/src/root.rs
- crates/buzz-semantic-query/src/frontier.rs
- crates/buzz-semantic-query/src/result.rs

### 7.2 Wire、SDK 与 CLI

- crates/buzz-core/src/kind.rs
- crates/buzz-relay/src/api/bridge.rs
- crates/buzz-sdk/src/semantic_coordinate_search.rs
- crates/buzz-sdk/src/semantic_one_hop_search.rs
- crates/buzz-sdk/src/semantic_graph.rs
- crates/carryforth-cli/src/client.rs
- crates/carryforth-cli/src/commands/project_context_one_hop.rs

### 7.3 DB 与 Relay

- crates/buzz-db/src/semantic_coordinate_search.rs
- crates/buzz-db/src/semantic_query.rs
- crates/buzz-db/src/semantic_query/scoped_search.rs
- crates/buzz-db/src/semantic_query/scoped_search_tests.rs
- crates/buzz-relay/src/semantic_one_shot.rs
- crates/buzz-relay/src/semantic_coordinate_search.rs
- crates/buzz-relay/src/semantic_one_hop_search.rs
- crates/buzz-relay/src/semantic_graph_query.rs
- crates/buzz-relay/src/semantic_graph_traversal.rs
- crates/buzz-relay/src/semantic_graph_response.rs

## 8. 既有真实资格证据

已有但需要重新纳入统一运行的历史证据：

- Coordinate 10k production exact SQL qualification；
- Coordinate 本地单Relay真实Provider canary；
- one-hop disposable pgvector scoped search；
- one-hop 本地单Relay真实Provider canary；
- 完整路径 synthetic exact-kernel qualification；
- 完整路径本地单Relay真实Provider/SDK/Desktop D6 canary。

这些历史报告证明相应阶段曾经工作，不替代本次统一 runner，也不证明多Pod、长期429/timeout恢复、公平性
或 production SLO。

### 8.1 本轮真实数据库资格

本轮实际执行了：

- `just semantic-test`：pgvector能力、升级和fresh-schema parity、Coordinate real-pgvector、one-hop
  scoped search、fleet最终授权矩阵全部通过；
- `just semantic-query-qualification`：10k eligible sources、4个Q0/Qi channel的exact kernel测量完成；
- `just coordinate-search-qualification`：10k eligible Coordinates、limit 32、4 clients并发测量完成且
  110次查询零错误。

本地ignored产物及SHA-256：

~~~text
test-results/semantic-exact-query-qualification/20260815T152745Z-3715874/qualification.json
a12e377f01da7317e44cd1ebefba6abbe03d5a4050593c9c94123c8a6b51137a

test-results/coordinate-search-exact-qualification/20260815T152846Z-3720187/qualification.json
24fda3f56dc81d7bd65faeeef76f3776ab1dd1022d0721233e7026e5c74e80af
~~~

这些是本地合成数据库测量，不冻结SLO。完整路径target-default的p50落在0.35–0.40秒、p95落在
0.45–0.50秒；Coordinate并发测量的p50落在0.25–0.30秒、p95落在0.40–0.45秒。范围只描述本次
运行画像，不是后续兼容golden。

### 8.2 本轮真实 Provider 资格

未运行。受支持的语义 Provider 三项配置均不存在，因此没有启动Relay、没有短期开gate、没有外发请求，
也没有需要回滚的临时运行状态。历史三surface canary仍只作为既有证据，不冒充本轮统一canary。

## 9. Known deviations 与未关闭资格

1. Coordinate search 当前 query template 与 one-hop/完整路径 Q0 不同；相同自然语言不会得到相同输入。
2. One-shot 与完整路径的 release snapshot 合同当前不同。
3. One-shot 无内部 retry；完整路径只在特定 churn 下存在一次受限 root retry。
4. 当前没有可重复执行的三-surface真实 Provider canary runner；历史 native seam 部分已经删除。
5. 三个 surface 尚缺同一故障注入下完整的 disconnect/cancel 与 Provider/DB work-stop 对照。
6. One-hop 尚无独立目标规模 scope-join/hydration SLO；共享 exact-kernel测量不能替代它。
7. 完整路径 D6 known-negative 仍返回候选；这是相关性质量问题，不是本兼容基线要修复的行为。
8. 目标部署SLO、长期Provider故障、混合并发、公平性和多Pod资格尚未关闭。

这些条目不自动成为永久产品合同。

## 10. 本轮新增门

~~~text
just semantic-retrieval-compatibility-baseline
~~~

聚合入口负责：

- manifest SHA-256 与结构检查；
- 四 operation、三 surface、40912/40913/40914 检查；
- Coordinate 输入与 Q0 当前差异检查；
- one-hop 与完整路径 Q0、vector digest一致性；
- 禁止 raw embedding 与credential-shaped value；
- pure、wire、DB、Relay 和 CLI characterization；
- default-feature production check；
- 可选 B5 production-path freeze diff。

服务型资格仍由 semantic-test、semantic-query-qualification 和 coordinate-search-qualification 等显式入口
执行。

## 11. 执行进度

| Phase | 状态 | 证据 |
| --- | --- | --- |
| B0 只读盘点 | 完成 | 本文第2–5、7–9节 |
| B1 fixture/manifest | 完成 | tracked JSON、SHA-256、compatibility_baseline Rust test |
| B2 pure/wire/CLI | 完成 | 聚合runner全套通过；四operation/三surface deterministic characterization通过 |
| B3 DB/Relay/race | 完成（保留known deviation） | semantic-test、两个target-scale qualification通过；跨surface取消对照仍单列 |
| B4 real Provider canary | 外部阻断，未运行 | 缺少受支持的`BUZZ_SEMANTIC_*` Provider配置；零外发、零gate变更 |
| B5 freeze/review | 完成 | freeze diff、default-feature check、严格Clippy、test-unit、just ci、隐私与manifest门通过 |

## 12. 隐私与产物

- manifest 只包含合成 query、合成 UUID、fake vector digest 和固定合同元数据；
- 不记录真实 query、context overview、title、summary、Document正文或项目identity；
- 不记录Provider请求/响应、headers、URL、真实vector、精确真实score；
- 不记录private key、NIP-98 payload或完整caller identity；
- 真实资格原始产物只写入 ignored test-results；
- 仓库只回填content-free计数、分类、范围化指标和rollback状态。

## 13. 下一步

兼容基线作为不可改写的历史v1 oracle继续保留。第一阶段
[统一语义计算实现计划](project-context-unified-semantic-computation-implementation-plan.md)已经完成，并由独立
[资格记录](project-context-unified-semantic-computation-qualification.md)维护当前route、差分、数据库与
target-scale证据；本记录不回填新profile或重写历史golden。

真实Provider统一canary仍因缺少受支持的`BUZZ_SEMANTIC_*`配置而未运行；跨surface生产容量与完整故障恢复
仍不属于第一阶段结论。下一阶段应单独设计统一可靠性运行时，不得借更新本基线引入retry或资源治理行为。
