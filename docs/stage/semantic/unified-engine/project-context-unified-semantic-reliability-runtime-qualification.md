# Project Context 统一可靠性运行时资格记录

> 状态：R0–R6 主体已交付；correctness 修复 **F0–F5 全部交付（§9–§14）**——RFX-01..RFX-06
> 关闭、rfx 基线保持清零、deterministic/disposable DB/migration/unit/integration/真实 Provider
> canary 门绿色（§2；`just ci` 唯一失败为与语义运行时无关的 main 既有 release-asset pin）。
> 分层结论：**Phase 2 correctness implementation 已交付；deployment qualification 未完成**——
> 真实 fleet old/new digest 切流、gate/drain/re-attest 与 binary rollback 演练未执行（无真实
> fleet），修复计划 §7.2 未关闭，不得声明 "Phase 2 已完整交付" 或 production-ready
>
> 日期：2026-08-17（R6 收口）；2026-08-18（correctness 修复 F0 基线、F1/F2/F3/F4/F5 交付）
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
| `bash scripts/check-semantic-retrieval-compatibility-baseline.sh all`（`just semantic-retrieval-compatibility-baseline`） | 通过（2026-08-18 F5 复跑） | 兼容基线 manifest、sha256、freeze diff 与确定性测试 |
| `bash scripts/check-semantic-retrieval-computation.sh all`（`just semantic-retrieval-computation`） | 通过（2026-08-18 F5 复跑） | Phase 1 计算合同门；R6 未重跑，F0–F5 复跑确认未被 Phase 2 破坏 |
| `bash scripts/check-semantic-retrieval-reliability.sh all`（`just semantic-retrieval-reliability`） | 通过（2026-08-18 F5 复跑） | manifest 结构门、sha256、characterization golden、freeze diff、`buzz-semantic-query --lib`、`buzz-relay --lib semantic_`、三 crate `cargo check`。rfx 基线在过滤器之外；F1–F4 起本门 digest golden 已随日期化 descriptor 显式重钉（§5） |
| `cargo test -p buzz-relay --lib semantic_` | 通过（2026-08-18 F5 复跑） | 132 通过（F4 新增 3 个 release-confirmation runtime 测试与 1 个 root-attempt identity 钉住测试）、2 个 gated 真实 Provider canary 显式 `#[ignore]`（该 canary 于 F5 实际运行，见下行） |
| `cargo test -p buzz-semantic-query --lib` | 通过（2026-08-18 F5 复跑） | 53 通过；`runtime_digest_is_stable_and_nonzero` 随 F1–F4 descriptor 轮换重钉到新 digest |
| `cargo clippy -p buzz-semantic-query -p buzz-relay --all-targets -- -D warnings` | 通过（2026-08-18 F4 复跑；F5 `just clippy` 全 workspace 亦绿） | F0 起随各修复阶段复跑 |
| `cargo fmt -p buzz-semantic-query -p buzz-relay` | 通过 | 每次提交 hooks 执行 |
| `cargo test -p buzz-relay --lib`（全量） | 通过（2026-08-18 F5，986 通过/0 失败/31 ignored） | 无服务环境曾为 978 通过/8 环境性失败（`api::media`×7、`api::admin::feedback_attachment`×1，`ensure_configured_community` 需真实 Postgres）；F5 起 compose Postgres/Redis 在位，全量转绿：986 通过、0 失败，`reliability_fix_regressions` 16/16 绿（rfx 基线保持清零） |
| `just test-unit` | 通过（2026-08-18 F4 复跑，exit 0） | 无服务 unit 门：buzz-core/auth/db/conformance、Project View/Document/Context、buzz-acp `--lib`、buzz-relay `meeting` 过滤器（该门不含 buzz-relay 全量 `--lib`，全量见上行），28 组全绿、耗时 73s。F5 经 `just test`/`just ci` 组合复跑同为绿色 |
| `just ci` | 部分通过（2026-08-18 F5） | `check` 链唯一失败为 `ci-source-contracts` → release-asset-inventory 对 `desktop/src-tauri/icons/**` 的既有 tree_sha256 pin（`fe7cf9d1…` vs 实际 `51403134…`）——该 pin 与 icons 均先于本分支（main 上即失败，分支对二者零改动），与本修复无关；修复建议：独立 change 重新核对并显式重钉 `release/packaged-assets.json`。其余组件逐项全绿：fmt-check、clippy（全 workspace）、web-check、desktop-check、desktop-tauri-fmt-check、desktop-tauri-clippy、test-unit、desktop-test、desktop-build、desktop-tauri-check、desktop-tauri-test、web-build |
| `just semantic-test`（= pgvector + migration） | 通过（2026-08-18 F5） | 两门均以脚本自管 disposable Docker Postgres 运行，见下两行 |
| `just semantic-pgvector-test` | 通过（2026-08-18 F5） | 脚本自起 disposable pgvector 容器（pgvector 0.8.5 / Postgres 17.10）：2048 维 vector/halfvec roundtrip、halfvec HNSW 索引、sqlx roundtrip、`buzz-admin semantic preflight` `ready: true` |
| `just semantic-migration-test` | 通过（2026-08-18 F5） | disposable 容器上 semantic 迁移、desired-schema 与 ledger-less fresh-schema 门全绿（含 real-pgvector scoped search 与 fleet policy matrix 用例） |
| `just test`（完整 integration） | 通过（2026-08-18 F5，31 组全绿，112s） | compose Postgres/Redis 在位运行；此前的 `api::media`/`api::admin` 8 个 DB 用例失败随服务在位全部转绿 |
| 真实 Provider canary（`cargo test -p buzz-relay --lib -- --ignored real_provider`） | 通过（2026-08-18 F5，2/2） | 本机 `.env` 提供完整 `LLM_*` 三元组，端点可达；`real_provider_semantic_input_canary` 与 `real_provider_reliability_canary_is_bounded_and_content_free` 通过——仅断言 content-free 不变量（bounded attempts、closed failure taxonomy），不保存 query/vector/body |
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
| `94b3912f…`（上 row target） | `745ca5843c4bd8f22a33fc9fe9d6a9f7dff51fb8f137efe8b18df4dab2a36b47`（F3 `semantic-query-reliability-20260818-phase2-f3-correctness-v1`） | 2026-08-18 | correctness F3 交付 | 同上（F1–F3 均未部署，freeze base 内） | 仓库内编译期切换 + characterization golden/sha256、inline 稳定性测试、binding test 三处同步重钉；未经真实 fleet |
| `745ca584…`（上 row target） | `8dc7f5e862f58627777c66abc50188b4d37266a9d11891267f0f589a98829cdc`（F4 `semantic-query-reliability-20260818-phase2-f4-correctness-v1`） | 2026-08-18 | correctness F4 交付 | 同上（F1–F4 均未部署，freeze base 内） | 仓库内编译期切换 + characterization golden/sha256、inline 稳定性测试、binding test 三处同步重钉；未经真实 fleet |

