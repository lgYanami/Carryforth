# Meeting Desktop 缺陷修复分阶段实现计划

> 状态：实施完成（阶段一至九已交付；阶段九待确认提交）
>
> 日期：2026-08-05
>
> 缺陷基线：
> [Meeting V2 缺陷清单](./meeting-v2-defect-list.md) 第 6 节 `MFX-007` 至 `MFX-015`
>
> 产品基线：
> [Meeting Desktop 产品规格](../desktop/meeting-desktop-spec.md)
>
> 范围：仅包含 Desktop React、Desktop Tauri/Rust 可信读取适配、Desktop mock bridge 与
> Desktop 自动化；不修改 Relay、DB、Meeting 协议、CLI、ACP、Web 或 Mobile。

## 1. 结论

第 6 节最初列出 9 个 Meeting Desktop 缺陷。本计划拆成 9 个交付阶段，严格遵守：

> 一个阶段只修复、验收并关闭一个缺陷。

阶段按实现依赖排序，而不是机械按 MFX 编号排序：

```text
阶段一   MFX-009  清除主状态条的原始 revision
阶段二   MFX-008  补齐宽屏 Board 收起与恢复
阶段三   MFX-014  建立 Meeting 活动记录
阶段四   MFX-012  补齐 Speech 身份与 Directed Handoff
阶段五   MFX-011  补齐 participant 会议状态面板
阶段六   MFX-010  补齐 Agent 主持的只读观察面
阶段七   MFX-013  补齐侧栏 attention 与排序
阶段八   MFX-015  补齐终态摘要
阶段九   MFX-007  统一收口 Meeting 标题栏
```

这样排序的原因是：

- `MFX-008` 先提供真正可用的宽屏 Board trigger，最终标题栏只负责统一组织它；
- `MFX-014` 先交付独立可用的活动记录，Speech 和最终标题栏再引用该入口；
- `MFX-011` 先建立 participant 的稳定只读状态表达，`MFX-010` 再组合 Agent 主持观察面；
- `MFX-015` 先完成终态产品语义，最终标题栏再组织终态只读信息；
- `MFX-007` 是多个已有能力的界面整合，因此最后交付，避免前面阶段反复重写标题栏。

阶段实现可以为后续阶段增加共享 primitive，但本阶段只能把对应的一个 MFX 标记为已解决。
例如阶段二增加 Board trigger 时，不提前关闭 `MFX-007`；阶段三增加活动入口时，也不提前关闭
`MFX-007`。

## 2. 修复目标

本轮修复不重新设计 Meeting，也不改变已经交付的生命周期。目标是让真正的 `MeetingScreen`
完整表达现有 Meeting V2 产品语义：

- Meeting 是有主持人、冻结 roster、共享 Board 和受控 Floor 的独立协作空间；
- 正式 Speech 与控制过程分离；
- Human 与 Agent 的冻结身份、当前发言状态和主持过程可以被可靠观察；
- Human 只能操作自己的身份，不能通过 Desktop 冒充 Agent；
- Board、Floor、Action Finalization 和终态均来自权威 Meeting 投影；
- 普通产品界面不要求用户理解 event、epoch、revision、lease 或内部 State；
- canonical Speech unread 与需要当前身份行动的 attention 保持分离；
- Meeting 页面不重新带回普通 Channel 的管理、自由发言、Thread、Reaction 或 Huddle 入口。

## 3. 实现范围与边界

### 3.1 本计划包含

- `desktop/src/features/meeting` 下的 Meeting 页面、标题栏、状态条、Board、Speech、participant、
  观察面、活动记录、终态和侧栏实现；
- `desktop/src/shared/api/tauriMeetings.ts` 的 Desktop Meeting 产品 DTO；
- `desktop/src-tauri/src/commands/meetings.rs` 及其子模块中的可信读取、验证和产品级投影；
- 既有 Meeting query、live invalidation、profile、导航和本地 UI 状态的必要衔接；
- `desktop/src/testing/e2eBridge.ts` 的 Meeting mock 状态；
- Desktop native 测试、React/TypeScript 测试、Playwright E2E 和必要的 scoped 截图。

Desktop Tauri/Rust 是客户端可信适配边界，属于 Desktop-only 范围，不等于修改 Meeting 后端。

### 3.2 本计划不包含

