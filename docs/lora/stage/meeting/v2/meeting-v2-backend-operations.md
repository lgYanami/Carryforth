# Meeting V2 后端发布与运维手册

> 日期：2026-08-03
>
> 适用范围：Meeting V2（`v=3` + `moderated-board-v1` / `moderated-board-actions-v1`）
> Relay、DB、CLI、ACP，以及 action-capable policy 的 Project View v2 materializer。
>
> 不包含：前端、模板、会议类型、Project View 之外的 materializer 与执行 Work 本身。

## 1. 发布决策原则

Meeting V2 只有在下列四类证据同时成立时才是可灰度的后端发布候选：

1. `just test-meeting-backend` 在干净环境通过；
2. Relay NIP-11、readiness 和 ACP capability 与预期一致；
3. 真实 provider 的 qualification 证据包通过硬门禁；
4. 监控、告警、排空和回滚路径已在目标 Community 验证。

确定性测试不代替真实 provider；真实 provider 的输出质量也不代替 Relay/DB/wire
不变量。任一硬不变量非零时必须停止扩灰。

## 2. 发布前门禁

### 2.1 完整 Meeting 后端门禁

```bash
. ./bin/activate-hermit
just test-meeting-backend
```

该入口包含：

- ACP Meeting 全量单测和 qualification observer feature 测试；
- Relay Meeting 合同和低基数 metric label 测试；
- Postgres Meeting 状态机、竞态、fresh/upgrade/concurrent migration 测试；
- migration-built Meeting 对象与 `schema/schema.sql` 的漂移检查；
- 真实 Relay + 真实 `buzz` CLI 的 V0/V1/V2 E2E；
- 关闭 V1/V2 Create 后已有 Session 继续运行与结束；
- 安全撤权与历史读取边界；
- ACP/Relay capability 以及 qualification verifier 正反 fixture；
- action deadline、abort/return、target ingest fence 和部分成功恢复；
- 在全部旧协议场景之后运行的 action policy 三 Agent Project View v2 E2E。

该脚本会创建并清理隔离数据库，且要求指定 Relay 端口事先空闲。不得将其
指向开发者正在使用的 Relay 或共享环境。

### 2.2 独立 migration drift 检查

```bash
PGHOST=localhost \
PGPORT=5432 \
PGUSER=buzz \
PGPASSWORD=buzz_dev \
PGDATABASE=<migration-built-database> \
./scripts/meeting-v2-schema-drift.sh
```

输入数据库必须是通过 SQLx migrations 构建到最新版本的一次性数据库。脚本只对
`meeting_` 对象失配失败，不会用其他历史对象差异削弱 Meeting gate。

## 3. capability 与 readiness

### 3.1 ACP 静态能力

```bash
buzz-acp capabilities --json | jq .
```

调度层至少要求：

```jq
.meeting.protocols[]
| select(.schemaVersion == "3" and .policy == "moderated-board-v1")
| (.roles == ["participant", "moderator"])
  and (.turns | index("intent") != null)
  and (.turns | index("granted_speech") != null)
  and (.turns | index("board_maintenance") != null)
  and (.turns | index("floor_decision") != null)
  and (.currentBoard == "authoritative_read_before_each_semantic_turn")
  and (.boardFloorDeadlines == "independent")
```

`qualificationEvidenceCompiled` 只表示该二进制包含隐私过滤后的验收 sink。正常生产二进制
应为 `false`；真实 qualification 专用二进制应通过以下方式构建：

```bash
cargo build --release -p buzz-acp --features meeting-acceptance
target/release/buzz-acp capabilities --json \
  | jq -e '.meeting.qualificationEvidenceCompiled == true'
```

该静态输出不证明 provider 已登录、指定模型存在或一次 Turn 能成功。这些必须由
qualification preflight 与运行证据证明。

### 3.2 Relay NIP-11

```bash
curl -fsS \
  -H 'Accept: application/nostr+json' \
  -H 'Host: <community-host>' \
  https://<relay-host>/ \
  | jq '.supported_extensions'
```

- `buzz-meeting-v2`：有稳定 Relay signer 且完整 V2 runtime schema 可用；
- `buzz-meeting-v2-create`：在 runtime capability 之上，当前实例允许创建新 V2。

