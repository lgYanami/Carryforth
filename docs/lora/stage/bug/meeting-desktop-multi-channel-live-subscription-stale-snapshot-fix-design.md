# Meeting Desktop 多频道实时订阅失效与 Snapshot 停滞修复设计

> 状态：核心修复已实现；自动化验证通过；待真实 Relay 现场会议验收
>
> 记录日期：2026-08-06
>
> 范围：Desktop Meeting 实时同步、React Query cache 收敛、Relay REQ 频道索引语义
>
> 关联设计：
> [Meeting V2 Floor Decision 空等与 Action Finalization 硬超时修复设计](meeting-v2-floor-decision-and-action-finalization-timeout-fix-design.md)、
> [Meeting V2 Board→Action 连续性、Return-to-Board 投影与 Desktop 终态收敛修复设计](meeting-v2-board-action-continuity-return-to-board-and-directory-convergence-fix-design.md)

## 1. 结论

最近一次会议“对话聊天 Agent：第一阶段交付切片评审”在 Relay 和 ACP 侧已经正常推进并关闭：

- Meeting：`0ed366aa-6f94-4eff-83db-b8bf081fbf35`；
- canonical 终态：`status=ended`、`terminal_outcome=closed`、State Revision `64`；
- Action Run：`5514df7e-002f-44d6-9e51-118a4e24cbd1`；
- Action Finalization 持续约 3 分 31 秒，期间 8 次 lease renewal 均被接受，最后以
  `terminal_status=completed_closed` 正常结束；
- Floor 的有效决策通常在 6～9 秒内提交；两次异常 attempt 分别在约 30 秒和 24 秒后被判为
  `invalid_output`，没有等待 3 分钟 decision deadline，Relay 随后正常恢复推进。

因此用户看到的“页面还在等待/似乎超时”，不是本轮 Meeting 又发生了 3 分钟协议超时，也不是
Action renewable lease 失效，而是 **Desktop 没有收到 Meeting 实时失效信号，当前页面一直渲染旧的
canonical snapshot cache**。切换到其他页面再切回来会触发组件重挂载和 snapshot refetch，于是立即显示
真实的新状态。

跨层根因已经定位：

1. Desktop 把最多 64 个 Meeting UUID 放进同一个 `#h` filter；
2. Relay 只能为一个实时订阅提取一个频道 UUID；遇到多个不同 `#h` 时返回 `None`；
3. 该订阅因此被登记到 global subscription index，但 Meeting State/Speech/End 都是
   `channel_id=Some(meeting_id)` 的频道事件；
4. Relay 的安全 fan-out 不会把频道事件投递给 global subscription；
5. REQ 本身被接受，没有 `CLOSED` 或前端错误，所以 Desktop 会静默停在旧数据上；
6. 现有 12 秒 fallback 只刷新 Meeting directory，不刷新当前 Meeting snapshot，无法修复主页面停滞。

本次修复采用 Desktop 侧的最小安全边界：**每个非终态 Meeting 建立一个只含单个 `#h` 的实时订阅，
并为当前打开的非终态 snapshot 增加低频 canonical refetch 兜底。** 不修改 Relay 的频道隔离与 fan-out
安全语义，不迁移、不清理、不重写任何 Meeting 数据。

## 2. 故障记录

### 2.1 用户可见现象

会议运行期间，Desktop 当前 Meeting 页面出现以下表现：

1. 主持人实际上已经决定下一位发言人，页面仍显示旧 decision deadline 并继续倒计时；
2. Speech、Board、Action Finalization 等阶段已经在后端推进，页面内容不变化；
3. 页面可能显示“等待超时”，但 Relay canonical state 并未处于相应超时状态；
4. 切换到其他页面再切回 Meeting 后，状态立即跳到最新值；
5. 左侧目录可能在低频 fallback 后更新，而当前 Meeting 主页面仍停留在旧 snapshot。

“切页后恢复”是关键诊断信号：页面重挂载触发 `useMeetingSnapshot()` 的
`refetchOnMount: "always"`，说明 canonical read 正常，失效的是页面驻留期间的刷新链路。

