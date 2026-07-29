# Meeting V1 后端实现设计

> 状态：后端实现设计 V1；阶段一、阶段二、阶段三已交付，阶段四待实施
>
> 前置概念设计：[Meeting V1：主持式发言权接力协议](./meeting-v1.md)
>
> 决策变更记录：[Meeting V1 Changelog](./changelog.md)
>
> 实现范围：Relay、数据库、协议事件、SDK、CLI、ACP Agent 和后端测试
>
> 暂不包含：Desktop、Web、Mobile、TTS、音视频及任何产品界面

## 1. 文档目的

本文把 Meeting V1 的概念语义落实为可分阶段开发的后端方案，回答以下问题：

- V1 如何与已经存在的 Meeting V0 共存；
- 发言意图、Offer、Grant、Human 请求、主持决策和 Handoff 使用哪些事件；
- `MeetingBatonState`、Offer 和 Grant 如何持久化；
- 每次状态迁移的事务边界是什么；
- 5 分钟 Grant、3 分钟主持决策等超时如何恢复；
- 普通 Agent 和主持 Agent 如何在不阻塞会议的前提下参与；
- 每个开发阶段交付什么，以及如何判定完成。

本文是实现约束，不重新讨论 [meeting-v1.md](./meeting-v1.md) 已确定的产品语义。若二者冲突，
应先修改概念设计，再调整实现。

## 2. 实现结论

V1 后端采用以下总体方案：

1. V1 使用新的 wire schema `v=2` 和固定策略名
   `moderated-baton-v1`；V0 保持 `v=1 + uniform-v0`。
2. 一个 MeetingSession 创建后永久绑定一种 Floor 策略，不支持会中切换，也不原地升级
   V0 Session。
3. V1 新建独立的 `meeting_baton` 数据库模块，不把新分支继续塞入 V0 的
   `meeting_floor.rs`。
4. 继续复用 kind `42100` Create、kind `42101` End、Relay 签名的 kind `42103`
   State，以及 Grant-bound kind `9` speech。
5. 为 Intent、Moderator Action、Human Request、Offer Response 和 Grant Signal
   分配独立的新事件 kind，避免再次形成一个语义过载的通用 Signal。
6. 每个会议仍以同一个私有 stream Channel 作为共享空间；所有参会者独立订阅同一个
   `#h=<session_id>` 范围，消息是广播扇出，不是竞争消费队列。
7. `MeetingBatonState` 是一场会议唯一的权威当前投影；Offer 和 Grant 使用 Relay
   预生成的独立稳定对象 ID，不借用可变 State event ID。
8. 每次写操作都锁定对应 `meeting_sessions` 行，在一个数据库事务中完成命令持久化、
   状态迁移、Relay State 事件和 outbox 写入。
9. Agent Offer ACK 是本地运行时的确定性容量确认，不调用 LLM。只有获得 Grant 后才生成
   完整发言。
10. 所有 deadline 使用数据库时间和持久时间戳；Relay sweeper 与任意后续命令的 lazy
    recovery 共同保证重启恢复。

## 3. 范围

### 3.1 本次后端范围

涉及以下模块：

- `buzz-core`：事件 kind、公共协议常量和基础校验；
- `buzz-sdk`：严格的 V1 event builder；
- `buzz-db`：迁移、投影、状态机、事务和恢复查询；
- `buzz-relay`：鉴权、命令处理、State 签名、outbox 投递和 deadline runtime；
- `buzz-cli`：Agent-first 和测试用的完整协议操作面；
- `buzz-acp`：普通参会 Agent、主持 Agent、确定性 ACK、Progress 和恢复账本；
- `buzz-dev-mcp`：Meeting Turn 可复用的会议历史与项目上下文工具；
- `buzz-test-client`：协议 fixture、Relay E2E、并发和恢复测试；
- `schema/schema.sql`：与迁移后的最终 schema 保持同步。

唯一允许触及客户端目录的机械改动，是把新增 kind 同步到
`desktop/src/shared/constants/kinds.ts` 与
`mobile/lib/shared/relay/nostr_models.dart`，以遵守仓库协议常量一致性；不接入查询、
状态管理或任何 UI。

### 3.2 明确不做

本轮不实现：

- Desktop、Web、Mobile 页面或状态管理；
- Offer targeting Human 的弹窗、输入框 heartbeat 等具体交互；
- 音频、视频、TTS；
- 议程、决议、表决、任务执行或项目写回；
- 动态增删参会者；
- 主持权转移、多主持人或主持人选举；
- V0 Session 到 V1 Session 的在线迁移；
- 在 Meeting Agent Turn 中执行会改变项目或外部系统的工具。

CLI 是协议验证面和 Agent 操作面，不属于本轮所说的产品前端。

## 4. 复用与替换

| 能力 | V1 处理 |
|---|---|
| 私有 Meeting Channel | 复用 |
| `session_id = channel UUID` | 复用 |
| 固定参会名单 | 复用 |
| Create / End 生命周期 | 扩展到 wire `v=2` |
| Channel 查询与 WebSocket 订阅鉴权 | 复用 |
| kind `9` 正式 speech | 扩展 V1 tags |
| Relay 签名 State | 复用 kind `42103`，使用 V1 schema |
| 单会议行锁 | 复用 |
| 事务 outbox 与 pub/sub fan-out | 复用 |
| ACP 历史同步、验签与私有账本 | 复用并升级账本版本 |
| Claim / Ready / Pass / cohort | V1 不使用 |
| 随机 winner selection | V1 不使用 |
| V0 round 状态机 | 保留，仅服务 V0 |
| V1 Baton 状态机 | 新模块实现 |

## 5. 协议版本与 Session 创建

### 5.1 版本绑定

Meeting Create kind `42100` 根据 `v` 选择协议：

```text
v=1  → floor_policy_version=uniform-v0
v=2  → floor_policy_version=moderated-baton-v1
```

Relay 必须先读取 Session 绑定的 `schema_version` 和 `floor_policy_version`，再把后续事件
分发到 V0 或 V1 handler。客户端不能通过后续事件的 tags 改变 Session 策略。

旧 V0 Session 的事件和仲裁结果不得因部署 V1 而变化。

隔离不能只依赖 Relay handler：

- Create 根据 `v` 只初始化对应状态机；
- End 和 kind `9` 先读取持久 policy，再调用 V0 或 V1 transaction；
- kind `42102/42104` 只允许 `schema=1 + uniform-v0`；
- kind `42105..42109` 只允许 `schema=2 + moderated-baton-v1`；
- V0 `recover_due_floors` 的 SQL 必须显式过滤
  `ms.floor_policy_version = 'uniform-v0'`，不能把没有 `meeting_rounds` 的 V1 Session 当成
  “缺失 V0 初始轮”；
- 每个 V0 DB public mutation 在取得 Session 锁后都再次 assert schema/policy；
- 每个 V1 DB public mutation也做对称 assert，不能信任上层已经正确路由。

这样内部调用、sweeper 或将来的 handler 回归都不能在同一 Session 旁路创建另一套状态。

### 5.2 V1 Create 事件

kind `42100` 的 V1 形状：

```text
required:
  ["h", "<session UUID>"]
  ["name", "<title>"]
  ["v", "2"]
  ["policy", "moderated-baton-v1"]
  ["moderator", "<pubkey>"]
  one or more ["p", "<participant pubkey>"]

optional:
  ["about", "<description>"]
  ["source", "<source channel UUID>"]
```

规则：

- 事件作者仍是 Meeting 创建者和 backing Channel Owner；
- `p` tags 只列事件作者之外的参与者；Relay 把作者加入完整 roster，作者不能在 `p`
  中重复；
- `moderator` 必须出现在完整固定名单中；
- 未显式提供 moderator 的 SDK/CLI 可以默认使用事件作者，但最终签名事件中必须包含该 tag；
- 创建者、Channel Owner 和 Moderator 是不同职责；
- Relay 根据权威用户目录判定每名参会者是 `human` 还是 `agent`，并在创建事务中冻结；
- V1 类型判定必须 fail closed：权威 identity resolver 必须为每名参会者返回明确
  `human|agent`；不能沿用 V0 “缺少 users 行就默认 Human”的 fallback。身份投影缺失或
  Agent registry 与 users 数据冲突时拒绝 Create；
- participant type 不能从 Channel 的 `owner/member/bot` role 临时推断；
- V1 继续使用现有最多 12 名参与者、最多 4 个受管 Agent 的限制，除非后续单独调整。

### 5.3 初始状态

创建事务完成后：

- `speech_revision = 0`；
- `intent_revision = 0`；
- `floor_revision = 1`；
- `state_revision = 1`；
- `control_epoch = 1`；
- `decision_epoch = 0`（尚未进入一次有 deadline 的 moderator decision window）；
- `handoff_depth = 0`；
- `forced_return_to_moderator = false`；
- phase 为 `moderator_idle`；
- Control Token 属于 moderator；
- 没有活动 Offer 或 Grant。

`moderator_idle` 对应概念设计中的 `HOST_IDLE`，不是“没有主持人”，而是“Control Token
在主持人处，当前没有必须立即处理的
工作”。新的 Intent、Human Request 或主持人自己的 Intent 会唤醒状态机。

实现中的 wire/schema 名称统一使用 `moderator_*`；`host_pubkey` 只表示 V0 已有的
创建者/Channel Owner，不能用来判断主持权限。

## 6. 事件模型

### 6.1 kind 分配

| kind | 名称 | 签名者 | 用途 |
|---:|---|---|---|
| `9` | Meeting Speech | 当前 Grant holder | 唯一正式发言，可原子携带 Handoff |
| `42100` | Meeting Create | 创建者 | 创建 V1 Session、名单和 moderator |
| `42101` | Meeting End | 有权结束会议者 | 终止 Session |
| `42103` | Meeting State | Relay | 权威 Baton 投影 |
| `42105` | Speech Intent Command | 参会者 | submit、refresh、withdraw |
| `42106` | Moderator Command | moderator | select、reject、dismiss-handoff、recall |
| `42107` | Human Floor Request | Human 参会者 | request、withdraw |
| `42108` | Offer Response | Offer target | ack、decline |
| `42109` | Grant Signal | Grant holder | progress、yield |

kind `42102` 和 `42104` 只保留 V0 语义。V1 不重新解释 Claim、Ready、Pass 或 V0 Yield。

`42103` 的现有 Rust 常量可以保留兼容别名，但公共文档和新代码应使用更通用的
`Meeting State` 命名，而不是把 V1 称为 Round State。

### 6.2 通用规则

所有 V1 控制事件：

- 必须有且只有一个 `h` tag；
- 必须有 `["v", "2"]`；
- 必须由该 kind 允许的固定参会者签名；Meeting End 额外允许 Community owner/admin，
  安全撤权 End 只允许 Relay/system key；
- 使用严格 tag vocabulary，未知控制 tag 直接拒绝；
- 逻辑对象使用稳定 ID，重试必须重放完全相同的已签名事件；
- 不接受客户端提供的 deadline、revision 或 participant type 作为权威值；
- reason、summary 等文本执行 UTF-8、长度、首尾空白和控制字符校验。

Wire 时间统一使用 JSON integer Unix epoch milliseconds，字段名以 `_ms` 结尾，例如
`ack_deadline_ms`、`hard_deadline_ms`。Nostr envelope 的 `created_at` 仍按标准使用 Unix
seconds，但不参与协议 deadline 判定。

除 Create、End 和 State 的完整示例外，下文命令示例只列该 action 的特有 tags；实际
builder/fixture 必须再带本节要求的唯一 `h` 和 `v=2` tags。

首版文本上限：

| 字段 | 上限 |
|---|---:|
| Intent summary | 512 bytes |
| selection reason | 512 bytes |
| rejection/defer reason | 1024 bytes |
| Offer decline reason | 512 bytes |
| Yield reason | 512 bytes |
| Handoff reason | 1024 bytes |
| speech content | 沿用现有 256 KiB |

### 6.3 V1 End：kind `42101`

Manual End 使用独立的 V1 builder：

```text
tags:
  ["h", "<session>"]
  ["v", "2"]
  ["policy", "moderated-baton-v1"]
  ["e", "<V1 Create event id>"]
  ["reason", "manual"]

content:
  empty
```

正常情况下只有 Meeting Owner 可以签名；Community owner/admin 可以用相同
`reason=manual` 形状执行恢复性结束，即使其不在固定名单中。DB 必须重新验证 Community
role，不能让通用“非参会者”拒绝提前遮蔽这一恢复路径。

身份撤权终止由 Relay 签名：

```text
tags:
  ["h", "<session>"]
  ["v", "2"]
  ["policy", "moderated-baton-v1"]
  ["e", "<V1 Create event id>"]
  ["reason", "participant_revoked"]
  ["p", "<revoked participant pubkey>"]

content:
  empty
```

现有 V0 End builder 和 wire 形状保持不变；handler 读取持久 Session policy 后选择严格
validator。V1 不能接受缺少 `v=2/policy` 的 End，V0 也不能接受 V1 形状。

### 6.4 SpeechIntent：kind `42105`

#### Submit

```text
tags:
  ["h", "<session>"]
  ["v", "2"]
  ["action", "submit"]
  ["basis-speech-revision", "<n>"]
  optional ["addressed-to", "<pubkey>"]

content:
  一句简短 summary

intent_id:
  本次 Submit 事件 ID
```

#### Refresh

```text
tags:
  ["h", "<session>"]
  ["v", "2"]
  ["action", "refresh"]
  ["intent", "<intent_id>"]
  ["prev", "<current intent event id>"]
  ["basis-speech-revision", "<n>"]
  optional ["addressed-to", "<pubkey>"]

content:
  新 summary
```

#### Withdraw

```text
tags:
  ["h", "<session>"]
  ["v", "2"]
  ["action", "withdraw"]
  ["intent", "<intent_id>"]
  ["prev", "<current intent event id>"]

content:
  empty
```

`prev` 提供 compare-and-swap 语义，防止离线客户端用旧 refresh 覆盖新版本。

默认限制：

- summary 为 `1..=512` bytes；
- `0 <= basis-speech-revision <= current speech_revision`；旧 basis 可以进入池并由
  moderator 判断是否 stale，未来 revision 一律拒绝；
- 同一作者最多一个 `pending` Intent；
- `addressed-to` 如果存在，必须是固定参会者且不能是作者自己；
- Offer 失败时 Intent 仍为 pending；
- Moderator selection 的 Offer ACK 后，Intent 进入 selected；
- Grant 成功发言后进入 consumed；
- Grant Yield、过期或 Meeting End 前未发言时进入 stale/ended；
- Human Request 或 Directed Handoff 产生的 Grant 不自动消费目标无关的 pending Intent。

