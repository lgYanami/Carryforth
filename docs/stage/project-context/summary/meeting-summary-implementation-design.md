# Meeting 来源摘要实现设计

> 状态：已实现
>
> 日期：2026-08-09
>
> 代码基线：`feat/project-view-summary` @ `d7a459528`
>
> 范围：Meeting 自有可选摘要、Meeting 元数据写入命令、Agent Action Finalization Prompt、
> Human Action Finalization Desktop 编辑入口、CLI / Desktop / Project Context 读取与展示
>
> 明确排除：Meeting 状态机修改、Board / Speech / State / End 协议修改、
> `COMPLETE` / `actions-recorded` ACK 修改、自动摘要生成、历史摘要回填、
> Project Context Node 摘要、Edge 摘要、Meeting 终态摘要修复协议、
> Mobile / Web 的 Meeting summary 编辑入口
>
> 关联文档：
> [Project View 来源对象摘要实现设计](project-view-summary-implementation-design.md)、
> [Meeting V2 Action Finalization 设计](../../meeting/v2/meeting-v2-action-finalization-design.md)、
> [Meeting Action Finalization 逻辑主持人 ACK 简化设计](../../meeting/fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)、
> [Project Context V2 领域规范](../project-context.md)、
> [Meeting 上下文讨论历程](../../meeting/context/meeting-context-discussion-history.md)

## 1. 文档目的

本文把已经确认的 Meeting summary 设计映射到当前 Buzz 实现。

Meeting 是 Project Context 的一种 Coordinate。Agent 在渐进检索图时，不应为了判断一场 Meeting
是否值得加载，就先读取完整 Board 和大量正式 Speech。因此 Meeting 自己应保存并提供一份可选的
检索摘要。

这份摘要和 Project View 对象摘要遵循同一个归属原则：

> 摘要属于它所描述的来源对象，不属于 Project Context Graph。

最终分工为：

```text
Meeting
├── title / description                 创建时的议题与目标
├── Board / Speech                      完整会议内容
├── State / End                         权威生命周期事实
└── optional summary                    Meeting 自己拥有的检索提示

Project Context Edge
└── Meeting coordinate identity         不复制 summary

Project Context query
└── Meeting CoordinatePreview.summary   从 Meeting 来源水合
```

summary 的正常写入时点是 Action Finalization。主持 Agent 或 Human 主持人在完成 Board 要求的
Project View、Document、Project Context 及其他业务物化后，使用普通 Meeting 元数据写入能力保存
summary；随后仍按现有流程返回 `COMPLETE` 或点击 `Confirm`，由原有 ACK 关闭会议。

## 2. 已确认的总体决策

### 2.1 summary 是 Meeting 自有字段

Meeting 增加：

```rust
pub summary: Option<String>
```

该字段的 canonical owner 是 Meeting domain。物理存储落在 `meeting_sessions`，而不是：

- `channels.description`；
- Meeting Board；
- Meeting End content；
- Relay State；
- Project Context Coordinate；
- Project Context Edge / Binding / Meta；
- 独立 Coordinate Node projection。

Project Context 和 UI 只能读取并展示 Meeting 当前 summary，不能成为它的第二 owner。

### 2.2 summary 可空

`summary = None` 是合法状态。

以下 Meeting 默认保持 `None`：

- 现有历史 Meeting；
- V0 / V1 Meeting；
- 不支持 Action Finalization 的 V2 Meeting；
- 没有进入 Action Finalization、直接从 Floor 关闭的 Meeting；
- 尚未完成 summary 维护的 active Meeting；
- Human 主持人明确选择不填写摘要的 Meeting。

缺失 summary 表示“当前没有摘要”，不表示 Meeting 与问题无关，也不表示内容为空。

### 2.3 summary 是普通物化，不属于 ACK

Action Finalization 本来就通过 CLI 或 Desktop 逐项写入外部业务状态：

```text
Project View write
Document write
Project Context write
other business write
Meeting summary write
```

这些写入不会与最终 Meeting End 组成跨系统原子事务。Meeting summary 与它们属于同一类普通物化。

因此本实现不把 summary 加入：

- `DirectActionOutput`；
- Agent `COMPLETE` JSON；
- Human `Confirm` input；
- `MeetingV2ActionsEndParams`；
- `actions-recorded` End；
- terminal State projection。

如果 summary 已写入而 ACK 暂时失败，Meeting 可以处于：

```text
lifecycle = finalizing_actions
summary = Some(...)
```

这不是不一致。summary 只描述当前 Meeting 内容；是否已经关闭仍由 State / End 决定。

### 2.4 Agent 与 Human 共用一条写路径

同一个 Meeting summary mutation 同时服务：

| 写入者 | 入口 | 行为 |
|---|---|---|
| Agent 主持人 | `buzz meetings update` | SET / CLEAR，随后 `meetings show` 回读 |
| Human 主持人 | Desktop Action Finalization 卡片 | 编辑、保存或清除，随后独立点击 Confirm |

两种入口最终生成相同的签名元数据命令，使用相同权限、校验、存储和读取投影。

Desktop 不建立一套只存在于本地或 Tauri 的 summary 状态；CLI 也不绕过 Relay 直接写数据库。

### 2.5 Meeting 控制流程完全不变

本次不增加任何 Meeting phase、turn、control action 或 completion step。

保持不变：

- kind `42100` Meeting Create；
- kind `42101` Meeting End；
- Board command / projection；
- Speech、Intent、Offer、Grant 与 Floor；
- Action Run、Action Window 与 lease；
- frozen Board fence；
- `COMPLETE | BLOCK | RETURN_TO_BOARD | ABORT` 输出 schema；
- Human `Confirm` action；
- `attestation=actions-recorded`；
- End / State / action-run 的原子关闭事务；
- deadline、retry 与 recovery 逻辑。

新增的是一条 Meeting 元数据 mutation，不是新的 Meeting 控制转换。

### 2.6 首版只允许 Action Finalization 期间维护

summary 写入要求：

- 当前 Meeting 使用支持 Action Finalization 的 current V2 policy；
- Meeting 仍然 active；
- canonical runtime phase 为 `finalizing_actions`；
- current Action condition 为 `runnable`；
- current Action lease / window 尚未过期；
- 写入者是 immutable Meeting host / moderator；
- 当前 Community 与 Meeting 身份匹配。

首版终态 Meeting 的 summary 只读。终态纠错、撤密或 operator repair 以后单独设计，不借本次实现
开放通用 Meeting metadata 修改。

### 2.7 不增加 summary 专属长度上限

Meeting summary 不设置字数、句数或 UTF-8 byte 的专属硬上限。

仍受现有通用边界约束：

- Nostr event 总大小；
- Relay request / event 总大小；
- CLI / Tauri command transport 总大小；
- `Some(summary)` 必须非空且不包含 NUL；
- SET 不接受空字符串；
- CLEAR 使用显式 mutation 语义；
- Prompt 要求尽量简洁，但不把写作建议实现成领域长度常量。

读取层不得静默把截断文本冒充完整 summary。若 UI 只显示若干行，可以做纯视觉 line clamp；复制、
Inspector 和 API 中仍保留完整值。

## 3. summary 与其他 Meeting 内容的边界

### 3.1 字段分工

