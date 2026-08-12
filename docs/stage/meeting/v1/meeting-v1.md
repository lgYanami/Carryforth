# Meeting V1：主持式发言权接力协议

> 状态：概念设计已完成，后端实现设计见
> [Meeting V1 后端实现设计](meeting-v1-backend-implementation-design.md)
>
> 冻结后的决策调整见：[Meeting V1 决策变更记录](changelog.md)
>
> 协议名称：Moderated Baton Protocol（主持式发言权接力协议）
>
> 本文定义 Meeting V1 的产品语义、参与角色、发言意图、发言权传递、Human 介入、
> 主持人仲裁和故障边界。事件 kind、数据库表、具体超时和兼容迁移在实现设计中确定。

## 1. 背景

[Meeting V0](../meeting-v0.md) 已经证明 Human 与 Agent 可以进入同一个私有文字会议室，
共享固定名单、完整消息记录和 Relay 权威的唯一发言权。V0 使用 Ready、Claim、Pass 和
随机仲裁决定每轮 speaker。

这套机制能够保证“只有一人发言”，但它把 Agent 的 Intent LLM 延迟带入了共享仲裁：
第一份 Claim 到达后，Relay 可能等待已经 Ready 的多个 Agent 全部产生 Claim 或 Pass。
慢模型、模型池拥塞或 Agent 故障因此可能拖慢其他参会者。

Meeting V1 不再把所有潜在发言者组成一个同步决策 cohort，而是采用：

> 主持人维护全局会议方向，当前 speaker 可以围绕明确问题进行局部接力，Human 可以预订
> 下一次发言权，Relay 始终维护唯一且可恢复的 Speech Grant。

## 2. 目标与非目标

### 2.1 目标

Meeting V1 要提供：

1. **稳定性**：任一 Agent 的 LLM 延迟、离线或崩溃不会冻结全部参会者；
2. **连贯性**：主持人维护会议方向，当前 speaker 可以把具体问题直接交给明确对象；
3. **灵活性**：主持人、定向接力、Human 介入和异步发言意图可以同时存在；
4. **可解释性**：为什么选择、传递或撤销发言意图，都有签名且可恢复的控制记录；
5. **唯一发言权**：任何时刻最多只有一个有效 Grant，每个 Grant 最多接受一条 speech；
6. **Agent 友好**：轻量发言意图与完整发言生成分离，未获权者不生成完整候选发言；
7. **异步性**：意图提交、意图整理和主持人预判不要求当前 Control Token 在主持人手中。

### 2.2 非目标

本文不定义：

- 正式议程、候选决议、表决、人类确认或项目写回；
- 动态参会名单；
- 多主持人协同、正式主持权转移或主持人选举；
- 音频、视频、TTS 或实时打断当前语音；
- Desktop 具体界面；
- 事件 kind、SQL schema 和 ACP 内部代码结构。

## 3. 核心模型

Meeting V1 把“控制下一位”和“允许当前发言”分开：

```text
ControlToken
    表示谁有权安排下一位 speaker

SpeechGrant
    表示谁被 Relay 授权发表一条正式 speech
```

主持人持有持续的会议协调权，但不能绕过 Relay 直接发布消息。主持人需要发言时，也要由
Relay 为自己创建一次性 Speech Grant。

```text
HOST_CONTROL
    ├── Human 请求优先
    ├── 主持人选择自己
    ├── 主持人选择一个 SpeechIntent
    └── 没有合适对象时保持 HOST_IDLE
              ↓
          OFFERED(target)
              ↓ ACK
          GRANTED(target)
              ↓ 一条 speech
    ┌─────────┴──────────┐
    │                    │
Directed Handoff      返回 HOST_CONTROL
```

产品界面可以把 `ControlToken` 和 `SpeechGrant` 都称为“发言权”，但协议必须区分两者，
避免主持人协调权、speaker 单次授权和 Human 下一轮请求被混成同一种租约。

