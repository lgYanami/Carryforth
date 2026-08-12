# Meeting Desktop Agent 执行活动可观察性设计

> 状态：已实现并通过真实 Meeting 验收
>
> 日期：2026-08-07
>
> 范围：Desktop Meeting 页面；不修改 Relay、Meeting 协议、ACP 调度或数据库
>
> 关联设计：
> [Meeting Desktop 产品规格](../desktop/meeting-desktop-spec.md)、
> [Meeting Desktop Board 与进程面板布局修复设计](meeting-desktop-board-floor-layout-fix-design.md)、
> [Meeting Desktop 缺陷修复实施计划](meeting-desktop-defect-fix-implementation-plan.md)

## 0. 结论

Meeting 页面应补齐与普通 Channel 一致的 Agent 执行可观察能力：

1. 左侧会议工作区底部显示当前 Meeting 中正在运行的 Agent；
2. 点击 Agent 后，在右侧查看该 Agent 仅属于当前 Meeting 的实时 Activity，包括 Thinking、工具调用、
   文件操作和运行结果；
3. 宽屏下 `Meeting Board` 与 `Agent activity` 复用同一个右侧栏，不新增第三列；
4. 中窄屏下两者使用互斥的右侧 Sheet；
5. 直接复用现有 observer ingestion、`agentWorkingSignal`、`BotActivityComposerAction` 和
   `AgentSessionThreadPanel`，不建立 Meeting 专用运行状态协议或第二套缓存；
6. Agent Activity 是 owner-scoped、易失的运行可观察信息，不是 canonical Meeting 状态，不能影响
   Board、Floor、Speech、timeout、Action Finalization 或终态判断。

该能力属于 Desktop 信息架构和既有状态接入，不需要数据迁移，也不会写入或改动 Meeting 数据。

## 1. 用户目标

普通 Channel 已经提供以下工作流：

```text
Agent 开始工作
  → 左下角出现 Agents working
  → 点击某个 Agent
  → 右侧打开 Activity
  → 实时查看 Thinking、工具调用和运行输出
```

Meeting 中的 Agent 同样会经过 ACP 执行 Board Maintenance、Floor Decision、Intent、Granted Speech
和 Action Finalization，但 Meeting 页面当前只展示：

- canonical Speech；
- Meeting Board；
- Floor/Host/Action 的权威进程状态；
- Relay 验证的 Meeting activity history。

这些信息可以证明会议协议走到了哪个阶段，却不能回答“主持或参会 Agent 此刻具体在做什么”。本次
优化补齐这一观察面，使用户无需离开 Meeting 或反复切换页面即可查看 Agent 的真实执行进度。

## 2. 术语与两种 Activity 的边界

Meeting 页面已经存在 `MeetingActivityPanel`。它与本次新增的 Agent Activity 不是同一类数据：

| 名称 | 数据来源 | 内容 | 权威性 | 可见范围 |
|---|---|---|---|---|
| Meeting activity | Relay 验证的 Meeting projection | Board、Offer、Grant、Handoff、Action、End 等协议转换 | canonical | 冻结 roster 中有读取权限的 participant |
| Agent activity | owner-encrypted ACP observer frame | Thinking、工具调用、文件操作、运行结果、turn lifecycle | 非 canonical、易失 | 当前 identity 有 observer 查看权限的 Agent |

界面文案必须稳定区分：

- 既有入口继续叫 `Meeting activity`；
- 本次新增面板叫 `Agent activity`；
- 不把 ACP tool call 写入 Meeting activity，也不把 Meeting control event 伪装成 Agent tool call。

## 3. 当前代码基础与真实缺口

### 3.1 Meeting turn 已有正确的 observer scope

ACP 执行普通 Channel 和 Meeting turn 时都使用 `PromptSource::Channel(channel_id)`。对于 Meeting，
这里的 `channel_id` 就是 `meetingId`。`../../../../crates/buzz-acp/src/pool.rs` 随后把该 UUID 写入每个 observer
frame 的 `channelId`。

因此现有数据天然支持：