| 内容 | 语义 | 是否权威 | 主要读取入口 |
|---|---|---:|---|
| title | Meeting 名称 | 是，创建事实 | metadata / show |
| description | 创建时的问题、背景和目标 | 是，创建事实 | metadata / show |
| Board | 当前完整结论、约束和行动合同 | 是 | board get |
| Speech | 正式有序讨论证据 | 是 | history |
| summary | “包含什么、何时值得加载”的检索提示 | 否 | metadata / show / Context preview |
| State / End | lifecycle、outcome、ACK 与 fence | 是 | show / state |
| materialized objects | 实际 Project View / Document / Context 结果 | 各自领域权威 | 各自 canonical read |

summary 不能替代 Board、Speech、State、End 或物化对象的 canonical read。

### 3.2 summary 应描述什么

主持 Agent 或 Human 应从以下已经确认的事实生成摘要：

- Meeting 的问题或目标；
- final frozen Board 的结论；
- 已实际物化并 canonical readback 的主要产出；
- 对未来检索有区分度的范围、约束或未决边界；
- 什么类型的问题下值得加载这场 Meeting。

典型形状：

```text
这场会议讨论了什么，形成了什么结论或行动结果；处理哪些后续问题时值得读取其 Board、正式发言和关联对象。
```

### 3.3 summary 不应包含什么

- 逐轮会议纪要；
- 某个发言人的评价；
- 当前 Role、当前任务或某条 Edge 的视角；
- 尚未成功物化的结果；
- 工具命令或权限声明；
- 对未来 Agent 的操作指令；
- Board、Speech 或 canonical readback 不支持的结论；
- 不适合在当前 Community Meeting 可见范围内暴露的信息；
- 仅为了搜索命中的关键词堆积。

summary 是 untrusted project data。模型看到它时必须把它当检索提示，而不是系统指令或授权。

## 4. 当前实现与缺口

### 4.1 当前 Meeting 存储没有 summary

`../../../../migrations/0037_meeting_v0_lifecycle.sql` 建立 `meeting_sessions`，保存 Meeting identity、Create、host、
source、schema、status 与 End 等生命周期事实。

Meeting title / description 因为复用 backing Channel，当前物理存在 `channels.name` /
`channels.description`。这不意味着新增 summary 也应复用 description：

- description 是创建输入；
- summary 是 Action Finalization 后形成的 Meeting 检索元数据；
- 两者生命周期和写入权限不同。

### 4.2 当前 Meeting 房间禁止普通 metadata update

`../../../../crates/buzz-relay/src/handlers/ingest.rs` 当前对 `room_kind=meeting` 做 fail-closed 路由：

- kind `9` 进入 Meeting Speech；
- canonical Meeting command 由 command executor 处理；
- kind `9002` 等普通 admin metadata 即使通过基础校验，最终仍被拒绝为
  `not part of the canonical Meeting log`。

因此不能只给 CLI 添加一个表面命令，也不能把普通 Channel metadata handler 整体放行给 Meeting。
实现必须增加一条 Meeting-owned、summary-only 的元数据 mutation；它是写入口，不是新的 Meeting
控制阶段。

该入口不能顺带放开以下 Meeting 修改：

- name；
- about / description；
- visibility；
- archived；
- ttl；
- topic / purpose。

### 4.3 当前 Action Finalization 输出不应扩展

`../../../../crates/buzz-acp/src/meeting_v1.rs` 的 `DirectActionOutput` 是严格 closed JSON：

```rust
struct DirectActionOutput {
    action: String,
    reason: String,
    reason_code: Option<String>,
}
```

`COMPLETE` 直接进入现有 `prepare_v2_action_close`。本实现不向该结构增加 summary。

summary 应在模型返回 `COMPLETE` 之前，通过本次新增的普通 CLI mutation 完成。

### 4.4 当前读取面把 description 当作唯一轻量 Meeting 说明

当前 kind `39000` group metadata 提供：

- `name`；
- `about`；
- `room_kind=meeting`；
- archived 状态。

`crates/buzz-cli/src/commands/meetings.rs` 的 `MeetingSummary` 实际是 Meeting metadata descriptor，
不是本设计新增的 source-owned summary。它目前只有 title / description / lifecycle metadata。

`crates/buzz-cli/src/commands/project_context.rs::meeting_coordinate_output` 明确写死
`summary: None`。Desktop Project Context Meeting hydration 也写死 `None`。

## 5. Canonical 数据模型

### 5.1 SQL migration

新增：

```text
migrations/0056_meeting_summary.sql
```

最小字段：

```sql
ALTER TABLE meeting_sessions
    ADD COLUMN summary TEXT;

ALTER TABLE meeting_sessions
    ADD CONSTRAINT chk_meeting_summary_non_empty
    CHECK (summary IS NULL OR btrim(summary) <> '');
```

PostgreSQL `TEXT` 本身不接受 NUL；SDK、Relay、CLI 和 Tauri 仍应在边界显式拒绝 NUL，避免错误只在
数据库层出现。

本次不增加：

- `summary_revision`；
- 独立 summary table；
- summary projection kind；
- background generation state；
- migration completion watermark。

原因是 current implementation 只有一个 immutable host 在一个 Action Finalization 阶段维护摘要，
SET / CLEAR 经过 Relay 串行提交并由调用方 readback；此时不需要先引入第二套 revision 协议。

### 5.2 领域读取结构

`buzz-db::meeting::MeetingRecord` 增加：

```rust
/// Optional Meeting-owned retrieval summary.
pub summary: Option<String>,
```

所有构造与读取路径都必须显式处理该字段：

- Create 后为 `None`；
- old row 为 `None`；
- SET 后为 `Some`；
- CLEAR 后为 `None`；
- End 不修改当前值；
- revocation / abort 不自动清空当前值。

### 5.3 summary lifecycle

```text
Create
  summary = None

Action Finalization
  None  --SET-->  Some
  Some  --SET-->  Some(new)
  Some  --CLEAR-> None
  None  --CLEAR-> no semantic change

Return / Block / retry / ACK failure
  preserve current summary

Close / Abort / revocation
  preserve current summary and make it read-only
```

Prompt 和 UI 会尽量只在已经确定可以正常完成时写最终摘要，但系统不假装外部物化可以回滚。
如果 summary 写入后 ACK 失败、Human 返回 Board 或 Meeting 后来 abort，summary 可以继续存在；读取方必须
同时展示 lifecycle，不能把 summary 的存在解释成正常关闭证明。

## 6. Meeting summary mutation wire

### 6.1 新增专用 Meeting metadata command

CLI 名称仍然是普通的 `buzz meetings update`，但 wire 不复用 kind `9002`。

原因不是要增加一条控制流程，而是当前 kind `9002` 的语义、权限和提交方式都属于普通 Channel：

- Meeting room 当前明确拒绝 kind `9002`；
- 普通 kind `9002` 使用 Channel owner / admin 权限，不能表达 immutable Meeting host；
- 普通 kind `9002` 的 side effect 是 best-effort，不能保证 event 与 Meeting source field 一起提交；
- summary 写入必须绑定当前 Action Finalization 的 run、window 和 frozen Board，防止上一轮延迟事件
  写入后来重新进入的 Finalization。

因此新增 author-signed command kind：

```rust
pub const KIND_MEETING_SUMMARY_COMMAND: u32 = 42113;
```

它属于 Meeting metadata plane，不是 State、Board、Speech、End 或 Action control event。

SET：