`HOST_CONTROL / HOST_IDLE`、`OFFERED` 和 `GRANTED` 是互斥的权威 Floor 状态。一个会议
不能同时保留未决 Offer 和活动 Grant。

V1 还区分三类单调版本：

- `speech_revision`：只在 canonical speech 被接受时递增；
- `intent_revision`：SpeechIntent 和 Human Request 池改变时递增；
- `floor_revision`：Control Token、Offer、Grant、Recall、Yield、Expiry 或 Directed
  Handoff 控制投影改变时递增。

`grant_id` 是活动 Grant 的稳定身份，不能再隐含等于“最新 Floor State event ID”。其他
参会者可以在 `GRANTED` 期间异步提交 Intent 或 Human Request，这些更新不能使当前
Grant 失效。

## 4. 角色

### 4.1 主持人

每场 Meeting V1 恰好有一名主持人。协议字段使用 `moderator_pubkey` 表达该身份，不复用
Meeting V0 中表示创建者或 Channel Owner 的 `host_pubkey`：

- 主持人必须是固定参会者；
- 主持人可以是 Human，也可以是 Agent；
- 创建会议时显式指定主持人；未指定时由创建者担任；
- 创建者、Channel Owner 和主持人可以是同一身份，但它们是不同职责；
- Meeting V1 第一版不支持正式主持权转移；
- 主持人拥有最初的 Control Token；
- 主持人可以在任何时间整理或撤销发言意图；
- 主持人只有持有 Control Token 时才能选择下一位 speaker；
- 主持人可以请求自己发言，但 Human Floor Request 的优先级更高。

主持人不可绕过 Grant 直接发言，也不能为两个参会者同时创建 Grant。

### 4.2 普通参会者

Human 与 Agent 普通参会者都可以：

- 阅读完整 speech timeline、共享控制日志和参会名单；
- 异步提交一个发言意图；
- 撤回或刷新自己的发言意图；
- 获得 Grant 后发表一条 speech；
- 在 speech 中发起一次有原因的 Directed Handoff；
- 接受或拒绝指向自己的 Offer。

Human/Agent 类型由 Relay 根据权威身份目录判定，并在会议创建时冻结为独立参与者属性。
不能根据 `owner`、`member` 或 `bot` Channel role 临时推断，因为 Agent 主持人也可能同时
是 Channel Owner。Human Floor Request 只能由冻结类型为 Human 的身份提交。

### 4.3 Human 参会者

Human 除普通参会能力外，还可以提交 Human Floor Request，无需等待主持人选择自己的
SpeechIntent。

Human Floor Request 表示：

> 不打断当前已开始的 speech，但无条件预订下一次可用发言权。

多个 Human 请求按 Relay 接收顺序排队。每名 Human 同时最多有一个未处理请求。

若 moderator 本身是 Human，仍通过主持人的 self Intent 和 moderator selection 发言，不使用
Human Floor Request；否则同一身份会绕过“主持人自发言低于其他 Human”和连续自发言
约束。Human Floor Request 在本协议中指非 moderator Human 的直接介入能力。

## 5. 发言意图

### 5.1 SpeechIntent

SpeechIntent 是参会者发给主持人的一句简短发言摘要，不是候选 speech：

```text
SpeechIntent
- intent_id
- author_pubkey
- basis_speech_revision
- summary
- addressed_to?
- created_at
- updated_at
- expires_at?
- state:
    pending | selected | rejected | withdrawn | stale | consumed
```

语义规则：

1. 同一参会者最多有一个 `pending` SpeechIntent；
2. summary 必须是简短的一句话，概括准备表达的内容；
3. Intent 不携带完整候选正文、隐藏推理或工具结果；
4. 在 Intent 被选择、撤销、拒绝或失效之前，作者不得创建第二个 Intent；
5. 作者可以撤回自己的 Intent；
6. 讨论变化时，作者可以刷新同一个 `intent_id`，而不是重复创建；
7. Intent 被选择并产生 Grant 后进入 `selected`，正式 speech 被接受后进入 `consumed`；
8. Intent 不因任意一条新 speech 自动删除，但必须保存 `basis_speech_revision`，便于主持人
   判断其是否已经过时；
