# 角色连续性集成验收报告

> 结论：**有条件通过**
>
> Project View、Role Continuity、ACP Role Context、Runtime supervision、数据库事务、
> 真实 Relay/CLI 纵向链路以及全仓回归均通过；没有发现未关闭的 P0/P1。
>
> 唯一未完成的必选观察项是：本轮无法自动驱动真实 Tauri WebView 连接临时 Relay，
> 因而没有把“真实 Desktop dev build 中点击 View 并观察 live Checkpoint”记为已验证。
> Desktop 的 JS、Tauri native boundary、E2E build 和 28 项 mock Playwright 均通过，
> 但这些证据没有被冒充为真实 WebView smoke。

本报告对应
[角色连续性集成验收方案](./integration-acceptance-plan.md)。

## 1. Run 信息

| 项目 | 值 |
|---|---|
| Run ID | `505CCAB4-B34F-4563-9867-96F29F714FB6` |
| 初始修复基线 | `651afb227307ed4b5b31ba9f3e295208242d9cfe` |
| 最终验收树 | `504ef027ce3c6d49aaa4a6cc2f1060f3c04365bc` |
| Branch | `feat/project-view-v0` |
| 开始时间 | `2026-07-29T22:38:00+08:00` |
| 结束时间 | `2026-07-29T23:34:49+08:00` |
| Rust | `rustc 1.95.0` / `cargo 1.95.0` |
| Node / pnpm | `v24.14.0` / `11.4.0` |
| Docker | `29.5.3`，Compose `5.1.4` |
| 服务 | `buzz-postgres`、`buzz-redis`、`buzz-minio` 均 healthy |
| 本地证据目录 | `test-results/role-acceptance-505CCAB4/` |

初始基线 `651afb2` 修复了上一轮发现的 v1→v2 cutover head revision 缺陷，并补充共享
assembler 回归。专项门禁和真实 Role/ACP/Runtime 纵向编排在该提交上执行。

最终 `just ci` 首次运行时发现 `desktop/src-tauri/src/managed_agents/runtime.rs` 超过既有
文件行数门禁。函数实现随后只按职责移动到 `runtime/metadata.rs`，函数签名和行为均未
改变，并以 `504ef027` 独立提交。相关 3 项 targeted test 和完整 `just ci` 在与最终提交
字节一致的工作树上重新执行并通过。

执行前外部 `BUZZ_AUTH_TAG`、`BUZZ_RELAY_URL`、`BUZZ_PRIVATE_KEY`、`DATABASE_URL` 和
`REDIS_URL` 均未设置；临时进程也显式使用自己的 Relay、数据库、Community host 和密钥。
验收没有连接 staging/production。

执行前工作区仅有用户自己的未跟踪文件：

```text
docs/lora/stage/role/log.md
```

该文件未被读取、修改、暂存或提交。

## 2. 总体判定

| 层级 | 结果 | 说明 |
|---|---|---|
| L0 领域与协议 | PASS | Project View domain/core/SDK/Relay/CLI 共 93 项专项测试通过 |
| L1 数据库与迁移 | PASS | PostgreSQL 19 项，migration/schema-drift 6 项通过 |
| L2 Relay 与真实 CLI | PASS | 现有真实 E2E 通过；新增 Human–Agent 纵向 E2E 通过 |
| L3 Desktop | CONDITIONAL | JS、Tauri、build、28 项 Playwright 通过；真实 WebView smoke 未执行 |
| L4 ACP 与 Runtime | PASS | 确定性 child + 真实 Relay + ACP + supervisor 纵向编排通过 |
| L5 真实模型 smoke | SKIPPED | 可选项；本轮按方案使用确定性 fake ACP child |
| L6 全仓回归 | PASS | 最终树完整 `just ci` 通过 |

本轮所有规范状态、安全 fence、原子性、恢复和故障结束条件均通过。L3 剩余项属于无法由
当前自动化驱动的人工 UI smoke，不影响已经取得的后端与 Agent 正确性证据，因此结论为
“有条件通过”，而不是“不通过”或无条件“通过”。

## 3. 自动化门禁

### 3.1 Project View

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
| PostgreSQL transaction/concurrency | 19 | 0 |
| migration/schema drift | 6 | 0 |
| 独立 Relay + 真实 CLI E2E | 1 | 0 |

上一轮失败的 v1→v2 cutover 现在能用 Human、CLI 和 ACP 共用的 verified assembler 组装。
数据库回归同时覆盖“保留已有 head revision”和“完整 verified snapshot”。