- 新的 Relay event kind、wire tag、状态机阶段、数据库字段或 HTTP endpoint；
- 修改 Meeting 的 Board-first、Floor、Action Finalization、Close 或 Abort 语义；
- 修改 Human/Agent 冻结 participant type 或主持人身份；
- Human 代替 Agent 提交 Intent、ACK、Speech、Yield、主持或行动完成声明；
- Agent hidden reasoning、模型草稿、工具调用日志或 ACP 内部 Session 日志展示；
- 普通 Channel 的成员增删、归档、可见性、类型编辑、Composer、Thread、Reaction 或 Huddle；
- Board 版本、Diff、更新通知或多人共同编辑；
- Project View 专用 Meeting 表单、Action Plan、Step、receipt 或 operation list；
- Web、Mobile 或普通 Channel 页面适配；
- 与本缺陷清单第 5、7、8 节对应的后端、文档和真实边界验收问题。

如果某个产品字段无法从现有已签名事件和 Relay 权威投影中可靠得到，本轮不得从 Markdown、
Profile、时间差或本地事件顺序猜测。应在对应阶段记录具体阻断，再单独讨论是否需要后端契约修正。

## 4. 当前实现基线

| 区域 | 当前主要实现 | 已确认差距 |
|---|---|---|
| Meeting 页面 | `MeetingScreen.tsx` | 标题栏不完整；宽屏 Board 常驻；状态条显示 revision |
| Board | `MeetingBoardPanel.tsx`、`useMeetingBoardDraft.ts`、`useResizableMeetingBoardWidth.ts` | 宽屏不能收起；只有窄屏 Sheet trigger |
| Speech | `MeetingSpeechTimeline.tsx`、native `MeetingSpeech` | 缺少冻结身份、主持标识和结构化 Handoff |
| Participants | `MeetingParticipantsPanel.tsx` | 平铺 roster；只显示 host 与 speaking |
| 主持过程 | `MeetingHostConsole.tsx`、Intent/Handoff list、snapshot `host` | Human host 可操作，但 Agent host 的非主持 Human 缺少完整只读观察面 |
| Meetings 侧栏 | `MeetingsSidebarSection.tsx`、native `MeetingListItem` | attention 只覆盖 Human Offer/Grant，排序只看 `updatedAt` |
| 活动记录 | 尚无统一产品组件和 query | 重要控制转换没有独立、可读的产品记录入口 |
| 终态 | `MeetingTerminalSummary.tsx`、native `MeetingEndState` | reason、终止来源和 closed/aborted 语义不完整 |
| 自动化 | `meeting-*.spec.ts`、native Meeting tests | 生命周期主路径已有覆盖，但上述产品展示缺口缺少专项断言 |

现有 `MeetingSnapshot.host` 已包含经过 native 验证的 pending Intent、open Handoff、Board control
和 Floor decision 数据。`MFX-010` 应优先建立安全的只读展示，不再发明一份主持状态机。

## 5. 共同实现原则

### 5.1 Tauri 继续是可信读取边界

React 不读取或解释 raw Meeting State JSON。需要新增的数据应由 native 层：

1. 使用显式 kind 与 `h` scope 查询现有事件；
2. 校验签名、Meeting、roster、schema、policy 和权威 signer；
3. 折叠为有界、产品级 DTO；
4. 由 React 只做展示和有限的视图组合。

Speech Handoff、活动记录、终态来源和侧栏 attention 不得通过解析 Speech/Board Markdown 得到。

### 5.2 不复制 Meeting 状态机

Desktop 可以将权威字段映射为产品文案、Badge、分组和排序，但不能根据本地时间或先后收到的
live event 宣布新的 Offer、Grant、Board outcome、Action condition 或终态。

Live event 仍只触发 query invalidation，界面以重新读取的可信 snapshot 为准。

### 5.3 观察能力不扩大操作能力

只读 participant、Intent、Handoff、Board/Floor 和活动展示不得复用成可冒充其他身份的按钮。
所有 Human 写操作继续同时满足：

- 当前签名身份；
- Create 时冻结的 participant type；
- moderator identity；
- 当前权威 window/fence；
- native command 的既有授权校验。

### 5.4 产品状态与诊断信息分层

普通界面只显示用户需要理解的状态。原始 event ID、epoch、revision、lease、control token 和
raw State：

- 可以继续作为 native 内部校验或 opaque command fence；
- 不进入主状态条、participant 状态、普通活动记录或终态摘要；
- 如未来确需展示，只能进入明确的诊断层，不能成为完成会议操作的前提。

### 5.5 unread 与 attention 分离

- unread 只由新的 canonical Speech 产生；
- Board、Intent、Offer/Grant 底层事件和活动记录不增加 Speech unread；
- attention 表示当前身份需要操作或查看稳定异常；
- active attention 随权威状态解除；终态 attention 使用独立确认状态，不篡改 Speech read marker。