- 当前编译目标 digest：`8dc7f5e8…`（F1 起 deadline-admission、one-shot eighths reserve、
  lifecycle 与 cancellation 四行，F2 起 release 行补入
  `unsigned-result-validated-before-confirmation;permit-linear-move-consume-into-single-signer`，
  F3 起 attempt-ledger/circuit-gate/handoff 三行——预算在 circuit gate 之前以 non-counting
  token 保留、physical 只计真实 handoff、circuit 拒绝仅在 fresh authorization 复核后对调用方
  可见、最终 fence 与预算消费合并进单一同步 handoff 点——F4 起 retry-fresh-plan 行补入有序
  input 恒等门（kinds/digests/顺序/generation 钉住，恒等变化经共享 attempt ledger 返回 typed
  operation restart）、release 行补入共享 bounded release confirmation（两 surface 同一 helper、
  上限 2、仅 no-permit/no-unknown-side-effect transient 原地重试、closed outcome 永不重试、
  `expected_snapshot` 两 surface 各自保持）——进入 reliability contract digest；
  characterization golden、inline 稳定性测试与 binding test 三处钉住）。历史行保持原值，不
  静默改史。
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

## 12. Correctness 修复 F3 交付记录（2026-08-18）

F3（fix plan §3.4「Circuit、handoff 与 physical ledger」/§2.4、§2.5）关闭 RFX-04 与
RFX-05（rfx04/rfx05 转绿，**F0 红色基线清零**；RFX-06 按 §9 保持条件式，待 F4 test-first）。
生产路径变更（`semantic_query_runtime.rs` 统一执行器，one-shot 与 complete-path 两个 surface
迁入）：

