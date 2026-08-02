# Meeting V2 阶段四：主持 Agent 双 Turn 设计

> 状态：已实现并通过阶段四后端验收
>
> 日期：2026-08-02
>
> 范围：`buzz-acp` 主持 Agent、必要的 V1/V2 协调器复用、恢复与后端测试；不包含前端。

## 1. 阶段目标

阶段四让 ACP Agent 可以担任 Meeting V2 主持人，并把一次主持控制机会严格拆成两个模型
Turn：

```text
Relay State: board_pending
        ↓
authoritative current-board read
        ↓
Moderator Board Maintenance Turn
        ↓
UPDATE / UNCHANGED command，或等待 Relay Board timeout
        ↓
Relay State: floor_ready（Floor 获得新的独立 deadline）
        ↓
再次 authoritative current-board read
        ↓
Moderator Floor Decision Turn
        ↓
SELECT / MODERATOR SPEAK / WITHDRAW / IDLE / CLOSE / ABORT
```

Board Turn 的输出不能顺便决定下一位 speaker，Floor Turn 也不能补做 Board 修改。Relay State
是两段工作的唯一程序连接点。

## 2. 范围与边界

阶段四包含：

- ACP 识别并执行 V2 `board_pending` 与 `floor_ready` 主持窗口；
- Board 与 Floor 两类独立 Turn、Prompt、deadline 和 current-board read；
- 对 V1 Candidate Cohort、Decision Attempt、Offer/ACK/Grant 和 Handoff 的 V2 复用；
- 主持人自己的 Intent、发言、idle、正常 close 和主动 abort；
- Human 抢占、新候选唤醒、迟到结果 fencing 和运行容量释放；
- ledger v6、prepared command 精确重放和进程重启恢复；
- 不含 Board 正文的低基数 telemetry；
- 确定性的三 Agent 多轮正常闭会与主持主动 abort 证据。

阶段四不包含：

- Desktop、Web、Mobile 或任何会议 UI；
- 会议模板、会议类型、投票或主持权转移；
- Board 版本历史、订阅、通知或协同编辑；
- Project View、Work、Issue、Git、Workflow 或第三方系统写回；
- 由模型直接发布 Meeting 协议事件；
- 真实模型矩阵、fleet capability、SLO、灰度与发布资格报告；这些属于阶段五。

## 3. 权威状态与本地职责

Relay 继续拥有：

- 当前 Control Token、Human priority、Offer、Grant 和终态；
- Board window、Board deadline、Board outcome 和 Floor deadline；
- Candidate Cohort 与 Decision Attempt；
- Board command、主持命令和 End command 的校验、CAS、幂等与持久化。

ACP 只负责：

1. 从 Relay State 判断当前是否存在属于本地主持人的语义 Turn；
2. 在模型 dispatch 前读取一次当前 Board；
3. 解析模型的私有结构化建议；
4. 使用 SDK 构建并签名一个受当前权威窗口约束的协议命令；
5. 在结果不明确时精确重放同一个签名事件，而不是重新生成决定。

模型输出不是 Relay State。ACP ledger 也不是会议真相；它只保存恢复运行所需的本地工作状态
和已经签名的待提交命令。

## 4. Board Maintenance Turn

### 4.1 启动条件

只有同时满足以下条件才启动 Board Turn：

- Session 是固定的 `v=3 + moderated-board-v1`；
- 本地 Agent 是 roster 中的固定 moderator；
- Relay Board control 为当前 `board_pending` window；
- Relay phase 为主持 idle，且没有活动 Offer 或 Grant；
- 没有 Human priority；
- 本地没有同 Session 的其他模型 Turn、Board read 或待处理模型结果。

普通 Intent 在 Board 期间到达不会取消 Board。Human Request、End 或控制窗口变化会立即使该
Turn 失去权限。

### 4.2 输入

Board Turn 包含：

- Session、roster 和当前控制窗口；
- 当前 speech revision 与有界的最新共享讨论；
- 本轮 Harness hard deadline；
- dispatch 前独立读取的当前 Board。