### 2.2 本次 Meeting 的真实协议时间线

Floor Decision attempt 的实际耗时如下：

| Epoch | 耗时 | 结果 |
|---:|---:|---|
| 1 | 6.383 秒 | committed |
| 2 | 6.876 秒 | committed |
| 3 | 8.877 秒 | committed |
| 4 | 8.374 秒 | committed |
| 5 | 7.355 秒 | committed |
| 6 | 30.348 秒 | discarded / `invalid_output` |
| 7 | 24.363 秒 | discarded / `invalid_output` |
| 8 | 7.391 秒 | committed / `action_finalization` |

Epoch 6、7 的 ACP 精确错误为：

```text
moderator_speak requires a self Intent in Candidate Cohort
```

它们属于主持模型产生了不满足当前 Candidate Cohort 的语义动作，Relay/ACP 已按恢复路径丢弃并重试。
它们没有触发 3 分钟 deadline，也不是 Desktop 长期不刷新的原因。

Action Finalization 的实际结果为：

```text
Action Run:      5514df7e-002f-44d6-9e51-118a4e24cbd1
运行时间:        约 3 分 31 秒
accepted renewals: 8
renewal 间隔:     约 25.5 秒
lease TTL:        每次 90 秒
operator cap:     1 小时
terminal_status:  completed_closed
```

这证明上一轮交付的 renewable lease 与 Action close 路径已经生效。页面上继续显示旧硬截止时间，属于
展示缓存陈旧，不能反向解释为后端 lease 已到期。

### 2.3 Relay 事件没有缺失

本次 Meeting 期间 Relay 已产生 canonical Meeting 事件并发布到对应频道路径，其中包括：

- 64 条 kind `42103` Meeting State；
- 最终 kind `42101` Meeting End；
- 7 条 canonical Speech；
- 每条 Meeting 频道事件都带单一、正确的 `h=<meeting_id>`，数据库
  `channel_id=Some(meeting_id)`；
- Human 位于 frozen roster，历史读取和重新挂载回读均成功。

因此问题不是 Relay 没有写入事件、用户无权读取、outbox 失败或数据被删除，而是已经连接的 Desktop
subscription 没有被放入能够收到这些频道事件的索引。

## 3. 根因

### 3.1 Desktop 构造了多值 `#h` live filter

`desktop/src/features/meeting/hooks.ts` 的 `useMeetingLiveSync()` 当前对全部 Meeting ID 排序去重后，按
`MEETING_LIVE_BATCH_SIZE = 64` 分批，并为每批建立一个订阅：

```ts
for (const batch of chunks(stableIds, MEETING_LIVE_BATCH_SIZE)) {
  await relayClient.subscribeLive({
    kinds: [KIND_STREAM_MESSAGE, KIND_MEETING_STATE, KIND_MEETING_END],
    "#h": batch,
    // ...
  });
}
```

只要社区中可见 Meeting 多于一个，该 filter 就携带多个不同频道 UUID。当前现场存在 3 个 Meeting，
因此稳定复现。

故障具有容易误导排查的数量边界：

- 只有 1 个 Meeting 时，batch 只有一个 `h`，实时刷新正常；
- 第 2 个 Meeting 出现后，同一 batch 内的 Meeting 全部静默失去实时事件；
- 超过 64 个 Meeting 时，前面的完整 batch 都不刷新；如果最后一个 batch 恰好只有 1 个 Meeting，
  该 Meeting 反而可能正常刷新，形成选择性故障。

从 Nostr filter 匹配角度看，多值 `#h` 表示 OR，并非非法 JSON；但 Buzz Relay 的实时订阅索引要求一个
subscription 只能被归入一个频道索引。Desktop 没有遵守这个本地 Relay 约束。

在同一个 REQ 中放入多个“各自只有一个 `h`”的 filter 也不能规避问题：
`extract_channel_id_from_filters()` 会跨全部 filters 汇总频道，只要发现两个不同 UUID，整个 subscription
仍返回 `None`。正确边界必须是一次 `subscribeLive()` 对应一个 Meeting。