```json
{
  "kind": 42113,
  "tags": [
    ["h", "<meeting-uuid>"],
    ["v", "3"],
    ["policy", "moderated-board-actions-v3"],
    ["action", "set"],
    ["action-run", "<action-run-uuid>"],
    ["action-window", "<positive-integer>"],
    ["board", "<frozen-board-event-id>"]
  ],
  "content": "<complete summary>"
}
```

CLEAR：

```json
{
  "kind": 42113,
  "tags": [
    ["h", "<meeting-uuid>"],
    ["v", "3"],
    ["policy", "moderated-board-actions-v3"],
    ["action", "clear"],
    ["action-run", "<action-run-uuid>"],
    ["action-window", "<positive-integer>"],
    ["board", "<frozen-board-event-id>"]
  ],
  "content": ""
}
```

规则：

- 上述 tag 各恰好一个；
- `action=set` 要求 content 非空且不包含 NUL；
- `action=clear` 要求 content 为空；
- SET 可以用 trim 判断“是否全为空白”，但 accepted value 必须按原始 UTF-8 精确保存，不静默 trim 或截断；
- 未知、缺失、bare 或重复 tag 按 closed schema 拒绝；
- 不在 command 中承载 name / description / visibility / archived / ttl / topic / purpose；
- 不增加 summary 专属长度限制，继续受现有 event 总边界约束。

### 6.2 SDK builder

不要扩展通用 `build_update_channel(...)` 的参数列表。新增 Meeting 语义明确的 builder，例如：

```rust
pub enum MeetingSummaryMutation<'a> {
    Set(&'a str),
    Clear,
}

pub struct MeetingSummaryUpdateParams<'a> {
    pub session_id: Uuid,
    pub mutation: MeetingSummaryMutation<'a>,
    pub action_fence: MeetingV2ActionRunFence<'a>,
}

pub fn build_meeting_summary_update(
    params: MeetingSummaryUpdateParams<'_>,
) -> Result<EventBuilder, SdkError>;
```

这样可以防止 CLI 或 Desktop 意外组合 Meeting summary 与普通 Channel metadata 修改。

builder 校验：

- meeting UUID 非 nil；
- SET 非空；
- SET 不含 NUL；
- SET 不增加 summary 专属长度限制；
- action run UUID 非 nil；
- action window 为正数；
- frozen Board event ID 是 64 位 hex；
- SET 把 summary 写入 content，CLEAR 使用空 content。

### 6.3 Relay 路由

在 `command_executor` 中把 kind `42113` 路由到专用 Meeting summary handler。现有 Meeting room 对 kind
`9002` 的拒绝保持不变；不能为了本功能给普通 Channel metadata 开例外。

```text
room_kind == meeting
AND kind == 42113
AND exact Meeting summary command schema
```

summary command 属于 Meeting metadata plane，不进入：

- canonical Speech history；
- Board history；
- State transition list；
- Action Run transition；
- participant Floor conversation。

### 6.4 使用专用事务

summary command 进入一个小型专用事务：

```text
verify signed event / auth / scope
  -> lock Meeting row
  -> verify project/community/host/policy/phase/action fence
  -> persist command event
  -> SET or CLEAR meeting_sessions.summary
  -> commit
  -> emit kind 39000 discovery best-effort
  -> return standard write receipt
```

这不是 Meeting control transition；它只保证“签名 mutation event”和“Meeting 当前 summary”不会部分提交。

锁顺序必须与 Action Return / Block / End 路径兼容，并至少串行化同一个 Meeting session 与 current
Action Run。并发结果只能是：

- summary transaction 先提交，随后控制动作看到并保留该外部效果；或
- 控制动作先离开 `finalizing_actions` / 结束 Meeting，summary transaction 在锁内重检后拒绝。

不得出现“事务外预检仍在 Finalization，End 已提交后又成功写 terminal Meeting”的竞态。

### 6.5 authorization 与 phase gate

Relay / DB 必须在同一事务内重新验证：

- auth scope 包含 `ChannelsWrite`；
- authenticated channel token / `auth.channel_ids()` 包含该 Meeting ID；
- Meeting 存在且属于 host-derived Community；
- backing Channel 的 `room_kind=meeting`；
- policy 是 current action-capable V2；
- Meeting status 仍是 `active`；
- runtime phase 是 `finalizing_actions`；
- current Action Run condition 是 `runnable`；
- current Action lease、operator deadline 与 action window 尚未过期；
- event author 等于 persisted immutable host / moderator；
- author 当前仍通过 Community restriction / revocation gate；
- 当前未终结 Action Run 与 command 的 `action-run` 一致；
- 当前 action window 与 command 的 `action-window` 一致；
- 当前 frozen Board event 与 command 的 `board` 一致；
- event 没有跨 Community 或跨 Meeting identity。

deadline / lease 判断使用 Relay / DB 的 authoritative clock，并在同一锁内先执行或复用现有 due-action
recovery；不能相信 CLI / Desktop 预检时间。

不能仅因为 actor 是 Community owner/admin 就允许修改别人的 Meeting summary。Operator 的 recovery / abort
权限不等于 Meeting 语义摘要写权限。

### 6.6 receipt、duplicate 与 canonical readback

写入沿用 Agent CLI 的标准 write response：

```json
{"event_id":"<hex>","accepted":true,"message":"..."}
```

不为 summary 发明第二种 HTTP response envelope。标准 receipt 只证明该 signed command 被接受；当前
summary 仍必须通过 `meetings show` 回读。

同一 signed event ID 重试：

- 不重复改变 Meeting control state；
- 返回 accepted duplicate；
- 不把同文本不同 event 自动声称为 duplicate；
- duplicate path 不重放 source mutation，但应安全地重试 / repair discovery emission；
- 如果后来已有更新，duplicate receipt 不证明旧值仍是 current。

不同 event 请求已经成立的同一状态（SET 相同文本或对 `None` CLEAR）可以按幂等 no-op 接受：event 仍按
命令审计策略处理，但不改 canonical summary。handler 仍须检查并重建当前 kind `39000` head，或至少触发
一次可重试的 discovery emission；否则第一次 DB commit 后的 projection emission 失败会被 no-op 永久
固化。Prompt、CLI 与 Desktop 仍应优先通过 KEEP / disabled Save 避免制造这种无意义请求。

## 7. Agent CLI

### 7.1 命令形状

在 `MeetingsCmd` 增加：

```text
buzz meetings update --meeting <UUID> --summary <TEXT>
buzz meetings update --meeting <UUID> --summary -
buzz meetings update --meeting <UUID> --clear-summary
```

规则：

- `--summary` 与 `--clear-summary` 恰好选择一个；
- `--summary -` 从 stdin 读取完整文本，避免复杂 shell quoting；
- SET 的空白文本拒绝；
- CLEAR 不接受同时提供 summary；
- CLI 使用当前 identity 签名；
- CLI 先读取 current Action Finalization status，从 verified current Action Run 提取
  `action-run`、`action-window` 与 frozen Board event ID；这些 fence 不要求用户手工输入；
- Relay definitive conflict / phase rejection 使用现有错误映射；
- 网络不确定时不得盲目生成不同写入，先读取当前 Meeting。

### 7.2 读取与回读

`buzz meetings show --meeting <UUID>` 增加：

```json
{
  "summary": "... or null"
}
```

Agent 的成功条件不是只看到 kind `42113` accepted receipt，而是随后读到：

