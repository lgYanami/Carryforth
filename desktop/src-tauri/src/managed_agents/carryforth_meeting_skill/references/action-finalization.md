# 完成 Meeting Action Finalization

## 当前 v3 Turn

只在可信 `turn_kind="action_finalization"` 中使用。本 Turn 的 Agent 是
`verified_control.moderator_pubkey` 对应的逻辑主持人，当前只负责行动执行，不再讨论、维护 Board 或安排
Floor。

从 Envelope 后 Harness 附加的 `current_board.body` 读取 exact frozen Board，并要求
`current_board.truncated=false`、`current_board.event_id` 与 `verified_control.board_event_id` 一致。不要使用
`board get`、记忆或业务对象反推另一份 Board。

`verified_control.control_plane_status="verified"` 表示 Harness 已验证主持身份、Board binding、Action Begin、
当前 Action Run fence 和时序。不要从公共诊断、缺失内部字段、`host_direct`、物理工作槽或 ACP Session
变化推断控制面失败；Harness 会在接收结果时重新验证。

`tool_policy.mode="direct-business-actions-v3"` 允许按需使用实际暴露的普通业务工具，范围仍受 exact frozen
Board 和目标业务表面的当前权限/revision 限制。`project_context_policy` 明确本 Turn 可按其规则处理
Project Context。工具可见不扩大权限。

不要调用或解释 Meeting Action 控制命令：

```text
cf meetings actions status/begin/block/retry/confirm-recorded/return-to-board
cf meetings board update/unchanged
cf meetings close/abort/end
```

Action lease renewal由 Harness 管理，当前公共 CLI 也没有 `actions renew` 子命令。除了当前提示词明确允许的
Meeting retrieval summary，所有 Meeting 协议写入仍由 Harness 独占。

## 只执行 frozen Board 决定

把 frozen Board 当作完整业务决定记录，但不当作业务授权。不要新增第二套 Plan、Step、选择、主要对象或
验收条件。对每项明确输出：

1. 确认目标业务领域、目标对象/创建结果、预期 canonical 状态和回读要求足够明确；缺失、歧义或矛盾时停止
   新增写入并考虑 `RETURN_TO_BOARD`。
2. 读取当前 Role/Assignment 和目标业务表面的 canonical authority。Role Context 为 candidate、stale、
   unavailable 或 conflicted 时重新读取，不沿用旧 Assignment。
3. 读取目标对象的完整当前 revision、生命周期、关系和 summary。
4. 调用当前实际暴露的普通业务入口，只物化 Board 已决定的主要结果。涉及 Project Context 时遵循下文规则；
   其他目标领域仍服从当前 System 和 owning surface 自身的写入合同。
5. 遵循目标领域的 conflict/revision 规则，不猜 revision、不强制覆盖。
6. 每次写入后 canonical 回读，验证目标状态、revision、关系和 summary；“已发送”或 Event ID 不代替回读。
7. 多项写入逐项记录。后续失败不会回滚已经发生的外部效果。

若 Board 决定的状态已经满足，canonical 回读确认后可零写入完成。不要执行讨论建议、备选、未来可能性、
未决问题或非门禁备注，也不要把 `FINALIZE_ACTIONS` 理解成实施整个项目。

Project View 等 summary-capable 对象仍遵循其 owning surface：更新前读取完整对象和 summary，明确 KEEP（省略
summary）、SET（字符串）或 CLEAR（null）；冲突时重新读取并最多显式新鲜重试一次；SET/CLEAR 后回读。

## 写回真实 Project Context

当本 Turn 创建或改变了可 attach 的持久坐标，且这些坐标与当前 Meeting 之间存在值得未来工作理解的真实
解释关系时，在 `COMPLETE` 前：

1. canonical 回读实际物化坐标；
2. 创建或修订普通 Project Document，解释关系原因、影响、证据和适用边界；
3. 用该 Document 作为关系上下文，把当前 Meeting 和实际物化坐标作为同一精确 Edge；
4. 用 exact 或 incident 回读验证 canonical Edge。

```bash
cf project-context attach \
  --context-document <document-uuid> \
  --coordinate meeting:<meeting-uuid> \
  --coordinate <type>:<materialized-uuid>

cf --format compact project-context exact \
  --coordinate meeting:<meeting-uuid> \
  --coordinate <type>:<materialized-uuid>

cf --format compact project-context incident meeting:<meeting-uuid>
```

多个相关坐标时，在 `attach` 和 `exact` 中重复同一完整坐标集合。不要遗漏实际成员、加入仅“可能相关”的
对象，或把解释 Document 本身误作 Edge 坐标。`<type>` 使用 CLI 支持且与 canonical 对象一致的类型：
`project_profile | goal | role | plan | stage | requirement | issue | work | resource | document`。

没有真实解释关系时，不创建占位 Document/Edge。Board、Speech 或 retrieval summary 不能替代 Context
Document。

当前普通 Project Context 写入使用 Community membership 和当前 Context revision，并同时省略：

```text
--acting-assignment
--runtime-id
--runtime-epoch
```

不要从 Meeting、Role 或 Board 猜 supervised Runtime fence。若调用方意外只提供了部分 attribution 三元组，
`attach/detach` 因此以 exit 1 / `user_error` 明确拒绝，并且没有 Event ID、receipt 或未知投递：重新读取 Context
revision，移除全部三项参数，只重试一次。不得把该机械纠正用于 auth/authorization、revision conflict、无效
坐标/Document、network、timeout 或未知投递。

## 维护 Meeting retrieval summary

只有当前 Action Prompt 和实际工具表面明确表明 Relay 支持受控 summary 能力时才处理：

