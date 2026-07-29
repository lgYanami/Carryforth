# 角色连续性集成验收报告

> 结论：**不通过**
>
> 原因：真实 Relay/CLI 纵向测试在 v1→v2 cutover 后稳定产生一份无法由共享 SDK
> 组装的 Project View 快照。该问题阻断第一个 Human–Agent 黄金路径，属于 P1。
>
> 本报告对应
> [角色连续性集成验收方案](./integration-acceptance-plan.md)。验收期间未修改生产实现。

## 1. Run 信息

| 项目 | 值 |
|---|---|
| Run ID | `F724D000-31C7-4F8A-8FE8-4450F958FB9F` |
| Commit | `8faf6917b81485d17cb15f5eabe3cce118877bd4` |
| Branch | `feat/project-view-v0` |
| 开始时间 | `2026-07-29T14:02:12Z` |
| 结束时间 | `2026-07-29T14:10:24Z` |
| Rust | `rustc 1.95.0` / `cargo 1.95.0` |
| Node / pnpm | `v24.14.0` / `11.4.0` |
| Docker | `29.5.3`，Compose `5.1.4` |
| 服务 | `buzz-postgres`、`buzz-redis`、`buzz-minio` 均 healthy |

执行前 `BUZZ_AUTH_TAG`、`BUZZ_RELAY_URL`、`BUZZ_PRIVATE_KEY`、`DATABASE_URL` 和
`REDIS_URL` 均未设置；每个测试进程仍显式移除了这些外部变量。验收没有连接
staging/production。

执行前工作区仅有以下两个未跟踪文档：

```text
docs/lora/stage/role/integration-acceptance-plan.md
docs/lora/stage/role/log.md
```

`log.md` 未被修改或纳入验收产物。

## 2. 总体判定

| 层级 | 结果 | 说明 |
|---|---|---|
| L0 领域与协议 | PASS | Project View、core、SDK、Relay、CLI 共 93 项专项测试通过 |
| L1 数据库与迁移 | PASS | 18 项事务测试、6 项迁移/schema-drift 测试通过 |
| L2 Relay 与真实 CLI | FAIL | v1 流程通过；v2 cutover 后第一次 Agent 快照读取失败 |
| L3 Desktop | BLOCKED | JS、Tauri、build、28 项 mock Playwright 通过；真实 Relay smoke 被 L2 阻断 |
| L4 ACP 与 Runtime | BLOCKED | ACP/component tests 通过；真实 Relay + ACP + supervisor 纵向链路无有效 Role 前置状态 |
| L5 真实模型 smoke | SKIPPED | 可选项，且 L2 已失败 |
| L6 全仓回归 | SKIPPED | 方案规定专项通过后才执行 `just ci`；本轮不满足前置条件 |

按照验收方案，只要任一必选纵向一致性场景失败就不能判定通过。L2 已出现稳定 P1，
因此不能用其余绿色单元测试降级为“有条件通过”。

## 3. 已执行门禁

### 3.1 Project View 专项

命令：

```bash
just project-view-test
```

结果：

| 组件 | 通过 | 失败 |
|---|---:|---:|
| `buzz-project-view` unit/properties/relations/wire | 61 | 0 |
| `buzz-core` Project View | 2 | 0 |
| `buzz-sdk` Project View | 9 | 0 |
| `buzz-relay` Project View | 8 | 0 |
| `buzz-cli` Project View | 13 | 0 |
| PostgreSQL transaction/concurrency | 18 | 0 |
| migration/schema drift | 6 | 0 |
| 真实 Relay/CLI E2E | 0 | 1 |

真实 E2E 在独立数据库、随机 Community host 和随机端口上执行。v1 阶段已经完成：

- WS 初始化；
- HTTP Agent mutation；
- revision-pinned 分页；
- 真实 Human `buzz` CLI 写入；
- live membership revoke；
- stale page 与 stale mutation conflict；
- v1 Role 创建；
- v1→v2 cutover；
- owner 发出 v2 Role offer。

