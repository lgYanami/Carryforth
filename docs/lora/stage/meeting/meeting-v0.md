# Meeting V0：Human 与 Agent 的共享文字会议室

> 状态：第一版设计
>
> 本文定义 Meeting V0 的产品边界、核心语义、Buzz 映射、Agent 行为和验收标准。
> 它不定义候选决议、人类确认、决定写回或其他正式治理程序。

## 1. 文档目的

[项目定位与目标](../../project-positioning.md)要求 Human 与 Agent 围绕项目形成持续的
共享协作空间；[项目空间宪法](../../project-space-constitution.md)进一步把正式会议定义为
项目的重要治理机制和人类介入点。完整的正式会议最终需要议题、必要参与角色、候选决议、
人类确认和项目写回，但这些能力都依赖一个更基础的问题先得到验证：

> Human 与 Agent 能否进入同一个共享空间，以对等身份进行可靠的多对多文字交流？

Meeting V0 只回答这个问题。

第一版不尝试证明“会议可以形成有效决定”，而是先证明以下基础能力成立：

1. 多名 Human 和 Agent 可以进入同一个会议室；
2. 每名参会者都可以自主申请发言权，只有本轮抢夺成功者可以发言；
3. 每名参会者都可以看到同一份完整消息记录；
4. 每条消息都有可验证的作者身份；
5. 每名参会者都可以看到会议的完整参会者名单；
6. 断线、重连和 Agent 运行实例重启不会破坏上述事实。

Meeting V0 与 [Project View](../project-view/project-view.md) 可以并行开发。它只依赖
Buzz 已有的 Community、身份、私有 Channel、消息和 Agent Harness，不依赖 Project
View 对象先完成。

## 2. 核心结论

Meeting V0 采用以下模型：

> 一个 `MeetingSession` 是一个带有会议标记的、私有的、平铺文字流 Channel。

产品界面中可以直接称其为“会议”或“会议室”；领域与协议层使用
`MeetingSession`，以便将来与具有治理效力的 `FormalMeeting` 区分。

```text
FormalMeeting（未来）
    └── MeetingSession（本次 V0）
            ├── Participant Roster
            ├── Relay-authoritative SpeechRound / Floor State
            └── Shared Message Log
```

这意味着：

- V0 不建立第二套消息系统；
- V0 不直接复用带音频状态机的 Huddle；
- V0 复用 Buzz 的私有 Channel、NIP-29 成员关系、签名消息、历史查询和实时分发；
- V0 增加会议身份、会议生命周期、Relay 权威发言轮次和 Agent 会议模式；
- 会议中的聊天不会自动成为候选决议或项目决定。

## 3. V0 要证明什么

### 3.1 多对多

会议不是“一名 Human 向一个 Agent 提问”的会话，也不是“主持人向多个 Agent
广播任务”的控制面。

只要仍是参会者，任意 Human 或 Agent 都具有相同的基础通信能力：

- 读取会议消息；
- 查看当前轮次和发言权状态；
- 自主决定申请或本轮不申请发言权；
- 在抢夺成功后发送一条本轮消息；
- 看到消息作者；
- 读取参会者名单。

Human 与 Agent 拥有相同的基础抢夺资格，但不拥有绕过发言权直接写入消息流的资格。
`@mention` 可以影响参会者是否想发言，但不直接授予发言权。

### 3.2 共享

“共享”包含两个相互独立的事实：

- **共享名册**：所有参会者读取同一份权威参会者集合；
- **共享消息流**：所有参会者最终收敛到同一组已接受消息。

在线状态、窗口是否打开和 Agent 进程是否正在运行，都不改变参会者身份。

### 3.3 主动申请发言

“主动发言”表示：

> 参会者可以在没有收到点名、提问或上一条定向消息的情况下，自主判断自己是否应当发言，
> 并申请本轮发言权；只有 Relay 授予发言权后才能发布会议消息。

对于 Human，可以随时准备草稿并申请发言；抢夺失败时草稿保留，下一轮由 Human 决定
是否重新申请。

对于 Agent，观察消息和公开发言分成两个阶段：

1. **发言意图判断**：基于完整 Inbox 输出结构化的 `CLAIM` 或 `PASS`；
2. **发言权执行**：只有 `CLAIM` 抢夺成功后，Harness 才允许 Agent 发布一条签名消息。

Agent 的 `PASS` 是明确、可由 Harness/Observer 观察并恢复的调度结果，不再依赖“模型也许
会保持沉默”；它不是其他参会者可见的会议事件。发言权控制谁可以进入公开时间线，发言
意图判断控制 Agent 是否应当参与本轮抢夺。

## 4. MeetingSession 模型

V0 的逻辑模型如下：

```text
MeetingSession
- session_id
- title
- description?
- source_channel_id?
- host_pubkey
- status: active | ended
- participants[]
- current_round
- floor_revision
- floor_policy_version
- created_at
- ended_at?
- ended_by?
- schema_version: 1
```

字段语义：

| 字段 | 语义 |
|---|---|
| `session_id` | 会议场次稳定身份；V0 直接等于底层 Channel UUID |
| `title` | 会议室标题，不等同于正式会议议题 |
| `description` | 可选的简短背景，不承担完整上下文或议程职责 |
| `source_channel_id` | 可选来源引用；只建立导航关系，不继承或扩张权限 |
| `host_pubkey` | 创建会议的签名身份；V0 同时是会议室 Owner |
| `status` | 创建事务成功即为 `active`；结束后进入终态 `ended` |
| `participants` | 权威参会者集合，由会议室成员关系提供 |
| `current_round` | Relay 权威维护的当前发言轮次 |
| `floor_revision` | Relay 每次改变共享发言权状态时递增的 Session 级版本 |
| `floor_policy_version` | 本 Session 使用的发言权仲裁策略版本 |
| `schema_version` | 会议标记与元数据的版本 |

V0 的一个 Session 只对应一个会议室，一个会议室也只属于一个 Session。会议室不复用。

每个活动 Session 恰好具有一个当前发言轮次：

```text
SpeechRound
- session_id
- round_number
- phase: open | claiming | granted | closed
- claims[]
- claim_deadline?
- holder_pubkey?
- grant_event_id?
- lease_expires_at?
- outcome?: spoken | expired | ended
- speech_event_id?
- policy_version: uniform-v0
```

`round_id` 由 `(session_id, round_number)` 唯一确定。SpeechRound 是 Relay 的共享协议
状态，不是某个 Agent 本地的推理轮次。

## 5. 参会者与身份

### 5.1 参会者的定义

V0 中：

> 参会者 = 会议室的有效成员。

这是一项持久的授权和协作事实，不是 Presence 状态。

界面可以显示“参会者”，但不能把以下情况解释为“已经离会”：

- Human 关闭会议页面；
- Desktop 断线；
- Agent 运行实例退出；
- 某名参会者暂时没有发言；
- 某名参会者当前不在线。

### 5.2 首版名单

V0 在创建会议时一次确定完整初始名单，并在会议激活后冻结名单。