### 5.6 本地视图状态不得破坏草稿

Board 展开/收起、Sheet、活动面板和 participant 面板属于本地视图状态。切换这些视图时：

- timeline 不重新建立为另一条消息流；
- Floor/Speech 草稿不被清空；
- Board 草稿继续绑定原权威 Board window；
- 权威 window 改变时仍按现有 stale draft 规则处理，不能因面板操作绕过 fence。

## 6. 阶段顺序与依赖

| 阶段 | 唯一关闭缺陷 | 主要交付 | 后续依赖 |
|---:|---|---|---|
| 一 | MFX-009 | 主状态条只保留产品状态 | 后续界面统一使用产品语言 |
| 二 | MFX-008 | 宽屏 Board 可收起、恢复和安全自动打开 | MFX-007 复用统一 Board trigger |
| 三 | MFX-014 | 可信、分页、产品级 Meeting 活动记录 | MFX-012、MFX-007 复用活动入口 |
| 四 | MFX-012 | Speech 身份、主持标识和 Directed Handoff | participant/Profile 表达统一 |
| 五 | MFX-011 | 分组 roster 与稳定会议状态 | MFX-010 复用 participant 状态表达 |
| 六 | MFX-010 | Agent 主持的完整只读主持观察面 | Agent-host Meeting 产品闭环 |
| 七 | MFX-013 | Native attention 与产品排序 | 侧栏发现和恢复闭环 |
| 八 | MFX-015 | 完整 closed/aborted 终态摘要 | MFX-007 复用终态信息 |
| 九 | MFX-007 | 完整 Meeting 标题栏与菜单整合 | 第 6 节整体收口 |

## 7. 阶段一：MFX-009 主状态条产品化

> 状态：已完成（2026-08-05）

### 7.1 目标

从 Meeting 主状态条移除 `Speech r... · State r...`，让用户只看到当前会议产品状态。

### 7.2 关键实现点

- 删除 `MeetingScreen` 状态条中的原始 revision 文本；
- 保留 `stateRevision`、`speechRevision` 等字段供可信排序、分页、失效和 command fence 使用；
- 复核 `meetingStatusText` 对 initializing、Board Maintenance、Floor Decision、Offer、Grant、
  Action runnable/blocked、closed 和 aborted 的产品文案；
- 不在本阶段临时增加一个“高级信息”区域来搬运 revision；诊断入口不属于本缺陷的必要交付。

### 7.3 主要修改位置

- `desktop/src/features/meeting/ui/MeetingScreen.tsx`
- 对应 Meeting read-only/recovery E2E fixture 与断言

### 7.4 验收标准

- 主状态条不出现 `Speech r`、`State r`、event ID、epoch 或 lease；
- 当前 speaker、Offer、Board、Action 和终态仍有明确的产品状态；
- revision 字段未从内部 DTO 中误删，既有并发与分页测试保持通过；
- 本阶段只把 `MFX-009` 标记为已解决。

## 8. 阶段二：MFX-008 宽屏 Board 收起与恢复

> 状态：已完成（2026-08-05）

### 8.1 目标

宽屏 Board 默认打开，但用户可以收起并随时重新打开；视图切换不丢失会议上下文或草稿。

### 8.2 关键实现点

- 为 Meeting 页面增加明确的 Board 可见状态，宽屏初次进入默认 `open`；
- 让同一个 Board trigger 在宽屏控制右侧 `aside`，在中窄窗口控制既有 Sheet；
- 收起只改变布局，不卸载 timeline、Floor Dock 或 Speech draft 所属状态；
- Board draft 继续保存在 `MeetingScreen` 上层 hook，并绑定既有 `controlToken`；
- 保留已调整的 Board 宽度，收起再打开不重置用户宽度；
- 新的 Board Maintenance window 需要 Human 主持操作时自动打开 Board；
- 自动打开按新的权威 control token 触发一次，不反复抢夺 focus，不覆盖或重建现有草稿；
- Community/Meeting 切换后的默认与清理行为遵循现有 community-scoped 状态边界。

### 8.3 主要修改位置

- `MeetingScreen.tsx`
- 必要时新增独立的 Board visibility hook；不把该状态塞入普通 Channel shell
- `useMeetingBoardDraft.ts`、`useResizableMeetingBoardWidth.ts` 的兼容断言
- `meeting-recovery.spec.ts` 或新的聚焦 E2E

### 8.4 验收标准