9. 明显过时的 Intent 可以由作者刷新、撤回，或由主持人以 `superseded` 原因拒绝；
10. Meeting End 使所有未处理 Intent 进入终态。

SpeechIntent 可以在发言权不属于主持人时提交。主持人也可以在其他人发言期间异步整理、
排序、合并判断和准备下一次选择。

### 5.2 可见性

SpeechIntent 属于共享 floor control log，不进入正式 speech timeline。

默认所有参会者都可以查看：

- Intent 作者与摘要；
- Intent 当前状态；
- 主持人的选择或拒绝；
- 拒绝原因。

这样可以减少重复意图，并使主持人的议程控制具有可审计性。客户端可以在主时间线之外
以“举手列表”或“发言意图池”展示。

### 5.3 主持人拒绝

主持人可以在任何时间拒绝一个 pending Intent。拒绝必须形成持久、签名的控制记录：

```text
IntentRejection
- intent_id
- rejected_by
- reason_code
- reason_text
- rejected_at
```

建议的 reason code：

- `off_topic`
- `duplicate`
- `superseded`
- `unsupported`
- `agenda_mismatch`
- `meeting_ended`

`reason_text` 必填，并直接通知 Intent 作者。被拒绝后，作者可以基于新的语义依据提交新的
Intent。

## 6. 主持人选择

主持人持有 Control Token 时，按以下优先级安排下一步：

1. 最早的 Human Floor Request；
2. 主持人自己的有效 SpeechIntent；
3. 主持人从 pending SpeechIntent 或未完成 Directed Handoff 中选出的目标；
4. 没有合适发言者时进入 `HOST_IDLE`。

第 3 项中的普通 Intent 与未完成 Handoff 不设固定相对顺序，由主持人根据会议方向、
问题重要性和上下文判断；二者都低于主持人的有效 self Intent。

主持人选择其他参会者时，应引用 `intent_id`，并可以记录简短的 selection reason。
重新安排未完成定向问题时，应引用稳定的 `handoff_id`；目标和原始原因由 Relay 得出，
不能由客户端改写。
选择结果不会直接产生长租约，而是先产生短暂 Offer。

主持人自己的 SpeechIntent 同样是一次性的。为避免主持人长期垄断：

- 主持人不能把 Directed Handoff 发给自己；
- 主持人的每个 Intent 只能消费一次；
- 默认最多连续一次主持人自发言；存在其他有效 Intent 时，下一次应选择、拒绝或明确
  延后其他 Intent，而不是自动继续主持人发言。

## 7. Offer 与 Speech Grant

### 7.1 Offer

任何安排都先创建短暂 Offer：

```text
FloorOffer
- offer_id
- target_pubkey
- allocation_source:
    moderator_select | directed_handoff | human_request | fallback
- turn_role:
    participant | moderator_self
- source_intent_id?
- source_request_id?
- source_handoff_id?
- source_speech_event_id?
- selection_reason?
- reason_type?
- reason_text?
- ack_deadline
```

目标 ACK 只表示：

> 当前参与实例仍然在线，愿意接受这次发言机会。

ACK、Decline 和 Offer 超时都不需要 LLM。Agent Harness 可以依据参会策略确定性 ACK；
Human 通过界面接受或拒绝。

Offer 被拒绝或超时后：

- 不产生 Speech Grant；
- 不增加 Directed Handoff 深度；
- Moderator selection 引用的 SpeechIntent 仍为 pending，不会因为一次 Offer 失败而被消费；
- Directed Handoff 本次结束，作为所有参会者可见的共享未完成定向问题保留，不会稍后
  自动执行；
- Human Request 的 Decline 或超时结束本次请求，Human 可以重新提交；
- Moderator-self Offer 失败后回到主持人控制，其 SpeechIntent 仍为 pending；
- Control Token 最终回到主持人，除非仍有更高优先级的 Human 请求。

