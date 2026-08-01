# Project Document 阶段 5 v3 cutover 与 canary

执行边界：只允许一个或事先声明的小规模 Community cohort。第一方图形客户端范围仅为
Desktop；Mobile 与 Web 不在阶段 5 的兼容性或 rollout 声明内。`project_context_enabled` 在整个
阶段保持 `false`。

本文是 operator runbook，也是阶段 5 证据清单。代码、单元测试、Desktop mock E2E 可以在本地完成；
真实运行验收同样可以在本地完成，但必须启动真实Relay、PostgreSQL、Redis和ACP进程，并使用真实签名
事件执行完整maintenance/cutover与empty-state canary，不能由mock替代。项目未发布意味着不需要生产
cohort或历史客户端兼容承诺，不意味着可以省略本地真实运行。若未来针对已部署环境执行，则还必须由
拥有目标Community、数据库、Relay signer与发布权限的operator授权。

## 1. 发布与 cohort 前置条件

在选择 Community 前，先部署同时支持 v2/v3 的 Relay、`buzz-admin`、`buzz` CLI、ACP harness 和
Desktop。不要先切换 schema，再补客户端。

为每个候选 Community 保存一份不含 Secret 的声明记录：规范化 host、owner、reviewer、维护窗口、
预期 Relay pubkey、ACP maintenance protocol（本阶段为 `1`）、旧客户端观察版本，以及备份位置。
确认：

- Project View 当前为 enabled、schema v2 且 structural ready；
- Project Documents 已启用并通过 verify；
- 每个 active managed-Agent Assignment 都有唯一、未撤销的 supervisor binding；
- 全部 ACP 已升级并保持 long-lived maintenance polling；
- 没有未处置的 security invalidation；
- 已完成数据库与 Relay signer 的既有备份流程；
- review 目录为 owner-only，且不位于同步盘或公开 artifact 中。

```bash
buzz-admin project-view status --community "$COMMUNITY"
buzz-admin project-document status --community "$COMMUNITY"
```

任何前置检查失败都应停止，而不是通过关闭校验、修改数据库或代替 supervisor ACK 继续。

## 2. reviewed Resource → Guide 准备

1. 导出绑定 exact schema-v2 base 的本地 draft：

   ```bash
   install -d -m 700 "$REVIEW_DIR"
   buzz-admin project-view v3 resources export \
     --community "$COMMUNITY" \
     --out "$REVIEW_DIR"
   ```

2. reviewer 使用 Desktop Documents 为每个 active legacy Resource 创建或选择一个 active Guide。
   Guide 正文记录经审阅的访问方法、locator 和使用限制；不要把 Secret 写进 Document。
3. 在 draft 中填写每项 locator-free `resource_kind`、可选 summary 与 exact
   `guide_document_id`。未知 `resource_kind` 是合法值，不得由客户端私自归类或丢弃。
4. Human reviewer 通过成员 CLI 对当前 v2 Resource、Guide head/revision 与 mapping digest 做 detached
   approval：

   ```bash
   buzz project-view v3 resources approve \
     --manifest "$REVIEW_DIR/resource-mapping-draft.json" \
     --out "$REVIEW_DIR/resource-mapping-reviewed.json"
   ```

5. operator 在 DB control plane 重新计算并固化所有 canonical evidence：

   ```bash
   buzz-admin project-view v3 resources validate \
     --community "$COMMUNITY" \
     --manifest "$REVIEW_DIR/resource-mapping-reviewed.json"
   ```

保留 owner-only draft、reviewed manifest、文件 digest、数据库备份坐标和审批者身份。它们与数据库中的
immutable staging/committed child ledger、cutover receipt 一起构成 migration archive。legacy locator
只能继续存在于 reviewed Guide、受限 v2 history/backup 和该 archive 中，不能进入 v3 Resource authority。

## 3. exact maintenance、freeze 与 cutover

先开始一个绑定 stable signer 和最低 ACP 协议的 exact epoch：

```bash
buzz-admin project-view maintenance begin \
  --community "$COMMUNITY" \
  --required-client-protocol-version 1 \
  --expected-pubkey "$RELAY_PUBKEY" \
  --idempotency-key "$BEGIN_KEY"
```

从 receipt 固定 `maintenance_epoch`，此后所有命令都使用同一个值。`begin` 会立即隐藏 Project View
capability并停止普通写入。ACP 必须按以下不可交换的顺序收敛：

```text
latch epoch / withdraw Runtime fence / pause renewal
  → stop admission and cancel+join all lifecycle work
  → reap active and idle Agent child process groups
  → ACK every exact Runtime baseline
  → durable read-back
  → ACK the Assignment as quiesced
  → final durable read-back
```

