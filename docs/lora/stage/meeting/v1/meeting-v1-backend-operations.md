# Meeting V1 后端发布与运维说明

本文是 `moderated-baton-v1` 后端的发布、观测和回滚入口。协议语义以
[`meeting-v1.md`](meeting-v1.md) 为准，实现细节以
[`meeting-v1-backend-implementation-design.md`](meeting-v1-backend-implementation-design.md)
为准。

本阶段不包含 Desktop、Web 或 Mobile 会议界面。

## 1. 稳定后端边界

Meeting V1 使用现有私有 Meeting channel 作为共享消息空间，并通过独立的 V1
控制事件与 Relay-signed State 实现主持式发言权传递。

前端和 Agent Runtime 可以依赖以下后端约束：

- V0 `uniform-random-v0` 与 V1 `moderated-baton-v1` 按 Session 冻结并存；
- V1 Create 显式携带 `v=2` 和 `policy=moderated-baton-v1`；
- 同一 Session 任一时刻至多一个 Offer 或 Grant；
- Human Request、Moderator control、Offer ACK、Grant、Progress、SAY/YIELD、
  Handoff、Recall 和 End 均由数据库事务串行化；
- 每次规范状态变化产生唯一、连续 revision 的 Relay-signed State；
- canonical Meeting 事件通过 durable outbox 按 Session 顺序投递；
- 多 Relay 副本重领参会资格撤销任务时使用单调 claim token；旧 worker 不能推进游标、
  完成任务或释放新 worker 的租约；
- Relay 和 ACP 重启后以数据库 State 与本地 prepared-event ledger 恢复；
- 权威 End 到达 ACP 后立即停止该 Session 的周期同步，取消运行 Turn，并删除终态
  ledger 中的 speech、summary、reason 和 prepared signed event；
- V1 Offer 可以回收运行中的 V0 Intent；排队的 V0 Granted 可以抢占 V1
  Intent/Moderator，但任何协议的 Granted Turn 都不会被容量调度抢占。

稳定 kind、tag 和 State shape 见实现设计的 wire contract 章节。后续前端不得自行推演
发言权；只显示 Relay State 和 canonical control/speech log。

## 2. 灰度开关

```text
BUZZ_MEETING_V1_CREATE_ENABLED=false
```

默认关闭。它只阻止新的 V1 Create：

- 已存在 V1 的 command、deadline recovery、outbox 和 End 继续工作；
- 已存在 V1 的同一个签名 Create 重放仍返回 duplicate success；
- V0 Create 不受影响。

自动化的两阶段 Relay 重启测试会先开启开关创建 V1，再关闭开关重启，验证上述三项。

ACP 相关开关：

```text
BUZZ_ACP_MEETING_V1_AUTO_ACCEPT=true
BUZZ_ACP_MEETING_V1_LEDGER_PATH=
```

Meeting Turn 沿用 Agent 的正常上下文工具。当前版本通过 Meeting prompt 要求只读调查，
不在代码中强制 MCP/CLI/HTTP 只读；这是已经记录的可信 Runtime 策略边界。

## 3. Metrics

Relay 暴露以下低基数 Prometheus metrics：

| Metric | 关键标签 | 含义 |
|---|---|---|
| `meeting_v1_command_total` | `action,outcome,duplicate` | V1 command 的规范结果 |
| `meeting_v1_command_latency_seconds` | 同上 | command 数据库事务延迟 |
| `meeting_v1_command_recovery_transitions` | 同上 | command lazy recovery 数量 |
| `meeting_v1_recovery_scan_total` | `outcome` | due-session 扫描结果 |
| `meeting_v1_recovery_scan_sessions` | 无 | 每轮取出的 due Session 数 |
| `meeting_v1_recovery_scan_last_sessions` | 无 | 最近一轮扫描取出的 due Session 数；失败时归零 |
| `meeting_v1_recovery_scan_saturated` | 无 | 最近一轮是否成功取满 batch（`0` 或 `1`，失败时归零） |
| `meeting_v1_recovery_result_total` | `outcome` | Session recovery 的 recovered/noop/error |
| `meeting_v1_recovery_transition_total` | `transition` | 闭合 deadline transition 类型 |
| `meeting_v1_recovery_lag_seconds` | `deadline_type` | 数据库 deadline 到实际恢复的延迟 |
| `meeting_v1_outbox_delivery_total` | `outcome` | V1 outbox 的 `delivered`、`claim_lost`、`worker_error` 或 `dispatch_failed` |
| `meeting_v1_outbox_delivery_latency_seconds` | `outcome` | 规范事件接收到投递尝试的延迟 |
| `meeting_v1_worker_errors_total` | `worker` | recovery/outbox worker 闭合错误位置 |