最后一条避免一个定向问题静默删除目标准备讨论的另一项议题。

### 6.5 Moderator Command：kind `42106`

#### Select

```text
tags:
  ["action", "select"]
  exactly one of:
    ["intent", "<pending intent_id>"]
    ["handoff", "<open handoff_id>"]
  ["expected-control-epoch", "<n>"]
  ["expected-decision-epoch", "<n>"]
  ["expected-intent-revision", "<n>"]
  ["expected-speech-revision", "<n>"]
  when selecting a handoff:
    ["expected-handoff-attempt-count", "<n>"]

content:
  {
    "selection_reason": "<optional>",
    "deferrals": [
      {
        "intent_id": "<other pending intent>",
        "prev": "<current intent event id>",
        "reason": "<required>"
      }
    ]
  }
```

选择主持人自己的 Intent、其他 Intent 与一个尚未回答的 Handoff 使用同一操作：

- Intent source：Relay 从 Intent 作者得出 Offer target；
- Handoff source：Relay 从持久 Handoff 得出 target、source speech 和原始 reason；
- Handoff 必须仍是 `question_state=open`，一次 Offer/Grant 尝试的失败不等于问题已关闭；
- Handoff Select 的 expected attempt count 必须等于当前持久值，防止旧计划在一次失败后
  无意重放；
- 客户端不能重复声明或修改 target；
- Handoff source 的 `source_intent_id=null`，不会消费目标无关 Intent；
- Handoff source 是 moderator 重新调度，目标 Grant 从 handoff depth 0 开始；
- 只有 Intent source 指向 moderator 自己时，`turn_role=moderator_self`；
- `deferrals` 是必填数组；只有 moderator self Intent Select 可以非空，其他 Select
  必须传空数组。

Select 只在 phase 为 `moderator_idle | moderator_control`、即 moderator 持有 Control
Token 时有效，并且不能越过已排队的 Human Request。Open Handoff 是唯一待处理对象时
phase 仍为 `moderator_idle`，该状态必须允许显式 Select。

Relay 还要执行主持人自己的发言优先级：

- moderator 有 pending self Intent 时，不能直接选择其他普通 Intent 或 open Handoff；
- moderator 可以先撤回自己的 Intent，再选择其他人；
- moderator 连续完成一次 self speech 后，如果还有其他有效 Intent，不能立即再次选择新的
  self Intent；
- moderator 必须先选择、拒绝，或在本次 self Select 中明确列出需要延后的其他 Intent。

Deferral 不是一个可独立悬挂的操作，只能作为 self Select 的原子附属项：

- Relay 要求所有阻止连续 self speech 的有效 Intent 都已被 Reject，或出现在
  `deferrals` 中；
- 每项 Deferral 都绑定本次 moderator-self Offer；
- Offer 被抢占、Recall、Decline 或 timeout 时立即解除；
- ACK 后绑定对应 Grant，Grant SAY、Yield、Expiry 或 End 时立即解除；
- 因此 moderator 在 Select 后崩溃，也不会留下永久不可选择的 Intent。

#### Reject

```text
tags:
  ["action", "reject"]
  ["intent", "<pending intent_id>"]
  ["prev", "<current intent event id>"]
  ["reason-code", "<enum>"]
  ["p", "<intent author pubkey>"]

content:
  必填 reason_text
```

Reject 可以在其他人持有 Offer 或 Grant 时异步执行。默认 reason code 使用概念设计中的
`off_topic`、`duplicate`、`superseded`、`unsupported`、`agenda_mismatch` 和
`meeting_ended`。其中 `meeting_ended` 只用于 Relay 在 End transition 中映射未处理
Intent；moderator 不能在已结束会议上提交 Reject。`p` 是给订阅者的通知索引，Relay
必须验证它恰好等于持久 Intent 的 author；不匹配时拒绝整条命令。

#### Dismiss Handoff

```text
tags:
  ["action", "dismiss-handoff"]
  ["handoff", "<open handoff_id>"]
  ["expected-speech-revision", "<n>"]
  ["expected-handoff-attempt-count", "<n>"]
  ["reason-code", "superseded" | "answered_elsewhere" | "out_of_scope" |
                  "no_longer_needed"]

content:
  必填 reason_text
```

Moderator 可以异步关闭一个不再需要处理的 open question，但该 Handoff 不能正被活动
Offer 或 Grant 引用。操作把 `question_state` 改为 `dismissed`，递增
`floor_revision/state_revision` 并发布原因；它不改变目标已有的 SpeechIntent。若当前
attempt 仍活动，moderator 应先 Recall 或等待其终结，再提交 Dismiss。Speech revision
和 attempt count 都必须匹配，避免旧清理计划关闭刚刚重试过的问题。

#### Recall

```text
tags:
  ["action", "recall"]
  ["control-epoch", "<n>"]

content:
  optional reason
```

Recall 的 Relay 行为：

- `allocation_source != human_request` 的 Offer 尚未 ACK：取消并准备回到 moderator；
- human_request-sourced Offer：不取消，但锁存 `forced_return_to_moderator`；Human 队列
  结束后必须归还；
- Grant 已存在：仅锁存 `forced_return_to_moderator`；
- 已在 `moderator_idle` 或 `moderator_control`：作为幂等无操作或返回当前状态。

`control_epoch` 在 Control Token 每次真正回到 moderator 时加一，在同一段 Offer/Grant/
Handoff/Human 链中保持稳定，因此 Recall 不会与每 10 秒一次的 Progress revision 竞速。
迟到 Recall 若引用已经完成的 epoch，Relay 返回“已归还”的幂等结果；不能把它施加到
一个无关的新 control epoch。

### 6.6 Human Floor Request：kind `42107`

#### Request

```text
tags:
  ["action", "request"]

content:
  empty

request_id:
  本次 Request 事件 ID
```

#### Withdraw

```text
tags:
  ["action", "withdraw"]
  ["request", "<request_id>"]

content:
  empty
```

只有冻结类型为 Human、且不是 moderator 的参会者可以提交。Human moderator 仍通过
self Intent 和 moderator Select 发言，不能用 Human Request 绕过“低于其他 Human”与连续
self speech 规则。每名 eligible Human 同时最多一个 active request。
Relay 使用数据库生成的单调 `queue_position` 决定 FIFO，不依赖客户端时间。

Human Request 到达后：

- `moderator_idle` 或 `moderator_control`：直接创建最早 Human 的 Offer；
- 活动的 non-human_request-sourced Offer 尚未 ACK：原子抢占并创建
  human_request-sourced Offer；
- 活动 human_request-sourced Offer：加入队列，不抢占更早 Human；
- 活动 Grant：加入下一席队列，不撤销 Grant。

### 6.7 Offer Response：kind `42108`

```text
tags:
  ["action", "ack" | "decline"]
  ["meeting-offer", "<offer_id>"]

content:
  ACK 为空；Decline 可携带简短原因
```

只有当前 Offer target 可以响应。ACK 表示运行实例已经在线、预留本地容量并愿意承担本次
Granted Turn，不表示模型已经组织好内容。

Agent ACK 不调用 LLM。Harness 必须先原子预留本地 turn slot，再发送 ACK。

### 6.8 Grant Signal：kind `42109`

#### Progress

```text
tags:
  ["action", "progress"]
  ["meeting-grant", "<grant_id>"]
  ["progress-seq", "<positive integer>"]
  ["stage", "context_sync" | "tool_use" | "generating" | "composing" |
            "submitting"]

content:
  empty
```

Progress 由 Harness 或 Human 客户端根据可观察运行阶段确定性发送，不由 LLM 决定。
Human 输入期间使用 `composing`。
`progress-seq` 对同一 Grant 严格单调。每次接受都会把 soft lease 延长到：

```text
min(database_now + soft_lease, hard_deadline)
```

由于 Progress 改变了活动 Grant 的租约状态，实现上它会递增 `floor_revision`，但不会改变
`grant_id`。

#### Yield

```text
tags:
  ["action", "yield"]
  ["meeting-grant", "<grant_id>"]
  optional ["reason-code", "no_longer_needed" | "unable_to_answer" |
                           "insufficient_context" | "tool_failure" |
                           "cancelled"]

content:
  optional short reason
```

Yield 立即结束 Grant，不等待任何 lease。

### 6.9 正式 speech：kind `9`

V1 speech 必须包含：

```text
["h", "<session>"]
["v", "2"]
["meeting-grant", "<grant_id>"]
["speech-revision", "<grant speech_revision + 1>"]
zero or more ["p", "<mentioned participant pubkey>"]
```

可选 Directed Handoff 必须全部出现或全部不出现：

```text
["handoff-to", "<participant pubkey>"]
["handoff-type", "question" | "information_request" | "clarification" |
                  "review" | "response_requested"]
["handoff-reason", "<required reason text>"]
```

规则：

- author 必须是当前 Grant holder；
- `speech-revision` 必须正好是下一 revision；
- content 非空并沿用现有消息大小上限；
- Handoff target 必须是另一名固定参会者；
- 每条 speech 最多一个 Handoff；
- Handoff 与 speech 在同一事务接受或拒绝；
- 只有 Grant `turn_role=moderator_self` 时，moderator 的 Handoff 才视为主持调度并从深度
  0 开始；
- moderator 若因 Human Request、Directed Handoff 或 fallback 获得 Grant，本次身份是
  ordinary speaker，其成功 Handoff 同样受当前深度和 Session 冻结的接力上限约束；
- ordinary speaker 的 Handoff Offer ACK 后，Grant 暂占
  `previous_handoff_depth + 1`；目标 canonical speech 才提交该深度，Yield/Expiry 必须
  恢复 `previous_handoff_depth`。

## 7. Relay 权威 State

### 7.1 `MeetingBatonState` 职责

`MeetingBatonState` 是“现在发生什么”的唯一持久投影，负责：

- 当前 phase；
- moderator；
- 三种 revision；
- State 总序和 control epoch；
- 当前 Offer 或 Grant 的引用；
- 当前尚未回答的 Directed Handoff 集合；
- 当前 `handoff_depth`；
- moderator 连续发言计数；
- Recall/强制归还锁存；
- 主持决策 deadline；
- 下一次需要 runtime 处理的时间。

它不替代：

- SpeechIntent 历史；
- Human Request FIFO；
- Offer/Grant 尝试历史；
- Directed Handoff 的完整历史与各次 Offer/Grant 尝试；
- 正式 speech timeline。

### 7.2 phase

V1 使用：

```text
moderator_idle
moderator_control
offered
granted
ended
```

含义：

- `moderator_idle`：Control Token 在 moderator，没有通过全部公平 gate 的 deterministic
  fallback candidate；可以仍有已尝试但 pending 的 Intent、被连续 self gate 阻止的
  self Intent 和仅供主持人判断的 open Handoff；
- `moderator_control`：Control Token 在 moderator，存在至少一个 deterministic fallback
  candidate 和 decision deadline；
- `offered`：唯一 Offer 等待 target ACK/Decline；
- `granted`：唯一 Grant 等待 Progress、SAY、Yield 或 expiry；
- `ended`：终态。

`offered` 与 `granted` 永不共存。

### 7.3 Revision

- `speech_revision`：只在 canonical speech 被接受时加一；
- `intent_revision`：Intent 或 Human Request 池的权威状态变化时加一；
- `floor_revision`：phase、Offer、Grant、Recall、Progress、Yield、Expiry、Directed
  Handoff 控制投影或 Control Token 变化时加一。

所有 revision 都由 Relay 在会议行锁内分配，永不回退。

一份原子 transition/State 对每个受影响的 domain revision **最多加一**，不按
`effects[]` 数量或受影响行数累加；`state_revision` 则每份 State 必加一。例如：

| transition | `speech_revision` | `intent_revision` | `floor_revision` |
|---|---:|---:|---:|
| self Select + N 个 Deferral | `+0` | `+1` | `+1` |
| ACK source Intent 并创建 Grant | `+0` | `+1` | `+1` |
| SAY + consume Intent + release Deferral + answer/create Handoff | `+1` | 有 Intent/Deferral 变化时 `+1` | `+1` |
| End 批量终结活动对象 | `+0` | Intent/Request 池变化时 `+1` | `+1` |

若同一数据库事务先做 recovery、再接受新 command，它们是两份 State，因此各自按上表独立
递增。

此外实现增加三个不改变概念语义的协调序号：

- `state_revision`：每生成一份 Relay State 都加一，用于订阅/回填总排序；
- `control_epoch`：每次 Control Token 真正回到 moderator 时加一；同一直接接力链、
  Human 队列和 Progress 期间保持不变。
- `decision_epoch`：每次从没有活动 decision window 的状态进入
  `moderator_control` 时加一；它与 Control Token 的归属序号独立。

Intent 在 Grant 期间发生变化时，只增加 `intent_revision`，不会改变 `grant_id`。Grant
校验永远引用稳定 `grant_id`，不引用最新 State event ID。

`meeting_baton_state.handoff_depth` 在 `depth_mode=increment_provisional` Grant 活动
期间可以是暂占值。Grant 同时携带 `previous_handoff_depth`：canonical speech 提交暂占
值；Yield、soft expiry 或 hard expiry 恢复旧值。Offer 失败从未改变深度。

### 7.4 State 事件：kind `42103`

每次至少有一种 revision 变化时，在同一事务中生成新的 Relay-signed State：

```text
tags:
  ["h", "<session>"]
  ["v", "2"]
  ["policy", "moderated-baton-v1"]
  ["phase", "<phase>"]
  ["floor-revision", "<n>"]
  ["intent-revision", "<n>"]
  ["speech-revision", "<n>"]
  ["state-revision", "<n>"]
  ["moderator", "<pubkey>"]
  optional ["p", "<active Offer or Grant target>"]
```

content 是完整、可恢复的会议内权威投影：

