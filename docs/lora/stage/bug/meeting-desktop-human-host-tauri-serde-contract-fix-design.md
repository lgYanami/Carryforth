# Meeting Desktop Human Host Tauri 契约与流程收口修复设计

> 状态：修复已实现，自动化回归通过，等待真实 Tauri Human Host 验收
>
> 记录日期：2026-08-07
>
> 范围：Meeting Desktop、Tauri IPC、Human Host/Floor/Action 命令、跨 Meeting 本地状态隔离、
> 终态回执校验与自动化门禁
>
> 关联设计：
> [Meeting V1 产品设计](../meeting/v1/meeting-v1.md)、
> [Meeting Desktop 产品规格](../meeting/desktop/meeting-desktop-spec.md)、
> [Meeting Desktop 实现计划](../meeting/desktop/meeting-desktop-implementation-plan.md)、
> [Meeting Desktop 验收说明](../meeting/desktop/meeting-desktop-acceptance.md)、
> [Meeting Desktop Human Grant 软租约未续期修复设计](meeting-desktop-human-grant-soft-lease-renewal-fix-design.md)

## 1. 结论

本次 Human 主持会议中，主持人在 Floor Decision 点击“Invite test-…”后收到：

```text
invalid args `input` for command `submit_meeting_host_action`:
unknown field `intentId`, expected one of
`intent_id`, `selection_reason`, `deferral_reason`
```

根因不是 Meeting 权限、Human moderator 身份、Floor deadline 或 Relay 状态机，而是
**Desktop TypeScript 与 Tauri Rust 在 internally tagged enum 的 variant 字段命名上不一致**：

- TypeScript 按 Desktop IPC 约定发送 `intentId`、`selectionReason`、`reasonCode` 等 camelCase 字段；
- Rust 外层 input struct 已配置 camelCase；
- 但内层 `MeetingHostAction` 只有 `rename_all = "snake_case"`；
- 该属性只把 Rust enum variant 名转换为 `select_intent`，不会把 struct-variant 内的
  `intent_id` 转换为 `intentId`；
- `deny_unknown_fields` 因而在 Tauri 参数反序列化阶段拒绝请求。

这条请求尚未进入 command body，没有读取或签署 Meeting event，更没有到达 Relay。因此当前
Meeting 没有因本次失败产生部分写入；修复并重启 Desktop 后可以在同一 canonical Floor window
仍有效时重新选择，若 window 已变化则按最新 snapshot 操作。

全流程 review 同时确认四个相邻问题：

1. Floor `grant_yield`、Action `block` 和多数 Human Host 动作存在相同输入字段缺陷；
2. Create/Host/Floor/Action 的 Result enum 也没有把 variant 字段序列化为 TypeScript 声明的
   camelCase；Create 成功后的真实返回尤其可能被 Desktop 当成失败，进而诱发重复创建；
3. 在 Meeting A 与 Meeting B 之间切换时，React 内的 unresolved command、Board draft 和 Speech
   draft 没有按 `{Community, identity, Meeting}` 隔离；
4. Human Host 的 Close/Abort 回执只确认“某种终态”，没有确认终态与本次请求一致。

Relay/DB 的 frozen roster、immutable moderator、Human Request priority、Board/Floor 顺序、
Offer/ACK/Grant、self Intent、Handoff、Close gate 与 Action fence 经静态 review 未发现新的确定性
权限或状态机错误。本次不需要修改 Meeting wire、Relay/DB schema、Community 权限或已有数据。

## 2. 事故路径与数据边界

### 2.1 实际调用链

本次点击路径为：

```text
MeetingHostIntentList
  -> controller.submit({ type: "select_intent", intentId })
  -> submitMeetingHostAction(input)
  -> Tauri invoke("submit_meeting_host_action", { input })
  -> serde_json / Tauri 参数反序列化
  X  MeetingHostAction::SelectIntent 尚未构造成功
```

对应边界：

- TypeScript：`desktop/src/shared/api/tauriMeetings.ts`
- Host UI：`desktop/src/features/meeting/ui/MeetingHostIntentList.tsx`
- Host controller：`desktop/src/features/meeting/useMeetingHostActionController.ts`
- Native input：`desktop/src-tauri/src/commands/meetings/host.rs`

外层 `MeetingHostActionInput` 使用：