### 3.2 ACP 与 CLI

```bash
cargo test -p buzz-acp --lib
cargo test -p buzz-cli --lib
```

结果：

- ACP：613 passed，0 failed；
- CLI：264 passed，0 failed。

这些测试覆盖 Role Brief cache/refresh、candidate/assigned/unavailable、dynamic runtime
fence、runtime supervisor state、binding/assignment convergence 和完整 Role history
pagination。

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
Human/Agent 交替修改、replacement、revision invalidation、可信读取失败、断线 stale 状态
和 Community 隔离。其后端是 E2E mock bridge，所以真实 WebView smoke 仍单独列为限制。

### 3.4 全仓回归

最终树执行：

```bash
just ci
```

结果：PASS。

覆盖 Rust workspace 与 Tauri 的 fmt/clippy，Desktop/Web/Mobile 检查，Rust/Desktop/Tauri/
Mobile 测试，以及 Desktop/Web 构建。最终 Tauri 单元结果为 1644 passed、14 ignored，
Mobile 为 568 passed、1 skipped。

## 4. Human–Agent–ACP–Runtime 真实纵向验收

新增的临时验收 harness 使用：

- 独立 PostgreSQL database；
- 随机 Community host 和 Relay port；
- Human Owner、Leader、Agent A、Agent B、Supervisor 的独立临时身份；
- 真实 `buzz-relay`、真实 `buzz` CLI、真实 `buzz-acp`；
- 确定性的本地 fake ACP child；
- 真实 runtime fence 文件、SIGKILL、restart、lease 与 recovery deadline。

最终结果：

```text
role_continuity_human_agent_acp_and_runtime_vertical ... ok
1 passed; 0 failed; finished in 134.53s
```

临时 Rust 测试源码在执行后删除，没有纳入生产实现；运行脚本和长日志仅保留在 gitignored
的 `test-results/role-acceptance-505CCAB4/`。

### 4.1 关键坐标

| 对象 | 验收值 |
|---|---|
| Project | `00000000-0000-4000-8000-00000000a11c` |
| 最终 project revision | `20` |
| 普通 Role | `fea3af09-6e5e-411e-910a-97509fd0be3d` |
| Leader Role | `eeeba121-0fc4-4802-ad45-1d1572f093a1` |
| Agent A Assignment | `72eb589a-bdc5-413e-99d0-ef5e8b943d0a` |
| Agent B Assignment | `126dfaf1-2d0f-44e7-8908-3240814cdf98` |
| Agent A Commitment | `8c19d2d0-09c1-48da-ba0d-6127df174569` |
| Agent B Commitment | `5c723321-81fa-4642-a40e-ecf9c56fe3cb` |
| 首个 Checkpoint | `f01b4207-14f8-40c7-a972-65d4b230eb40` |
| 修正 Checkpoint | `8de1b41f-e874-4ffe-b436-794fb49f06f4` |
| member Handoff | `ab869cc0-617c-4018-9f2c-bc8a15a0a44b` |
| 恢复 Runtime | `e11b044c-68ee-4082-a930-1f21ab6ebabf`，epoch `1→2` |
| 恢复耗尽 Runtime | `c5c9faae-577b-4ab4-8561-6f88dfba3fd2`，epoch `2` |
| 健康 sibling Runtime | `163a09cc-1ddc-4016-adbd-f716987bf972` |
| 自动结束 system change 数 | `1` |

### 4.2 已验证行为

1. v1 初始化 Profile/Goal/Issue/Work 后完成 v2 cutover；既有 Community admin 被映射为
   active Leader Assignment，membership 仍为 admin。
2. Owner offer Agent A；A 接受并承担唯一 active Assignment；A 自行 end 被拒绝。
3. Owner 指定 responsible Work；A 接受形成 Commitment。
4. A 追加结构化 Checkpoint、context-only Handoff 和 superseding correction；
   Handoff 不结束 Assignment。
5. Role history newest-first、无重复；跨 Role cursor 被拒绝；直接 UPDATE Checkpoint 和
   DELETE Handoff 被数据库 append-only guard 拒绝。
6. Agent A 的新 ACP session 收到完整 `[Role Brief]`，之后收到 compact
   `[Role Binding]`；子进程只有 mode `0600` fence path，没有 supervisor 私钥或 state
   path。
7. Owner 原子替换 A→B；A 收到 candidate Brief，旧 fence 删除，旧 Assignment 写入被
   Relay 拒绝。