- 同一 Meeting identity；
- exact intended `Some(summary)` 或 `None`；
- lifecycle 仍与当前 Action Finalization 判断一致。

如果 discovery projection 暂时落后，CLI 不得伪造 readback。Agent 可以有限重读；仍无法确认时返回
现有 `BLOCK`，而不是声称 COMPLETE。

### 7.3 不新增 CLI closure shortcut

禁止增加以下组合命令：

```text
buzz meetings update-summary-and-close
buzz meetings actions confirm-recorded --summary ...
```

summary mutation 和 ACK 在产品流程上相邻，但保持两个独立动作，才能让 Agent 与 Human 使用相同的
Meeting metadata 能力，也避免把检索提示升级为 closure protocol 字段。

## 8. Agent Action Finalization Prompt

### 8.1 Prompt 落点

主要修改：

- `crates/buzz-acp/src/meeting_v1.rs::build_v2_action_finalization_prompt`；
- `V2_ACTION_FINALIZATION_TOOLS`，显式允许受控的 `buzz meetings update` summary metadata mutation；
- 对应纯 Prompt / coordinator tests。

`../../../../crates/buzz-acp/src/meeting_context.rs` 在 action-finalization 段补充一条窄例外：主持人可通过受控 CLI
维护 Meeting retrieval summary；这不是让 Agent 自行构造或发布 State、Board、End、Action 等控制事件。
稳定合同版本从 `4` 升为 `5`，由现有 contract rotation 让旧 ACP Session 失效。

本 Prompt 把维护要求明确绑定到 Relay 的 `buzz-meeting-summary-v1` 能力。`buzz meetings update` 在签名和
提交前通过 verified NIP-11 identity 检查该 extension；缺失时保留旧 Action Finalization 行为，不能因一个
Relay 尚未升级而让原本可关闭的 Meeting 永久 BLOCK。

不修改 `DirectActionOutput` parser。

### 8.2 Agent 的固定执行顺序

Prompt 要求：

```text
1. 读取 exact frozen Board。
2. 读取 canonical target state。
3. 物化 Board 已决定的 Project View / Document / 其他业务结果。
4. canonical readback 每个结果。
5. 需要时维护 Project Context Document / Edge 并 readback。
6. 判断当前状态是否已经可以 COMPLETE。
7. 从 frozen Board + 实际 readback 结果生成 Meeting summary。
8. 读取当前 Meeting summary：
   - 已准确：KEEP，不制造无意义写入；
   - 缺失或不准确：SET；
   - 不应继续暴露且无安全替代：CLEAR。
9. 使用 buzz meetings update 写入，并用 buzz meetings show 回读。
10. 确认所有要求完成后，按原 schema 返回 COMPLETE。
```

summary 只能在 Agent 已判断 `COMPLETE` 合理之后写。若 Board 不充分、物化不完整或关系仍需决策，
应先 `RETURN_TO_BOARD` / `BLOCK`，不要发布一个伪装成最终结果的 summary。

### 8.3 KEEP / SET / CLEAR

| 判断 | Agent 行为 |
|---|---|
| 当前 summary 会让未来 Agent 作出相同加载判断 | KEEP，不写 |
| 缺失，且当前 Turn 已完整理解 final Board 与物化结果 | SET |
| 主题、结论、范围、主要产出或未决边界变化 | SET |
| 当前 summary 不准确或包含不应继续暴露的内容，且可安全重写 | SET |
| 当前 summary 必须撤回且没有可信安全替代 | CLEAR |
| 无法读取完整 Board / target / current summary | 不猜，BLOCK 或 RETURN_TO_BOARD |

普通 status、revision、assignee、工具日志或措辞变化不应触发 summary 重写，除非它们真的改变未来加载判断。

### 8.4 失败映射

| 失败 | Action Finalization 输出 |
|---|---|
| summary mutation 被拒绝或写入失败 | `BLOCK(external_operation_failed)` |
| phase / current state 与预期冲突 | `BLOCK(external_state_conflict)` |
| 已广告的 CLI / Relay surface 在执行中不可用 | `BLOCK(tool_unavailable)` |
| Relay 未广告可选 summary capability | 保留旧 closure flow，不因 summary 单独 BLOCK |
| provider 无法继续 | `BLOCK(provider_failure)` |
| Board 不足以形成可信摘要 | `RETURN_TO_BOARD` |
| summary 已正确 readback | 继续原有 `COMPLETE` |

这里明确分两层：

- Relay / Human / legacy 层：summary 永远不是 close gate，`None` 不阻止 Confirm、End 或 Abort；
- 在 `buzz-meeting-summary-v1` 已广告的当前 managed Agent policy 下：如果 current summary 需要
  SET / CLEAR，mutation 与 exact readback 未完成时不得返回 `COMPLETE`；若 current 值已经准确则 KEEP，
  不需要为了 closure 制造写入。

这是主持 Agent 在 Action Finalization 中对已决定物化项的执行合同，不是 Meeting protocol 对所有关闭路径
增加“summary 必填”约束。

### 8.5 ACK 失败后的重试

如果 summary 已成功写入并 readback，但随后 ACK 网络不确定或被暂时拒绝：

- 不清除 summary；
- 不假定 Meeting 已关闭；
- 继续使用现有 exact End replay / Action recovery；
- 重进 Action Finalization 时先读取 current summary；
- summary 已准确则 KEEP；
- final Board 或物化结果发生变化才 SET 新值。

## 9. Human Action Finalization Desktop 入口

### 9.1 UI 位置

在 `../../../../desktop/src/features/meeting/ui/MeetingActionFinalizationCard.tsx` 中，放在“Final Board is frozen”
材料区与底部 action buttons 之间，增加独立的 Meeting summary 编辑区。

内容：

- 标题：`Meeting summary`；
- 说明：用于未来 Project Context 检索，不替代 Board 或会议记录；
- 多行 Textarea；
- `Save summary`；
- `Clear summary`；
- `Reset draft`；
- saved / unsaved / saving / error 状态；
- 当前 canonical summary 的只读基线。

不设置 summary 专属 `maxLength`。Textarea 可以有合理初始 rows 和视觉高度，但不得裁剪保存值。

### 9.2 Human 操作流程

```text
打开 Project View / 其他业务界面完成物化
  -> 回到 Meeting Action Finalization
  -> 编辑 summary
  -> Save
  -> Native 签名并提交相同 summary mutation
  -> refresh / readback
  -> Human 点击原有 Confirm
  -> 原有 actions-recorded End 关闭 Meeting
```

Save 和 Confirm 是两个独立 action：

- Save 不自动 Confirm；
- Confirm 不把 draft 隐式塞进 End；
- Save 失败不自动关闭 Meeting；
- Confirm 的 input、event builder、receipt 和 recovery 不增加 summary；
- summary 为空且没有 dirty draft 时，Human 仍可 Confirm。

### 9.3 dirty draft

不能让 Human 输入内容后无提示地点击 Confirm 丢失 draft。

UI 状态固定区分为：

- draft 与 canonical summary 不同：显示 `Unsaved summary`；
- snapshot 后台刷新只有在 draft 不 dirty 时才同步新的 canonical summary；dirty 时保留本地输入并提示
  来源值已经变化；