```rust
#[serde(rename_all = "camelCase", deny_unknown_fields)]
```

因此 `submissionId`、`meetingId` 和 `expectedControlToken` 可以正常进入 Rust。内层 action 使用：

```rust
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
```

所以 `type: "select_intent"` 能被识别，但 `intentId` 不能映射到 `intent_id`。

仓库的 Project Document 已经出现并修复过完全同类的问题。正确范式位于
`desktop/src-tauri/src/commands/project_document/model.rs`：

```rust
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
```

### 2.2 本次失败没有产生 canonical 副作用

Tauri 在调用 `submit_meeting_host_action` 函数前完成参数反序列化。当前错误发生在这一步，因此：

- 没有创建 `submission_id` 对应的 native pending command；
- 没有读取或消费当前 `control_token`；
- 没有签署 moderator select event；
- Relay、DB 和 live subscription 都没有收到该选择；
- pending Intent 仍由 canonical Meeting State 决定；
- 不需要迁移、回滚、删除或重建该 Meeting。

### 2.3 这不是最近续约修复引入的回归

`MeetingHostAction` 与 TypeScript camelCase 类型从最初的 Human Host Desktop 实现中同时引入，
缺陷一直潜伏。此前真实验收大量使用 Agent host，或只经过 `board_update`、`close`、`begin`、
`confirm` 等没有复合字段的动作，因此没有触发这条边界。

Human Grant renewal 只负责持有 Human Speech Grant，与 `select_intent` 的 Tauri 参数解析无关。

## 3. 输入契约完整影响矩阵

### 3.1 Human Host actions

| 动作 | TypeScript 字段 | 当前结果 | 说明 |
|---|---|---|---|
| `board_update` | `body` | 可用 | 字段名无需转换 |
| `board_unchanged` | 无 | 可用 | 无 variant 字段 |
| `intent_submit` | `summary`, `addressedTo?` | 条件可用 | 不带 addressee 时可用；带 `addressedTo` 时失败 |
| `intent_refresh` | `intentId`, `summary`, `addressedTo?` | 失败 | `intentId` 必带 |
| `intent_withdraw` | `intentId` | 失败 | `intentId` 必带 |
| `select_intent` | `intentId`, `selectionReason?`, `deferralReason?` | 失败 | 本次现场错误 |
| `select_handoff` | `handoffId`, `selectionReason?` | 失败 | `handoffId` 必带 |
| `reject_intent` | `intentId`, `reasonCode`, `reason` | 失败 | 两个复合字段 |
| `dismiss_handoff` | `handoffId`, `reasonCode`, `reason` | 失败 | 两个复合字段 |
| `recall` | `reason?` | 可用 | 字段名无需转换 |
| `close` | 无 | 可用 | 另有第 6 节回执语义缺口 |
| `abort` | `reasonCode`, `reason?` | 失败 | `reasonCode` 必带 |

因此当前 Human Host 不是“只有邀请按钮坏了”，而是只能完成 Host action surface 的一部分。
既有完成条件“Human host 可以完成 Board、Intent、Floor、self Speech、Close、Abort 和 Action”
尚不能成立。

### 3.2 Human Floor actions

| 动作 | 当前结果 | 说明 |
|---|---|---|
| `request` / `withdraw` | 可用 | 无 variant 字段 |
| `offer_ack` | 可用 | 无 variant 字段 |
| `offer_decline` | 可用 | `reason` 名称一致 |
| `speech` | 可用 | `content`、`mentions`、`handoff` 名称一致；嵌套 Handoff 已单独配置 camelCase |
| `grant_yield` | 失败 | UI 会传 `reasonCode`，Rust 只接受 `reason_code` |

该问题影响所有 Desktop Human participant，也包括 Human moderator 获得 self Offer/Grant 后主动 Yield。

### 3.3 Human Action Finalization actions

| 动作 | 当前结果 | 说明 |
|---|---|---|
| `begin` | 可用 | 无 variant 字段 |
| `block` | 失败 | `reasonCode` 与 `reason_code` 不一致 |
| `retry` | 可用 | 无 variant 字段 |
| `return_to_board` | 可用 | 无 variant 字段 |
| `confirm` | 可用 | 无 variant 字段 |

所以 Human host 可以进入 Action Finalization 并确认，但无法从 Desktop 正确报告一个 runnable action
为 blocked。