```json
{
  "phase": "granted",
  "state_revision": 18,
  "floor_revision": 12,
  "intent_revision": 7,
  "speech_revision": 4,
  "control_epoch": 3,
  "decision_epoch": 5,
  "baton_config": {
    "timing_profile_version": "moderated-baton-v1-default",
    "agent_offer_ack_ms": 5000,
    "human_offer_ack_ms": 15000,
    "moderator_decision_ms": 180000,
    "grant_soft_lease_ms": 30000,
    "progress_interval_ms": 10000,
    "grant_hard_deadline_ms": 300000,
    "agent_safety_margin_ms": 30000,
    "max_handoff_depth": 5,
    "max_open_handoffs": 32,
    "fallback_policy_version": "fallback-v1"
  },
  "moderator_pubkey": "<hex>",
  "participants": [
    {
      "pubkey": "<hex>",
      "participant_type": "human",
      "channel_role": "owner"
    }
  ],
  "pending_intents": [
    {
      "intent_id": "<event id>",
      "current_event_id": "<event id>",
      "author_pubkey": "<hex>",
      "basis_speech_revision": 4,
      "summary": "<one sentence>",
      "addressed_to": null,
      "created_at_ms": 1785230900000,
      "deferred": false,
      "selection_attempt_count": 1,
      "last_offer_id": "<object id>",
      "last_attempt_outcome": "declined"
    }
  ],
  "human_queue": [
    {
      "request_id": "<event id>",
      "requester_pubkey": "<hex>",
      "queue_position": 27,
      "state": "queued"
    }
  ],
  "unresolved_handoffs": [
    {
      "handoff_id": "<handoff source speech event id>",
      "source_speech_event_id": "<handoff source speech event id>",
      "from_pubkey": "<hex>",
      "to_pubkey": "<hex>",
      "reason_type": "question",
      "reason_text": "<why this participant should answer>",
      "created_at_ms": 1785230890000,
      "question_state": "open",
      "attempt_count": 2,
      "last_offer_id": "<source offer object id>",
      "last_grant_id": "<active grant object id>",
      "last_attempt_outcome": "granted",
      "blocked_by": null
    }
  ],
  "handoff_depth": 2,
  "consecutive_moderator_speeches": 0,
  "forced_return_to_moderator": false,
  "moderator_decision_deadline_ms": null,
  "next_action_at_ms": 1785230990000,
  "offer": null,
  "grant": {
    "grant_id": "<active grant object id>",
    "holder_pubkey": "<hex>",
    "allocation_source": "directed_handoff",
    "turn_role": "participant",
    "source_offer_id": "<source offer object id>",
    "allocation_event_id": "<handoff source speech event id>",
    "selection_reason": null,
    "source_intent_id": null,
    "source_request_id": null,
    "source_handoff_id": "<handoff source speech event id>",
    "source_speech_event_id": "<handoff source speech event id>",
    "depth_mode": "increment_provisional",
    "previous_handoff_depth": 1,
    "handoff_depth": 2,
    "handoff_context": {
      "from_pubkey": "<hex>",
      "reason_type": "question",
      "reason_text": "<why this participant should answer>"
    },
    "basis_speech_revision": 4,
    "created_at_ms": 1785230900000,
    "soft_lease_expires_at_ms": 1785230990000,
    "hard_deadline_ms": 1785231200000,
    "progress_seq": 3
  },
  "transition": {
    "primary_type": "grant_progressed",
    "outcome": "accepted",
    "primary_object_id": "<active grant object id>",
    "caused_by_event_id": "<progress event id>",
    "deadline_type": null,
    "blocked_by": null,
    "at_ms": 1785230960000,
    "effects": [
      {
        "type": "grant_progressed",
        "object_type": "grant",
        "object_id": "<active grant object id>",
        "from": "active",
        "to": "active"
      }
    ]
  }
}
```

在 `phase=offered` 时，`grant` 必须为 null，`offer` 使用以下完整形状：

```json
{
  "offer_id": "<object id>",
  "target_pubkey": "<hex>",
  "target_participant_type": "agent",
  "allocation_source": "directed_handoff",
  "turn_role": "participant",
  "allocation_event_id": "<source speech event id>",
  "selection_reason": null,
  "source_intent_id": null,
  "source_request_id": null,
  "source_handoff_id": "<source speech event id>",
  "source_speech_event_id": "<event id>",
  "basis_speech_revision": 4,
  "depth_mode": "increment_provisional",
  "previous_handoff_depth": 1,
  "requested_handoff_depth": 2,
  "handoff_context": {
    "from_pubkey": "<hex>",
    "reason_type": "question",
    "reason_text": "<why this participant should answer>"
  },
  "created_at_ms": 1785230950000,
  "ack_deadline_ms": 1785230955000
}
```

`handoff_context` 仅在 `source_handoff_id` 非空时出现。这样 State 本身就是 Offer/Grant 的
权威 transport：目标无需先访问 Relay 内部投影，就能看到发起者、原始 speech、原因和
deadline。Intent/Human source 分别通过 `source_intent_id/source_request_id` 指回共享
控制事件；target、source 与 reason 只能由 Relay 从持久对象派生。

全量 State 数组必须 canonical 排序：`participants` 按 pubkey bytes；
`pending_intents` 按 `(created_at_ms, intent_id)`；`human_queue` 按
`queue_position`；`unresolved_handoffs` 按 `(created_at_ms, handoff_id)`；
`transition.effects` 使用下文定义的 effect 顺序。同一逻辑投影不能因 SQL 未写
`ORDER BY` 而在 Progress State 中随机重排。ModeratorPlan 的 fingerprint 按对象 ID 与
版本字段比较键值集合，不按 JSON 数组位置或原始序列化 bytes 判断 stale。

Relay 在事务内先用密码学安全随机源生成 32-byte `offer_id` 或 `grant_id`，再把 64 位
hex 编码写入 State、数据库和后续参与者事件。对象 ID 不是 Nostr event ID；State 自己的
`event.id` 只标识这份快照。这样首次 State 可以完整携带对象 ID，也不存在 event hash
自引用。

每个 State 必须有一个结构化 `transition`。`primary_type` 表示触发这次状态迁移的主结果，
有序 `effects[]` 完整列出该原子迁移改变的所有 Intent、Request、Offer、Grant、Handoff
和 phase。没有 participant command 的 timeout、fallback、Handoff attempt failure、
forced return 和撤权终止也使用相同形状给出对象、结果、原因和发生时间。若一个事务依次
完成 deadline recovery 和一个仍有效的新命令，应按实际顺序生成两份 State/transition，
并依靠 outbox sequence 投递。

首版 transition type 至少包括：

```text
meeting_created
intent_submitted | intent_refreshed | intent_withdrawn |
intent_rejected | intent_deferred | intent_reactivated
human_requested | human_withdrawn
offer_created | offer_acked | offer_declined | offer_timed_out |
offer_preempted | offer_recalled | offer_source_changed
grant_created | grant_progressed | grant_yielded |
grant_soft_expired | grant_hard_expired
speech_accepted
handoff_created | handoff_attempt_failed | handoff_answered | handoff_dismissed |
handoff_open_limit_blocked
recall_latched | forced_return_completed
moderator_fallback
meeting_ended | participant_revoked
```

`effects[].type` 使用下面的闭合枚举；`object_type/object_id` 必须与对应行一致：

| object type | effect type |
|---|---|
| `meeting` | `meeting_created`、`meeting_ended`、`participant_revoked` |
| `phase` | `phase_changed` |
| `intent` | `intent_submitted`、`intent_refreshed`、`intent_attempted`、`intent_attempt_failed`、`intent_selected`、`intent_deferred`、`intent_reactivated`、`intent_rejected`、`intent_withdrawn`、`intent_consumed`、`intent_stale`、`intent_ended` |
| `human_request` | `human_requested`、`human_offered`、`human_granted`、`human_withdrawn`、`human_declined`、`human_timed_out`、`human_ended` |
| `offer` | `offer_created`、`offer_acked`、`offer_declined`、`offer_timed_out`、`offer_preempted`、`offer_recalled`、`offer_source_changed`、`offer_source_withdrawn`、`offer_ended` |
| `grant` | `grant_created`、`grant_progressed`、`grant_spoken`、`grant_yielded`、`grant_soft_expired`、`grant_hard_expired`、`grant_ended` |
| `speech` | `speech_accepted` |
| `handoff` | `handoff_created`、`handoff_attempted`、`handoff_attempt_failed`、`handoff_answered`、`handoff_dismissed`、`handoff_open_limit_blocked`、`handoff_ended` |
| `recall` | `recall_latched`、`recall_cleared` |
| `control` | `forced_return_latched`、`forced_return_completed`、`control_returned`、`fallback_attempt_recorded` |

每个 effect 都有 `object_type/object_id/type`；有状态机的对象还必须给 `from/to`，新建对象
的 `from=null`，没有状态变化的 Progress 可以 `from=to=active`。`outcome_code`、
`reason_code`、`blocked_by` 只在对应 effect 合法时出现。`phase/control/meeting` 的
`object_id` 使用 session UUID；其他对象使用各自稳定 ID。

同一 transition 内 effects 的 canonical 顺序为：触发命令对象、被终结的活动
Offer/Grant、canonical speech、Intent/Request/Handoff 投影、创建的新 Offer/Grant、
Recall/Control、最后 `phase_changed`。同类多个对象按 bytewise object ID 排序。SDK、
fixture 与 State builder 都使用这个顺序。

Intent 与 Handoff 的 `last_attempt_outcome` 使用同一闭合枚举：

```text
offered | granted |
declined | timed_out | preempted | recalled |
source_changed | source_withdrawn |
spoken | yielded | soft_expired | hard_expired | ended
```

创建 Offer 时写 `offered`，ACK 后写 `granted`，后续终态映射到同名 outcome。尚无 Offer
attempt 的 open Handoff 为 null；`blocked_by` 单独使用
`human_request | recall | max_depth | open_question_limit`，不能塞入 attempt outcome。

`primary_type` 的确定规则固定：ACK 使用 `offer_acked`，SAY 使用 `speech_accepted`，
Human 抢占使用 `human_requested`，End/撤权使用 `meeting_ended/participant_revoked`，
deadline 使用对应 `*_expired/*_timed_out`。例如 SAY 可以在同一 State 的
`effects[]` 中依次包含 Grant spoken、Intent consumed、旧 Handoff answered、新 Handoff
created 和下一 Offer created；不得为了这些 effects 人为生成多份中间 State。
`caused_by_event_id` 对 participant/moderator command 必填；deadline 或内部 fallback
使用 null，并填写 `deadline_type`/`outcome`。

其余 command-to-primary 映射同样固定：

| 触发 | `primary_type` |
|---|---|
| Create | `meeting_created` |
| Intent submit/refresh/withdraw | `intent_submitted/refreshed/withdrawn` |
| Moderator Select/Reject/Dismiss | `offer_created/intent_rejected/handoff_dismissed` |
| Recall 取消 Offer / 仅锁存 | `offer_recalled/recall_latched` |
| Human request/withdraw | `human_requested/human_withdrawn` |
| Offer ACK/Decline | `offer_acked/offer_declined` |
| Grant Progress/Yield | `grant_progressed/grant_yielded` |
| deterministic fallback | `moderator_fallback` |
| Offer/Grant deadline | `offer_timed_out/grant_soft_expired/grant_hard_expired` |

`state_revision` 在每一份 Relay State 上加一，是客户端选择最新快照的唯一总序。三种
domain revision 继续表达 speech/intent/floor 各自是否变化，但不再承担事件总排序。
同一 Session 的 `state_revision` 只能对应一个 canonical Relay State。

## 8. 数据库设计

阶段一使用 `0029_meeting_v1_baton.sql`，阶段二使用
`0030_meeting_v1_stage2.sql`。迁移保持 additive，不删除 V0 数据和约束语义。

下文子表字段清单为主要字段；除非特别说明，每张表都包含
`community_id, session_id`，并以复合外键指向 `meeting_sessions`。pubkey、event ID 和
Relay 生成的 Offer/Grant object ID 都使用 32-byte `BYTEA` 与长度 CHECK。活动
Intent/Request/Offer/Grant 使用 partial unique index 约束各自唯一槽位。

### 8.1 `meeting_sessions` 扩展

新增或调整：

- `schema_version` 允许 `1 | 2`；
- `floor_policy_version` 允许 `uniform-v0 | moderated-baton-v1`；
- `moderator_pubkey BYTEA NULL`，V1 必填；
- `security_order BIGINT` 从全局 `meeting_security_order_seq` 分配；
- 原有 `host_pubkey` 保持“创建者/Channel Owner”语义；
- shape constraint 保证 V0 行没有被错误解释为 V1。

V0 的 `current_round` 和 `floor_revision` 继续保留，但只由 `uniform-v0` 读取。V1 不以
round 作为调度主键，也不把三种 V1 revision 镜像到 `meeting_sessions`；
`meeting_baton_state` 是 V1 current revision 的唯一数据库权威，避免跨表漂移。

`security_order` 不是业务 revision，而是 Meeting Create 与安全撤权共享的数据库因果序。
Create 和撤权事务先通过同一组身份行锁线性化，再分别为 Session 或 revocation job 取得
下一个序号。它避免使用事务开始时间或同一微秒的 wall-clock 时间猜测“旧会议/新会议”。

### 8.2 `meeting_participants`

V1 新增冻结名单投影：

```text
community_id
session_id
pubkey
participant_type: human | agent
channel_role
created_at
```

主键为 `(community_id, session_id, pubkey)`。它用于：

- Human Floor Request 鉴权；
- Agent ACK deadline 选择；
- ACP 自动接入；
- 在 Agent moderator 同时是 Channel Owner 时仍准确识别类型。

### 8.3 `meeting_baton_config`

每个 V1 Session 一行不可变配置快照：

```text
community_id, session_id
timing_profile_version
agent_offer_ack_ms
human_offer_ack_ms
moderator_decision_ms
grant_soft_lease_ms
progress_interval_ms
grant_hard_deadline_ms
agent_safety_margin_ms
max_handoff_depth
max_open_handoffs
fallback_policy_version
created_at
```

Create 时把经过校验的 Relay 默认值写入此表；首版
`max_open_handoffs=32`。后续创建 Offer、Grant、decision deadline、Handoff 和 fallback
时只读这个快照，不重新读取进程环境变量。这样 Relay 重启、滚动升级或配置变化都不会
改变一场既有会议的协议时间和容量边界。Relay State 携带这个配置的完整公开快照，
CLI/ACP 不需要猜测进程默认值。

数据库约束固定 `max_open_handoffs BETWEEN 1 AND 32`；32 也是首版协议硬上限，不允许
环境配置继续放大。结合最多 1024 bytes 的 Handoff reason，完整 State 在签名前仍要执行
现有 event content 大小检查。

### 8.4 `meeting_baton_state`

每个 V1 Session 一行，主要字段：

```text
community_id, session_id
phase
floor_revision, intent_revision, speech_revision
state_revision, control_epoch, decision_epoch
state_event_id
active_offer_id
active_grant_id
handoff_depth
consecutive_moderator_speeches
forced_return_to_moderator
recall_event_id
moderator_decision_started_at
moderator_decision_deadline
next_action_at
created_at, updated_at
```