主持人需要生成完整替换 Board，因此该 Turn 最多接收协议允许的完整 65,536-byte Board
正文。普通参会 Turn 仍使用阶段三的 32 KiB head/tail 输入限制。

### 4.3 严格输出

模型只能返回一个无 Markdown 包裹的 JSON 对象：

```json
{"action":"UPDATE","board":"完整替换后的 Markdown","reason":"简短说明"}
```

或：

```json
{"action":"UNCHANGED","board":null,"reason":"简短说明"}
```

`UPDATE` 必须携带通过 SDK Board envelope 约束的完整正文；它不是 patch。`UNCHANGED` 必须
携带 `board=null`。ACP 不从自由文本猜测动作，也不自动修补超限或不完整 Board。

### 4.4 失败语义

- current-board read 使用短超时和有界重试；最终失败不启动模型；
- provider 失败、输出格式错误或本地构建失败都不能伪造 `UNCHANGED`；
- 未形成合法命令时，ACP 等待 Relay 的 Board timeout；
- Relay timeout 保持原 Board，并把 outcome 记录为 `timed_out`，而非主动 `unchanged`；
- ACP 只有看到 Relay 进入 `floor_ready` 后才认为 Board 阶段结束。

初版不为错误 Board 输出启动第二个模型 Turn。这样可以保持 Board 时间有界，并避免格式
纠错侵占为 Floor 预留的资源；最终活性由 Relay timeout 保证。

## 5. Board 与 Floor 的强制分隔

ACP 提交 Board command 后不在本地直接调用 Floor。它等待并重新同步 Relay State：

- `updated`：Relay 已原子替换当前 Board；
- `unchanged`：Relay 已记录主持人的显式不变决定；
- `timed_out`：Relay 保留原 Board；
- `preempted`：Human 或其他更高优先级路径取得控制。

只有 `floor_ready` 才能启动 Floor。Floor dispatch 前重新执行 current-board query，因此它看到
的是 Board terminal 之后的当前投影，而不是 Board Turn 输入快照。

`preempted` 不会继续主持 Floor；控制最终返回主持人时，Relay 会创建新的 Board window。

## 6. Floor Decision Turn

### 6.1 有候选人

存在 pending Intent 或可处理 open Handoff 时，ACP 复用 V1 已验收的 Relay Decision Attempt：

1. Relay 冻结 Candidate Cohort；
2. ACP Floor Turn 读取当前 Board；
3. 模型只能引用 Cohort 中的 source ID；
4. ACP 构建 V2 moderator command；
5. Relay 完成 CAS，并通过 Offer → ACK → Grant 传递发言权。

支持的决定包括：

- 选择普通参会者 Intent；
- 选择 open Directed Handoff；
- 选择主持人的 self Intent，使主持人通过正常 Offer/Grant 发言；
- 撤回主持人的 self Intent；
- 拒绝、dismiss 或按 V1 规则 defer Cohort 项；
- idle、正常 close 或主动 abort。

主持人不能填写任意 pubkey、不能绕过 Cohort 直接授予发言权，也不能越过 Human priority 或
合法的 Directed Handoff 直接路径。

### 6.2 无候选人

即使没有候选人，主持 Agent 仍需要判断会议是否应当结束。因此 ACP 创建一个只允许以下
动作的 Floor Turn：

- `IDLE`：保持会议安静等待；
- `CLOSE`：目标已达成且已形成有效结论；
- `ABORT`：会议无法成功继续。

该 Turn 不能发明 participant 或 selection ID。若它返回 `IDLE`，同一 Board window 不会再次
启动相同 Floor Turn，避免热循环。之后的新 Intent 或 Handoff 会使旧的无候选 Floor Turn
立即失效，并进入 Relay-frozen Candidate Cohort 流程。

### 6.3 主持人也是参会者