### 3.2 Relay 将多频道 filter 解析为“逻辑全局”，但安全 fan-out 不会投递频道事件

`crates/buzz-relay/src/handlers/req.rs` 的 `extract_channel_id_from_filters()` 只在所有 filter 都指向同一个
频道时返回 `Some(channel_id)`。遇到两个不同 UUID 时返回 `None`：

```text
多个不同 #h
  → extract_channel_id_from_filters(...) = None
  → register_scoped(..., channel_id=None)
  → global_kind_index
```

`crates/buzz-relay/src/subscription.rs` 的 `fan_out_scoped()` 则维持严格的对称隔离：

```text
event.channel_id = Some(...)
  → 只检查 channel_kind_index / channel_wildcard_index

event.channel_id = None
  → 只检查 global_* indexes
```

因此这个订阅虽然保留了原始 filter，实际却永远不会成为 Meeting 频道事件的候选订阅。
`filters_match()` 没有机会执行。

这里不能简单让 global index 接收频道事件。该隔离用于防止频道内容向全局订阅泄漏，也避免每条频道事件
扫描全部 global subscriptions。为了修一个 Desktop 调用错误而扩大 Relay fan-out，会引入安全、性能和
PubSub topic 路由的跨层风险。

### 3.3 订阅是“静默失效”，现有重试不会触发

REQ 在语法和权限层面被接受，Relay 正常返回 EOSE 并登记 subscription，没有向 Desktop 发送错误。
因此：

- `subscribeLive()` promise 正常完成；
- Desktop 不记录 `Failed to subscribe to Meeting updates`；
- exponential retry 不启动；
- snapshot→subscription race 的初始 `signal()` 只刷新一次，之后再无信号；
- 用户看到的是永久陈旧，而不是明确的断线状态。

只修 retry 或增加错误 toast 无法解决，因为当前系统没有认为订阅失败。

### 3.4 Directory fallback 不能刷新当前 snapshot

上一轮修复已为 `useMeetingDirectory()` 增加 12 秒低频 canonical fallback：只要目录中存在 readable
且非终态 Meeting，就重新执行 `listMeetings()`。它解决的是 sidebar directory 漏掉终态事件后的最终
收敛，并且该机制本身应保留。

当前打开的主页面使用独立 query key：

```text
meetingSnapshotQueryKey(communityId, meetingId)
```

`useMeetingSnapshot()` 只有：

- `refetchOnMount: "always"`；
- `refetchOnWindowFocus: true`；
- 没有驻留期间的 `refetchInterval`。

当前 deadline hook 会在本地旧 deadline 到零时再触发一次 snapshot refetch。这会进一步制造“后端真的
等满了旧 deadline”的错觉：页面不是在实时跟随后端推进，而是在陈旧倒计时归零后才偶然自愈。它不能
替代 live invalidation 或稳定 fallback。

Directory fallback 更新的是：

```text
meetingDirectoryQueryKey(communityId, meetingIds)
```

两份 cache 不会因为一份被轮询而自动同步。于是左侧目录能够最终收敛，当前页面仍可能无限停留在旧的
phase、deadline、speaker 或 Action 状态。

这补充并收窄了上一份设计中的诊断：此前的 snapshot→directory reconciliation 和 directory fallback
是正确且必要的，但它们只保证“主页面已知道终态时推动目录收敛”和“目录自身最终收敛”，没有保证
selected snapshot 在实时事件丢失时主动收敛。

## 4. 修复边界与不变量

### 4.1 必须保持

1. WebSocket live event 只作为 cache invalidation hint；React 不直接信任 event payload；
2. UI 每次变化仍由 Tauri/native bridge 重新读取并验证 Relay canonical projection；
3. 一个 Meeting 是一个频道，实时订阅必须保持明确的单 `h` scope；
4. Community 切换时必须清理旧社区全部 Meeting subscription、timer 和 pending retry；
5. reconnect 后继续保留短 lookback，并关闭 snapshot→subscription race；
6. 终态 Meeting 不再变化，不能永久保持 live subscription 或 fallback polling；
7. 网络不可用时保留最后一次已验证数据，不伪造新阶段、终态或 deadline；
8. 不迁移、不删除、不重放、不重写任何现有 Meeting、Speech、Board、Action 或 Project View 数据。