- dirty draft 时点击 Confirm 或 Return，dialog 明确提示尚未保存；
- dialog 至少提供 `Cancel` 与 `Discard draft and continue`，也可引导先返回编辑器 Save；
- `dirty` 只表示尚未签名/提交的本地文本，可以显式 Discard；
- `saving` 表示 exact event 已签名并正在提交，此时暂时不发送 Confirm / Return；
- submit 结果不确定时进入 `indeterminate`，保留 exact signed event，只能重试同一事件或读取 canonical
  Meeting metadata 进行 reconciliation，不能把它当普通 draft 丢弃；
- 只有确认 command definitive reject、canonical readback 已确定其结果，或 canonical Meeting 已先离开该
  Action window 后，才解除 saving / indeterminate；
- 保存失败保留 draft，但 Human 仍可以显式 Discard 后按原流程 Confirm；
- summary 本身仍可空，Reset 到 canonical `None` 后可以正常 Confirm。
- Save / Clear 只有在 exact canonical readback 成功后才更新 baseline 并清除 dirty。

这是 Desktop 对一条已经开始的外部写操作做安全收敛，不是 Relay End gate。没有正在提交或结果不确定的
mutation 时，Confirm 的协议能力绝不以 summary 非空、保存成功或没有草稿为条件。

### 9.4 Native / TypeScript mutation

新增独立 Tauri command，例如：

```text
update_meeting_summary
```

输入表达 closed mutation：

```ts
type MeetingSummaryMutation =
  | { type: "set"; summary: string }
  | { type: "clear" };
```

Native：

- reload verified current Meeting snapshot；
- 验证 current identity 是 host；
- 验证 lifecycle 是 `finalizing_actions`；
- 从 current Action Run 取得 exact run / window / frozen Board fence；
- 使用 shared SDK builder；
- 签名并提交；
- 对 indeterminate submit 保留 exact event 或要求 canonical readback；
- definitive reject 后不盲目 retry；
- 返回 accepted / indeterminate 的 typed result。

不要把 summary mutation 增加为 `MeetingActionFinalizationAction` 的新 variant。后者仍只承载
Begin / Block / Retry / Return / Confirm 控制动作。

### 9.5 可见性与权限

编辑器只对当前 Human host 且仍可接受 summary command 的 current Action window 显示。blocked / expired
window 可以显示 current summary，但 Save / Clear 保持不可用，直到 Retry 建立新的 verified window。普通参与者和
Community observer：

- 可以读取 summary；
- 不能看到可编辑控件；
- 不能调用有效 mutation；
- 不能通过手工构造 Tauri input 绕过 Native 与 Relay authority check。

terminal Meeting 只读展示 summary，不显示 Save / Clear。

## 10. 读取与投影

### 10.1 kind 39000 discovery

`emit_group_discovery_events` 在 `room_kind=meeting` 时读取 Meeting current summary：

```text
Some(summary) -> ["summary", "..."]
None          -> omit summary tag
```

kind `39000` 是可重建 discovery projection，不是 summary 的 owner。canonical value 仍在
`meeting_sessions.summary`，写入来源仍是 accepted host mutation event。

CLI 与 Desktop 只接受当前 Relay 签名、`d` 等于 Meeting ID、`room_kind=meeting` 的 metadata head；
`summary` tag 最多一个，bare / duplicate tag 视为 malformed projection，不能从任意 Channel event 或
调用方自报文本构造 canonical readback。

当前 CLI 的 `meeting_summary(...)` 只是对 query JSON 取 tag，并没有独立验证 Relay author。实现必须先从
NIP-11 获得并验证 Relay identity，在 query filter 加 `authors`，再把返回值解析为 Nostr Event 并校验
signature、author、kind、`d` 与 room kind。`fetch_meeting_context_summaries` 接收同一个 expected Relay
identity；`show`、`list` 与 Project Context hydration 不能各自退化成未验证的 JSON adapter。

`emit_group_discovery_events` 当前主要从 `channels` 组装 metadata。实现时仅对
`room_kind=meeting` 按同一 Community + Meeting ID 读取 `meeting_sessions.summary` 并加入投影；不要把
Meeting summary 复制进 `channels.description`，也不要给普通 Channel 伪造同名来源字段。

summary 更新后：

- best-effort 重发 kind `39000`；
- 沿用 `emit_addressable_discovery_event` 的单调 `created_at` 规则，确保新 head 可替换旧 head；
- 扩展 `reconcile_channel_events`：对 Meeting 不能只检查 kind `39000` 是否存在，还要比较 current head 的
  summary 与 `meeting_sessions.summary`；缺失或不一致时重发；
- duplicate / semantic no-op command 也触发同一 projection check，因此可修复“DB 已提交、首次 emission
  失败”的 stale head；
- projection emission 失败不回滚已经提交的 summary mutation；
- Agent / Human 必须通过 readback 识别暂时不可见，而不是伪造成功。

### 10.2 CLI Meeting 输出

`MeetingSummary` metadata DTO 增加：

```rust
pub(crate) summary: Option<String>
```

以下命令输出 summary：

- `buzz meetings list`；
- `buzz meetings show`；
- Meeting metadata batch hydration；
- Project Context Meeting coordinate hydration。

当前类型名 `MeetingSummary` 容易与新增领域字段混淆。实现时可改名为 `MeetingMetadata` 或
`MeetingMetadataSummary`，但这是代码清晰度重构，不改变 wire 输出。

### 10.3 Project Context CLI

修改 `meeting_coordinate_output`：

```rust
summary: metadata.summary.clone()
```

同时删除当前无条件 `state: "terminal"` 的错误假设：unarchived metadata 至少输出 `active`，archived 才输出
`terminal`；更精确的 `finalizing_actions / closed / aborted` 仍从 verified Meeting detail / show 读取。
summary 可以在 active Finalization 中存在，预览绝不能因看见 summary 就伪造 terminal state。

保持：

- Coordinate identity 不变；
- EdgeKey 不变；
- Context revision 不变；
- `meeting_fetch` 仍提供 show / board / history；
- summary missing 不等于 unavailable；
- metadata unavailable 时保留 coordinate identity 并报告 unavailable；
- summary 只帮助选择是否继续加载 Board / Speech。

同时修正当前 `meeting_fetch` 生成器的参数形状。它现在遗漏 Clap 要求的 `--meeting`，本次交付固定输出：

```text
buzz meetings show --meeting <UUID>
buzz meetings board get --meeting <UUID>
buzz meetings history --meeting <UUID>
```

这不是新的遍历协议，而是让摘要预览给出的渐进加载入口实际可执行。

### 10.4 Desktop Meeting DTO

以下 DTO 增加可空 summary：

- `MeetingSnapshot`；
- `MeetingListItem`；
- `MeetingContextInspectorDetail`；
- 对应 TypeScript 类型；
- Project Context Meeting hydration DTO。

Human Action Finalization editor 从 `MeetingSnapshot.summary` 初始化 canonical baseline。

`load_meeting_snapshot_at` 把 verified kind `39000` head 的 `created_at` 纳入
`authoritative_updated_at` 合并；否则只修改 summary 时 Meeting List / Context cache 观察不到更新。该时间只
表示 verified source projection发生变化，不把 39000 提升为 lifecycle权威事件。

### 10.5 Project Context Desktop 展示

此前 Project View summary 交付已经让通用 graph node / Inspector 支持 `summary`。

本次只需让 Meeting hydration 填入来源值：