SpeechIntent 只有在对应 Offer 已 ACK、Relay 真正创建 Grant 后才进入 `selected`。Offer
创建本身不等于获得发言权。

### 7.2 Speech Grant

Offer ACK 后，Relay 创建唯一、一次性的 Speech Grant：

```text
SpeechGrant
- grant_id
- holder_pubkey
- floor_revision
- speech_revision
- source
- soft_lease_expires_at
- hard_deadline
- handoff_depth
```

Grant 规则：

1. 同一会议任何时刻最多一个活动 Grant；
2. 只有 holder 可以消费；
3. 每个 Grant 最多接受一条正式 speech；
4. speech 被接受、Grant 被消费和下一控制状态必须原子提交；
5. holder 可以主动 Yield；
6. soft lease 可以由确定性的、单调递增的 Progress 续期；
7. Progress 永远不能越过 hard deadline；
8. speech 或 Yield 一旦成功，立即推进，不等待 lease 结束；
9. Grant、Yield、Expiry 和 ACK 重试都以事件 ID 幂等。

推荐实现以短 soft lease 检测 Agent 崩溃，以最长 5 分钟 hard deadline 容纳模型推理和
为发言调查上下文的工具调用。具体时长属于实现参数。

## 8. Directed Handoff

### 8.1 目的

允许非主持人传递发言权，不是为了让参会者取代主持人控制全局方向，而是为了处理局部、
明确的定向对话：

- speaker 提出了一个有明确回答对象的问题；
- speaker 需要另一名参会者提供事实、证据或上下文；
- speaker 希望另一名参会者澄清、检查或回应具体观点。

当前 speaker 最了解刚刚提出的问题应该由谁回答。允许有界 Handoff 可以跳过一次主持人
重新理解和选择，减少主持 Agent 的 LLM 延迟，并保持问答连续。

### 8.2 Handoff 形状

Handoff 必须和当前正式 speech 绑定：

```text
DirectedHandoff
- handoff_id
- from_pubkey
- to_pubkey
- source_speech_event_id
- reason_type
- reason_text
- handoff_depth
```

建议的 reason type：

- `question`
- `information_request`
- `clarification`
- `review`
- `response_requested`

`reason_text` 必填，明确说明“为什么把下一次发言机会交给你”。目标收到 Offer 时必须同时
看到：

- 发起者；
- 原始 speech；
- reason type 与 reason text；
- 当前会议上下文；
- Offer ACK 截止时间。

Directed Handoff 不要求目标事先拥有 SpeechIntent，也不自动消费目标已有的 voluntary
Intent。

### 8.3 原子边界

推荐把 Handoff 元数据附加在当前 speech 上。Relay 接受 speech 时，在同一事务中：

1. 消费当前 Grant；
2. 保存 canonical speech；
3. 保存 Directed Handoff；
4. 根据优先级决定创建下一 Offer 或归还主持人。

这样不会出现“speech 已接受但 Handoff 丢失”或“Handoff 引用了未接受 speech”的半完成
状态。

### 8.4 五次接力上限

`handoff_depth` 表示自上次 Control Token 真正回到主持人后，成功发生的参会者到参会者
直接传递次数。

默认规则：

- 主持人选择或主持人发言后安排第一位 speaker 时，深度为 0；
- 每次非主持人 Directed Handoff 的 Offer 被 ACK 并真正创建 Grant 时，目标 Grant
  暂占下一个深度；只有目标发表 canonical speech 后才提交这次增加；
- 最多允许 5 次；
- 第五次 Handoff 的目标仍可正常发言；
- 该目标发言结束后不得创建第六次直接传递，Control Token 必须归还主持人；
- Offer Decline、Offer 超时、Yield、Grant Expiry 和无效 Handoff 不增加深度；若目标
  Grant 已暂占深度，非 speech 终态必须恢复此前值；