```text
Agent pubkey + meetingId → 该 Agent 在这一场 Meeting 中的 observer events
```

不需要给 Meeting 新增事件 kind、HTTP 接口或特殊 observer 字段。

### 3.2 App 已经全局接收并归并 Agent 运行事件

`useAgentObserverIngestion` 在 AppShell 层全局挂载，负责：

- 订阅当前 identity 所拥有 Agent 的加密 observer frame；
- 写入 `observerRelayStore`；
- 把 turn start/liveness/terminal 折叠进 `activeAgentTurnsStore`；
- 通过 `agentWorkingSignal` 输出按 channel UUID 聚合的 working 状态。

这意味着即使用户停留在 Meeting 页面，数据也会持续进入 store；此前 Meeting 前端被动刷新问题的
修复与本能力不冲突。

### 3.3 缺口只在 Meeting UI 没有消费既有能力

普通 Channel 已经使用：

- `useChannelWorkingAgentPubkeys(channelId)` 获取正在工作的逻辑 Agent；
- `BotActivityComposerAction` 展示 `Agents working`；
- `AgentSessionThreadPanel` 按 `channelId` 过滤实时和归档 observer event。

`../../../../desktop/src/features/meeting` 当前没有接入这些 API，所以 Meeting Agent 即使正在执行，页面也没有
入口打开其 Activity。这是前端接线和布局缺口，不是 ACP 没有生成日志，也不是 Relay 没有推送状态。

## 4. 产品交互设计

### 4.1 左下角 Agents working

在 `MeetingScreen` 左工作区中，把 Agent 工作提示放在 Speech timeline 与 `MeetingFloorDock` 之间：

```text
┌──────────────────────────────────────┬──────────────────────┐
│ canonical Speech timeline            │                      │
│                                      │    Meeting Board     │
├──────────────────────────────────────┤                      │
│ Agents working: test-1 +2            │                      │
├──────────────────────────────────────┤                      │
│ Meeting process / Floor Dock         │                      │
└──────────────────────────────────────┴──────────────────────┘
```

规则：

- 没有可观察的运行中 Agent 时整行不渲染，不保留空白；
- 单 Agent 时显示头像、名称和现有 compact Activity headline；
- 多 Agent 时沿用现有头像堆叠、数量和 Popover 列表；
- 同一逻辑 Agent 的多个 ACP slot/turn 只显示一项，以 pubkey 聚合；
- 列表只包含冻结 roster 中 `participantType === "agent"` 的 Agent；
- 其他 Channel、其他 Meeting 或未参会 Agent 的运行状态不得进入列表；
- working 信号消失后入口可以隐藏，但已打开的 Activity 面板继续保留历史，直到用户关闭或切换
  Meeting。

该区域使用独立的 `meeting-agent-activity-row`，不侵入 `MeetingFloorDock` 的复杂控制状态机。

### 4.2 宽屏右侧栏：Board 与 Agent Activity 复用

宽屏不增加第三列。右侧栏有两种内容模式：

```text
right rail = board | agent_activity(pubkey)
```

状态转换：

| 用户动作 | 结果 |
|---|---|
| 默认进入 Meeting | 按现有规则显示或隐藏 Board |
| 点击 Agents working 中的 Agent | 右栏切换为该 Agent 的 Activity |
| Activity 中切换另一 Agent | 保持右栏，仅替换选中的 Agent |
| 点击 Activity 的 Back | 返回 Board，并确保 Board 可见 |
| 点击 Activity 的 Close | 关闭 Activity；恢复打开 Activity 前的 Board 显隐状态 |
| Activity 打开时点击标题栏 Board | 立即切回 Board，不执行旧的“反向关闭 Board”逻辑 |
| Human host 进入强制 Board Maintenance | Board 获得优先级并切回前台，避免必需操作被 Activity 遮挡 |

Activity 与 Board 复用 `useResizableMeetingBoardWidth` 的宽度和 resize handle。切换不改变用户保存的
右栏宽度。

