# Agent Project Context 渐进观察与一跳语义选择资格记录

> 状态：代码与本地单Relay真实链路验收完成；feature-off；非production-ready
>
> 日期：2026-08-15
>
> 实现分支：`feat/agent-context-search`
>
> 实现基线：B6 `03f9fbef7`后的B7资格与修复增量
>
> 实现计划：
> [Agent Project Context渐进观察与一跳语义选择分阶段实现计划](project-context-progressive-observation-cli-implementation-plan.md)

## 1. 结论

本轮已经证明两个原子一跳语义选择操作在当前本地单Relay夹具上按合同工作：

```text
Coordinate + natural-language query
  → current incident Edge relation Documents exact cosine
  → ranked Edges + canonical Document previews

Edge + natural-language query
  → current complete Edge member overviews exact cosine
  → ranked Coordinates + canonical source previews
```

前端/后端各一组Coordinate→Edge查询，都把预标注的对应Edge排在rank 1；前端/后端各一组
Edge→Coordinate查询，都把预标注的对应Role或Work排在rank 1。四次显式调用对应四次Provider请求，
没有retry、fallback或额外embedding调用。所有候选都携带current canonical preview、source basis和typed
read descriptor；两个result variant没有越界返回另一层结构。

这不等于Agent已经会渐进遍历。visited set、循环防护、分支、停止条件、如何把当前Role/Work/Issue/会议目的
写入query，以及如何形成不同上下文路径，仍属于后续独立prompt/traversal设计。本资格也不证明production-ready：
one-hop canonical join的目标规模SLO与multi-pod/load-balancer资格尚未完成。

## 2. 已验收的交付面

结构观察CLI：

- `cf project-context coordinate show <TYPE:UUID>`；
- `cf project-context coordinate edges <TYPE:UUID> [paging]`；
- `cf project-context edge documents <EDGE_KEY> [paging]`；
- `cf project-context edge coordinates <EDGE_KEY>`。

一跳语义选择CLI：

- `cf project-context coordinate edge-search <TYPE:UUID> --query <TEXT> [--limit 8]`；
- `cf project-context edge coordinate-search <EDGE_KEY> --query <TEXT> [--limit 8]`。

安全与wire：

- 两个operation共用一个closed tagged one-hop request/result family；
- Relay-only virtual result kind `40914`，普通存储、REQ、COUNT、NIP-50、by-id和fanout均拒绝；
- 独立NIP-11 capability `carryforth-project-context-one-hop-semantic-search-http`；
- 独立process master `CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE`，缺省false；
- Community自然语言出域授权复用`semantic_graph_query_enabled`；
- exact body NIP-98、Relay signer、caller、Project、request、result marker和request-binding均由SDK重验；
- 复用现有active generation、current-head exact cosine、Provider admission/rate debt和release fences；
- 不进入root fusion、Qi、MMR、coherence、floor、beam、traversal或path packing；
- query、Provider input、vector、结果正文、候选identity和私钥不进入本文或普通低基数日志。

## 3. 真实链路发现并修复的问题

首次真实一跳查询在Relay的database-result验证阶段fail closed。根因不是Provider、索引或权限，而是
Project View v3持久化body与typed enum envelope的形状不同：数据库body把业务字段与
`context_references`平铺存储，Role还带治理字段`level`；typed `ProjectViewObjectDataV3`要求
`{object_type,data}` envelope。

Disposable fixture只覆盖Document候选，因此没有提前发现该问题。B7修复做了三件事：

1. preview hydration同时读取数据库`object_type`与body；
2. 按canonical v3 reader相同规则验证并剥离`context_references`，Role再剥离`level`，随后重建typed envelope；
3. 增加Work、Role、missing references和subtype drift纯回归，并在当前真实数据库执行read-only scoped canary。

外部错误仍是固定content-free闭集；新增内部诊断只记录低基数verification stage，不记录query、scope identity、
title、summary、description、raw body或caller。

## 4. 本地单Relay真实Provider canary

边界：当前本地Community、正常Desktop keyring身份解析、当前授权member、真实Relay signer、真实Provider、
真实PostgreSQL/pgvector、NIP-98和SDK verifier。用户已明确允许自然语言query外发。报告仅保存聚合统计，
不保存query原文、Coordinate/Edge/Document identity、完整caller、title、description或summary。

夹具使用两个语言高度相似且共享Issue/Stage的前端/后端Edge。每个Edge包含Role、Work、Issue、Stage四个成员；
共享Issue作为两次incident scope，分别用前端与后端任务意图查询。随后分别在前端/后端Edge内查询对应Role/Work。