- `>= 1280px` 时 Board 默认打开，可收起、可从持续可见入口恢复；
- 中窄窗口仍使用 Sheet，行为没有退化；
- 收起/恢复后 timeline 位置、Speech/Floor 草稿、Board 草稿和调整后的宽度不丢失；
- 新 Board Maintenance window 可安全自动打开；stale window 仍不能提交；
- 本阶段不重做完整标题栏，只把 `MFX-008` 标记为已解决。

## 9. 阶段三：MFX-014 Meeting 活动记录

> 状态：已完成（2026-08-05）

### 9.1 目标

提供独立、可信、产品级的 Meeting 活动记录，使控制过程可查看但不污染 canonical Speech。

### 9.2 可信投影

新增有界的 Desktop Meeting activity read projection。它只从现有已验证 Meeting 事件和当前权威
状态生成 typed activity item，首版至少覆盖：

- Board `updated | unchanged | timed_out | preempted`；
- Floor Offer、ACK/Decline、Grant、Yield、Recall 和必要的 expiry；
- Directed Handoff 的建立、尝试和稳定结束状态；
- 进入 Action Finalization；
- action `blocked | retry | return-to-board`；
- `closed | aborted`。

DTO 只表达产品类别、发生时间、可公开 actor/target 和产品摘要。raw event ID、revision、epoch、
lease、State JSON 与 control token 不进入普通 activity item。

读取必须有上限和稳定 cursor，避免为了打开活动面板无界加载整场会议。具体 page size 和 item
折叠规则在阶段设计时确定。

### 9.3 Desktop 呈现

- 新增独立 Activity Sheet/Dialog，使用 timeline 式或分组列表展示；
- 本阶段提供一个可发现的临时稳定入口，确保活动记录本身可以完整使用；
- `MFX-007` 阶段再把入口统一组织进标题栏更多菜单，阶段三不提前重写完整标题栏；
- 普通产品活动与可选诊断信息严格分层；首版可以完全不展示诊断信息；
- 活动 item 不渲染成普通 Speech，不出现 Reply、Reaction 或普通 message action；
- 打开、分页或新增活动均不产生 canonical Speech unread。

### 9.4 主要修改位置

- native `commands/meetings` model/projection/query 与 command 注册
- `tauriMeetings.ts`、Meeting hooks/query keys
- 新的 `MeetingActivityPanel` 及入口
- `e2eBridge.ts`、native fixtures、Meeting E2E

### 9.5 验收标准

- roster participant 可以读取产品级活动；非 roster 仍得到 forbidden；
- 重要 Board/Floor/Action/End 转换按权威顺序展示；
- 活动记录不泄漏 raw ID、epoch、revision、lease 或 State JSON；
- 活动不会增加 Speech unread，也不会混入 canonical Speech page；
- 分页有界，重复读取不会生成重复 item；
- 本阶段只把 `MFX-014` 标记为已解决。

## 10. 阶段四：MFX-012 Speech 身份与 Handoff 语义

> 状态：已完成（2026-08-05）

### 10.1 目标

让每条 canonical Speech 明确显示 speaker 的冻结 Human/Agent 身份、主持人标识和结构化
Directed Handoff。

### 10.2 可信读取

- native `MeetingSpeech` projection 解析并校验 `handoff-to`、`handoff-type`、`handoff-reason`；
- 三个 Handoff 字段必须 all-or-none；target 必须属于冻结 roster，type 必须是协议允许值；
- DTO 返回结构化 `handoff`，React 不从正文、mention 或 Board 猜测；
- speaker 的 Human/Agent 类型和 moderator 标识只与同一份可信 snapshot 的冻结 roster 合并，
  不根据 Profile、managed-by 或当前 Channel role 重推断；
- 继续只读取成功消费 Grant 的 canonical kind `9` Speech，保持现有 cursor 与 revision 顺序校验。

### 10.3 Desktop 呈现

- Speech header 增加 `Human | Agent` 标识；
- speaker 同时为 moderator 时显示 Host 标识；
- 有 Handoff 时，在 Speech 正文后显示目标、类型和 Human 可读 reason；
- 其他 Offer、Grant、Board command 等控制过程继续进入阶段三交付的活动记录；
- 不增加 Thread、Reply、Reaction、自由 Composer 或编辑旁路。

### 10.4 主要修改位置

- native `MeetingSpeech` model 与 `parse_speech`
- `tauriMeetings.ts`
- `MeetingSpeechTimeline.tsx` 及必要的 presentation selector
- native Speech fixtures、read-only/floor E2E

### 10.5 验收标准

- Human、Agent 和 Host badge 只按冻结 Meeting 身份显示；
- 合法 Directed Handoff 显示准确目标、类型和原因；
- 缺字段、非法 type、非 roster target 或冲突数据 fail closed；
- 普通 mention 不被误判成 Handoff；控制事件不被误判成 Speech；
- 本阶段只把 `MFX-012` 标记为已解决。