数据库 CHECK 保证：

- `offered` 只有 `active_offer_id`；
- `granted` 只有 `active_grant_id`；
- moderator phase 二者都为空；
- `handoff_depth BETWEEN 0 AND 255` 作为协议硬上限，实际允许值读取 Session 冻结的
  `max_handoff_depth`；
- `consecutive_moderator_speeches >= 0`；
- `next_action_at` 与当前 phase/deadline 形状一致；
- ended 不保留活动对象或 deadline。

所有 V1 mutation 先 `SELECT ... FOR UPDATE` 锁定 `meeting_sessions`，再读取和更新此行。

### 8.5 `meeting_baton_state_history`

每份 Relay State 一行：

```text
community_id, session_id
state_revision
state_event_id
floor_revision, intent_revision, speech_revision
control_epoch, decision_epoch
transition_primary_type
transition_effects_json
created_at
```

关键唯一约束：

```text
UNIQUE (community_id, session_id, state_revision)
UNIQUE (community_id, state_event_id)
```

`meeting_baton_state.state_event_id/state_revision` 指向此历史表的最新行。

### 8.6 `meeting_speech_intents`

主要字段：

```text
intent_id
author_pubkey
current_event_id
basis_speech_revision
summary
addressed_to
state: pending | selected | rejected | withdrawn | stale | consumed | ended
selected_grant_id
reason_code, reason_text, terminal_event_id
created_at, updated_at, terminal_at
selection_attempt_count
last_offer_id, last_attempt_outcome
deferred_by_offer_id
defer_event_id, defer_reason
```

关键约束：

- 每名参与者最多一个 `pending` Intent；
- `current_event_id` 唯一；
- `selected_grant_id` 若存在必须唯一关联 Grant；
- rejected 必须有 moderator 和 reason；
- deferred Intent 仍是 pending，但只在绑定的 moderator-self Offer/Grant 活动期间不参与选择；
- refresh/withdraw 使用 `prev` CAS；
- 每次以该 Intent 创建 Offer 时递增 `selection_attempt_count`，Offer/Grant 的非 speech
  终态更新 `last_attempt_outcome`；这些变化都递增 `intent_revision`；
- summary、reason 长度在数据库和 handler 双重校验。

### 8.7 `meeting_human_floor_requests`

主要字段：

```text
request_id
requester_pubkey
queue_position BIGINT identity
state: queued | offered | granted | withdrawn | declined | timed_out | ended
offer_id, grant_id
request_event_id, terminal_event_id
created_at, terminal_at
```

关键约束：

- 每名 Human 最多一个 active request；
- active 只包括 queued/offered；ACK 创建 Grant 时 `granted` 成为 terminal consumed 状态；
- `queue_position` 唯一并作为 FIFO 权威顺序；
- moderator 不能写入终态；
- request 只可由本人撤回；
- 一旦 Grant 创建，本次 request 即被消费，不会在 Grant Yield/Expiry 时再次改状态或重复
  增加 revision；
- Request 已 consumed 后，同一 Human 可以再次 Request；新请求按新的 queue position
  参与下一席排序。

### 8.8 `meeting_baton_offers`

主要字段：

```text
offer_id
target_pubkey
allocation_source: moderator_select | directed_handoff | human_request | fallback
turn_role: participant | moderator_self
allocation_event_id
selection_reason
source_intent_id
source_request_id
source_handoff_id
source_speech_event_id
reason_type, reason_text
basis_speech_revision
depth_mode: reset | preserve | increment_provisional
previous_handoff_depth
requested_handoff_depth
ack_deadline
state: pending | acked | declined | timed_out | preempted | recalled |
       source_changed | source_withdrawn | ended
response_event_id
created_at, resolved_at
```

`offer_id` 是 Relay 在状态迁移前预生成的独立 32-byte 对象 ID，不等于
`state_event_id`。

`allocation_source` 说明“谁安排了本席”，`turn_role` 说明“本席是否属于 moderator
self speech”，两者必须正交。Relay 根据 target 和 source Intent 推导 `turn_role`，不接受
客户端自报。Fallback 若选中 moderator 自己的 Intent，仍是
`allocation_source=fallback + turn_role=moderator_self`；Directed Handoff 指向
moderator 则仍是 `turn_role=participant`。

Source shape 与深度模式固定如下，Relay/DB CHECK 不允许其他组合：

| 安排路径 | `allocation_source` | 唯一 source | `turn_role` | `depth_mode` |
|---|---|---|---|---|
| moderator Select Intent | `moderator_select` | `source_intent_id` | target 是 moderator 时 `moderator_self`，否则 `participant` | `reset` |
| moderator Select open Handoff | `moderator_select` | `source_handoff_id` | `participant` | `reset` |
| moderator-self speech 的即时 Handoff | `directed_handoff` | `source_handoff_id` | `participant` | `reset` |
| ordinary speech 的即时 Handoff | `directed_handoff` | `source_handoff_id` | `participant` | `increment_provisional` |
| Human Floor Request | `human_request` | `source_request_id` | `participant` | `preserve` |
| deterministic Intent fallback | `fallback` | `source_intent_id` | 按 target 推导 | `reset` |

`depth_mode` 由 Relay 从安排路径和 source Grant role 得出并持久化，客户端不能自报。
`increment_provisional` 是唯一会在 ACK 时暂占下一深度、并在无 speech 终态恢复
`previous_handoff_depth` 的形状。`reset` 和 `preserve` 永不靠 requested/previous 数值
反推来源。

`source_handoff_id` 在 Offer/Grant 上建立普通非唯一复合索引
`(community_id, session_id, source_handoff_id, created_at)`；同一 open Handoff 对应多次
attempt 是预期行为，不能误建唯一约束。

`allocation_event_id` 指向真正安排本席的签名事件：moderator Select、Human Request 或
携带即时 Handoff 的 source speech；fallback 为 null，并通过 creation State 的
`primary_type=moderator_fallback` 识别。Moderator Select 的 `selection_reason` 原样复制到
Offer，并在 ACK 时连同 `allocation_event_id/source_offer_id` 冻结到 Grant。这样 ACP
重启对账和历史查询可以沿
`Grant → Offer → allocation command/source object` 完整追踪，不依赖到达顺序猜测。

### 8.9 `meeting_baton_grants`

主要字段：

```text
grant_id
holder_pubkey
allocation_source
turn_role
source_offer_id
allocation_event_id
selection_reason
source_intent_id
source_request_id
source_handoff_id
source_speech_event_id
basis_speech_revision
depth_mode: reset | preserve | increment_provisional
previous_handoff_depth
handoff_depth
soft_lease_expires_at
hard_deadline
progress_seq
state: active | spoken | yielded | soft_expired | hard_expired | ended
speech_event_id
created_at, terminal_at
```

关键约束：

- 一个 Grant 最多一个 `speech_event_id`；
- 一个 speech 最多消费一个 Grant；
- `grant_id` 是 Relay 预生成的独立 32-byte 对象 ID，不等于 `state_event_id`；
- `soft_lease_expires_at <= hard_deadline`；
- holder、allocation source、turn role 和 source object 形状一致；
- 同一 Session 只有 `meeting_baton_state.active_grant_id` 指向的 Grant 可消费。

### 8.10 `meeting_grant_progress`

每个 accepted Progress 一行：

```text
community_id, session_id
grant_id
progress_seq
progress_event_id
stage
soft_lease_expires_at
accepted_at
```

主键为 `(community_id, grant_id, progress_seq)`，`progress_event_id` 唯一。不同签名事件
争用同一 seq 时返回这里记录的 canonical event ID；旧 Progress 重放也能恢复第一次响应。

### 8.11 `meeting_directed_handoffs`

一条 speech 最多一行：

```text
handoff_id
source_speech_event_id
from_pubkey
to_pubkey
reason_type, reason_text
requested_depth
question_state: open | answered | dismissed | blocked | ended
initial_disposition: offered | blocked
blocked_by: human_request | recall | max_depth | open_question_limit
last_offer_id, last_grant_id
last_attempt_outcome
attempt_count
answered_by_speech_event_id
dismiss_event_id, dismiss_reason_code, dismiss_reason_text
created_at, answered_at, dismissed_at, terminal_at
```

Handoff 表表达“定向问题是否仍待回答”，Offer/Grant 表表达一次次 transport attempt。
新 Handoff 从 `question_state=open` 开始；被 Human、Recall 或深度上限覆盖，Offer
Decline/timeout，以及目标 Grant Yield/Expiry，都只结束本次 attempt，问题仍保持 open。
只有来源引用该 Handoff 的 Grant 接受 canonical speech 后才进入 `answered`；moderator
可以把没有活动 attempt 的 open Handoff 显式变为 `dismissed`；Meeting End 把剩余 open
行变为 `ended`。因此一次失败不会抹掉问题，也不会消费目标无关 Intent。

同一 Session 的 open 行在 Session 锁内计数，并由事务检查限制在冻结的
`max_open_handoffs`（首版 32）。若一条合法 speech 在已满时携带新 Handoff，Relay 仍
原子接受 speech，但把 Handoff 保存为
`question_state=blocked + blocked_by=open_question_limit`，并在 transition 中公开说明；
不能创建 Offer，也不能静默丢弃。主持人先 Dismiss 旧问题后，后续 speech 才能创建新的
open Handoff。

DB CHECK 区分两种终态：`dismissed` 必须有 moderator 签名的
`dismiss_event_id/reason` 且 `blocked_by IS NULL`；`blocked` 只允许
`blocked_by=open_question_limit`，没有伪造的 moderator event/reason。`answered` 必须
有 `answered_by_speech_event_id`；所有非 open 状态必须有对应 terminal timestamp。

Relay 不会因为 open 状态自行重放旧 Offer。只有 moderator 显式 Select 才能为同一
Handoff 创建新的 Offer；每次 attempt 都以新的独立 `offer_id`/`grant_id` 写入历史表。

每条 speech 最多一个 Handoff，因此固定：

```text
handoff_id = source_speech_event_id
```

后续 Offer、State transition、失败结果和 CLI history 都使用这个稳定 ID。

### 8.12 `meeting_baton_fallback_attempts`

对自动选择的 Intent basis 去重：

```text
community_id, session_id
intent_id
current_intent_event_id
speech_revision
offer_id
attempted_at
```

唯一键为
`(community_id, session_id, intent_id, current_intent_event_id, speech_revision)`。
Fallback 对同一 basis 最多自动 Offer 一次：

- Intent refresh 产生新的 `current_event_id`；
- 新 canonical speech revision 允许重新评估；
- 显式 moderator Select 不写此表，也不受自动尝试限制。

Open Handoff 不写此表，也不是 fallback basis；只能由 moderator 签名 Select
重新安排。目标自己的 pending Intent 是独立 basis，正常参加 Intent fallback。

### 8.13 `meeting_v1_command_receipts`

非公开幂等 read model：

```text
community_id, session_id
command_event_id
author_pubkey
kind, action
accepted
outcome_code
canonical_object_id
state_revision
response_json
recorded_at
```

主键为 `(community_id, command_event_id)`；同一个签名命令在一个 Community 内只能有
一份 canonical receipt。

规则：

- accepted command 与 terminal semantic rejection 都保存；
- preflight 签名/shape 错误、无权访问和可重试内部错误不保存；
- receipt 不进入会议 control log，也不使 rejected command 成为 accepted Nostr event；
- 只有通过当前 Auth/私有 Meeting 访问检查，且 receipt 的 `author_pubkey` 与签名作者
  一致，才可返回 receipt；安全撤权后不能借 receipt 读取旧 response；
- receipt lookup 发生在 Session 锁和 lazy recovery 之后，不提供绕过 deadline recovery
  的锁前 fast-path；
- `response_json` 使用版本化 DTO，不存私有模型推理。

### 8.14 `meeting_revocation_jobs`

安全撤权的耐久工作队列：

```text
job_id
community_id
revoked_pubkey
revocation_event_id
security_order
state: pending | running | completed
cursor_session_id
attempts
next_attempt_at
last_error
created_at, completed_at
```

`revocation_event_id` 唯一。只有
`job.security_order > session.security_order` 时，该 job 才永久作用于该 Session；
恢复身份后创建的新 Session 不继承旧撤权。每场目标 Session 的 End 本身幂等，worker
可以在崩溃后从头枚举或从 cursor 继续；不能把 NIP-IA archive 写入此队列。

### 8.15 Outbox

继续使用 `meeting_event_outbox`。按 transition 写入逻辑顺序：

1. 已到期 recovery State；
2. 若新 command 仍有效，原始签名 command/speech；
3. 该 command 产生的 Relay-signed State；
4. 通知事件。

若没有 recovery，从第 2 步开始；若 command 被 recovery 拒绝，则只有第 1 步，不发布
rejected command。

`sequence` 表示数据库内的因果排队顺序，但多 Relay worker、lease 重试和网络 fan-out
不能保证客户端严格按 sequence 到达。客户端必须：

- 按 event ID 去重；
- 按 `state_revision` 忽略旧 State；
- live-first 后分页 backfill 补洞；
- 不因先看到 State、后看到 source command 就回滚权威快照。

数据库增加：

```text
UNIQUE (community_id, session_id, state_revision)
UNIQUE (community_id, state_event_id)
```

V1 Create/End 也必须进入同一 meeting outbox，不能沿用当前 commit 后
`dispatch_meeting_command_event` 的直接 fan-out，否则初始/终态 State 可能和 command
走两套投递路径并被重复发送。实现时把 `persist_meeting_event_tx` 和 outbox worker 辅助
提取为 policy-neutral 内部模块；V0 路径一并改为单一投递方式并做回归测试。

阶段一的 outbox 因果承诺只覆盖 canonical Meeting 日志，即
`Create/End/State`。现有 Channel discovery 和成员通知仍是 commit 后的
best-effort 投影，不保证相对 canonical 日志的到达顺序，也不能作为客户端判断 Session
状态的依据。上面第 4 项所指的协议通知事件从阶段二开始必须进入 outbox；在产品前端接入
前，再单独决定是否把 discovery/成员通知也迁入同一耐久投递链路。

## 9. 状态迁移与事务边界

### 9.1 通用写路径

每个 V1 写操作都遵循：

