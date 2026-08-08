# Meeting V2 后端发布与运维手册

> 适用范围：`v=3` 的 `moderated-board-v1` 与
> `moderated-board-actions-v3`；Relay、DB、CLI 和 ACP 后端。
>
> 不包含：Desktop、Meeting 模板、Meeting 类型，以及外部业务系统自身的正确性。

## 1. 现行行动收口模型

`moderated-board-actions-v3` 使用 direct action 收口：

```text
final Board
  → FINALIZE_ACTIONS
  → finalizing_actions/runnable
  → 主持人使用普通业务工具
  → End(closed, attestation=actions-recorded)
  → ended/closed
```

- Agent 主持由同一逻辑 Agent 的任意健康 slot 执行；已有 Meeting channel Session 只是调度偏好，
  不是权限或正确性门禁；
- Human 主持使用现有业务界面或普通 CLI；
- Project View 只是可选业务目标之一；
- Meeting 内不存在 Action Plan、Step、编译器或专用 Project View materializer；
- Relay 只验证主持身份、最终 Board、run/window、lease/deadline 和 attestation；
- Relay 不判断 Board 与外部系统是否语义一致，也不要求至少发生一次外部写入。

主持人若判断无需外部写入，也可以在行动 Turn 中确认这一判断后提交
`actions-recorded`。

## 2. 发布前门禁

运行完整 Meeting 后端门禁：

```bash
. ./bin/activate-hermit
./scripts/run-meeting-backend-tests.sh
```

门禁包括：

- SDK、Relay、DB、CLI 和 ACP 合约；
- direct policy/capability/Create gate；
- migration 与 `schema/schema.sql` drift；
- Board/Floor/direct-action 生命周期；
- 三 Agent roster capability gate；
- block、retry、return-to-board 和 attested close；
- 旧 Plan/Step 命令与旧 policy 的拒绝；
- logical-host 调度、进程级 lease renewal 与显式 `actions-recorded` ACK。

单独检查 Meeting schema drift：

```bash
./scripts/meeting-v2-schema-drift.sh
```

## 3. 代际切换前置条件

从 action-finalization capability v3 切换到 v4 不新增或修改数据库 schema，也不重写历史 Meeting。
发布前必须只读确认没有 active Meeting 和 non-terminal Action Run：

```sql
SELECT community_id, session_id, status, floor_policy_version
FROM meeting_sessions
WHERE status = 'active';

SELECT community_id, session_id, action_run_id, action_condition
FROM meeting_v2_action_runs
WHERE terminal_status IS NULL;
```

两项结果都必须为空。若非空则停止切换，不能自动 abort、删除或修改 Meeting。

现行 schema 应继续满足：

```sql
SELECT
  to_regclass('meeting_v2_action_runs') IS NOT NULL AS direct_runs_present,
  to_regclass('meeting_v2_action_command_receipts') IS NOT NULL AS receipts_present,
  to_regclass('meeting_v2_action_steps') IS NULL AS old_steps_absent,
  to_regclass('meeting_v2_action_step_attempts') IS NULL AS old_attempts_absent;
```

四项都应为 `true`。

## 4. 能力与配置

### 4.1 ACP capability

现行 capability：

```text
meeting-v2-action-finalization-v4
```

检查生产 ACP artifact：

```bash
target/release/buzz-acp capabilities --json | jq -e '
  (.meeting.capabilities | index("meeting-v2-action-finalization-v4") != null)
  and any(
    .meeting.protocols[];
    .schemaVersion == "3"
      and .policy == "moderated-board-actions-v3"
      and .capability == "meeting-v2-action-finalization-v4"
      and .moderatorExecution == "logical_agent_channel_session_preferred"
      and .actionCompletion == "explicit_actions_recorded_ack"
      and (.turns | index("action_finalization") != null)
  )
'
```

`meeting-v2-action-finalization-v2` 与 `meeting-v2-action-finalization-v3` 只用于历史诊断，
不能满足新建 Meeting 的 roster gate。profile reconcile 必须删除旧代际并只发布 v4。

完整 frozen roster 中的每个 managed Agent 都必须声明该能力。只有主持 Agent 执行 action
Turn，但所有 Agent runtime 都必须理解 direct State 和终止清理。