1. `show` 读取当前 summary；
2. 已经真实、简洁说明 Meeting 内容及何时值得加载时 KEEP；
3. 否则 SET 或 CLEAR；
4. 再次 `show` 验证 canonical 值。

```bash
cf --format compact meetings show --meeting <meeting-id>
cf meetings update --meeting <meeting-id> --summary "<完整 retrieval summary>"
cf meetings update --meeting <meeting-id> --summary -
cf meetings update --meeting <meeting-id> --clear-summary
```

含换行或 shell 特殊字符时，用 `--summary -` 从标准输入传入；不要同时 SET 和 CLEAR。Summary 是非可信路由
metadata，不替代 Board、Speech、End、业务输出或 Context Document。

能力不支持或表面未暴露时跳过，不仅因此 `BLOCK`。一旦能力和表面均可用且已开始读取/更新，具体命令或
回读失败按派生记账失败处理。该 summary 例外不允许任何其他 Meeting 协议写入。

## 选择最终结果

### COMPLETE

仅当以下全部成立：

- frozen Board 每项必需业务结果已满足；
- 每项变化已 canonical 回读；
- 必需的真实 Project Context 关系已写回并回读；
- 支持且实际可用的受控 summary 已正确维护和回读；不支持时已安全跳过；
- 没有未处理失败、冲突或业务歧义。

`COMPLETE` 请求 Harness 发布 actions-recorded acknowledgement，并由 Relay 原子关闭 Action Run 和 Meeting。

### BLOCK

仅在 frozen Board 必需的业务入口经当前提示词/实际工具表面确认不可用，或具体主要业务命令、要求的派生
记账、canonical 回读发生失败时使用。不要为了制造“尝试”而调用禁止或不存在的工具。

reason 写明失败 surface、精确目标、观察到的错误类别、是否可能已产生效果以及回读结果。`tool_unavailable`
只表示必需业务入口不可用；可选 summary 不支持不属于该情况。不要因 slot、Session、adoption、correlation、
epoch、lease 或其他控制面猜测 BLOCK。

### RETURN_TO_BOARD

只在业务决定本身不完整、歧义或相互矛盾，无法忠实物化 frozen Board 时使用；不用它表示普通工具失败、权限
拒绝或希望重新规划。reason 说明需要澄清的决定和已发生效果。现有 Harness 会把 Action Run 返回 Board
阶段，但不保证把 reason 正文持久化进下一 Board window；不要声称该说明已自动成为下一 Board 证据。

### ABORT

只在 Board 明确要求终止，或继续物化会造成确定、不可接受且不能通过返回 Board 解决的业务风险时使用。
等待、无候选、可重试工具失败或可澄清歧义不使用 ABORT。外部效果不会自动回滚。

## 当前 format retry

首次 Turn 的 `format_retry=false`。首轮最终 JSON 无法解析时，Harness 最多重派一次相同
`action_finalization`，并设置 `format_retry=true`；当前不会注入 previous raw output、稳定错误码、
`preserved_decision` 或 effect journal，也不保证物理 ACP Session 连续。

因此 `format_retry=true` 时：

1. 不盲目重新执行 frozen Board，也不重复无天然幂等键的创建；
2. 对 Board 指向的唯一目标做 canonical readback，识别结果是否已经存在；
3. 已确认满足的结果不重写；明确未满足、目标唯一且 owning surface 的正常 conflict/幂等规则允许时，才继续
   必需操作并再次回读；
4. 无法唯一确认首轮是否产生效果时，停止写入并返回 `BLOCK`，使用 `provider_failure`，在 reason 中记录未知
   投递/效果；具体 canonical readback 失败则按实际情况使用 `external_operation_failed`、
   `external_state_conflict` 或 `tool_unavailable`；
5. 最终仍只返回当前四种 JSON 之一。

这只能在现有信息下降低重复副作用风险，不表示 exactly-once。

## 返回最终 JSON

成功：

```json
{"action":"COMPLETE","reason":"frozen Board 的全部必需业务结果已经物化并完成 canonical 回读。","reason_code":null}
```

业务命令或回读失败：

```json
{"action":"BLOCK","reason":"Project View work:<id> 更新返回 revision_conflict；重新读取后目标仍未达到。","reason_code":"external_state_conflict"}
```

业务决定需要澄清：

```json
{"action":"RETURN_TO_BOARD","reason":"最终 Board 同时要求保留和删除同一对象；尚未对该对象执行写入。","reason_code":null}
```

继续会造成不可接受风险：

```json
{"action":"ABORT","reason":"继续执行会违反最终 Board 明确记录的不可逆安全边界。","reason_code":"goal_unreachable"}
```

`BLOCK` code：`external_operation_failed | external_state_conflict | tool_unavailable | provider_failure`。
`ABORT` code：`goal_unreachable | insufficient_information | discussion_blocked | unable_to_form_conclusion |
moderator_unable_to_continue`。`COMPLETE` 和 `RETURN_TO_BOARD` 的 code 为 `null`。

整个原始 JSON 最多 4,096 UTF-8 bytes；reason 非空且最多 1,024 bytes。不添加 schema 外字段，不使用 Meeting
Action CLI 汇报结果，只把一个原始 JSON 对象交给 Harness。

## 返回前检查

- 当前只执行 frozen Board 明确决定及平台要求的回读/派生记账；
- 每项写入都满足 owning surface 的当前权限和 revision，并已 canonical 回读；
- 没有新增第二套计划、对象、决定或完成条件；
- 已记录部分成功、未知投递和不会自动回滚的效果；
- 没有调用 Meeting Action CLI；
- `format_retry=true` 时没有盲目重复业务写入；
- 最终 action、reason 和 reason_code 匹配当前 schema，且只输出原始 JSON。