```text
preflight signed event shape and signature
begin transaction
  lock meeting_sessions row
  load MeetingBatonState
  assert persisted policy and actor authorization
  read database clock
  append zero or more due recovery transitions where now >= deadline
  look up any terminal command receipt
  if receipt exists:
    return its canonical command outcome after persisting recovery transitions
  verify role, revision, logical object and command against recovered state

  if command remains valid:
    persist accepted signed command
    apply one atomic command transition with one or more ordered effects
    persist command receipt
  else if validation produced a terminal semantic rejection:
    do not persist or publish the rejected signed command
    persist a private terminal command receipt
  else:
    return a non-receipted preflight/auth/retriable error

  build one Relay-signed State per transition with database time
  persist projections, State events and outbox
commit
publish from outbox
```

先执行 lazy recovery 再验证命令，可以明确 deadline 边界：

```text
now < deadline   → 操作仍有效
now >= deadline  → 先执行 timeout，迟到操作被拒绝或返回终态
```

DB API 必须区分“事务失败”和“recovery 已提交、但本次 command 被拒绝”：

```text
Committed {
  recovery_transitions,
  command_outcome:
    Accepted | Duplicate |
    RejectedTerminal {
      reason,
      canonical_object_id
    } |
    RejectedAfterRecovery {
      reason,
      canonical_object_id
    }
}

RolledBack {
  error
}
```

例如迟到 ACK 触发 Offer timeout 时：

1. timeout projection、Relay State 和 outbox 必须提交；
2. 迟到 ACK 不作为 accepted meeting event 发布，但保存一条非公开 terminal response
   receipt，保证相同 event ID 重试得到相同响应；
3. commit 完成后 Relay 向客户端返回 expired/conflict 和 canonical Offer outcome；
4. 不能用普通 `Err` 让 `?` 回滚刚完成的 recovery。

如果 recovery 后的新命令仍然有效，例如 Grant expiry 后同一 Human 提交新的 Request，
同一事务可以依次生成 recovery State 和 command State；outbox sequence 保留历史顺序。

Receipt 只在 Session 锁内检查：两个相同 event ID 并发时，第二个请求等待锁后读取第一
个请求的结果，而不是重复执行；即使是重放，也会先提交当时已经 due 的 recovery。
accepted event insert 仍使用唯一约束作为最后防线；若 `ON CONFLICT`，读取 receipt 或
canonical object 后返回。

### 9.2 Intent

- Submit/Refresh/Withdraw 更新 Intent 投影并增加 `intent_revision`；
- 如果当前是 `moderator_idle` 且出现新的 deterministic fallback candidate，递增
  `decision_epoch`、进入 `moderator_control`，并启动一次新的 3 分钟 decision window；
- 当前是 `offered` 或 `granted` 时，不改变活动对象，moderator Agent 可以异步更新私有
  ModeratorPlan；
- 上一条只适用于与活动对象无关的 Intent。若 Refresh/Withdraw/Reject 的正是当前 Offer
  的 `source_intent_id`，事务必须先把旧 Offer 终结为 `source_changed` 或
  `source_withdrawn`，再更新/终结 Intent，并按 Human 队列和 moderator 优先级重新调度；
- Intent 已进入 selected、对应 Grant 已创建后，不再接受 Refresh/Withdraw/Reject；
  holder 应通过 SAY 或 YIELD 完成本次机会；
- 如果 moderator 拒绝或作者撤回最后一个可处理 Intent，`moderator_control` 返回
  `moderator_idle` 并取消 decision deadline；open Handoff 不阻止该转换；
- 已处于同一 `moderator_control` decision window 时，新 Intent、refresh 或
  ModeratorPlan 结果都不能重置 3 分钟期限。

### 9.3 Moderator Select / Reject / Recall

- Select 在 revisions 匹配且不存在 Human Request 时创建短 Offer；
- Select 必须恰好引用一个 pending Intent 或一个 open Handoff；Relay 分别校验当前
  Intent event ID 集合和 open Handoff ID 集合，客户端不能提供 target；
- `expected-control-epoch` 与 `expected-decision-epoch` 都必须匹配当前 State；从
  `moderator_idle` 手工选择时使用当前（可能没有活动 deadline 的）decision epoch；
- moderator 有 pending self Intent 时，选择其他 Intent 或 open Handoff 均返回 conflict；
- Intent 在 Offer 阶段仍保持 pending，但创建 Offer 会原子增加
  `selection_attempt_count/intent_revision`；Handoff source 则增加
  `attempt_count/floor_revision`；
- self Select 携带 Deferral 时，Relay 在同一事务把它们绑定到新 Offer，并让
  `intent_revision` 增加一次；ACK 后通过 Grant 的 `source_offer_id` 继续判断绑定关系，
  不需要第二份 `deferred_by_grant_id`；
- Reject 原子保存原因、终结 Intent、增加 `intent_revision`；
- Dismiss Handoff 原子保存原因并从 State 的 `unresolved_handoffs` 移除；它不创建
  decision deadline，也不改变任何 Intent；
- Recall 在 Offer 阶段按 `allocation_source=human_request` 与其他 source 的规则处理，
  在 Grant 阶段只锁存；
- Recall 取消 non-human_request-sourced Offer 时同样写 source attempt
  outcome/revision；
- 任一选择动作都不能由 ModeratorPlan 直接写数据库，必须经过 moderator 签名事件和 Relay
  验证。

### 9.4 Human Request

Request 与当前状态在同一行锁下排序：

1. `granted`：只入队；
2. `offered` 且当前 `allocation_source != human_request`：终结旧 Offer为 preempted，
   立即 Offer 队首 Human；
3. `offered` 且已经是 human_request-sourced Offer：只入队；
4. moderator phase：立即 Offer 队首 Human。

被抢占的 Moderator-selected Intent 保持 pending；Handoff 的当前 Offer attempt 标记
preempted，Handoff 本身仍为 open，不自动排到 Human 队列之后执行。两种 source 都更新
attempt outcome/revision，旧 ModeratorPlan 不能在 Human 结束后机械重放。

Human Withdraw 的闭环规则：

- queued Request：直接终结；
- Request 正是当前 human_request-sourced Offer 的 source：原子取消 Offer并终结
  Request，然后 Offer 下一名 Human 或归还 moderator；
- Request 已经 ACK 并创建 Grant：不能再 Withdraw，只能 SAY 或 YIELD。

### 9.5 Offer ACK / Decline / Timeout

ACK 原子执行：

1. 验证 target、offer_id 和 `now < ack_deadline`；
2. 终结 Offer 为 acked；
3. 任何 Offer 只要 `source_intent_id IS NOT NULL`，都把该 Intent 标为 selected；这同时
   覆盖 explicit moderator selection 和 fallback；
4. 若 `allocation_source=human_request`，把该 Request 标为 granted；
5. 把 Offer 冻结的 `previous_handoff_depth/requested_handoff_depth` 复制到 Grant：
   moderator 调度为 0，moderator-self Handoff 为 0，ordinary Handoff 为当前深度加一，
   Human Request 保留当前深度；
6. 创建稳定 `grant_id`；
7. `depth_mode=increment_provisional` 的 Grant 把 State 的 `handoff_depth` 暂设为
   requested 值；其他 source 使用其冻结值；
8. 设置 soft lease 与 5 分钟 hard deadline；
9. phase 进入 `granted`。

Intent/Request 从 pending/queued 进入 selected/granted 时同时增加 `intent_revision`。

Decline 或 timeout：

- 不创建 Grant；
- 不增加 Handoff 深度；
- Moderator-selected Intent 保持 pending；
- Human Request 终结；
- Handoff 的本次 attempt 终结，但问题仍为 open；
- source Intent/Handoff 都记录 `last_attempt_outcome` 并增加相应 revision，使旧
  ModeratorPlan 失效；
- 然后按 `Human 队列 > moderator` 继续调度。

同一 Offer 的 ACK、Decline、Human 抢占和 timeout 通过会议行锁决定唯一结果。相同 event ID
重放返回原结果；不同事件争用同一逻辑终态时返回 canonical object。

### 9.6 Progress

- 只接受 holder 对活动 `grant_id` 的下一个 `progress_seq`；
- Progress 不能在 soft lease 或 hard deadline 之后复活 Grant；
- 接受后更新 soft deadline、增加 `floor_revision` 并发出新 State；
- 达到 hard deadline 后，即使 Progress 正在重试也必须 expiry；
- Progress 不触发任何 LLM，也不改变 speech/intent revision。

### 9.7 SAY 与 Handoff

SAY 事务：

1. 验证 holder、grant_id、speech revision 和 deadline；
2. 保存 canonical kind `9` speech；
3. 将 Grant 终结为 spoken；
4. `speech_revision += 1`；
5. Grant `turn_role=moderator_self` 时增加 `consecutive_moderator_speeches`，任何其他
   turn role 的 canonical speech 都重置为 0；
6. 如果 Grant 来源是 selected Intent，将其标为 consumed；
7. 如果 Grant 的 `source_handoff_id` 非空，将该 open Handoff 标为 answered，并记录
   `answered_by_speech_event_id`；
8. 如果这是 `depth_mode=increment_provisional` Grant，提交已暂占的
   `handoff_depth`；
9. 如果 speech 携带新的 Handoff，校验并保存为新的 open question；
10. 按下一节的固定优先级创建 Offer 或归还 moderator；
11. 发出新的 Relay State。

如果本次 Grant `turn_role=moderator_self`，且优先级允许执行其 Handoff，Relay 在创建目标
Offer 前把 `handoff_depth` 原子重置为 0；这次传递属于主持调度。moderator 通过其他
turn role 获得 Grant 时不能触发重置。若 Recall/forced return 已锁存，Handoff 仍被阻止，
不能借重置绕过更高优先级。

Intent 被 consumed 时同时增加 `intent_revision`。若本次是 moderator-self Grant，与该
Offer/Grant 绑定的 Deferral 同时解除；相关 Intent 仍为 pending，不需要伪造参与者事件。

speech、Grant 消费和 Handoff 永远不能拆成三个事务。

创建新 Handoff 前，Relay 在同一 Session 锁内计算 open question 数。低于冻结上限时保存
为 open，再由固定优先级决定是否立即 Offer；达到上限时保存
`blocked + blocked_by=open_question_limit`，不创建 Offer，并在同一 State transition
公开结果。Speech 本身仍为 canonical，客户端可以据此提示 speaker 由主持人先清理旧问题。

### 9.8 Grant Yield / Expiry

Yield、soft expiry 和 hard expiry都立即终结 Grant：

- 来源 selected Intent 进入 stale，而不是静默恢复为 pending；
- Human Request 已经消费，Human 可以重新 Request；
- 来源 Handoff 仍为 open，只终结本次 Grant attempt；
- `depth_mode=increment_provisional` Grant 恢复 `previous_handoff_depth`，不把未发言
  的目标计入连续接力；
- 没有 canonical speech，不增加 `speech_revision`；
- 然后处理 Human 队列；
- 无 Human 时 Control Token 回到 moderator；
- 晚到的 LLM 结果和 prepared SAY 必须丢弃。

selected Intent 进入 stale 时增加 `intent_revision`；Human Request 已在 ACK 时成为
terminal granted，Grant 结束时不再次改变 Request 或重复增加 `intent_revision`。Grant
自身的终态只增加 `floor_revision`。

moderator-self Offer 或 Grant 以任何非 speech 结果终结时，也必须在同一事务解除其全部
Deferral并增加一次 `intent_revision`。

### 9.9 Grant 结束后的固定优先级

```text
1. 最早 Human Floor Request
2. forced_return_to_moderator / Recall
3. handoff_depth 已达到 Session 的 `max_handoff_depth`
4. 当前 speech 的合法 Directed Handoff
5. moderator
```

默认 timing profile 的 `max_handoff_depth=5`：第五次成功 Handoff 的 target 可以正常
发言；其 canonical speech 被接受后必须锁存并完成 forced return，不能创建第六次
Handoff。若该 Grant Yield/Expiry，先恢复此前深度，不因未发生的 speech 消耗一次接力。
若未来 profile 使用其他值，状态机读取冻结值，语义同样是“达到上限的目标说完后归还”。

Human 队列可以在 forced return 之前逐个发言，但这期间所有新 Handoff 都记录为 blocked。
Human 队列清空后必须进入 moderator control，随后：

- 清除 Recall/forced return；
- `handoff_depth = 0`；
- 只根据 deterministic fallback candidate 进入 `moderator_control` 或
  `moderator_idle`；open Handoff 本身不启动 deadline。

不仅 forced return如此：任何路径只要 Control Token 真正回到 moderator，都会把
`handoff_depth` 重置为 0，并把 `control_epoch` 加一。

### 9.10 Meeting End

End 继续使用既有授权语义，并增加此前尚未实现的安全撤权终止链路：

- Owner 或 Community owner/admin 签名的 manual End；
- relay membership removal、ban、account deactivation 等真正收回访问权的 security
  revocation；
- NIP-IA identity archive 只是 UI visibility hint，不等同 ban，不自动结束 Meeting。

V1 固定名单中的任一身份发生 security revocation 时，会议整体结束；moderator 被撤权
不会触发自动换主持人。

撤权不能在一个身份事务里锁定并结束任意多个 Meeting。实现新增耐久、幂等的
`meeting_revocation_jobs`：

1. security revocation 事务先使 Auth/read/write gate 立即失效，并写入 job；
2. worker 枚举包含该 pubkey 的 active V0/V1 Session；
3. 按 session ID 为每场会议开启短事务，先锁 `meeting_sessions`；
4. 生成 Relay-signed End `reason=participant_revoked`；
5. 按持久 policy 关闭 V0 floor 或 V1 baton、归档 Channel并写 outbox；
6. 每个 Session 独立记录完成，失败可重试，全部完成后 job 终结。

任意 Meeting 写事务也执行轻量 roster security check；若发现尚未被 worker 处理的撤权，
它先提交同一终止迁移，再拒绝原命令，形成 lazy recovery。这样 job 延迟不会让已撤权者
恢复访问，也不会让其他写操作长期延续无效名单。

历史读取和 live fan-out 不把冻结名单或 Channel membership cache 当成持续授权。它们先
检查当前主体状态，再以冻结名单和 `security_order` 撤权栅栏作为最终过滤；成员被快速
重新加入或解禁，也不能恢复对旧 Session 的读取、receipt 或实时事件。

终止事务中：

- Session 与 Channel 进入终态/归档；
- active Intent、Request、Offer 和 Grant 全部进入 ended；
- Baton phase 进入 ended；
- 清空 deadline；
- 发出最终 State；
- 保留全部历史供仍具 Community 授权的名单成员只读查询；被安全撤权身份不再具有读取权。

End 高于所有其他状态，但仍与并发 SAY/ACK 通过同一会议行锁形成唯一提交顺序。
撤权 worker 与 SAY、ACK、Progress 并发时也使用同一规则：数据库中先取得 Session 锁的一方先
提交，后一方读取终态后不得复活会议。

