# Meeting Desktop-only 分阶段实现计划

> 状态：实施中（阶段一至五已完成，阶段六待开始）
>
> 产品规格：
> [Meeting Desktop 产品规格](./meeting-desktop-spec.md)
>
> 后端语义基线：
> [Meeting V2：主持人维护的共享会议看板](../v2/meeting-v2.md)与
> [Meeting V2：主持人直接完成行动收口的后端修正方案](../fix/meeting-v2-direct-action-finalization-backend-plan.md)
>
> 本文只规划 Buzz Desktop 交付，包括 React 前端、Desktop Tauri/Rust 适配层、Desktop
> mock bridge 和 Desktop 测试。Relay、数据库、CLI、ACP、Meeting 协议与 Project View
> 领域接口均视为已经交付的依赖，不在本计划中修改。
>
> 本文给出关键实现边界、阶段依赖、阶段交付物和验收门槛。完整 DTO、组件 props、Tauri
> command 参数、状态 JSON 字段、视觉尺寸和 PR 拆分，在进入对应阶段时再细化。

## 1. 计划目的

Meeting V2 已经具备完整后端生命周期，但当前 Desktop 仍把会议房间当成普通 Channel：

- `Channel` 模型没有暴露协议元数据中的 `room_kind=meeting`；
- 左侧导航没有独立的 `Meetings` 分组；
- `/channels/$channelId` 一律进入普通 `ChannelScreen`；
- 普通页面会加载自由消息 Composer、Thread、Reply、Reaction、Huddle 和频道管理；
- Desktop 没有 Meeting 当前状态投影、Board、Floor Dock、Host Console 或 Action
  Finalization 操作；
- Agent 已经能通过 ACP/CLI 使用会议，Human 还没有等价的 Desktop 操作面。

本计划把 Desktop 交付拆成六个严格有依赖关系、可以分别 review 和验收的阶段：

```text
阶段一：只读纵向链路与房间分流
    ↓
阶段二：Human 发起会议
    ↓
阶段三：Human 参会者 Floor 生命周期
    ↓
阶段四：Human 主持讨论生命周期
    ↓
阶段五：Action Finalization 与终态闭环
    ↓
阶段六：恢复、兼容、体验与真实验收收口
```

最终目标不是在 Desktop 中复制一份会议状态机，而是让 Human 使用现有 Relay 权威状态机，
并让 Agent 主持或参会的过程得到可信、不可冒充的可视化。

## 2. Desktop-only 范围

### 2.1 本计划包含

- `desktop/src-tauri` 中的 Meeting 查询、协议适配、签名事件准备和 capability 读取；
- `desktop/src` 中的 Meeting 类型、React Query、实时失效、导航、侧栏、页面和 Human 操作；
- `room_kind` 在 Desktop Rust DTO 与 TypeScript `Channel` 模型中的透传；
- 当前 Community 内的 Meeting active/history 发现；
- Meeting Create、Board、Speech、Intent、Floor、Close、Abort 和 Action Finalization UI；
- Human、Agent、主持人、普通参会者和终态的身份门控；
- canonical Speech 未读与 Meeting attention；
- 现有 Project View、普通 Channel 和其他外部引用的导航衔接；
- Desktop 单元测试、Tauri 测试、mock bridge、Playwright E2E、截图与真实 Relay 手动验收。

这里的 `desktop/src-tauri` 是 Desktop 客户端自身的 native bridge，属于 Desktop-only 范围，
不等于修改 Relay 后端。

### 2.2 本计划不包含

- Relay handler、数据库 migration、Meeting sweeper 或 outbox 修改；
- `buzz-cli`、`buzz-acp`、Agent slot 或 ACP Session 调度修改；
- Meeting event kind、wire schema、policy、fence、优先级或 deadline 语义修改；
- 新的 Project View API、Meeting 专用 Project View endpoint 或业务 materializer；
- Human 行动物化表单、Requirement/Work 专用向导、Plan 或 Step；
- Meeting 类型、模板、RSVP、Join、主持权转移、动态 roster、投票或 Huddle 联动；
- Board 版本、Diff、变更通知或多人协同编辑；
- Web 或 Mobile 实现。

实现中若发现现有后端契约不足，当前阶段应记录阻断事实并单独讨论后端修正，不得在 Desktop
中推断、伪造或旁路补出新的权威语义。

## 3. 现有 Desktop 实现映射