### 4.2 本次不做

1. 不改变 Relay 的 global/channel fan-out 隔离；
2. 不把多 `#h` filter 解释为可接收任意可访问频道事件的 global subscription；
3. 不在本次新增 Relay 多频道 subscription index；
4. 不调整 Floor decision deadline 或 Action lease policy；
5. 不把 epoch 6、7 的模型 `invalid_semantics` 混入实时刷新修复；
6. 不用 Desktop 本地倒计时推断 canonical phase 已经推进；
7. 不通过刷新整个应用、切换页面或强制 reload 规避 cache 生命周期问题。

如果未来要在 Relay 原生支持一个 subscription 对应多个频道，需要单独设计：多频道 access
resolution、每频道 PubSub topic retain/release、索引去重、连接/订阅配额、移除订阅时的反向清理及信息
泄漏测试。它不是本次最小修复的前置条件。

## 5. 修复方案

### 5.1 每个非终态 Meeting 建立单频道 live subscription

移除 Meeting live path 的 64-ID batching。规划器应输出：

```text
Meeting A → { kinds: [...], "#h": [A] }
Meeting B → { kinds: [...], "#h": [B] }
Meeting C → { kinds: [...], "#h": [C] }
```

而不是：

```text
{ kinds: [...], "#h": [A, B, C] }
```

实现要求：

1. desired set 由当前 Community 已发现且用户是成员的 Meeting rooms 与 directory 投影共同生成；
2. 已有 `compatibility=ready` 投影时，只订阅 `initializing`、`active`、`finalizing` 等非终态 Meeting；
3. 新建 room 尚无 directory 投影时，将其视为 initializing/unknown 并先建立单频道订阅，避免“必须先收到
   State 才能知道需要订阅 State”的发现死锁；
4. `closed`、`aborted`、unsupported、forbidden、not-found 不建立或继续保持 live subscription；
5. directory 新发现非终态 Meeting 时增量建立订阅；
6. Meeting 进入终态、退出 Community、失去可读性或房间被移除时释放对应订阅；
7. 单个 Meeting 订阅失败时独立清理该 attempt 已建立的 disposer 并重试，不能拆掉其他健康 Meeting
   的订阅；
8. filter builder/planner 必须是可单测的纯函数，并保证每个输出 filter 的 `#h.length === 1`；
9. desired set 只按 Meeting ID、可读性和终态集合变化，不得因 directory `updatedAt` 或普通 snapshot
   revision 改变而反复销毁、重建订阅。

按 Meeting 拆分订阅增加了 subscription 数量，但当前 UI 只对可见、非终态 Meeting 保持订阅，实际规模
有界。相比改造 Relay 多频道索引，这一方案更符合现有安全模型，也能隔离单个 Meeting 的重连故障。

### 5.2 保留 live event 作为低延迟失效信号

收到以下事件时，先确认事件的单个 `h` 与所管理 Meeting 匹配，再以约 150 ms debounce 合并失效：

| Event | 需要失效的 cache |
|---|---|
| kind `42103` Meeting State | 该 Meeting snapshot、directory、必要时 activities |
| kind `42101` Meeting End | 该 Meeting snapshot、directory、channels |
| kind `9` Meeting Speech | 该 Meeting snapshot、speeches、directory |

优先使用精确 query key，而不是每条事件都 invalidation 整个 `meetingQueryRoot`。这样多个 Meeting 并发时
不会产生不必要的全目录、全 snapshot 请求风暴。

无论事件 content 携带什么状态，UI 都不得直接 `setQueryData()` 拼装 canonical snapshot。event 只负责
告诉 React Query“该对象可能变了”，随后由现有 native command 完成签名、schema、权限和投影校验。