## 10. 主持人决策

### 10.1 Decision deadline

默认期限为 **3 分钟**，不是 15 秒。

计时起点是：

> 状态进入 `moderator_control`，并且存在至少一个通过公平 gate 的 deterministic
> fallback candidate 时的数据库时间。

规则：

- deadline 是本次连续 `moderator_control` decision window 的绝对期限，并由
  `decision_epoch` 标识，不由 `control_epoch` 兼任；
- 新 Intent、Intent refresh、LLM retry 或新的 ModeratorPlan 不重置期限；
- Human Request 到达时不等待主持决策，立即走 Human 优先路径；
- moderator 在期限内提交有效 Select/Reject 后，状态按结果推进；
- moderator 选择自己后，进入独立 Offer/Grant 期限，不继续使用 decision deadline；
- 没有 deterministic fallback candidate 后进入 `moderator_idle`，清除 deadline并结束
  本次 window；未来再次从 idle 进入 control 时，即使 `control_epoch` 未变化，也递增
  `decision_epoch` 并获得新的 3 分钟窗口。

### 10.2 Agent moderator 的异步 ModeratorPlan

ModeratorPlan 只保存在 ACP 私有账本，不发布到共享会议，并拆成两层：

```text
AgendaRanking:
  observed_speech_revision
  observed_intent_revision
  intent_fingerprint[]:
    intent_id
    current_event_id
    selection_attempt_count
    last_attempt_outcome?
  open_handoff_fingerprint[]:
    handoff_id
    attempt_count
    last_offer_id?
    last_attempt_outcome?
  active_grant_id?
  ranked_candidates[]:
    type: intent | handoff
    id
  proposed_rejections[]
  proposed_handoff_dismissals[]
  proposed_moderator_summary?
  state: preparing | ready | stale

ControlDecision:
  control_epoch
  decision_epoch
  speech_revision
  intent_revision
  intent_fingerprint[]
  open_handoff_fingerprint[]
  rejections[]
  handoff_dismissals[]
  deferrals[]
  next_action: select_intent | select_handoff | moderator_speak | idle
  selected_intent_id?
  selected_handoff_id?
  state: preparing | ready | stale | consumed
```

`moderator_speak` 必须引用 moderator 已有的 pending self Intent；如果计划首次提出主持人
要讲的新内容，Harness 先以 participant 身份提交 self Intent，重新同步后再 Select，不能让
ModeratorPlan 绕过共享 Intent 记录。

其他人发言期间可以生成 AgendaRanking。它不以 `floor_revision` 判 stale：Progress、
soft-lease 延长等事件不改变排名语义。当前 speaker 的最终 speech 尚未出现，因此
AgendaRanking 只是预处理结果，不是假装已经完成最终决策。

Control Token 返回时才生成或复验 ControlDecision：

1. 先同步最新 State、Intent 池、open Handoff fingerprint、Human 队列和 speech；
2. Human Request 已出现则不提交计划；
3. 使用缓存排名和最新 speech 快速完成语义复验；
4. `control_epoch`、`decision_epoch`、`speech_revision`、`intent_revision`、所引用
   Intent 和完整 open Handoff fingerprint 仍完全匹配时才提交；
5. 先逐个提交仍可独立成立的 Reject 和 Handoff Dismiss；每次后重新同步，active
   attempt 或 stale Handoff conflict 不得影响其他候选；
6. 最后提交至多一个 Select；
7. stale decision 直接丢弃并重新规划。

这样异步排名不会被 10 秒 Progress 反复打旧，而最新 speech 仍会在最终选择前进入判断。
如果 moderator 有 pending self Intent，ControlDecision 不得选择其他 Intent 或 Handoff；
Harness 应先选择/撤回 self Intent，不能依赖 Relay conflict 代替计划约束。

每个 Select 一提交，Harness 就把对应私有 candidate plan 标为 consumed，不能在新的
control epoch 重新包装。Intent Offer 创建时增加 `selection_attempt_count` 和
`intent_revision`；其 Decline/timeout/preempt/Recall 或 Grant Yield/Expiry 再更新
`last_attempt_outcome` 和 revision。Handoff 则在每次 Offer 时增加 `attempt_count`，
失败时更新完整 fingerprint。任何 attempt 变化都使旧 AgendaRanking/ControlDecision
stale。

Agent moderator 想再次选择同一 pending Intent 或 open Handoff 时，必须基于本次失败原因
运行新的主持判断；尤其不能对 Handoff 做缓存计划自动重试。Human moderator 仍可读取
最新 fingerprint 后显式重试。

### 10.3 确定性 fallback

3 分钟到期后 Relay 依次选择：

1. 最早 Human Request；
2. moderator 的 fallback-eligible self Intent，但连续 self speech gate 要求先处理其他
   valid pending Intent 时除外；
3. 等待最久的 fallback-eligible 普通 pending Intent；
4. 没有通过上述 gate 的候选时进入 `moderator_idle`，open Handoff 保持可见。

Fallback 只创建 Offer，不直接创建 Grant，也不改变 moderator 身份。

`valid pending` 指 state 是 pending、没有活动 Deferral、作者仍是有效参会者且不存在引用
它的活动 Offer/Grant。某个 valid pending Intent 只有在当前
`(intent_id, current_event_id, speech_revision)` 尚无 fallback attempt 时，才进一步是
`fallback-eligible`。

“等待最久”使用 Intent 初次进入 pending 时的数据库 `created_at`，稳定排序为
`ORDER BY created_at, intent_id`；Refresh 不改变初次排队时间，只改变
`current_event_id` 并形成新的 fallback basis。

Fallback 不自动重试任何 open Handoff。Offer 失败后的 Handoff，以及被 Human、Recall
或深度上限覆盖的 Handoff，都只能由 moderator 的签名 Select 重新安排；否则 Relay 会
绕过“归还主持人重新判断”的语义。目标若仍主动想回答，可以独立提交 SpeechIntent，
但该 Intent 是自己的 voluntary basis，不能冒充旧 Handoff 的 continuation。

Fallback 仍执行连续 self speech gate：当
`consecutive_moderator_speeches >= 1` 且存在其他 valid pending Intent 时，moderator 的
self Intent 不属于 fallback 候选。Relay 不能替 moderator 编造 Deferral reason：有
fallback-eligible 普通 Intent 时选择它；普通 Intent 都已尝试时进入 `moderator_idle`，
不能因为它们暂时不是自动候选就再次连续选择 self。

Self Intent 的 fallback Offer 失败后，该 basis 已不再 eligible；下一次 deadline 可以
选择尚未尝试的普通 Intent，即使 self Intent 仍为 pending。这是 Relay liveness fallback
对“self 优先”的唯一例外：moderator 的显式 Select 仍受 self-priority gate 约束。当前
speech revision 下没有通过公平 gate 的自动候选时，状态进入 `moderator_idle`，保留
pending Intent 供 moderator 手工 Select/Reject/Withdraw；Intent Refresh、新 Intent、
相关 Intent 终结或新的 canonical speech 会重新计算候选，并在至少一个候选出现时进入
`moderator_control`。

为避免离线参与者造成无限自动循环，Relay 在
`meeting_baton_fallback_attempts` 记录 Intent basis；同一 Intent current event 在相同
speech revision 下只自动尝试一次。显式 moderator Select 不受此限制；Intent refresh
或新的 canonical speech 会开启新的 fallback basis。

## 11. 超时与恢复

### 11.1 默认值

| 超时点 | 默认值 | 是否等待完整时长 |
|---|---:|---|
| Agent Offer ACK | 5 秒 | 否，ACK/Decline 立即推进 |
| Offer targeting Human ACK | 15 秒 | 否，Human 响应立即推进 |
| Moderator Decision | 3 分钟 | 否，有效决策立即推进 |
| Grant Soft Lease | 30 秒 | 否，SAY/YIELD 立即推进 |
| Progress Interval | 10 秒 | 不是 Relay deadline |
| Grant Hard Deadline | 5 分钟 | 否，SAY/YIELD 立即推进 |
| Agent Hard-deadline Safety Margin | 30 秒 | Harness 本地预算 |
| Participant Intent Turn | 建议 60 秒 | 非阻塞共享会议 |

“5 分钟 Grant”表示最长允许时间，不表示每轮固定等待 5 分钟。绝大多数发言在 SAY 或
YIELD 成功时立即结束。

所有默认值通过 V1 专用配置读取并在 Session 创建时冻结一份策略版本或 timing profile，
避免运行中修改环境变量改变既有 Grant 的 deadline。

### 11.2 没有自动超时的对象

默认不自动过期：

- `moderator_idle`；
- 没有进入 selection 的 pending Intent；
- 排队中的 Human Request；
- open Handoff；
- MeetingSession 本身；
- Recall/forced return 锁存。

这些对象由显式操作或 Meeting End 终结。未来可以增加产品级 expiry，但不属于首版后端。

### 11.3 权威时间

- 所有 deadline 由 PostgreSQL `clock_timestamp()` 计算；
- 客户端 `created_at` 不参与先后和超时判断；
- Human FIFO 使用数据库 queue position；
- State 事件使用同一事务取得的数据库时间；
- Relay 进程本地时钟只用于调度唤醒，不用于最终合法性判断。

### 11.4 Sweeper

`meeting_runtime` 增加 V1 recovery：

```text
select due session ids from meeting_baton_state
where next_action_at <= clock_timestamp()
order by next_action_at
limit batch
```

每个候选在独立短事务中重新锁定 Session 并检查：

- Offer ACK deadline；
- Grant deadline 按固定顺序检查：先 `now >= hard_deadline` 产生
  `grant_hard_expired`，否则再以 `now >= soft_lease_expires_at` 产生
  `grant_soft_expired`；
- moderator decision deadline。

Progress 可以把 soft lease 截到恰好等于 hard deadline，因此上述 hard-first 规则同样
适用于 sweeper、lazy recovery 和测试时钟；同一数据库时刻不能在两个终态中任选其一。

`next_action_at` 始终是当前状态的最早有效 deadline。每次 recovery 要么完成迁移，要么把
它更新到未来，禁止 due row 无变化地长期占据 batch 前部。

多 Relay 实例可并行 sweep；候选查询本身不持有 Baton 行锁，每个候选随后在独立事务中
先锁 `meeting_sessions`，再按全系统统一顺序锁 Baton 投影并复查。多个实例选中同一
Session 只会产生一次迁移。

如果以后为吞吐量增加 `SKIP LOCKED` 或 deadline claim lease，也必须保持
`meeting_sessions → meeting_baton_state` 的统一加锁顺序，不能先锁 Baton 行再等待
Session 行，否则会与普通命令形成锁顺序反转。

### 11.5 Lazy recovery

任何作用于 V1 Session 的写命令在验证自身之前，都会检查并推进已到期状态。因此即使：

- sweeper 暂停；
- Relay 刚刚重启；
- 某条 timeout State 的 fan-out 延迟；

后续 ACK、Progress、SAY 或 moderator command 也不能复活已过期对象。

## 12. 共享消息与订阅

Meeting V1 不创建新的消息队列。会议仍是一个私有 Channel：

```text
session_id == channel_id
channel_type == stream
room_kind == meeting
visibility == private
```

每个 Human/Agent 客户端都：

1. 订阅同一个 `#h=<session_id>`；
2. Relay 对每个已授权订阅独立推送同一事件；
3. 任一客户端读取事件不会使其他客户端“消费不到”；
4. 离线者重连后通过 REQ/Query 补齐历史；
5. 客户端按 event ID 合并 live 与 backfill，消除竞态重复；
6. 当前状态只认最高 `state_revision` 的 Relay State，不按 `created_at` 或到达顺序覆盖；
7. 历史必须分页读取，不能沿用“最多取 1000 条 State 就视为完整”的 V0 快捷路径。

V1 查询必须显式指定 kinds，例如：

```text
9, 42100, 42101, 42103, 42105, 42106, 42107, 42108, 42109
```

展示层以后可以把它们分成：

- Speech timeline：仅 canonical kind `9`；
- Floor control log：Intent、Request、Moderator、Offer、Grant、Handoff 和 State。

Relay 在 Query、HTTP bridge 和 WebSocket subscription 三条路径上都执行同一私有 Channel
成员鉴权。非参会者不能借助已知 session UUID 读取任何会议事件。

最新 State 已携带 frozen config、participants、pending Intent、open Handoff、按
`queue_position` 排序的 Human queue，以及活动 Offer/Grant，因此 CLI/ACP 只使用通用
Query 就能恢复权威当前状态；不需要访问 Relay 内部 SQL 表或新增专用 HTTP endpoint。
历史 transition 通过分页的 `42103` State 恢复。

## 13. Relay 与代码组织

### 13.1 `buzz-db`

推荐新增：

```text
crates/buzz-db/src/meeting_baton.rs
```

它拥有：

- V1 类型和状态解析；
- 每种 command 的事务函数；
- deadline advancement；
- State event 构建；
- snapshot/query；
- due candidate recovery；
- V1 outbox 辅助调用。

`meeting_floor.rs` 保持 V0-only。共享的小型持久化辅助函数可以提取到内部模块，但不能让
两个策略共用一个巨大分支状态机。

### 13.2 `buzz-relay`

`command_executor` 按 kind 和 Session policy 路由到严格 handler：

- create/end；
- intent；
- moderator command；
- human request；
- offer response；
- grant signal；
- V1 speech。

handler 负责 wire 校验和鉴权；所有状态决定由 DB transaction 函数完成，避免 Relay
内存状态成为权威来源。

当前 command kinds 会在 generic Channel membership gate 之前进入
`command_executor`，因此每个 V1 DB transaction 都必须在查询 Intent/Offer/Grant 细节
之前重新验证 frozen participant、moderator、Human type、target 或 holder。不能只写
handler 层检查；非成员错误继续使用防枚举响应。Meeting End 的 Community admin 恢复
例外单独验证。

`meeting_runtime` 同时运行 V0 和 V1 recovery，但使用独立配置、查询和 metrics。

### 13.3 `buzz-core` 与 `buzz-sdk`

- kind registry 新增 `42105..42109`；
- 新公共 API 写 doc comments；
- builder 在签名前完成 tag vocabulary、枚举、长度、UUID/pubkey/event ID 校验；
- V0 builder 保持兼容；
- V1 使用独立函数名和结构体参数，避免十多个位置参数；
- fixture 固定每种合法/非法事件形状，供 SDK、Relay 与 ACP 共用。

### 13.4 `buzz-cli`

CLI 是 V1 后端的首个完整使用者。建议命令面：