## 4. 输出 DTO 契约缺口

### 4.1 四个 Result enum 与 TypeScript 不一致

以下 Rust enum 都只配置了 variant 的 snake_case，没有配置 struct-variant 字段的 camelCase：

- `CreateMeetingResult`；
- `MeetingFloorActionResult`；
- `MeetingHostActionResult`；
- `MeetingActionFinalizationResult`。

真实 Tauri 返回字段因此是：

```text
meeting_id
event_id
host_pubkey
participant_pubkeys
canonical_object_id
state_revision
```

而 `tauriMeetings.ts` 声明并由 UI 使用的是：

```text
meetingId
eventId
hostPubkey
participantPubkeys
canonicalObjectId
stateRevision
```

Host/Floor/Action controller 当前主要读取 `status`、`action` 和 `message`，这些单词字段碰巧一致，
所以错误暂时被掩盖。它们的公开 API 契约仍然是假的，后续读取 event/revision 元数据时会再次失败。

### 4.2 Create 的影响更严重

Meeting Create 成功后，Desktop 会立即读取：

```ts
result.meetingId
result.hostPubkey
result.participantPubkeys
```

并用它们更新本地 Channel/Meeting cache。真实 Rust 返回没有这些 key，成功回调可能在 Relay 已经完成
创建后抛错。随后 UI 把它当成普通失败并清掉当前 pending input；用户再次点击时会生成新的
`submissionId` 和新的 Meeting UUID，从而可能出现两个同名 Meeting。

当前验收截图中确实出现了两个同名 Human-hosted Meeting，这与该失败机制高度一致；在没有逐个回读
Create event 与 submission 记录前，不把该截图单独作为因果证明。但该输出契约错误本身由代码可以
确定，必须与输入修复一起处理。

### 4.3 不扩大到不相关 DTO

`MeetingLoadResult::UnsupportedProtocol` 的 TypeScript 类型当前明确使用 `meeting_id`、
`schema_version`。本次不盲目批量修改全部 enum；只修复已声明为 camelCase 的四个 mutation Result，
并用测试固定每个公开边界的实际 wire shape。

## 5. Human Host 流程 review

### 5.1 已确认正确的权限与状态机边界

除 Desktop IPC 和本地状态隔离外，Human Host 主链路的分层基本合理：

1. **身份与 roster**
   - Desktop 只为当前 frozen participant type 为 Human、且 pubkey 等于 immutable moderator 的身份
     显示 Host controls；
   - Tauri 每次操作重新读取当前签名身份和 canonical snapshot，再次校验 frozen roster、Human type
     与 moderator pubkey；
   - managed-by、Community owner 或 Agent 配置关系不能冒充 Meeting moderator。

2. **Board Maintenance → Floor Decision**
   - Board 只能在 `board_pending`、精确 control epoch 与 board window 中 updated/unchanged；
   - Board terminal 前不允许选择 speaker、Close 或进入 Action Finalization；
   - Floor Decision 使用独立 decision window，不复用 Board deadline。

3. **Human priority 与发言权**
   - 非主持 Human Request 由 Relay 保持优先；Human moderator 不能用普通 Human Request 绕过 self
     Intent；
   - moderator select 校验 control/decision/intent/speech revision 和候选 eligibility；
   - Select 只生成 Offer，目标 ACK 后 Relay 才生成唯一 Grant；
   - Human moderator 自己获得 Offer/Grant 时复用普通 Human Offer/Speech 控件，不可直接发 Speech。

4. **Intent 与 Handoff**
   - self Intent submit/refresh/withdraw/select 的作者和优先级由 Native 与 Relay 双重校验；
   - 连续 moderator self Speech 且存在其他 eligible Intent 时要求 deferral reason；
   - active Handoff attempt 不能被 dismiss，重试使用 canonical attempt 计数；
   - Recall 只锁存当前 turn 后返回控制，不伪造中断中的 Speech。

5. **终止与 Action Finalization**
   - direct Close 要求显式 final Board、主持人持有 idle Floor 且没有待处理 Intent/Handoff；
   - begin actions 与 direct Close 是两条不同命令；
   - begin/block/retry/return/confirm 使用 action run/window/Board fence；
   - Human Action renewal 与 Agent Action runtime 分离；
   - Agent host 对 Human Desktop 保持只读，Human 不能接管 Agent Session。