用 readiness 查看具体阻断项；自动化或发布流水线使用 `ack-probe` 的退出码，不解析日志文本：

```bash
buzz-admin project-view maintenance readiness \
  --community "$COMMUNITY" --epoch "$EPOCH" --max-poll-age-seconds 30

buzz-admin project-view maintenance ack-probe \
  --community "$COMMUNITY" --epoch "$EPOCH" --max-poll-age-seconds 30
```

只有 `ready_to_freeze = true` 且 `ack-probe` 返回 0 时才能 freeze：

```bash
buzz-admin project-view maintenance freeze \
  --community "$COMMUNITY" --epoch "$EPOCH" \
  --idempotency-key "$FREEZE_KEY"
```

freeze 后再次 validate reviewed manifest；cutover 本身还会在事务中重验 exact manifest、signature、
Resource base、Guide head/revision/content、membership、Relay signer 和 maintenance epoch：

```bash
buzz-admin project-view v3 resources validate \
  --community "$COMMUNITY" \
  --manifest "$REVIEW_DIR/resource-mapping-reviewed.json"

buzz-admin project-view v3 cutover \
  --community "$COMMUNITY" \
  --manifest "$REVIEW_DIR/resource-mapping-reviewed.json" \
  --maintenance-epoch "$EPOCH" \
  --idempotency-key "$CUTOVER_KEY" \
  --relay-key-file "$RELAY_KEY_FILE" \
  --expected-pubkey "$RELAY_PUBKEY"

buzz-admin project-view maintenance verify \
  --community "$COMMUNITY" --epoch "$EPOCH" \
  --idempotency-key "$VERIFY_KEY" \
  --expected-pubkey "$RELAY_PUBKEY"

buzz-admin project-view maintenance resume \
  --community "$COMMUNITY" --epoch "$EPOCH" \
  --idempotency-key "$RESUME_KEY" \
  --expected-pubkey "$RELAY_PUBKEY"
```

`resume` 是验证后的显式恢复动作，并原子恢复 eligible Community 的 v3 capability；不要在 frozen
期间用普通 `enable` 绕过状态机。若 cutover 前决定停止，可对 exact epoch 执行 `maintenance abort`。
cutover receipt 一旦提交就不可 rollback；后续只能在 frozen 状态使用 typed repair/reproject、重新
verify，再向前 resume。Redis fan-out 在 commit 后失败时同样保持 frozen，不能把已提交 cutover 当作
失败重跑。

## 4. 恢复后的验收

恢复后验证 NIP-11 只广告 `buzz-project-view-v3`，不广告 `buzz-project-view-v2` 或
`buzz-project-context-v1`。`project_context_enabled` 必须仍为 `false`，任何 nonempty Context write
都必须失败。

```bash
buzz --format compact project-view get
buzz --format compact project-view get-object resource "$RESOURCE_ID"
buzz resources guide "$RESOURCE_ID" --content-only
```

逐项核对：

- 每个 active Resource 都解析到 active Guide；
- converted Resource 的 object revision 恰好 `+1`，updated-by/provenance 指向 detached reviewer；
- v3 Resource 不含 `locator` 或 `resource_type`；
- 至少一个未知 `resource_kind` 能在 CLI、Tauri、Desktop 与 Resource → Guide 链路中原样往返；
- Desktop 只通过 native verified Document API 打开 Guide；
- managed ACP 解析 strict base `RoleBriefV3`，Context 状态为 `not_advertised_empty`，Document metadata
  状态为 `not_required`；
- maintenance 前后的 Runtime ID/epoch 不同，旧 fence 文件未被重新发布；
- v2-only 客户端明确显示 unsupported，且不能写入；不得为它恢复 v2 capability或 dual write；
- migration archive 与备份仍可读取，但不会通过普通 v3 API暴露 legacy locator。

观察窗口内出现 integrity error、旧 fence、漏 Guide、错误 attribution、Context 广告、dual write 或
无法解释的 unsupported 以外旧客户端行为，都应停止扩大 cohort。

## 5. empty-state direct-v3 canary

选择一个独立、disabled、uninitialized、只有直接 Human owner/admin 的空 Community。它不走 legacy
manifest或 maintenance cutover：

```bash
buzz-admin project-view prepare-v3 \
  --community "$EMPTY_COMMUNITY" \
  --idempotency-key "$PREPARE_KEY"
```

