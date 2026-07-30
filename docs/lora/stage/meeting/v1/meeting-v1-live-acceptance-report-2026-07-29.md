# Meeting V1 真实 Codex Qualification 报告

> 日期：2026-07-29
>
> 结论：真实 Codex 调用与 Meeting 协议闭环成立；C6 是 clean qualification，
> C10/C12 暴露 ACP cancel-drain 稳定性失败。正式发布验收未通过。
>
> 范围：Meeting V1 后端、Buzz ACP、`codex-acp`、真实 Codex；不包含前端

本报告记录
[`meeting-v1-live-acceptance-plan.md`](meeting-v1-live-acceptance-plan.md)
的第一次真实执行结果。Runner 最初只检查终态协议不变量，因此把 C6/C10/C12 都标记为
PASS；事后代码与完整日志审查发现 C10/C12 有未纳入门禁的 ACP runtime anomaly。本报告
采用修正后的结论，不沿用过宽的原始 manifest verdict。

## 1. 执行结论

真实调用链已经得到验证：

```text
Meeting State / Offer / Grant
  -> buzz-acp
  -> @agentclientprotocol/codex-acp 1.1.7
  -> Codex CLI 0.144.4
  -> authenticated real Codex session
```

所有 32 个 Meeting Agent 身份都建立了真实 ACP/Codex Session，并在各自 Agent
日志中记录了目标模型成功应用：

- 每场 Moderator：`gpt-5.6-sol[max]`；
- 其他 Agent：`gpt-5.6-sol[high]`；
- `CODEX_CONFIG` 同时请求对应的 `model_reasoning_effort`，并关闭 Codex 内部
  multi-agent；
- 没有 fake provider、预录输出、unsupported model、模型切换失败或认证失败。

执行结果支持以下判断：

1. 当前“每场最多 4 Agent”的协议容量内，可以同时运行 3 场、合计 12 个真实 Codex
   Agent；
2. 本轮共接受 120 条 canonical speech，没有双 Offer、双 Grant、revision 缺口、
   未清空 outbox 或无法 End 的会议；
3. 真实 Agent 的 Offer ACK 全部小于 500 ms，说明“不调用 LLM 的确定性 ACK”有效地
   隔离了模型冷启动和推理延迟；
4. 最慢 Agent Grant 到 Speech 为 242.896 秒，仍低于 Harness 的 270 秒本地 safety
   deadline，但已经接近边界，不应据此缩短 5 分钟 Grant；
5. 本轮未观察到 Relay 429、Agent Offer timeout、provider quota 错误或未处理的 Handoff，
   但 C10/C12 共发生 3 次 cancel-drain timeout 和 Codex 子进程 respawn；
6. C4 发现并复现了一个真实协议缺陷：Human priority 结束后，被
   `blocked_by=human_request` 延迟的 Directed Handoff 没有恢复。修复后同一场景重新执行
   成功，并增加了 PostgreSQL 回归测试。

因此，本轮结论是：

> Real Codex availability：PASS。
>
> Meeting canonical protocol completion：PASS。
>
> ACP runtime stability：FAIL。先修复或明确处置 cancel-drain churn，再继续重复与 soak。
>
> Production：NO-GO，也不支持提高单场 4 Agent 上限。

## 2. 环境与证据口径

| 项目 | 值 |
|---|---|
| Buzz HEAD | `b37e3494bea2a5d81838f6103c14b0f217016527` |
| Codex CLI | `0.144.4` |
| ACP adapter | `@agentclientprotocol/codex-acp@1.1.7` |
| Moderator | `gpt-5.6-sol[max]` |
| Participant | `gpt-5.6-sol[high]` |
| Codex auth | `codex login status` 确认使用 ChatGPT 登录 |
| Agent pool | 每个 Buzz 身份独立进程，`BUZZ_ACP_AGENTS=1` |
| Agent cold start | `BUZZ_ACP_LAZY_POOL=false` |
| 工具权限 | `bypass-permissions`，隔离工作区，Meeting prompt 约定只读 |
| 基础设施 | 每次 Tier 使用独立 PostgreSQL 数据库、Redis DB、Relay 和身份 |

本轮 Relay 与二进制包含 HEAD 之上的未提交验收修复；扩展样本的
`workspace-before.diff` 保存了 tracked diff，不能只凭 HEAD commit 重建完全相同的
运行状态。第一次 Runner 还没有记录二进制和 Runner 自身的 hash。后续 Runner 已补充：

- Runner SHA-256；
- `buzz`、`buzz-acp`、`buzz-admin`、`buzz-relay`、`buzz-test-cli` SHA-256；
- tracked workspace diff SHA-256。

