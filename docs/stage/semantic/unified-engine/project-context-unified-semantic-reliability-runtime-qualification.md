# Project Context 统一可靠性运行时资格记录

> 状态：R0–R6 主体已交付；**correctness 修复中**——代码审计确认七项与冻结合同的偏差
> （RFX-01..RFX-07，见
> [正确性修复计划](fix/project-context-unified-semantic-reliability-runtime-correctness-fix-plan.md)），
> F0 红色基线已建立（§9），F1（§10）与 F2（§11）已交付，RFX-01..RFX-03 关闭，F3–F5 修复
> 未开始。修复关闭前不能声明统一可靠性运行已按实现计划完整交付，也不能声明 production
> qualification 完成
>
> 日期：2026-08-17（R6 收口）；2026-08-18（correctness 修复 F0 基线、F1/F2 交付）
>
> 结论边界：可声明“统一可靠性原语与 Provider 执行层的主体实现已落地”。不能声明 Phase 2
> 按实现计划完整交付；不能声明统一资源治理（bounded queue、fairness、capacity）完成，不能
> 声明 production SLO 或多 Pod 部署资格完成。
>
> 关联文档：
> [统一可靠性运行时实现计划](project-context-unified-semantic-reliability-runtime-implementation-plan.md)、
> [正确性修复计划](fix/project-context-unified-semantic-reliability-runtime-correctness-fix-plan.md)、
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

在交付分支 `feat/semantic-engine`（freeze base `db6c8c1d5`）上运行。correctness 修复（F0 起）之后的
状态逐项更新；未运行的门不得由其他门的绿色替代声明：

| 门 | 状态 | 说明 |
| --- | --- | --- |
| `bash scripts/check-semantic-retrieval-compatibility-baseline.sh all`（`just semantic-retrieval-compatibility-baseline`） | 通过（2026-08-18 F2 复跑） | 兼容基线 manifest、sha256、freeze diff 与确定性测试 |
| `bash scripts/check-semantic-retrieval-computation.sh all`（`just semantic-retrieval-computation`） | 通过（2026-08-18 F2 复跑） | Phase 1 计算合同门；R6 未重跑，F0–F2 复跑确认未被 Phase 2 破坏 |
| `bash scripts/check-semantic-retrieval-reliability.sh all`（`just semantic-retrieval-reliability`） | 通过（2026-08-18 F2 复跑） | manifest 结构门、sha256、characterization golden、freeze diff、`buzz-semantic-query --lib`、`buzz-relay --lib semantic_`、三 crate `cargo check`。rfx 红色基线（§9）在过滤器之外，不影响本门。F1/F2 起本门 digest golden 已随日期化 descriptor 显式重钉（§5） |
| `cargo test -p buzz-relay --lib semantic_` | 通过（2026-08-18 F2 复跑） | 127 通过、2 个 gated 真实 Provider canary 显式 `#[ignore]` |
| `cargo test -p buzz-semantic-query --lib` | 通过（2026-08-18 F2 复跑） | 53 通过；`runtime_digest_is_stable_and_nonzero` 随 F1/F2 descriptor 轮换重钉到新 digest |
| `cargo clippy -p buzz-semantic-query -p buzz-relay --all-targets -- -D warnings` | 通过（2026-08-18 F2 复跑） | F0 起随各修复阶段复跑 |
| `cargo fmt -p buzz-semantic-query -p buzz-relay` | 通过 | 每次提交 hooks 执行 |
| `just test-unit`（`cargo test -p buzz-relay --lib` 全量） | **红色（预期收窄）** | F2 后 `reliability_fix_regressions` 仅 rfx04/rfx05（F3）按计划保持红；969 通过。另有 8 个环境性失败（`api::media`×7、`api::admin::feedback_attachment`×1）：这些测试需真实 Postgres（`ensure_configured_community`），本沙箱 5432 端口无服务，与修复无关——有服务环境下复跑 |
| `just ci` | **未运行** | 本环境无法完成完整本地 PR gate。复跑：`just ci` |
| `just semantic-test`（= pgvector + migration） | **未运行** | 见下两行 |
| `just semantic-pgvector-test` | **未运行** | 本环境无 disposable Postgres。复跑：`just semantic-pgvector-test`（`scripts/test-semantic-pgvector.sh`，设置 `BUZZ_TEST_SEMANTIC_DATABASE_URL`）。单元绿色不替代本门 |
| `just semantic-migration-test` | **未运行** | 同上。复跑：`just semantic-migration-test` |
| `just test`（完整 integration） | **未运行** | 本环境无 Postgres/Redis。已知与本阶段无关的既有基线：`api::media`/`api::admin` 8 个 DB 用例失败。复跑：`just test` |
| 真实 Provider canary（`cargo test -p buzz-relay --lib -- --ignored real_provider`） | **未运行** | 无受支持的真实 embedding Provider 配置。复跑：设置完整 `LLM_*` 三元组后执行 |
| 真实 fleet old→new digest 切流 | **未运行** | 本环境没有真实多 Relay fleet。切流行模板见 §5 |

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
| `2c898e16…`（上 row target） | `3677625395e79b386fcc4445a52ccbe10224b1e1669c1b8c04b0a0732bf28993`（F1 `semantic-query-reliability-20260818-phase2-f1-correctness-v1`） | 2026-08-18 | correctness F1 交付 | R6 binary（`1d8be4643` 后未部署，freeze base 内） | 仓库内编译期切换 + characterization golden/sha256、inline 稳定性测试、binding test 三处同步重钉；未经真实 fleet |
| `36776253…`（上 row target） | `94b3912fe39bdff87335b105067e28667ec1fa19b173bac7bbd97395e0385ace`（F2 `semantic-query-reliability-20260818-phase2-f2-correctness-v1`） | 2026-08-18 | correctness F2 交付 | 同上（F1/F2 均未部署，freeze base 内） | 仓库内编译期切换 + characterization golden/sha256、inline 稳定性测试、binding test 三处同步重钉；未经真实 fleet |