## 11. 阶段五：MFX-011 Participant 会议状态面板

> 状态：已完成（2026-08-05）

### 11.1 目标

把冻结 roster 从平铺成员名单升级为只读的 Meeting participant 状态面板。

### 11.2 关键实现点

- roster 分为 Host、Human participants、Agent participants；每个 pubkey 只出现一次；
- Host 自身仍保留 Create 时冻结的 Human/Agent 类型；
- 从可信 snapshot 建立 participant presentation state，至少表达：
  - 当前 Grant/正在发言；
  - 当前 Offer/等待 ACK；
  - Human Floor Request 及队列状态；
  - pending Intent；
  - idle/无当前会议动作；
- 多个状态同时存在时使用固定产品优先级，避免同一 participant 显示互相冲突的主状态；
- Agent runtime 只在现有来源能提供带明确 freshness 的可靠状态时展示；缺少信号时省略，不把
  “未知/暂不可见”解释成离线、退出会议或释放 roster；
- Profile 只改善名称和头像，不改变冻结身份与会议状态。

### 11.3 只读边界

Participant 面板不提供：

- 添加或移除 participant；
- 修改 Channel role；
- 转移主持人；
- 替 Agent ACK、Speech、Yield 或提交 Intent；
- 把 runtime offline 当作退会。

### 11.4 主要修改位置

- `MeetingParticipantsPanel.tsx`
- 必要的 participant presentation selector/type
- 若已有 snapshot 不足，只扩展 Desktop native 产品投影，不改 Relay 协议
- read-only/floor/host E2E

### 11.5 验收标准

- Host、Human、Agent 分组稳定，初始化期间 unknown 不被伪装成人类；
- Request、Intent、Offer、Grant/Speaking 状态准确且优先级明确；
- 状态随权威 snapshot 更新，没有依赖 raw event arrival order；
- 面板没有 roster 管理或身份代操作入口；
- 本阶段只把 `MFX-011` 标记为已解决。

## 12. 阶段六：MFX-010 Agent 主持只读观察面

> 状态：已完成（2026-08-05）

### 12.1 目标

当主持人是 Agent 时，Human roster participant 可以理解主持进程，但不能接管或冒充主持人。

### 12.2 关键实现点

- 复用当前可信 snapshot 已有的 `host.boardControl`、pending Intent、open Handoff、Floor 和 action
  状态；不建立第二份 Agent 主持状态机；
- 新增明确的只读 Host Observation 组件，展示：
  - 当前处于 Board Maintenance、Floor Decision、Offer/Grant 或 Action Finalization；
  - pending Intent 的作者、摘要和稳定状态；
  - open Directed Handoff 的来源、目标、原因和稳定状态；
  - 当前 speaker、Offer target、Board outcome/deadline 的产品表达；
  - action runnable/blocked/终态的产品表达；
- 复用阶段五的 participant 状态组件，避免 Host Observation 与 roster 面板对同一状态给出不同
  解释；
- 只读观察面与 Human Host Console 使用不同的操作边界。可以共享纯展示 primitive，但不能通过
  `readOnly` CSS 隐藏的方式保留可触发 handler；
- 当前 Human 自己的 Floor Request、Offer response、Grant Speech/Yield 继续由既有 Floor Dock
  正常提供；
- 不展示模型 reasoning、未提交草稿、工具调用日志、ACP slot/session 诊断或 Agent 私有上下文。

### 12.3 身份约束

- managed Agent 的 Human owner 关系不赋予主持签名能力；
- 非主持 Human 看不到 update/select/reject/dismiss/Recall/Close/Abort/Action completion 按钮；
- native 写命令的现有 signer/moderator/fence 校验保持不变；
- 观察面不能把 opaque control token 暴露成用户可编辑信息。

### 12.4 主要修改位置

- `MeetingFloorDock.tsx`
- 新的 `MeetingHostObservation` 及可复用只读 Intent/Handoff item
- `MeetingHostConsole.tsx`、Intent/Handoff list 的展示 primitive 拆分
- `meeting-read-only.spec.ts`、`meeting-floor.spec.ts`、`meeting-host.spec.ts`

### 12.5 验收标准

- Agent 主持场景中，Human 能看到 Board、Intent、Handoff、Floor 和 action 的稳定产品状态；
- Human 自己的合法 Floor 操作仍可用；
- 页面没有任何 Agent 主持代操作按钮或可触发 handler；
- Agent hidden reasoning、草稿和工具日志不可见；
- 本阶段只把 `MFX-010` 标记为已解决。