把 receipt 的 exact `operation_id` 填入
[`ProjectViewInitializeV3`](../../../nips/NIP-PV3.md#greenfield-projectviewinitializev3) command；所有 Goal、
Role、Proposal 与 Assignment ID 都使用新的 UUID v4，所有 initial Context set 为空。由 direct Human
owner 提交：

```bash
buzz project-view init-v3 --command "$INITIALIZE_V3_JSON"
buzz-admin project-view status --community "$EMPTY_COMMUNITY"
buzz-admin project-view enable --community "$EMPTY_COMMUNITY"
buzz --format compact project-view get
```

确认 schema v3、revision 1、单一 Project Profile、exact Human governance、Context disabled，以及 v2-only
客户端 unsupported。该 canary 稳定也不能成为阶段 5 broad rollout 的依据。

## 6. 本地交付门禁与证据状态

代码合并前至少执行：

```bash
. ./bin/activate-hermit
cargo test -p buzz-acp project_view --lib
cargo test -p buzz-cli --lib
cargo test -p buzz-relay project_view_extension_parses_versioned_current_and_role_history_scopes --lib
cd desktop && pnpm check && pnpm test
cargo test --manifest-path desktop/src-tauri/Cargo.toml project_view
cd desktop && pnpm test:e2e:smoke -- project-view.spec.ts
```

完整 PR 仍需 `just ci`、`just test`、`just project-document-test` 与 `just project-view-test`。需要
PostgreSQL/Redis/真实 Relay 的命令不能用 mock 结果替代。交付记录必须分别标注：自动化代码验证、
isolated canary、真实 bounded Community canary；未执行的层级保持 pending。

截至2026-08-01，本工作区的证据状态为：

| 证据层级 | 状态 | 已验证内容 |
|---|---|---|
| 自动化代码门禁 | completed | ACP全量619项、Project View unit/property/relation/wire、CLI/Relay/SDK相关门禁、Desktop check与全量测试、Tauri Project View 17项、相关Rust Clippy `-D warnings` |
| isolated PostgreSQL | completed | Project View DB 20项；包含maintenance begin、fresh supervisor poll、Runtime/Assignment baseline与durable ACK推进到`ready_to_freeze` |
| isolated Relay | completed | WebSocket/HTTP write/read、分页与live revocation E2E |
| Desktop v3 Resource | completed | Project View Playwright E2E 30项；包含2条strict-v3 Resource/Guide saga，以及Guide已提交后Resource conflict不误回滚 |
| bounded legacy Community | completed | 真实本地栈完成reviewed Guide、detached approval、exact Runtime/Assignment ACK、freeze/cutover/verify/resume与strict v3恢复检查 |
| empty-state direct-v3 Community | completed | 独立真实本地Community完成prepare、Human签名initialize、enable、v3读取与v2-only unsupported观察 |
| broad rollout | not authorized | 阶段5/6仅允许声明过的bounded canary；阶段7 gate前不得扩大 |

### 2026-08-01 本地真实运行记录

- 时间：`2026-08-01T10:58:58Z`；执行类型：`real_local`；服务：PostgreSQL、Redis、Relay、
  `buzz-cli`、`buzz-admin`、`buzz-acp`、ACP child。
- bounded target：`localhost:29826`；maintenance epoch `1`；cutover change
  `a64243b6cd925985ff3fcda610695dde709fbc286a1cfeb6e6ab43000e8abbf3`；project revision
  `6 → 7`、projection generation `2 → 3`。Assignment
  `90e3611b-b710-408f-9d32-1c929e13730c`的Runtime从
  `a566e78c-72e1-4781-b53d-3be7b24660a7:1`更换为
  `6cb27198-d334-47bc-b367-6cd12538876a:1`，旧fence未复用。
- Resource `9b27c50e-af8c-47e0-91ba-709aff37f25e`绑定Guide
  `1e9f8c46-5a27-40d6-8202-8979fb7f0396`；reviewed manifest文件SHA-256为
  `4c78102962d722ee6f8c65dbf2bc34eea27349cb4d03d3e33a2151bc391428f2`，cutover canonical
  manifest digest为`9e55ba9ce480b438176df33baa893874ce250edb1f0ea9ce053f3dfca019d2ae`。
- empty target：`127.0.0.1:29826`；prepare operation
  `608c71cd-d588-4fb6-b066-0c6907708352`；initialize event
  `992593167e499b43ecbfa1af7f392ba012fb9290bb82811457df2ddeb2b8c33c`；最终revision/generation
  为`1/1`，Context关闭，exact Human governance与3个初始对象通过核对。
- 证据目录：`test-results/stage5-canary/20260801T105825Z-1319826`。逐文件digest记录
  `artifact-digests.sha256`的SHA-256为
  `246aa8a657b2ef4f8f931291557db421cad3d4ea5ab33876e1c3cf0e1001516a`，`sha256sum -c`全部通过；
  scratch数据库已删除。

该记录完成阶段5的两个Community级本地验收，只证明bounded local target，不授权生产部署、阶段6或
broad rollout。