- 当前编译目标 digest：`94b3912f…`（F1 起 deadline-admission、one-shot eighths reserve、
  lifecycle 与 cancellation 四行，F2 起 release 行补入
  `unsigned-result-validated-before-confirmation;permit-linear-move-consume-into-single-signer`
  进入 reliability contract digest；characterization golden、inline 稳定性测试与 binding test
  三处钉住）。历史行保持原值，不静默改史。
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

## 9. Correctness 修复 F0 红色基线（2026-08-18）

代码审计（fix plan §1）确认 RFX-01..RFX-07 与冻结合同的偏差后，F0 建立失败回归基线：
`crates/buzz-relay/src/reliability_fix_regressions.rs`（7 个 `rfx*` 测试，全部在当前代码上失败，
构成修复计划 F0 退出门要求的可重复、内容无关证据）：

| 测试 | RFX | 当前失败事实 |
| --- | --- | --- |
| `rfx01_partial_tail_must_survive_earlier_window_cutoff` | RFX-01 | work 窗口已耗尽、SnapshotClose/Absolute 仍开放时，指向 Absolute 的 tail stage 被 `Deadline(ProviderStart)` 拒绝——合法 partial 无法发布 |
| `rfx02_deadline_expiry_writes_timed_out_latch_state` | RFX-02 | `deadline_expired()` 后 latch 实际状态是 `Cancelling`，`TimedOut` 无写入路径 |
| `rfx02_finalizing_forbids_new_work_stages` | RFX-02 | `Finalizing` 被视为允许新工作，generic stage admission 通过 |
| `rfx03_unsigned_result_validation_precedes_release_finalize` | RFX-03 | 模拟当前 one-shot 顺序（release/finalize 先于结果验证失败）后 latch 停在 `Finalizing`，而合同要求仍是 `Active` |
| `rfx04_circuit_refusal_requires_fresh_authorization_first` | RFX-04 | 授权无法证明（数据库不可达）时，调用方仍观察到 circuit 的 `AdmissionBusy` |
| `rfx05_refused_egress_counts_no_physical_attempt` | RFX-05 | circuit fast-gate 拒绝的零外发请求把 physical attempt 计为 1 |
| `rfx07_qualification_record_carries_the_gate_inventory` | RFX-07 | 本记录 §2 的机械门清单断言（每个 §5 门具名 + 状态显式 + 未运行不得缺失） |