- Human Floor Request 不重置深度，也不能绕过五次限制；
- 只有 Control Token 真正回到主持人时，深度重置为 0。

主持人还可以设置 `RECALL_AFTER_CURRENT`，要求当前 speech 结束后提前收回 Control
Token。Recall 不打断已经开始的 speech。

Recall 在 `OFFERED` 阶段可以取消尚未 ACK 的非 Human Offer；在 `GRANTED` 阶段只锁存
forced return，等待当前 Grant 结束。已经排队或已经 Offered 的 Human Request 仍具有
更高优先级。

### 8.5 未完成问题的生命周期

Handoff 表达一个定向问题，Offer/Grant 表达回答它的一次尝试。Handoff 创建后保持
`open`；Offer Decline/timeout、被更高优先级覆盖，以及目标 Grant Yield/Expiry，都不会
把问题误标为已回答。

- 只有主持人显式重新选择 open Handoff，Relay 才能再次向原目标创建 Offer；
- 基于该 Handoff 获得 Grant 的目标发表 canonical speech 后，问题进入 `answered`；
- 主持人可以用必填原因把当前未处于活动 Offer/Grant 的 open Handoff 标为 `dismissed`；
- Meeting End 把其余 open Handoff 终结；
- 实现必须限制一场会议同时存在的 open Handoff 数量，避免完整 State 无界增长；达到
  上限的新 Handoff 要形成可解释的 blocked 结果，不能静默丢失。

## 9. Human Floor Request

Human Floor Request 是高于主持人自发言和 Directed Handoff 的下一轮优先请求。

语义规则：

1. Human Request 不撤销当前有效 Grant，也不打断当前 speech；
2. 当前 speech、Yield 或 Expiry 结束后，最早的 Human Request 获得下一个 Offer；
3. 多个 Human Request 按 Relay 接收顺序排队；
4. 每名 Human 同时最多一个未处理请求；
5. 主持人不能拒绝或撤销 Human Request；
6. Human 可以主动撤回自己的请求；
7. Human speech 结束后继续适用普通 Handoff、五次上限和归还主持人的规则；
8. Human Request 不重置现有 handoff depth。

Human Request 到达时的并发边界：

- 当前只是 `OFFERED`、目标尚未 ACK：Human Request 可以抢占并取消该 Offer；
- 当前已经 `GRANTED`：不撤销 Grant，只把 Human 加入下一席队列；
- Offer ACK 与 Human Request 并发时，以 Relay 锁定会议后的数据库接收顺序决定边界；
- 被 Human 抢占的 moderator-selected Intent 仍为 pending；被抢占的 Handoff 不自动延后。

## 10. 控制优先级

当前 Grant 结束时，Relay 按以下顺序决定下一状态：

```text
1. 存在 Human Floor Request
      → OFFER 最早请求的 Human

2. 主持人已设置或锁存 RECALL_AFTER_CURRENT
      → HOST_CONTROL

3. handoff_depth 已达到 5，已锁存 forced return
      → HOST_CONTROL

4. 当前 speech 携带合法 Directed Handoff
      → OFFER handoff target

5. 其他情况
      → HOST_CONTROL
```

因 Human Request、Recall 或深度限制而未执行的 Handoff 不会在未来偷偷自动执行。它仍
作为共享控制记录存在，并可以成为主持人重新取得 Control Token 后需要处理的定向问题。

未完成定向问题与一次 Offer/Grant 尝试是不同对象。Handoff 创建后保持 open；Offer
Decline/timeout、被更高优先级覆盖，以及目标 Grant Yield/Expiry，都只终结当前尝试。
只有主持人显式重新选择该 `handoff_id` 才能再次创建 Offer；基于它获得 Grant 的目标
发表 canonical speech 后，问题才视为 answered。Meeting End 终结所有仍 open 的问题。

Recall 和五次上限形成持续的 `forced_return_to_moderator`。已经排队的 Human 可以先按
FIFO 发言，但在 forced return 完成前不得执行新的 Directed Handoff；Human 队列清空后
必须进入 `HOST_CONTROL`，随后清除 Recall、forced return 并把 handoff depth 重置为 0。