6. **不确定结果**
   - Native pending event 绑定 Relay、signer、Meeting、submission 与 payload fingerprint；
   - response loss 时保留同一个已签名 event，当前页面的 Retry 使用相同 submission，而不是产生第二个
     canonical write。

这些边界不应在本次修复中放宽或重写。

### 5.2 确认问题：跨 Meeting 本地控制状态未隔离

`ChannelRouteScreen` 在 Meeting A 切到 Meeting B 时仍渲染相同类型、相同位置的 `MeetingScreen`，没有
为 Meeting 建立 React remount/scope boundary。与此同时，下列状态是普通 component state：

- Host controller 的 `unresolved`；
- Floor controller 的 `unresolved`；
- Action controller 的 `unresolved`；
- Board draft/stale draft；
- Speech draft、Grant binding 与 stale Speech draft；
- mutation error/pending presentation。

因此存在确定的串场风险：

1. A 的 indeterminate Host command 可以让 B 的 Host controls 继续 disabled；
2. 在 B 点击“Retry exact action”可能提交 A 的 input；Native 会按 A 的 binding 处理，但 B 的 UI
   会错误展示并刷新 B；
3. A 的 Board 草稿或 stale Speech draft 可能在 B 中显示；
4. identity 切换时也可能暂时展示旧身份的 unresolved 状态，虽然 Native 最终会拒绝错误 signer。

Community 切换已有 `AppReady key={communityKey}` 的整体 remount，但这不能解决同一 Community 内的
Meeting A→B，也不能单独证明 identity boundary。

### 5.3 补强项：Human Action renewal 初始失败不可见

Human Grant renewal 的 ensure 失败会显示告警、refetch 并重试；Human Action renewal 的 initial
ensure 失败当前只执行 `console.error`。Native renewal task 一旦建立会自行重试，但“尚未建立 task”
的 transient failure 可能让 Human host 不知道 action lease 正在失去续约。

这不是本次 `intentId` 错误的原因，也不是 Relay 状态机缺陷，但应在同轮 Human Host 收口中补齐：

- 在 exact runnable action 上记录 ensure failure；
- 触发 canonical refetch；
- 在 scope 未变化且 action 仍 exact 时有界重试；
- Action card 显示可理解的告警；
- 不把本地 ensure 失败伪装为 Relay 已 blocked。

## 6. 终态回执语义缺口

Human Host `Close` 与 `Abort` 共用 `ReceiptKind::End`。当前 Tauri 只验证：

```text
status == ended
terminal_outcome in {closed, aborted}
```

它没有验证：

```text
Close  -> terminal_outcome == closed
Abort  -> terminal_outcome == aborted
```

Relay 对一个已经 terminal 的 Meeting 可以返回 `already_ended=true` 和真实 canonical outcome。若 Close
与并发 Abort 竞争，当前 Desktop 可能把“实际已 aborted”误报为本次 Close accepted；反方向同理。
页面随后 canonical refetch 最终会显示真实终态，但 mutation receipt 的产品语义不正确。

修复时还应区分两种 receipt failure：

- **不可验证**：event ID、JSON 或必要字段无法验证，保留 pending event，进入 exact retry；
- **已验证的 canonical conflict**：Meeting 已以与请求不同的 outcome 结束，删除本地 pending，返回
  明确的 definitive conflict 并立即 refetch，不能让用户无限重试一个永远不可能改变的终态。

Action `confirm` 已要求 `terminal_outcome=closed`，但同样应把“已验证为 aborted”的错配归类为 definitive
conflict，而不是不可判定结果。

## 7. 修复原则

1. Desktop/Tauri 公开 IPC 统一使用 camelCase 字段，业务 discriminator 保持既有 snake_case 值；
2. 保留 `deny_unknown_fields`，不同时接受 snake_case/camelCase 两套输入；
3. 不把 TypeScript 改成手写 snake_case payload；
4. 不修改 Relay Meeting event、DB schema、Project View 或 Community ACL；
5. 不迁移、不删除、不重建已有 Meeting、消息、Project View、Agent 或 Document 数据；
6. canonical snapshot 和 Relay/DB 仍是权限与生命周期真相，React 不自创第二套状态机；
7. exact retry 必须保持原 Relay、signer、Meeting、submission 和签名 event 绑定；
8. 修复应覆盖整个 Human Meeting mutation surface，不能只给 `intentId` 加一个字段 alias。