产品允许任意有效 Community 成员（Human 或 Agent）发起会议。一个会议至少包含两名
参会者；V0 的能力验收必须同时包含 Human 和 Agent，但协议不禁止纯 Human 或纯 Agent
的文字 Session。为限制每轮 Claim 和 Agent 意图判断的成本放大，V0 默认最多 12 名参会者，
其中最多 4 名 Agent；限制由 Relay 配置，但不能由客户端绕过。

首版不支持：

- 中途邀请；
- 中途踢出；
- 主持权转移；
- 把“关闭会议页面”解释为退出参会者集合。

选择固定名单是为了让第一版集中验证共享房间本身，并使“所有参会者从开始起拥有同一份
完整记录”成为简单、清晰的不变量。完整名单必须和会议室在同一个 Relay 事务中创建；
不能先创建可见 Channel，再通过多个独立成员事件逐个补齐。需要不同名单时创建新的
Session。

创建者自动进入名单并成为 Owner。其他 Human 使用 `member` 角色，Agent 使用 `bot`
标识；`bot` 只表达身份类型。V0 中 Owner、Human member 和 Agent bot 具有相同的 Claim
资格与等概率中奖权重，但都不能直接发言。角色由 Relay 根据权威身份目录确认，不能相信
创建事件中自报的 Human/Agent 类型。

### 5.3 作者身份

消息作者必须由事件签名公钥确定，不能相信消息正文或客户端提交的 `author` 字段。

每条消息至少展示：

- 显示名称；
- Human 或 Agent 标识；
- 发送时间；
- 可查看的稳定公钥身份。

显示名称只是展示信息。两名参会者可以使用相同名称，系统仍必须根据公钥区分作者。
当 Profile 无法解析时，使用稳定的公钥摘要作为回退，不能显示成无法追责的“匿名用户”。

## 6. 消息模型

### 6.1 单一平铺消息流

V0 使用一个平铺时间线：

```text
Participant
    └── signed kind:9 message
            ├── h = meeting room channel UUID
            ├── meeting-round = current round number
            └── meeting-grant = relay-signed grant event ID
```

V0 不引入会议专用正文消息 kind。主消息时间线只写入、查询和订阅 kind `9`；Claim 和
Round State 使用独立控制事件。会议身份来自消息所属会议室。kind `40002` 不属于
Meeting V0 的规范消息集合，也不能由通用客户端写入会议室。由于 V0 会议室都是新建的，
不需要承担旧消息 kind 的历史兼容。

第一版界面不提供线程、消息编辑、消息删除、定时消息或仅部分成员可见的会内私信。
这些能力不是多对多会议可行性的必要条件，并会使“完整消息”产生额外语义。
Relay 的 meeting policy 必须把允许写入的消息集合限制为不带 thread/reply 引用的
kind `9`，并拒绝 kind `40002`、编辑、删除、Diff、定时消息以及其他通用 Channel
消息变体，避免不同客户端得到不同的规范日志。

kind `9` 只是最终发言。Claim、Round State 和 Grant 属于独立的会议控制日志，不混入
主消息时间线；所有参会者仍能查询这些事件并看到当前轮次、抢夺状态和发言权持有者。

### 6.2 “完整消息”的准确含义

V0 对完整消息作如下定义：

> 会议稳定后，每名参会者都能查询到从会议开始到当前为止相同的、Relay 已接受的会议
> 消息 event ID 集合，并能按相同规则形成确定的显示顺序。

这里的“消息记录”特指成功获得发言权后写入的 canonical speech log。Claim、Round State、
Grant 及超时转换形成另一份同样共享、可恢复的 floor control log；控制日志不计作参会者
发言，但可以用于解释每条消息为何有权进入时间线。

它不承诺网络传输层的 exactly-once。客户端必须：

1. 在订阅实时消息的同时分页读取历史；
2. 合并历史结果和实时结果；
3. 以 event ID 去重；
4. 在断线重连后从最后确认的游标补齐；
5. 使用 `(meeting-round ASC, event_id ASC)` 形成稳定显示顺序。

每轮最多只有一条规范消息，因此 `meeting-round` 是 Relay 验证过的权威发言顺序。
`created_at` 只用于展示，并继续接受 Buzz 现有的时间偏差校验；不能让客户端时间戳改变
会议顺序。Round State 则按 `floor_revision` 归约，不能按事件到达顺序覆盖当前状态。

“完整”也不表示必须把全部历史一次性放入模型上下文。完整日志属于会议室；Agent 的
单次上下文可以只包含近期窗口，但必须获得历史游标，并能通过 CLI 分页读取完整记录。

### 6.3 消息归属

一条消息只有在以下条件同时满足时才属于会议记录：

- 签名有效；
- 作者是会议参会者；
- `h` tag 指向该会议室；
- 会议仍处于 `active`；
- `meeting-round` 指向当前 `granted` 轮次；
- `meeting-grant` 指向该轮 Relay 签名的有效授权事件；
- 作者就是当前发言权持有者，且授权租约尚未到期；
- 该授权尚未使用过；
- Relay 已接受并持久化该事件。

本地草稿、发送失败、被 Relay 拒绝或只存在于某个 Agent 内存中的内容，都不属于共享
会议记录。消息被接受时，Relay 必须在同一事务中消费发言权、结束当前轮并创建下一轮，
避免同一 Grant 被并发使用两次。

写入 Handler 必须先按 `(community, event_id)` 查询既有结果，再校验当前 Round：

- 同一个已接受 signed event 因 ACK 丢失而重试时，返回原 accepted 结果，即使会议已经
  进入下一轮；
- 同一 Grant 提交第二个不同 event ID 时，以 `grant_consumed` 拒绝，并返回已经接受的
  speech event ID；
- 格式错误、签名错误或其他无效消息不会消费 Grant；holder 可以在 lease 内修正后重试。

## 7. 生命周期

### 7.1 状态机

```text
create request
      ↓ Relay 单事务提交房间、名单和创建事件
    active
      ├── open → claiming → granted → closed → next open
      ↓ Owner 结束会议
     ended
```

`preparing` 只可以是发起客户端尚未得到 Relay 回执时的本地 UI 状态，不是共享领域状态。
Relay 要么一次接受完整 Session，要么拒绝整个创建事件：

- 成功时，房间、固定名单、创建事件和 `active` 状态同时可见；
- 成功事务同时创建 `round_number=1` 的开放发言轮次；
- 失败时，不留下 Channel、成员关系、会议事件或成员通知；
- 任一初始参会者无效、无权访问来源或拒绝 Agent Channel Add Policy，都会使整个创建
  失败；
- Relay 回执必须指出失败身份和原因，便于发起者调整名单后重新提交。

Session 本身不设置活动 TTL。文字会议允许异步参与，“一段时间没有消息”不等于会议
已经结束；但 Claim 竞争窗口和已授予的发言权必须分别具有短 deadline 和 lease。

### 7.2 创建提交

创建事件本身就是激活边界。事务提交之后：

- Desktop 可以发现和打开会议；
- 所有参会 Agent 都能通过成员通知或启动扫描发现会议；
- Relay 开始接受 Round 1 的发言权 Claim；
- 创建事件中的参与者快照与成员投影必须完全一致。

成员通知和实时 fan-out 必须在事务提交后发生。若进程在提交后、通知前崩溃，Agent
仍必须能在重启扫描中发现该会议，不能把一次性通知当作唯一事实来源。