| 现有能力 | 当前主要位置 | Meeting 使用方式 |
|---|---|---|
| Channel 数据入口 | `desktop/src-tauri/src/commands/channels.rs`、`nostr_convert.rs`、`shared/api/tauriChannels.ts` | 增加 additive `roomKind`，将 Meeting 从普通房间分流 |
| Channel 路由 | `app/routes/channels.$channelId.tsx`、`ChannelRouteScreen.tsx` | 保留 `/channels/$channelId`，按 `roomKind` 懒加载 `MeetingScreen` |
| 普通频道页面 | `features/channels/ui/ChannelScreen.tsx`、`ChannelPane.tsx` | 不直接复用整页；只复用安全的布局、Markdown、身份与时间线呈现基础 |
| 左侧导航 | `features/sidebar/ui/AppSidebar.tsx` 及 section primitives | 增加独立 Meetings section；会议不进入 Channels、Forums、自定义 section 或频道浏览器 |
| Relay WS | `shared/api/relayClientSession.ts` | 使用显式 kind + `#h` 的 live signal；live payload 只触发权威重读 |
| Relay HTTP/Tauri | `desktop/src-tauri/src/relay.rs` | 复用认证 `/query`、签名与提交基础，组装经过验证的 Meeting read model |
| 事件 builder | 工作区 `buzz-sdk`，Desktop 已依赖 `buzz_sdk_pkg` | Tauri 直接复用已冻结的 Meeting builders，不在 TypeScript 手写 wire tags |
| Project View | `/view`、`features/project-view`、`tauriProjectView.ts` | Human 行动收口只打开现有页面；不嵌入或包装第二套业务编辑器 |
| 身份与 Agent | `features/profile`、`features/agents`、`useNewMessageRecipients` | 复用候选检索、Profile、managed/relay Agent 和 capability 数据，增加 Meeting roster 约束 |
| 辅助面板 | `shared/layout/AuxiliaryPanel*`、`RightAuxiliaryPane` | 承载默认打开、可调整宽度的 Board 与窄窗口 Sheet |
| Community 切换 | `useCommunityInit.ts`、`AppReady key={communityKey}` | Query key 必须带 Community；若新增 module singleton，必须接入 `resetCommunityState()` |
| 自动化 | `desktop/src/testing/e2eBridge.ts`、`desktop/tests/e2e` | 增加 Meeting 权威状态 seed、命令接受/拒绝和全生命周期 E2E |

当前 `AppShell.tsx` 与 `AppSidebar.tsx` 已经很大。Meeting 实现应新增独立
`features/meeting` 边界，只在现有编排文件中接入少量数据和事件处理；不得继续把完整 Meeting
状态机与界面堆入这两个文件。

## 4. 总体实现结论

### 4.1 Meeting 仍使用 Channel 定位，但不使用普通 Channel 产品模式

Desktop 的 `ChannelType` 继续只有 `stream | forum | dm`。新增独立的房间语义字段，例如：

```text
Channel
├── channelType: stream | forum | dm
└── roomKind: meeting | null
```

`roomKind` 来自 kind `39000` 元数据的 `room_kind` tag。它必须贯穿 Rust model、Tauri raw DTO
和 TypeScript domain model，并且只按精确值 `meeting` 识别会议。

会议详情继续复用 `/channels/$channelId`：

```text
ChannelRouteScreen
    ├── roomKind != meeting → 现有 ChannelScreen
    └── roomKind == meeting → 新 MeetingScreen
```

这样可以保留现有 deep link、窗口历史、Community 恢复和可见 Channel reconnect 优先级，
同时确保 Meeting 永远不会短暂挂载普通 Composer、Thread、Reaction、Huddle 或频道管理。

会议历史首版使用与现有 Channel Browser 相近的独立 Dialog/Sheet；从历史选择会议后仍进入同一
Channel 路由。若阶段实现中发现独立历史 route 对窗口恢复更合适，可以调整历史入口，但不改变
房间详情的分流原则。

### 4.2 Tauri 是 Meeting 协议适配边界

React 不直接解释原始 Meeting State JSON，也不手写 epoch、window、event kind 和 tag 组合。
建议新增 Desktop Meeting native 模块，职责分为：

1. 读取 NIP-11 Meeting extensions，返回 `unsupported | readable | creatable` 等语义能力；
2. 使用已有认证 `/query` 读取 Create、State、Board、End、canonical Speech 和必要活动；
3. 校验事件签名、Community/meeting scope、协议 discriminator、Relay-authored projection signer、
   revision 单调性和 Board envelope；
4. 将原始事件确定性组装成 Desktop read model；
5. 使用现有 `buzz-sdk` Meeting builders 准备由当前 Human 身份签名的命令事件；
6. 只向 React 返回产品状态和内部 opaque concurrency token，不把原始 State JSON 变成组件 API。

建议的读取边界至少包含：

- Meeting summary/list；
- 当前 Meeting snapshot；
- 当前 Board；
- keyset 分页的 canonical Speech；
- 必要的折叠活动记录；
- Relay Meeting capability。

写入边界按业务动作表达，例如 Create、Board updated/unchanged、Human request、Offer response、
Speech/Yield、Intent 操作、host selection、Recall、Close/Abort 和 Action 命令。具体是一条统一的
typed command 还是多个 Tauri command，在阶段设计时确定。

Tauri 应复用 `buzz-sdk` builder 生成并签名事件，再把同一份已签名事件交给现有 Relay publish
通道。这样 WebSocket 发送失败或响应丢失时可以重放同一 event，而不是重新签名一个语义相同但
event ID 不同的命令。Relay 接受后，React 只做 query invalidation 并等待权威 snapshot，不从
命令请求本身推导新状态。