报告核验完成后，7 个本轮创建的 disposable PostgreSQL 数据库、Redis DB 11–13 和保留的
C4 身份私钥文件均已删除；脱敏 artifact 与 Agent/Relay 日志继续保留。验收进程和
localhost:3000 listener 均已停止。

模型证据的准确含义是“目录支持 + Buzz 通过 adapter 成功应用目标 effort-qualified model
ID + 真实 Codex Session 实际完成 prompt”，不是 provider 签发的独立 attestation。正式
报告应继续使用 `requested_catalog_supported_and_adapter_session_log`，不能写成
provider-attested。

## 3. 用例与规模结果

### 3.1 汇总

| Tier | Meeting | Agent | Human | Agent Speech | Human Speech | 总 Speech | Answered Handoff | 结果 |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| C4 | 1 | 4 | 2 | 6 | 3 | 9 | 3 | 功能通过；非 clean sample |
| C6 | 2 | 6 | 4 | 12 | 12 | 24 | 10 | Clean PASS |
| C10 | 3 | 10 | 6 | 21 | 20 | 41 | 协议 PASS；runtime FAIL |
| C12 | 3 | 12 | 6 | 24 | 22 | 46 | 协议 PASS；runtime FAIL |
| 合计 | 9 | 32 Session | — | 63 | 57 | 120 | 50 | — |

C6、C10、C12 中每个 Agent 至少完成两次 canonical speech。C10 在 Harness 达成目标并
调用 End 时，恰好有一个由自然讨论新增的 Grant/Handoff 已开始，因此终态中有一个
`grant_state=ended` 和一个 `handoff_state=ended`；它们由 Meeting End 正常终结，不是
悬挂对象，也不计入失败。

### 3.2 C4：完整角色与 Human 介入

Artifact：`/tmp/buzz-meeting-v1-c4-rerun.dDrgwx`

拓扑为 2 Human + 4 Agent，其中 1 个 Agent 使用 max 担任 Moderator，其他 3 个 Agent
使用 high。六名参会者都至少发言一次。

| 指标 | 结果 |
|---|---:|
| 终态 | `ended` |
| State revision | 1–92，连续且无重复 |
| Offer | 9 acked，2 timed_out |
| Grant | 9 spoken |
| Human Request | 3 granted，2 timed_out |
| Receipt | 89，distinct 89，1 个预期 ACK race rejection |
| Outbox | pending 0，error 0 |
| Agent Offer ACK max | 303.6 ms |
| Agent Grant → Speech max | 163.839 s |
| Outbox delivery max | 425.1 ms |

两个 Human timeout 来自人工操作 Harness，而不是 Agent ACK：

1. 第一次通过 Relay 高频查询轮询 Offer，触发 429，错过 15 秒 Human Offer；
2. 第二次 Human Request 与 Agent selection/ACK 发生竞态，人工 ACK 未及时完成；
3. 第三次改用只读数据库观察时序后，Human priority、ACK、Speech 和 Directed Handoff
   完整成功。

因此 C4 证明了真实 Human/Agent 闭环，也暴露了“Human 交互必须由事件订阅或低延迟本地
状态驱动，不能高频轮询 Relay”的 Harness 约束；它不是“健康场景零 timeout”的正式重复
样本。

### 3.3 C6

Artifact：
`/tmp/buzz-meeting-v1-live-runs/c6-20260729T155306Z-3502`

| 指标 | 结果 |
|---|---:|
| Meeting | 2，均 ended |
| State revision | 87、86，历史均从 1 连续到终态 |
| Offer / Grant | 24 acked / 24 spoken |
| Handoff | 10 answered |
| Agent Offer ACK | min 256.7 ms，median 438.7 ms，max 493.2 ms |
| Agent Grant → Speech | min 22.843 s，median 74.573 s，max 200.974 s |
| Human Offer ACK max | 285.6 ms |
| Outbox delivery | median 138.2 ms，max 510.9 ms |
| Outbox | pending 0，error 0，max attempts 1 |

C6 没有 `cancel_drain_timeout`、子进程 respawn 或 Meeting action `uncertain`，是本轮
唯一同时通过协议和 ACP runtime review 的扩展 Tier。

### 3.4 C10

Artifact：
`/tmp/buzz-meeting-v1-live-runs/c10-20260729T160519Z-81926`

| 指标 | 结果 |
|---|---:|
| Meeting | 3，均 ended |
| State revision | 94、92、100，历史均连续 |
| Offer | 42 acked |
| Grant | 41 spoken，1 ended |
| Handoff | 18 answered，1 ended |
| Agent Offer ACK | min 260.4 ms，median 372.7 ms，max 474.4 ms |
| Agent Grant → Speech | min 16.529 s，median 74.960 s，max 242.896 s |
| Human Offer ACK max | 419.7 ms |
| Outbox delivery | median 137.5 ms，max 705.9 ms |
| Outbox | pending 0，error 0，max attempts 1 |