Meeting End 高于所有优先级，并使 Intent、Request、Offer、Grant 和 Recall 全部进入
终态。

## 11. 主持人的异步 Agent 模式

> 2026-07-30 变更：本节的投机 `ModeratorPlan`、完整 fingerprint 失效和“取消后重判”
> 语义已由
> [主持人乐观决策设计](meeting-v1-moderator-optimistic-decision-design.md)
> 替代。当前语义只在主持人取得 Control Token 后启动完整 LLM 判断，late Agent Intent
> 进入下一候选批次，判断期间不因 Meeting State 变化物理 Cancel。下文保留为变更前的
> 概念背景，不再作为待实现规范。

主持人是 Agent 时，不能等到 Control Token 返回后才开始理解 Intent 池。Moderator
Controller 应当在其他人发言期间异步维护：

```text
ModeratorPlan
- observed_speech_revision
- observed_floor_revision
- pending_intent_ids
- open_handoff_ids
- proposed_action:
    moderator_speak | select_intent | select_handoff | idle
- proposed_target?
- proposed_handoff_dismissals[]:
    handoff_id
    reason_code
    reason_text
- selection_reason?
- state:
    preparing | ready | stale | consumed
```

ModeratorPlan 只是私有、可失效的预判，不是 Relay 权威决定。Control Token 返回主持人时，
Harness 必须先同步最新 speech、Intent 和 Floor，再决定：

- 计划仍新鲜：提交选择；
- 计划已经过时：取消并重新判断；
- Human Request 已出现：直接服从 Human 优先级；
- Moderator Agent 不可用：进入主持人超时兜底。

主持人的 LLM 选择可以在别人发言期间完成，因此通常不会进入换人关键路径。
Open Handoff 本身只是主持人可处理的上下文，不单独启动或重置主持人 decision
deadline；没有 pending Intent 或 Human Request 时，会议仍可保持 `HOST_IDLE`。

## 12. 主持人不可用

主持人是单一协调角色，但不能成为永久停会的单点。

当 Control Token 已回到主持人、存在可处理工作，而主持人在配置的 decision deadline
内没有有效动作时，Relay 使用版本化的确定性兜底策略：

1. Human Floor Request；
2. 主持人的有效 SpeechIntent，但连续自发言规则要求先处理其他 Intent 时除外；
3. 等待时间最长的有效普通 pending Intent；
4. 没有有效 Intent 时保持 `HOST_IDLE`。

兜底只安排下一位 speaker，不转移主持人角色。主持人恢复后继续拥有协调和拒绝权限。
兜底不自动重试任何未完成 Directed Handoff；这类定向问题只能由主持人重新判断并显式
选择。目标若仍想回答，可以另行提交自己的 SpeechIntent。
正式主持权转移留给后续版本。

## 13. Agent LLM 与工具边界

Meeting V1 把 Agent 工作分成三类：

### 13.1 Participant Intent

- 异步判断是否有必要提交一句 SpeechIntent；
- 可以由参与模式、mention、定向问题或项目结果触发；
- PASS 只写入 Agent 私有账本，不进入共享控制日志；
- 同一 Agent 同时只维护一个 pending Intent；
- 新 speech 到达时合并到最新上下文，不为每条控制事件启动模型。

### 13.2 Moderator Planning

- 只由主持 Agent 运行；
- 在别人发言期间异步整理 Intent；
- 输出主持人自发言、选择 Intent 或保持 idle；
- 不直接签发 Grant，由 Harness 提交、Relay 验证。

### 13.3 Granted Speech

- 只有 Grant holder 运行完整发言 Turn；
- 可以调用 Agent 正常暴露的工具获取项目上下文和证据；初版通过 Meeting prompt 约定
  只调查、不执行任务或持久写操作；