### 4.3 React 只维护权威投影和有限本地草稿

Meeting server state 使用 React Query，query key 至少包含：

```text
communityId + meetingId + resource
```

实时事件只作为失效信号：

```text
explicit kinds + #h Meeting scope
    → debounce/coalesce
    → invalidate Meeting query
    → Tauri 重新验证并组装 snapshot
```

不得把未经验证的 live payload 直接 patch 成 Offer、Grant、Board、closed 或 action 状态。
这与现有 Project View 的“live signal → verified snapshot”模式一致，也能处理 out-of-order 和
reconnect replay。

允许保留的客户端本地状态只有：

- 创建会议草稿；
- 尚未提交的 Speech 草稿；
- 当前合法 Board window 绑定的 Board 编辑草稿；
- Dialog、Sheet、面板宽度、折叠和 timeline 位置；
- 已准备但尚未确定 Relay 结果的同一签名命令。

Board 草稿必须绑定读取它时的 opaque Board window。权威窗口改变后，草稿可供 Human 复制，
但不能继续提交。Meeting 不新增 Board 版本、更新通知或长期本地 Board cache。

### 4.4 Board、Floor 和 deadline 必须保持分离

Host Console 只映射 Relay 已有状态：

```text
Board Maintenance
    → Relay 接受 updated | unchanged，或权威 timeout/preemption
Floor Decision
    → select / self / handoff / idle / close / finalize actions
```

两个阶段使用各自权威 deadline。Desktop 只显示 server timestamp 的倒计时，不依据本地计时器
宣布超时、blocked 或转入下一阶段。deadline 到达后禁用旧按钮并重新查询权威状态。

### 4.5 正式 Speech 使用专用数据链路

Meeting timeline 只接受经过 Meeting Grant 消费并通过 Tauri 校验的 canonical kind `9`
Speech。它可以复用现有 Markdown、代码块、链接、头像、Profile 和虚拟列表基础，但不能复用：

- 普通 `MessageComposer` 发送路径；
- Reply、Thread、Reaction、Edit、Delete；
- Typing、普通 Channel system row；
- Huddle 主操作；
- 频道 Canvas 或成员管理。

Meeting 控制事件进入状态条、Host Console 或折叠活动记录，不能伪装成 Speech 行。

### 4.6 未读与 attention 是两条状态

普通未读只由新的 canonical Speech 产生。Meeting 不应继续进入普通 Channel 的 thread/mention
推导路径；可以复用 NIP-RS read marker 的持久机制，但需要专用 Meeting event filter 和计数器。

attention 从当前权威 snapshot 派生，至少覆盖：

- 当前 Human 的 Offer 或 Grant；
- Human 主持人的 Board Maintenance 或 Floor Decision；
- Human 主持的 runnable/blocked Action Finalization；
- blocked 或异常中止。

attention 随权威状态解除立即消失，不写成伪消息，也不增加普通 Speech 未读数。

### 4.7 Project View 继续是外部页面

Human 主持人在 Action Finalization 中使用现有 `goView()` 打开 `/view`，通过正常窗口历史返回
Meeting。Meeting 页面只保存它自己的定位和未提交文本状态，不保存 Project View 操作清单。

Desktop 不新增：

- Requirement/Work 表单；
- Action Plan/Step；
- Meeting 专用 mutation；
- operation count 或 receipt；
- 对 Board 与 Project View 一致性的检查。

没有 Project View 引用、Project View 不可用或没有发生任何外部写入，都不能阻止 Human 主持人
在 `runnable` 中确认行动产出已经登记或无需新增登记。

### 4.8 Feature gate 与协议 capability 分开

Desktop rollout gate 只控制新 UI 是否公开；Relay capability 决定当前 Community 实际能做什么。
建议在 `preview-features.json` 增加 Desktop Meeting preview gate，并在 native capability 中区分：

- Relay 未声明 `buzz-meeting-v2`，不支持读取 Meeting V2；
- Relay 声明读取能力，但未声明 `buzz-meeting-v2-create`，Create gate 关闭；
- Relay 声明 `buzz-meeting-v2-direct-actions`，可以继续读取 direct-action V2；
- Relay 声明 `buzz-meeting-v2-direct-actions-create`，Desktop 才可以创建 direct-action V2；
- 当前 roster 某个 Agent 缺少 `meeting-v2-action-finalization-v2`；
- Project View 不可用，但 Meeting action 仍可继续。

关闭 Desktop preview gate 不得改变 Relay 状态；关闭 Relay Create gate 也不得隐藏已有 Meeting。

## 5. 分阶段交付

### 5.1 阶段一：只读纵向链路与房间分流

#### 目标

> Desktop 能稳定识别、发现和只读打开 Meeting；会议不会再落入普通 Channel 产品模式。

#### 本阶段关键工作

1. **房间识别**
   - 在 Rust `ChannelInfo/ChannelDetailInfo`、Nostr conversion、raw TypeScript DTO 和 `Channel`
     domain model 中透传 `roomKind`；
   - 为缺少 tag 的现有 Channel 保持 `null` 默认值；
   - 将 member rooms 分成普通 Channel 与 Meeting room；
   - 从普通 Channels、Forums、自定义 sections、Starred、Channel Browser、Huddle 和频道管理中
     排除 Meeting。

