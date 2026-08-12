# Meeting Desktop Human Grant 软租约未续期修复设计

> 状态：修复已实现并通过 native/Desktop 自动化回归；待真实 Meeting 60～90 秒现场验收
>
> 记录日期：2026-08-07
>
> 范围：Meeting V2 Human Floor、Desktop/Tauri、Grant Progress、实时状态收敛与测试隔离
>
> 关联设计：
> [Meeting V1 产品设计](../meeting/v1/meeting-v1.md)、
> [Meeting V1 后端实现设计](../meeting/v1/meeting-v1-backend-implementation-design.md)、
> [Meeting Desktop 产品规格](../meeting/desktop/meeting-desktop-spec.md)、
> [Meeting Desktop 实现计划](../meeting/desktop/meeting-desktop-implementation-plan.md)、
> [Meeting Desktop 多频道实时订阅失效修复设计](meeting-desktop-multi-channel-live-subscription-stale-snapshot-fix-design.md)

## 1. 结论

最近一场“面向 Agent 的记忆系统：产品定位与首期边界评审”整体已经正常完成，但其中的
Human Floor 生命周期暴露了一个确定的 Desktop 协议实现缺口：

- Relay 为每个 Speech Grant 同时设置 30 秒 soft lease 和 5 分钟 hard deadline；
- Grant holder 必须按 Progress 协议续期 soft lease，任何续期都不能越过 hard deadline；
- 协议明确要求 Human 客户端在输入期间发送 `stage=composing` 的 Progress；
- CLI 可以手工发送 Progress，ACP 会为 Agent 自动发送 Progress；
- Desktop 只显示 5 分钟 hard deadline，却没有发送任何 Human Grant Progress；
- 因此 Human 接受 Offer 后，如果未在约 30 秒内提交 Speech 或 Yield，Relay 必然以
  `grant_soft_expired` 回收发言权，即使 Desktop 仍显示还有约 4 分半钟。

这不是主持人提前结束 Human 发言，也不是 Relay 把 5 分钟计算错了。Relay 按冻结的 timing profile
正确执行了 soft lease；错误在于 Desktop 展示了 hard deadline，却没有履行维持该时间窗口所需的
Progress 协议。

正确修复是：**为当前 Human 持有的精确 Grant 建立 Desktop native renewal task，按权威
`soft_lease_expires_at` 和 `progress_seq` 自动提交 `composing` Progress，直到 Speech、Yield、Grant
终结、身份/Community 切换或 5 分钟 hard deadline。**

不修改 Relay 的 soft lease 语义，不让 React 定时器直接承担协议存活性，也不迁移、删除或重写任何
已有 Meeting 数据。

## 2. 故障记录

### 2.1 会议与冻结配置

本次诊断选择标题相同的会议中创建时间最新的一场：

```text
Meeting:                 103fe83a-d398-4738-bb11-3eb4af742958
created_at:              2026-08-07 11:52:00 +08:00
ended_at:                2026-08-07 12:02:21 +08:00
terminal status:         ended / closed
timing profile:          moderated-board-v1-baton-default
human_offer_ack_ms:      15000
grant_soft_lease_ms:     30000
progress_interval_ms:    10000
grant_hard_deadline_ms:  300000
moderator_decision_ms:   180000
```

当前 Human：

```text
f6e7bfa49dd1d137dc43674703c3a225651c84d84e673087ec40a20a490d9e37
```

### 2.2 Human 发言权时间线

| 时间（Asia/Shanghai） | 权威事件 | 结果 |
|---|---|---|
| 11:54:32.746 | `human_requested` | Human Floor Request 已提交并进入优先路径 |
| 11:54:39.612 | `offer_acked` | Grant `955151af...cc546` 创建并激活 |
| 11:55:09.612 | soft lease deadline | 30 秒内没有收到任何 Progress |
| 11:55:09.697 | `grant_soft_expired` | Relay 回收 Grant，控制权返回主持流程 |
| 11:59:39.612 | hard deadline | 原本的 5 分钟绝对上限；实际未到达 |