### 5.3 为当前打开的非终态 snapshot 增加 canonical fallback

为 `useMeetingSnapshot()` 增加前台低频 fallback refetch，建议复用现有
`MEETING_DIRECTORY_FALLBACK_INTERVAL_MS = 12_000`：

```text
snapshot.status == ready
且 lifecycle 非 closed/aborted
且窗口在前台
  → 每 12 秒 canonical refetch

terminal / unsupported / forbidden / not-found / query disabled
  → 停止 interval
```

职责划分如下：

- 单频道 WebSocket subscription：正常情况下在事件到达后快速刷新；
- selected snapshot fallback：订阅静默失效、漏事件或重连竞态时，在一个 interval 内自愈；
- directory fallback：保证 sidebar 独立 cache 最终收敛；
- window focus / remount：保留为额外恢复路径，但不再是必需操作。

fallback 必须只在前台运行，避免后台长期请求；持续断网时遵循 React Query retry/error 行为并保留最后一次
verified data。恢复连接后的下一次成功读取完成收敛。

### 5.4 正确处理订阅生命周期和重连

不要用“整体 effect 一次创建/整体销毁全部订阅”的实现覆盖动态目录。建议维护
`Map<meetingId, SubscriptionHandle>`，按集合差异执行：

```text
desired active IDs - subscribed IDs → subscribe
subscribed IDs - desired active IDs → dispose
```

每个 handle 至少包含：

- dispose function；
- retry attempt/timer；
- generation 或 cancelled 标记，阻止过期异步 subscribe 回写；
- debounce invalidation state（可按 Meeting 或共享但必须精确失效）。

建立成功后主动 invalidation 一次，继续关闭：

```text
第一次 canonical snapshot 读取
  → live subscription 真正建立
```

之间的竞态。重连沿用 `since = now - MEETING_LIVE_LOOKBACK_SECONDS`，重复事件只触发幂等 query
invalidation，不直接重复写入本地状态。

Community 切换/remount 时必须：

1. dispose 全部 Meeting subscription；
2. 清除所有 debounce/retry timer；
3. 阻止旧 community 的异步 callback invalidate 新 community query；
4. 不新增无法由 `resetCommunityState()` 清理的 module-level singleton。

### 5.5 对非法多频道规划 fail fast

虽然产品实现不再生成多 `#h` filter，仍应在 Meeting live subscription helper 的开发/测试边界验证：

```text
#h 缺失或长度 != 1 → 不发起 subscribe，记录结构化错误
```

这能避免未来为了“优化订阅数量”重新引入同一 BUG。不要依赖 Relay 返回错误，因为当前多值 filter 从
Nostr filter 语法上合法，Relay 也不会发出订阅失败信号。

## 6. 可观测性

开发态或既有结构化日志中增加以下低噪声信号：

1. Meeting live subscription `established / disposed / retrying`；
2. 当前 community 下 desired/subscribed subscription 数量；
3. Meeting ID、收到的 event kind、触发的精确 query 类别；
4. selected snapshot fallback refetch 及成功后的 `stateRevision/lifecycle/phase`；
5. 发现 `#h.length !== 1` 时的显式 invariant violation。

日志不得记录消息正文、Board 正文、私钥、auth tag 或完整 Agent prompt。生产路径避免每次 countdown render
打印日志；只记录订阅生命周期、事件失效和 canonical 收敛节点。

Relay 侧可在后续增加独立诊断 metric，统计“filter 带多个不同 `#h`，因此无法建立单频道索引”的 REQ，
但这不是 Desktop 修复的阻塞项，也不应改变投递语义。

## 7. 测试计划

### 7.1 纯函数与 Hook 定向测试