关闭 `BUZZ_MEETING_V2_CREATE_ENABLED` 只会移除 `buzz-meeting-v2-create`，不得移除
`buzz-meeting-v2`。若 runtime capability 也消失，该实例不能用于已有 V2 排空。

### 3.3 Relay readiness

```bash
curl -fsS https://<relay-host>/_readiness | jq .
```

`meeting_v2=true` 表示该实例对当前数据库状态是部署安全的：

- Create 关闭且没有 active V2 时，允许二进制先于 migration 部署；
- Create 开启或存在 active V2 时，必须有完整 schema 与稳定 signer；
- 关闭 Create 不能让一个仍有 active V2 但缺 signer/runtime 的实例恢复 ready。

readiness 是实例能否接流量的判断；NIP-11 是客户端能否使用 V2 的能力声明。两者
不可互相替代。

### 3.4 action-capable policy 能力与 Create gate

Action runtime 使用 capability `meeting-v2-action-finalization-v1`。ACP 二进制必须同时在
顶层 capability 列表和 `moderated-board-actions-v1` protocol 条目中声明它：

```bash
target/release/buzz-acp capabilities --json | jq -e '
  (.meeting.capabilities | index("meeting-v2-action-finalization-v1") != null)
  and any(
    .meeting.protocols[];
    .policy == "moderated-board-actions-v1"
      and .capability == "meeting-v2-action-finalization-v1"
      and .moderatorContinuity == "exact_agent_slot_and_acp_session"
  )
'
```

每个 managed Agent 还必须用自己的 kind `10100` replaceable profile 发布同一 capability。
创建 action-capable Meeting 时，Relay 检查固定 roster 中的全部 managed Agent；Human 被忽略。
缺一名 Agent 的声明都拒绝 Create。旧 profile writer 省略 `capabilities` 时保留数据库中的最后
一次列表，显式空数组则撤销全部能力。

Action Create 同时要求：

```text
BUZZ_MEETING_V2_CREATE_ENABLED=true
BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED=true
```

两个开关都默认 `false`。只开启 action 开关不会绕过基础 V2 gate。NIP-11 的声明为：

- `buzz-meeting-v2-actions`：实例可以读取、恢复和排空存量 action Session；
- `buzz-meeting-v2-actions-create`：两个 Create gate 都开启，允许新建。

关闭任一 Create gate 只能移除 `buzz-meeting-v2-actions-create`，不能移除 runtime capability。
此外，创建前应确认 action roster 的 Agent profile 已同步，并确认目标 Community 的 Project
View 已经是 ready 的 schema v2；后者不是 Create 的强制条件，因为会议仍可以选择直接
`CLOSE`，但它是执行 Project View 行动的必要前置条件。

## 4. 指标与告警

所有 label 都是固定低基数枚举。禁止将 Session ID、公钥、event ID、Board、Speech、
Prompt 或原始错误作为 label。

| 信号 | 主要指标 | 建议告警 |
|---|---|---|
| Board command | `meeting_v2_board_command_total` / `_latency_seconds` | 排除 conflict/expired 后 5m 成功率 < 99.9% |
| current Board read | `meeting_v2_board_read_total` / `_latency_seconds` | 授权请求 5m error/not_found 率 > 0.1% |
| End | `meeting_v2_end_total` | `reason_code=other` 突增，或非预期 `participant_revoked` |
| Baton command | `meeting_baton_command_total{protocol="v2"}` | error 突增 |
| Action command | `meeting_v2_action_command_total` | 排除预期 conflict 后 error/expired 突增 |
| Action phase | `meeting_v2_action_phase_transition_total` | blocked 比例突增或长期没有 ready-to-close |
| Action step | `meeting_v2_action_step_total` / `_latency_seconds` | reject 增长或 P99 超出 action deadline 预算 |
| Action blocked/retry | `meeting_v2_action_blocked_total` / `_retry_total` | `affinity_lost`、`provider_failure` 或 deadline 连续增长 |
| Action affinity | `meeting_v2_action_affinity_mismatch_total` | 任意非零都停止扩灰并核查槽/Session |
| Action close gate | `meeting_v2_action_close_gate_rejection_total` | 持续非零说明主持端过早 End 或状态同步异常 |
| recovery | `meeting_baton_recovery_*{protocol="v2"}` | saturation 连续 3 个扫描，或 lag > 30s |
| outbox | `meeting_baton_outbox_delivery_*{protocol="v2"}` | P99 > 5s，或 worker_error/claim_lost 持续增长 |
| worker | `meeting_baton_worker_errors_total` | 5m 增量非零并持续两个窗口 |