## 8. 修复实现方案

### 8.1 修复三个输入 action enum

在以下 enum 上增加 `rename_all_fields = "camelCase"`：

- `MeetingHostAction`；
- `MeetingFloorAction`；
- `MeetingActionFinalizationAction`。

统一形式：

```rust
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
```

保留：

- `type: "select_intent"`、`type: "grant_yield"`、`type: "return_to_board"` 等现有 discriminator；
- Rust 内部 snake_case 命名；
- 现有输入长度、canonical ID、authority 和 state fence 校验；
- `DirectedHandoffInput` 自己已有的 camelCase 配置。

### 8.2 修复四个 mutation Result enum

在以下 enum 上增加 `rename_all_fields = "camelCase"`：

- `CreateMeetingResult`；
- `MeetingFloorActionResult`；
- `MeetingHostActionResult`；
- `MeetingActionFinalizationResult`。

保留 `status: "accepted" | "indeterminate"` 的值不变，只把 variant 内的复合字段改为真实
TypeScript 契约。不要顺手改变 `MeetingLoadResult` 等明确使用其他 wire shape 的读取 DTO。

### 8.3 建立 Human Meeting command scope

定义唯一 scope：

```text
communityKey + relay identity/pubkey + meetingId
```

按该 scope 隔离：

- Host/Floor/Action unresolved inputs；
- exact-retry presentation；
- Board draft 与 stale Board draft；
- Speech draft、Grant binding 与 stale Speech draft；
- mutation error/attention state。

实现要求：

1. Meeting B 永远不能读取、显示或 retry Meeting A 的 unresolved command；
2. identity B 永远不能读取、显示或 retry identity A 的 command；
3. 返回 Meeting A 时仍要能恢复 A 的 exact retry，不能仅靠 `key={meetingId}` 把它静默丢失；
4. scope store 只保存已经提交所需的 public payload，不保存私钥；
5. store 有界，并在 definitive accepted/rejected 后清除；
6. 如果使用 module-level store，必须接入 `resetCommunityState()`，避免 Community 切换泄漏；
7. Native 仍执行最终 Relay/signer/Meeting/fingerprint binding 校验，前端 scope 不是授权机制。

本次采用有界 `sessionStorage` scoped pending store：最多保存 64 条 public mutation input，键由
`{Community, identity, Meeting, lane}` 构成。它跨 React remount 保留 exact retry，但不保存私钥；
Native 仍按 relay、signer、Meeting、fingerprint 绑定已签名 event。由于 Community 已进入存储键，旧
Community 的条目既不会被新 Community 读取，也无需依赖 module singleton reset。不能用“切页直接清空
unresolved”破坏 response-loss 的 exact retry。

### 8.4 收紧 End receipt 校验

让 Host End receipt validator 接收请求动作的 expected outcome：

```text
close -> closed
abort -> aborted
```

结果分类：

- event ID、Meeting ID、状态、字段和 expected outcome 全部匹配：accepted；
- canonical outcome 已确定但与请求不一致：definitive conflict，删除 pending，返回明确错误并 refetch；
- receipt 丢失、损坏或无法证明：indeterminate，保留同一 event 供 exact retry。

对 Action Confirm 做相同 conflict 分类，不改变其只允许 `closed` 的业务语义。

### 8.5 补齐 Human Action renewal 初始失败体验

复用 Human Grant ensure 的成功模式，但保持 Action 自己的 run/window/Board fence：

- 仅对 exact `finalizing_actions + runnable + Human moderator` 启动；
- ensure failure 显示在 Action card；
- refetch 后只有 exact run/window/Board 仍成立才重试；
- scope/identity/Meeting 变化立即停止旧重试；
- Native task 一旦建立，React 导航不取消其已有 renewal；
- 不修改 Agent host 的 ACP renewal。

## 9. 自动化修复方案

### 9.1 Native literal JSON contract tests

现有 Rust 测试直接构造 `MeetingHostAction::SelectIntent` 等 enum，绕过了真实 JSON 反序列化；现有
Playwright 又把 JS object 直接交给 TypeScript mock，同样绕过 Tauri/Serde。

新增以 Desktop 实际 payload 为输入的 `serde_json::from_value` 测试，至少覆盖：