2. **Desktop Meeting read model**
   - 补齐 Desktop 尚未声明的 Meeting Board、Board Command 和 Action Command kinds，常量值以
     `buzz-core`/`buzz-sdk` 为唯一基线；
   - 增加 native Meeting capability、list、snapshot、Board、Speech page 读取；
   - 严格按 `kinds` 和 `#h` 查询，避免 Relay p-gate 与跨房间混读；
   - group metadata 只用于发现和标题；active/action/closed/aborted 以及终态 outcome 必须来自
     Create、当前 State 与 End，不能只把 metadata `archived=true` 推断成正常关闭；
   - 使用 Create 时冻结的 moderator、participant type、roster 和 policy，不从当前 Profile 或
     managed-by 关系重推身份；
   - 对 `v=3 + moderated-board-actions-v2` 和 `v=3 + moderated-board-v1` 建立显式能力；
   - 对无法完整理解的旧协议 fail closed：仍按 Meeting 隔离，不提供普通 Channel Composer，
     显示明确兼容说明。

3. **导航与页面**
   - 新增独立 `Meetings` 侧栏 section、active/action items 和会议历史入口；
   - active item 显示名称、状态、当前 speaker/简短状态；
   - history 区分 `closed` 与 `aborted`，按结束时间分页或有界加载；
   - `ChannelRouteScreen` 在加载普通 Channel 数据前完成 Meeting 分流；
   - Meeting 页面复用现有 top chrome、Community、Profile 与辅助面板框架。

4. **只读 Meeting 房间**
   - 标题栏、状态条、canonical Speech timeline、默认打开的 Board 和 participant panel；
   - active、finalizing_actions、closed、aborted 的只读表现；
   - Agent 主持、Agent 参会与当前 Human 无操作权时的稳定观察文案；
   - source Channel、Project View deep link 和普通 URL 的安全入口；
   - 终态页面显示最终/最后 Board、Speech、结束信息和 action attestation 结果。

5. **实时与未读基础**
   - 对 Meeting State、End 和 Speech 使用 scoped live invalidation；
   - Board 正文不建立通知或版本订阅，只在 spec 规定节点和 State 失效后重读；
   - canonical Speech 使用 Meeting 专用未读链路；
   - 当前 Meeting 调用现有 `setVisibleChannel`，保留 reconnect 优先级。

#### 本阶段不做

- Create UI；
- 任何 Human Meeting 写命令；
- Host Console、Floor Dock 或 Action 操作；
- 高级历史搜索和筛选。

#### 自动化与验收

- Rust/TypeScript 测试覆盖 `room_kind` 缺失、普通 Channel、Meeting 和未知值；
- native 测试覆盖签名、Relay signer、protocol、revision 和跨 `h` scope 拒绝；
- sidebar 测试证明同一会议只出现在 Meetings，不出现在 Channels/Browser/Starred；
- Playwright 覆盖 active、action、closed、aborted 和非 roster/unsupported 页面；
- E2E 证明 Meeting 路由从未挂载普通 Composer、Thread、Reaction 或频道管理。

#### 完成标志

> Human 可以从 Meetings 分组和历史稳定打开所有自己可读的会议，看到可信的当前 Board、正式
> Speech、roster 和生命周期状态；任何 Meeting 都不能再形成普通频道旁路讨论。

### 5.2 阶段二：Human 发起会议

#### 目标

> 当前 Human 可以从 Desktop 创建一场由自己主持、固定 roster、使用 direct-action policy 的
> Meeting V2，并立即进入只读基础已经稳定的会议房间。

#### 本阶段关键工作

1. **统一创建入口**
   - Meetings section 的 `+`；
   - 普通 Channel 更多菜单的`发起会议`，只预填可删除的 source；
   - 创建 Dialog 作为独立 feature 组件接入 AppShell overlay，不扩张普通 CreateChannelDialog。

2. **创建草稿与初始 Board**
   - 名称、讨论目标、可排序议程、固定 roster、可选背景、source 和外部引用；
   - 客户端将表单确定性生成完整 Markdown Board；
   - 提交前提供 Board 预览或直接 Markdown 修订；
   - 校验空内容、NUL、UTF-8 byte 上限和总 roster `2..=12`；
   - 不引入 template ID、meeting type 或结构化 Board schema。

3. **参会者选择器**
   - 复用用户目录、Profile、managed Agent、relay Agent 和已有 recipient picker primitives；
   - 创建者固定加入且不可移除，其他 participant 为 1 至 11 人；
   - 显示 Human/Agent、owner/managed-by 和按需公钥信息；
   - 合并重复候选，并把 Agent action capability 表示为 compatible/incompatible/unknown；
   - unknown 在提交前重新读取；明确 incompatible 时阻止，不静默降级 policy。