`useMeetingBoardDraft` 继续位于 `MeetingScreen` 外层，所以切换到 Agent Activity 不得清空 Board
草稿、stale draft 或 control token。实现应尽量保持 Board DOM/滚动位置；即使采用条件渲染，也必须
通过自动化证明草稿不丢失。

### 4.3 中窄屏

继续沿用当前 `1280px` Meeting Board breakpoint：

- Board 使用既有 Sheet；
- Agent Activity 使用同级 Sheet/overlay；
- 两者互斥，打开 Agent Activity 必须关闭 Board Sheet，打开 Board 也必须关闭 Agent Activity；
- 关闭 overlay 后回到完整 Meeting timeline/Floor，而不是导航到普通 Channel；
- 不允许出现 Board Sheet 与 Activity overlay 同时叠加、双 backdrop 或焦点陷阱嵌套。

### 4.4 Meeting 生命周期

- `active` 与 `finalizing_actions`：正常显示 working 入口；
- `closed` / `aborted`：canonical 终态优先，不再依据陈旧 working frame 显示“仍在运行”；
- Meeting 结束时已打开的 Agent Activity 可以继续作为只读历史查看，关闭后不再自动重新打开；
- Agent Activity 绝不能把 terminal Meeting 显示回 `in progress`。

## 5. 数据选择与权限模型

### 5.1 冻结 roster 是 Meeting 范围的唯一来源

Meeting Agent 候选集必须从 `readySnapshot.participants` 中筛选，不得直接使用整个 Community 的
managed/relay Agent 列表：

```text
frozen roster agents
  ∩ current identity 可观察的 Agent
  ∩ useChannelWorkingAgentPubkeys(meetingId)
  = 左下角 Agents working
```

managed/relay Agent query 只用于补齐名称、头像、状态和 observer 可用性，不能改变冻结 roster。

### 5.2 observer 权限

ACP observer frame 是 owner-scoped 的加密信息。首版只展示：

- 当前 Desktop 本地 managed 的 Agent；或
- NIP-OA profile 明确声明 `ownerPubkey === current identity` 的 relay Agent。

非 owner 参会者仍能看到该 Agent 的 canonical Speech、Offer/Grant 和 Meeting 状态，但不能因为加入
同一场 Meeting 就读取其 Thinking、tool call 或私有执行细节。

如果未来需要“Meeting roster 共享 Agent Activity”，必须单独设计 Relay/observer 授权协议，不能在
Desktop 通过放宽筛选伪造权限。

### 5.3 没有信号不等于 idle

远程 harness、网络中断、observer 未启用或当前用户无解密权限都可能导致没有 working signal。
因此首版只做正向提示：

- 有可信 active-turn/typing signal：显示 `Working`；
- 没有信号：不显示入口；
- 不推断、不显示 `Idle`、`Offline` 或“未参与会议”。

### 5.4 多槽聚合

一个逻辑 Agent 可能有多个 ACP slot，但 UI 身份是 Agent pubkey：

- working 列表按 normalized pubkey 去重；
- Activity panel 合并该 Agent 在 `meetingId` scope 下的 observer event；
- 不向用户暴露“槽 0/槽 1”作为不同参会者；
- turn/session 信息仍可在既有 raw/debug 模式中按当前权限查看，但不进入主列表文案。

## 6. 权威状态与操作边界

Agent Activity 只能观察，不能成为 Meeting 决策输入：

- 不根据 Thinking 文本判断主持人已经完成 Floor Decision；
- 不根据 tool result 判断 Action Finalization 已物化；
- 不根据 observer terminal event 关闭 Meeting；
- 不修改 decision deadline、Grant lease 或 Action lease；
- 不产生 Speech、Intent、Board 或 Meeting activity；
- 不参与 unread/attention 的 canonical 计算。

首版在 Meeting 的 Agent Activity 面板中不提供 `Stop current turn`。普通 Channel 的中断按钮是
Agent 控制面能力，而 Meeting turn 还受 Offer/Grant、moderator control、Action lease 和 fallback
管理；将两者一起开放会扩大本次“观察能力”的边界。若后续确有人工中断 Meeting Agent 的需求，应
单独定义中断后 Relay/ACP 的恢复语义和 UI 反馈。