- Create input；
- Host 全部 12 个 action variant；
- Floor 全部 6 个 action variant；
- Action 全部 5 个 action variant；
- 每个 optional 复合字段至少有一例携带非空值；
- `Speech.handoff.targetPubkey/handoffType` 的嵌套形状；
- camelCase outer input 与 snake_case discriminator 的组合。

负向测试必须证明：

- `intent_id`、`reason_code` 等旧 snake_case variant 字段仍被 `deny_unknown_fields` 拒绝；
- unknown field、错误 discriminator、缺失必填字段仍 fail closed；
- 修复没有放宽 canonical ID、文本上限或 authority 校验。

### 9.2 Result serialization contract tests

对 Create/Host/Floor/Action 的 Accepted 与 Indeterminate 分支执行 `serde_json::to_value`，精确断言：

- `status` 和 `action` 值；
- `meetingId/eventId/hostPubkey/participantPubkeys`；
- `canonicalObjectId/stateRevision`；
- 不出现相应 snake_case key。

测试应使用 exhaustive helper 或显式全 variant matrix，使后续新增 action/result variant 时编译或测试
明确要求更新 contract，而不是再由真实 Desktop 验收发现。

### 9.3 Desktop E2E

保留现有 mock E2E 作为 UI/state 行为测试，但不再把它当作 Tauri wire 证明。补齐：

1. Host `addressedTo/intentId/selectionReason/deferralReason/reasonCode` payload 断言；
2. Floor `grant_yield.reasonCode`；
3. Action `block.reasonCode`；
4. Create accepted result 的关键字段与 cache/navigation；
5. Meeting A indeterminate → 切换 B：B 不被锁、看不到 A 的草稿、不能 retry A；
6. 返回 A：只恢复 A 的 exact command；
7. identity A→B→A 与 Community A→B→A 的同类隔离；
8. Close/Abort outcome race 的 definitive conflict 展示；
9. Human Action ensure failure 的告警、refetch、exact retry 和 scope cancel。

### 9.4 Native/Relay 语义回归

继续运行现有 native authority/builder tests 和 Relay/DB Meeting tests，证明：

- Board/Floor 顺序不变；
- Human Request priority 不变；
- self Intent、deferral、Handoff attempt 和 Recall 不变；
- Offer/Grant/Speech/Yield 仍由 Relay canonical state 控制；
- direct Close、Abort、Action begin/block/retry/return/confirm fences 不变；
- response loss 仍重放同一 event；
- Agent host 与 Agent participant 不进入 Human Tauri mutation path。

### 9.5 真实 Tauri 验收

mock 无法替代本轮验收。修复后至少完成一场 Human-hosted、Human/Agent 混合 roster Meeting：

1. Desktop 创建 Meeting，成功后只出现一个 Meeting，并可立即导航；
2. Board updated 与 unchanged 各有覆盖；
3. 选择 Agent Intent → Offer → Agent ACK → Grant → Speech；
4. reject Intent；
5. self Intent submit/refresh/withdraw/select → self Offer/ACK/Grant/Speech；
6. select/dismiss Handoff；
7. Human participant Request/withdraw 与 priority；
8. Human Speech + Directed Handoff；
9. Human `grant_yield`；
10. Recall；
11. direct Close；
12. 另一场覆盖 Abort；
13. Action begin → block → retry → return-to-board，以及 begin → confirm；
14. Meeting A/B 切换、断线 exact retry 和 identity/Community 隔离；
15. Agent-hosted Meeting 对 Human 仍为只读 Host observation。

## 10. 验收标准

### 10.1 本次现场问题

- 点击“Invite test-…”不再产生 `unknown field intentId`；
- Tauri 接受 camelCase payload，Relay 创建正确的 Offer；
- UI 只在 Relay canonical snapshot 出现 Offer 后推进，不制造本地 Grant；
- 旧 Meeting 无需重建，若原 decision window 已失效则显示最新合法操作。

### 10.2 全面收口

- 第 3 节所有 Human Host/Floor/Action 动作均可穿过真实 Tauri boundary；
- 四组 mutation Result 与 TypeScript 类型逐字段一致；
- Create 成功不会被本地回调误判失败，不因重试生成重复 Meeting；
- A 的未决命令、错误和草稿不出现在 B；返回 A 仍能 exact retry A；
- Close 不能把 canonical Abort 报为成功 Close，Abort 也不能把 canonical Close 报为成功 Abort；
- Human Action renewal 初始失败可见且可恢复；
- Relay/DB 权限、deadline、lease、fence 和 canonical state 语义没有变化；
- 实现和测试不删除、清空、覆盖或迁移当前开发数据。