- **统一物理 attempt 执行器 `execute_provider_attempt`**：ProviderStart 窗口准入 →
  `latest_start` → 预算保留 → circuit fast gate →（过闸后）数据库 reservation → wait →
  wait-stale circuit 复核 → routing trust → final confirmation → **`authorize_provider_handoff`**
  （最终 epoch revalidate + 预算消费合并为单一同步点，其间无 await）→ lazy encode 闭包仅在
  handoff 之后构造执行 → Success/`from_attempt_failure` 对同一 handoff permit 观测一次。
  Deadline/Cancelled 在 encode 段不观测 circuit（permit 无观测丢弃），保持 R5 既有语义。
- **circuit 拒绝的 fresh authorization 复核（RFX-04 TOCTOU #1）**：fast-gate 与 wait-stale 两处
  拒绝都不立即对调用方可见——先回调调用方提供的 `reauthorize_without_reservation` 闭包
  （closed coordinator：重读 DB writer-fence ticket、不做 reservation/编码/查询），复核
  `Ok` ⇒ 拒绝以冻结 `AdmissionBusy` 出栈；复核自身失败 ⇒ 该失败（数据库不可达 ⇒
  `Database`，保住 `AccessDenied→Restricted` 冻结映射；合同漂移 ⇒ `ProviderUnavailable`；
  generation 漂移 ⇒ `ContextChanged`）原样出栈；stage abort ⇒ `egress_stage_abort`。
  授权不可证时调用方永远看不到 Busy。
- **handoff 线性化（RFX-04 TOCTOU #2）**：最终 circuit fence 与 physical 预算消费合并进
  `authorize_provider_handoff` 的同一同步段——拒绝则预算 token 走 Drop（physical 增量
  零、transport-retry token 退还），通过则同步消费 token 并交出 `ProviderHandoffPermit`；
  coordinator 不再跨 gap 持有 admission，encode 闭包只在 permit 之后就绪，观测
  （`observe_outcome`）与 permit 一一绑定。
- **non-counting 预算 token（RFX-05）**：`reserve_provider_attempt_budget` 在 circuit gate
  **之前**返回 `ProviderAttemptBudgetToken`（`#[must_use]`，`consume_at_handoff` 按值取走并
  commit；Drop 即释放保留并退还 transport-retry token）。physical ledger 只计真实 handoff；
  pre-handoff 任何拒绝（circuit/DB/deadline/cancel/exhaustion）physical delta 为零。caps 不变：
  one-shot 2、complete-path 3；`can_begin_provider_attempt` 同时计数 committed+reserved。
- **错误映射逐面保持**：one-shot 首轮 admission 失败经 `map_egress_failure` 投影与旧
  prepare 路径一致（同走 `map_one_shot`）；complete-path `ProviderUnavailable` 仍由
  `classify_ticket_failure` 重分类，其余经 `map_provider_egress_failure`。one-shot 重试前
  `refresh_ticket_for_retry` 采纳 fresh generation（与旧行为一致）。
