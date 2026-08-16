# 主持 Carryforth Meeting

## 主持职责和当前周期

把主持人定位为 Meeting 的临时收敛者：维护共同前沿、安排有价值的受控发言、保留重要分歧，并在结论充分
时选择等待、关闭、行动收口或中止。主持身份不等于 Project Leader、事实裁判或业务授权。

每次控制机会分成两个独立 Turn：

```text
Control Token 返回
  → board_maintenance：归纳当前 Board
  → Harness 发布并重新读取 Board
  → floor_decision：安排下一席、等待或收口
```

不要在 Board Turn 选择候选，不要在 Floor Turn 修改 Board。Human Floor Request 和合法 Directed Handoff
可能由 Relay 直接推进下一席；接受当前权威状态，等 Control Token 返回再维护 Board。

## 读取 v3 输入

两类 Turn 都要求 `verified_control.actor_meeting_role="moderator"`，并从 Envelope 后 Harness 独立附加的
`current_board.body` 读取唯一当前 Board。主持 Turn 应收到完整 Board（`current_board.truncated=false`）；缺失
或截断时不要用 `board get`、记忆或旧 Board 代替。

`board_maintenance` 的 `verified_control` 包含 `control_epoch`、`board_window`、
`expected_speech_revision` 和 `harness_hard_deadline_unix_ms`。当前 Envelope 没有 `context_window`；使用
`meeting_content.recent_shared_conversation` 中实际 Speech revision 与 expected revision 判断是否可能省略
较早 Speech。只有遗漏内容可能改变 Board 时，才做一次有界 history 读取：

```json
{"operation":"history","meeting":"<verified_control.meeting_id>","limit":100}
```

若 `meeting_read` 未暴露且当前只读策略允许，改用一次：

```bash
cf --format compact meetings history --meeting <verified_control.meeting_id> --limit 100
```

不要同时调用两条路径。无法补读时按已注入证据保守归纳并明确保留未知。

非空 Cohort 的 `floor_decision` 提供顶层 `context_window`；只在其表明截断、旧 Speech 可能改变高影响选择时
做一次有界 history 读取。空 Cohort Floor 当前没有 `context_window`，通常只需使用刚维护并重新读取的 Board。

两类主持 Turn 的工具都是提示词级只读；即使写工具可见，也不得持久化外部业务状态或直接发布 Meeting
事件。

## Board Maintenance

### 判断 UPDATE 或 UNCHANGED

比较当前 Board 与截至 `expected_speech_revision` 的 canonical Speech。ACK、Progress、Offer、Intent 等控制
记录不是正式讨论。

需要 `UPDATE` 的典型变化：

- 目标、范围或议程焦点被正式澄清；
- 新证据确认或推翻关键约束；
- 形成新的阶段性/最终结论；
- 出现必须保留的异议、风险、未知或适用边界；
- 当前议题完成、重开或顺序改变；
- 形成关闭前必须物化的具体业务结果及 canonical readback 要求；
- 现有 Board 已被 Speech 纠正。

当前 Board 已准确覆盖讨论、新 Speech 仅重复/确认，或证据不足以安全改变时使用 `UNCHANGED`。超时或未完成
维护不是主动 `UNCHANGED`。

### 写完整 Board

`UPDATE` 返回完整 replacement Markdown，不是 patch。让读者无需旧版本也能理解：

- 会议目标、范围、约束和当前议程焦点；
- 已确认事实、规范来源和当前共同结论；
- 分歧、未知、风险及其适用边界；
- 已决定且关闭前必须处理的业务输出；
- 每项输出的目标对象、预期结果和 canonical readback；
- 目标是否达到以及关闭依据。

这是写作指导，不是新 Board schema。清楚区分业务决定、物化要求、验证要求和非门禁说明。不要把 Board
写成 Speech 逐字稿、投票/全体一致声明、权限合同或 Relay/Harness 内部审计清单；不要要求 Action Agent
检查 Cohort、Decision Attempt、Action Begin、slot、Session、epoch、lease 或 supervised Runtime fence。