- **RFX-06 的红色证据按修复计划记录为条件式**：其两处偏差（complete-path fresh-plan 输入
  identity 比较缺失、complete-path release transient 未接入有界重验）都位于数据库依赖的
  coordinator 路径之后，无服务环境不可执行；其失败测试在 F4 开工时以 test-first 交付（修复
  计划 §4.5 清单），本记录不以其缺位冒充已验证。
- 基线测试放在独立模块 `reliability_fix_regressions`（lib.rs 仅 `#[cfg(test)]` 声明），测试名
  刻意避开 `semantic_` 子串：三个确定性 characterization 门的过滤器保持历史范围与绿色，
  `just test-unit` 全量红色即修复基线本身。
- 测试通过 `state.rs` 新增的 `#[cfg(test)] app_state_for_reliability_fix_tests` 构造无服务
  AppState（lazy PgPool + 不可达 Redis + 手工 Provider config），rfx04/rfx05 在 circuit fast
  gate 拒绝路径上执行真实 `execute_provider_egress`，数据库从未被触及。
- 当前 runtime digest（`2c898e16…`）、characterization manifest/sha256 与 binary（`1d8be4643`）
  保持为诊断/rollback 基线；F0 未修改任何生产路径、公开合同或 digest。

## 10. Correctness 修复 F1 交付记录（2026-08-18）

F1（fix plan §3.2「Deadline 与 lifecycle」）关闭 RFX-01 与 RFX-02（两测转绿），生产路径变更：

- **target-window admission**（RFX-01）：`admit_stage(window)` 只检查目标窗口自身到期；更早
  窗口的合法花费是 cutoff 而非 terminal 拒绝。complete-path 尾段（bridge 响应组装）指向
  `Absolute`，合法 partial tail 不再被 `Deadline(ProviderStart)` 误拒。one-shot 各
  stage→window 映射修正：ticket 读→Work、短 RR（begin read/search/commit）→SnapshotClose、
  release 确认→SnapshotClose、finalize post-check→仅 Absolute。
- **`TimedOut` 真实 latch**（RFX-02）：`timeout()` 对 latch 执行 CAS 写入 `LIFECYCLE_TIMED_OUT`，
  不再「cancel 后重标返回值」；latch 仲裁失败方记录 discard。
- **`Finalizing` stage 所有权**（RFX-02）：`SemanticStageOwner::{Generic, Finalizer}`——generic
  stage 仅从 `Active` 准入（`Finalizing` 拒绝并以 `LatchClosed` 映射为冻结公开
  `DeadlineExceeded`），finalize stage 仅从 `Finalizing` 准入（F2 signer guard 的 seam）。
- **one-shot eighths reserve**：`ONE_SHOT_RESERVE_DENOMINATOR = 8` 冻结常量——
  `for_one_shot_reserved_budget`：provider_start=5/8、work=6/8、snapshot_close=7/8、
  absolute=公开 45s 合同不变。`for_one_shot_hard_deadline` 仅保留给 gated 真实 Provider
  canary。
- **真实 cancellation 接线**（RFX-02 生产面）：`SemanticShutdownSubscription`（watch channel
  订阅 relay shutdown，含晚订阅者 AtomicBool 前置轮询）与 `SemanticCallerGuard`（请求
  future 被 drop 时以 `CallerDisconnected` 取消——该取消源首次获得生产路径）。guard 生命周期
  语义：one-shot 挂在 attempt、complete-path 由 session 移入 traversal outcome，请求存活期间
  不误发。