- graph node 显示受 line-clamp 的 summary；
- Coordinate Inspector 显示完整 summary；
- Edge Inspector 中 Meeting member 显示 summary；
- Meeting content Inspector 显示 summary；
- Meeting picker 的轻量说明优先使用 summary，缺失时才回退 description；
- terminal Meeting 详情独立展示 retrieval summary，同时保留 closed / aborted outcome；
- null 时不渲染空摘要块；
- 不把 description 自动复制进 summary。

### 10.6 不做每 Turn 注入

Meeting summary 不新增到：

- 每个 Agent Turn；
- Role Brief；
- Meeting participant envelope 的公共正文；
- Board Maintenance；
- Floor Decision；
- Speech prompt。

它首先服务 metadata-first 检索。Agent 只有在读取 Meeting metadata 或遍历 Project Context 时看到它。

## 11. 乐观生命周期与失败语义

### 11.1 “乐观”指什么

本设计的乐观机制是：

- Relay 不根据 Board / Speech / Object revision 自动生成 summary；
- 内容变化不会自动清空 summary；
- 主持 Agent / Human 根据当前完整上下文判断 KEEP / SET / CLEAR；
- 写后通过当前来源投影回读；
- 失败不回滚其他已经物化的业务效果；
- 后续重试重新读取并修正。

它不是“summary 与 End 必须原子”，也不是“只要 write receipt accepted 就不做 readback”。

### 11.2 summary 成功、ACK 失败

允许：

```text
summary SET accepted
summary readback succeeded
actions-recorded ACK indeterminate/rejected
Meeting remains finalizing_actions
```

读取方看到 summary 时仍同时看到 lifecycle。summary 不授予 terminal 语义。

### 11.3 summary 后又 Return to Board

Prompt / UI 应避免在尚未确认完成时保存“最终”摘要，但系统不能假设绝不发生。

若发生：

- 不自动清空；
- Return to Board 继续按原控制协议执行；
- 下一轮 Board / Action Finalization 重新判断 SET / CLEAR；
- summary 只作提示，不作为当前 Board 的证据。

### 11.4 summary 后 Meeting abort

正常 Agent `ABORT` 不写 summary。但如果 summary 已经写入后发生 operator abort、revocation 或其他
终止，当前 summary 可以保留。

UI 必须同时展示 `aborted`；不得因为 summary 语气像成功结论就隐藏 terminal outcome。

### 11.5 discovery 延迟

DB mutation 成功而 kind `39000` 尚未更新时：

- canonical row 已拥有新值；
- 旧 metadata head 可能暂时可见；
- CLI / Desktop 显示 saving / readback pending；
- Agent 不得在看不到 exact value 时声称维护完成；
- repair / retry emission 后恢复；
- 不重复创建不同 summary 以“推动”投影。

## 12. 安全与权限

### 12.1 summary 不扩大读取权限

Meeting 已按当前设计 Community-readable。summary 跟随同一 Meeting read policy。

Project Context Edge 不因为包含 Meeting coordinate 而授予新权限；metadata query 和 Meeting read gate
仍按当前 Community / host-derived tenant 边界执行。

### 12.2 summary 不扩大写权限

- Project Context attach 权限不授予 Meeting summary update；
- Channel admin 权限不自动授予 Meeting summary update；
- Community operator 的 abort / recovery 权限不授予语义编辑权；
- participant roster 身份不授予 summary update；
- immutable host / moderator 是首版唯一 writer。

### 12.3 Prompt injection

summary、title、description、Board、Speech 和物化对象内容都属于 untrusted data。

读取或展示 summary 时：

- 不执行其中命令；
- 不改变 Agent identity、Role、tools 或 permissions；
- React 只按文本渲染；
- CLI JSON 正确 escape；
- Prompt 若包含 summary，必须放在明确的数据边界中；
- summary 不能构造 Meeting control output。

### 12.4 Community 信息泄漏

主持人在写 summary 时必须遵守 Meeting 当前 Community-readable 边界。不能把只存在于更窄私有来源、
且不应被整个 Community 读取的内容复制到 summary。

Action Finalization Prompt 必须明确这一点；Relay 无法从自由文本自动证明不存在语义泄漏。

## 13. 兼容与发布

### 13.1 数据兼容

SQL migration 只增加 nullable column：

- 不回填历史值；
- 不扫描旧 Board / Speech 自动生成；
- 不重写历史 End；
- old rows 读取为 `None`；
- Project Context 不建立迁移节点；
- migration rollback 前不得依赖已写入 summary 的新 binary 回退。

这里需要 schema migration，但没有历史数据 migration / backfill。

### 13.2 wire 兼容

- kind `42113` 是新增的 Buzz Meeting metadata command；
- Relay NIP-11 增加独立 feature extension：`buzz-meeting-summary-v1`；
- 它不改变 Meeting Create 声明的 policy、schema version、direct-actions runtime capability 或 create gate；
- old clients 通常会忽略 kind `39000` 未知 tag；
- old Relay 不认识 kind `42113`，会拒绝 summary mutation；
- 新 CLI update 在 extension 缺失时 fail fast 为 unavailable；Desktop 不显示可编辑入口；
- ACP Prompt 在发布顺序上最后启用，不能先要求 Agent 调用旧 Relay 不支持的surface；
- `summary` 缺失始终按 `None`；
- 不修改 Meeting schema version 或 action capability version。

建议发布顺序：

```text
1. DB migration + Relay summary handler / discovery
2. SDK + CLI read/write
3. Desktop read/editor
4. ACP Action Finalization Prompt
```

Prompt 最后发布，避免 Agent 被要求调用尚不存在的 CLI / Relay surface。

### 13.3 不改变 Meeting compatibility 判定

Meeting summary 缺失不能让一个原本 supported 的 Meeting 变成 unsupported；summary capability 缺失也不能
改变历史 Meeting 的读取兼容性。

只在写入时，旧 Relay 返回明确 unsupported / invalid error。

## 14. 代码影响面

### 14.1 Migration / DB

- `../../../../migrations/0056_meeting_summary.sql`
- `../../../../crates/buzz-db/src/meeting.rs`
- 新增或邻近 Meeting summary mutation transaction
- DB migration parity / schema tests

DB 工作：

- nullable summary column；
- MeetingRecord hydration；
- host / policy / phase checked SET / CLEAR；
- event + summary transaction；
- duplicate receipt；
- Community collision isolation。

### 14.2 SDK / core wire

- `../../../../crates/buzz-core/src/kind.rs`
- `../../../../crates/buzz-sdk/src/builders.rs`
- `../../../../crates/buzz-relay/src/nip11.rs` 与 CLI / Desktop Meeting capability parser
- SDK tests / golden tag fixtures
- 如 CLI / Desktop 需要共享 enum，增加 documented public API

新增 `KIND_MEETING_SUMMARY_COMMAND = 42113`，并把它加入 author-signed command kind registry；不得把它
标为 relay-only projection kind。同步更新 `ALL_KINDS`、`is_command_kind` 与相关 compile-time
classification assertions。

### 14.3 Relay

- `../../../../crates/buzz-relay/src/handlers/command_executor.rs`
- 新增 Meeting summary command parser / handler，或放在清晰的 Meeting metadata module
- `../../../../crates/buzz-relay/src/handlers/ingest.rs`（`required_scope_for_kind = ChannelsWrite`、command-kind routing、
  Meeting fail-closed回归）
- `crates/buzz-relay/src/handlers/side_effects.rs::emit_group_discovery_events`
- `crates/buzz-relay/src/handlers/side_effects.rs::reconcile_channel_events`（检测已有但summary stale的39000）
- read / auth / tenant / event visibility tests