该 Grant 的最终状态为：

```text
state:                  soft_expired
progress_seq:           0
accepted Progress rows: 0
Speech:                 none
Yield:                  none
```

从 Grant 创建到回收约 30.085 秒，与冻结的 `grant_soft_lease_ms=30000` 一致。Relay runtime 同时记录：

```text
Recovered due Meeting V1 baton transitions
meeting=103fe83a-d398-4738-bb11-3eb4af742958
transition_count=1
```

因此可以排除：

- 主持人 Recall；
- Human 主动 Yield；
- Meeting 提前关闭；
- 5 分钟 hard deadline 计算错误；
- Speech 提交失败后被误判成功；
- Relay/ACP 进程崩溃。

### 2.3 会议其余流程

本场 Meeting 除 Human Grant 外正常推进：

- State Revision 最终为 `64`；
- 共有 7 个 Grant，其中 6 个进入 `spoken`，只有 Human Grant 为 `soft_expired`；
- 形成 6 次 canonical Speech；
- 没有 `moderator_decision_timed_out`；
- 主持 Agent 的较长 Grant 产生 2 次 `grant_progressed`，随后正常 Speech；
- Action Run `6264e8e5-bc50-4062-8221-7939b66684de` 完成 7 次 lease renewal；
- Action Run 最终为 `completed_closed`，Meeting 最终为 `ended / closed`。

所以本次不是整场 Meeting 运行异常，而是 Human Grant 的 Desktop 续约路径缺失。

### 2.4 用户影响

该缺陷不是本场会议或某个账号的偶发状态。只要同时满足以下条件即可稳定复现：

1. Human 从 Desktop 接受 Meeting Offer；
2. Relay 为其创建活动 Grant；
3. Human 没有在第一个 30 秒 soft lease 内提交 Speech/Yield；
4. 没有通过 CLI 额外手工发送 Grant Progress。

因此所有 Community、所有 Human participant、Human moderator 被 Directed Handoff/fallback 指向的
发言路径都会受影响。Agent 不普遍受影响，是因为 ACP 已经实现了自动 Progress。

## 3. 预期协议语义

### 3.1 两个 deadline 不是二选一

Grant 的时间模型为：

```text
soft lease        30 秒：检测持有方客户端/runtime 是否仍存活
progress cadence  10 秒：在可观察活动期间续期 soft lease
hard deadline      5 分钟：本轮发言不可突破的绝对上限
```

每次合法 Progress 将 soft deadline 更新为：

```text
min(database_now + grant_soft_lease, hard_deadline)
```

所以“5 分钟”表示客户端持续存活时能够使用的最长发言窗口，不表示获得 Grant 后无需任何续约即可静默
占有 5 分钟。

### 3.2 Human Progress 是既有协议的一部分

Meeting V1 后端实现设计已经规定：

- Progress 由 Harness 或 Human 客户端根据可观察阶段确定性发送；
- Human 输入期间使用 `composing`；
- Progress 不由 LLM 决定；
- Progress 不改变 Speech/Intent revision，不产生正式发言；
- Progress 只能由当前 active Grant holder 提交；
- `progress_seq` 必须严格单调；
- Progress 不能复活已经到期或终结的 Grant。

因此无需发明新协议、事件 kind 或 Relay 授权模型。需要补齐的是既有 kind `42109` 的 Desktop Human
客户端实现。

### 3.3 soft lease 仍有必要

不建议把所有 Human Grant 改成只使用 5 分钟 hard deadline。这样虽然能掩盖当前问题，却会导致：

- Desktop 崩溃或退出后仍占用 Floor 最长 5 分钟；
- 网络断开后主持人和其他参与者长期等待；
- Human 与 Agent 使用不同的 Grant 终结模型；
- 已有 recovery、测试和观测语义分叉。

修复客户端续约后，soft lease 可以继续承担“Desktop/runtime 已消失”的快速恢复职责。