Runtime review 发现：

- 2 个 Moderator 各发生 1 次 `cancel_drain_timeout`；
- 两次都由 `buzz-acp` 杀死并重建 `codex-acp` 子进程，随后恢复并完成会议；
- 另有 1 次 Moderator `select_handoff` 提交结果为 `uncertain`，最终 canonical State
  仍成功推进并回答 Handoff。

因此 C10 不是 clean qualification。原 manifest 的 PASS 只代表它检查到的协议终态门槛
通过。

### 3.5 C12

Artifact：
`/tmp/buzz-meeting-v1-live-runs/c12-20260729T161739Z-87972`

| 指标 | 结果 |
|---|---:|
| Meeting | 3，均 ended |
| State revision | 103、104、99，历史均连续 |
| Offer / Grant | 46 acked / 46 spoken |
| Handoff | 19 answered |
| Agent Offer ACK | min 234.4 ms，median 374.9 ms，max 488.9 ms |
| Agent Grant → Speech | min 17.948 s，median 65.107 s，max 239.043 s |
| Human Offer ACK max | 376.5 ms |
| Outbox delivery | median 138.6 ms，max 581.7 ms |
| Outbox | pending 0，error 0，max attempts 1 |

Runtime review 发现 1 个 Moderator 发生 `cancel_drain_timeout` 和子进程 respawn。会议在
respawn 后恢复并完成，但这仍违反“无需运行时重启即可稳定完成”的 qualification 目标。

### 3.6 ACP cancel-drain 观察

| Tier | Meeting Turn dispatched | 正常完成 | 预期取消 | cancel-drain respawn | 提交 uncertain |
|---|---:|---:|---:|---:|---:|
| C4 | 47 | 40 | 7 | 0 | 0 |
| C6 | 50 | 46 | 4 | 0 | 0 |
| C10 | 99 | 84 | 13 | 2 | 1 |
| C12 | 122 | 115 | 6 | 1 | 0 |

异常都发生在 Moderator 的快速状态切换窗口：

```text
Moderator Turn dispatched
  -> 新 State 到达，Buzz 发送 Cancel
  -> 下一 Turn 很快开始创建新 ACP Session
  -> 又一个 State 触发 Cancel
  -> session/cancel 已发送，但 5 秒内没有完成 drain
  -> buzz-acp 判定该子进程不可信，respawn
  -> 新 Session 重新应用 gpt-5.6-sol[max]，会议继续
```

这说明恢复机制有效，但也说明“快速 Cancel → 新 Session → 再 Cancel”在真实
`codex-acp` 下不稳定。它不是模型不可用，也没有破坏 canonical State；它是需要修复或
改变调度策略的 ACP runtime 问题。

## 4. C4 发现并修复的协议问题

### 4.1 症状

Agent 持有 Grant 时，Human Request 正确进入队列。该 Agent 的 Speech 同时创建
Directed Handoff 时，Handoff 正确保存为：

```text
question_state = open
initial_disposition = blocked
blocked_by = human_request
```

Human priority 结束后，原实现没有清空 `blocked_by`。主持人会把它理解成当前仍不可选择，
而 moderator fallback 又只消费 Intent，不消费 Handoff，会议可能保持 idle。

### 4.2 修复

当最后一个 queued/offered Human Request 结束、控制权返回 Moderator 时，同一 Session
锁和事务会：

1. 清除所有 open Handoff 的瞬时 `blocked_by=human_request`；
2. 保留 `initial_disposition=blocked` 作为历史事实；
3. 在同一 canonical State 中发布 `handoff_unblocked` effect；
4. 让 Moderator 立即 Select 或 Dismiss，Relay 不替主持人自动决定。

C4 修复后重跑观察到：

```text
handoff_unblocked
  -> moderator Select 原 source_handoff_id
  -> target Offer / ACK / Grant
  -> target Speech
  -> source Handoff answered
```

PostgreSQL 回归测试
`human_priority_release_unblocks_directed_handoff_for_moderator_selection`
覆盖了这条状态链。

## 5. 质量与行为观察

本轮对 C12 历史进行了人工抽查。Agent 能：

- 读取实际 Rust、SQL migration、测试和权威 State；
- 区分静态代码证据、确定性测试证据和本场真实观察；
- 回答 Directed Handoff 指定的问题；
- 给出可观测停止条件，而不是笼统宣称可以扩容；
- 主动指出 provider-blind admission、Grant 尾延迟、Session 热行、outbox
  at-least-once 和 Progress 恢复预算等风险；