Relay 通过 `command_executor` 处理 exact kind `42113` schema；普通 `handle_edit_metadata` 和 Meeting 对
kind `9002` 的 fail-closed 行为保持不变。command kind 在 generic channel gate 前分流，因此 summary
handler 必须像其他 participant command 一样显式检查 authenticated `channel_ids`，不能只依赖后续 DB
host 校验。

### 14.4 CLI

- `crates/buzz-cli/src/lib.rs::MeetingsCmd`
- `crates/buzz-cli/src/commands/meetings.rs`
- `crates/buzz-cli/src/commands/project_context.rs`
- CLI parse / output / readback tests

CLI 影响还包括提取 / 复用 verified NIP-11 Relay identity，并让 list、show、batch Context hydration 走同一
39000 exact verifier；不能只给 `MeetingSummary` DTO 增加字段。

### 14.5 ACP

- `../../../../crates/buzz-acp/src/meeting_v1.rs`
- `../../../../crates/buzz-acp/src/meeting_context.rs`（只在稳定合同需要短规则时）
- prompt contract / coordinator tests

明确不改：

- `DirectActionOutput`；
- `parse_direct_action_output`；
- `prepare_v2_action_close`；
- End builder；
- prepared End ledger。

### 14.6 Desktop Native

- `../../../../desktop/src-tauri/src/commands/meetings/model.rs`
- `../../../../desktop/src-tauri/src/commands/meetings.rs` 及 snapshot projection loading
- `../../../../desktop/src-tauri/src/commands/meetings/directory.rs`
- `../../../../desktop/src-tauri/src/commands/meetings/context.rs`
- 新增 `../../../../desktop/src-tauri/src/commands/meetings/summary.rs` 或邻近 metadata module
- `../../../../desktop/src-tauri/src/commands/project_context/meeting_hydration.rs`
- `../../../../desktop/src-tauri/src/lib.rs` command registration
- Native unit tests

明确不向 `MeetingActionFinalizationAction` 增加 summary variant。

### 14.7 Desktop TypeScript / React

- `../../../../desktop/src/shared/api/tauriMeetings.ts`
- `../../../../desktop/src/features/meeting/hooks.ts` 中的 summary mutation与query invalidation
- `../../../../desktop/src/features/meeting/ui/MeetingActionFinalizationCard.tsx`
- `../../../../desktop/src/features/meeting/ui/MeetingTerminalSummary.tsx`
- `../../../../desktop/src/features/meeting/ui/MeetingScreen.tsx`
- Project Context Meeting Inspector / query model / picker adapter
- `../../../../desktop/src/testing/e2eBridge.ts`
- UI unit tests与 Meeting Playwright E2E

通用 Project Context graph node 已支持 summary，原则上只需数据贯通和 Meeting-specific fixtures。
`MeetingHeader` 仍展示 description；terminal presentation和Project Context Inspector另行展示 retrieval summary，
不能用一个字段覆盖另一个。

## 15. 测试设计

### 15.1 SDK

- SET 生成 exact Meeting identity、policy、action 与 Action fence tags，summary 位于 content；
- CLEAR 生成同一 fence、`action=clear` 与空 content；
- nil Meeting ID 拒绝；
- empty / whitespace SET 拒绝；
- NUL 拒绝；
- 不增加 summary 专属长度测试；
- builder 不允许组合普通 Channel metadata。

### 15.2 DB

- old Meeting summary 为 NULL；
- host 在 finalizing_actions SET 成功；
- SET→SET 更新；
- SET→CLEAR 为 NULL；
- SET same value / CLEAR on NULL 不改变 canonical summary，但会检查或修复 discovery head；
- ordinary active / floor_ready 拒绝；
- blocked / expired / lease-lost Action window 拒绝；
- terminal 拒绝；
- V0 / V1 / board-only V2 / legacy actions V2 拒绝；
- non-host / participant / Community admin 拒绝；
- cross-Community UUID collision 不泄漏、不串写；
- mutation 不改变 Board、State、floor/state revision、Action fence；
- mutation 不改变 Project Context revision / EdgeKey；
- transaction 失败不留下 event / summary 半状态；
- 与 Return / End 并发时只允许“summary先提交并保留”或“控制转换先提交、summary拒绝”；
- 上一轮延迟command不能通过新一轮Action Finalization的fence；
- duplicate signed event不重复应用 mutation，随后readback返回真正的current summary。

### 15.3 Relay

- Meeting summary command kind `42113` accepted；
- stale action run / window / Board fence 拒绝；
- kind `42113` 缺失、重复或未知 tag 拒绝；
- 缺少 `ChannelsWrite` scope 或 auth channel token 未包含 Meeting 时拒绝；
- Meeting name/about/visibility 等 kind `9002` 继续拒绝；
- wrong signer / wrong host / wrong Community 拒绝；
- ordinary Channel kind `9002` 行为无回归；
- NIP-11 只在 write surface ready 时广告 `buzz-meeting-summary-v1`；
- summary 更新后 kind `39000` 携带 summary；
- CLEAR 后 kind `39000` 省略 summary；
- 首次 emission 失败后，duplicate / no-op retry 与 startup reconciliation 都能修复 stale kind `39000`；
- raw event visibility保持 Community / channel scope；
- Meeting formal history不把 metadata event当 Speech。

### 15.4 CLI

- `--summary text`；
- `--summary -`；
- `--clear-summary`；
- mutual exclusion；
- NIP-11 extension缺失时update在签名/提交前返回unavailable；
- show / list 输出 Some / None；
- wrong Relay author / invalid signature / wrong `d` / duplicate summary tag拒绝；
- readback exact / delayed / unavailable；
- Relay definitive reject 映射；
- Project Context Meeting coordinate 输出 summary；
- active / terminal Meeting coordinate state不再硬编码为terminal；
- Project Context `meeting_fetch` 三条命令都包含 `--meeting` 且可被CLI解析；
- summary missing 不变成 unavailable。

### 15.5 ACP Prompt

- Prompt 包含先物化、readback、再 summary、再 COMPLETE 的顺序；
- 明确使用 `buzz meetings update` 与 `buzz meetings show`；
- summary 是 retrieval hint，不是 Board / ACK；
- current summary 准确时 KEEP；
- mutation/readback失败映射 BLOCK；
- Board 不充分映射 RETURN_TO_BOARD；
- 不要求把 summary 放入最终 JSON；
- `DirectActionOutput` parser golden 不变；
- End event golden 不变；
- `COMPLETE` 仍生成同一个 actions-recorded ACK shape。

### 15.6 Desktop Native

- Snapshot / List / Context Inspector summary Some / None；
- summary-only metadata head推进snapshot `authoritative_updated_at`；
- host finalizing_actions SET / CLEAR；
- summary extension缺失时不暴露可写能力；
- non-host拒绝；
- terminal拒绝；
- accepted / indeterminate / definitive reject；
- exact signed event retry；
- Confirm event不携带 summary，golden不变；
- Project Context hydration把 summary传到 Coordinate detail。

### 15.7 Desktop UI