### 7.3 结束

V0 由会议 Owner 结束；Community 管理员保留恢复性结束能力。

结束后：

- 状态不可回到 `active`；
- 当前 Claim 窗口和未使用 Grant 立即失效；
- Relay 拒绝新的会议消息；
- 会议记录继续对原参会者可读；
- 会议进入只读归档；
- 重复结束请求是幂等的。

结束与并发消息的边界由 Relay 的接受顺序确定：结束生效前已经接受的消息保留在记录中，
结束生效后到达的消息被拒绝。

结束事件、当前 Round 的 `closed/ended` 状态和 Channel `archived_at` 必须在同一个事务中
提交。
该事务同时递增 `floor_revision`。Claim 接受、winner 选择、lease sweeper、Say 和 End
都锁定同一 Session/Round 或使用带预期状态的 CAS；End 一旦提交，任何延迟 worker 都因
`session.status != active` 而 no-op，不能再产生 `granted` 或下一轮 `open`。
客户端同样把 `ended` 视为高于任何 Floor phase 的终态；晚到的旧 Round State 不能重新
启用 Claim 或 Send。
会议状态由持久投影恢复，不依赖某个 Desktop 或 ACP 的内存状态。

### 7.4 Community 撤权

固定名单不能阻止更高层安全撤权。参会者被移出 Community、身份被归档或授权被管理员
紧急撤销时：

- 该身份立即失去会议读取、订阅和发送权限；
- 原始参与者快照不被改写，而是标记该身份已经撤权；
- Relay 在同一撤权流程中记录一个系统签名的 Meeting End，并以
  `participant_revoked` 原因结束活动 Session；
- 其余参会者仍能读取结束前的完整记录。

这项例外用于保护权限边界，不属于 V0 的动态名单管理。

## 8. 在 Buzz 上的实现映射

### 8.1 复用关系

| Meeting V0 概念 | Buzz V0 映射 |
|---|---|
| MeetingSession / 会议室 | `private + stream` Channel |
| Session ID | Channel UUID |
| 会议标记 | `KIND_MEETING_CREATE` 与 `room_kind=meeting` 投影 |
| 标题、描述 | Channel name / about |
| Host | Channel creator / owner |
| 参会者 | NIP-29 Channel members |
| Human / Agent 类型 | member role、Agent Profile 与身份目录 |
| 会议消息 | kind `9`，使用 `h=<session_id>` |
| 参会者投影 | Relay-signed kind `39002` |
| 激活 | `KIND_MEETING_CREATE` 与完整名单原子提交 |
| 当前会议状态 | `room_kind=meeting` 与 Channel `archived_at` 的持久投影 |
| 发言权申请 | participant-signed `KIND_MEETING_FLOOR_CLAIM` |
| 轮次与授权 | Relay-signed `KIND_MEETING_ROUND_STATE` 与持久 Round 投影 |
| 当前发言权读取 | 现有 Channel Detail 中的 Floor 投影 + kind `42103` 历史/实时事件 |
| 结束 | `KIND_MEETING_END` 与 `archived_at` 原子提交 |
| 历史与实时消息 | 现有 Query + WebSocket subscription |

会议标记是稳定的机器语义，不能通过标题前缀或自由文本 `purpose` 猜测。

V0 不把 `meeting` 加入 `ChannelType`。`stream` 表达消息组织方式，`meeting` 表达该
Channel 承载的领域用途，这两个维度应当分开。

`channels` 读取模型增加稳定的 `room_kind` 字段，值至少包含 `standard | meeting`。
Relay 从 Meeting Create 事件产生这个投影，Channel Detail、Channel Summary 和 Agent
可用的查询结果都必须返回它。不能要求客户端在重启后扫描自由文本或依赖本地缓存识别
会议。

Meeting Channel Detail 还必须返回当前 `round_number`、`floor_revision`、phase、
deadline、canonical Claim IDs/claimants、当前查看者的 canonical Claim ID（若有）、
holder、Grant ID、lease 和前序 Round 结果引用。CLI、ACP 与 Desktop 用这份权威快照完成
同步，再用 kind `42103` 增量推进；这只是扩展现有读取模型，不增加会议专用 HTTP
endpoint。

V0 分配四个新事件 kind，并首先登记到 `buzz-core/src/kind.rs`：

```text
KIND_MEETING_CREATE      = 42100
KIND_MEETING_END         = 42101
KIND_MEETING_FLOOR_CLAIM = 42102
KIND_MEETING_ROUND_STATE = 42103
```

原子创建事件形状：

```json
{
  "kind": 42100,
  "tags": [
    ["h", "<session-uuid>"],
    ["name", "<title>"],
    ["about", "<optional-description>"],
    ["v", "1"],
    ["source", "<optional-source-channel-uuid>"],
    ["p", "<participant-1-pubkey>"],
    ["p", "<participant-2-pubkey>"]
  ]
}
```

创建事件作者隐含在名单中并成为 Owner；`p` tags 表达其余完整参与者集合。Relay 根据
权威身份目录把成员投影为 `member` 或 `bot`，不接受客户端自报的身份类型。

结束事件形状：

```json
{
  "kind": 42101,
  "tags": [
    ["h", "<session-uuid>"],
    ["e", "<meeting-create-event-id>"],
    ["reason", "manual"]
  ]
}
```

MeetingSession 生命周期由这两个不可变事件及其持久投影恢复：

```text
active = valid MeetingCreate exists && archived_at == null
ended  = valid MeetingEnd exists    && archived_at != null
```

发言权申请事件：

```json
{
  "kind": 42102,
  "tags": [
    ["h", "<session-uuid>"],
    ["meeting-round", "<round-number>"]
  ],
  "content": ""
}
```

Claim 不携带候选发言内容；草稿在获权并形成 kind `9` 之前只保留在参与者本地。

Relay-signed Round State 使用 kind `42103` 表达 `open`、`claiming`、`granted` 和
`closed` 转换。`granted` 状态事件的 event ID 同时作为不可伪造的 Grant ID：

```json
{
  "kind": 42103,
  "tags": [
    ["h", "<session-uuid>"],
    ["meeting-round", "<round-number>"],
    ["floor-revision", "<monotonic-session-revision>"],
    ["phase", "granted"],
    ["holder", "<winner-pubkey>"],
    ["policy", "uniform-v0"]
  ],
  "content": "{\"lease_expires_at_ms\": 1730000000000, \"claim_event_ids\": [\"<claim-event-id>\"]}"
}
```

所有 Round State 都携带 `floor-revision`。`closed` 事件在 content 中记录
`outcome=spoken | expired | ended`，并在 `spoken` 时记录 accepted speech event ID；
后续 `open` 使用更大的 revision，并重复 `previous_round`、`previous_outcome` 和可选
`previous_speech_event_id`。因此即使 outbox 乱序，客户端也知道开放 Claim 前必须补齐
哪条前序发言。

每份首次接受的 canonical Claim 也产生一个新的 `claiming` Round State，content 携带
截至该 revision 按 event ID 排序的完整 canonical Claim ID 集合。第一份 Claim 执行
`open → claiming`；后续 Claim 执行 claim 集合严格扩大的 `claiming → claiming`。
相同 event ID 重试或同一身份的冲突 Claim 不推进 revision。

