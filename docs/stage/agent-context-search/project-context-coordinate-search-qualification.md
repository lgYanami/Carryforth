# Agent Project Context Coordinate起点检索资格记录

> 状态：代码与本地单Relay真实链路验收完成；feature-off；非production-ready
>
> 日期：2026-08-14
>
> 实现分支：`feat/agent-context-search`
>
> 实现基线：`7e53094c1c1f`后的未提交交付工作树
>
> 实现计划：
> [Agent Project Context自然语言Coordinate起点检索分阶段实现计划](project-context-coordinate-search-implementation-plan.md)

## 1. 结论

本轮已证明新入口的核心合同成立：

```text
one natural-language query
  → exactly one Provider embedding input/call
  → current authorized active-edge Coordinate exact top-K
  → one Relay-signed kind:40913 result
  → cf/SDK exact request-binding verification
```

4个非敏感、预标注的前端/后端中英文case均在rank 1返回至少一个可接受Role/Work起点，
Recall@1、Recall@3、Recall@8和MRR均为1.0。与此同时，前端与后端case的top-8均有6个候选重叠，
Jaccard为0.6。这一结果符合产品定位：自然语言检索可以帮助Agent选择起点，但不是hard scope，
也不替代后续`Coordinate → Edge → Coordinate`渐进判断。

本轮不能据此宣称production-ready。目标部署的SLO尚未冻结，多Pod/load-balancer同构资格未执行，
标注集规模仍小，跨语言、更多Coordinate类型和长期Provider稳定性仍需后续资格。

## 2. 交付合同

已交付：

- `cf project-context coordinate-search --query <text> [--limit 1..32]`；
- 独立query/result、query-text contract、SDK builder/verifier；
- virtual response kind `40913`，ordinary存储、REQ、COUNT、NIP-50、Redis fanout全部拒绝；
- 独立NIP-11 capability `carryforth-project-context-coordinate-search-http`；
- 独立process master `CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE`，默认false；
- 复用Community级`semantic_graph_query_enabled`自然语言出域授权；
- active Edge Coordinate并集先去重，再以一个query vector做exact cosine top-K；
- relation-only Document、graph-external source、deleted Edge Coordinate和missing/currentness-invalid head排除；
- 一次Provider调用、无retry、无root/MMR/traversal/Edge/path；
- ACP Project Space提示词从路径型`semantic-query`切到Coordinate起点发现，并明确not-every-turn、
  candidate-not-fact、可拒绝全部候选与后续canonical reread。

未交付且未暗示已存在：

- Coordinate metadata/summary读取新命令；
- Coordinate→Edge、Edge→Document、Edge→Coordinate渐进遍历命令；
- Role/Work hard scope、caller可调floor/权重、rerank、ANN；
- Desktop/Web UI。

## 3. Schema、wire与安全证据

- migration：`0059_project_context_coordinate_search.sql`；
- fresh schema、migration runner、readiness和drift检查同步；
- result marker：`carryforth-project-context-coordinate-search-result`；
- query extension：`carryforth_project_context_coordinate_search`；
- runtime contract digest：`855a6169cc40bbb132d76f965a77d3a1b3dc75fbe232bf30e6794e1ca0d01446`；
- raw extension body在反序列化前受限，request query为trim后非空、无NUL、UTF-8不超过16KiB；
- NIP-98使用exact body，SDK重验Relay signer、caller、Project、request、Event ID与request-binding；
- signed result content要求canonical bytes、连续rank、唯一Coordinate、score降序与稳定tie；
- query、embedding、Coordinate ID、title/summary、exact body与私钥均不进入普通日志或本文。

资格过程中发现并修复一个运维缺口：`buzz-admin semantic query-readiness/query-enable`原先只识别旧路径
查询master。现在它接受“路径查询或Coordinate Search至少一个surface master”，live `/_status`同时验证两个
surface共享的fleet policy、deployment/instance identity与runtime digest，仍通过同一个显式problem-egress确认
开启Community gate。没有使用SQL直改gate绕过正常管理入口。

## 4. Production SQL目标规模测量

执行入口：

```bash
just coordinate-search-qualification
```

本次隔离产物：

```text
test-results/coordinate-search-exact-qualification/20260814T122734Z-361623/qualification.json
SHA-256 49099d05386084c8961a1d5958dd7368c7456c23b6113bd6f35e38bc37d03aa2
```

数据与环境：

| 项目 | 实测 |
|---|---:|
| PostgreSQL | 17.10 |
| pgvector | 0.8.5 |
| dimensions | 2048 |
| active indexed Coordinates | 10,000 |
| active Coordinates missing current head | 1,000 |
| deleted-edge indexed distractors | 5,000 |
| graph-external indexed distractors | 5,000 |
| limit | 32 |

结果：

| 场景 | p50 | p95 | p99 | 其他 |
|---|---:|---:|---:|---|
| sequential，15次 | 276ms | 299ms | 299ms | 0错误 |
| 4 clients，8秒 | 267ms | 358ms | 404ms | 115完成，0错误 |

EXPLAIN：planning 39.491ms，execution 262.092ms；累计shared hit 313,664 blocks，shared read 0，
temp read/write 0，没有spill。测量状态固定为`measurement_complete_slo_not_frozen`：它证明production SQL
在该隔离规模可执行且predicate前置有效，不自动成为目标部署SLO。

## 5. 真实Provider与授权canary