最小 PromQL 示例：

```promql
sum(rate(meeting_v2_board_command_total{outcome="accepted"}[5m]))
/
sum(rate(meeting_v2_board_command_total{outcome=~"accepted|error"}[5m]))
```

```promql
histogram_quantile(
  0.99,
  sum by (le) (
    rate(meeting_baton_outbox_delivery_latency_seconds_bucket{protocol="v2"}[5m])
  )
)
```

```promql
max_over_time(meeting_baton_recovery_scan_saturated[3m]) == 1
```

PromQL 中 Board command 的 conflict/expired 是客户竞态结果，不纳入 Relay 可用性分母。Board read
`denied` 也不得算作服务器失败；它应单独用于安全趋势观测。

## 5. 最小运维查询

以下 SQL 只返回计数、延迟和协议维度，不读取 Board/Speech 正文。

active V2 数量：

```sql
SELECT count(*)
FROM meeting_sessions
WHERE status = 'active'
  AND schema_version = 3
  AND floor_policy_version IN ('moderated-board-v1', 'moderated-board-actions-v1');
```

V2 运行阶段分布：

```sql
SELECT v.runtime_phase, count(*)
FROM meeting_v2_bootstrap_state v
JOIN meeting_sessions s
  ON s.community_id = v.community_id AND s.session_id = v.session_id
WHERE s.status = 'active'
GROUP BY v.runtime_phase
ORDER BY v.runtime_phase;
```

Action run 阶段与条件分布：

```sql
SELECT run.action_phase, run.action_condition, run.last_error_code, count(*)
FROM meeting_v2_action_runs run
JOIN meeting_sessions session
  ON session.community_id = run.community_id
 AND session.session_id = run.session_id
WHERE session.status = 'active'
  AND session.schema_version = 3
  AND session.floor_policy_version = 'moderated-board-actions-v1'
  AND run.terminal_status IS NULL
GROUP BY run.action_phase, run.action_condition, run.last_error_code
ORDER BY run.action_phase, run.action_condition, run.last_error_code;
```

到期或即将到期的 action window：

```sql
SELECT
  count(*) FILTER (WHERE run.action_deadline_at <= clock_timestamp()) AS due,
  count(*) FILTER (
    WHERE run.action_deadline_at > clock_timestamp()
      AND run.action_deadline_at <= clock_timestamp() + interval '30 seconds'
  ) AS due_within_30_seconds,
  COALESCE(
    EXTRACT(EPOCH FROM clock_timestamp() - min(run.action_deadline_at))
      FILTER (WHERE run.action_deadline_at <= clock_timestamp()),
    0
  ) AS oldest_due_seconds
FROM meeting_v2_action_runs run
JOIN meeting_sessions session
  ON session.community_id = run.community_id
 AND session.session_id = run.session_id
WHERE session.status = 'active'
  AND run.terminal_status IS NULL
  AND run.action_condition = 'runnable';
```

最早到期 Baton：

```sql
SELECT
  count(*) FILTER (WHERE b.next_action_at <= clock_timestamp()) AS due,
  COALESCE(
    EXTRACT(EPOCH FROM clock_timestamp() - min(b.next_action_at))
      FILTER (WHERE b.next_action_at <= clock_timestamp()),
    0
  ) AS oldest_due_seconds
FROM meeting_baton_state b
JOIN meeting_sessions s
  ON s.community_id = b.community_id AND s.session_id = b.session_id
WHERE s.status = 'active'
  AND s.schema_version = 3
  AND s.floor_policy_version IN ('moderated-board-v1', 'moderated-board-actions-v1');
```

V2 pending outbox：

```sql
SELECT
  count(*) AS pending,
  COALESCE(EXTRACT(EPOCH FROM clock_timestamp() - min(o.available_at)), 0)
    AS oldest_pending_seconds
FROM meeting_event_outbox o
JOIN meeting_sessions s
  ON s.community_id = o.community_id AND s.session_id = o.session_id
WHERE o.delivered_at IS NULL
  AND s.schema_version = 3
  AND s.floor_policy_version IN ('moderated-board-v1', 'moderated-board-actions-v1');
```