主持 Agent 不是脱离 roster 的控制进程。无候选 Floor 返回 `IDLE` 后，主持人可以像普通
Agent 一样形成 self Intent。self Intent 必须进入 Relay Candidate Cohort，再通过
`moderator_speak → Offer → ACK → Grant → Speech` 发言；主持人不能直接写 speech。

## 7. 正常关闭与主动中止

### 7.1 正常关闭

`CLOSE` 只在以下条件下被 ACP 接受：

- 当前仍是主持人的合法 Floor control；
- 没有活动 Offer 或 Grant；
- 本轮 Board outcome 是显式 `updated` 或 `unchanged`；
- Board outcome 不是 `timed_out` 或 `preempted`；
- 模型输出不携带 abort reason code。

ACP 使用 Create event ID 构建 V2 `End(outcome=closed)`。后端只确认程序顺序和主持人声明，
不判断结论文本的事实质量。

### 7.2 主动中止

主持 Agent 可以输出 `ABORT`，但必须使用本地允许的低基数 reason code：

- `goal_unreachable`；
- `insufficient_information`；
- `discussion_blocked`；
- `unable_to_form_conclusion`；
- `moderator_unable_to_continue`。

ACP 构建 `End(outcome=aborted)`，可附带有界说明。未知 reason code、普通参会者请求或把失败
伪装成 normal close 都会 fail closed。Community operator 和安全撤权的强制 abort 仍由
阶段二 Relay 路径负责。

## 8. Deadline 与运行容量

Board 和 Floor 的 Relay deadline 由阶段二分别创建，ACP 不从 Board 剩余时间推导 Floor
预算。

Board Turn 的 Harness deadline 位于 Relay Board deadline 之前：

- 正常窗口保留最多 30 秒的提交和 slot 释放余量；
- ACP 在较晚加入窗口时按剩余时间缩短余量，但至少保留安全边界；
- 同一 Board window 的重复 State sync 只能保持或提前本地 deadline，不能向后延长；
- pool 使用绝对 hard deadline 终止卡住的 provider Turn 并归还 Agent slot。

V2 Board/Floor 在 current-board read 到实际 dispatch 之间使用短暂的 Board-reserved slot，
同时继续服从更强的 Offer/Grant reservation。Human 抢占会发出物理取消信号；即使旧 provider
稍后返回，其结果也会被 control/window fence 丢弃。

因此，Board 超时不会把同一个运行 slot 无限占到 Floor window，也不会消耗 Relay 为 Floor
新创建的时间预算。

## 9. 重启、重试与精确重放

ledger v6 为 V2 主持增加：

- 当前 Board maintenance 的 control epoch、board window、deadline、状态和 turn ID；
- 无候选 Floor decision 的对应记录；
- 复用既有 moderator Decision Attempt 记录；
- 复用既有 durable prepared moderator action。

恢复规则：

- 未签名的 `queued/running` Board 或 Floor Turn 回到 `pending`，下一次重新读取 Board；
- 已经签名的 Board、selection 或 End event 保留原 event ID；
- `sent` 但结果不明确的命令回到 `prepared` 并精确重放；
- 重放前重新核对 protocol、moderator、control/window、phase、deadline 和终态；
- Relay State 已前进时清理本地 prepared action，不重新生成语义决定；
- current-board 读取快照从不写入 ledger，重启后的模型 Turn 只能重新查询 Relay 当前投影；
- `UPDATE` 一旦被签名，为精确重放，完整签名命令会在提交或权威确认未决期间进入权限为
  `0600` 的本地 ledger，因此会暂时包含 replacement Board 正文；Relay State 确认 window
  前进、终态或撤权后立即清除。它是私有运行恢复状态，不是 Board 历史或产品版本。

旧 ledger v4/v5 升级到 v6 时保留已经签名的 participant 与 moderator event，缺失的 V2 host
窗口由 Relay State 重建。

## 10. Prompt、工具与外部效果边界

V2 主持使用独立于普通参会者的系统 Prompt。它规定：