- 最终输出 SAY 或 YIELD；
- Harness 规范路径中的 SAY 只能经专用 Meeting sender 提交；初版不对 Agent 的普通工具
  建立能力隔离，绕行禁令由 prompt 约定，所有来源的事件仍由 Relay 做协议校验；
- Progress、ACK、重试、过期和格式失败回退由 Harness 确定性处理，不需要 LLM；
- 晚于 Grant hard deadline 的结果必须丢弃。

## 14. 防循环与连贯性

除五次 Handoff 上限外，Agent 还应遵守：

- 自己的 speech 不为自己创建新的 Participant Intent；
- Floor、Offer、Grant、Progress 和 Revision 变化不单独触发语义 LLM；
- 同一 Intent 只能产生一次成功 speech；
- 新 speech 到达时，未开始的旧 Agent Intent Turn 合并到最新上下文；
- 已过时的模型结果不能提交 Intent、选择或 speech；
- Directed Handoff 的 reason 是目标 Granted Turn 的主要 basis；
- 连续 Agent 对话达到 Handoff 上限后必须回到主持人；
- 主持人可以提前 Recall，但不能打断当前已接受的 speech。

## 15. 共享日志

Meeting V1 继续区分两份共享记录：

### 15.1 Speech timeline

只包含成功消费 Grant 的 canonical kind `9` speech，按 speech revision 形成稳定顺序。

### 15.2 Floor control log

包含：

- SpeechIntent submit、refresh、withdraw、select 和 reject；
- Human Floor Request；
- Moderator Recall；
- Offer、ACK、Decline；
- Grant、Progress、Yield 和 Expiry；
- Directed Handoff、重新选择和带原因关闭；
- Relay 权威 Floor State。

所有控制事件都有签名作者、会议范围、逻辑对象 ID 和幂等语义。控制日志可供所有参会者
读取，但不作为正式发言展示。

## 16. 安全与活性不变量

Meeting V1 必须始终满足：

1. 任意时刻至多存在一个活动 Offer 或一个活动 Grant，二者不得共存；
2. Offer 与 Grant 都绑定唯一 target；
3. 每个 Grant 最多接受一条 speech；
4. 每条 speech 必须由 holder 签名并引用当前有效 Grant；
5. speech 接受与 Grant 消费原子提交；
6. End 是高于所有 Floor 状态的终态；
7. 非参会者不能读取 Intent、Handoff、控制日志或 speech；
8. 客户端不能自行签发 Relay Grant；
9. Human Request、Recall、Handoff depth 和选择优先级由 Relay 权威执行；
10. 没有任何 LLM 结果是 Relay 安全不变量的唯一依据；
11. Offer、soft lease、hard deadline 和 host decision deadline 都有持久恢复路径；
12. 重连、重启、ACK 丢失和事件重放不会产生双 Grant 或双 speech；
13. 单个 Agent 变慢只影响自己的 Intent、ModeratorPlan 或 Granted Turn；
14. 没有 Request 和 Intent 时，会议可以安静地保持 HOST_IDLE，不产生空轮循环。

## 17. 关键场景

概念协议至少覆盖以下场景：