## 7. 前端实现方案

### 7.1 复用现有 working signal

在 Meeting adapter 中调用：

```text
useChannelWorkingAgentPubkeys(meetingId)
```

该 hook 已经以 observer active turn 为主、typing 为兜底，并在 Community 切换时统一 reset。本次不
新增 Meeting 专用 singleton、Map、轮询或计时器。

Meeting 当前不依赖 typing indicator 才能工作；ACP observer turn 是主路径。也不需要为 Meeting
新增公开 typing 协议。

### 7.2 建立纯 roster adapter

新增 `meetingAgentActivityModel.ts`，提供可单测的纯函数，负责：

1. normalized pubkey 去重；
2. 从冻结 roster 排除 Human/unknown participant；
3. managed Agent 元数据优先于 relay Agent；
4. profile 名称作为 fallback，最后才截断公钥；
5. 根据 current identity 与 `ownerPubkey` 标记 Activity 是否可观察；
6. 将 working pubkey 与可观察 roster Agent 求交集；
7. 保持 roster 的稳定顺序，避免每次 liveness frame 导致列表跳动。

可以复用 `buildChannelAgentSessionCandidates` 的元数据优先级，但不能复用整个
`useChannelAgentSessions`：后者强耦合普通 Channel 的 Thread/Profile 返回栈和成员查询。

### 7.3 新增 Meeting Activity adapter 组件

建议新增两个小组件，避免继续扩大已较大的 `MeetingScreen.tsx`：

- `MeetingAgentActivityDock.tsx`
  - 包装 `BotActivityComposerAction variant="inline"`；
  - 固定传入 `channelId={meetingId}`；
  - 只接收已经过滤后的 roster Agent 和 working pubkeys；
  - 无 working Agent 时返回 `null`。
- `MeetingAgentActivityRail.tsx`
  - 包装 `AgentSessionThreadPanel`；
  - 固定传入 `channelId={meetingId}`，禁止 all-channels 模式；
  - 提供 `Meeting · {title}` scope label 和 Meeting 专用 empty copy；
  - 强制 observation-only，不提供 turn interruption；
  - 宽屏使用 `layout="split"`，窄屏置于 Meeting 管理的 Sheet 中。

`AgentSessionThreadPanel` 只需补充通用的展示型可选参数，例如：

- `scopeLabelOverride`；
- `emptyDescription`；
- 必要时增加明确的 `allowInterruptTurn`，默认保持普通 Channel 现状，Meeting 传 `false`。

不复制 transcript、archived observer、raw mode、anchored scroll 或 activity renderer。

### 7.4 Meeting-scoped selection state

选中状态至少按以下 scope 隔离：

```text
communityId : identityPubkey : meetingId
```

推荐保存为：

```text
{ scopeKey, selectedPubkey, boardWasOpen }
```

规则：

- scope 不一致时视为没有选中 Agent；
- 切换 Community、identity 或 Meeting 时立即清除旧选择；
- snapshot 更新后若 selected pubkey 不再属于当前冻结 roster，清除选择；
- `BotActivityComposerAction` 使用 Meeting scope key 重建内部 Popover/headline 状态；
- Activity rail 使用 `scopeKey + selectedPubkey` 作为实例 key，避免 A Meeting 的滚动、raw mode 或
  anchor 污染 B Meeting；
- 不新增 module-level 状态，因此无需扩展 `resetCommunityState()`。

### 7.5 右栏状态收口

调整 `MeetingScreen` 当前 Board render condition：

```text
wide rail visible = wideBoardOpen || selectedAgentPubkey != null
wide rail content = selectedAgentPubkey ? agent_activity : board
```

Board trigger 必须识别当前 rail mode；当 Agent Activity 在前台时，点击 Board 是“切回 Board”，
而不是依据旧 `wideBoardOpen` 值把 Board 关闭。