## 13. 阶段七：MFX-013 侧栏 attention 与排序

> 状态：已完成（2026-08-05）

### 13.1 目标

Meetings 侧栏准确区分 Speech unread 与当前身份 attention，并按产品优先级排序。

### 13.2 Native 产品投影

扩展 `MeetingListItem`，由 native 针对当前 Desktop 身份给出不泄漏其他 participant 状态的
产品级 attention。至少覆盖：

- 当前 Human 的 Offer；
- 当前 Human 的 Grant；
- Human moderator 的 Board Maintenance；
- Human moderator 的 Floor Decision；
- Human moderator 的 action runnable；
- Human moderator 的 action blocked/retry/return-to-board；
- Meeting blocked 或异常中止等稳定异常。

建议返回产品级 `needsAttention` 与有限 `attentionReason`，不把 event ID、revision、epoch 或完整
pending roster 数据塞入 list item。React 不再通过 `currentOfferPubkey` 等零散字段自行重建完整
attention 规则。

### 13.3 排序与解除

可见 Meeting 顺序使用稳定比较键：

```text
当前身份需要 attention
    → 非终态 active
    → 最近权威活动
    → meetingId 稳定兜底
```

- active 状态解除后，attention 随下一份权威 list projection 消失；
- aborted 等终态 attention 在当前身份查看终态后单独确认，不写入或伪造 Speech read marker；
- 普通 closed 且无 attention 的 Meeting 继续进入 history；
- 同一状态重复 live invalidation 不产生重复 attention；
- Board update、Intent revision 和 control heartbeat 不产生 Speech unread。

终态 attention 的本地确认键必须包含 Community 和 Meeting，并在 Community 切换时遵守现有
状态隔离。具体存储复用现有 read-state manager 还是建立专用轻量状态，在阶段设计时决定。

### 13.4 主要修改位置

- native `MeetingListItem` projection
- `tauriMeetings.ts`
- `MeetingsSidebarSection.tsx`、`useMeetingShellState.ts`
- Community-scoped attention acknowledgement（若终态需要）
- native list tests、sidebar/recovery E2E

### 13.5 验收标准

- Offer、Grant、Human Host Board/Floor/Action 和异常状态产生正确 attention；
- attention 只针对当前身份，不泄漏其他 participant 的待办；
- 侧栏顺序为 attention 优先、active 次之、最近活动再次之，并有稳定兜底；
- 只有新的 canonical Speech 产生 unread；attention 不改变 Speech 计数；
- 权威状态解除或终态被确认后，attention 按对应规则消失；
- 本阶段只把 `MFX-013` 标记为已解决。

## 14. 阶段八：MFX-015 完整终态摘要

**交付状态（2026-08-05）：已完成。** Native 已在验证 End 签名后按当前协议可证明的信息投影
`host | relay | unknown` 终止来源，并保留 Human 可读 reason。Desktop 已区分目标达成的正常关闭、
主持人中止和 Relay 中止，准确解释 `actions-recorded` 只确认行动产出登记；只有权威 Action 状态
存在时才提示外部效果可能保留。终态仍为永久只读，未增加 reopen、外部 operation list 或 receipt。

### 14.1 目标

终态页面准确表达正常结束或异常中止，不夸大 Meeting 对外部业务结果的掌握程度。

### 14.2 可信投影

- 保留并展示 Human 可读 `reason`，不只显示 `reasonCode`；
- native 投影提供稳定的终止来源分类；只在现有 verified signer/reason 能可靠区分时返回
  `host | relay/system | operator/security` 等产品值，不能可靠区分时返回 `unknown` 而不是猜测；
- `actionsAttested` 继续只表达主持人已确认行动产出登记，不解释为 Work 已完成；
- `actionStarted` 继续来自权威 action state，用于提示外部效果可能已经保留。

具体来源枚举以现有协议实际可证明的信息为准；如果协议只能证明“主持人”或“Relay”，Desktop
不得凭 reason 文案进一步声称是某个具体 Operator。

### 14.3 Desktop 呈现

`closed` 至少显示：

- 会议目标已由主持人判断达成；
- 结束人和结束时间；
- 是否经 `actions-recorded` 确认关闭，或直接正常关闭。

`aborted` 至少显示：

- 未作为目标达成正常结束；
- reason category、Human 可读说明和可用的终止来源；
- 如果曾进入 Action Finalization，明确提示外部系统效果可能保留。

两种终态都永久只读，不显示 reopen，也不声称列出全部、部分成功或未完成的外部业务操作。

### 14.4 主要修改位置