排空状态：

```sql
SELECT
  (
    SELECT count(*)
    FROM meeting_sessions
    WHERE status = 'active'
      AND schema_version = 3
      AND floor_policy_version IN ('moderated-board-v1', 'moderated-board-actions-v1')
  ) AS active_sessions,
  (
    SELECT count(*)
    FROM meeting_event_outbox o
    JOIN meeting_sessions s
      ON s.community_id = o.community_id AND s.session_id = o.session_id
    WHERE o.delivered_at IS NULL
      AND s.schema_version = 3
      AND s.floor_policy_version IN ('moderated-board-v1', 'moderated-board-actions-v1')
  ) AS pending_outbox;
```

## 6. 灰度顺序

1. 保持所有 Community 的 `BUZZ_MEETING_V2_CREATE_ENABLED=false` 和
   `BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED=false`。
2. 先部署 DB migration。
3. 部署所有能读取、恢复、结束 V2 的 Relay/worker，确认 readiness 为 ready。
4. 确认 fleet 每个 Relay 都声明 `buzz-meeting-v2` 和 `buzz-meeting-v2-actions`。
5. 部署 ACP，对每个候选二进制执行 capability probe。
6. 让候选 Agent 发布 kind `10100` capability profile，并抽查完整目标 roster；不要只检查主持人。
7. 对需要行动物化的测试 Community 确认 Project View schema v2、Relay signer、主持人的 active
   Assignment 和承接人的 active Role Assignment。
8. 运行 Meeting backend gate、旧 policy 真实 qualification，以及一次有界的 action policy
   真实 provider acceptance。
9. 先只开启基础 V2 Create，验证 `buzz-meeting-v2-create`；再在隔离测试 Community 开启 action
   Create，验证 `buzz-meeting-v2-actions-create`。
10. 确认指标、零不变量、blocked 处置和排空路径后，再按 Community 扩大。

任一个实例在 Create 开启期间缺少 runtime capability，都应先关闭该 Community 的 Create，
而不是继续扩灰。

## 7. 故障处置

### 7.1 Board/Floor timeout

- 确认 timeout 后已产生权威 State，且 Board 迟到结果没有持久化；
- 确认 Floor Decision 使用自己的完整 deadline，不与 Board Turn 共享预算；
- 检查 provider 延迟与 Relay recovery lag，不要用放宽 Relay deadline 掩盖 worker 阻塞；
- 若多个 Session 同时受影响，关闭 Create 并优先排空。

### 7.2 current Board read failure

- 检查 NIP-11 runtime capability、Relay signer 与 `meeting_v2_board_read_total` outcome；
- `denied` 优先排查 roster/撤权/身份，`not_found` 优先排查 schema/projection；
- ACP 在 Intent 读板失败时必须 pass，Grant 读板失败时必须 yield，不得用旧 Board
  继续调用模型。

### 7.3 worker backlog / outbox

- 检查 scan saturation、oldest due SQL、pending outbox SQL 与 Redis/DB 健康；
- 不得手动删除 outbox 或修改 `next_action_at` 来伪造收敛；
- 在实例能力一致前，保持 Create 关闭并扩容兼容的 Relay/worker。

### 7.4 ACP/provider 不可用

- Relay 状态机与 deadline 继续权威，不得由运维人工伪造 Agent 输出；
- 确认 ACP capability 与 provider 登录/模型目录是两个独立检查；
- 对持续失败的会议，由主持人按协议 abort；安全撤权路径由 Relay 生成
  `participant_revoked` 终态。

### 7.5 硬不变量非零

立即停止扩灰，关闭 Create，保留数据库、指标快照、ACP 隐私过滤事件与进程日志。
在原因确认前不得修表、重放模型输出或修改 qualification 数值。

### 7.6 Action Finalization blocked 或停滞

- 先读取权威 Meeting State 和 `meeting_v2_action_runs`，区分 `planning`、`applying`、
  `ready_to_close`，以及 `runnable`/`blocked`；不要只看 ACP 日志。
- `action_deadline_exceeded` 表示 sweeper 或下一条命令已把到期窗口原子收敛为 blocked。确认原
  ACP Session 仍可验证后，使用协议 `retry` 开新 action window；不要直接改 deadline。