4. **source 可读预检**
   - 无 source 始终合法；
   - open source 可直接保留；
   - private source 使用现有权威成员读取检查完整 roster；
   - 不可读时要求移除 source 或调整 roster，不自动加人或改权限。

5. **Create 提交**
   - 读取 NIP-11 direct-action create capability；
   - native 使用 `build_meeting_v2_actions_create`，当前身份同时成为 author/host/moderator；
   - Relay 接受前不创建 optimistic room；
   - 接受后刷新 Channel/Meeting summary 并导航；
   - 网络、capability 或 Relay 拒绝时保留完整创建草稿。

#### 本阶段不做

- 选择 Agent 为主持人；
- RSVP、Join 或动态成员维护；
- 自动向 source Channel 发布一条会议通知；
- 创建普通 `moderated-board-v1` 或旧 policy 的用户选项。

#### 自动化与验收

- Board 生成与 byte 上限单元测试；
- roster 去重、范围、Human/Agent 冻结类型与 capability tri-state 测试；
- source open/private/不可读矩阵；
- Relay Create 关闭、Agent capability 缺失和响应丢失场景；
- Playwright 覆盖从 Meetings 与 Channel 两个入口创建、草稿保留和成功导航。

#### 完成标志

> Human 可以创建无 Project View、无 source 或带可选上下文的 Meeting；自己必然是主持人，
> roster 在创建时冻结，所有 Agent 满足 direct-action capability，创建成功后不需要 Join。

### 5.3 阶段三：Human 参会者 Floor 生命周期

#### 目标

> 非主持 Human 可以在 Desktop 中完成 Request → Offer → Grant → Speech/Yield 的完整合法路径，
> 且每个 Grant 最多产生一条正式 Speech。

#### 本阶段关键工作

1. **Floor Dock 状态机视图**
   - 从权威 snapshot 派生当前 Human 的 idle/requested/offered/granted/read-only 状态；
   - 只有 Create 时冻结为非主持 Human 的身份显示 Human Floor Request；
   - 申请、撤回、Offer ACK/Decline 均等待 Relay 接受，不乐观宣布 Grant；
   - 当前不是操作者时显示 speaker、等待对象和会议状态，不展示无效按钮。

2. **Grant-bound Speech composer**
   - 使用专用 Meeting Speech composer，不复用普通 channel send mutation；
   - 支持一条 Markdown Speech、participant mentions 和可选 Directed Handoff；
   - Handoff 目标、类型和原因遵守现有协议约束；
   - 支持合法 Yield reason；
   - 提交成功或权威 Grant 消失后立即关闭发送能力；
   - stale/expired Grant 保留文本供复制，但不得自动向新 Grant 重放。

3. **Human priority 与 attention**
   - Human Request 在 Board preemption、Offer priority 和 queue 状态中使用独立产品呈现；
   - Offer、Grant 产生 attention，状态解除后清除；
   - control event 不增加普通 Speech 未读；
   - deadline 只触发重新查询，不由本地直接 Decline/Yield。

4. **共享 Human Offer/Grant 控件**
   - 同一控件也支持 Human 主持人被 Directed Handoff 或 fallback 指向时的 ACK/Decline、
     Speech 和 Yield；
   - 它不替代阶段四的主持 self Intent 流程，也不给主持人 Human Floor Request。

#### 本阶段不做

- 主持人 Board、Intent selection、self Intent 或 Close；
- Human 代 Agent ACK、Speech 或 Yield；
- 多 Speech、自由 Reply 或旁路聊天。

#### 自动化与验收

- request/withdraw、Offer ACK/Decline/timeout/preemption；
- Relay ACK 之后、Grant 之前 composer 仍不可用；
- 每 Grant 一条 Speech、重复点击和响应丢失不产生双 Speech；
- Speech + Directed Handoff 原子提交；
- Agent participant 与非 roster 身份没有 Human 控件；
- Human 主持人收到合法外部 Offer 时可使用共享控件。

#### 完成标志

> Human 参会者无需 CLI 即可完整参与由 Agent 或 Human 主持的 Meeting，同时不能绕过 Relay
> Floor 或替 Agent 行动。

### 5.4 阶段四：Human 主持讨论生命周期

#### 目标

> Human 主持人可以从初始控制机会开始，按 Board Maintenance → Floor Decision 顺序维护会议、
> 安排发言并直接正常关闭或异常中止。

#### 本阶段关键工作

1. **Board Maintenance**
   - 合法窗口自动打开 Board editor，并先重新读取权威当前 Board；
   - 只提供`保存并继续`和`看板无需修改`；
   - Board update 提交完整 Markdown，opaque token 绑定 control epoch 与 Board window；
   - timeout 明确区别于 unchanged；
   - Human Request 抢占或 control loss 后终止可提交资格，保留不可提交草稿；
   - Board 成功 terminal 前不展示下一席、Close 或 Finalize Actions。

2. **Floor Decision**
   - 只有权威 State 进入 decision 后才显示操作；
   - 独立显示完整 Floor deadline，不继承 Board 倒计时；
   - pending Intent、open Handoff、Human Request 和 self Intent 按协议优先级呈现；
   - Board timeout 后只显示继续讨论或 idle 所需合法操作，隐藏 Close/Finalize；
   - 保持 idle 后等待新工作，不制造空 Speech 或客户端轮询动作。