- **egress 前置准入**：`execute_provider_egress` 以 `ProviderStart` 窗口准入——F2/F3 的
  retry/circuit 前置条件（`provider_start_before` 后不再发起物理尝试）。
- **descriptor 轮换**：`SEMANTIC_RELIABILITY_RUNTIME_CONTRACT` 日期化 token
  `20260818-phase2-f1-correctness-v1` + 4 行新合同；digest
  `2c898e16… → 36776253…`（§5 第三行），三处 golden 显式重钉。
- **公开合同不变**：45s absolute、closed error 集、HTTP status、`retryable` 均未变；
  `LatchClosed` 内部 abort 一律映射既有冻结错误。

F1 退出门核对：rfx01/rfx02×2 绿；rfx03（F2）、rfx04/rfx05（F3）保持预期红；
`buzz-relay --lib semantic_` 127 绿；三个确定性门复跑绿（§2）；clippy/fmt 绿。

## 11. Correctness 修复 F2 交付记录（2026-08-18）

F2（fix plan §3.3「Release-finalize 线性化」/§2.3）关闭 RFX-03（三测转绿），生产路径变更：

- **unsigned result 前移**（F2 item 1）：两个 one-shot surface（Coordinate、one-hop）的
  request binding 推导、结果构造、`validate_for_request` canonical 验证与 unsigned Event
  builder 构造（含 response cap）全部移动到 release 确认之前——contract/size 失败不再消耗
  release permit，也不再为从未有效的结果锁上 `Finalizing`。
- **单一同步 signer guard**（F2 item 2）：`SemanticExecutionContext::begin_release_signer /
  sign_released`——permit 按值 move 进 guard（唯一构造点，无 `Clone`），仲裁顺序
  cancellation → 公开 `Absolute` → 一次性 `Finalizing` CAS；拒绝路径消费 permit 且绝不调用
  signer 闭包；签名期间到达的 cancel/deadline 只由 §4.1 post-check 丢弃已签名结果；干净
  post-check 完成 latch。`SemanticReleaseSigner` 为 `#[must_use]`、content-free。
- **complete-path 迁入同一形状**（F2 item 3）：bridge 尾段删除手写
  `begin_finalize`/`sign_released_semantic_graph_response`/discard 检查/`complete` 四段，改用
  同一 guard helper；one-shot 侧 `confirm_release` 不再内置 latch 仲裁（纯 release 确认），
  `finalize_completed` 删除。
- **两类 release policy 保留**（F2 item 4）：one-shot exact-snapshot
  （`expected_snapshot: Some`）与 complete-path current-authorization（`None`）不变。
- **竞态覆盖**（F2 item 5）：`rfx03_unsigned_result_validation_precedes_release_finalize`、
  `rfx03_refused_or_spent_release_signer_never_signs`（拒签不进闭包、单次签名、二次授权拒绝）、
  `rfx03_discard_during_synchronous_signing_drops_the_signed_result`（签名中取消→丢弃、
  latch 停 `Finalizing`）。
- **descriptor 轮换**：token `20260818-phase2-f2-correctness-v1`，release 行补入
  `unsigned-result-validated-before-confirmation;permit-linear-move-consume-into-single-signer`；
  digest `36776253… → 94b3912f…`（§5 第四行），三处 golden 显式重钉。
- **公开合同不变**：Event kind、builder、size cap、closed 错误集与 45s absolute 均未变。

F2 退出门核对：每个成功 Event 的 signer 均按值消费一个 release permit（类型与测试双向证明）；
rfx03×3 绿；rfx04/rfx05（F3）保持预期红；`buzz-relay --lib semantic_` 127 绿；三个确定性门
复跑绿（§2）；clippy/fmt 绿。