8. B 的 Brief 包含 waiting-for-continuation、最新修正 Checkpoint、member/system
   Handoff；B 显式接受 Work 后形成自己的 Commitment。
9. Leader 只能用精确 active Assignment 治理普通 Role；同级 admin Role 操作返回稳定
   `owner_required`；Owner 结束 Leader 后 membership 原子降为 member，Owner 保持 owner。
10. stale authorizer 不会部分消费 proposal；stale Leader fence 被拒绝。
11. binding revoke 动态删除 runtime fence；重新注册无需重启 ACP 即产生新的可信 runtime。
12. SIGKILL 后以相同 supervisor state 重启，同一 runtime ID 从 epoch 1 恢复为 epoch 2。
13. 拷贝的旧 fence 即使带 `BUZZ_MANAGED_AGENT=1`，role-bearing 写入仍被拒绝。
14. graceful stop、普通离线和 opt-out 不自动结束 Assignment。
15. recovery 尝试耗尽后 runtime 进入 unavailable；仍健康且持续续租的 sibling runtime
    阻止自动结束。
16. sibling 停止后，supervisor heartbeat 触发一次且仅一次
    `end_unrecoverable_assignment`：Assignment/Commitment 原子结束，生成 system
    Handoff，membership 同步。

## 5. 黄金路径

| 故事 | 结果 | 说明 |
|---|---|---|
| E2E-01 Human 建项目、Agent A 承担 Role | PASS / Desktop ENV | DB、Relay、CLI、ACP 全链通过；真实 WebView 卡片观察未执行 |
| E2E-02 Agent A 追加 Checkpoint/Handoff | PASS / Desktop ENV | append-only、Brief、history 全链通过；真实 WebView live 观察未执行 |
| E2E-03 Human 原子替换 A→B | PASS / Desktop ENV | 原子替换、旧 fence、B 接续通过；真实 WebView 中间态观察未执行 |
| E2E-04 Leader 与 Community 权限一致 | PASS | 精确 Assignment fence、owner-only、membership 降级均通过 |

这里的 `Desktop ENV` 不是后端或 Desktop 自动化失败，而是明确保留的真实 Tauri 人工观察项。

## 6. 专项矩阵摘要

| 范围 | 结果 | 证据摘要 |
|---|---|---|
| PV-01～PV-08 | PASS | unit/DB/migration/真实 Relay CLI；cutover verified snapshot 已回归 |
| RC-01～RC-06 | PASS | 状态机与 DB guard，加真实 self-end、replacement、owner/Leader stale fence |
| WORK-01～WORK-04 | PASS | 非 responsible 拒绝、Commitment 原子结束、B 显式接续 |
| CT-01～CT-06 | PASS | 真实 append/history/correction，加领域 reference 与 DB append-only guard |
| ACP-01～ACP-06 | PASS | 真实 candidate/full/compact/unavailable/dynamic refresh，加 component tests |
| ACP-07 | PASS | component native-steer 覆盖，加真实 stale Assignment/Runtime 写入拒绝 |
| ACP-08 | PASS | component Community reset 覆盖；跨 identity/Relay supervisor state fail closed |
| RT-01～RT-12 | PASS | 真实 ACP child、fence、epoch、kill/restart、sibling health 与自动结束 |

上述 PASS 使用方案要求的组合证据：真实 Relay/ACP 纵向链路证明授权边界，component/
property/DB tests 补足难以在单条黄金路径中穷举的 closed-schema 和负向组合。

## 7. 缺陷、限制与验收编排校正

### 7.1 未关闭问题

- P0：无；
- P1：无；
- P2：无未关闭项；
- ENV：真实 Desktop dev build + Tauri WebView + 临时 Relay 的人工/半自动 smoke 未执行；
- 可选 L5：真实外部模型 smoke 未执行。

### 7.2 验收中发现并关闭的门禁问题

首次 `just ci` 在 Desktop 文件大小检查停止：

```text
src-tauri/src/managed_agents/runtime.rs: 2251 lines (limit 2216)
```

没有提高限制。两个 runtime metadata helper 被移动到独立子模块，主文件降为 2197 行。
targeted tests 和第二次完整 `just ci` 通过。修复提交：

```text
504ef027 refactor(desktop): split runtime metadata helpers
```

### 7.3 临时 harness 校正

正式纵向断言编排期间保存了 9 份失败日志。逐项定位后均为验收 harness 本身的问题，而非
生产实现缺陷：