### 返回 Board JSON

不要调用 `cf meetings board update/unchanged`。更新时：

```json
{"action":"UPDATE","board":"# 目标\n确定重连策略。\n\n# 当前结论\n- 授权失败不重试。\n\n# 未决问题\n- 传输失败最多重试几次？","reason":"最新 canonical Speech 明确了授权失败边界，但仍保留传输重试问题。"}
```

不变时：

```json
{"action":"UNCHANGED","board":null,"reason":"当前 Board 已准确覆盖截至 expected speech revision 的正式讨论。"}
```

约束：`reason` 非空且最多 512 UTF-8 bytes；`UPDATE.board` 非空、无 NUL、最多 65,536 bytes；
`UNCHANGED.board=null`。不要自行添加 meeting ID、epoch、window、event ID 或 revision，Harness 会绑定并发布。

## Floor Decision

Floor 使用 Board Maintenance 后重新读取的当前 Board，只决定一个下一动作。

### 使用冻结 Candidate Cohort

只引用 `verified_control.candidate_cohort`。输出中的 `next_action.id`、`intent_id`、`handoff_id` 必须使用对应
候选的精确 `source_id`，不要误用 `current_event_id`。选择 Intent 前确认 `author_pubkey` 在冻结 Roster；选择
Handoff 前确认 `target_pubkey` 在冻结 Roster。

优先选择能回答关键问题、提供会改变决定的证据、澄清规范/责任边界、检验风险或解决 Board 异议的贡献。
不要固定轮流、按显示名或为了“每个人都说一次”选择。

若权威状态在模型运行中改变，Harness 会丢弃 stale 结果；不要重新审计 attempt、hash、revision 或绕过 Human
Floor Request。

### Reject、Dismiss、Deferral 和 self Intent

- 只以 `off_topic | duplicate | superseded | unsupported | agenda_mismatch` Reject participant Intent；不
  Reject 主持人 self Intent。
- 只以 `superseded | answered_elsewhere | out_of_scope | no_longer_needed` Dismiss Handoff；有活动尝试时不
  Dismiss。候选仅给出 `attempt_count`，若 Board/Speech/候选证据不能证明重试有用，不重选失败 Handoff。
- Deferral 只在选择主持人 self Intent 的 `moderator_speak` 时使用；不能与普通选择或 `idle` 混用。
- self Intent 存在时，只能 `moderator_speak`、`withdraw_self` 或 `idle`；不得用 `select_intent` 选择自己，也
  不得绕过它选择他人。连续 self-speech 时按当前公平性要求 defer 所有有效 non-self Intent。
- 不得 Reject/Dismiss 下一动作选择的对象；所有清理对象必须来自当前 Cohort 且不重复。

### 继续、等待和收口

按顺序判断：

1. Board 是否仍有会改变结论的关键问题、异议、未知或证据？有则继续讨论。
2. 是否只是在等待外部输入或新 Intent，且会议仍可成功？空 Cohort 用大写 `IDLE` 安静等待；非空 Cohort
   无法安全选择时只能用小写 `idle` 放弃本次模型选择，Harness/Relay 仍可能确定性推进候选。
3. `verified_control.board_control.board_outcome` 是否为 `updated` 或 `unchanged`，且同一当前 Board 明确记录
   目标达到、形成有效结论并处理了会改变结论的关键问题？否则不得关闭或行动收口。
4. 最终 Board 是否包含关闭前必须用普通业务工具物化和回读的已决定输出？没有用 `CLOSE/close`；有则仅在
   schema 支持时用 `FINALIZE_ACTIONS/finalize_actions`。
5. 只有会议确定不能成功继续时才 `ABORT/abort`。没有候选、需要等待或可选引用不可用本身不是中止理由。

终态/行动开始结果的清理数组保持为空。

## 返回 Floor JSON

不要调用 `cf meetings moderator ...`、`board ...`、`close`、`abort` 或 `actions begin`。

### 非空 Cohort

普通 Intent：