- Human host在 Action Finalization 看见编辑器；
- Agent-host Meeting 的 Human observer不看见编辑器，摘要由主持Agent经CLI维护；
- participant / observer看不到编辑入口；
- existing summary预填；
- Save、Clear、Reset；
- dirty / saving / error状态；
- indeterminate write保留exact event并reconcile，不能当draft丢弃；
- Save不自动Confirm；
- Confirm不自动Save；
- dirty draft通过显式Discard后仍可走原Confirm/Return；
- null summary仍可Confirm；
- terminal只读显示；
- graph node / Coordinate Inspector显示 Meeting summary；
- missing summary不渲染空块。

### 15.8 E2E

至少覆盖：

1. Agent-host Action Finalization：业务物化 → summary SET/readback → 原 `COMPLETE` → closed；
2. Human-host Action Finalization：Desktop SET/readback → 原 Confirm → closed；
3. summary SET 后 ACK indeterminate：Meeting 仍 finalizing，summary仍可读，exact ACK retry正常关闭；
4. summary SET 后 Return to Board：外部效果和 summary保留，新 Board 可再次更新；
5. summary SET 后 operator abort：summary与 aborted lifecycle同时显示；
6. 从未进入Action Finalization的direct Floor close：summary为NULL；
7. SET → Return to Board → 后续direct Floor close：保留既有summary；
8. legacy Meeting：summary为NULL且读取兼容；
9. Project Context Edge中Meeting node显示summary，EdgeKey不变；
10. malicious summary按文本显示，不能改变UI或Agent控制。

## 16. 实施阶段

### Phase 1：来源字段与写事务

1. migration；
2. MeetingRecord summary；
3. SDK summary builder；
4. Relay kind `42113` command router；
5. host / phase checked DB transaction；
6. standard receipt与duplicate；
7. DB / Relay / SDK tests。

完成条件：CLI尚未接入时，底层 signed summary mutation 已能SET/CLEAR且不影响Meeting控制状态。

### Phase 2：CLI 与读取投影

1. `meetings update`；
2. `meetings show/list` summary；
3. kind `39000` projection；
4. Project Context CLI hydration；
5. readback与CLI tests。

完成条件：Agent手工使用CLI可以写、清除、回读summary，图查询可以看到来源摘要。

### Phase 3：Agent Action Finalization

1. dynamic Prompt；
2. stable Meeting contract短规则与版本 `4 -> 5`；
3. failure映射；
4. Prompt / coordinator regression。

完成条件：Agent在同一Action Finalization Turn内用CLI维护summary，最终输出和End协议保持原样。

### Phase 4：Human Desktop

1. Native / TS DTO；
2. summary mutation command/hook；
3. Action Finalization editor；
4. dirty/save/clear/error UX；
5. Project Context Desktop hydration；
6. unit / E2E。

完成条件：Human主持人保存summary后仍使用原Confirm关闭，非主持人无写入口。

### Phase 5：全链路验收

1. Agent + Human E2E；
2. ACK indeterminate / retry；
3. Return / abort中间状态；
4. Project Context graph展示；
5. compatibility / rollout验证；
6. `just ci` 与相关integration suite。

## 17. 验收不变量

实现完成后必须同时满足：

1. Meeting summary 只有一个 canonical owner：Meeting domain；
2. `meeting_sessions.summary` 可空，旧 Meeting 不需要回填；
3. Project Context 不存摘要副本；
4. Agent和Human共用同一SET/CLEAR mutation；
5. Agent通过CLI写，Human通过Desktop写；
6. summary只在Action Finalization阶段可写；
7. immutable host / moderator是首版唯一writer；
8. summary写入后必须可回读；
9. summary的存在不证明Meeting已关闭；
10. summary写成功、ACK失败是合法乐观状态；
11. DirectActionOutput、Human Confirm、End与ACK schema不变；
12. Meeting状态机、phase、fence、deadline和Action Run不变；
13. description、Board、Speech、summary、State/End语义不混用；
14. Project Context Meeting preview展示来源summary；
15. summary更新不改变EdgeKey或Context revision；
16. 缺失 summary 是 unknown，不是 irrelevant；
17. summary是untrusted data，不提供指令或权限；
18. terminal summary首版只读，修正协议不被偷偷夹带。

## 18. 明确拒绝的替代方案

### 18.1 不把 summary 放入 COMPLETE / End

这会把普通检索元数据耦合到 closure protocol，并迫使Agent与Human拥有不同写法。本设计已确认不采用。

### 18.2 不在 Confirm 时自动保存 draft

自动保存会把两个独立动作重新耦合，并让summary写失败改变Confirm请求语义。Human应显式Save/Clear，
然后使用原Confirm。

### 18.3 不复用 description

description描述创建时目标，不能在会后被静默改写成结果摘要。

### 18.4 不复用 Board

Board是完整权威工作文档，不是轻量检索摘要；读取summary也不能替代读取Board。

### 18.5 不给 Project Context Node 建摘要

否则会产生Meeting与Graph两份summary owner、revision漂移和权限复制。

### 18.6 不开放 Meeting kind 9002 metadata

summary 使用专用 kind `42113`。Meeting 的 name / about / visibility / archive 等普通 kind `9002`
metadata 继续受现有 fail-closed 边界保护。

### 18.7 不自动总结 Board / Speech

Relay不调用模型，不从最后Speech、Board首段或description机械合成summary。维护判断仍由主持Agent/Human完成。

## 19. 最终实现形态

```text
Agent host
  -> buzz meetings update --summary
  -> Meeting summary transaction
  -> buzz meetings show readback
  -> unchanged COMPLETE
  -> unchanged actions-recorded End

Human host
  -> Desktop summary editor Save/Clear
  -> same Meeting summary transaction
  -> Desktop refresh/readback
  -> unchanged Confirm
  -> unchanged actions-recorded End

Project Context
  -> incident/exact Edge query
  -> hydrate Meeting metadata title + summary
  -> Agent decides whether to load Board / Speech
```

本设计增加的是一个来源对象字段、一个窄元数据写入口、两个调用界面和读取水合；Meeting 的讨论、控制、
行动收口与关闭流程保持原样。

## 20. 本次交付结果

本次实现已经完成上述最小闭环：

- `meeting_sessions.summary` 作为唯一来源字段，历史 Meeting 保持 `NULL`；
- kind `42113` 提供带 Action Run / Window / frozen Board fence 的 SET / CLEAR 写入口；
- Relay 在同一事务内验证 Community、immutable moderator、Action Finalization phase 与 runnable fence，
  同时提交签名命令和来源字段；
- kind `39000` 作为可重建 discovery projection 暴露 summary，并由启动 reconciliation 修复缺失、畸形或
  stale 的 Meeting summary metadata；
- CLI 提供 `meetings update`、verified `show/list` readback，并把来源摘要水合进 Project Context preview；
- Action Finalization Prompt 指导主持 Agent 在物化及 canonical readback 后维护摘要，再沿原有
  `COMPLETE` 输出关闭；
- Desktop 为 Human moderator 提供独立 Save / Clear / Reset 编辑器，Confirm 与摘要保存保持两个动作；
- Meeting terminal view、Project Context graph、picker 与 Inspector 均展示来源摘要；
- `DirectActionOutput`、Human Confirm、Meeting End、State、Board、Action Run 与 ACK wire 均未加入 summary。

验证覆盖 SDK/CLI/ACP/DB/Relay/Desktop 的编译与单元边界、Meeting Action E2E 编译、Desktop
TypeScript 构建，以及 Human summary 保存/失败不阻断 Confirm 和 Project Context 展示的 Playwright
回归。