## 4. 根因

### 4.1 Desktop API 没有 `grant_progress`

`../../../desktop/src/shared/api/tauriMeetings.ts` 的 `MeetingFloorAction` 只包含：

```text
request / withdraw
offer_ack / offer_decline
grant_yield
speech
```

`../../../desktop/src-tauri/src/commands/meetings/floor.rs` 的 native `MeetingFloorAction` 与 builder dispatch 也只有
同一组动作。Desktop 没有任何路径构造并签名 `build_meeting_v2_grant_progress(...)`。

### 4.2 UI 只消费 hard deadline

`MeetingSpeechComposer` 的倒计时为：

```text
grant.hardDeadlineMs - Date.now()
```

它没有使用已经投影到 `MeetingGrant` 中的：

- `softLeaseExpiresAtMs`；
- `progressSeq`。

因此界面在 Relay 已经接近回收 soft lease 时仍显示约 270 秒，回收后只能等待新的 canonical snapshot
让 composer 消失。用户看到的时间承诺与客户端实际可维持的时间不一致。

### 4.3 React 层没有 renewal owner

当前 Meeting 页面存在 deadline refetch 和实时 snapshot invalidation，但没有任何 Human Grant renewal
生命周期。仅在 `Textarea.onChange` 里补一个 Progress 也不正确：Human 可能在阅读、思考、使用其他页面
查资料或等待输入法，而 Desktop 进程仍然健康。

Progress 表达的是 holder 客户端仍然存活并承担当前 Grant，不是“刚刚敲了一个键”。

### 4.4 自动化没有覆盖真实软租约窗口

现有 Desktop E2E 为 Grant 注入 soft/hard deadline，但测试会立即提交 Speech/Yield，没有让 Human
composer 跨过真实的 30 秒 soft lease，也没有断言 Desktop 发出 `composing` Progress。测试因此只能证明
按钮和一次性命令，不能证明完整的 Human Grant 生命周期。

## 5. 修复边界与不变量

### 5.1 必须保持

1. Relay 继续权威执行 frozen timing profile、soft expiry 和 hard expiry；
2. Progress 只能由当前签名身份为精确 active Grant holder 时提交；
3. 每个 Progress 使用 canonical `progress_seq + 1`；
4. 同一个待确认 Progress 必须重放同一个已签名事件，不能因网络结果不明确制造多个 seq；
5. 任何续约都不能越过 Relay hard deadline；
6. Speech、Yield、Recall、Expiry、Meeting End 后立即停止续约；
7. Community、Relay 或身份切换时同步取消旧 renewal task；
8. React event/timer 只触发 native ensure，不直接成为协议权威；
9. UI 仍以重新读取的 canonical snapshot 决定 Grant 是否有效；
10. 草稿在 Grant 丢失后可以保留供复制，但不得自动提交到新 Grant；
11. 不迁移、不删除、不回写任何已有 Meeting、Speech、Board、Action 或 Project View 数据。

### 5.2 本次不做

1. 不删除或放宽 Relay soft lease；
2. 不调整默认 30 秒、10 秒、5 分钟 timing profile；
3. 不让 Progress 产生 Speech、Board 更新或业务权限；
4. 不允许 Human Desktop 替 Agent Grant 续约；
5. 不允许 managed-by/owner 关系替代 frozen participant 与 holder 身份；
6. 不改变 ACP 已工作的 Agent Progress 路径；
7. 不用本地倒计时自行宣告 Grant 已过期；
8. 不让旧 Grant renewal task 对新的 Grant 或新的 Meeting identity 生效；
9. 不对已结束 Meeting 补写伪造 Progress。

## 6. 修复方案

### 6.1 增加独立的 Human Grant renewal runtime

参考已经通过现场验收的 Human Action renewal 模式，在 Tauri `AppState` 增加独立的
`MeetingGrantRenewalRuntime`。它与 `MeetingActionRenewalRuntime` 分开，避免 Floor Grant 和 Action
window 的 fence、终止条件及 receipt 语义混用。