- **已知偏差（如实记录）**：half-open probe lease 的归还依赖既有 R5 probe-budget timeout
  （descriptor 行 `probe-budget-reclaims-abandoned-lease` 不变）而非 Drop——`ProviderCircuitToken`
  为 `Copy`，pre-handoff 被弃的 lease 不会被本阶段新增的 Drop 路径回收，但「绝不永久占用」
  不变量仍由既有 timeout 兜底成立。
- **descriptor 轮换**：token `20260818-phase2-f3-correctness-v1`；合同新增/改写
  attempt-ledger、circuit-gate、handoff 三行（§5 引文）；digest
  `94b3912f… → 745ca584…`（§5 第五行），三处 golden 显式重钉。
- **公开合同不变**：Busy/Unavailable/Restricted/Conflict 冻结映射、HTTP status、retryable、
  45s absolute 均未变；`observe_provider_circuit` 与 `begin_provider_attempt` 旧入口删除，
  全部调用方（one-shot、complete-path、canary 测试）迁入新执行器。

F3 退出门核对：rfx04（两子案：复核失败不出 Busy 且零编码、复核通过才出 Busy）、rfx05
（拒绝零 handoff、零 physical、transport-retry 退还）绿；`reliability_fix_regressions` 14/14
绿——F0 基线红测清零；`buzz-relay --lib semantic_` 128 绿；`buzz-relay --lib` 全量 972 绿
（8 个环境性失败见 §2）；`buzz-semantic-query --lib` 53 绿；三个确定性门复跑绿（§2）；
clippy/fmt 绿；`just test-unit` exit 0。

## 13. Correctness 修复 F4 交付记录（2026-08-18）

修复计划 §3.5/F4（RFX-06：两条 retry 边界没有闭合）已交付。实现要点：

- **complete-path fresh-plan 输入恒等门（§2.6 第 1–2 项）**：每个 root attempt 在构建
  `input_build` 后立即以 `RootAttemptInputIdentity::of(&inputs, &ticket)` 钉住有序输入束恒等
  ——有序 channel kinds（含每个 conditioned context coordinate）、有序精确 input digests、
  contract-bearing generation id。同 attempt 内 Provider retry 的 fresh plan 重建后重新计算
  恒等：完全相同 ⇒ 按既有 Provider retry ledger 继续本 attempt；任一维度移动 ⇒ 返回 typed
  `RootAttemptError::ReturnToOperationForInputRebuild`。该值永不进入公开错误映射：outer
  coordinator 在 restart 预算内（`attempt == 0`）记一次 generation retry 并重建 root attempt
  （旧 ticket/reservation/circuit token/egress permit 全部不复用），预算耗尽 ⇒ 浮出冻结
  `ContextSourceChanged`（409 conflict），语义与既有 churn 路径完全一致。
- **无嵌套预算**：Provider retry 与 operation restart 共享同一 request context 的同一
  attempt ledger（`begin_operation_attempt`），restart 不新开预算，caps 不变（one-shot
  physical 2、complete-path physical 3、operation attempt 2）。churn-restart 与恒等门互不
  干扰：reuse stash 只在成功 encode（即已退出循环）后填充；restart 重建 per-attempt 恒等，
  门只作用于同一 attempt 内的 Provider retry。
- **共享 bounded release confirmation（§2.6 第 3 项）**：`confirm_release_with_bounded_retry`
  进入 `semantic_query_runtime`——`begin_release_confirmation` ledger（上限 2）+ 目标窗口
  `run_stage` + 分类器 `release_confirmation_transient`（R4 item 7 原分类器移入共享层）+
  `buzz_semantic_release_retry_total` 决策计数。仅「closed、明确 no-permit/no-unknown-
  side-effect」的 ReleaseConfirmationTransient DB 错误原地重试；Denied/SnapshotChanged/
  FleetUnavailable、非 transient DB 错误、deadline/cancel（stage abort）立即返回；
  预算耗尽返回 freshest transient（终态优先，§4.5 固定优先级）或 `Busy`（fresh ledger
  不可达，fail-closed 保留）。
