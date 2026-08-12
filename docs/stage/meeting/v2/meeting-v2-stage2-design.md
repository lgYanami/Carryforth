# Meeting V2 阶段二：权威控制周期与终态设计

> 状态：已冻结、已实现并通过后端验收
>
> 日期：2026-08-02
>
> 范围：后端 DB、Relay、SDK、`buzz-cli`；不包含 ACP 与前端。

## 1. 阶段目标

阶段二把阶段一只能创建和读取看板的 V2 Session，扩展成可以仅用 CLI 完成多轮讨论、
维护看板并正常或异常结束的完整后端生命周期：

```text
board_pending
    ├── update
    ├── unchanged
    ├── timeout
    └── Human Request preemption
             ↓
floor_ready
    ├── moderator decision / fallback
    ├── Offer → Grant → speech / Yield
    ├── Human priority
    ├── Directed Handoff chain
    ├── control returned → next board_pending
    ├── close → closed
    └── abort → aborted
```

V2 复用 V1 已验收的 Baton 对象和优先级，不复制第二套 Offer、Grant、Intent、Handoff
状态机。所有复用入口先按 Session 的持久协议 discriminator 分流，V1 与 V2 的 wire 和
State 事件保持严格隔离。

## 2. Wire 契约

### 2.1 版本与 policy

- V2 command、speech、End、State 与 Board projection 均使用 `v=3`；
- policy 固定为 `moderated-board-v1`；
- V1 继续使用 `v=2` / `moderated-baton-v1`，现有 fixture 不改变；
- Relay 先读取 Session 持久协议，再选择 parser；客户端不能用 tag 把命令导入另一代
  状态机。

### 2.2 Board Action

新增客户端命令 kind `42111`，只表示主持人的 Board Maintenance 结果；当前看板继续是
Relay-only kind `42110`。

共同 tags：

- `h`：Meeting Session UUID；
- `v=3`；
- `policy=moderated-board-v1`；
- `action=update|unchanged`；
- `expected-control-epoch`：当前 Control Token epoch；
- `board-window`：Relay State 暴露的当前内部 Board window fencing token。

`update` 的 content 是阶段一已经冻结的严格 Markdown board envelope；`unchanged` 的
content 必须为空。Board window token 只用于并发 fencing，不是看板业务 revision，调用者
不比较看板版本，也不从它重建看板历史。

Board Action command 本身不进入普通事件时间线或 Meeting outbox。接受 `update` 时 Relay
生成新的 kind `42110` 当前投影，并移除旧投影；接受 `unchanged` 时不生成 Board 事件。
两种结果都会生成不携带 Board 正文的权威 State transition。

### 2.3 Baton command 与 speech

V2 复用 V1 已有 command kinds 和字段词汇，唯一 wire 代际差异是 `v=3`。包括 Intent、
Human Request、Offer response、Grant signal、moderator command，以及 Grant-bound kind 9
speech。State kind 仍为 Relay-only `42103`，但 V2 State 使用 V2 discriminator。

### 2.4 Close 与 Abort

V2 继续使用 kind `42101`，严格 tags 为：

- `h`、`v=3`、`policy=moderated-board-v1`；
- `e` 指向 Create；
- `outcome=closed|aborted`；
- `reason-code` 只在 `aborted` 时出现；可选非空 content 提供简短说明。

`closed` 只允许主持人在合法 `floor_ready` 窗口、无 Offer/Grant 且本轮 Board outcome 为
显式 `updated` 或 `unchanged` 时提交。主持人可以主动 `aborted`；Community owner/admin 与
安全撤权路径可以从任意 active phase 强制 `aborted`。普通参会者不能 close 或主动 abort。

## 3. 持久状态

阶段一表 `meeting_v2_bootstrap_state` 保留名称以避免无价值的数据搬迁，但在阶段二扩展为
V2 程序 gate：

- `runtime_phase=bootstrap_locked|board_pending|floor_ready|ended`；
- `control_epoch`：必须与 Baton 当前 Control Token epoch 对齐；
- `board_window`：每次开放 Board Maintenance 单调增加；
- `board_started_at`、`board_deadline_at`；
- `board_outcome=updated|unchanged|timed_out|preempted`；
- `terminal_outcome=closed|aborted`；
- `terminal_reason_code` 与终态时间。

Floor、Offer 和 Grant 的权威 deadline 继续存放在 `meeting_baton_state`。Board deadline 只
存放在 V2 gate 中。当前待处理时间是二者的最早值，但二者永不同时 active。

V2 增加冻结配置 `meeting_v2_config.board_maintenance_ms`。默认值为 180 秒，合法范围与
Baton 最大 duration 相同。Floor Decision 仍使用 Baton 的 `moderator_decision_ms`。二者在
创建时分别冻结，所以 Board 结束的同一数据库事务才会按当时数据库时间创建完整 Floor
deadline。

`meeting_sessions.status` 继续只使用兼容的 `active|ended`。新增 nullable
`terminal_outcome`/`terminal_reason_code`：V0/V1 保持 NULL；V2 active 必须为 NULL，V2 ended
必须为 `closed|aborted`。

Board command 的幂等结果写入独立 receipt。V2 复用的 Baton command 继续使用既有 Baton
receipt 表；表的历史 V1 命名不构成协议边界。