精确 binding 至少包含：

```text
api_base_url
signer_pubkey
meeting_id
grant_id
hard_deadline_ms
```

registry key 使用：

```text
api_base_url + signer_pubkey + meeting_id
```

行为要求：

- 相同 binding 重复 ensure 返回 `already_active`；
- 同一 Meeting 出现新 `grant_id` 时取消旧 generation，再启动新 task；
- 旧 generation 的 `finish()` 不能删除新 task；
- workspace/Community 切换、identity replace/clear 和应用退出取消全部 task；
- claim 不保存私钥，task 每次签名前重新读取当前 Community 与 identity。

现有 Action renewal 已经具备上述 generation、cancel 与 identity-bound 模式。实现可以复用其结构思想和
小型无状态 helper，但不应在本次大幅重构已经稳定运行的 Action renewal 主路径。

### 6.2 新增幂等 ensure 命令

新增 Desktop native 命令：

```text
ensure_meeting_human_grant_renewal({ meetingId, grantId })
  → started | already_active
```

ensure 执行前必须重新读取并验证 canonical snapshot：

1. Relay 仍支持当前 Meeting V2 协议；
2. Meeting 未终结；
3. 当前签名身份位于 frozen roster 且 participant type 为 Human；
4. `floor.grant.grant_id` 与输入完全相同；
5. Grant holder 等于当前 signer pubkey；
6. Grant 仍 active，当前时间未达到 hard deadline；
7. snapshot 的 State、Grant ID、deadline 与 progress sequence 通过现有完整性校验。

React 每次看到“当前 Human 持有 active Grant”时都可以调用 ensure。重复 render、live invalidation 或
snapshot refetch 不会启动重复 renewal task。

### 6.3 native renewal loop

renewal loop 不依赖浏览器 `setInterval`，以避免窗口后台、React 重挂载、路由切换和浏览器节流导致
软租约失效。

每轮执行：

1. 重新读取当前 identity、Community 和 canonical Meeting snapshot；
2. 若 exact Grant 不再 active，立即结束；
3. 读取 canonical `progress_seq`、`soft_lease_expires_at_ms` 和 `hard_deadline_ms`；
4. 在 soft deadline 的安全余量前调度下一次续约，正常 profile 下约每 10 秒一次；
5. 构造 `progress_seq + 1`、`stage=composing` 的 V2 Grant Progress；
6. 使用当前 Human identity 签名并提交；
7. 验证响应 event ID、accepted outcome、canonical object/fence；
8. 重新读取 canonical snapshot，确认 progress sequence 已推进并取得新的 soft deadline；
9. 重复直到 exact Grant 终结或 hard deadline 到达。

调度不能只硬编码 10 秒。native task 应以权威 `soft_lease_expires_at_ms` 计算剩余时间，并使用有界安全
余量；这样即使某场 Meeting 使用不同冻结 profile，Desktop 仍不会在 soft deadline 之后才发送。

建议规则：

```text
remaining_soft = soft_deadline - local_now
next_delay = min(10s, remaining_soft / 2)
inside safety margin → 立即提交
```

本地时间只决定“何时尽早发送”，不能决定 Relay 是否接受；Relay 数据库时间仍是权威期限。

### 6.4 不明确提交结果与 sequence reconciliation

网络失败可能发生在 Relay 已接受 Progress 之后。此时不能直接创建下一个签名事件。

task 必须保存当前 prepared event：

```text
grant_id + progress_seq + event_id + signed event
```

恢复规则：

- 明确接受：清除 prepared event，回读 canonical State；
- 请求可能已到 Relay：重放同一个签名事件，并同步 canonical State；
- 回读发现 `canonical progress_seq >= prepared progress_seq`：本次已被接受或被等价推进，清除 prepared；
- 回读仍为旧 seq：在 soft deadline 前继续重放同一 event；
- `grant_not_active`、`grant_already_terminal`、身份/Community/fence 不匹配：结束 task；
- `stale_progress_sequence`：回读 canonical Grant；只有 exact Grant 仍 active 时才从新的 seq 重新准备；
- soft/hard deadline 已经过期：不得尝试复活。