- 遵守只读约定，运行前后 tracked/untracked 状态和 tracked diff 均未变化。

但这还不是正式质量签收：

- 尚未按固定 rubric 对全部 120 条 speech 逐条评分；
- 定向复核 prompt 相似，部分回答出现结构和风险点重复；
- 回答普遍偏长，不足以证明日常会议的表达效率；
- Agent 对“本轮是否执行某项测试”的判断只来自它的 Turn 上下文，不能代替 Harness 的
  权威测试记录。

所以本报告不声称达到“90% 相关且有新增信息”或“中位数 4/5”的质量门槛。

## 6. Runner 偏差与未计入样本

调试阶段产生的失败或主动终止 run 没有计入 PASS：

1. macOS Bash 不支持 `${value,,}`；
2. `psql` 变量插值和动态 Community ID 处理错误；
3. 使用基础 `BUZZ_ACP_MODEL=gpt-5.6-sol` 时，Buzz 无法在新的 Session model catalog
   中完成目标匹配并触发 model fail-open。原生 Codex session metadata 显示模型实际正确，
   但该 run 仍按 fail-closed 规则终止；
4. Moderator 合法 Dismiss 重复 Handoff 被 Driver 误判为失败；
5. 第一份 C6 汇总 SQL 的 revision `GROUP BY` 写错。该样本没有直接签收；修复查询后在保留
   的同一数据库重新核验，所有门槛通过；
6. Runner 最初没有把 `agent_returned — respawning` 和 Meeting action
   `outcome=uncertain` 纳入失败门禁，因此错误地把 C10/C12 manifest 标成 PASS；后续
   Runner 已把这两类日志设为硬失败；
7. `buzz-test-cli` 的 channel subscription 最初错误使用 `#e`，修复为 NIP-29 `#h`；
8. 模型硬门禁逐 Agent 通过，但 `model-proof.txt` 收集正则多转义了一层，只汇总出
   `agent_pool_ready`。原始日志仍逐 Agent 保存了 `applied model`，并据此复核为
   C6 6/6、C10 10/10、C12 12/12；后续 Runner 已修正正则。

前五项和证据汇总问题发生在 Harness；cancel-drain/respawn 则是真实 ACP runtime
稳定性问题，不能归咎于 Harness。第一次真实验收同时也是对验收工具本身的资格测试；
正式重复必须使用修复后的同一 Runner 版本和 hash。

## 7. 当前尚未证明的内容

正式签收至少还缺：

1. C4、C6、C10、C12 每个 Tier 三次独立 cold-start 重复；
2. C12 至少 60 分钟、有持续 workload 的 soak；
3. Participant crash、Moderator crash、Relay restart、provider stall、429/5xx、
   long tool read、prompt injection、End during Turn 故障矩阵；
4. 统一关联的 Provider/ACP trace、首个 provider update、token 与成本数据；
5. 固定 barrier 下的真实同时在途 Turn。当前样本证明多个 Agent/Meeting 进程并存并真实
   工作，但驱动以自然调度和逐场定向复核为主，不等价于 12 路严格同步冲击；
6. 全量人工质量评分；
7. 同一个 Meeting 内 6/10/12 Agent。当前 C6/C10/C12 是跨多个 Meeting 的总 Agent
   并发，受当前单场 4 Agent 契约限制；
8. `just ci` 和正式发布候选 commit 上的全门禁复跑。

Agent Speech 的尾延迟是本轮最值得持续观察的指标。C10/C12 最大值约 239–243 秒，只给
270 秒本地模型截止留下约 27–31 秒余量。正式重复应记录 p95，但单次样本量不足 30 时只
报告原始值与最大值，不能发布不稳定的 p95。

## 8. 下一步

建议按以下顺序继续：

1. 先诊断并修复 Moderator 快速状态切换下的 cancel-drain churn，或设计不需要立即取消
   provider Turn 的协调策略；
2. 为该时序增加确定性 ACP 回归测试，并确保 Runner 对 respawn/uncertain fail-closed；
3. 把修复和 Runner 固化到一个 commit，以该 commit 重新构建并记录二进制 hash；
4. 重跑完整 `just test-meeting-backend`；
5. 从 C4/C6 开始重新 qualification，再进入 C10/C12；每 Tier 最终达到三次 clean run；
6. 然后执行故障矩阵、C12 60 分钟 soak 和人工 rubric 评分；
7. 汇总后再做 production go/no-go；同场容量扩展另立设计，不与当前验收混合。