- `affinity_lost` 禁止在新槽或新 ACP Session 上重做旧语义 Turn。可以显式 abort；若仍需继续，
  按产品流程返回 Board 并建立新的控制周期，而不是伪造连续性。
- `provider_failure`、`project_view_v2_unavailable` 或 `assignee_unresolved` 先修复外部前置条件，
  再显式 retry。系统不会自动创建 Role/Assignment，也不会替主持 Agent 发明行动。
- `published`/`indeterminate` attempt 不允许 `RETURN_TO_BOARD` 或静默重做。先按 exact event ID
  查询 Project receipt；只有确定 reject 后才能准备下一 attempt。
- `ready_to_close` 仍无法 End 时，核对 action run/window/plan fence 和 verified projection；
  禁止绕过 close gate 或把 abort 伪装为 closed。
- abort、return-to-board 与 target ingest 竞态后，检查 attempt 是否收敛到 `accepted` 或
  `abandoned`。不要删除 action ledger、Project receipt 或 outbox 来“解锁”。

## 8. 关闭新建与回滚

### 8.1 关闭新建

1. 将目标 Community 的 `BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED` 设为 `false`；若要停止所有
   V2 新建，再将 `BUZZ_MEETING_V2_CREATE_ENABLED` 设为 `false`。
2. 确认 NIP-11 移除对应 `*-create`，但仍保留 `buzz-meeting-v2` 和
   `buzz-meeting-v2-actions` runtime capability。
3. 确认 readiness 仍为 ready。
4. 使用排空 SQL 观察 active Session 和 pending outbox 收敛。

### 8.2 安全回滚

回滚目标必须：

- 仍声明 `buzz-meeting-v2`；
- 仍声明 `buzz-meeting-v2-actions`；
- 理解当前 V2 schema、两个 policy、action tables 与 ledger generation；
- 能恢复、读 Board、继续 Floor 并结束所有 active V2；
- 能阻塞/恢复 action deadline，并保留 Project View prepared/accepted attempt fence；
- 保留当前 migration，不执行 down migration。

完全不理解 V2 的旧二进制不是安全回滚目标。禁止将 V2 Session 改写为 V1，也禁止
通过删除 Board/runtime 行强制排空。

## 9. 真实 provider qualification

一个完整的证据包至少包含：

```text
manifest.json
protocol-invariants.json
acceptance-events.ndjson
roster.tsv
meetings.tsv
metrics.prom
processes.tsv
security-probes.json
preflight/
logs/relay-create-enabled.log
logs/relay.log
logs/agents/*.log
workspace-before.status
workspace-after.status
workspace-before.diff.sha256
workspace-after.diff.sha256
sha256.txt
```

运行时使用全新 DB、隔离 Redis、身份和 Meeting。私钥和 ACP 恢复 ledger 只放在 `0700` 的临时 secret
目录，不得进入证据包。证据包必须同时包含 mixed、all-agent、moderator abort 和
admin/security abort 四类场景。

先构建真实验收专用二进制：

```bash
. ./bin/activate-hermit
cargo build --release -p buzz-cli -p buzz-admin -p buzz-relay
cargo build --release -p buzz-acp --features meeting-acceptance
```

再运行完整矩阵：

```bash
./scripts/meeting-v2-live-qualification.sh
```

runner 会优先使用 `MEETING_V2_CODEX_ACP_BIN`，其次检查当前 `PATH`，最后检查 Buzz 托管的
`~/.local/share/Buzz/node-tools/bin/codex-acp`。它要求包身份严格为
`@agentclientprotocol/codex-acp 1.1.7`，不会把 `command -v` 的结果作为唯一安装判断。

runner 会先在 Create 开启时创建四场会议，再重启同一 Relay、关闭 Create，并让已有会议在
runtime capability 保留的条件下继续运行和结束。默认输出目录是
`/tmp/buzz-meeting-v2-qualification/<run-id>`；可用第一个参数覆盖 artifact root。

常用隔离参数：

- `MEETING_V2_QUALIFICATION_ARTIFACT_ROOT`：默认 artifact root；位置参数优先；
- `MEETING_V2_QUALIFICATION_MODEL`：基础模型 ID，runner 分别请求 `[max]` 与 `[high]`；
- `MEETING_V2_CODEX_ACP_BIN`：指定待验收的 `codex-acp`；
- `MEETING_V2_QUALIFICATION_RELAY_PORT`、`MEETING_V2_QUALIFICATION_HEALTH_PORT`、
  `MEETING_V2_QUALIFICATION_METRICS_PORT`：三者必须互不相同且启动前未被占用；