Human host 获得新的可编辑 Board window 时，既有 auto-open effect 除了打开 Board，还要清除/挂起
Agent Activity 前台选择，保证主持必需操作不会藏在观察面之后。

## 8. 预计修改位置

| 文件 | 修改内容 |
|---|---|
| `../../../../desktop/src/features/meeting/meetingAgentActivityModel.ts` | 冻结 roster、owner 可见性和 working Agent 的纯映射 |
| `../../../../desktop/src/features/meeting/ui/MeetingAgentActivityDock.tsx` | 左下角 Agents working adapter |
| `../../../../desktop/src/features/meeting/ui/MeetingAgentActivityRail.tsx` | Meeting-scoped Agent Activity 右栏/Sheet adapter |
| `../../../../desktop/src/features/meeting/ui/MeetingScreen.tsx` | 查询 Agent 元数据、维护 scope state、接入 Dock、收口 Board/Activity 右栏 |
| `../../../../desktop/src/features/channels/ui/AgentSessionThreadPanel.tsx` | 增加通用 scope/empty copy/只读展示参数，不改变 Channel 默认行为 |
| `../../../../desktop/src/features/meeting/meetingAgentActivityModel.test.mjs` | roster、owner、scope、去重和稳定顺序单测 |
| `../../../../desktop/tests/e2e/meeting-agent-activity.spec.ts` | Meeting 实时 working 与 Activity 交互、隔离、响应式和 Board 往返 |
| `../../../../desktop/playwright.config.ts` | 将新增 spec 纳入对应 Desktop smoke project |

如果实现后组件边界更适合一个 `useMeetingAgentActivity` hook，可以把 query/selection 组合放入 hook；
纯映射仍保留在无 React 依赖的 model 文件中。

## 9. 测试方案

### 9.1 纯模型单测

至少覆盖：

1. 只保留冻结 roster 中的 Agent participant；
2. Human、unknown 和非 roster Agent 被排除；
3. pubkey 大小写归一化；
4. managed 元数据覆盖 relay 元数据；
5. profile owner 匹配当前 identity 时可观察，不匹配时不可观察；
6. 同一 Agent 多 slot/turn 只生成一个 working item；
7. working 事件顺序变化不导致 roster 显示顺序变化；
8. Meeting A 的 working pubkey 不会进入 Meeting B；
9. 没有信号返回空列表，不返回 idle Agent。

### 9.2 Desktop E2E

使用现有 `__BUZZ_E2E_SEED_OBSERVER_EVENTS__` 或对应 observer helper 注入真实形状的 turn event，
不要用一个独立的 Meeting-only mock boolean 伪造工作状态。

需要证明：

1. 用户停留在当前 Meeting 页面时，`turn_started` 到达后入口实时出现，无需切页刷新；
2. `turn_completed` 后入口消失；
3. 同一 Meeting 中两个 Agent 同时工作时 Popover 显示两项；
4. 同一 Agent 多槽活动只显示一个逻辑 Agent；
5. 非 roster Agent 即使有相同 `meetingId` 的恶意/错误 signal 也不显示；
6. 其他 Channel 或 Meeting 的活动不显示；
7. 点击 Agent 后，右栏只包含 `meetingId` 对应的 observer transcript，不混入该 Agent 的其他
   Channel 历史；
8. 面板 scope 显示 `Meeting · title`，不显示 `All channels` 或误导性的 `#channel`；
9. Activity 中没有可用的 `Stop current turn`；
10. Activity → Back → Board 后 Board draft、宽度和编辑状态保留；
11. Activity 的 Close 恢复打开前的 Board 显隐状态；
12. Human host 进入 Board Maintenance 时 Board 自动回到前台；
13. Meeting A 打开 Agent Activity 后切换 Meeting B，不出现 A 的 Agent、transcript 或选择状态；
14. Community 切换后 observer/working 状态不泄漏；
15. 窄屏 Board Sheet 与 Agent Activity Sheet 互斥，焦点和 Escape 关闭正常；
16. closed/aborted Meeting 不被陈旧 working signal 重新标记为运行中。