## 11. 实施记录

2026-08-07 已完成以下交付：

1. `CreateMeetingResult`、Host/Floor/Action input 与 result enum 已统一为 camelCase variant
   fields，业务 discriminator 继续使用 snake_case；`deny_unknown_fields` 保持不变；
2. Native literal JSON contract matrix 已覆盖 Host 12 个、Floor 6 个、Action 5 个 action
   variant，以及 Create/Host/Floor/Action result；snake_case variant fields 有负向拒绝测试；
3. Host Close/Abort 与 Action Confirm 的回执已区分“无法证明”与“canonical outcome 明确冲突”：
   前者保留 exact retry，后者清除 pending 并返回确定性错误；
4. Host/Floor/Action unresolved command 已按
   `{Community, identity, Meeting, lane}` 隔离并有界保存；Meeting A/B E2E 证明 B 不显示或重放 A
   的命令，返回 A 后仍重放完全相同的 submission；
5. `MeetingFloorDock` 按完整 command scope remount，Board/Speech 草稿与错误展示不会跨 Meeting 或
   identity 复用；Board draft hook 在 scope 改变的同一 render 即 fail closed；
6. Human Action Finalization 初次 renewal 失败现在会显示告警、回读 canonical snapshot，并仅在
   exact `{actionRunId, actionWindowEpoch, boardEventId}` 仍成立时重试；
7. Human Host 全流程复核和自动化路径覆盖 Board、Intent、Offer、Grant、Speech、Yield、Recall、
   Close/Abort、Handoff、Action Finalization，以及 Agent-hosted read-only 边界，未发现需要修改
   Relay/DB 权限或状态机的新问题；
8. 为遵守 Desktop 文件大小门禁，Host receipt validator 已拆入独立模块，没有增加门禁例外。

自动化结果：

- Desktop Tauri Meeting tests：52 passed；
- Desktop unit tests：3563 passed；
- Human Meeting Playwright smoke：28 passed；
- Desktop check、typecheck 与 Tauri clippy：通过；
- `git diff --check`：通过。

运行态交付也已完成：只清理并重建 Buzz 可执行目标，未删除容器或 volume；`localhost:3000`
Project View 状态仍为 schema 3、initialized/strict-ready/enabled，Project revision 46，NIP-11 继续
宣告 `buzz-project-view-v3`、Project Context、Project Document 与 Meeting V2；Desktop、Relay 和原有
managed ACP 均已重新启动。

尚未在本文声称完成第 9.5 节真实 Tauri 人工验收。重建后的现场验收应重点确认本次原始路径：
Human 主持人在 Floor Decision 点击 `Invite test-*` 能生成 canonical Offer，不再出现
`unknown field intentId`。

## 12. 交付顺序

1. 为三个 action enum 和四个 mutation Result enum 补齐 Serde 字段映射；
2. 增加 Native literal JSON 输入/输出 contract tests；
3. 收紧 Host/Action End receipt 的 expected outcome 与 definitive conflict 分类；
4. 建立 `{Community, identity, Meeting}` scope，隔离 unresolved command 与草稿；
5. 补齐 Human Action renewal initial ensure 的 UI 告警和有界重试；
6. 扩展 Create/Host/Floor/Action Playwright payload、隔离和恢复测试；
7. 运行 Desktop Tauri tests、Meeting E2E、typecheck、build、fmt/clippy 和相关 Relay/DB tests；
8. 在不清理任何现有数据的前提下重新构建 Desktop；
9. 完成第 9.5 节真实 Tauri Human Host 验收并更新本文状态。

## 13. 非目标

- 不新增主持权转移、副主持或 takeover；
- 不改变 Community owner/admin/member 权限；
- 不让 Human 代 Agent ACK、Speech、主持或确认 Agent Action；
- 不把 Meeting 改成普通 Channel；
- 不为旧 snake_case Desktop payload 增加双协议兼容层；
- 不回滚现有 Meeting、Project View、Document、消息或 Agent 数据；
- 不用删除数据库、重建 Community 或重新初始化 Project View 作为修复手段。