无需增加会议专用 HTTP endpoint；写入继续通过签名事件，读取继续通过 Buzz 的通用
Query/Count/WebSocket 能力。

### 8.2 发言权状态机

V0 采用以下权威状态机：

```text
open
  ↓ 第一份有效 Claim
claiming（短竞争窗口）
  ↓ Relay 在有效 Claim 中等概率选出一人
granted（唯一 holder + 限时 lease）
  ├── holder 发出一条有效消息 ──→ closed/spoken ──→ 下一轮 open
  └── lease 到期未发言 ─────────→ closed/expired ─→ 下一轮 open
```

V0 把“抢夺”明确解释为“短窗口报名 + 有效 Claim 等概率仲裁”，不是网络包最先到者直接
获权。这样可以降低纯网络延迟造成的偏置，并为后续 Leader 权重、身份权重和 Human 直获
策略保留同一套 Claim/Grant 协议。

具体规则：

1. Meeting Create 事务同时创建 Round 1 的 `open` 状态；
2. `open` 没有 Claim 时可以无限保持安静；
3. 有效 Claim 必须来自活动参会者，并且指向当前 `open` 或尚未截止的 `claiming`
   Round；旧轮、未来轮和截止后的 Claim 都被拒绝；
4. 第一份有效 Claim 启动默认 3 秒、Relay 可配置的竞争窗口，使 Human 和较慢 Agent
   都有实际参加机会；
5. deadline 边界只使用 Claim 事务锁定 Round 后取得的数据库 receive time：
   `received_at < claim_deadline` 才进入候选集合；到达 deadline 的请求先触发本轮结算
   再被拒绝；
6. 每名参会者每轮最多提交一份 Claim；相同 signed event ID 重试返回原 accepted 结果，
   同一身份提交第二个不同 event ID 时返回 conflict 和 canonical Claim ID；每份新的
   canonical Claim 都在同一事务中保存 Claim event/投影、递增 `floor_revision` 并保存
   携带最新 Claim 集合的 Relay-signed `claiming` State 与 outbox 记录；
7. deadline 到达时冻结候选集合，Relay 使用安全随机源按 `uniform-v0` 等概率选择一名
   持有者，并在同一事务中持久化候选集合与 winner；测试环境可以注入确定性选择器；
   “等概率”只针对 deadline 前已进入候选集合的有效 Claim，不承诺不同延迟的参会者具有
   相同入选机会；
8. 未获选 Claim 不自动进入下一轮，参会者必须基于最新上下文重新判断；
9. Claim 前应已准备草稿，因此 Grant 默认租约为 10 秒、Relay 可配置；持有者未在租约内
   发出有效消息时，本轮进入
   `closed/expired`；
10. 只有 holder 可以消费 Grant，且每个 Grant 最多接受一条消息；Say 同样以数据库
    事务锁定 Round 后取得的 receive time 判断，只有
    `received_at < lease_expires_at` 才能接受；
11. 消息接受、Grant 消费、当前轮 `closed/spoken` 和下一轮 `open` 必须在同一个事务中
    完成；
12. Grant 过期、当前轮 `closed/expired` 和下一轮 `open` 也必须在同一个事务中完成；
13. winner 与 `claiming → granted` 必须在同一事务中用数据库锁或 CAS 原子提交；
14. Claim 窗口、租约 deadline 和选择结果使用数据库时间并持久化，Relay 重启后不得
    重新打开窗口或抽取新的 winner；
15. deadline 由持久 sweeper 推进，并在下一次相关请求到达时先执行 lazy recovery，
    不能只依赖进程内 timer；
16. 每次状态变化都先确定并签名唯一的 Round State event ID，再在同一事务中保存 Round
    投影、signed event、递增后的 `floor_revision` 和 transactional outbox 记录；outbox
    只负责提交后的可靠 fan-out，commit 后不得重新生成或重签 Round State；
17. 所有状态转换 CAS 都必须同时校验 `session.status=active` 和预期的
    `(round_number, phase, floor_revision)`；Meeting End 提交后，延迟的仲裁器或
    sweeper 只能 no-op；
18. 客户端只应用更大的 `floor_revision` 并验证合法状态转换；发现 revision 跳号时先
    进入 reconcile 补齐缺口，不能让事件乱序回退或跳过必要上下文后直接开放发言。

V0 的选择策略对所有身份等权，不提供连续发言优先、Leader 权重或 Human 直获发言权。
仲裁器必须保存 `policy_version`，使后续替换策略时不改变 Claim、Grant 和消息协议。

Relay 的持久投影至少保存：

```text
session_id, round_number, floor_revision, phase, claim_deadline,
canonical_claim_ids, holder_pubkey, grant_event_id, lease_expires_at,
outcome, speech_event_id, policy_version
```

Claims 对 `(session_id, round_number, claimant_pubkey)` 唯一；Grant 对
`(session_id, round_number)` 唯一；被 Grant 接受的 speech event 也必须唯一。这些约束
是防止并发双授予和双花的最后防线。

### 8.3 创建编排

`createMeeting` 提交一个 kind `42100` 事件。该 kind 与 NIP-29 Create 一样，在普通
`h`-scoped Channel 存在性校验之前进入专用创建路径。Relay 在接受事件前完成：

1. 校验创建者、标题和完整参会者集合；
2. 校验所有参会者属于当前 Community；
3. 如果指定来源 Channel，校验每名参会者已经拥有来源读取权限；
4. 校验每个 Agent 的 Channel Add Policy；
5. 校验 Session UUID 尚未使用，名单无重复，并满足总人数与 Agent 数上限；
6. 在单个数据库事务中保存 Meeting Create 事件、private Stream Channel、
   `room_kind=meeting`、Owner、全部成员关系、Round 1 `open` 投影、Relay-signed
   Round State 和对应 outbox 记录；
7. 提交后由 outbox 发布成员投影、成员通知、首个 Round State 和实时事件；
8. 返回 accepted 回执，客户端随后导航到会议室。

这条特殊路径不能转译成当前“先发 kind `9007`、再循环发送 kind `9000`”的客户端编排。
任何步骤失败都回滚整个事务，不需要留下待补偿的共享 `preparing` 房间。事务提交后的
通知应通过可重试 outbox 或等价机制发送；即使通知丢失，启动扫描仍能从持久投影恢复。

### 8.4 权限

会议室必须满足：

- 只能是 `private`；
- 只有参会者能够发现标题、名单和消息；
- 所有参会者都可以查询、订阅，并在每轮提交一次 Claim；
- 只有当前 Grant holder 可以发送一条 kind `9` 会议消息；
- 无 Grant、旧 Grant、过期 Grant、他人 Grant 和已经消费的 Grant 都必须被拒绝；
- kind `42103` 只能由当前 Community 配置的 Relay/system pubkey 签名，Relay 拒绝其他
  身份提交该 kind；客户端也必须验证这个权威 key，不能信任任意自称 Relay 的签名者；