同一个 Desktop 进程内只允许一个 exact Grant renewal owner，避免自身并发争用 sequence。

### 6.5 Frontend 激活与展示

Meeting screen controller 在 verified snapshot 满足以下条件时调用 native ensure：

```text
snapshot.lifecycle 非终态
floor.grant != null
floor.grant.holderPubkey == current Human pubkey
```

Frontend 不需要自己每 10 秒 invoke。native task 启动后，即使用户临时切换到 Project View 或其他页面，
只要 Desktop、Community 和 identity 仍然有效，就继续维持同一个 Grant，直到 hard deadline 或权威终结。

`MeetingSpeechComposer` 继续显示 hard deadline，但文案应明确这是本轮最晚结束时间。异常情况下：

- ensure 启动失败：显示“无法维持发言权，正在重新确认会议状态”，并立即 refetch；
- live snapshot 显示 Grant 已终结：关闭发送能力，保留草稿；
- 网络中断：使用现有断线状态，不承诺倒计时仍有效；
- hard deadline 本地归零：只触发 canonical refetch，不本地伪造 expiry。

soft deadline 是 runtime liveness 机制，不必作为第二个主倒计时暴露给普通用户；但诊断日志必须能够说明
最近一次 accepted Progress、seq 和下一 soft deadline。

### 6.6 与现有 live sync 的关系

Progress 接受后会增加 `floor_revision` 并产生新的 Relay State。现有 Meeting live subscription 会使
snapshot cache invalidated，低频 canonical polling 继续作为兜底。

renewal loop 不能依赖 React Query 已经及时收到该 State；native task 必须自己回读 canonical snapshot
完成 sequence reconciliation。这样即使 UI 实时订阅临时延迟，也不会重复准备错误 seq。

UI 不需要把每次 `grant_progressed` 渲染成正式时间线内容或普通未读。它只更新当前 Grant 的租约投影。

### 6.7 可观测性

增加结构化日志/指标，至少包含非敏感字段：

```text
meeting_id
grant_id
progress_seq
renewal_outcome
soft_remaining_ms
hard_remaining_ms
retry_reason
stop_reason
```

建议稳定 stop reason：

```text
spoken
yielded
soft_expired
hard_expired
recalled
meeting_ended
identity_changed
community_changed
grant_replaced
definitive_rejection
```

不得记录私钥、签名密钥材料或完整 Speech 草稿。

## 7. 实现范围

预计修改：

| 层 | 文件/模块 | 工作 |
|---|---|---|
| Tauri runtime | `../../../desktop/src-tauri/src/meeting_runtime.rs` | 增加独立 Human Grant renewal registry、generation 与 cancel |
| Tauri state | `../../../desktop/src-tauri/src/app_state.rs` | 注册 `meeting_grant_renewals` |
| Identity/workspace | `../../../desktop/src-tauri/src/commands/identity.rs`、`workspace.rs` | 与 Action renewal 一起同步取消 |
| Meeting native | `../../../desktop/src-tauri/src/commands/meetings/floor.rs` 或独立 `grant_renewal.rs` | exact Grant 回读、Progress builder、提交/重试/reconciliation |
| Tauri registry | `../../../desktop/src-tauri/src/lib.rs`、`commands/meetings.rs` | 导出并注册 ensure command |
| Desktop API | `../../../desktop/src/shared/api/tauriMeetings.ts` | ensure input/result 与 invoke 包装 |
| Desktop UI | Meeting screen/Floor controller | verified own-Grant 出现时幂等 ensure、失败反馈 |
| Composer | `../../../desktop/src/features/meeting/ui/MeetingSpeechComposer.tsx` | truthful hard-deadline/renewal failure 展示 |
| Mock/E2E | `../../../desktop/src/testing/e2eBridge.ts`、`../../../desktop/tests/e2e/meeting-floor.spec.ts` | renewal invocation、跨软租约、导航与终止测试 |