### 9.3 回归检查

- 普通 Channel `Agents working`、Activity、Thread/Profile 返回栈和 Stop turn 行为保持原样；
- Meeting activity history 仍只展示 Relay 验证的协议转换；
- `MeetingHostObservation` 仍是只读 canonical 进程面，不被 Agent transcript 取代；
- Board resize、收起、自动打开、草稿和窄屏 Sheet 不回归；
- Floor Request/Offer/Grant/Speech、Human Host 和 Action Finalization 控件不受影响；
- working frame 到达时只重渲染 Dock/Activity 相关组件，不使整条 Speech timeline 重渲染。

## 10. 非目标

本次不处理：

- 改变 observer frame 的加密对象或授权模型；
- 让所有 Meeting participant 读取所有 Agent 的 Thinking/tool call；
- 新增 Meeting 专用日志存储、轮询、WebSocket subscription 或 event kind；
- 使用 Agent Activity 驱动 Meeting 状态机；
- 从 Activity 面板人工授予/回收 Floor；
- 在 Meeting 中中断 Agent turn；
- 把 ACP session/slot 当作新的 Meeting participant；
- 修改 Relay、DB migration、Project View、Document 或 Resource 数据。

## 11. 实施顺序

### 阶段一：纯模型与 scope

- 建立 roster/owner/working 映射；
- 增加单元测试；
- 固定 Community/identity/Meeting scope。

### 阶段二：左下角实时入口

- 接入 `useChannelWorkingAgentPubkeys(meetingId)`；
- 复用 `BotActivityComposerAction`；
- 证明实时出现/消失、多 Agent 和跨 Meeting 隔离。

### 阶段三：右栏与 Board 协同

- 建立 Board/Agent Activity rail mode；
- 复用 `AgentSessionThreadPanel`；
- 完成 Back、Close、Board trigger 和强制 Board Maintenance 优先级。

### 阶段四：响应式、回归与真实验收

- 完成窄屏互斥 Sheet；
- 跑 Meeting/Channel Activity 相关单测和 Playwright；
- 重新构建 Desktop；
- 使用真实 Agent-host 与 participant turn 验收 live Activity、Board 往返和数据隔离。

## 12. 完成条件

满足以下条件后可以认为本优化完成：

1. Meeting Agent 开始 turn 后，无需切换页面即可实时看到 `Agents working`；
2. 点击后能查看严格限定到当前 `meetingId` 的实时与已归档 Activity；
3. 只显示冻结 roster 中、当前 identity 有 observer 权限的逻辑 Agent；
4. 多 Agent、多 slot、跨 Channel、跨 Meeting 和跨 Community 均不串状态；
5. Board 与 Agent Activity 在宽屏共用右栏、窄屏互斥，且 Board 草稿/宽度不丢失；
6. Meeting activity、Agent Host Observation、Agent Activity 三类信息语义清晰且互不冒充；
7. Agent Activity 不影响任何 Meeting 权限、deadline、lease、Floor、Action 或终态；
8. 普通 Channel 的现有 Activity 功能无回归；
9. 自动化通过，并在真实 Desktop 中完成至少一场 Agent host Meeting 的实时观察验收。

## 13. 实施记录

已于 2026-08-07 完成 Desktop 实现：

- Meeting 冻结 roster、owner observer 权限与 working pubkey 的纯模型映射；
- Speech timeline 下方的实时 `Agents working` 入口；
- 按 `meetingId` 隔离的 ACP Activity transcript；
- 宽屏 Board/Activity 共用右栏，窄屏使用互斥 Sheet；
- Activity 的 Back、Close、Board 草稿保留与 Human Board Maintenance 优先恢复；
- Meeting Activity 禁用 `Stop current turn`，不改变任何会议协议状态；
- 单元测试、Desktop 构建、新增 E2E 以及既有 Meeting 回归套件均通过。

真实 Meeting 已完成实时 Activity、Board 往返和数据隔离验收；本项交付完成，无需协议或数据迁移。