- 非参会者即使知道 UUID，也不能读取 Query、Count、Search 或实时事件；
- 会议来源引用不授予来源 Channel 权限；
- 把 Agent 加入会议不会把它加入来源 Channel；
- 只有 Owner 可以正常结束会议，Community 管理员可以执行恢复性结束，Relay 只能因
  Community 安全撤权执行系统结束；
- Relay 必须拒绝 Meeting Channel 上的 kind `9000`、`9001`、`9021`、`9022` 和角色
  变更，防止从通用 Channel API 绕过固定名单；
- Relay 必须拒绝 Meeting Channel 上的通用 archive、unarchive 和 delete；生命周期
  只能通过 kind `42101` 结束；
- 会议结束后不可重新激活或删除历史。

这些属于 Relay 约束，不能只依赖 Desktop 隐藏按钮。

### 8.5 不直接复用 Huddle

Huddle 可以提供编排思路，但不能作为 Meeting V0 的运行基座：

- Huddle 创建后强制初始化音频，音频失败会结束房间；
- Huddle 使用活动 TTL，文字会议不应因静默自动结束；
- Huddle 的 Participant 是本地音频连接状态，不是权威成员名单；
- Huddle 的 active speaker、麦克风和语音指南不属于文字会议；
- Huddle 当前可能在添加 Agent 时顺带扩张来源 Channel 权限；
- Huddle 的创建者和运行阶段包含本地内存状态，不适合作为可恢复的会议事实。

因此，V0 只抽取“创建隔离房间并加入 Agent”的通用编排方式，不调用现有
`start_huddle`，也不复用 kind `48100–48106`。

## 9. Agent 会议模式

### 9.1 观察与唤醒必须分离

Agent 是否看见消息，不能由“这条消息是否触发了一次模型推理”决定。

每个参会 Agent 都维护一个 Session 级 Inbox：

```text
Meeting Event
    ↓ 验证签名、房间和作者
Session Inbox
    ├── 持久游标 / event ID 去重
    └── Speech Intent
            ├── PASS
            └── CLAIM
                    ↓ 抢夺成功
                Granted Turn
                    └── one signed speech
```

所有 speech 和 floor control 事件都先进入 Inbox。Agent 可以持续观察，但观察本身不
产生公开消息；只有结构化意图为 `CLAIM` 且 Relay 授权成功，才创建 Granted Turn。

### 9.2 会议级订阅

Agent 通过两条路径发现会议：

- 收到事务提交后的 Channel 成员通知；
- ACP 启动或重连时扫描自身有效 Channel，并读取 `room_kind`。

任何一条路径发现活动 Meeting Channel 后，都建立专用订阅：

```text
kinds = [9, 42101, 42102, 42103]
#h    = [session_id]
#p    = omitted
```

会议订阅不能沿用 ACP 默认的 mention-only 过滤，也不能沿用只接受 Owner 消息的
author gate。

会议 author gate 应当只接受：

- 固定参会者签名的规范 speech 和 Claim；
- 当前 Community 配置的 Relay/system pubkey 签名的 Round State；
- 有权身份签名的 Meeting End。

这项放宽只作用于该会议室，不能把 Agent 在所有 Channel 中全局改成
`respond-to=anyone`。

### 9.3 历史补齐

Agent 在以下时点必须执行历史回填：

- 首次发现会议；
- ACP 重连；
- Agent 运行实例重启；
- 检测到游标缺口。

回填与实时订阅并行进行，并按 event ID 合并去重。Agent 不能只从收到成员通知的时间
开始读取，否则会在重启或延迟启动后丢失会议早期内容。

Agent 在首次进入和每次重连时必须经过同步屏障：

1. 先建立 speech 与 floor control 的实时订阅，并缓存屏障期间到达的事件；
2. 读取当前 Meeting 与 Floor 投影，记下 `floor_revision`；
3. 分页回填 speech 与 floor control 历史直至快照边界/EOSE；
4. 合并缓存事件，再读取一次 Floor 投影并应用到最高 `floor_revision`；
5. 只有历史无缺口、当前状态已对齐后才标记 `meeting_synced=true`。

在同步完成前，Agent 可以缓存事件，但不能执行 Speech Intent、提交 Claim 或发送消息。
这防止 Agent 因先看到一个 `open` 事件、尚未看到此前会议内容而抢夺发言权。
同步屏障在运行期间也持续生效：若 `closed/spoken` 引用的 speech event 尚未进入 Inbox，
Agent 必须先按 event ID 补齐，再处理更高 revision 的 `open`。

每次 Agent Turn 的系统输入至少包含：

- Session ID、标题和状态；
- 完整参会者名单及 Human/Agent 类型；
- 当前 round number、floor revision、phase、竞争 deadline、holder 和 lease；
- 本轮尚未处理的消息；
- 近期上下文窗口；
- 完整历史的读取入口和游标。

### 9.4 Agent 发言意图与抢夺

Speech Intent 必须建立在稳定的语义依据上，而不是 Floor 状态变化本身：

```text
IntentBasis
- activation:<session-id>
- speech:<latest-canonical-speech-event-id>
- trigger:<unique-internal-or-external-trigger-id>
```

新 speech、会议激活或唯一的 meeting-scoped trigger 可以创建新的 `intent_basis_id`。
`open`、`claiming`、`granted`、超时和 `floor_revision` 变化只驱动调度与并发校验，
不能单独成为再次调用模型的语义依据。

当前 Round 处于 `open` 或未截止的 `claiming`、同步屏障已经完成且存在尚未处理的
IntentBasis 时，参会 Agent 执行结构化 Speech Intent：

```text
SpeechIntent
- intent_basis_id
- decision: CLAIM | PASS
- reason
- based_on_speech_cursor
- observed_floor_revision
- candidate_draft?
```

Harness 把结果持久化为按 `(session_id, agent_pubkey, intent_basis_id)` 唯一的
`IntentResult`，其生命周期为 `pending | resolved`。`PASS` 创建后即为 `resolved`，只
进入这份私有运行账本与 Observer，不产生共享 Claim 事件；`CLAIM` 先进入 `pending`，
只有在具体 Round 提交 kind `42102` 才参加抢夺。进程重启必须先恢复 IntentResult，不能
对同一 basis 重复推理。

V0 的默认 Agent 规则：

1. Agent 只有在能够补充新事实、回答问题、纠正错误、报告相关结果或提出必要异议时才
   应选择 `CLAIM`；
2. 仅表示收到、重复他人观点、没有足够把握或没有新增价值时应选择 `PASS`；
3. `@mention` 可以提高 Agent 选择 `CLAIM` 的意愿，但仍不能直接授予发言权；
4. Agent 应在 Claim 前准备候选内容，避免抢到发言权后才开始长时间推理；
5. `PASS` 终结当前 basis；没有新 basis 时，即使 Floor 多次换相或 Agent 重启也不会
   再次判断；
6. 每个 Agent 每轮只有一个 ClaimAttempt slot，按
   `(session_id, agent_pubkey, round_number)` 持久唯一；若同时存在多个 pending
   `CLAIM` IntentResult，调度器必须先选择一个或合并为 `basis_ids[]` 和一个明确的
   candidate draft，再签名唯一的一份 kind `42102`；