| case | returned | 预标注rank | score min→max | latency | preview | variant隔离 |
|---|---:|---:|---:|---:|---|---|
| frontend incident Edge | 3 Edges | 1 | 638431→678007 | 2744ms | 完整 | 通过 |
| backend incident Edge | 3 Edges | 1 | 676434→682459 | 2951ms | 完整 | 通过 |
| frontend Edge members | 4 Coordinates | 1 | 642389→739874 | 2700ms | 完整 | 通过 |
| backend Edge members | 4 Coordinates | 1 | 612530→693091 | 2777ms | 完整 | 通过 |

Provider request counter在四次调用前后增量严格为4。两个incident scope各有3条active Edge、3条active/scorable
relation binding、0 omitted；两个Edge scope各有4个完整/scorable成员、0 omitted，其中各1个title-only source。
所有场景`truncated=false`。

解释边界：这组结果证明结构限域后，直接Q0语义信号足以在小候选集里把前端/后端对应对象推到rank 1；
它不证明任何自然语言都能正确区分上下文，也不把score当作事实。canonical preview的产品作用正是让Agent在
不立即读取完整正文的前提下，结合自身已知上下文排除语义相近但任务不合适的候选。

## 5. 性能证据边界

共享exact cosine kernel已有本地合成规模产物：

```text
test-results/semantic-exact-query-qualification/20260814T174724Z-1191821/qualification.json
SHA-256 121551a09294fc93239d416c0b8967966790d6d408e126131955e10006bc1b38
```

该产物证明10k source规模的共享exact scorer、predicate-before-distance、cancellation和短并发soak可执行；
它不包含本功能的incident relation binding聚合、完整Hyperedge member join、canonical preview hydration和40914
response packing。因此不得把它冒充one-hop目标规模SLO。

本轮只记录真实小夹具端到端延迟约2.7–3.0秒。产品/运维尚未冻结目标p95/p99，尚未执行
1/32/1024 Edge、1/3/2048 relation Documents、2/32/4096 members的专用规模矩阵，也未执行multi-pod路由。

## 6. Feature-off与rollback

真实canary结束后按正常运维入口执行：

1. `buzz-admin semantic query-disable`关闭共享Community query gate，semantic index继续；
2. NIP-11立即撤下one-hop capability；
3. 再次调用语义CLI在capability preflight处exit 1，Provider request counter增量为0；
4. `coordinate show/edges`与`edge documents/coordinates`四个结构读取继续成功；
5. Relay重启为one-hop process master=false；
6. 重启后capability仍缺失、Community gate=false、canonical Coordinate读取成功、semantic worker继续。

当前本地拓扑使用`trusted-single-relay`，因此fleet attestation/revoke为not-required；没有伪造multi-pod fleet
证据。该rollback只证明本地单Relay，不外推到load balancer或生产多实例。

## 7. 已运行门禁

阶段性已通过：

- `cargo test -p buzz-db project_view_preview_ --lib`：2通过；
- 当前真实数据库read-only one-hop scoped canary：1通过；
- `cargo clippy -p buzz-db -p buzz-relay --all-targets -- -D warnings`；
- `scripts/test-semantic-pgvector.sh`；
- `scripts/test-semantic-migrations.sh`：upgrade/fresh schema/readiness、Coordinate Search、one-hop scoped real pgvector与fleet回归通过；
- `just test-unit`：28组全部通过；
- `just test`：31组全部通过；
- `just ci`：完整本地PR门通过，包括root workspace fmt/clippy、Rust unit、Desktop check与
  3707个前端测试、Tauri check与1729个测试（另14个环境依赖测试按声明忽略）、Web check/build；
- B6 `cargo test -p carryforth-cli`：332通过；
- B6 SDK one-hop定向测试：5通过；
- `git diff --check`。

`just test`第一次执行只有两个`buzz-agent/tests/fake_llm.rs`计时相关case失败；两者紧接着单独复跑均通过，
与本功能没有代码交集。完整第二次执行31组全部通过，其中这两个case也正常通过；其后完整`just ci`
同样通过。

## 8. 未关闭的生产资格

以下事项继续阻止production-ready：

1. one-hop canonical scoped join/preview/packing没有目标规模EXPLAIN、p50/p95/p99和冻结SLO；
2. 未在真实load balancer与multi-pod同构fleet执行attestation、并发、乱序release和rollback soak；
3. 当前paired fixture只有前端/后端一组，未覆盖大规模Document、多Island、Meeting、Resource、source churn与空结果质量；
4. 未执行长期Provider限流、429、timeout和公平性soak；
5. Agent traversal/prompt尚未设计，不能声称已交付visited set、循环防护、分支、停止策略或不同上下文路径。

因此当前准确状态是：

> Carryforth已经交付原子结构观察和两个结构限域的一跳语义选择CLI，并完成本地单Relay真实链路与rollback验收；
> one-hop capability保持feature-off，尚未production-ready。