## 4. 初始化与升级

新 V2 Create 在同一事务中提交：Create、私有名单、初始 Board、冻结 Baton/V2 配置、初始
Baton State、首个 `board_pending` window，以及 Create/State outbox。Board projection 不进
outbox。

阶段一已经存在的 `bootstrap_locked` Session 在第一次 V2 command、deadline claim 或显式
recovery 时，在 Session 行锁内只初始化一次：

1. 冻结默认配置；
2. 生成初始 Baton State；
3. 开放首个完整 Board deadline；
4. 将 State 进入 outbox；
5. 把 runtime 从 `bootstrap_locked` 原子推进到 `board_pending`。

多 worker 同时初始化时只有持有 Session 行锁的一个事务可以成功；后续 worker读取同一
权威结果。

## 5. 控制顺序

### 5.1 Board terminal → Floor Decision

`update`、`unchanged` 或 timeout 在同一事务中：

1. 终结当前 Board window；
2. 若 update，替换当前 Board projection；
3. 将 runtime 置为 `floor_ready`；
4. 根据当时可选 Intent/Handoff 决定 Baton 是 idle 还是 moderator control；
5. 若需 moderator control，从当前数据库时间创建完整
   `moderator_decision_ms` deadline；
6. 生成不含 Board 正文的 State/outbox。

timeout 保留原 Board，并记录 `timed_out`，不能冒充 `unchanged`。同一轮 timeout 后可以
继续讨论，但不能直接 normal close；关闭前必须完成一个新的显式 Board Maintenance。

### 5.2 Control return

speech、Yield、Offer/Grant expiry、Recall 完成等路径真正把 Control Token 还给主持人时，
V2 不启动 moderator deadline，而是：

- 清理 V1 已有活动对象；
- 按原规则增加 control epoch；
- 开放新的 Board window 和完整 Board deadline；
- Baton 保持 moderator idle，Floor deadline 为 NULL；
- 在同一次 State transition 中暴露新的 Board gate。

Directed Handoff 能直接产生下一 Offer 时不发生 Control return，因此不插入 Board gate。

### 5.3 Idle wake

主持人处于真正 idle 且新 Intent 使其需要重新决策时，先开放新 Board window，不能直接
启动 moderator fallback。Board pending 期间到达的普通 Intent 只更新 Intent pool。

### 5.4 Human preemption

合法的新 Human Request 在 Board pending 时，在同一 Session 行锁内先把 Board window 标记
为 `preempted`，再按 V1 原规则直接创建/排队 Human Offer。迟到的 Board Action 因
`control_epoch + board_window + runtime_phase` 不匹配而失败。

若 Board update 先提交，Human Request 在线性化后的新 Board 上继续；若 Human Request 先
提交，看板不被迟到结果覆盖。Human FIFO、Recall 和直接接力优先级不改变。

## 6. Recovery 与锁顺序

所有 V2 写入口采用同一顺序：

1. `meeting_sessions` 行锁；
2. V2 runtime/Baton current rows；
3. 冻结 roster 与安全身份锁；
4. 当前 Offer/Grant/Intent/Handoff 对象；
5. Board projection；
6. receipt、State history 和 outbox。

任意写命令先按数据库时钟执行 lazy recovery。Sweeper 用
`FOR UPDATE SKIP LOCKED` 领取 Board 或 Baton 最早 deadline，随后重新持有 Session 锁复查。
Board timeout、Floor start 和 State 发布原子提交。Relay 重启只会继续当前持久 window，
不会重开 Board、缩短 Floor budget或重复替换看板。

End/安全撤权在 Session 锁下优先终态化所有 active Intent、Request、Offer、Grant、Handoff、
moderator attempt 和 Board window。任何较晚结果只能得到 terminal/conflict receipt，不能
复活会议。

## 7. CLI 面

阶段二提供：

- `meetings board get`；
- `meetings board update --board <file|-> --control-epoch N --board-window N`；
- `meetings board unchanged --control-epoch N --board-window N`；
- V2 `meetings floor status/history`；
- V2 Intent、Human Request、Offer/Grant、speech、Yield、Handoff 与 Recall；
- `meetings close` 和 `meetings abort`。

CLI 每次操作先读取最新 State；可由命令自动推导的 epoch/window 不要求用户手工保存。
显式参数仍保留用于竞态测试和脚本 CAS。Stage 2 不启动或模拟 ACP Turn。

## 8. 验收矩阵

必须以确定性测试证明：

- Create 后先 Board、后 Floor，两个 deadline 都获得完整预算；
- update、unchanged、timeout 三种 Board terminal 可区分；
- Board command 重放幂等，旧 window 不能覆盖当前 Board；
- Intent 不抢占 Board；Human Request 会抢占且迟到 Board 失败；
- Directed Handoff 不插入 Board，Control return 才插入；
- Offer/Grant/speech/Yield/Recall/fallback 与 V1 一致；
- normal close 不能绕过最终显式 Board Maintenance；
- closed 与 aborted 可查询区分，终态后 Board/历史只读；
- sweeper、lazy recovery、重启和并发 worker 收敛到单一 State；
- Board 更新正文不进入 outbox/State/speech timeline；初始正文仍按阶段一 wire 包含在 Create；
- V0/V1 fixture、路由和行为无回归。