7. 抢夺失败后不得发送消息，也不得自动复制上一轮 Claim 事件；下一轮开放时，Harness
   必须基于最新 Inbox 重新确认绑定的 pending IntentResult 仍有效，才能为新 round 建立
   ClaimAttempt；
8. 抢夺成功后，Agent 先对齐最新 Inbox 与 Floor revision，再快速复验候选内容，只能在
   租约内提交一条消息；speech 被 Relay 接受后，winning ClaimAttempt 绑定的 basis
   必须持久标记为 `resolved`；
9. 候选内容已经过时或不再必要时，Agent 不发送，等待租约超时；
   该 basis 随后标记为 resolved，不能因超时换轮再次 Claim；
10. Grant 始终属于参会者 pubkey，而不是某个 Agent 进程；进程退出不撤销或转移 Grant，
    使用同一身份恢复的实例可以在剩余 lease 内查询并消费，否则由租约自动回收；
11. 同一 pubkey 的两个运行实例并发发送时，仍只有一个 event 能消费 Grant；
12. Agent 自己刚发送的 speech 不为自己创建新的 IntentBasis；若之后有新 basis，它在
    新一轮仍与其他成员等权抢夺，V0 不提供连任优先；
13. Session 结束后停止 Speech Intent、Claim 和发送，并把该 Session 尚未完成的 basis
    标记为 `resolved`。

Agent 可以由会议激活、新 speech、内部任务结果或明确的 meeting-scoped trigger 获得新的
IntentBasis，因此“主动”仍然成立；主动的是是否申请发言权，而不是绕过 Relay 直接向
时间线写消息。新 Round 只会唤醒 Claim 调度器，不会凭空制造新的发言理由。

必须诚实区分两项能力：发言权机制可以硬性限制“谁在何时能够进入时间线”，并保证每轮
最多一条消息；它不能自动证明发言内容在语义上一定合适。V0 验证交流秩序、并发控制和
消息上界，Agent 的相关性评分、可信度门槛和更精细的 Claim 策略留给后续版本。

### 9.5 强制平铺发送路径

Meeting Turn 不能沿用 ACP 的普通 Thread Reply 目的地。专用 Meeting sender 只在
Relay-signed `granted` 状态把当前 Agent 指定为 holder 后开放，并必须：

- 使用 kind `9`；
- `h` 只指向 Session ID；
- `meeting-round` 等于当前获授权轮次；
- `meeting-grant` 等于当前 `granted` Round State 的 event ID；
- `thread_ref = None`；
- 不产生 `e` root/reply、`q` 或其他线程引用；
- 可以携带合法 `p` mention，但它不影响 Grant 的效力。

模型选择普通回复工具时，Harness 也必须在提交前检查有效 Grant 并规范化到这条路径；
没有有效 Grant 时应拒绝输出，不能把依赖模型“记得先申请发言权”当作正确性保证。

真实 ACP 验收必须覆盖：会议激活、无 mention 的 Human speech 或内部任务结果形成
IntentBasis，Agent 主动作出 `CLAIM`；抢夺成功后输出带有效 `meeting-round` 和
`meeting-grant`、且无 thread/reply tags 的平铺 kind `9`。Relay 接受消息后，全部参会者
看到消息与下一轮；仅有 Round State 变化时不产生新的模型调用。

### 9.6 Agent 操作面

Agent-facing 操作首先进入 `buzz-cli`：

```text
buzz meetings create
buzz meetings list
buzz meetings show
buzz meetings participants
buzz meetings history
buzz meetings floor status
buzz meetings floor history
buzz meetings floor claim [--wait]
buzz meetings say
buzz meetings end
```

命令可以复用现有 Channels 和 Messages Client，但应暴露 MeetingSession 语义，避免
Agent 自己猜测某个普通 Channel 是否是会议：

- `history` 只分页返回 canonical speech log；
- `floor history` 分页返回 Claim 与 Round State control log；
- `floor status` 返回当前 round、phase、`floor_revision`、deadline、Claim 集合、
  `viewer_claim_event_id`、holder 和 lease；
- `floor claim` 只为当前轮提交 Claim；`--wait` 必须先建立状态等待再提交，直到输出
  `won | lost | ended`，并返回 round、最新 `floor_revision`、Grant ID 与 lease
  （若获胜），避免“先 Claim、后订阅”丢失结果；
- `say` 必须读取或接收当前身份持有的有效 Grant，并由 Relay 再次校验，不能成为直接
  绕过发言权的发送入口。

V0 不提供 `yield`：不发言时由短租约回收发言权。

## 10. Desktop V0

### 10.1 发起会议

“发起会议”弹窗只包含：

- 标题；
- 可选描述；
- Human / Agent 参会者选择；
- 可选来源 Channel。

提交时显示等待 Relay 接受的本地状态。只有原子创建事务成功后才打开会议；失败时保留
用户填写内容并展示具体错误，不出现部分创建的会议。

### 10.2 会议室

会议室复用现有 Channel Timeline、Composer 和成员组件，并增加会议专用外壳：

- 顶栏显示“会议”、标题和 `进行中 / 已结束`；
- 参会者入口始终可见；
- 参会者面板显示头像、名称、Human/Agent 标识和稳定身份；
- 主时间线显示全部会议消息；
- 每条消息明确显示作者；
- 顶栏或 Composer 始终显示当前轮次、`开放抢夺 / 抢夺中 / 发言中` 状态；
- `claiming` 时显示竞争窗口倒计时，`granted` 时显示 holder 和租约倒计时；
- 所有活动参会者都可以在本地准备草稿；
- 只有 phase 为 `open` 或 deadline 前的 `claiming`，且 `viewer_claim_event_id` 为空时，
  主操作才是“申请发言权”；其他非 holder 不能直接发送草稿；
- Claim 提交后显示“等待本轮结果”，同一轮不能重复申请；
- `granted` 时非 holder 只显示等待状态；
- 抢夺成功后，holder 的主操作变为“发送本轮发言”，且最多发送一次；
- 抢夺失败后保留草稿，但不会自动参加下一轮；由用户再次决定是否申请；
- Grant 过期时恢复为等待下一轮状态，并保留尚未发送的草稿；
- Owner 可以结束会议；
- 已结束会议只读打开。

Meeting Composer 只能复用普通 Composer 的视觉与草稿能力，不能复用其通用发送命令。
“申请发言权”和“发送本轮发言”必须分别调用 meeting 专用 Claim 与 Grant-bound Say
路径。

页面首次打开或重连时，先建立 floor control 订阅并缓存事件，再读取当前 Floor 投影，
最后应用缓存中更高的 `floor_revision`。在 speech 历史与 Floor 状态完成 reconcile 之前，
必须禁用 Claim 和 Send；`closed/spoken` 引用的 speech 尚未补齐时也保持禁用。不能因为
旧 `granted` 事件晚到而重新开放 Composer。

V0 不显示麦克风、扬声器、录音、TTS、STT 或 active speaker 状态。

### 10.3 会议列表

Desktop 提供当前身份可访问的会议列表，至少区分：

- 进行中；
- 已结束。

列表只来自当前 Community 的持久 `room_kind` 投影。Community 切换后必须清空会议级
缓存、订阅和游标，不能泄漏旧 Community 状态。带 meeting 标记的 Channel 由会议列表
接管，不应同时作为普通 Stream 重复出现在 Channel 列表中。