- `MEETING_V2_QUALIFICATION_TIMEOUT_SECONDS`：完整场景超时，下限为 60 秒；
- `MEETING_V2_QUALIFICATION_KEEP_DATABASE=true`：仅用于失败诊断；正常运行应让 runner 删除
  一次性数据库。

运行后可独立复核：

```bash
./scripts/verify-meeting-v2-qualification.sh <run-directory>
jq . <run-directory>/qualification-gates.json
```

verifier 会先确认所有不可变证据（包括进程日志和 preflight）都纳入 `sha256.txt` 并校验其
内容，独立比较两份 workspace 快照，再交叉检查 roster、Session、provider/model 日志、
capability、指标、四类场景、observer 证据、安全探针与全部零不变量。manifest 对 workspace
或 provider 的自声明、单个日志关键词、人工观看或模型回答质量都不能单独决定 PASS。

若缺少真实 provider 环境或任一场景证据，可以完成代码与确定性门禁，但不得将阶段五
或发布候选标记为 PASS。

当前后端发布候选的已通过样本见
[Meeting V2 阶段五真实 Agent Qualification 报告](./meeting-v2-qualification-report.md)。原始
证据包包含私有运行日志，不提交到仓库；报告只保留非敏感聚合值、来源哈希与复现方式。

### 9.1 Action Finalization 有界真实验收

Action policy 使用单独的非重试 runner：

```bash
. ./bin/activate-hermit
cargo build --release -p buzz-cli -p buzz-admin -p buzz-relay
cargo build --release -p buzz-acp --features meeting-acceptance
./scripts/meeting-v2-actions-live-acceptance.sh
```

runner 只创建一场 `1 Agent 主持 + 1 Human` Meeting，并要求真实主持槽在同一 ACP Session 中
依次完成最终 Board、`FINALIZE_ACTIONS` 和 Materialization Intent；Harness 随后机械创建一个
Requirement、一个 Work、一个 responsibility，并在 verified projection 后关闭会议。默认产物
位于 `/tmp/buzz-meeting-v2-actions-acceptance/<run-id>`，也可用第一个参数或
`MEETING_V2_ACTIONS_ACCEPTANCE_ARTIFACT_ROOT` 指定根目录。

该脚本不自动 retry。失败时保留 `failure.txt`、observer、进程日志和 preflight，但清理私钥、
ACP ledger、一次性数据库和 Redis。修复代码后是否再运行必须是新的、明确授权的手动签收，
不得覆盖失败目录或只挑选成功样本。首次运行发现并保留了 frozen Board pre-fetch 围栏缺陷
的 FAIL 证据；实现修复且自动化门禁通过后，经用户明确授权执行一次新的非重试签收，Run ID
`meeting-v2-actions-20260803T122438Z-1420859` 为 PASS。证据表明 Board、Floor 与 Action 使用
同一 `agent_index=0` 和 ACP Session，3/3 物化步骤成功，会议在 verified projection 后以
`ended/closed` 收敛，运行前后源码哈希一致且证据校验和全部通过。详细记录见
[行动收口设计 §23.4](./meeting-v2-action-finalization-design.md#234-真实-provider-手动签收记录)。

## 10. 已知边界

- Meeting 不必关联 Project View；关联存在时也只是 Board 中的可选上下文。
- Board 是 pull-only current projection，没有 Board 版本管理和 participant 通知。
- 初版没有会议模板、会议类型、主持权转移、投票和前端会议看板。
- creator = owner = moderator，roster 在创建后固定。
- 模型不能直接发布事件；ACP 只把结构化结果翻译为受 Relay/DB 校验的协议命令。
- Action Finalization 只物化 Meeting 已决定的 Requirement、Work 和责任 Role，不执行 Work，
  不创建 Commitment，也不自动创建 Role/Assignment。
- Action Plan/step 是内部恢复和去重结构，不写回 Board，也不是 Project View 业务对象。
- `moderated-board-actions-v1` 的新建保持默认关闭；旧 `moderated-board-v1` 行为和零外部写入
  不变量不变。
- 关闭 Create 是排空工具，不是中止已有 Session 的开关。
