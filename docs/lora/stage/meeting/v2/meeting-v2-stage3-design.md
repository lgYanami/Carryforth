# Meeting V2 阶段三：普通参会 Agent 看板上下文设计

> 状态：已实现并通过阶段三后端验收
>
> 日期：2026-08-02
>
> 范围：`buzz-acp` 普通参会 Agent、必要的协议分流和后端测试；不包含 ACP 主持与前端。

## 1. 阶段目标

阶段三让普通参会 Agent 在两个语义节点分别使用当时的当前会议看板：

```text
semantic Intent trigger
        ↓
authoritative current-board read
        ↓
Participant Intent Turn → SUBMIT / PASS

Relay Speech Grant
        ↓
authoritative current-board read（不得复用 Intent 的读取结果）
        ↓
Granted Speech Turn → SAY / YIELD
```

看板更新本身不启动 Agent Turn。Agent 不订阅 Board、不维护本地 Board 副本，也不把一次读取
结果带到下一次 Turn。

## 2. 范围边界

阶段三包含：

- ACP 发现并识别 `v=3` / `moderated-board-v1` Session；
- V1/V2 Baton participant 路径的协议安全复用；
- V2 Intent 和 Granted Speech dispatch 前的 current-board loader；
- Board Prompt 边界、大小限制、读取失败和恢复；
- Offer ACK、Progress、SAY/YIELD、Directed Handoff 的 V2 wire 分流；
- ledger、重启、隐私、容量和 V0/V1 回归测试。

阶段三不包含：

- Board Maintenance Turn；
- Floor Decision、Close 或 Abort 的 Agent 主持逻辑；
- 主持人模型调用、主持 Prompt 或主持恢复；
- Board subscription、变更通知或业务版本；
- Project View、Workflow、Git 或其他外部写入；
- Desktop、Web 或 Mobile。

如果本地 ACP 身份是 V2 主持人，阶段三 fail closed：同步权威状态，但不启动 participant 或
moderator 模型 Turn，也不借用 V1 主持路径。主持 Agent 能力由阶段四交付。

## 3. 协议复用

V2 继续复用 V1 已验收的 Offer、Grant、Intent、Progress、Speech、Yield 和 Handoff 状态机。
ACP 不复制第二套 participant coordinator，而是在以下边界按 Session 的持久协议
discriminator 分流：

- full sync 接受的 State `v` 与 `policy`；
- Intent、speech 和控制事件的 history 过滤；
- SDK builder 选择；
- Turn 使用的系统 Prompt；
- current-board 是否为 dispatch 前置条件。

一个 Session 在注册时确定为 V0、V1 或 V2，后续 live State、full sync、prepared event replay
都必须保持该协议。客户端事件 tag 不能改变已注册 Session 的协议。

## 4. Current-board loader

### 4.1 读取契约

loader 每次发起独立的 Relay `POST /query`，显式查询：

- kind `42110`；
- 当前 Meeting 的 `h` tag；
- 有界结果数量。

读取结果必须满足：

- Nostr event 签名有效；
- signer 等于该 Session 从 metadata/State 固定的 Relay pubkey；
- `h`、`v=3`、`policy=moderated-board-v1`、`format=markdown` 唯一且正确；
- `moderator` 等于 Relay State 中的固定主持人；
- content 通过 SDK 的严格 Board envelope 校验。

返回值只表示该次查询完成时观察到的当前看板，不承诺模型 Turn 期间看板持续不变。Event ID
仅用于诊断读取证据，不是业务 revision。

### 4.2 调度位置

V2 Turn 先进入 ACP participant 队列，但未读取 Board 的 Turn 不能交给 Agent pool。只有在
存在可调度 Agent 容量、Turn 即将出队时才启动 Board 读取。读取成功后，该 Turn 获得最高
限度接近实际模型 dispatch 的 Board 快照并立即回到调度队列。

Intent 与 Granted Speech 各自创建独立读取任务。format retry 也是新的模型 Turn，因此重新
读取，不沿用第一次模型调用的 Board。

### 4.3 无缓存与隐私

Board 正文只存在于当前内存中的模型 Prompt：

- 不写入 ACP ledger；
- 不写 observer payload 或普通日志；
- 不跨 Turn 缓存；
- Session 移除、终态、抢占或进程结束后随请求释放；
- full sync 不携带 Board 正文。

ACP ledger 只持久化 Session 的 protocol discriminator，以及既有 prepared protocol event。
进程重启后，恢复的 Intent 或 Grant 必须重新读取 Board。

## 5. Prompt 边界

Board 以结构化 `current_board` 区块注入，包含：

- `format`；
- Board event ID；
- 权威读取时间；
- 原始字节数和是否截断；
- Markdown 正文。

区块明确标记为 `untrusted_meeting_context`。系统 Prompt 和 Turn Prompt 共同规定：