## 11. 关键不变量

Meeting V0 必须持续满足：

1. 一个 Session 只有一个私有会议室。
2. 房间、创建事件和完整名单要么一起提交，要么都不存在。
3. 一个会议室只有一个固定参会者快照；Community 安全撤权只改变有效访问并结束会议，
   不改写历史快照。
4. Human 与 Agent 具有相同的基础读取权、Claim 资格和 V0 等概率中奖权重。
5. 没有有效 Grant 的参与者一律不能写入会议消息；`@mention` 不是发言许可。
6. 作者身份来自事件签名公钥。
7. 规范会议日志只包含允许形状的 kind `9` 消息。
8. 每个活动 Session 恰好有一个当前 Round。
9. 每名参会者每轮最多有一份有效 Claim。
10. 每轮最多产生一个 Grant，并且最多只有一名 holder。
11. 只有 holder 能在租约内消费 Grant，且一个 Grant 最多接受一条消息。
12. 每轮最多向规范会议日志写入一条发言。
13. 发言接受或 Grant 过期时，当前轮关闭与下一轮开放原子发生。
14. Relay 重启不能重开竞争窗口、重新抽取 winner 或遗失有效 deadline。
15. 发言权状态只能按 `floor_revision` 单调前进，旧事件晚到不能造成状态回退。
16. 所有参会者最终收敛到同一组会议消息和发言权控制事件。
17. 在线订阅丢失可以由持久历史恢复。
18. Agent 运行实例不是参会者身份；实例重启不改变名单。
19. 会议消息和控制事件只对有效参会者可见。
20. 来源 Channel 只建立引用，不产生隐式授权。
21. 结束前接受的消息保留，结束后 Claim、Grant 和消息都被拒绝。
22. 已结束 Session 不可原地重开。
23. Meeting Turn 的 Agent 输出始终走带有效 Grant 的平铺 kind `9` 专用路径。
24. 会议聊天不会自动成为候选决议、决定或 Project View 当前状态。

## 12. 验收标准

V0 的主验收场景为：

> 两名 Human 和两个 Agent 进入同一个会议。四方都可以不依赖点名自主申请发言权；
> 每轮只有抢夺成功者可以发言。四方看到相同的参会者名单、轮次、发言权持有者、消息
> 作者和最终消息序列。

必须覆盖以下场景：

### 12.1 基本多对多

- H1、H2、A1、A2 在同一轮并发 Claim 时，只产生一个 Grant 和一个 holder；
- 同一参会者重复提交 Claim 时只留下第一份 canonical Claim；
- 每份新的 canonical Claim 恰好推进一次 `floor_revision`；重试和冲突不推进；
- 旧轮、未来轮和竞争 deadline 后到达的 Claim 都被拒绝；
- Owner、Human member 和 Agent bot 的有效 Claim 在 `uniform-v0` 中使用相同权重；
- winner 可以发送一条消息，另外三方在没有 Grant 时直接发送均被拒绝；
- 抢夺失败者不会自动进入下一轮，但能在下一轮重新判断并再次 Claim；
- 使用可注入的确定性测试仲裁器跑过多个 Round，证明 H1、H2、A1、A2 都能成为 winner
  并发送独立消息；
- 覆盖 Human → Human、Human → Agent、Agent → Human、Agent → Agent 的可见性；
- A1 在没有点名和定向入站消息时可以主动选择 `CLAIM`；
- kind `40002`、thread/reply、编辑和删除事件被拒绝，不进入任一方的规范日志；
- 所有客户端最终拥有相同 speech 与 floor control event ID 集合，且每个 event ID
  只出现一次；
- Claim 或 speech 已被接受但 ACK 丢失时，重发同一 signed event ID 仍返回原 accepted
  结果；同一 Claim slot 或 Grant 的第二个不同 event ID 被拒绝。

### 12.2 名单与身份

- 四方看到完全相同的四名参会者；
- Human/Agent 标识正确；
- 两名同名参会者仍按公钥正确区分；
- 无 Profile 时使用公钥摘要；
- 无效签名和伪造作者字段不会进入会议记录；
- 创建后的通用加人、移除、离开和角色变更事件都被 Relay 拒绝。

### 12.3 历史与重连

- Human 或 Agent 断线期间产生消息，重连后可以完整补齐；
- Human 或 Agent 断线期间发生 Claim、Grant、过期和换轮，重连后能恢复相同当前状态；
- 旧 Round State 在新状态之后到达时，客户端按 `floor_revision` 忽略状态回退；
- 下一轮 `open` 先于上一轮 speech 到达时，客户端补齐 `closed/spoken` 引用的 event 后
  才开放 Claim；
- 历史查询与实时消息交叉到达时不丢失、不重复；
- Desktop、ACP 或 Agent 进程重启后能从 Relay 恢复；
- Desktop 或 CLI 在 `claiming` 中重启后仍识别当前身份已有的 canonical Claim，不显示
  或提交第二份；
- speech 或 floor control 历史超过单次查询上限时都能够继续分页。

### 12.4 权限隔离

- 非参会者即使知道 Session ID 也不能读取名单、历史、搜索结果或实时消息；
- 非参会者不能提交 Claim 或发送消息；
- 非参会者不能读取 floor log；客户端不能提交或信任伪造的 kind `42103`；
- 相同 UUID 在不同 Community 中不能串读；
- 加入会议不会获得来源 Channel 权限。

### 12.5 生命周期

- 任一成员配置失败时，Channel、事件、成员和通知都不存在；
- 超过 Relay 的总人数或 Agent 数上限时，创建被整体拒绝；
- 在事务提交点之前没有任何一方能发现房间或提交 Claim；
- 在 `claiming` 期间重启 Relay，窗口按持久 deadline 继续且最终只产生一名 winner；
- 在 `granted` 期间重启 Relay，winner 和剩余 lease 不变，不重新抽取；
- holder 崩溃或选择不发言时，Grant 到期后自动进入下一轮；
- 同一 Grant 的两个并发发送请求中最多一个成功；
- 消息发送与 Owner 结束并发时，按 Relay 的事务线性化顺序恰有一种结果生效；
- Claim 与 End、仲裁器与 End、lease sweeper 与 End 并发时，End 之后都不会再产生
  `granted` 或下一轮 `open`；
- lease deadline 上 Say 与 expiry worker 并发时，按数据库时间和 CAS 恰有“消息接受”
  或“Grant 过期”一种结果；
- Round 状态事务 commit 后、fan-out 前 Relay 崩溃时，outbox 恢复后发布相同 event ID，
  不生成新的 Round State；
- 会议结束前已接受的消息保留；
- 结束后 Human 与 Agent 都不能继续 Claim 或发送；
- 原参会者仍能读取归档；
- 重复结束不会产生冲突；
- unarchive 被拒绝；
- Community 撤权立即收回访问并自动结束会议，但保留历史参会者快照。

### 12.6 发言权与 Agent 控制

- Agent 的 `PASS` 不产生 Claim 或公开消息；
- 历史和 Floor 同步屏障完成前，Agent 不执行 Intent、Claim 或 Say；
- 延迟发现或重启后，Agent 仍用同一个 `activation:<session_id>` basis，不重复创建激活
  Intent；
