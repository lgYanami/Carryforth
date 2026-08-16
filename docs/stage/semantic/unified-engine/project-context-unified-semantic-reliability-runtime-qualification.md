# Project Context 统一可靠性运行时资格记录

> 状态：R0–R6 已交付并通过本记录所列的确定性资格门；真实数据库资格、真实 Provider canary 与
> 真实 fleet 切流未在交付环境运行，逐项列明原因与复跑配方
>
> 日期：2026-08-17
>
> 结论边界：可以声明**统一可靠性原语与 Provider 执行层已交付**。不能声明统一资源治理
> （bounded queue、fairness、capacity）完成，不能声明 production SLO 或多 Pod 部署资格完成。
>
> 关联文档：
> [统一可靠性运行时实现计划](project-context-unified-semantic-reliability-runtime-implementation-plan.md)、
> [统一语义检索引擎规范](project-context-unified-semantic-retrieval-engine-spec.md)、
> [统一语义计算资格记录](project-context-unified-semantic-computation-qualification.md)、
> [Stage TODO](../../TODO.md)

## 1. 交付范围

R0 characterization、R1 typed failure/执行上下文、R2 共享 Provider 可靠性执行器、R3
deadline/cancellation/release-finalize、R4 安全 retry/backoff/request-local vector 复用、R5
共享 process-local Provider circuit、R6 资格与文档收口，全部按实现计划交付。四个逻辑
operation（whole-graph Coordinate、两个 one-hop variant、bounded complete path）共享：

- 同一个 `SemanticExecutionContext`（deadline windows、聚合 cancellation、latch、attempt ledger）；
- 同一个 Provider reservation/wait/egress/encode-once 执行器与 R5 circuit gate；
- 同一套 typed failure 分类、closed retry policy 与冻结公开错误映射；
- 同一个 process-local Provider 故障域（Provider 实例持有的 `Arc` circuit）。

## 2. 资格门运行记录

在交付分支 `feat/semantic-engine`（freeze base `db6c8c1d5`）上运行：

| 门 | 状态 | 说明 |
| --- | --- | --- |
| `bash scripts/check-semantic-retrieval-reliability.sh all` | 通过 | manifest 结构门、sha256、characterization golden、freeze diff、`buzz-semantic-query --lib`、`buzz-relay --lib semantic_`、三 crate `cargo check` |
| `cargo test -p buzz-relay --lib semantic_` | 通过 | 127 通过、2 个 gated 真实 Provider canary 显式 `#[ignore]` |
| `cargo clippy -p buzz-semantic-query -p buzz-relay --all-targets -- -D warnings` | 通过 | 无告警 |
| `cargo fmt -p buzz-semantic-query -p buzz-relay` | 通过 | 已格式化 |
| `just test-unit` | 通过 | 全仓库单元套件（无需服务） |
| `just semantic-pgvector-test` | **未运行** | 本环境无 disposable Postgres。复跑：`just semantic-pgvector-test`（`scripts/test-semantic-pgvector.sh`，设置 `BUZZ_TEST_SEMANTIC_DATABASE_URL`）。单元绿色不替代本门 |
| `just semantic-migration-test` | **未运行** | 同上。复跑：`just semantic-migration-test` |
| `just test`（完整 integration） | **未运行** | 本环境无 Postgres/Redis。已知与本阶段无关的既有基线：`api::media`/`api::admin` 8 个 DB 用例失败。复跑：`just test` |
| 真实 Provider canary（`real_provider_semantic_input_canary`、`real_provider_reliability_canary_is_bounded_and_content_free`） | **未运行** | 无受支持的真实 embedding Provider 配置。复跑：设置完整 `LLM_*` 三元组后 `cargo test -p buzz-relay --lib -- --ignored real_provider` |
| 真实 fleet old→new digest 切流 | **未运行** | 本环境没有真实多 Relay fleet。切流行模板见 §5 |

未运行的门不得由其他门的绿色替代声明；上表即计划要求的诚实记录。

## 3. R6 交付项与证据映射

