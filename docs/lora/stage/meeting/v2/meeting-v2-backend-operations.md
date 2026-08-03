# Meeting V2 后端发布与运维手册

> 日期：2026-08-02
>
> 适用范围：Meeting V2（`v=3` + `moderated-board-v1`）Relay、DB、CLI 和 ACP。
>
> 不包含：前端、Project View 联动、模板、会议类型与外部系统写入。

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
- ACP/Relay capability 以及 qualification verifier 正反 fixture。

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

## 4. 指标与告警

所有 label 都是固定低基数枚举。禁止将 Session ID、公钥、event ID、Board、Speech、
Prompt 或原始错误作为 label。

| 信号 | 主要指标 | 建议告警 |
|---|---|---|
| Board command | `meeting_v2_board_command_total` / `_latency_seconds` | 排除 conflict/expired 后 5m 成功率 < 99.9% |
| current Board read | `meeting_v2_board_read_total` / `_latency_seconds` | 授权请求 5m error/not_found 率 > 0.1% |
| End | `meeting_v2_end_total` | `reason_code=other` 突增，或非预期 `participant_revoked` |
| Baton command | `meeting_baton_command_total{protocol="v2"}` | error 突增 |
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
  AND floor_policy_version = 'moderated-board-v1';
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
  AND s.floor_policy_version = 'moderated-board-v1';
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
  AND s.floor_policy_version = 'moderated-board-v1';
```

排空状态：

```sql
SELECT
  (
    SELECT count(*)
    FROM meeting_sessions
    WHERE status = 'active'
      AND schema_version = 3
      AND floor_policy_version = 'moderated-board-v1'
  ) AS active_sessions,
  (
    SELECT count(*)
    FROM meeting_event_outbox o
    JOIN meeting_sessions s
      ON s.community_id = o.community_id AND s.session_id = o.session_id
    WHERE o.delivered_at IS NULL
      AND s.schema_version = 3
      AND s.floor_policy_version = 'moderated-board-v1'
  ) AS pending_outbox;
```

## 6. 灰度顺序

1. 保持所有 Community 的 `BUZZ_MEETING_V2_CREATE_ENABLED=false`。
2. 先部署 DB migration。
3. 部署所有能读取、恢复、结束 V2 的 Relay/worker，确认 readiness 为 ready。
4. 确认 fleet 每个 Relay 都声明 `buzz-meeting-v2`。
5. 部署 ACP，对每个候选二进制执行 capability probe。
6. 运行 Meeting backend gate 和真实 provider qualification smoke。
7. 只在隔离测试 Community 开启 Create，验证 `buzz-meeting-v2-create`。
8. 确认指标、零不变量和排空路径后，再按 Community 扩大。

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

## 8. 关闭新建与回滚

### 8.1 关闭新建

1. 将目标 Community 的 `BUZZ_MEETING_V2_CREATE_ENABLED` 设为 `false`。
2. 确认 NIP-11 移除 `buzz-meeting-v2-create` 但仍保留 `buzz-meeting-v2`。
3. 确认 readiness 仍为 ready。
4. 使用排空 SQL 观察 active Session 和 pending outbox 收敛。

### 8.2 安全回滚

回滚目标必须：

- 仍声明 `buzz-meeting-v2`；
- 理解当前 V2 schema/policy 与 ledger generation；
- 能恢复、读 Board、继续 Floor 并结束所有 active V2；
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

## 10. 已知边界

- Meeting 不必关联 Project View；关联存在时也只是 Board 中的可选上下文。
- Board 是 pull-only current projection，没有 Board 版本管理和 participant 通知。
- 初版没有会议模板、会议类型、主持权转移、投票和前端会议看板。
- creator = owner = moderator，roster 在创建后固定。
- 模型不能直接发布事件；ACP 只把结构化结果翻译为受 Relay/DB 校验的协议命令。
- 关闭 Create 是排空工具，不是中止已有 Session 的开关。