标签禁止包含 session、pubkey、event ID、speech、Intent summary、Handoff/rejection
reason 或错误正文。ACP 默认进程日志同样不输出 Agent message chunk 原文。

建议至少配置以下告警：

1. `meeting_v1_worker_errors_total` 在 5 分钟窗口持续增长；
2. `meeting_v1_outbox_delivery_total{outcome!="delivered"}` 持续增长或 delivered 比例下降；
3. `meeting_v1_recovery_lag_seconds` 的高分位持续超过运行环境允许值；
4. `min_over_time(meeting_v1_recovery_scan_saturated[5m]) == 1`，表示成功扫描连续 5 分钟
   取满 batch，提示 due-row backlog 或 starvation 风险；
5. `meeting_v1_command_total{outcome="error"}` 突增。

具体阈值应按灰度环境的 Session 数和 SLO 设置，不把 Session ID 做成 metric label。

## 4. 发布与回滚

发布顺序：

1. 保持 V1 Create 开关关闭，部署能够读取和恢复 V1 的所有 Relay；
2. 运行 `just test-meeting-backend`；
3. 验证 V0、V1、recovery、outbox 和 ACP 指标；
4. 小范围开启 V1 Create；
5. 观察 command error、recovery lag 和 outbox failure；
6. 扩大灰度。

回滚时先关闭 V1 Create。关闭开关不能停止现有 V1 runtime。若要降级到不识别 V1
kind 的旧二进制，必须先结束所有活动 V1 Session。

## 5. 验收入口

```bash
. ./bin/activate-hermit
just test-meeting-backend
just ci
just test
```

`just test-meeting-backend` 会：

1. 运行完整 ACP lib 和 Relay Meeting 单元测试，覆盖 participant/moderator、ledger、
   跨协议调度、16 Session 并发 ACK 与 1205 个同秒 State 的无损分页；
2. 启动 Postgres、Redis、MinIO 与启用 V1 Create 的 Relay；
3. 串行运行 Meeting DB contract/race tests，包括 Agent Handoff decline/reselect、五次
   直接接力上限和 revocation stale-worker lease fencing；
4. 运行 V0 lifecycle、V0 floor、V1 Baton、2 Human + 2 Agent identity E2E，以及
   2 Human + 4 Agent 容量上限的 12 次多轮发言 E2E；
5. 关闭 V1 Create 后重启 Relay，执行两阶段灰度兼容测试；
6. 以 membership enforcement 再次重启，执行参会资格撤销 E2E。

该入口要求本机 `3000` 端口空闲，并由脚本管理测试 Relay。2 Human + 2 Agent 用例验证
四个签名身份共享同一 canonical timeline；2 Human + 4 Agent 用例验证六个身份各完成
两次发言、第五个 Agent 被容量限制拒绝、Human 优先、直接 Handoff 与主持人恢复，以及
结束后的完整历史可见性。确定性 fake ACP 测试验证真实 ACP `session/new`、Turn、
上下文工具继承和结构化输出，但它不调用真实 LLM；额外压力回归覆盖 8 个 Session 的
5 轮共 40 次发言，以及 10 个 Agent 身份观察 20 轮共享消息时的 Intent 去重。

## 6. 已知可靠性边界

- 这些测试使用确定性 Agent 输出验证协议与 Runtime，不验证具体 LLM provider 的可用性、
  延迟或发言质量。真实 Codex ACP、模型配置证明和 6/10/12 Agent 压测见
  [`meeting-v1-live-acceptance-plan.md`](meeting-v1-live-acceptance-plan.md)；首次
  qualification 结果见
  [`meeting-v1-live-acceptance-report-2026-07-29.md`](meeting-v1-live-acceptance-report-2026-07-29.md)。
- Meeting outbox 是至少一次投递。客户端和 canonical State 以 event ID 去重；但在
  Redis publish 已成功、outbox delivery ACK 随后失败或丢失 claim 的故障窗口，
  `audit_log` 可能为同一个 event ID 追加两条 `event_created`。这不会改变会议状态或
  破坏审计哈希链，但会放大审计计数。安全实现审计 exactly-once 需要为全局审计契约增加
  持久幂等键和唯一约束，并让 outbox 等待审计落库；不能仅把 audit enqueue 移到 ACK
  之后，否则会产生审计缺失窗口。