```text
buzz meetings create --policy moderated-baton-v1 --moderator <pubkey> ...

buzz meetings intents list|submit|refresh|withdraw
buzz meetings moderator select (--intent <id> | --handoff <id>)
buzz meetings moderator reject|dismiss-handoff|recall
buzz meetings floor request|withdraw
buzz meetings offer ack|decline
buzz meetings grant progress|yield
buzz meetings say [--handoff-to ... --handoff-type ... --handoff-reason ...]
buzz meetings floor status|history
```

要求：

- 所有读操作显式 kinds + `#h`；
- 所有写操作返回 `{event_id, accepted, message}` 及相关 object ID；
- 逻辑冲突使用现有 exit code `5`，并尽可能返回 canonical ID；
- `moderator select` 要求 `--intent` 与 `--handoff` 恰好一个，target 始终由 Relay
  从源对象派生；
- CLI 在发送 Select/Dismiss/Refresh 等 CAS 命令前先读取最高 `state_revision` 的 State，
  自动填充所需 `expected-*`/`prev`；exit code 5 后重新同步并展示 canonical object，
  不用旧 revision 静默重试；
- `floor status` 同时显示 phase、revisions、Offer/Grant、deadline、Human queue 和 Intent
  摘要及 open Handoff；
- CLI 可以手工发送 Progress，便于在尚无产品前端时验证 Human 长时间输入。

## 14. ACP Agent 实现

### 14.1 参加会议的主动与被动边界

V1 延续已经验证的参加语义：

| 层次 | 行为 |
|---|---|
| 名册身份 | 创建者把 Agent 加入固定名单；这是被动结果 |
| 加入授权 | 创建前由 Agent Channel Add Policy 决定是否允许被加入 |
| 观察会议 | Create 成功后 ACP 自动发现、订阅并回填；不再运行一次 LLM 决定“是否入会” |
| 发言意图 | Agent 主动判断 SUBMIT 或 PASS |
| 接受 Offer | Harness 根据在线状态、策略和已预留容量确定性 ACK/Decline |
| 正式发言 | 获得 Grant 后 Agent 主动组织 SAY 或 YIELD |

V1 首版不增加公开 RSVP/Join 状态。Agent 不想表达内容时保持没有 pending Intent；暂时无法
承担被点名的发言时快速 Decline，而不是让 LLM 决定是否仍属于参会者。

### 14.2 单个共享同步器

每个受管 Agent 对每场会议只维护一个同步器：

- live subscription 先建立，再分页 backfill；
- 验证 Relay State 签名；
- 合并并去重 speech/control events；
- 恢复自身 pending Intent、Offer、Grant 和 prepared event；
- speech 携带 Handoff 时，等待一份 `speech_revision >= 该 speech revision` 的权威 State
  后再向语义 Controller 暴露其 disposition，避免 speech/State 到达顺序制造竞态；
- 把协议状态写入版本化私有 ledger。

不能为每类事件各自启动独立历史流，否则会在 Grant 和 Intent revision 之间产生观察竞态。

### 14.3 三类 LLM Controller 与即时 Offer Controller

Offer Controller 是不经过模型队列的即时控制路径。三类 LLM 工作的优先级为：

```text
Granted Speech > Moderator Decision > Participant Intent
```

#### Participant Intent Controller

- 仅在新的语义 speech、mention、明确问题或策略触发时运行；
- Floor State、Progress、ACK 等控制噪声不单独启动 LLM；
- 最多生成一句 summary；
- 输出严格结构：

```text
SUBMIT(summary, addressed_to?) | PASS
```

- PASS 只写私有 ledger；
- 收到 `handoff-to=self` 的 speech 时，不立即提交 voluntary Intent：若匹配 State
  显示该 Handoff 已有指向自己的活动 Offer/Grant，交给 Offer/Granted Controller；只有
  Handoff 被 blocked/failed、没有活动 attempt 时，才可把原问题作为独立语义触发重新判断
  是否提交 Intent；
- 旧 Turn 尚未开始时，新 speech 合并到最新上下文；
- 旧 revision 的结果不得提交。

#### Offer Controller

不调用 LLM。收到指向自己的 Agent Offer 后：

1. 验证 Session、target、deadline 和签名；
2. 检查参加策略；
3. 取消或标 stale 当前可抢占的 Participant Intent/ModeratorPlan 低优先级 Turn；
4. 在本地原子预留一个 Granted Turn slot；
5. 有容量则发送 prepared ACK；
6. 只有不可抢占的全局容量不足或策略不允许时才立即 Decline；
7. 网络结果不明确时重放同一个签名事件并同步 State。

ACK 成功后如果响应丢失，但同步发现自己已是 Grant holder，应直接恢复 Granted Turn。
如果 canonical 结果是 preempted、recalled、declined、timed_out 或其他非 granted 终态，
必须立即释放为该 Offer 预留的 slot。进程重启时 ledger reconciliation 只保留仍对应
active Offer/Grant 的 reservation，清理所有孤儿 reservation。

#### Granted Speech Controller

- 只为自己的活动 Grant 启动；
- 共享同步器保存完整权威 speech；模型 prompt 提供近期精确窗口、窗口边界、当前 Intent、
  Handoff reason、参会名单和最新 State，较早原文可通过 `meeting_read` 上下文工具按需
  查询。不能为了“完整记录”把无界历史一次性塞入模型窗口；
- Stage 3 的 `meeting_read history` 单次最多返回最近 500 条且尚无游标；超长会议的更早
  原文仍由 Relay 和同步器完整保存，但跨 500 条的模型按需回看留作后续游标增强；
- 可以调用 Agent 正常暴露的工具查看项目现状、文件、任务、工作流、视图和其他证据；
- Progress heartbeat 与 LLM/tool future 分离，每 10 秒按可观察阶段续租；
- 输出严格结构：

```text
SAY(content, optional_handoff) | YIELD(reason?)
```

- Harness 在 5 分钟 hard deadline 前 30 秒停止新工具调用并准备提交或 Yield；
- SAY 在本地先构造并持久保存 prepared signed event，再发送；
- deadline 后完成的模型结果必须丢弃；
- SAY/YIELD 成功立即释放本地 slot。

#### Moderator Controller

- 维护异步 ModeratorPlan；
- 只在自己是 moderator 时运行；
- ModeratorPlan 同时排序 pending Intent 与 open Handoff，可以批量提出少量 Intent
  Reject/Handoff Dismiss，但最多一个 next action；
- moderator-speak 所需的 Deferral 必须附在同一个 Select 中；
- Control Token 返回后必须重同步并重新验证；
- Human Request 优先于任何模型计划；
- 本地 decision Turn 的安全预算小于 Relay 的 3 分钟绝对 deadline；
- 迟到计划只记私有 telemetry，不可提交。

### 14.4 LLM 与工具约定

Meeting 的目的仍是讨论，不是在会议内执行任务。

Meeting V1 初版采用 advisory 工具策略。Meeting Turn 继承 Agent 的正常 MCP、CLI、HTTP
和原生工具能力，不强制 Plan mode，也不由 Harness 缩减为专用只读 MCP。system prompt
要求 Agent：

- 仅为形成发言而调查仓库、文档、任务、工作流、项目视图和其他已授权上下文；
- 不修改文件、Git、任务、工作流、项目状态或外部资源；
- 不发送会议之外的消息，不在 Meeting Turn 中执行后续任务；
- 发现需要执行的事项时，把它作为结论、问题或后续行动建议写入发言；
- 不通过工具自行发布 Meeting speech 或控制事件。

前四项是 Agent 行为约定，不是 Runtime、MCP 或 OS 级安全隔离。Codex 等 ACP Runtime
可能拥有不经过 `buzz-dev-mcp` 的原生工具；V1 不宣称能阻止受到提示注入或行为失控的
模型通过这些工具产生副作用。

Harness 管理的自动发布路径仍由代码约束：模型对该路径只返回结构化提议，Harness 在重新
校验最新权威 State 后构造、签名并提交协议事件，不会把模型文本直接当作待发布事件。

但“不通过工具自行发布”与其他无副作用要求一样属于 advisory 约定。正常工具面可能包含
Shell、带参会身份凭据的 Buzz CLI 或第三方客户端，因此 V1 不声称代码能阻止模型绕过
Harness 自动路径尝试提交事件。无论事件来自 Harness 还是普通工具，Relay 都必须执行相同
的成员身份、revision、Grant、deadline 和其他协议校验。

Project View 与 Meeting 仍按既定计划并行开发。Project View 后端尚未进入本分支时，
Meeting Stage 3 先提供会议历史、仓库文档、代码和已有文件状态的上下文读取能力；等
Project View 后端或 CLI 契约可用后，Meeting Agent 可以通过其正常工具面查询。工具是否
可用由 Agent Runtime 和部署配置决定，Meeting Harness 不为此维护独立 allowlist。

同一边界也适用于 Relay 的通用 Workflow Engine：Meeting outbox 中的 speech、State 和
控制事件只做持久化与订阅 fan-out，不作为通用 Workflow trigger。需要执行的后续工作应在
会议之外通过显式决议/任务机制创建，不能把一条会议发言隐式解释为有副作用的任务命令。

### 14.5 Prompt 分层

建议拆成三个独立 prompt，而不是在一个 prompt 中让模型自行识别阶段：

```text
meeting_participant_intent_prompt.md
meeting_moderator_prompt.md
meeting_granted_speech_prompt.md
```

共同要求：

- 不输出隐藏推理；
- 不把控制事件当成需要回复的发言；
- 引用明确对象时使用固定 pubkey/participant ID；
- 不承诺已经执行尚未执行的工作；
- 信息不足时可以 PASS、IDLE 或 YIELD；
- Handoff 必须给出明确对象和原因。

## 15. 幂等、冲突与错误

### 15.1 两层幂等

事件级：

- 已接受命令或 terminal semantic rejection 会写 command receipt；相同签名 event ID
  重放时返回该 canonical 结果；
- preflight 签名/shape 错误、无权访问和可重试内部错误不写 receipt，修复前置条件后可以
  重试或以新事件重新提交；这类响应不承诺“第一次结果”语义；
- 有 receipt 的重放不重复增加 revision，不重复写 outbox。

逻辑对象级：

- 同一 Intent 的 stale `prev`；
- 同一 Offer 的第二种响应；
- 已消费 Grant 的第二条 speech；
- 同一 Human 的第二个 active Request；
- 已终结对象的不同新命令；

均返回 conflict，并携带 canonical object/event ID。

### 15.2 错误类别

沿用现有 CLI/Relay 映射：

- input/shape error；
- authentication error；
- authorization/roster error；
- stale revision；
- object conflict；
- deadline expired；
- meeting ended；
- transient network/internal error。

错误文本不应泄露私有会议是否存在。对非成员，Not Found 与 Access Denied 继续服从现有
私有 Channel 防枚举策略。

## 16. 安全不变量

数据库约束、事务代码和测试必须共同保证：

1. 一个 V1 Session 只绑定一种 policy；
2. 同一时刻至多一个活动 Offer 或一个活动 Grant；
3. 只有 Relay 可以创建 State、Offer 和 Grant；
4. Offer target 才能 ACK/Decline；
5. Grant holder 才能 Progress/Yield/SAY；
6. 每个 Grant 最多一条 canonical speech；
7. speech 与 Grant 消费原子提交；
8. Handoff 与 source speech 原子提交；
9. Human FIFO、Recall 和深度上限不能被客户端绕过；
10. 超过 Session 冻结 `max_handoff_depth` 的直接 Handoff 永远不会创建 Offer；
11. deadline 后的事件不能复活对象；
12. async Intent 更新不能改变活动 `grant_id`；
13. Meeting End 之后没有新控制写入；
14. 非参会者看不到任何 speech 或 control event；
15. 任一 LLM 崩溃最多影响自己的 Turn，不成为 Relay 安全条件。

## 17. 测试设计

### 17.1 协议与 SDK

- 每种 event 的合法 fixture；
- 缺失、重复和未知 tags；
- 非法 enum、pubkey、UUID、event ID 和文本长度；
- V0/V1 builder 不混用；
- V1 拒绝 `42102/42104` 且永不创建 `meeting_rounds`，V0 拒绝 `42105..42109`；
- V0 sweeper 永不选择 V1 Session，两个 DB 模块都 fail closed 校验 policy；
- V1 speech 的 Handoff tags 必须 all-or-none；
- participant type 缺失/冲突时 Create fail closed，不默认 Human；
- State 数组 canonical order、effect enum/from/to shape 和 last-attempt outcome 映射；
- Granted State 要求 `grant.basis_speech_revision == speech_revision`，且
  `next_action_at_ms = min(soft_lease_expires_at_ms, hard_deadline_ms)`；
- 复合 SAY fixture 同时包含 grant spoken、speech accepted、Intent consumed、旧/新
  Handoff 与下一 Offer effects，顺序唯一；

### 17.2 DB 状态机

至少覆盖：

- V1 创建后 moderator + moderator_idle；
- Intent submit/refresh CAS/withdraw/duplicate；
- 一人一个 pending Intent；
- moderator select self/other、reject reason；
- moderator dismiss open Handoff、active-attempt conflict 与 reason；
- moderator self priority、连续 self speech 限制与原子 Deferral；
- pending moderator self Intent 会阻止选择其他 Intent 或 open Handoff；
- moderator-self Offer/Grant 的每一种终态都会解除 Deferral；
- Human FIFO 与每人一个 Request；
- Human moderator 不能用 Human Request 绕过 moderator-self 规则；
- Human Request 抢占 non-human_request-sourced Offer，但不抢占 Grant；
- Offer ACK/Decline/timeout；
- Agent 5 秒与 Human 15 秒 deadline；
- Progress 续 soft lease但不越过 hard deadline；
- soft lease 恰等于 hard deadline 或恢复时两者都 overdue，只产生 hard_expired；
- SAY/YIELD/soft expiry/hard expiry；
- source Intent 的 selected/consumed/stale；
- unrelated pending Intent 在 Handoff/Human Grant 后保留；
- speech + Handoff 原子性；
- source-shape matrix 的六种合法组合和所有非法 source/depth_mode 组合；
- 新 Handoff 保持 open，Offer Decline/timeout、抢占和 Grant Yield/Expiry 只终结 attempt；
- open Handoff 上限 32、达到上限的可解释 blocked 结果与 Dismiss 后释放容量；
- moderator Select open Handoff → ACK → SAY 后标记 answered，并保留一对多 attempt 历史；
- moderator Select open Handoff 不消费目标无关 pending Intent；
- moderator-self speech 的 Handoff 从深度 0 开始；
- moderator 作为普通深链 target 发言后再 Handoff，不会重置深度；
- direct-Handoff ACK 只暂占深度，目标 Yield/soft/hard Expiry 都恢复 previous depth；
- depth 4 的第五席目标 Yield 后不触发 forced return，后续合法 Handoff 仍可成为第五次；
- 第五次 Handoff 可发言，第六次不能创建；
- Recall 和 forced return；
- 3 分钟 moderator deadline 不被新 Intent 重置；
- moderator fallback 不能绕过连续 self speech gate；
- ordinary Intent 已 fallback-attempted、self Intent 尚未尝试且连续 self gate 生效时进入
  idle，不再次自动选择 self；