随后 Agent 执行 `roles proposals --status open` 时，共享 SDK 拒绝快照。

### 3.2 ACP 与 CLI

```bash
cargo test -p buzz-acp --lib
cargo test -p buzz-cli --lib
```

结果：

- ACP：613 passed，0 failed；
- CLI：264 passed，0 failed。

其中 Role Brief cache/refresh、dynamic runtime fence、supervisor state、binding/assignment
convergence 等 component tests 均通过。它们没有覆盖本次失败的真实 cutover 投影组合，
因此不能替代 L2/L4。

### 3.3 Desktop

```bash
just desktop-test
just desktop-tauri-test
pnpm -C desktop build:e2e
cd desktop
pnpm exec playwright test --project=smoke tests/e2e/project-view.spec.ts
```

结果：

- Desktop JS：3507 passed，0 failed；
- Tauri：1644 passed，14 ignored；
- Tauri diagnostic integration：3 passed；
- E2E frontend build：PASS；
- Project View Playwright：28 passed，0 failed。

Playwright 覆盖 verified map、Role/Work Inspector、历史分页、offer、Checkpoint/Handoff、
Human/Agent 交替修改、replacement、revision invalidation、可信读取失败、断线 stale 状态和
Community 隔离。但其数据来自 E2E mock bridge，不能证明真实 Relay cutover 快照可用。

## 4. P1 缺陷

### 4.1 现象

真实 CLI 返回退出码 4：

```json
{
  "error": "error",
  "message": "Project View v2 integrity error: invalid Project View projection: ordinary object body disagrees with its projection revision",
  "retryable": false
}
```

首轮失败后只进行了一次带 backtrace 和日志归档的诊断复现。第二次结果相同，触发位置为：

```text
e2e_project_view.rs:779
roles proposals --status open
```

诊断日志：

```text
test-results/role-acceptance-F724D000/project-view-e2e-diagnostic.log
SHA-256 9179742551211fd4ec96298a8eb504e9f1e46d1dbdfa9f08e45b5af8f9c1a8b6
```

### 4.2 已确认的数据路径

cutover 读取既有 canonical object，保留 object body 中原来的 `project_revision`：

- `crates/buzz-db/src/project_view_v2.rs:924`
- `crates/buzz-db/src/project_view_v2.rs:946`

随后用 cutover 的 `next_revision` 构造所有新 generation 的外层投影：

- `crates/buzz-db/src/project_view_v2.rs:960`
- `crates/buzz-db/src/project_view_v2.rs:1016`

cutover 内部验证只比较外层 `parsed.project_revision == next_revision`，没有比较
`parsed.object.project_revision`：

- `crates/buzz-db/src/project_view_v2.rs:1028`
- `crates/buzz-db/src/project_view_v2.rs:1038`

共享 verified assembler 明确要求 object body revision 与该 object projection revision
相等：

- `crates/buzz-sdk/src/role_brief.rs:363`

因此 cutover 事务可以成功、NIP-11 可以宣告 v2 ready、owner 也可以提交不依赖完整快照的
offer，但 Agent/Human 一旦读取完整 v2 snapshot，就会被 SDK 正确地 fail closed。

### 4.3 影响

- Human 与 Agent 无法在 cutover 后读取同一份可验证 Project View；
- Agent 无法查看或接受 Role Proposal；
- Assignment、Work、Checkpoint、Handoff 和 Role Brief 的真实纵向路径无法开始；
- Desktop 连接真实 Relay 后同样应拒绝该快照；
- 现有数据库 cutover 测试存在覆盖缺口：验证了 event envelope 和 schema，却没有用共享
  `VerifiedRoleBriefSnapshot` 组装完整 cutover 结果。

该缺陷没有造成越权或静默接受坏状态；客户端按设计 fail closed。但它使 Role Continuity
主流程不可用，所以分级为 P1，而不是 P2 或 ENV。