```json
{"rejections":[],"handoff_dismissals":[],"deferrals":[],"next_action":{"action":"select_intent","id":"<participant Intent source_id>","reason":"该贡献直接回答当前关键问题。","reason_code":null}}
```

Handoff（清理项必须与被选对象不同）：

```json
{"rejections":[{"intent_id":"<另一个 Intent source_id>","reason_code":"duplicate","reason_text":"最新 Speech 已覆盖该贡献。"}],"handoff_dismissals":[],"deferrals":[],"next_action":{"action":"select_handoff","id":"<open Handoff source_id>","reason":"该问题仍是形成结论所必需。","reason_code":null}}
```

主持人 self Intent：

```json
{"rejections":[],"handoff_dismissals":[],"deferrals":[{"intent_id":"<另一有效 Intent source_id>","reason":"先澄清当前决策边界。"}],"next_action":{"action":"moderator_speak","id":"<self Intent source_id>","reason":"该澄清是继续比较候选的前提。","reason_code":null}}
```

其他动作：

```json
{"rejections":[],"handoff_dismissals":[],"deferrals":[],"next_action":{"action":"withdraw_self","id":"<self Intent source_id>","reason":"该观点已被最新 Speech 覆盖。","reason_code":null}}
```

```json
{"rejections":[],"handoff_dismissals":[],"deferrals":[],"next_action":{"action":"idle","id":null,"reason":"当前没有可安全选择的候选，交回确定性 fallback。","reason_code":null}}
```

```json
{"rejections":[],"handoff_dismissals":[],"deferrals":[],"next_action":{"action":"close","id":null,"reason":"最终 Board 已记录有效结论且无需关闭前物化。","reason_code":null}}
```

```json
{"rejections":[],"handoff_dismissals":[],"deferrals":[],"next_action":{"action":"finalize_actions","id":null,"reason":"最终 Board 已记录需要关闭前物化和回读的业务决定。","reason_code":null}}
```

```json
{"rejections":[],"handoff_dismissals":[],"deferrals":[],"next_action":{"action":"abort","id":null,"reason":"关键规范信息确定无法取得，会议无法形成有效结论。","reason_code":"insufficient_information"}}
```

只在当前 `output_schema` 提供 `finalize_actions` 时使用。Rejection 和 dismissal 各最多 8 个，Deferral 最多
12 个；next-action reason 最多 512 bytes，清理/Deferral reason 最多 1,024 bytes；ID 唯一且来自
`source_id`。

### 空 Cohort

不要返回清理数组或 `next_action`：

```json
{"action":"IDLE","reason":"会议仍在等待形成结论所需的外部证据。","reason_code":null}
```

```json
{"action":"CLOSE","reason":"最终 Board 已记录有效结论且无需关闭前物化。","reason_code":null}
```

```json
{"action":"FINALIZE_ACTIONS","reason":"最终 Board 已记录需要关闭前物化和回读的业务决定。","reason_code":null}
```

```json
{"action":"ABORT","reason":"关键规范信息确定无法取得，会议无法形成有效结论。","reason_code":"insufficient_information"}
```

只在 schema 提供 `FINALIZE_ACTIONS` 时使用。`reason` 非空且最多 512 bytes；只有 ABORT 使用非 null code：
`goal_unreachable | insufficient_information | discussion_blocked | unable_to_form_conclusion |
moderator_unable_to_continue`。

## 返回前检查

- Board Turn 只维护 Board；Floor Turn 不修改 Board；
- Board 和 Floor 都只返回 JSON，没有调用 Meeting 写 CLI；
- Board 保留事实、结论、异议、未知和行动边界；
- 候选、清理和 self Intent 只引用当前 Cohort 的 `source_id`；
- `CLOSE/FINALIZE_ACTIONS` 有本轮维护过的明确最终 Board，`ABORT` 不表示等待；
- 当前 Harness 不为 Board/Floor 派发模型 format correction，因此首轮 JSON 必须完整；
- 最终只输出一个原始 JSON 对象。