1. 输入 3 个非终态 Meeting，规划出 3 个 filter；每个 filter 的 `#h` 恰好 1 个值；
2. 输入 1 个 active、1 个 closed、1 个 aborted，只为 active 建立订阅；
3. 已发现 Meeting room 尚无 directory projection 时，仍为其建立 initializing subscription；
4. directory 中 Meeting 从 active 变 closed 后，释放其 subscription；
5. 新发现 active Meeting 时只新增对应 subscription，不重建其他健康订阅；
6. 仅 `updatedAt/stateRevision` 改变不重建 subscription；
7. 一个 Meeting subscribe 失败时，清理该 attempt 的部分 handle 并只重试该 Meeting；
8. kind `42103` 只失效对应 snapshot 与 directory；
9. kind `9` 额外失效对应 speeches；
10. kind `42101` 触发 snapshot、directory 和 channels 收敛；
11. selected active snapshot 在没有任何 live event 时，于一个 fallback interval 内重新读取；
12. snapshot 进入 closed/aborted 后停止 fallback；
13. unsupported/forbidden/not-found 不轮询；
14. effect cleanup 后，迟到 subscribe promise/callback 不再修改 query；
15. Community 切换清理旧订阅和 timer，旧事件不能失效新社区 cache。

### 7.2 Relay 回归测试

本次不改变 Relay fan-out，但应保留/补强既有不变量测试：

1. 单 `#h=A` filter 被登记到 A 的 channel index，并收到 A 的频道事件；
2. global subscription 不接收 `channel_id=Some(A)` 的频道事件；
3. channel subscription 不接收 `channel_id=None` 的全局事件；
4. 多个不同 `#h` 当前不会被错误归入任一单频道 index；
5. 不通过扩大 global fan-out 让测试“变绿”。

第 4 项用于记录 Relay 当前边界，不代表鼓励客户端使用多频道 live filter。

### 7.3 Desktop 集成/E2E

至少构造 3 个 Meeting，以确保旧实现必然生成多 `#h` batch：

1. 保持 Meeting A 主页面一直 mounted；
2. 向 A 依次注入/产生新的 State、Speech、Action renewal、End；
3. 不切换页面、不改变 window focus；
4. 断言 phase、speaker、Board/Action 状态和 deadline 随 canonical revision 更新；
5. 断言 A 的事件不会错误刷新 B/C 的 snapshot；
6. 临时丢弃 live callback，断言 A 在 12 秒 fallback 内仍收敛；
7. 恢复 live/reconnect 后断言 lookback 事件不会造成重复 UI 行或状态倒退；
8. Meeting closed 后断言主页面与左侧目录都显示终态，且 polling/subscription 停止；
9. 切换 Community，断言旧 Meeting 事件不能污染新社区。

测试必须验证“页面驻留时自动刷新”，不能用 navigate away/back 或 remount 作为成功条件。
如果 mock bridge 自己按 Nostr OR 语义正确匹配多值 `#h`，它会掩盖真实 Relay 索引缺陷；因此测试还必须
断言实际发出的每个 REQ/filter 形状，或增加一条使用真实 Relay 的多 Meeting 集成测试。

## 8. 现场验收

修复交付后新建至少 3 个 Meeting，其中一个正常运行并完成：

1. 用户始终停留在该 Meeting 页面；
2. 主持人决策提交后，页面在 live latency 内切换 speaker/phase，不等待旧 deadline；
3. Speech 和 Board 更新自动出现；
4. Action lease renewal 期间页面 deadline/progress 持续更新；
5. Action 完成和 Meeting End 后，主页面与左侧目录自动显示终态；
6. 人为断开/恢复 Relay 后，在重连或最多一个 fallback interval 内收敛；
7. Desktop/ACP/Relay 重启前后既有 Meeting 数据仍可读取；
8. 验收过程不得执行数据清理、破坏性 migration test 或删除 Docker volume。

## 9. 实施顺序

1. 抽取并测试 Meeting live subscription planner；
2. 将 multi-`#h` batch 改为非终态 Meeting 的 per-ID subscription；
3. 实现 per-Meeting subscribe/retry/dispose 生命周期；
4. 将 live invalidation 收敛为按 Meeting 的精确 query keys；
5. 为 selected non-terminal snapshot 增加前台低频 fallback；
6. 增加 Community 切换和迟到 callback 清理测试；
7. 增加多 Meeting 驻留页面 E2E；
8. 构建 Desktop/ACP，保留现有数据库并现场验收。