3. **Intent 与 Handoff 管理**
   - 查看 pending Intents，并在允许时异步 reject，收集稳定 reason code 与说明；
   - decision window 中选择 Intent 后先显示 Offer，不直接显示 Grant；
   - 选择或 dismiss open Handoff，显示来源、目标、类型、原因和 attempt；
   - 提供合法 Recall，不能打断已 Granted Speech 或越过 Human Request。

4. **主持人 self Speech**
   - `我要发言`先创建带一句摘要的 self Intent；
   - 支持 refresh/withdraw；
   - 在 decision window 选择 self Intent，并按协议处理必要 deferral reason；
   - self Offer 仍由 Human ACK/Decline；Relay 生成 Grant 后才使用阶段三 composer；
   - Offer 失败不能静默消费 self Intent。

5. **discussion 终止**
   - 最终 Board 显式 updated/unchanged 后提供`直接结束会议`；
   - Close 确认明确表达目标达到和有效结论语义；
   - Abort 使用稳定 reason category、说明、不可恢复和外部效果不回滚提示；
   - Relay 接受前不显示 closed/aborted。

6. **Agent 主持只读边界**
   - 当前 Human 不是冻结主持人时，Host Console 变成观察状态；
   - managed-by/owner 关系不开放签名按钮；
   - 不展示 Agent 隐藏推理、模型草稿或工具日志。

#### 本阶段不做

- Action Finalization 的 begin/block/retry/confirm；
- 主持权转移、副主持或接管；
- Board 协同编辑、版本或 Diff。

#### 自动化与验收

- Board updated/unchanged/timeout 与 Floor 独立 deadline；
- Board 编辑期间 Human Request 抢占和迟到提交拒绝；
- Intent select → Offer → Grant，不出现本地虚假 Grant；
- self Intent submit/refresh/withdraw/select/Offer/Grant/Speech；
- Handoff、Recall、idle 和 Human priority 组合；
- 直接 Close gate、Abort 和 response-loss 同事件重放；
- Agent 主持页面没有 Human host mutation。

#### 完成标志

> 不进入行动收口时，一名 Human 主持人与 Human/Agent 混合 roster 可以完全通过 Desktop 完成
> 多轮会议并正常 closed 或 aborted；Board 与 Floor 的顺序和时间预算没有被 UI 合并。

### 5.5 阶段五：Action Finalization 与终态闭环

#### 目标

> Human 主持人可以选择“记录行动产出后结束”，使用现有业务界面完成外部登记，并在 Meeting
> 生命周期内确认、阻塞、恢复、返回 Board 或中止；Agent 主持路径保持只读观察。

#### 本阶段关键工作

1. **进入行动收口**
   - 在满足 gate 的最终 Floor Decision 中并列显示`直接结束会议`与`记录行动产出后结束`；
   - begin 使用当前 State、Board、control 与 window fences；
   - Relay 接受后才切换 `finalizing_actions/runnable`；
   - Board 冻结只读，Speech/Floor/Intent 全部禁用，timeline 继续可浏览。

2. **Human 主持行动卡**
   - 展示冻结 Board、runnable/blocked、权威 deadline 和外部上下文入口；
   - 使用现有 `/view`、source Channel 和安全外部链接；
   - 返回 Meeting 后重新读取当前 action window；
   - 不显示表单、Plan、Step、业务预览、操作数量或验证进度。

3. **完成、阻塞与恢复**
   - runnable 提供`确认行动产出已完成并结束会议`；
   - 允许零外部写入，确认表达主持判断而非 Meeting 自动验证；
   - block 收集后端支持的稳定 reason code 和可选说明；
   - blocked 只能 retry 取得新 window 后再确认；
   - runnable/blocked 均可`返回会议看板`，二次确认已有外部效果继续保留；
   - return-to-board 成功后结束旧 action run，重新进入阶段四 Board Maintenance；
   - Action Abort 明确保留可能发生的外部效果。

4. **原子关闭与终态**
   - 完成确认携带当前 run/window/Board fence 和 `actions-recorded` attestation；
   - action complete 与 Meeting closed 只在 Relay 接受的同一结果后展示；
   - 不创建 `planning | applying | ready_to_close` 客户端状态；
   - stale/expired/blocked fence 触发权威重读，不能关闭；
   - closed 页面区分直接关闭与经行动产出确认关闭；
   - aborted 且曾进入 action 时显示外部效果可能保留。

5. **Agent 主持呈现**
   - 显示原主持 Agent 正在记录行动产出或已 blocked；
   - Human roster participant 不获得 confirm/retry/return-to-board；
   - Desktop 不启动新 Agent Turn，不选择槽，也不改变 ACP Session。

#### 自动化与验收