- native `MeetingEndState` projection
- `tauriMeetings.ts`
- `MeetingTerminalSummary.tsx`
- terminal native fixtures、actions/host/read-only E2E

### 14.5 验收标准

- closed 与 aborted 的产品语义清楚且不互相冒充；
- aborted 显示 category、Human reason 和可证明的 termination source；
- action 曾开始时显示“外部效果可能保留”；
- 不显示 operation list、receipt 或“Relay 已验证全部行动”；
- 终态没有 Board、Floor、Speech、Action 或 reopen 写入口；
- 本阶段只把 `MFX-015` 标记为已解决。

## 15. 阶段九：MFX-007 Meeting 标题栏收口

> 状态：已完成（2026-08-05）

### 15.1 目标

把前八个阶段已经可用的 Meeting 能力统一组织成符合 spec 9.2 的标题栏，并完成第 6 节最后一个
缺陷收口。

### 15.2 标题栏信息

新增或拆分独立 `MeetingHeader`，至少展示：

- 明确的 Meeting 图标与标题；
- `进行中 | 行动收口中 | 已正常结束 | 已中止`；
- 主持人头像、名称和冻结 Human/Agent 类型；
- participant 头像组合与人数；
- 阶段二交付的持续可用 Board 展开/收起 trigger；
- 更多菜单。

Profile 只负责头像和名称。主持与 participant type 仍由 Meeting snapshot 决定。

### 15.3 更多菜单

按身份与状态组织：

- 查看完整参会名单；
- 查看来源上下文（存在 source 时）；
- 复制 Meeting 链接；
- 打开阶段三交付的 Meeting 活动记录；
- Human 主持人在合法非终态可进入既有 Abort 确认流程；
- 终态显示阶段八交付的只读结束信息入口。

复制链接复用 Desktop 已有导航/deep-link 基础；具体 link encoding 在阶段设计时与现有路由入口
对齐，不在本计划中创造未经应用支持的新 URI 协议。

Abort 菜单只打开既有、带 reason code/说明和 destructive confirmation 的主持流程，不新增一个
绕过 native fence 的直接结束命令。

### 15.4 明确禁止的 Channel 残留

标题栏和更多菜单不得出现：

- Add people、Create agent 或 roster 修改；
- Leave、Archive、Delete Channel；
- 修改 Channel name/topic/purpose/visibility/type；
- Huddle 主操作；
- 普通 Channel Canvas、成员管理或通知设置；
- Thread、Reaction、自由消息 Composer。

### 15.5 响应式与可访问性

- 宽屏 Board trigger 始终可见；中窄窗口使用同一语义打开 Sheet；
- avatar group、状态和菜单在窄窗口可以压缩，但主持人与 Meeting 状态仍可访问；
- 所有 icon-only 操作有可读 label、tooltip 和 keyboard focus；
- destructive Abort 与普通查看操作分组并明确区分；
- 使用 Desktop 既有 rem 字号 token，不新增任意 px/rem 文字尺寸。

### 15.6 主要修改位置

- 新的 `MeetingHeader.tsx`
- `MeetingScreen.tsx`
- participant、activity、terminal、Board trigger 的既有组件衔接
- 必要的 Meeting link helper
- Meeting E2E 与宽/窄 scoped screenshots

### 15.7 验收标准

- spec 9.2 要求的信息和菜单入口完整；
- Board、participants、source、activity、copy link、Abort 和 terminal 信息按条件出现；
- Human/Agent/host/terminal 的身份门控正确；
- 不出现任何普通 Channel 管理或自由消息入口；
- 宽屏和中窄窗口均可操作，截图状态互不重复；
- 本阶段只把 `MFX-007` 标记为已解决，并完成第 6 节全部缺陷收口。

### 15.8 交付与 review 结论

- 新增独立 `MeetingHeader`，统一展示 Meeting 图标、标题、生命周期、冻结的主持人身份与类型、
  participant 头像组合和人数；
- Board trigger 在宽屏与中窄屏均持续可用；participants、source context、activity、复制链接和终态
  摘要统一进入 Meeting 更多菜单；
- 复制链接复用 Desktop 当前已经支持的 hash route，不引入新的 URI 或未经支持的 deep-link 协议；
- Human 主持人的 Abort 菜单只打开既有受控确认对话框，并继续复用同一个 Floor controller、pending
  状态和 native fence；Agent 主持、非主持人及终态不会得到该入口；
- 标题栏没有重新引入 Channel 成员管理、编辑、离开、归档、Huddle 或自由消息操作；
- 新增宽屏与中窄屏专项截图和身份/状态/menu E2E；截图内容与哈希均不同，证明响应式状态没有退化为
  重复画面；