### 4.2 Relay NIP-11

runtime 就绪时：

```text
buzz-meeting-v2
buzz-meeting-v2-direct-actions
```

Create gate 同时开启时额外声明：

```text
buzz-meeting-v2-create
buzz-meeting-v2-direct-actions-create
```

检查：

```bash
curl -fsS -H 'Accept: application/nostr+json' https://RELAY/ | jq '.supported_extensions'
```

### 4.3 Create gate

```text
BUZZ_MEETING_V2_CREATE_ENABLED=false
BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED=false
```

两个 gate 都默认关闭。创建 direct-action Meeting 时必须同时开启。关闭 gate 只禁止新建，
不会关闭已经存在的 direct v2 Meeting。

旧配置 `BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED` 不再解析，也不是兼容别名。

## 5. 灰度顺序

1. 只读确认 active Meeting 与 non-terminal Action Run 都为零；
2. 保持 direct-action Create gate 关闭，统一发布 SDK、Relay、ACP 与 Desktop；
3. 重启受管 ACP，使 Contract 4/7、`meeting-context-v3` 与 capability v4 同时生效；
4. profile reconcile 删除旧 v2/v3 capability，只保留 v4；
5. 确认 Relay readiness、NIP-11、ACP capability 与 Agent profile；
6. 开启基础 V2 与 direct-action Create gate；
7. 执行一场短 Meeting，验证 Board、跨 slot Action Turn、续期与显式 ACK close；
8. 扩大流量。

禁止用旧 policy 创建新 Meeting：

```text
moderated-board-actions-v1
moderated-board-actions-v2
```

现行 policy：

```text
moderated-board-actions-v3
```

## 6. 权威状态查询

查看 active direct action run：

```sql
SELECT
  run.community_id,
  run.session_id,
  run.action_run_id,
  encode(run.board_event_id, 'hex') AS board_event_id,
  run.control_epoch,
  run.board_window,
  run.action_window_epoch,
  run.action_condition,
  run.action_deadline_at,
  run.last_error_code,
  run.updated_at
FROM meeting_v2_action_runs run
WHERE run.terminal_status IS NULL
ORDER BY run.updated_at;
```

查看终态：

```sql
SELECT
  session.session_id,
  session.terminal_outcome,
  run.terminal_status,
  encode(run.completion_event_id, 'hex') AS completion_event_id,
  run.terminal_at
FROM meeting_sessions session
LEFT JOIN meeting_v2_action_runs run
  ON run.community_id = session.community_id
 AND run.session_id = session.session_id
WHERE session.floor_policy_version = 'moderated-board-actions-v3'
ORDER BY COALESCE(run.terminal_at, run.updated_at) DESC;
```

正常 direct close 应为：

```text
meeting_sessions.status=ended
meeting_sessions.terminal_outcome=closed
meeting_v2_action_runs.terminal_status=completed_closed
meeting_v2_action_runs.completion_event_id=<该 End event>
```

## 7. CLI 运维

读取权威 action 状态：

```bash
buzz meetings actions status --meeting <uuid>
```

Human 主持或人工修复路径可使用：

```bash
buzz meetings actions begin --meeting <uuid>
buzz meetings actions block --meeting <uuid> \
  --reason-code tool_unavailable \
  --reason 'temporary business tool outage'
buzz meetings actions retry --meeting <uuid>
buzz meetings actions return-to-board --meeting <uuid>
buzz meetings actions confirm-recorded --meeting <uuid>
```

`confirm-recorded` 不是单独的 Complete 命令。它直接构建带
`attestation=actions-recorded` 的 End，并在同一 Relay/DB 事务中关闭 Meeting 和 action run。

普通：

```bash
buzz meetings close --meeting <uuid>
```

不能绕过 `finalizing_actions`。进入行动阶段后必须使用 `confirm-recorded`，或者合法
return-to-board/abort。

## 8. 故障处置

### 8.1 runnable 超时

Relay sweeper 会把超时 run 转为：

```text
action_condition=blocked
last_error_code=action_deadline_exceeded
action_deadline_at=NULL
```