Relay、DB、SDK builder 和 ACP 的主协议能力已经存在。本次只有在发现 receipt 缺少 Desktop 必需的稳定
reconciliation 字段时才补充 Relay response；不得改变 Progress 的授权、sequence 或 deadline 语义。

## 8. 测试方案

### 8.1 Native 单元测试

1. 相同 exact Grant 重复 ensure 只启动一个 task；
2. 新 Grant 替换旧 Grant 并取消旧 generation；
3. 旧 task finish 不会移除新 task；
4. identity/workspace cancel 会结束全部 Human Grant task；
5. 只有 frozen Human active holder 能启动；
6. Agent holder、非 roster Human、managed-by owner、旧 Grant 全部拒绝；
7. Progress 使用 canonical `progress_seq + 1` 和 `stage=composing`；
8. 调度发生在 soft deadline 安全余量之前；
9. hard deadline 限制永远不被续约突破；
10. ambiguous submit 重放相同 event ID；
11. canonical seq 已推进时不重复发送旧/新 seq；
12. Speech、Yield、soft/hard expiry、Meeting End 后 task 停止。

### 8.2 Desktop 测试

1. own Human Grant 出现时调用 ensure；
2. 重渲染和 snapshot Progress 更新不会启动多个 task；
3. Agent Grant 不调用 Human renewal；
4. hard deadline 倒计时继续正确显示；
5. ensure 失败时不继续展示无条件可用的 5 分钟承诺；
6. Grant 终结后 composer 禁用但草稿保留；
7. 路由切换后 native renewal 保持，返回 Meeting 后仍读取同一 active Grant；
8. Community/identity 切换后旧 task 不会继续签名。

### 8.3 Relay/DB 集成验收

使用隔离的临时数据库和唯一 Meeting/Community，禁止连接或清理本地开发主数据库：

1. Human ACK 后保持 90 秒不提交 Speech；
2. 至少观察到多个严格递增的 `grant_progressed`；
3. Grant 在第一个 30 秒之后仍为 active；
4. 90 秒时提交 Speech，Relay 接受且只生成一条 canonical Speech；
5. 停止 Desktop renewal 后，新 Grant 在 soft lease 后被 `grant_soft_expired`；
6. 持续 renewal 到 5 分钟时，Relay 仍按 hard deadline 终结；
7. 断网后请求不明确，恢复连接不产生 sequence 分叉或双 Progress；
8. 所有测试只删除自己的临时容器/数据库，不执行共享 schema 的 destructive reset。

### 8.4 回归测试

- Agent ACP Progress 与 Speech 不回归；
- Human Request、Offer ACK/Decline、Speech、Yield 不回归；
- Human Action renewable lease 与 `completed_closed` 不回归；
- Meeting live sync、终态目录收敛和 Community 切换不回归；
- Project View、Document 和普通 Channel 数据不参与本次测试清理。

## 9. 验收标准

修复只有同时满足以下条件才可视为完成：

1. Human 接受 Offer 后可以思考/输入超过 30 秒，并在 5 分钟内正常发表；
2. Relay 历史显示同一 Grant 的 Progress seq 严格递增；
3. Desktop 同一 Grant 只有一个 native renewal task；
4. UI 显示的剩余 hard deadline 与实际可用窗口一致；
5. Desktop 崩溃、退出或身份/Community 切换后，soft lease 能按设计回收 Grant；
6. Progress 不越过 hard deadline，不复活 terminal Grant；
7. Speech/Yield 成功后不再产生迟到 Progress；
8. 网络结果不明确时没有双事件、seq 跳跃或错误续期新 Grant；
9. Agent Grant 与 Action Finalization renewal 行为不受影响；
10. 真实 Meeting 现场验收中，Human 等待至少 60～90 秒后仍能成功提交 Speech；
11. 实现和测试过程中没有迁移、清理或覆盖任何现有开发数据。