1. 主持人创建会议并拥有初始 Control Token；
2. 主持人选择自己，获得 Grant 后发表开场；
3. 多名 Agent 在别人发言期间异步提交一句 SpeechIntent；
4. 主持人在取回 Control Token 前已经准备好下一次选择；
5. 主持人选择一个 Intent，目标 ACK 后获得唯一 Grant；
6. speaker 提问并携带 reason 把下一次发言机会交给明确对象；
7. Handoff target 拒绝或离线时，Offer 快速过期并回到主持人；
8. 连续五次成功 Handoff speech 后强制回到主持人，Yield/Expiry 不消耗接力深度；
9. 主持人提前 Recall，当前 speaker 完成后不能继续 Handoff；
10. holder 没有指定下一位，Control Token 自动回到主持人；
11. 主持人拒绝 off-topic 或 duplicate Intent，并向作者发送原因；
12. 同一参会者不能同时提交第二个 Intent，但可以刷新或撤回原 Intent；
13. Human 在 Agent 持有 Grant 时请求发言，当前 speech 完成后 Human 获得下一个 Offer；
14. 多个 Human Request 按 Relay 接收顺序处理；
15. Human 在未 ACK Offer 期间请求发言，可以抢占 Offer，但不能抢占已经创建的 Grant；
16. Moderator selection Offer 失败时原 SpeechIntent 仍为 pending，不会被静默消费；
17. 主持人想发言时高于普通 Intent，但低于 Human Request；
18. 主持 Agent 变慢或重启时，其他 speaker 不被中断，主持人 deadline 后有确定性兜底；
19. Relay、ACP 或客户端重启后恢复相同 Intent、Offer、Grant、Handoff depth 和优先级；
20. Handoff attempt 失败后问题仍 open，主持人可以显式重新选择或带原因关闭；
21. Meeting End 立即终止未完成 Intent、Request、Offer 和 Grant，并保留只读历史。

## 18. 与 Meeting V0 的关系

可以继续复用：

- MeetingSession、私有会议室和固定参会者名单；
- Meeting Create/End 与 Meeting Channel 的终态只读归档语义；
- kind `9` Grant-bound speech；
- Relay-signed Floor State 和单调 `floor_revision`；
- 单 Grant、单 speech、事务消费和幂等重试；
- 通用 Query、Count、WebSocket 和 outbox；
- ACP Meeting discovery、完整历史同步、私有 Agent Ledger；
- Harness 内部的专用 Meeting sender，以及 Meeting Turn 的 advisory 工具行为约定。

这里的“归档”特指 Meeting End 后的 Session/Channel 终态，不等于 NIP-IA identity
archive。后者只是身份可见性提示，不自动撤权或结束会议；只有 Community membership
removal、ban、account deactivation 或管理员紧急撤权等真正使 Auth 失效的安全事件，才
必须立即收回该身份的会议访问并耐久地结束受影响 Session。

需要替换或扩展：

- 用 SpeechIntent 池替代 Ready/Pass decision cohort；
- 用 Moderator Control、Human Request 和 Directed Handoff 替代统一随机 Claim；
- 增加 Offer/ACK，避免给离线目标直接创建长 Grant；
- 区分 `speech_revision` 与 `floor_revision`；
- 增加 soft lease、Progress 与 hard deadline；
- 增加 handoff depth、Moderator Recall 和主持人 deadline fallback；
- 扩展 Agent Controller 为 Participant、Moderator Planning 和 Granted Speech 三种职责。

Meeting V1 是对 V0 Floor 协议的版本化替换，不应在同一 Session 中同时运行
`uniform-v0` 和 `moderated-baton-v1` 两套权威调度策略。

“Meeting V1”是产品/协议代际名称。由于 V0 已经使用 wire schema `v=1`，实现时应为
Moderated Baton 使用新的 wire schema `v=2`，不能重新解释已经发布的
`v=1` 事件。

## 19. 实现设计决策索引

以下问题不改变本文概念语义，均已在
[Meeting V1 后端实现设计](meeting-v1-backend-implementation-design.md) 中冻结：

- 新增哪些事件 kind，哪些现有 kind 可以扩展复用；
- Intent、Offer、Grant、Handoff 和 ModeratorPlan 的数据库表结构；
- soft lease、Offer ACK、moderator decision deadline 和 Intent summary 的默认长度；
- Relay 如何版本化选择策略与 fallback；
- V0 Session 是否只读保留，新建 Session 是否默认使用 V1；
- ACP 如何调度 Participant Intent、ModeratorPlan 和 Granted Turn 的模型池优先级；
- Progress 允许哪些可观察阶段以及续租频率；
- Desktop 如何展示 Intent 池、Handoff 原因、Human Request 和 Moderator Recall；
- 兼容 fixture、迁移顺序、灰度开关和回滚方式。

这些属于实现方案和发布策略，不再是 Meeting V1 概念设计的阻塞项。