确认外部状态后，可选择：

- `retry`：开启新 action window 和独立 deadline；
- `return-to-board`：保留已经产生的外部效果，回到新 Board window；
- `abort`：异常结束 Meeting。

### 8.2 普通业务工具失败

支持的低基数 block reason：

```text
external_operation_failed
external_state_conflict
tool_unavailable
provider_failure
action_deadline_exceeded
```

`affinity_lost` 只作为历史记录读取；current v4 runtime 不得再创建该 BLOCK reason。

Meeting 不回滚普通业务工具已经接受的写入。retry 前必须重新读取目标系统权威状态，并按该
业务 API 自己的幂等/CAS 规则处理。

### 8.3 slot 或 ACP Session 变化

这不再是错误。Harness 在同一逻辑主持 Agent 的健康 slot 注入完整 frozen Board 与 canonical Turn
envelope 后继续；旧 slot/Session 不参与授权、ACK 或 Relay correctness 判断。一个已经开始的 Turn
仍由单一 slot 执行至完成、明确取消或真实 provider failure，不在 tool call 中途迁移。

## 9. 指标与告警

至少关注：

- action command 的 accepted/rejected 计数；
- `action_condition=blocked` 数量；
- `action_deadline_exceeded`；
- 新 runtime 中出现历史专用的 `affinity_lost`（应告警为协议回退）；
- active action run 的最老 `updated_at`；
- Meeting outbox backlog；
- ACP provider failure 与 format retry。

告警只说明生命周期停滞，不代表外部业务状态错误。外部系统仍由其自己的指标和审计负责。

## 10. 关闭新建与回滚

紧急关闭新建：

```text
BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED=false
```

保留基础 V2 时不要关闭 `BUZZ_MEETING_V2_CREATE_ENABLED`。现有 direct Meeting 继续恢复和
结束。

历史 migration `0046` 已删除旧 Plan/Step 表，不能靠降级二进制安全回滚。若 current direct
Meeting 出现问题，应先关闭 Create gate，通过 forward fix 修复，并处理现有 Session；不要恢复旧
materializer、旧 v3 capability 或 exact-session affinity。

## 11. 真实 Provider 验收

一次有界真实 Provider smoke：

```bash
./scripts/meeting-v2-actions-live-acceptance.sh
```

该脚本验证：

- 真实 ACP adapter 和 model；
- direct v2 capability、NIP-11 和 Create gate；
- 在独立 scratch database 中以当前 Project View v3 greenfield
  `prepare-v3 → owner init-v3 → checked enable` 初始化业务目标，并通过 v3 Role
  Offer/Accept 建立主持 Assignment；不启动 Project View v1/v2 普通运行时；
- Board Maintenance → Floor `FINALIZE_ACTIONS`；
- 已有 Meeting channel Session 可用时优先复用；测试环境中还必须覆盖原槽不可用或 Session
  轮换后由同一逻辑主持 Agent 的其他健康槽执行 action Turn；
- Agent 直接使用普通 `buzz project-view` CLI 创建旧 materializer 不支持的 Resource；
- 进程级 renewal 在等待槽和执行期间持续维持 current Action Run；
- `COMPLETE` 产生 `actions-recorded` attested End；
- Meeting/action run 原子进入 closed 终态；
- 源码工作区未被 Agent 修改；
- 数据库不存在旧 Step 表。

脚本不自动重试 Provider 失败；失败证据保留供人工判断，避免形成无限验收循环。

## 12. 已知边界

- Meeting 不验证 Board 与外部系统的语义一致性；
- Meeting 不要求发生至少一次外部写入；
- Meeting 不提供跨系统事务、补偿或回滚；
- return-to-board 明确保留已有外部效果；
- Human 主持的 Desktop 操作将在后端修正完成后单独设计；
- action Turn 只负责记录会后行动产出，不执行 Work 本身。

当前完整代际为 runtime capability `meeting-v2-action-finalization-v4`、Meeting Contract `4`、
Project Space Contract `7` 与逐 Turn `meeting-context-v3`；不得以旧 v3 capability 或
`meeting-context-v2` 通过新建、运维或验收门禁。