- Relay 重启并修改默认配置后，既有 Session 仍使用冻结 timing profile；
- fallback 不无限重试同一 Intent basis；
- fallback Intent 在 ACK/SAY/Yield/Expiry 上同样进入 selected/consumed/stale；
- fallback 永不自动重试 open Handoff；
- fallback 选中 moderator self Intent 时仍使用 `turn_role=moderator_self`；
- Directed Handoff 指向 moderator 时仍使用 `turn_role=participant`；
- Meeting End 从每个 phase 进入终态；
- participant/moderator 撤权从每个 phase 终止 V1；
- security revocation job 可重试并逐 Session 结束；NIP-IA archive 不触发该 job；

### 17.3 并发边界

用独立连接同时提交：

- ACK 与 Human Request；
- ACK 与 Offer timeout；
- SAY 与 hard deadline；
- SAY 与 End；
- SAY/ACK 与 participant_revoked；
- Progress 与 soft expiry；
- 两个 Human Request；
- 两个 moderator 同时 Select 同一 open Handoff；
- 两条 speech 消费同一 Grant；
- refresh 与 reject/withdraw；
- active Intent Offer 与 refresh/withdraw/reject；
- active human_request-sourced Offer 与 Request withdraw。

每次只允许一个 canonical 结果，并验证 revision 与 outbox 没有缺口或重复。

### 17.4 Relay 与恢复 E2E

- 私有名单内多订阅者收到相同事件；
- 非参会者 Query/REQ/COUNT 均无泄露；
- 非参会者对 `42105..42109` 的每种写操作都在 DB fail closed 且不泄露 object；
- live-first + backfill 去重；
- 超过 1000 份 State 后分页回填，验证 `state_revision` 无缺口、event ID 去重且最终收敛；
- 初始 Handoff attempt timeout → moderator reselect 后 Decline → 再次 reselect 后
  ACK/Yield → 再次 reselect 后 ACK/SAY；断言同一 `handoff_id` 下有多个独立
  offer/grant ID、attempt count 正确、最终 answered，且目标无关 Intent 始终未消费；
- 仅有 open Handoff 时保持 `moderator_idle`，sweeper/fallback 永不自动创建 Offer，但
  moderator CLI 可从 idle 显式 Select 并创建 Offer；
- Relay 在 Offered、Granted、Moderator Control 时重启；
- ACP 在 ACK 发送前、发送后未收到响应、SAY 发送后未收到响应时重启；
- outbox 重投不产生双 Grant/双 speech；
- sweeper 多实例并发；
- due batch 中一个坏 Session 不饿死其他 Session；
- V0 全部既有 E2E 保持通过。

### 17.5 Agent 验收

后端最终场景至少使用 2 Human CLI 身份和 2 Agent：

1. Agent 异步提交 Intent；
2. Agent moderator 在别人发言时准备 ModeratorPlan；
3. Human 请求下一席；
4. Agent deterministic ACK 后使用正常上下文工具调查证据；
5. Agent speech 携带 Handoff reason；
6. 目标 Agent Decline 和重试恢复；
7. 连续五次 Handoff 强制归还；
8. moderator 故障触发 3 分钟 fallback；
9. 任一 Agent 变慢时其他参会者仍可提交 Intent/Human Request；
10. `handoff-to=self` speech 先于 State 到达时，目标不会同时提交重复 voluntary Intent
    和 ACK Handoff Offer；
11. 会议结束后历史完整且只读。

## 18. 可观测性

新增建议 metrics：

```text
meeting_v1_transition_total{from,to,reason}
meeting_v1_offer_total{allocation_source,turn_role,outcome,participant_type}
meeting_v1_offer_ack_latency_seconds
meeting_v1_grant_total{allocation_source,turn_role,outcome}
meeting_v1_grant_duration_seconds
meeting_v1_progress_total{stage}
meeting_v1_moderator_decision_seconds{outcome}
meeting_v1_intent_age_seconds{outcome}
meeting_v1_recovery_lag_seconds{deadline_type}
meeting_v1_outbox_delivery_total{outcome}
```

结构化日志包含：

- community/session；
- policy；
- event/object ID；
- revision；
- transition reason；
- deadline type 与 recovery lag。

日志和 metrics 不记录 speech、Intent summary、Handoff reason 或 rejection reason 正文。

## 19. 发布与兼容策略

### 19.1 灰度

建议增加只控制“新建 V1”的开关：

```text
BUZZ_MEETING_V1_CREATE_ENABLED
```

默认值为 `false`。

已存在的 V1 Session 无论开关是否关闭，都必须继续被 Relay runtime 服务。关闭开关只阻止
新建，不能把现有 V1 变成不可恢复状态。

集群发布顺序：

1. 部署 additive migration；
2. 部署所有能够读取但尚不创建 V1 的 Relay；
3. 验证 V0 回归和 V1 runtime；
4. 全部节点升级后开启 V1 Create；
5. 使用 CLI/ACP 灰度；
6. 产品前端完成后再讨论默认创建策略。

### 19.2 回滚

- V0 路径始终保留；
- 关闭 V1 Create 可以停止扩大范围；
- 已创建 V1 必须由支持 V1 的 Relay 继续服务；
- 不执行 destructive down migration；
- 若必须降级二进制，应先结束所有活动 V1 Session，不能让旧 Relay 误处理新 kind。

### 19.3 默认策略

在产品前端尚未设计前：

- 现有 Create 行为默认保持 V0；
- CLI 通过显式 `--policy moderated-baton-v1` 创建 V1；
- 是否让新的产品会议默认使用 V1，留到后端验收和前端设计完成后决定。

## 20. 分阶段开发规划

### 阶段一：协议与数据基础

目标：

> 建立不会影响 V0 的 V1 wire contract 和持久化骨架。

开发内容：

- 新 kinds、协议类型、严格 SDK builders 和 conformance fixture；
- 机械同步 Desktop/Mobile 的共享 kind 常量，但不实现前端行为；
- additive migration 与 `schema/schema.sql`；
- V1 Create、moderator 和 participant type 冻结；
- V1 manual End、V0/V1 policy 路由和新增的耐久 security-revocation job 骨架；
- `MeetingBatonState`、Intent、Request、Offer、Grant、Handoff 表；
- 新 `meeting_baton` 模块及初始 `moderator_idle` State；
- Session policy dispatch，保证 V0/V1 不混用；
- V0 DB mutation 和 sweeper 的 policy fail-closed 防线；
- `BUZZ_MEETING_V1_CREATE_ENABLED` 存在且默认关闭。

交付：

- 开启测试开关后，可通过 SDK/CLI 创建、查询和结束一个 V1 Session；
- 数据库中有完整名单、moderator、policy 和初始 Relay State；
- migration、schema、SDK 和 DB tests 通过；
- 全部 V0 测试保持通过。

阶段验收不要求完整发言闭环。

### 阶段二：Baton 协议闭环（已交付）

目标：

> 不依赖任何产品前端或 LLM，仅使用 CLI 完成一次完整主持式会议。

开发内容：

阶段二拆成三个可以独立合并和验收的 checkpoint：

#### 2A：最小 Baton 闭环

- Intent submit/refresh/withdraw；
- Moderator select/reject；
- Offer ACK/Decline；
- Grant Yield；
- Grant-bound SAY；
- CLI 完成 `Select → Offer → ACK → Grant → SAY/Yield`。

交付条件：单 moderator、两个普通 CLI 身份可以连续完成多席发言，单 Offer/Grant/
speech 不变量和基础幂等测试通过。

#### 2B：优先级与直接接力

- Human Request FIFO 与 Offer 抢占；
- Moderator Recall 和 self Select 原子 Deferral；
- 原子 Directed Handoff；
- open question 与 Offer/Grant attempt 分离，支持 moderator 显式重新选择或关闭 Handoff；
- 冻结的 open Handoff 容量上限；
- 默认五次上限和 forced return；
- moderator-self、fallback、Human 和 ordinary turn role；
- active source Intent/Request 的取消并发。

交付条件：Human 优先、Recall、Handoff reason、未回答问题的重新选择/关闭/容量上限、
深度上限及全部覆盖/取消场景可由 CLI 验证。

#### 2C：时间、恢复与并发

- Grant Progress；
- Offer targeting Agent/Human、soft/hard Grant、3 分钟 moderator deadline；
- deterministic fallback 与 fallback attempt 去重；
- sweeper、lazy recovery、command receipt 和 outbox fan-out；
- Relay restart、多实例 recovery、security-revocation worker；
- CLI 的完整 V1 命令面和并发 E2E。

交付条件：所有 deadline、recovery、restart、privacy 和 race 测试通过，迟到 command
能够提交 recovery 而不发布自身。

交付：

- 多个 CLI 身份可共享控制日志和 speech timeline；
- Human、moderator 和普通参与者完成全状态闭环；
- Relay/DB 并发、deadline、restart 和隐私 E2E 通过；
- 证明 5 分钟是 hard cap，提前 SAY 会立即推进。

这是 V1 后端协议可行性的核心里程碑。

### 阶段三：普通 Agent 参会（已交付）

目标：

> 普通 Agent 能异步表达发言意图，并只在真正获得 Grant 后生成完整发言。

开发内容：

- V1 共享同步器和 ledger 升级；
- Participant Intent Controller；
- 确定性 Offer ACK/Decline 与本地容量预留；
- Granted Speech Controller；
- 独立 Progress heartbeat；
- advisory 工具策略和正常上下文工具继承；
- prepared ACK/SAY/YIELD 的重试和重启恢复；
- Participant 与 Granted prompts。

交付：

- Agent 不为每个控制事件调用 LLM；
- 未获 Grant 的 Agent 不生成完整候选发言；
- 获 Grant 的 Agent 可调用正常上下文工具并在 5 分钟内 SAY/YIELD；
- Agent 慢、离线、ACK 丢失和 late result 都不冻结会议；
- 普通 Agent E2E 与 token/latency metrics 可观察。

### 阶段四：Agent 主持人

目标：

> Agent moderator 能在其他人发言期间异步规划，并在 Control Token 返回时连贯调度。

开发内容：

- ModeratorPlan 私有模型和 revision revalidation；
- Moderator prompt；
- pending Intent/open Handoff 排名、Reject + 单一 next action 的结构化输出；
- 带原因的 Handoff Dismiss proposal、逐项重验和 active-attempt conflict 处理；
- Granted > Moderator > Participant 的本地调度优先级；
- 3 分钟 deadline 的本地 safety budget；
- stale plan 丢弃、Human 优先和 Relay fallback 协作；
- moderator 重启恢复。

交付：

- ModeratorPlan 的 AgendaRanking 通常在 Control Token 返回前已经 ready；
- Human Request 永远能够覆盖 ModeratorPlan；
- stale ModeratorPlan 不产生错误选择；
- Agent moderator 能在容量耗尽前关闭 stale open Handoff，且不会关闭活动 attempt；
- moderator LLM 故障时 Relay 在 3 分钟后确定性继续会议；
- Agent moderator 与普通 Agent 可在同一进程/模型池内稳定运行。

阶段三和阶段四可以在共享同步器、ledger schema 和 Stage 2 协议冻结后部分并行开发；最终
验收仍以二者集成为准。

### 阶段五：后端综合验收与发布准备

目标：

> 把协议正确性提升为可灰度运行的后端能力。

开发内容：

- 2 Human + 2 Agent 综合场景；
- 并发边界、故障注入、Relay/ACP 重启和 outbox 重投；
- 长会议与多 Session 负载；
- metrics、日志和告警；
- V0/V1 共存及灰度开关；
- 运维、测试和协议文档收口。

交付：

- `just ci` 通过；
- 涉及 Relay/DB/Auth 的完整 `just test` 通过；
- V0 回归、V1 fixture、DB、Relay E2E 和 ACP E2E 全部通过；
- 已知 deadline 下没有双 Offer、双 Grant、双 speech 或 due-row starvation；
- 形成后端协议版本说明和前端可依赖的稳定事件契约。

完成此阶段后，Meeting V1 后端才视为交付完成。

## 21. 后端完成后的前端讨论输入

本次不设计前端，但后端最终应向后续 Desktop/Web/Mobile 设计提供稳定能力：

- 当前 State snapshot；
- 完整 participant type 和 moderator；
- SpeechIntent 列表及拒绝原因；
- 未回答 Handoff 列表及原始 reason、最近一次 attempt 结果；
- Human Request FIFO；
- Offer target、来源、Handoff 原因和 ACK deadline；
- Grant holder、soft/hard deadline 和 progress；
- Recall/forced return/handoff depth；
- Speech timeline 与 control log 的独立订阅；
- 对每个可执行动作的明确权限和 conflict 结果。

前端可以决定如何展示和交互，但不能自行改变 Relay 的优先级、deadline、Grant 或
Human FIFO。

## 22. 后端完成定义

Meeting V1 后端完成需要同时满足：

1. 可以显式创建 `v=2 + moderated-baton-v1` Session；
2. V0 Session 行为完全不变；
3. Human 与 Agent 共享同一私有会议事件空间；
4. Intent、Moderator、Human、Offer、Grant、Progress、SAY、Yield、Handoff 和 End
   全部有持久、幂等、可恢复的闭环；
5. 任何时刻至多一个 Offer 或 Grant；
6. Human 下一席、Recall 和五次 Handoff 上限由 Relay 权威执行；
7. Agent ACK 不调用 LLM，完整发言只在 Grant 后生成；
8. Agent 能在 Granted Turn 中调用正常工具获取项目上下文，并由 prompt 约定不执行写操作；
9. Grant 最多 5 分钟且提前完成立即推进；
10. moderator 最多等待 3 分钟且有确定性 fallback；
11. Relay/ACP 重启和网络结果不明确不会产生重复 speech；
12. CLI、fixture、DB、Relay、ACP 和 V0 regression tests 全部通过；
13. Desktop、Web、Mobile 可以只依赖稳定后端契约开始单独设计。