- direct close 与 begin actions 是不同命令和 gate；
- Human 从 Meeting 打开 Project View、执行任意现有操作或不操作、返回并确认；
- 界面不存在 Requirement/Work 表单、Plan、Step 或 materializer；
- runnable confirm 原子 closed，无 complete/ready 中间态；
- block → retry → confirm；
- runnable/blocked → return-to-board，外部效果保留提示与新 Board window；
- stale run/window/Board、deadline、断线和响应丢失；
- Agent host action 全程只读。

#### 完成标志

> Human 和 Agent 主持的 direct-action Meeting 都能从讨论进入行动收口并最终 closed/aborted；
> Desktop 没有重新引入已移除的 Plan/Step，也不限制主持人使用现有 Project View 或其他系统。

### 5.6 阶段六：恢复、兼容、体验与真实验收收口

#### 目标

> 把前五阶段收敛为可恢复、可访问、可灰度并经过真实 Relay/Agent 验收的 Desktop 发布候选。

#### 本阶段关键工作

1. **断线与不确定结果**
   - 断线时保留最后已验证内容并明确 stale，禁用依赖当前 window 的写操作；
   - 重连后先 snapshot，再恢复控件；
   - 同一 prepared/signed command 在结果不明确时只重放同一 event；
   - Relay 明确 stale/conflict 时不把旧业务意图自动改绑到新状态；
   - deadline 到达、系统休眠唤醒和时钟偏差后重新查询，不靠本地倒计时推进。

2. **Community 与窗口恢复**
   - 所有 query、草稿、panel preference 和 pending command 正确按 Community 隔离；
   - Community 切换清理旧订阅和不可提交状态；
   - 若实现引入 module-level cache/store，将 reset 函数接入 `resetCommunityState()`；
   - 应用重启或路由恢复不恢复过期 Offer、Grant、Board window 或 action window；
   - 从 Project View/source 返回后保留 timeline 和 Board 面板体验，但重新确认权威控制状态。

3. **兼容与 capability**
   - 已有 action V2、普通 board V2、旧 Meeting 和未知 policy 都有明确、安全表现；
   - Create gate 关闭不影响已有 Meeting 读取和结束；
   - Relay capability、Agent runtime capability、Project View capability 使用不同错误文案；
   - 非 roster deep link 不泄漏 title、Board、roster 或 Speech。

4. **响应式与可访问性**
   - 宽屏 timeline + resizable Board；中窄窗口 Board/participants 使用 Sheet；
   - 面板切换不丢 Floor/Board 草稿或 timeline 位置；
   - focus、keyboard、live region、destructive confirmation 和 screen reader label 收口；
   - 所有可读文字使用既有 rem token，不引入 px 或 arbitrary text size；
   - 状态不只依赖颜色，Human/Agent/host/speaker 有文本语义。

5. **性能与代码边界**
   - active/history 数量增大时采用有界 query、分页与虚拟列表；
   - live signals 合并失效，避免每个 control event 重建全 App；
   - 保持 props reference 稳定，避免 Meeting 状态导致整条 sidebar 和 timeline 无意义重渲染；
   - 遵守 Desktop 文件大小 guard，拆分页面、控制台、Board、Floor 和 action 组件；
   - 不在 `AppShell.tsx`、`AppSidebar.tsx` 或 `ChannelScreen.tsx` 内复制协议 reducer。

6. **测试与发布**
   - 扩展 `e2eBridge.ts` 的 Meeting seed、live signal、command accept/reject 和 Community 隔离；
   - 新增 Meeting Playwright spec，并注册到 smoke project；
   - 用 scoped locator 截取 create、participant、host、action、closed、aborted 和窄窗口状态；
   - 截图前使用 `waitForAnimations`，并检查 PNG hash 不重复；
   - 完成真实 Tauri + Relay 的 Human/Human、Human/Agent、Agent host 场景；
   - 运行 Desktop 完整 gate，最后运行仓库级 `just ci`。

#### 最低真实验收矩阵

| 场景 | 必须证明 |
|---|---|
| Human host + Human participant | Create、Request、Offer、Grant、Speech、Board、direct close |
| Human host + Agent participant | Agent Intent/Speech 可观察，Human host 可安排，不能代 Agent 操作 |
| Agent host + Human participant | Human Floor 完整可用，Host Console 只读，Agent 完成 Board/Floor |
| Human host action | 打开现有 Project View、返回、confirm；以及零写入 confirm |
| Agent host action | 原 ACP Session 完成或 blocked，Desktop 无接管按钮 |
| Recovery | Relay reconnect、Desktop restart、Community A→B→A、stale command |
| Terminal | direct closed、actions-recorded closed、discussion abort、action abort |

#### 完成标志

> Meeting Desktop spec 的关键验收场景均有自动化或真实运行证据；Desktop 全量质量门禁通过；
> 不存在跨 Community 泄漏、虚假控制权、重复 Speech、重复完成声明或普通 Channel 旁路。

## 6. 阶段依赖与交付纪律

六个阶段按 `1 → 2 → 3 → 4 → 5 → 6` 推进，不建议并行跳过基础阶段：