边界：本地单Relay、当前非敏感Project Context、正常Desktop keyring解析、当前授权Human、真实Provider、
真实Relay signer、真实NIP-98与SDK verifier。自然语言query经用户明确授权外发；报告不保留query原文、
Coordinate UUID、完整caller identity、title或summary。

四个case均请求limit 8；`truncated=true`只说明存在第9个eligible候选，不声明答案完整。

| case | count | Recall@1/3/8 | RR | score top→bottom | latency | 类型分布 |
|---|---:|---|---:|---|---:|---|
| frontend_zh | 8 | 1 / 1 / 1 | 1.0 | 768580→712652 | 3578ms | Role 3，Work 3，Stage 1，Meeting 1 |
| backend_zh | 8 | 1 / 1 / 1 | 1.0 | 761755→714262 | 2320ms | Role 3，Work 3，Stage 1，Document 1 |
| frontend_en | 8 | 1 / 1 / 1 | 1.0 | 759818→693960 | 2841ms | Role 3，Work 3，Stage 1，Meeting 1 |
| backend_en | 8 | 1 / 1 / 1 | 1.0 | 722490→682072 | 2267ms | Role 4，Work 3，Document 1 |

重叠：

| cohort | left/right | intersection | union | Jaccard |
|---|---:|---:|---:|---:|
| frontend/backend zh | 8 / 8 | 6 | 10 | 0.6 |
| frontend/backend en | 8 / 8 | 6 | 10 | 0.6 |

解释：定向信号足以把预标注前/后端起点推到top-1，但相似项目语义仍会让大部分top-8共享。
这不是权限失败，也不是Edge/path重复；新入口只负责高召回候选发现，Agent仍需读取候选摘要并自行选择。

## 6. Enable与rollback时序

执行顺序：

1. migration 0059存在，active generation/current heads与Project Context结构ready；
2. Relay仅开启Coordinate Search process master，旧semantic path master保持false；
3. `buzz-admin semantic query-readiness`观察live `/_status`：database、handler、parser、runtime和routing ready；
4. `query-enable --acknowledge-problem-egress`开启共享Community gate；
5. NIP-11只广告Coordinate Search，旧semantic path capability仍不广告；
6. 4次授权查询成功，Provider input/call计数均为4；
7. `query-disable`关闭共享gate；
8. NIP-11立即撤下Coordinate Search，Project Context Edge capability保持；
9. gate-off CLI请求在capability preflight处fail closed；
10. Provider input/call计数保持4→4；
11. Relay以Coordinate Search process master=false、Community gate=false恢复运行，semantic worker继续。

回滚结论只适用于本地单Relay。未将它外推为多Pod或load-balancer的完整rollback证明。

## 7. 已运行门禁

已通过：

- `cargo test -p buzz-semantic-query`：35通过；
- `cargo test -p carryforth-cli`：319通过；
- `cargo test -p buzz-sdk`：302通过；
- `cargo test -p buzz-acp`：837通过；
- `cargo test -p buzz-core`：240通过；
- `cargo test -p buzz-admin semantic::tests`：12通过；
- `scripts/test-semantic-migrations.sh`：upgrade/fresh schema/readiness通过；
- real disposable pgvector Coordinate-search integration：通过；
- target-scale exact SQL qualification：通过；
- Desktop临时native canary：gate-on 4/4，gate-off fail-closed；临时seam已删除，Desktop无交付diff；
- `just test-unit`：28组全部通过；
- `just test`：31组全部通过；
- `just ci`：最终通过，包含workspace Clippy、Desktop检查/构建/测试、Tauri检查与测试、Web production build；
- Desktop Tauri：1729通过、14 ignored；diagnostic tests 3通过；
- `cargo fmt --all -- --check`、公开发布面检查与`git diff --check`：通过。

全量`cargo test -p buzz-relay --lib`在本分支有一个与本功能无关、隔离可复现的既有失败：
`api::mesh_demo::tests::demo_join_forwarded_arm_round_trips_echo`在10秒内未收到echo，结果为504而非200；
其余901通过、29 ignored。没有修改mesh领域代码或弱化该测试；仓库要求的`just test-unit`、`just test`
和`just ci`均已通过。

最终CI的第一次运行在公开源码资产清单发现`nip11.rs`的embedded fixture locator随源码行号移动；清单已按
scanner实际定位更新并由open-source release surface重新验证。第二次运行在Desktop Tauri增量缓存写入时遇到
`ENOSPC`；仅清理可再生成的root与Tauri Cargo incremental目录后，专项Tauri检查和完整`just ci`均通过。
这两项均未通过跳过、降级或修改测试来绕过。

## 8. 未关闭的生产资格

以下仍阻止production-ready声明：

1. 目标部署的Provider、DB和HTTP p95/p99阈值尚未由产品/运维冻结；
2. 未在真实load balancer与多Pod同构fleet上做attestation、并发、乱序release和rollback soak；
3. 当前标注集只有4个case，尚未覆盖Requirement、Issue、Resource、Document、Meeting、title-only、
   多Island、source更新/detach/tombstone/generation切换；
4. 尚未运行长期Provider限流/429/timeout稳定性soak；
5. top-8前后端Jaccard 0.6意味着Agent后续筛选成本仍需在渐进遍历阶段验证。

因此最终发布状态保持：

> Coordinate-only Agent start search implemented, feature-off, not production-ready.