## 10. 交付顺序

1. 增加 native Grant renewal registry、exact binding 和取消生命周期；
2. 实现 ensure command、canonical reload、Progress 签名和 sequence reconciliation；
3. 接入 `AppState`、identity/workspace cancel 和 Tauri command registry；
4. 接入 Desktop verified own-Grant ensure 与失败展示；
5. 补齐 native、Desktop E2E 与隔离 Relay/DB 测试；
6. 构建并在不删除现有数据的前提下重启 Desktop/ACP；
7. 新建现场 Meeting，完成“Human 持有 60～90 秒后提交 Speech”的验收；
8. 回读 Grant Progress、Speech、Meeting terminal 状态并更新本文状态。

## 11. 实现与自动化验收记录（2026-08-07）

### 11.1 已实现

- Tauri 增加独立 `MeetingGrantRenewalRuntime`，binding 精确绑定
  `api_base_url + signer_pubkey + meeting_id + grant_id + hard_deadline_ms`；
- 相同 Grant 的重复 ensure 幂等返回，替换 Grant 会取消旧 generation，旧 task 结束时不会删除新 claim；
- workspace/Community 切换、identity replace/clear 和 sign-out 会同时取消 Human Grant renewal；
- 新增 `ensure_meeting_human_grant_renewal` native 命令，只接受 frozen roster 中、由当前签名 Human
  身份持有的 exact active Grant；
- native task 使用 Relay 冻结的 `progress_interval_ms` 与 canonical soft deadline 调度，提交
  `stage=composing`、`progress_seq + 1` 的 Progress；
- 提交结果不明确时保留并重放同一已签名 event，只有 canonical sequence 已推进后才分配下一 sequence；
- task 在每次签名前重新读取当前 Community、identity 和 canonical Meeting State；
- Desktop 在 verified own-Human-Grant 出现时调用幂等 ensure；native task 不依赖 React timer，页面路由
  切换不会中断已经建立的续约；
- ensure 失败时 UI 给出明确告警、触发 canonical refetch 并重试，同时保留未提交 Speech 草稿；
- 发言倒计时明确标注为 `Hard limit`，避免把 5 分钟绝对上限误解成无需续约的本地计时器；
- Agent Grant 不进入本路径；既有 ACP Progress、Action Finalization renewal、Relay/DB 授权与时限语义
  均未修改。

### 11.2 已通过的自动化检查

```text
cargo test --manifest-path desktop/src-tauri/Cargo.toml grant_renewal
  4 passed

cargo test --manifest-path desktop/src-tauri/Cargo.toml meeting_runtime::tests
  6 passed

pnpm exec playwright test tests/e2e/meeting-floor.spec.ts --project=smoke
  11 passed

cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
  passed

cargo fmt --manifest-path desktop/src-tauri/Cargo.toml --check
pnpm exec biome check <本次 Desktop 变更文件>
pnpm run typecheck
pnpm run build
git diff --check
  passed
```

Desktop E2E 已覆盖 own Human Grant 触发 ensure、Agent Grant 隔离、ensure 失败告警和草稿保留；native
单元测试覆盖 Progress exact binding/sequence/stage、receipt binding、错误分类、软租约安全调度，以及
renewal registry 的幂等、替换和取消语义。

### 11.3 尚待现场验收

自动化 mock 不伪装成真实 Relay 时间推进，因此验收标准第 1、2、10 项仍需新建一场真实 Meeting：

1. Human 接受 Offer 后保持 60～90 秒不提交；
2. 确认 Grant 跨过首个 30 秒 soft lease 后仍为 active；
3. 在 hard deadline 内提交 Speech 并确认成功；
4. 回读同一 Grant 的严格递增 Progress sequence；
5. 确认 Agent Grant、主持决策和 Action Finalization 没有行为变化。

本次实现、构建和自动化测试均未连接、迁移、清理或覆盖现有开发 Relay/数据库数据。