| 计划项 | 证据 |
| --- | --- |
| 1. deterministic fault matrix | `semantic_provider::tests::provider_fault_matrix_classifies_real_transport_failures`（500/429±Retry-After/4xx/200 协议违约逐行钉住 attempt 分类、circuit observation、retry 决策三个编译视图）与 `connect_failure_is_pre_handoff_health_and_retryable` |
| 2. disposable DB currentness/release race | 现有 env-gated 套件：`buzz-db` `final_confirmation_lock_orders_writer_first_and_permit_first_revocations`、`one_hop_scoped_search_*`、`coordinate_search_target_scale_exact_sql_qualification` 等（`BUZZ_TEST_SEMANTIC_DATABASE_URL` 门控）。本环境未运行，见 §2 |
| 3. fake Provider retry/circuit/attempt 证据 | `enforcing_circuit_moves_only_on_real_health_faults`（真实 500 恰在阈值 5 打开、refusal 走既有 Busy 形状；真实 429 只开独立 throttle 窗、不计健康）+ `semantic_query_runtime::tests::fleet_runtime_digest_binds_the_compiled_reliability_contract`（descriptor 与编译常量、ledger 预算双向钉住） |
| 4. gated 真实 Provider 短 canary | `real_provider_reliability_canary_is_bounded_and_content_free`（`#[ignore]`）：只断言 content-free 不变量——单次物理 attempt、circuit admit/observe、closed 失败分类；结果即丢弃，不保留 query/vector/body |
| 5. cancellation 与 shutdown soak | `semantic_query_runtime::tests::cancellation_and_shutdown_soak_stays_bounded_and_clean`：240 次迭代 × 四个 cancellation source × 三个生命周期形状（mid-stage 取消、窗口到期、finalize/cancel 竞速），每次迭代 5 秒预算内必须完成、pending stage 必须丢弃、拒绝必须稳定、post-check 必须丢弃迟到签名 |
| 6. fleet runtime digest 与同质性 | `SEMANTIC_RELIABILITY_RUNTIME_CONTRACT` 进入 `semantic_graph_http_runtime_digest()`；既有 fleet inventory 校验以 `RuntimeMismatch` 拒绝 digest 异构实例；characterization golden 重新钉住新 digest；buzz-relay binding test 把 descriptor 数值与编译常量互相绑定 |
| 7. qualification/spec/TODO/README/current-status 同步 | 本记录、规范头状态行、两份 Stage TODO、`docs/en/current-status.md` §4.6、README 无需新增入口（current-status 已链接） |
| 8. Phase 1 窗口记录 | 见 §4 |
| 9. 真实 fleet digest 切流记录与独立删除窗口 | 见 §5 |
| 10. 第三阶段未交付清单 | 见 §6 |

## 4. Phase 1 computation 窗口（计划项 8）

- 既有 owner：第一阶段统一语义计算实现计划/资格记录（U6/U7 收口）。
- 日期：legacy Coordinate SQL 与完整路径 compatibility adapter 的既定删除日期为
  **`2026-09-16`**（profile rollback 源，非运行时 flag）。
- 当前状态：保留为 rollback 源；删除门要求 U7 资格、至少一次部署/回滚演练证据与回滚窗口
  均闭合，否则必须以显式架构事项延期。
- Phase 2 对 Phase 1 legacy 证据的依赖：`crates/buzz-semantic-query/tests/computation_differential.rs`
  仍以 live legacy-vs-migrated 差分（`assert_eq!(legacy(shared), migrated(shared))`）作为 oracle；
  disposable pgvector 套件同样以 legacy 参考钉住 K/K+1/tie/scope 相等。**Phase 2 正常路径不依赖
  legacy 实现**（四 route 已全 Migrated，运行时无 legacy fallback），但**差分 oracle 仍依赖**。
- 结论：`2026-09-16` 到期时，Phase 1 删除 change 必须先重指向或替换上述差分 oracle，或显式
  延期；R6 不替代、也不顺延该独立 change。

## 5. reliability digest 切流记录（计划项 9）