- **两 surface 接线**：one-shot `confirm_release` 保持 `expected_snapshot: Some(snapshot)` +
  `SnapshotClose` 窗口 + 冻结映射（Denied→403 Restricted、SnapshotChanged→409 Conflict、
  FleetUnavailable→503 Unavailable、DB→`classify_database`、DeadlineExceeded→Timeout、
  Busy→Busy）；complete-path bridge 迁入同一 helper，保持 `expected_snapshot: None` +
  `Absolute` 窗口 + 冻结映射（Denied→403 `restricted:…:authorization_changed` +
  PostflightDenied、SnapshotChanged→409 `conflict:…:snapshot_changed`、FleetUnavailable→
  503 readiness+Readiness、DB/Busy→503 postflight+PostflightUnavailable fail-closed、
  DeadlineExceeded→deadline 错误）。两 surface 均未因复用 helper 收紧或放宽任何参数。
- **测试（§9 条件式红证据的交付半）**：`reliability_fix_regressions` 新增 rfx06 两项——
  fresh-plan 恒等门用真实 canonical encoder inputs 钉住 kinds/digests/顺序/generation 四维
  （overview 移动、context 增删、generation 移动均破坏恒等）；共享 release confirmation 钉
  住两 transient 耗尽预算（freshest 出栈、无第三次调用）、complete-path 同 helper 同窗口、
  closed outcome 与非 transient DB 失败单次确认。`semantic_query_runtime` 测试新增 3 项
  （transient 原地重试、closed outcome 即时/跳过、窗口耗尽 abort）。DB 依赖的 coordinator
  集成半（真实 restart 消费、真实 permit 路径——`SemanticGraphQueryReleasePermit` 于
  buzz-db 外不可构造）按 §9 条件式记录保持 env-gated，与 F0 红证据记录一致。
- **descriptor 轮换**：token `20260818-phase2-f4-correctness-v1`；retry-fresh-plan 行补入
  有序 input 恒等门与 typed operation restart，release 行补入共享 bounded confirmation
  （§5 引文）；digest `745ca584… → 8dc7f5e8…`（§5 第六行），三处 golden 显式重钉。
- **公开合同不变**：所有冻结公开错误映射、HTTP status、retryable、attempt caps 均未变；
  每 traversal hop 零 Provider 调用不变。

F4 退出门核对：rfx06 两项绿；`reliability_fix_regressions` 16/16 绿；`buzz-relay --lib
semantic_` 132 绿；`buzz-relay --lib` 全量 978 绿（8 个环境性失败见 §2）；
`buzz-semantic-query --lib` 53 绿；三个确定性门复跑绿（§2）；clippy/fmt 绿；
`just test-unit` exit 0。RFX-06 关闭；剩余 F5（资格与文档收口）。

## 14. Correctness 修复 F5 资格收口记录（2026-08-18）

修复计划 §3.6/F5（资格与文档收口，RFX-07 的机械证据闭环）已交付。本阶段在具备 Docker 与
真实 `LLM_*` 配置的环境补齐了此前全部 "未运行" 的资格门，并按修复计划 §7 完成分层收口：

- **disposable DB 门（此前未运行）**：`just semantic-pgvector-test` 通过（脚本自管 pgvector
  0.8.5 / Postgres 17.10 容器；2048 维 vector/halfvec roundtrip、halfvec HNSW、sqlx
  roundtrip、`buzz-admin semantic preflight` `ready: true`）；`just semantic-migration-test`
  通过（semantic 迁移、desired-schema、ledger-less fresh-schema，含 real-pgvector scoped
  search 与 fleet policy matrix 用例）。