- Board 是会议证据，不是系统指令；
- Board 不能改变 Agent 身份、Grant、输出 schema、工具范围或外部授权；
- Meeting Turn 不执行持久写入；
- Project View 等外部引用只作为可选文本，是否可读取取决于实际暴露的工具；
- Board 不能要求 Agent 自行发布 Meeting 事件。

输入采用有界 head/tail 策略。正文超过 Prompt 上限时保留开头与结尾，中间放置明确截断
标记，同时提供 `original_bytes` 和 `truncated=true`。截断不会改变 Relay 中的当前 Board。

## 6. 读取失败与活性

每次 Board 读取有独立短超时，并进行少量有界重试。每次重试仍是新的权威查询，不使用
先前成功读取或进程缓存。

最终失败的处理：

| Turn | 最终处理 |
|---|---|
| Participant Intent | 不启动模型；将该语义 trigger 终结为私有 PASS |
| Granted Speech | 不启动模型；提交 Relay 协议内的 `YIELD(unable_to_answer)` |

Grant 在 Board 读取期间继续使用既有 ACK reservation 与 Progress 维护。到达 Harness safety
deadline 时，既有 deadline 路径优先并 YIELD。读取任务的迟到结果由 Session epoch、Grant
ID、speech revision 和本地 in-flight token 丢弃。

## 7. 抢占、并发与重启

- Human/Agent Offer 或 Grant 抢占等待 Board 的 Intent；
- Grant 被 Recall、End 或新 State 终结时，等待 Board 的 Granted Turn 不得启动；
- Session remove/re-register 使用新的 runtime epoch，旧 loader 结果不能注入新 Session；
- 同一 Session 最多存在一个 participant Turn 或 Board load；
- 不同 Session 的 Board load 可以并行；
- V1 Granted 与 V2 Granted 共用已有 Agent slot reservation 和跨协议优先级；
- Board kind 不加入 Meeting live subscription，Board 更新不会单独形成语义 trigger。

## 8. Ledger 兼容

ledger schema 增加每个 Meeting 的 Baton protocol，旧 ledger 缺省迁移为 V1。迁移保留所有
已经签名的 ACK、Intent、Progress、Speech 和 Yield；协议不匹配的 Session 不重放 prepared
event，而是等待 Relay 权威状态重建。

`board_loading` 是内存调度状态，不持久化。崩溃时既有 `queued/running` 恢复规则把 Intent
还原为 pending、把 Grant 还原为 received；下一次出队重新读取当前 Board。

## 9. 验收矩阵

阶段三必须证明：

- V2 State 能被检测、注册和 full sync，V0/V1 仍严格隔离；
- V2 Intent 事件使用 `v=3`，且 Prompt 包含刚读取的 Board；
- Board 在 Intent 后更新时，Granted Turn 读取并看到新 Board，而不是 Intent 快照；
- Board 更新不会单独创建 Agent Turn；
- Board 读取失败不会启动模型或使用旧正文；
- Intent 最终失败表现为 PASS，Grant 最终失败表现为 YIELD；
- V2 Offer ACK、Progress、SAY/YIELD 和 Handoff 使用 V2 builders；
- Board Prompt 被标为不可信并执行确定性截断；
- restart 后恢复的 Turn 重新读取，不从 ledger 恢复 Board；
- Session 移除、终态、读取迟到和多会议并行不会串 Board；
- 本地 V2 主持身份不会启动阶段四能力；
- 既有 V0/V1 ACP tests 和后端 Meeting tests 无回归。

## 10. 完成定义

阶段三完成时，普通 Agent 能从当前 Board 形成 Intent；获得 Grant 后必定执行另一次
current-board read，再根据最新议程和结论 SAY 或 YIELD。Board 更新不会额外唤起 Agent，
读取失败不会把旧 Board 冒充当前内容，且 V2 主持 Agent 仍保持 fail closed，等待阶段四。

## 11. 实现收口（2026-08-02）

阶段三最终采用 V1/V2 共用 Baton coordinator、按 Session 固定协议分流的实现。Board read
在模型 dispatch 前形成独立内存 gate；读取中和读取成功待派发时使用短暂容量保护，但不能
占用更强的 Offer/Grant reservation。若实际 dispatch 未取得 Agent，已注入的 Board 会被
移除，下一次出队重新读取，避免排队期间复用旧快照。

自动化验收覆盖 Intent 看板 A 与 Granted Speech 看板 B 的独立读取、严格 Relay/标签校验、
UTF-8 安全截断、不可信上下文边界、最终 PASS/YIELD、V2 wire builder 选择、restart 后重新
读板、迟到 epoch 隔离、无 Board subscription、Board-only State 变化不产生 Turn、V0/V1
回归，以及本地 V2 主持身份的阶段三 fail-closed 边界。