## 10. 完成定义

满足以下条件后，本 BUG 才能关闭：

- Desktop 不再发出包含多个不同 Meeting UUID 的单个 `#h` live subscription；
- 每个 live Meeting subscription 都被 Relay 登记为明确的 channel-scoped subscription；
- 有多个 Meeting 时，当前页面无需切页、focus 或 reload 就能持续显示 canonical 变化；
- live 事件静默丢失时，selected active snapshot 在一个 fallback interval 内自愈；
- terminal 后 snapshot fallback、directory fallback 和对应 live subscription 全部停止；
- 主页面、左侧目录与 Tauri canonical read 最终一致；
- Relay global/channel 信息隔离不被放宽；
- Floor/Action 修复没有回滚，Action renewable lease 与正式 close 仍通过；
- 不发生数据迁移、清理、删除或历史 Meeting 重写。

## 11. 后续观察项

Epoch 6、7 暴露了主持模型仍可能生成 `moderator_speak`，但 Candidate Cohort 中不存在 self Intent。当前
恢复机制能够在几十秒内丢弃并继续，不会再伪装成 3 分钟超时。它可以作为单独的 prompt/semantic
validation 优化项观察，但不应与本次 Desktop cache 刷新 BUG 合并交付。

Action Begin 日志中还出现过“未包含可验证 lease timing receipt”的非阻塞警告，而 canonical receipt
实际包含 timing 字段且后续 renewal 正常接受。该诊断一致性也可后续单独核对；它没有阻止 Action 执行、
续约或关闭，不能作为本次页面停滞的根因。

## 12. 实施记录

2026-08-06 已完成以下交付：

1. 新增 `desktop/src/features/meeting/liveSync.ts`：
   - `meetingLiveFilter()` 固定生成单值 `#h`；
   - `MeetingLiveSubscriptionManager` 按 Meeting ID 增量建立、重试和释放订阅；
   - 一个 Meeting 失败只重试自身，已建立的其他订阅不重建；
   - late subscribe completion、终态移除和 Community cleanup 均会释放 disposer；
   - `MeetingLiveInvalidationScheduler` 合并事件 burst，并保留 canonical read 期间到达的 trailing refresh。
2. `useMeetingLiveSync()` 改为使用 Meeting rooms 与 directory 共同计算 desired set：
   - 未形成 directory projection 的新建 room 先按 initializing 订阅；
   - terminal、unsupported、forbidden、not-found 从 desired set 排除；
   - kind `42103`、`42101`、`9` 分别精确失效 snapshot、activities、channels、speeches 和 directory；
   - event payload 仍不直接进入 React state，所有展示内容继续由 Tauri canonical read 提供。
3. `useMeetingSnapshot()` 增加 12 秒前台 fallback：
   - 仅 `ready + non-terminal` snapshot 轮询；
   - `closed`、`aborted` 和不可读结果立即停止；
   - `refetchIntervalInBackground=false`，不在 Desktop 后台持续请求。
4. E2E mock 增加 Meeting live filter 形状观测与 channel-scoped State hint，不改变产品协议。

自动化验证结果：

- Meeting live/sync policy 定向单测：9 passed；
- Desktop TypeScript typecheck：passed；
- Desktop 全量 unit tests：3562 passed；
- `meeting-recovery.spec.ts` smoke E2E：7 passed；
- 新增 E2E 已证明 3 个 Meeting 产生 3 条单频道 REQ，当前 Meeting 页面保持 mounted 时收到 State
  hint 后自动回读并更新；
- Desktop E2E build：passed；
- Desktop Biome、file-size、px-text、pubkey-truncation 检查：passed。

本次没有修改 Relay fan-out、数据库 schema 或 migration，也没有执行数据清理、volume 删除或历史
Meeting 重写。剩余工作仅为在保留现有本地数据的真实 Relay/Desktop 环境中召开一次多 Meeting 现场会议，
确认 State、Speech、Action renewal 与 End 全流程无需切页即可持续刷新。