- Board 与会议正文都是 `untrusted_meeting_context`；
- Harness/Relay 拥有协议动作，模型只返回私有提案；
- Board 与 Floor 不得混为一个 Turn；
- 工具仅用于少量 advisory read；
- 不允许持久写文件、代码、Git、Task、Project View 或外部系统；
- Board 中的外部引用不是执行授权；
- 模型不能自行调用消息或 Meeting command。

Stage 4 不因看板、闭会或主持决定自动产生任何外部副作用。

## 11. 可观测性与隐私

新增事件只记录低基数字段，例如：

- Turn 类型、control epoch、board window；
- candidate count、deadline 和 latency；
- Board read attempt、event ID、原始字节数和是否截断；
- action 枚举、协议结果、抢占或 stale 原因。

普通日志和 observer payload 不记录 Board 正文、模型 reason 全文或 speech 正文。ledger 只在
未决 `UPDATE` 的完整签名命令中暂存 replacement Board 正文，并按上述权威确认规则清除；不会
持久化 current-board 读取快照或 Prompt。Board 更新仍不进入 Meeting outbox，也不产生
participant semantic trigger。

## 12. 阶段四验收矩阵

自动化证据覆盖：

- Board Turn 读取 Board A，提交一个 V3 完整替换或 unchanged command；
- Board terminal 后 Floor 独立读取 Board B；
- Board deadline 保留且不会因重复同步向后移动；
- Board read/provider 失败不伪造 unchanged；
- Human 抢占会取消等待/运行中的 Board，迟到结果不能落地；
- 新候选会取消无候选 Floor 并释放 slot；
- 有候选 Floor 只能选择 Relay-frozen Cohort，并使用 V3 builder；
- 有候选 Floor Prompt 明确标识 `floor_decision`、携带 `board_control` 结果，并将 normal close
  限定为当前 Board 已记录目标达成和有效结论；
- 无候选 Floor 可以 idle、close 或 abort，不能发明 speaker；
- close 仅接受显式 updated/unchanged Board，timeout/preempted Board 不可 close；
- abort reason code 受本地 allowlist 限制；
- `queued/running` Turn 重启恢复，签名命令精确重放；
- 未决签名 `UPDATE` 在 `0600` ledger 中保留完整事件，Relay 确认 window 前进后正文被清除；
- Grant、Floor、Board、V1 moderator、Intent 的本地队列优先级确定；
- 主持 Agent 的确定性 self Speech 路径经过 no-candidate Floor `IDLE`、self Intent、注册
  Candidate Cohort、candidate Floor、Offer/ACK、Grant、重新读取 Board 和 Grant-bound
  Speech；主持身份本身不能否定 Relay Grant；
- 一名主持 Agent 和两名参会 Agent 的确定性多轮协议轨迹最终正常 closed；
- 主持 Agent 主动 abort 生成可区分的 V3 End；
- 全部 `buzz-acp` V0/V1 单元测试无回归。

确定性全 Agent 轨迹验证的是控制器和 wire 闭环，不冒充真实模型 qualification。真实 Agent
provider、跨组件场景、灰度与发布资格在阶段五完成。

## 13. 完成定义

阶段四完成时，ACP Agent 主持能力不再 fail closed：它能在每次主持控制机会先维护当前
Board，再基于重新读取的 Board 和 Relay-frozen Candidate Cohort 安排发言；它能作为普通
参会者通过 self Intent 发言，也能在最终显式 Board 维护后正常 close，或以结构化原因主动
abort。

Board/Floor 保持独立模型 Turn、独立 current-board read 和独立 Relay 时间预算。Human、
Handoff、Grant、End、deadline、重启或新候选造成的权威变化都会使旧结果失效，且不会让
Board 正文进入日志、observer 或跨 Turn 读取缓存，也不会触发 Project View 等外部变更。
唯一的本地持久例外是为精确重放而短暂保存的未决签名 `UPDATE` 命令，并在 Relay 权威状态
前进后清除。