- 阶段二依赖阶段一的 room 分流、capability 与 verified read model；
- 阶段三依赖阶段一的 snapshot 和 canonical Speech，但不依赖阶段四主持 UI；
- 阶段四复用阶段三的 Human Offer/Grant/Speech 控件；
- 阶段五依赖阶段四最终 Board 与 Floor Decision gate；
- 阶段六不增加会议业务语义，只做恢复、兼容、体验和发布证据。

每个阶段开始前只冻结该阶段必要的详细设计。每个阶段完成时必须：

1. review 产品语义与 wire 使用，确认没有在 Desktop 自创状态；
2. review 身份门控，确认 Human 不能冒充 Agent；
3. review 所有 pending、timeout、stale 和 response-loss 路径；
4. 运行本阶段针对性测试和相关现有回归；
5. 更新本文阶段状态和交付证据；
6. 经确认后再提交并进入下一阶段。

不以无限重复的同类验收阻止阶段收口。自动化门槛、一次针对性 review 和约定的真实场景通过后，
剩余纯人工体验观察记录为后续问题，不反复重开已完成阶段，除非发现语义或数据安全缺陷。

## 7. 跨阶段关键测试矩阵

| 约束 | 主要阶段 | 自动化重点 |
|---|---:|---|
| `room_kind=meeting` 唯一分流 | 1 | 普通 Channel/Meeting/未知值、sidebar 去重、route 无闪现 |
| 非 roster 隐私 | 1、6 | list、deep link、Board、Speech、history 均 fail closed |
| frozen participant type | 1、3、4 | Profile/managed-by 变化不改变 Human/Agent 操作权 |
| canonical Speech only | 1、3 | 非 Grant kind 9、control event、reply/reaction 不进入 timeline |
| Board 先于 Floor | 4 | updated/unchanged/timeout/preemption 与独立 deadline |
| Human Request priority | 3、4 | Board/Offer 抢占、host 不可拒绝或重排 |
| 每 Grant 一条 Speech | 3、6 | double click、reconnect、response loss、expired Grant |
| Human host self Speech | 4 | self Intent lifecycle、Offer、Grant、失败后仍 pending |
| Agent 不可冒充 | 1、3、4、5 | managed owner 无 host/ACK/Speech/action 按钮 |
| direct close/action close 分离 | 4、5 | 两套 gate、attestation、无伪中间状态 |
| 无 Plan/Step | 5 | UI、DTO、Tauri command 和测试 seed 均无 materializer |
| Community 隔离 | 1、6 | Query/cache/draft/subscription/pending command A→B→A |
| 权威恢复 | 全阶段 | stale/timeout/reconnect 只 refetch，不本地推进 |

## 8. 阶段状态

| 阶段 | 状态 | 主要交付证据 |
|---|---|---|
| 1. 只读纵向链路与房间分流 | 已完成 | `736ab91c0`；roomKind、verified read model、Meetings 导航、只读房间与 E2E |
| 2. Human 发起会议 | 已完成 | `636c693d6`；Create dialog、roster/capability/source gate、创建 E2E |
| 3. Human 参会者 Floor 生命周期 | 已完成 | `24815ad00`；native verified Floor command、Request/Offer/Grant/Speech/Yield/Handoff、精确重试与 9 条 Floor E2E |
| 4. Human 主持讨论生命周期 | 已完成 | `56d4e8052`；verified Native host boundary、Board/Floor、Intent/self/handoff/recall、close/abort；Host E2E |
| 5. Action Finalization 与终态闭环 | 已完成 | Native direct-action boundary、opaque action fence 与精确重试；Human 行动卡、现有 Project View 导航、confirm/block/retry/return/abort；8 条 Action E2E，Meeting 合并回归 29/29 |
| 6. 恢复、兼容、体验与真实验收收口 | 待开始 | Desktop gates、真实 Relay/Agent 证据、发布候选 |

## 9. 整体完成定义

Meeting Desktop 可以认为交付完成，需要同时满足：

1. `room_kind=meeting` 稳定驱动独立导航和页面，普通 Channel 行为无法绕过 Floor；
2. Human 可以创建 action-capable V2，自己成为主持人并冻结 Human/Agent roster；
3. Human participant 可以完成完整 Floor 生命周期；
4. Human host 可以完成 Board、Intent、Floor、self Speech、Close、Abort 和 action 操作；
5. Agent host/participant 状态可观察，但 Human 不能替 Agent 签名或选择 slot/session；
6. Board 与 canonical Speech timeline 同时可读，Board 无版本、Diff 或更新通知产品；
7. Board Maintenance 与 Floor Decision 的界面、提交和 deadline 完全分离；
8. Human Request、Directed Handoff、Recall 与 self Intent 保持后端既有优先级；
9. Project View 与其他业务系统保持可选且独立，不存在 Meeting Plan/Step/materializer；
10. Action Finalization 始终留在 Meeting 生命周期内，直到原子 `closed` 或明确 `aborted`；
11. 断线、过期、冲突、重启和 Community 切换不会产生虚假状态或重复命令效果；
12. Desktop 自动化、真实 Relay/Agent 验收和仓库质量门禁通过。