1. 把 admin 文本输出当 JSON 解析；
2. 临时 hostname 含不合法下划线；
3. 测试 heartbeat 低于协议最小值；
4. max-turn 小于默认 idle-timeout；
5. prompt 断言使用了错误的 JSON 文本形态；
6. 权限断言没有匹配稳定错误码 `owner_required`；
7. stale runtime CLI 漏传 `BUZZ_MANAGED_AGENT=1`，因更早的 acting-assignment gate 被拒；
8. 重跑残留 state 被正确识别为其他 identity/Relay，并 fail closed；
9. 等待 recovery deadline 时没有给健康 sibling 续租，导致它按设计变为 unavailable。

校正后的最终 harness 一次完整通过。第 8 项还提供了 supervisor state 不跨身份/Relay
复用的额外 fail-closed 证据；第 9 项改为每 5 秒真实续租后，准确验证健康 sibling 会
阻止自动结束。

## 8. 关键证据与校验和

| 证据 | SHA-256 |
|---|---|
| `project-view-test.log` | `cd3b7d4b3549195ef45f020d4a5c5e14a8a0761e72cd59650231cb973a1590ce` |
| `buzz-acp-lib.log` | `e90e53f5d760e80f8952ef92f86685ff1238ee828081beae2894e85d63252223` |
| `buzz-cli-lib.log` | `f8048a858ffd6b2b74503b8af6113bb479e90316af0e8e8db2a03b6c8390a593` |
| `desktop-test.log` | `030a4dae5dcc43a5a8720b0e8507d0de583ae920261279765b44481cb9841f71` |
| `desktop-tauri-test.log` | `b7e11ae405bf54e35fb0e6242494b7ff9a4ff504226d05c011899768838800fe` |
| `desktop-build-e2e.log` | `b441d9ed94af5527125e351b60cc4ea7615cae53d83566992efe2c3bc9e8ec8c` |
| `playwright-project-view.log` | `15dd18f863ef1845fcdd490e133027b8173eb7883584de7f38f73a069d511e0e` |
| `role-vertical.log` | `23f1fe6f50600bd6c0f74aed2de83b610fda6ef73c322bc17595922355819a7a` |
| `role-vertical-relay.log` | `0e6639a919271efc8747866a5983a7a9a32c0ce547e45596eb5ee0e0b9c51211` |
| `vertical-evidence.json` | `86146f9467137469beed9146e8a4ea9e282924fa4b7ca69bcc1f6315fa952d99` |
| `just-ci.log` | `b566407c9e73151f3d72475274f95c1e62f5afdc2edb6301cdc80f4d57fde194` |

这些文件位于 `test-results/role-acceptance-505CCAB4/`，目录已被 gitignore。报告不包含
私钥、NIP-98 header、supervisor secret 或可复用 token。

## 9. 清理结果

- 没有遗留 `buzz-relay` 或 `buzz-acp` 验收进程；
- 没有遗留 `buzz_role_accept_*` 数据库；
- 执行前记录的 8 个 `buzz_pv_*` 数据库仍全部存在；
- 共享 `buzz-postgres`、`buzz-redis`、`buzz-minio` 保持运行且未被重置；
- 临时 Rust 验收源码已删除；
- 长日志、fake child、无效 fence 和运行脚本只保留在 gitignored `test-results`；
- 用户的 `docs/lora/stage/role/log.md` 保持未跟踪且未修改；
- 生产变更只有独立提交的 runtime metadata 文件拆分；验收报告已确认纳入收口提交。

报告提交后的预期工作区状态：

```text
?? docs/lora/stage/role/log.md
```

## 10. 最终结论

角色连续性 v0 的核心目标已经获得集成证据：

- Human 与 Agent 通过同一 verified Project View 读取和修改 Role/Work；
- 连续性归属于 Project-owned Assignment、Commitment、Checkpoint 和 Handoff，而不是
  某个 Agent session；
- Agent A→B 后，A 的旧 Assignment/Runtime 无法写，B 能从项目状态继续；
- Leader 权限、Community membership 和 Assignment 原子一致；
- ACP session、binding、Runtime epoch 和 supervisor 恢复均 fail closed；
- 自动 `unrecoverable` 只在全部条件满足后执行一次原子 system change。

因此本轮可以判定为**有条件通过**。补做一次方案 8.3 的真实 Desktop Relay smoke 并通过
后，可以把结论升级为“通过”；如果期间没有相关代码变化，不需要重跑已经通过的后端、
ACP 和 Runtime 纵向链路。