- **完整 integration 门（此前未运行）**：compose Postgres/Redis 在位后 `just test` 31 组全绿
  （112s）；此前 8 个环境性失败（`api::media`×7、`api::admin::feedback_attachment`×1）全部
  转绿，`cargo test -p buzz-relay --lib` 全量 986 通过/0 失败/31 ignored。
- **真实 Provider canary（此前未运行）**：`cargo test -p buzz-relay --lib -- --ignored
  real_provider` 2/2 通过——`real_provider_semantic_input_canary` 与
  `real_provider_reliability_canary_is_bounded_and_content_free` 对真实端点执行，仅断言
  content-free 不变量；不保存 query/vector/body。
- **`just ci`（此前未运行）**：`check` 链唯一失败为 `ci-source-contracts` 内
  release-asset-inventory 对 `desktop/src-tauri/icons/**` 的既有 tree_sha256 pin 不匹配
  （`fe7cf9d1…` vs `51403134…`）。证据表明与本修复无关：manifest（`590ce9b8b`）与 icons
  （`ca6f5ba5d`）最后一次改动均在 main 且先于本分叉，分支对二者零改动（`git diff
  main...HEAD` 为空），失败在 main 上同样成立。修复建议以独立 change 显式重钉
  `release/packaged-assets.json`，不并入本修复。其余 CI 组件逐项复跑全绿：fmt-check、
  clippy（全 workspace）、web-check、desktop-check、desktop-tauri-fmt-check、
  desktop-tauri-clippy、test-unit、desktop-test、desktop-build、desktop-tauri-check、
  desktop-tauri-test、web-build。
- **retry/recovery 故障门与最终组合 profile**：每行 closed 故障门由
  `failure_dispositions_follow_the_closed_matrix`、
  `retry_matrix_enables_exactly_the_compiled_route_rows` 与 rfx01–rfx06 回归逐行钉住并复跑
  绿；最终组合 profile 由三个确定性 characterization 门 + 全量 unit/integration + 真实
  Provider canary 共同验证，无 attempt 放大、late sign、授权/circuit 优先级倒置、snapshot
  或公开结果差异。
- **digest/manifest 轮换记录**：F1–F4 四轮日期化 descriptor 轮换与三处 golden 重钉完整记于
  §5（当前 `8dc7f5e8…`）；本阶段无代码变化，不轮换。
- **真实 fleet 切流（§7.2 项 2–3）**：无真实多 Relay fleet，old/new digest 切流、
  gate/drain/re-attest 与 binary rollback 演练未执行，切流行模板见 §5；此项保持未运行，
  不得由编译期切换或 canary 绿色替代声明。

### 分层完成结论（修复计划 §7）

- **§7.1 Correctness implementation：关闭。** 目标窗口 admission 与 partial tail 可达、
  lifecycle 状态真实、取消/关停/deadline 全 stage 可中断、Finalizing 单一同步 signer、
  unsigned 验证前移、permit 按值单次消费、circuit 前置 fresh auth 与单一 handoff 线性点、
  physical 只计真实调用、fresh-plan 恒等门与无嵌套 restart、两类 release policy 与有界
  transient retry、四 operation 差分合同（Phase 1 门复跑）全部成立；deterministic、
  semantic DB、migration、unit、integration 与真实 Provider canary 门绿色。可声明
  **"Phase 2 correctness implementation 已交付"**。`just ci` 的唯一红色为上述与语义运行时
  无关的 main 既有 release-asset pin（附证据与独立修复建议），不构成本计划的 correctness
  缺口。
- **§7.2 Deployment qualification：未关闭。** 真实 fleet 同质 digest 切流与 rollback 演练、
  rollback 窗口 owner/起止/保留结论均未执行/记录。状态保持 **"correctness implementation
  已交付、deployment qualification 未完成"**，不得使用无修饰的 "Phase 2 已完整交付 /
  具备部署资格 / production-ready" 表述。
- 第三阶段统一资源治理（bounded queue、fairness、capacity、fleet-shared circuit）保持未交付。