- 相同 `intent_basis_id` 只产生一份持久 IntentResult；ACP 重启不会重复执行；
- Agent 在 `PASS` 后没有新 IntentBasis 时不会反复运行；新 speech 为随之开放的新 Round
  形成 basis，唯一 internal trigger 可以在尚未结束的当前 Round 形成 basis；
- `floor_revision`、phase 变化和 Grant 过期本身不触发模型；
- 同一 Agent 同时存在多个 pending IntentBasis 时，每轮仍只签名、提交一份 canonical
  Claim；
- Agent 抢夺失败后不发送，也不把 Claim 自动带入下一轮；
- 多个 Agent 同时 Claim 时仍只产生一个 winner；
- 无 Grant、伪造 Grant、其他 holder 的 Grant、过期 Grant 和重复消费都被 Relay 拒绝；
- 当前发言者的消息只会开放下一轮，不会自动让其连续发言；
- speech 被接受后，winning ClaimAttempt 绑定的 IntentBasis 全部 resolved；没有新 basis
  时下一轮不再 Claim；
- Agent 获得 Grant 后崩溃不会阻塞会议，租约到期后自动换轮；
- Agent 因候选内容过时而让 Grant 到期后，不会用相同 IntentBasis 再次 Claim；
- ACP 在 Agent 获得 Grant 后重启时，同一 pubkey 可以在剩余 lease 内恢复并消费；
- 同一 pubkey 的两个实例同时恢复并发送时，仍最多接受一条消息；
- 真实 ACP 输出始终是带正确 `meeting-round`、`meeting-grant` 且无 thread/reply tags
  的平铺 kind `9`；
- 单个 Agent 故障不会阻断 Human 或其他 Agent 的会议交流。

隐私泄漏、Relay 已接受但任一参会者永久缺失的消息、重连历史缺口、作者身份错误、双
Grant、同轮双 speech、无/他人/过期 Grant 被接受、Floor revision 永久分叉或 Round 永久
停在已过 deadline 的 phase，均是 V0 发布阻断问题。

## 13. 建议实现顺序

阶段状态、交付物和完成条件见
[Meeting V0 分阶段开发计划](./meeting-v0-implementation-plan.md)。本节只保留设计层面的
技术依赖顺序。

### 13.1 第一段：CLI 与 Relay 证明

- 定义 kind `42100–42103`、`room_kind`、Round 投影和原子事务；
- 实现持久 Claim、等概率仲裁、Grant lease、CAS/唯一约束、deadline sweeper 和 outbox；
- 实现 `buzz meetings create/show/participants/history/say/end`，以及
  `buzz meetings floor status/history/claim --wait`；
- 完成私有房间、固定名单、Grant-bound kind `9` 白名单和归档只读约束；
- 用测试身份完成 2 Human + 2 Agent 的并发抢夺、单发言和状态收敛 E2E。

这一段先证明协议和共享状态，不依赖 Desktop 页面。

### 13.2 第二段：Agent 会议模式

- 识别会议成员通知；
- 增加 room-scoped 无 mention 订阅；
- 增加参会者 author gate；
- 增加历史补齐、Inbox 和游标；
- 增加结构化 `CLAIM | PASS` Speech Intent、同步屏障，以及持久 IntentResult /
  ClaimAttempt 去重；
- 订阅 Relay Round State，并在赢得 Grant 后创建 Granted Turn；
- 强制 Meeting Turn 使用 Grant-bound 平铺 kind `9` sender。

### 13.3 第三段：Desktop 会议室

- 发起会议弹窗；
- 会议列表和路由；
- 会议 Header、发言权状态与参会者面板；
- 复用 Timeline，并把 Composer 改为草稿、Claim 和获权发送三态；
- 结束与归档只读状态；
- Desktop E2E 覆盖 `open → claiming → won/lost → send → next open`、Grant 过期、
  重连乱序和 ended，只读状态，再完成真实 Relay smoke test。

这三段只依赖 Buzz 基础能力，可以与 Project View 的对象和页面并行推进。

### 13.4 预计代码触点

| 区域 | V0 主要变化 |
|---|---|
| `buzz-core` / `buzz-sdk` | kind `42100–42103` 常量与 builders、完整 `p` tags、Claim/Round State 和 Grant-bound kind `9` builder |
| `buzz-db` | `room_kind` migration、会议/轮次/Claim 投影、唯一约束、deadline 与原子事务 |
| `buzz-relay` | 原子创建、名单冻结、仲裁器、lease sweeper、Grant 校验、outbox 和权限测试 |
| `buzz-cli` | `meetings`、`floor status/history/claim --wait`、Grant-bound `say` 及双日志分页 |
| `buzz-acp` | 会议识别、全事件订阅、同步屏障、IntentResult/ClaimAttempt 账本、Roster 注入和平铺 sender |
| `desktop/src-tauri` | 原子创建、Floor status/claim、Grant-bound say、会议查询与结束命令 |
| `desktop/src/features/meetings` | 会议列表、创建弹窗、轮次/Claim/Grant UI、会议外壳和归档状态 |
| `buzz-test-client` | `e2e_meeting.rs`，覆盖原子创建、抢夺、双花、超时、重启、权限和结束竞争 |

## 14. V0 明确不做

- 音频、视频、TTS、STT、录音；
- 日历、预约、会议提醒；
- 在线 Presence 的权威判断；
- 动态增删参会者和主持权转移；
- 当前 holder 自动续得下一轮发言权；
- 按 Owner、Leader、Human、Agent 或其他身份设置不同抢夺权重；
- Human 直接取得或抢占发言权；
- Claim 撤回、主动让出 Grant 和复杂排队策略；
- 跨轮等待时长上界、饥饿防护和统计公平性承诺；
- 基于语义相关性、置信度或历史发言次数的 Relay 仲裁；
- Agent-only 对话的全局 causal-hop 上限、自动主持和自动休会；
- 线程、编辑、删除、反应、附件和会内私信；
- 议题程序、会议类型、必要角色和法定人数；
- 候选决议、投票、人类确认和决定写回；
- 自动纪要、总结和行动项；
- Project View 自动更新；
- 外部访客和跨 Community 会议；
- 把完整历史无限塞入每次 Agent 模型上下文。

## 15. 与未来正式会议的关系

Meeting V0 不是对[项目空间宪法](../../project-space-constitution.md)中“正式会议”的降级
定义，而是正式会议未来可以使用的一项基础设施。

未来关系应当是：

```text
FormalMeeting
- issue / type / governance scope
- required roles
- context snapshot
- candidate resolutions
- human confirmations
- final procedural result
    └── sessions: MeetingSession[]
```

一个正式会议可以关联零个、一个或多个文字交流 Session。MeetingSession 的参会者、
消息 event ID 和起止时间可以成为正式会议记录的证据，但聊天内容本身永远不会自动产生
治理效力。

因此，V0 成功的标志不是“Agent 已经可以替项目作出决定”，而是：

> Human 和 Agent 已经能够在一个身份清楚、名单共享、记录完整、可以恢复的空间中共同
> 交流。