### 4.4 修复验收要求

本轮不选择具体修复方案。修复必须先对齐 cutover 中“未变化对象的 body revision、投影
revision、updated_at 与 source”语义，然后至少补充以下回归：

1. cutover 事务提交前，用与 CLI/ACP 相同的共享 assembler 验证完整 snapshot；
2. 对包含多个不同历史 `project_revision` 的 Profile/Goal/Plan/Issue/Work/Role 执行
   cutover；
3. cutover 后立即通过真实 CLI 读取 `project-view get`、`roles proposals` 和
   `roles current`；
4. offer 提交后再次读取，证明未变化 ordinary heads 仍能与新 meta revision 组装；
5. 修复后从新的 commit 重新执行整份集成验收，而不是只重跑失败断言。

## 5. 黄金路径结果

| 故事 | 结果 | 停止位置 |
|---|---|---|
| E2E-01 Human 建项目、Agent A 承担 Role | FAIL | owner offer 已接受；Agent A 无法读取 Proposal |
| E2E-02 Agent A 追加 Checkpoint/Handoff | SKIPPED | 没有可验证的 active Assignment |
| E2E-03 Human 原子替换 A→B | SKIPPED | E2E-01 未形成规范前置状态 |
| E2E-04 Leader 与 Community 权限一致 | SKIPPED | 无法建立可信 Leader Assignment |

不能通过直接写数据库或注入 mock snapshot 绕过 E2E-01，因为那会跳过本轮需要验收的
canonical cutover 与 Relay projection 路径。

## 6. 专项矩阵摘要

| 范围 | 结果 | 说明 |
|---|---|---|
| PV-01～PV-07 | PASS | L0/L1 与 v1 真实 Relay 场景通过 |
| PV-08 v1→v2 cutover | FAIL | generation/schema 切换成功，但完整投影内部 revision 不一致 |
| RC-01～RC-06 | SKIPPED | component tests 通过；真实 Role 链路被 PV-08 阻断 |
| WORK-01～WORK-04 | SKIPPED | component tests 通过；无可信 Assignment 前置状态 |
| CT-01～CT-06 | SKIPPED | component/Playwright tests 通过；真实 append/history 链路未开始 |
| ACP-01～ACP-08 | SKIPPED | unit/mock 通过；真实 Brief 无法从坏快照生成 |
| RT-01～RT-12 | SKIPPED | DB/ACP component tests 通过；真实 Relay + ACP + supervisor 未编排 |

“SKIPPED”不代表对应能力失败，而是本轮没有获得方案要求的纵向证据，因此不能记为 PASS。

## 7. 清理结果

- 两次 E2E Relay 进程及其随机监听端口均已停止；
- 两次 `buzz_pv_e2e_*` 临时数据库均由测试脚本删除；
- Playwright HTTP server 已停止；
- 共享 `buzz-postgres`、`buzz-redis` 和 `buzz-minio` 保持运行且未被重置；
- 执行前记录的 8 个既有 `buzz_pv_*` 数据库仍原样存在；
- 没有遗留本轮数据库、Relay 或端口；
- 诊断日志保存在 gitignored `test-results`；
- 仓库生产代码无修改。

报告生成前的工作区状态为：

```text
?? docs/lora/stage/role/integration-acceptance-plan.md
?? docs/lora/stage/role/integration-acceptance-report.md
?? docs/lora/stage/role/log.md
```

## 8. 最终结论

角色连续性目前不能判定为完整交付。领域状态机、数据库原子性、权限 fencing、ACP/Runtime
组件和 Desktop mock 交互已经有较强覆盖，但真实 v1→v2 cutover 产生的 snapshot 无法被
Human/Agent 共用的 SDK 读取。

下一步应先修复并回归 PV-08；修复提交完成后重新执行 L0～L6、E2E-01～E2E-04 和
ACP/Runtime 纵向编排，再给出新的发布结论。