- 相关 TypeScript、Desktop lint、native Meeting 测试和全部 Meeting Desktop E2E 已通过；仓库级
  `just ci` 也已完整通过。

## 16. 每阶段交付纪律

每个阶段都按以下顺序独立交付：

1. 进入阶段前再次对照对应 MFX 与 spec，不顺手扩大到下一个缺陷；
2. 完成必要的 native projection、TypeScript DTO、React UI 和 mock bridge；
3. 增加该缺陷的专项自动化，并运行受影响的既有 Meeting 回归；
4. review 身份、隐私、权威状态、响应式和 Channel 隔离边界；
5. 在缺陷清单中只更新本阶段对应 MFX 的状态；
6. 形成一个可独立 review、可回退的阶段提交；
7. 向用户说明本阶段交付、review 结论和下一阶段唯一目标，再进入下一阶段。

如果阶段中发现另一缺陷，只能记录或为其建立不改变产品行为的共享基础，不能把两个 MFX 合并
成一个交付，也不能在没有专项验收时提前宣称另一缺陷已解决。

## 17. 测试策略

### 17.1 Native/Tauri 测试

涉及 read projection 的阶段至少覆盖：

- 正确签名、错误签名和错误 scope；
- frozen roster 与 participant type；
- Handoff all-or-none 与合法 target/type；
- activity 的有界分页、顺序和去重；
- attention 只针对当前身份；
- End reason、source 与 action-started；
- raw protocol 字段不会被错误投影为普通产品内容。

### 17.2 Playwright E2E

优先扩展现有场景：

- `meeting-read-only.spec.ts`：产品状态、Agent host observation、隐私与禁用操作；
- `meeting-floor.spec.ts`：Speech 身份、Handoff、participant Floor 状态；
- `meeting-host.spec.ts`：Board 自动打开、Host 状态、Abort 菜单；
- `meeting-actions.spec.ts`：活动记录、action attention、terminal summary；
- `meeting-recovery.spec.ts`：Board 收起/恢复、排序、Community 隔离和草稿保持。

E2E 必须继续证明 Meeting 页面不存在普通 Channel Composer、Thread、Reaction、Huddle 和成员管理。

### 17.3 视觉证据

需要截图的阶段使用 Desktop mock bridge 和 Playwright，至少覆盖：

- 宽屏 Board open/closed；
- Agent host read-only observation；
- grouped participant panel；
- Speech Handoff；
- activity panel；
- closed/aborted；
- 最终宽屏和窄屏标题栏。

截图前使用共享 animation wait；多状态截图使用 locator/clip 聚焦，并检查 hash 不重复。视觉截图
证明布局，不替代 native 投影和身份门控测试。

### 17.4 阶段与最终门禁

阶段内运行最小但完整的相关测试集。第九阶段收口后运行：

```bash
. ./bin/activate-hermit

cargo test --manifest-path desktop/src-tauri/Cargo.toml meetings

cd desktop
pnpm check
pnpm exec playwright test \
  tests/e2e/meeting-read-only.spec.ts \
  tests/e2e/meeting-floor.spec.ts \
  tests/e2e/meeting-host.spec.ts \
  tests/e2e/meeting-actions.spec.ts \
  tests/e2e/meeting-recovery.spec.ts \
  --project=smoke
```

最终仓库门禁为：

```bash
. ./bin/activate-hermit
just ci
```

## 18. 第 6 节完成定义

只有以下条件全部满足，才能认为“Meeting Desktop 与 spec 的缺陷”已经修复完成：

1. `MFX-007` 至 `MFX-015` 各自有一个独立阶段提交和专项验收；
2. Meeting 标题栏、Board、Speech、participant、Agent host observation、侧栏、活动和终态均符合
   当前 spec；
3. canonical Speech unread 与 attention 保持分离；
4. 所有可见业务状态来自可信 snapshot/activity/list projection，不从 Markdown 或 raw event 顺序
   猜测；
5. Human 无法冒充 Agent，非主持人无法执行主持操作；
6. Board 展开/收起和面板切换不破坏草稿、timeline 或权威 window 绑定；
7. 普通用户界面不泄漏 revision、epoch、event ID、lease、control token 或 raw State；
8. Meeting 页面没有重新出现普通 Channel 管理、自由消息、Thread、Reaction 或 Huddle；
9. Desktop native、TypeScript、E2E 和最终 `just ci` 通过；
10. 缺陷清单第 6 节的 9 个 MFX 均在对应阶段 review 后逐项标记为已解决，而不是一次性批量关闭。