| source digest | target digest | 时间 | owner | rollback binary | 演练 |
| --- | --- | --- | --- | --- | --- |
| `e49d7ae9e69a2818a9ce9c061443a4441d332c86a3f8b46824b147a5da716f40`（Phase 1 U6） | `d9878ff28260cc8161795ce8cd479ba879387f3f34ae35b389734fd6ea753bef`（coordinate-filter-v2，Phase 2 R0–R5 期间编译值） | 2026-08-16 | Phase 1 U6/类型过滤交付 | 前一已部署 binary | 仓库内编译期切换，未经真实 fleet |
| `d9878ff2…`（上 row target） | `2c898e16398d8c65d10c37052f08f07178586632e8e647ad5484d0bbff8bd4ae`（R6 `semantic-query-reliability-20260817-phase2-r6-v1`） | 2026-08-17 | R6 交付（本 change） | freeze base `db6c8c1d5` 起的可部署前一 binary | 仓库内编译期切换 + characterization golden/sha256 同步重钉；未经真实 fleet |

- 当前编译目标 digest：`2c898e16…`（reliability contract 进入 digest；characterization golden、
  inline 稳定性测试与 binding test 三处钉住）。
- 真实 fleet 行（source、时间、owner、rollback binary、gate/drain/redeploy/re-attest 演练结果）
  在每次实际切流时追加；本交付环境未执行任何真实 fleet 切流，**不存在用第一个编译窗口覆盖
  后续变化的情况**。
- Phase 2 legacy reliability 删除窗口：**未开始**。仅当最终目标 reliability digest 完成真实
  fleet 同质切流、确定性/disposable DB/真实 Provider 资格通过、并完成整 fleet binary rollback
  演练后，才独立记录开始/结束日期与 owner；该窗口不继承 `2026-09-16`。

## 6. 已知限制与第三阶段未交付清单（计划项 10）

Process-local 限制（R5 既定，R6 复核）：circuit epoch/lease/throttle 均为进程内状态，不跨
Pod 共享；**不宣称多 Pod 防惊群**。fleet-shared epoch/lease 属第三阶段。

第三阶段（统一资源治理）未交付事项：

- bounded queue 与队列容量治理；
- 负载反馈 admission（load-feedback admission）；
- 跨 operation 公平性（fairness）与并发阶梯（concurrency ladders）；
- PostgreSQL 资源隔离与长时 soak；
- fleet-shared circuit 状态与多 Pod 防惊群；
- production SLO 与完整恢复证据。

## 7. R0 known gaps 关闭状态

| gap | 状态 |
| --- | --- |
| `one_shot_release_permit_dropped` | R3 关闭：release permit 同步消费进 Event 签名 |
| `no_unified_request_cancellation` | R3 关闭：caller disconnect/server shutdown/deadline/explicit cancel 聚合进同一 context |
| `one_shot_deadline_does_not_bind_post_release_work` | R3 关闭：release 后工作进入 absolute 尾段合同，latch post-check 拒发已取消签名 |

R0 characterization manifest 作为历史基线保持冻结（phase=pre_phase2_reliability_runtime、
known_gaps 原文保留）；其 `http_runtime_digest` 字段记录当前编译 digest 并由测试重算钉住，
R6 的更新即 §5 记录的第二次编译期切换，非静默改史。

## 8. R6 审计说明

- freeze diff allowlist 增补 `crates/buzz-semantic-query/src/lib.rs` 与 `docs/en/current-status.md`：
  前者仅为 re-export 新公开常量 `SEMANTIC_RELIABILITY_RUNTIME_CONTRACT`（buzz-relay binding test
  的依赖方向所需），后者是计划项 7 要求的 current-status 同步；两文件在 freeze base 之后均无
  其他改动。
- 本阶段未引入新的公开 HTTP API、新的公开错误码或新的 Event kind；circuit refusal 经既有
  Busy/Unavailable 冻结映射出栈。
- 日志与 metrics 维持 content-free：circuit key、failure-domain key、observation/gate/transition
  标签均为 closed 低基数枚举或 digest。
