# Meeting V2 社区 Agent 能力声明生命周期缺口修复设计

> 状态：核心修复已实现并通过本地数据回填验收
>
> 记录日期：2026-08-05
>
> 范围：Meeting V2 direct actions、Agent Profile kind `10100`、`buzz-acp`、
> Desktop managed Agent 生命周期、Agent discovery 与 Meeting 创建体验
>
> **后续代际说明（2026-08-08）：**本文记录的 capability 宣告/reconcile 生命周期机制继续有效，
> 但其中 `meeting-v2-action-finalization-v2` / actions-v2 数字是事故当时事实。current create gate
> 只接受 `meeting-v2-action-finalization-v4` + `moderated-board-actions-v3`；profile reconcile
> 必须移除旧 v2/v3，而不是同时保留多个 active 代际。现行执行语义见
> [逻辑主持人 ACK 与同步简化实现设计](../meeting/fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)。

## 1. 结论

本次 Meeting 创建失败不是 Community 权限、Role、Assignment、Runtime supervisor、在线状态或
数据库迁移问题，而是 Agent 能力声明的生命周期没有接通：

- Relay 对 `moderated-board-actions-v2` roster 执行了正确的 fail-closed capability gate；
- 当前 `buzz-acp` 已实现并声明 `meeting-v2-action-finalization-v2`；
- 但 Desktop 创建、启动、恢复和升级 managed Agent 时，没有把这一能力以 Agent 自身签名的
  kind `10100` Profile 写入 Relay；
- 因而 Relay 的 canonical `users.capabilities` 对 `test-1`、`test-2`、`test-3` 均为 `NULL`，
  Meeting Create 在事务内被拒绝并完整回滚。

本修复不能只给这三个 Agent 做数据库补值。必须建立覆盖整个 Community Agent fleet 的持续
能力声明机制：

1. 实际运行的 ACP harness 是能力事实源；
2. Agent 自身签名的 kind `10100` 是跨客户端 canonical 声明；
3. Desktop 对自己管理的全部存量和未来 Agent 执行幂等对账；
4. Agent 创建、启动、恢复、harness 变更和 Desktop 升级均触发对账；
5. Meeting Desktop 在提交前显示每个 Agent 的兼容性并阻止不兼容 roster；
6. Relay 的事务内最终校验继续保留，不能因客户端预检而放宽；
7. 外部或其他设备管理的 Agent 必须由其真实 harness 自行声明，Desktop 不能替它伪造能力。

修复后的核心不变量是：

```text
Agent advertises meeting-v2-action-finalization-v2
    iff its effective ACP harness positively declares that capability

Meeting direct-actions-v2 roster is creatable
    iff every Agent participant has that canonical declaration
```

所有使用当前第一方 `buzz-acp` 的 Buzz managed Agent 都应自动满足该不变量，包括修复前创建的
存量 Agent、修复后新建的 Agent 以及后续重新安装或升级 harness 的 Agent。真正不支持该能力的
Agent 不应被伪装成支持，而应在 Meeting 选择阶段明确标为不兼容。

## 2. 故障记录

### 2.1 用户可见现象

Human 请求 `test-1` 创建一场包含 `test-1`、`test-2`、`test-3` 的模拟会议，Agent 收到：

```text
restricted: every Agent in the Meeting roster must advertise
meeting-v2-action-finalization-v2
```

结果符合事务语义：

- 没有创建普通 Channel；
- 没有生成 Meeting UUID；
- 没有写入部分 roster、Board 或 Meeting State；
- Create command 所在事务回滚。

### 2.2 已确认的 canonical 状态

故障发生时，Local Dev Community 中以下 managed Agent 的 `users.capabilities` 均为 `NULL`：

- `test-1`；
- `test-2`；
- `test-3`；
- 同一 Community 中其他已检查的 managed Agent 也没有能力列表。

该 Community 当时没有可用于这些 Agent 的 kind `10100` capability 声明。因此错误不是某一个
Agent 的偶发状态，而是当前 managed Agent 创建和运行链路的系统性缺口。

### 2.3 Relay 拒绝位置

`../../../crates/buzz-relay/src/handlers/command_executor.rs` 在新建
`ModeratedBoardActionsV2` Meeting 前调用：

```text
action_roster_supports_capability_tx(
    community,
    participant_pubkeys,
    meeting-v2-action-finalization-v2,
)
```

`../../../crates/buzz-db/src/meeting_v2.rs` 的查询只检查 roster 中具有
`agent_owner_pubkey IS NOT NULL` 的 Agent；Human participant 被有意忽略。任意 Agent 的
`users.capabilities` 不是数组或不包含目标 capability，整个 roster 即失败。

这说明该 gate 判断的是“Agent runtime 是否理解 direct action finalization”，而不是：

- Agent 是否在线；
- Agent 是否是 Community member；
- Agent 是否拥有 Role 或 Assignment；
- Agent 是否有 supervisor binding；
- Agent 是否有普通消息或 Project View 写权限。

## 3. 根因

### 3.1 能力实现与能力发布是两条未接通的路径

`crates/buzz-acp/src/lib.rs::meeting_capabilities()` 已返回：

```json
{
  "meeting": {
    "capabilities": ["meeting-v2-action-finalization-v2"]
  }
}
```

`buzz-acp capabilities --json` 也能在不启动 provider 的情况下探测该静态能力。但是该结果目前
只用于本地探测和诊断，没有进入 Agent Profile 发布流程。

与此同时，`desktop/src-tauri/src/relay.rs::sync_managed_agent_profile()` 只发布 kind `0` 的
名称与头像。它没有构建或发布 kind `10100`，因此以下路径都不会补齐 capability：

- 新建 managed Agent；
- 手动启动或重启 Agent；
- Desktop 启动时恢复 auto-start Agent；
- persona/team 导入；
- harness 安装、升级或切换；
- provider Agent 部署成功；
- Desktop 版本升级后首次打开已有 Community。

所以“重启 Agent”在当前代码中不会自动修复问题。重启只能证明进程重新启动，不能让 Relay
获得此前从未发布的 canonical 声明。

### 3.2 kind `10100` 的两个控制维度没有统一合成

kind `10100` 当前同时承载：

- `channel_add_policy`；
- 可选的 `capabilities`。

Relay side effect 要求 `channel_add_policy` 必须存在；若 `capabilities` 省略，则数据库保留旧
能力列表，兼容旧 writer。当前 `buzz channels set-add-policy` 只发布 policy，不携带完整
capabilities；Desktop discovery 又直接读取最新 kind `10100` 事件。

这会形成潜在分裂：

```text
users.capabilities        = 保留的旧值
latest kind:10100 content = 没有 capabilities
Desktop discovery         = []
```

因此新增 capability publisher 时不能再制造一个彼此覆盖的独立 writer。所有第一方 kind
`10100` writer 必须共享“读取当前完整状态、合并一个维度、发布完整快照、canonical 回读”的
逻辑。

### 3.3 Meeting Desktop 没有在提交前解释 roster 兼容性

Relay 的最终事务 gate 是必要的，但 Desktop 不能把它当作唯一反馈。当前用户可以选择看似
在线的 Agent，直到 Create 提交后才看到一条无法指出具体对象的 400 错误。

在线 presence 不能代替 capability。Agent 名称、在线状态和 Meeting 协议兼容性必须作为三个
独立字段展示和判断。

### 3.4 没有 fleet reconciliation

单次创建时发布不足以覆盖真实生命周期：

- 修复前已经存在的 Agent 不会重新经过 create；
- App 升级可能带来新的 `buzz-acp` capability；
- 用户可以切换或降级 ACP harness；
- Agent 可以来自 team/persona import 或 provider；
- 首次发布可能因 Relay 离线、限流或短暂网络故障失败。

如果没有启动对账、变更对账和可重试状态，未来还会再次出现“代码支持但 Profile 为空”的
同类问题。

## 4. 权威语义与边界

### 4.1 Capability 是兼容性声明，不是权限

`meeting-v2-action-finalization-v2` 只声明当前 Agent harness 能够理解并执行 Meeting V2 的
direct action finalization Turn 与结束清理语义。它不授予：

- Community membership；
- Channel、Project View、Document 或 Role 写权限；
- moderator 身份；
- Assignment；
- Runtime supervisor authority；
- 额外工具或外部系统权限。

实际操作仍由原有 Community ACL、业务对象权限与 Relay 状态机分别校验。

### 4.2 声明主体

能力必须最终由 Agent 自己的 Nostr key 签名。允许执行签名的第一方组件只有：

- 正在运行的 trusted `buzz-acp` harness；
- 持有该 managed Agent key 的 Desktop trusted backend，用于创建和 fleet reconciliation。

LLM provider、Codex/Claude 子进程和普通 Meeting 内容都不能控制 capability 声明。Relay、Human
key 或其他 Agent key 也不能替目标 Agent 声明。

### 4.3 Agent 范围

| Agent 来源 | 能力事实源 | 对账责任 | Meeting UX |
|---|---|---|---|
| Desktop 本地 managed Agent | 实际解析到的 ACP harness probe | Desktop + harness | 支持时自动可选 |
| Desktop provider managed Agent | provider 中实际启动的 harness | provider/harness；Desktop 观察结果 | 声明成功后可选 |
| 其他 Desktop/设备管理的 Agent | 该设备上的真实 harness | 对方 Desktop/harness | 本机只读取，不代签 |
| 外部/自定义 Agent | 自己的兼容实现 | 外部 Agent operator | 未声明时明确不兼容 |
| Human | 不适用 | 不需要 | capability gate 忽略 |

“覆盖 Community 所有 Agent”不等于给所有身份无条件写入 capability。正确含义是：

- 所有第一方 managed Agent 都进入自动发现、发布和重试生命周期；
- 所有 Community Agent 都在 Meeting roster UI 中得到明确兼容状态；
- 不支持或无法验证的 Agent 不会造成提交后才发现的模糊失败。

### 4.4 Relay 仍是最终裁决者

Desktop 预检只改善体验，不能成为安全边界。Create 提交到 Relay 后必须在同一事务内重新读取
canonical `users.capabilities` 并校验完整 frozen roster，以处理预检后的撤回、降级或并发更新。

## 5. 修复方案

### 5.1 定义类型化的 Agent runtime capability snapshot

在共享层定义稳定类型，不让 Desktop、ACP 和 CLI 各自拼接字符串：

```text
AgentRuntimeCapabilities {
    component,
    component_version,
    capabilities: sorted unique strings,
}
```

要求：

1. `buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY` 继续是唯一常量源；
2. capability 数量、长度、去重和控制字符限制与 DB validation 一致；
3. 输出排序稳定，避免仅因顺序差异反复发布；
4. probe 失败、超时、JSON 非法或未知 harness 一律为 `Unknown`，不能推断为支持；
5. 明确返回 `Supported(set)`、`Unsupported` 与 `Unknown(error)`，不能用空数组同时表达探测失败
   和确定不支持。

Desktop 必须对 spawn 时实际解析的 `acp_command` 执行 `capabilities --json`，不能探测 PATH 中
另一个同名二进制，也不能仅根据配置中的 runtime 名称猜测。

### 5.2 建立 kind `10100` 完整状态 publisher

新增共享的 Agent control profile publisher，替代各路径直接构造 kind `10100`：

```text
read latest agent-authored kind:10100
    -> validate/preserve channel_add_policy
    -> replace capabilities with exact probed set
    -> build full closed snapshot
    -> sign with the same Agent key
    -> submit using exact configured Relay authority
    -> query canonical latest event
    -> verify event id and normalized content
```

完整快照至少包含：

```json
{
  "channel_add_policy": "anyone",
  "capabilities": ["meeting-v2-action-finalization-v2"]
}
```

具体规则：

1. 已有合法 `channel_add_policy` 必须原样保留；
2. 从未发布 kind `10100` 时使用数据库 schema 同义默认值 `anyone`；
3. capabilities 是全量替换，不是只追加，以便 harness 降级后撤销陈旧声明；
4. event `created_at` 必须严格大于同一 author/kind 的当前 replaceable event，避免同秒 LWW
   竞争；
5. 提交成功不等于对账成功，必须 canonical 回读；
6. 如果回读发现并发 writer 获胜，重新读取、合并并进行有界重试；
7. 相同完整状态为 no-op，不重复发布；
8. 使用实际连接地址，例如 `ws://localhost:3000`。`127.0.0.1` 规范化值只可用于本地进程键和
   去重，不能改变 Community authority；
9. 不通过 SQL 直接填充 `users.capabilities`，因为那不是 Agent 签名的 canonical 来源，也无法
   覆盖后续 Community、设备或 Relay。

同时修改 `buzz channels set-add-policy`：它也必须通过同一个 read/merge/publish 语义保留完整
capabilities，避免 policy 更新把最新事件投影变成“能力为空”。

### 5.3 `buzz-acp` 启动时自声明

`buzz-acp` 在完成 Relay 连接和 Agent 身份校验后执行一次非阻塞但可观测的 capability
reconciliation：

1. 使用当前二进制的 `meeting_capabilities()` 生成 snapshot；
2. 使用当前 `BUZZ_PRIVATE_KEY` 以 Agent 身份签名；
3. 发布到当前 `BUZZ_RELAY_URL` 对应的 Community；
4. canonical 回读确认；
5. 相同状态不重复写；
6. 网络失败使用有上限的指数退避，并在重连后再次尝试。

能力发布失败不能让普通聊天进程退出，但必须：

- 写入结构化日志；
- 暴露为 runtime health 的独立 degraded 状态；
- 让 Meeting roster 保持 `capability pending/unavailable`，而不是假装可用。

这条路径覆盖由其他 Desktop、provider 或外部部署启动的第一方 `buzz-acp`，并确保声明来自
真正执行 Meeting Turn 的 harness，而不是 UI 的静态猜测。

### 5.4 Desktop managed Agent fleet reconciler

Desktop trusted backend 增加 Community-scoped、幂等、限流的 fleet reconciler。它遍历当前
Community 中由本 Desktop 管理且私钥可用的所有 Agent，不依赖 Agent 名称、是否在当前 Channel
或是否已被选择参加 Meeting。

触发矩阵：

| 触发点 | 对账对象 | 目的 |
|---|---|---|
| Desktop 升级后首次启动/Community init | 当前 Community 全部 managed Agent | 自动回填存量 |
| 新建 Agent，membership 建立后 | 新 Agent | 保证未来新建路径 |
| 手动 start/restart 成功 | 该 Agent | 对齐实际 harness |
| auto-start restore 成功 | 所有恢复成功的 Agent | 覆盖应用重启 |
| ACP harness 安装/升级/切换 | 所有使用该 harness 的 Agent | 发布新增或撤回能力 |
| persona/team import | 导入产生的全部 Agent | 覆盖批量创建 |
| provider deploy/upgrade 成功 | provider 确认的 Agent | 等待真实 runtime 声明 |
| Relay reconnect | 未完成或失败的 Agent | 收敛暂时网络失败 |
| 用户点击 Retry/Doctor | 指定 Agent 或全 fleet | 可操作恢复 |

对账过程必须：

1. 使用现有 managed Agent store lock 只读取必要 snapshot，不跨 `.await` 持锁；
2. 按 exact Community authority 分组并限制并发，避免启动时形成请求风暴；
3. 查询 canonical kind `10100`，只发布差异；
4. 记录每个 Agent 的 `synced / pending / unsupported / failed`，但不把本地状态当作 Relay
   authority；
5. 私钥缺失、Agent 已删除、owner 已失权或 Community 已切换时停止该任务；
6. Community 切换时取消旧 Community 的后台任务，不能把 capability 写到新 Community；
7. 同一 Agent 多个 ACP slot 只对账一次。capability 属于逻辑 Agent/harness，不属于 Channel
   ACP Session 或工作槽。

对于本地 managed Agent，即使它当前离线，Desktop 也可以探测其下一次 spawn 会使用的 exact
harness 并完成存量回填。对于 provider Agent，Desktop 不能用本地二进制替远端 runtime 作证，
必须等待 provider 或远端 `buzz-acp` 的实际声明。

### 5.5 存量 Agent 自动回填

修复版本首次运行时，不编写仅针对 `test-1/2/3` 的脚本，也不按名称筛选。流程为：

```text
load every managed Agent record for current Community
    -> resolve exact effective relay and ACP harness
    -> probe capability
    -> read current kind:10100
    -> publish full snapshot when missing/stale
    -> canonical read-back
    -> surface per-Agent result
```

成功标准是当前 Community 中所有由本 Desktop 管理、使用支持该能力的第一方 harness 的 Agent
均完成声明。`test-1/2/3` 只是该集合中的普通成员。

不执行数据库清空、Project View/Meeting 重初始化、身份重建或 Agent 重建；现有消息、Agent、
Project View、Document、Resource 和 Meeting 数据均不受影响。

### 5.6 Meeting Desktop 提交前校验

Meeting 创建表单为每个 Agent 展示独立状态：

```text
Compatible
Capability sync pending
Unsupported runtime
Capability unknown
Offline
```

其中 `Offline` 与 capability 状态并列，不能相互替代。

对 direct-actions-v2 roster：

1. Human 始终不参与 capability gate；
2. Compatible Agent 可以选择；
3. pending/unknown/unsupported Agent 默认不可选择，或在已选择后禁止提交；
4. 错误必须列出 Agent 显示名和缺少的 capability；
5. 提供刷新与 Retry capability sync；
6. kind `10100` live event 到达后实时刷新，不要求重启 Desktop 或重开弹窗；
7. Create 按钮提交前再次读取 roster capability snapshot；
8. Relay 仍在事务中作最终校验，防止 TOCTOU。

若 CLI 或旧客户端绕过 Desktop 预检，Relay 继续拒绝不合格 roster。这是预期的协议保护，不是
需要绕过的错误。

### 5.7 Relay 错误与诊断

将当前自然语言错误稳定化为机器可分类结果，例如：

```text
restricted:meeting:roster_capability_missing
```

命令响应可以在不泄露 roster 外身份的前提下返回：

```json
{
  "required_capability": "meeting-v2-action-finalization-v2",
  "missing_agent_pubkeys": ["..."],
  "missing_count": 2
}
```

请求者已经提供完整 roster，因此返回 roster 内失败 pubkey 不扩大可见范围。Desktop 使用自己
已有的 member/profile 映射显示名称。Relay 日志同时记录 Community、Meeting policy、缺失数量
和 pubkey，不能误报为 authorization、presence 或 Runtime supervisor 问题。

## 6. 并发、撤回与失败语义

### 6.1 多 writer

同一 Agent 可能同时存在：

- Desktop fleet reconciler；
- 正在启动的 `buzz-acp`；
- `buzz channels set-add-policy`；
- 另一个管理该 Agent 的合法设备。

所有第一方 writer 都发布完整 snapshot，并使用“读取—合并—发布—回读—有界重试”。policy
writer 只改变 policy，capability writer 只改变 capabilities；双方都保留另一维度的 canonical
值。最终事件必须同时完整表达二者。

### 6.2 降级和撤回

如果 exact harness probe 确认不再支持某 capability，reconciler 必须发布不包含它的完整列表。
不能只追加，也不能让旧声明永久保留。

如果 probe 是 `Unknown`，不得自动清空最近一次已验证声明，也不得新增声明；状态标为 pending，
等待下一次成功探测。只有明确的 `Unsupported` 才执行撤回，以避免短暂 PATH 或网络问题造成
能力抖动。

Meeting Relay 最终 gate 读取撤回后的 canonical DB projection，新的 roster Create 立即失败。
已创建 Meeting 的 frozen roster 和既有 lifecycle 按 Meeting 协议处理，本修复不在中途静默
替换参与者。

### 6.3 发布失败

能力同步失败时：

- 不删除 Agent；
- 不停止正常聊天；
- 不写数据库旁路值；
- 不把本地 probe 结果冒充 Relay canonical 状态；
- Meeting UI 明确显示 sync pending/failed；
- Relay reconnect、下次启动和用户 Retry 都会再次对账。

这使网络暂时失败成为可恢复的 degraded 状态，而不是下一次建会时才暴露的隐性错误。

## 7. 预计代码改动

### 7.1 `buzz-sdk`

涉及：

- `../../../crates/buzz-sdk/src/builders.rs`；
- kind `10100` 相关 builder/tests。

改动：

- 提供类型化的完整 Agent control profile builder；
- 复用 `MEETING_V2_ACTIONS_CAPABILITY`；
- 固定排序、去重、字段验证和完整快照语义；
- 不新增 Nostr event kind。

### 7.2 `buzz-acp`

涉及：

- `../../../crates/buzz-acp/src/lib.rs`；
- Relay publisher/lifecycle 模块。

改动：

- 复用现有 `meeting_capabilities()`；
- 在 authenticated Relay lifecycle 中执行 self-advertisement；
- reconnect 时幂等重试；
- 暴露 capability sync health 与结构化日志；
- 不把 provider/model capability 与 Buzz Meeting protocol capability 混为一谈。

### 7.3 Desktop Tauri

涉及：

- `../../../desktop/src-tauri/src/relay.rs`；
- `../../../desktop/src-tauri/src/commands/agents.rs`；
- `../../../desktop/src-tauri/src/managed_agents/restore.rs`；
- start/restart、runtime install、team/persona import 与 provider deploy 路径；
- Community init/reset 生命周期。

改动：

- 将现有 kind `0` metadata sync 与 kind `10100` controls sync 明确拆分；
- 新增 exact harness probe 与 fleet reconciler；
- 为所有创建/启动/恢复/升级入口接入同一个对账函数；
- 后台任务按 Community 隔离并可取消；
- 增加可读的 per-Agent sync 状态和 Retry/Doctor 入口。

### 7.4 Agent CLI

涉及：

- `crates/buzz-cli/src/commands/channels.rs`；
- 共享 Agent profile read/merge/publish helper。

改动：

- `set-add-policy` 保留并回写完整 capability 列表；
- canonical read-back；
- 旧 event 缺字段时按兼容规则归一化。

### 7.5 Meeting Relay/DB

涉及：

- `../../../crates/buzz-relay/src/handlers/command_executor.rs`；
- `../../../crates/buzz-db/src/meeting_v2.rs`。

改动：

- 保留现有事务内 `all Agents` gate；
- 返回稳定、可分类、带 roster 内缺失 pubkey 的错误；
- 增加 capability rejection metrics；
- 不自动补值、不根据 agent type 或在线状态猜测能力。

### 7.6 Meeting Desktop

涉及：

- Meeting create roster picker；
- kind `10100` live subscription/query cache；
- Community reset wiring。

改动：

- 展示并实时刷新兼容状态；
- 缺少 capability 时在提交前阻止；
- Relay 最终拒绝时映射到具体 Agent 名称；
- 新增的 Community-scoped cache 必须接入 `resetCommunityState()`，防止跨 Community 泄漏。

## 8. 数据与协议兼容

本修复：

- 不删除或重建数据库；
- 不修改 Meeting、Project View、Document 或 Resource 数据；
- 不新增 event kind；
- 不修改 Meeting V2 frozen roster 语义；
- 不降低 direct-actions-v2 capability gate；
- 不要求为 Agent 创建 Role、Assignment 或 supervisor binding；
- 不把 capability 写入 presence；
- 不按 `test-*` 名称做特殊处理。

现有 kind `10100` 缺少 `capabilities` 时继续可读，并归一化为“未声明/待对账”。首次成功对账
产生新的 Agent 签名 replaceable event，Relay side effect 自然更新已有 `users.capabilities`。
因此不需要 SQL migration 或一次性数据库脚本。

## 9. 测试方案

### 9.1 共享 builder 与合并逻辑

1. 完整 snapshot 同时携带 policy 与 capabilities；
2. capability 排序、去重和长度验证稳定；
3. 更新 capability 保留 policy；
4. 更新 policy 保留 capabilities；
5. 首次 Profile 使用 `anyone`；
6. 同状态为 no-op；
7. replaceable timestamp 单调递增；
8. 并发覆盖后 canonical read-back 能检测并重试；
9. `Unknown` 不新增也不撤回，明确 `Unsupported` 才撤回。

### 9.2 Desktop fleet 生命周期

至少覆盖：

1. 修复前创建且 `capabilities=NULL` 的多个 Agent 在首次启动时全部回填；
2. 测试不依赖 `test-1/2/3` 名称或固定 pubkey；
3. 新建 Agent 在 membership 建立后自动发布；
4. 手动 start/restart、auto-start restore 都会对账；
5. team/persona 批量导入的每个 Agent 都会对账；
6. harness 安装或升级后，所有受影响 Agent 都会对账；
7. harness 明确降级后能力被撤回；
8. Relay 暂时离线后重连自动收敛；
9. Community A 的后台任务不会写入 Community B；
10. `localhost` 不会被错误替换为 `127.0.0.1` 作为连接 authority；
11. 多槽 Agent 只发布一次，不按 Channel/ACP Session 重复发布；
12. 私钥不可用的 Agent 明确失败且不伪造签名。

### 9.3 Relay/DB

1. roster 只有 Human 时不要求 capability；
2. roster 所有 Agent 都声明时 Create 成功；
3. 任意一个 Agent 缺失时完整事务回滚；
4. `NULL`、非数组、空数组、错误 capability 均 fail closed；
5. 返回结果准确列出缺失 roster Agent；
6. kind `10100` publish 后 `users.capabilities` 与最新完整事件一致；
7. capability 撤回后新的 Create 被拒绝；
8. 旧 Meeting、消息、Project View 数据不变。

### 9.4 Desktop E2E

1. 三个 compatible Agent 均可选择并成功创建 Meeting；
2. 新建第四个 managed Agent 无需重启 Desktop 即实时变为 compatible；
3. incompatible/unknown Agent 显示名称和原因，Create 按钮不可提交；
4. capability live event 到达后 picker 原地更新；
5. presence offline 与 capability missing 显示为不同状态；
6. Relay 在预检后撤回 capability 时，最终错误仍正确映射；
7. 切换 Community 后不显示前一 Community 的 capability 状态。

### 9.5 真实本地验收

使用当前数据、不清库：

1. 启动修复版本；
2. 等待 fleet reconciliation；
3. 回读 Local Dev 全部 managed Agent 的 kind `10100` 和 DB projection；
4. 确认所有使用当前 `buzz-acp` 的 Agent 都包含目标 capability；
5. 新建一个 Agent，确认不需要手工补值；
6. 使用任意三个 compatible Agent 创建 Meeting；
7. 确认 Meeting、私有 room、初始 Board 和 roster 一次成功；
8. 重启 Desktop/Relay 后重复创建，确认声明仍有效且没有重复写风暴；
9. 确认历史消息、Project View、Documents、Resources 和已有 Agent 均保留。

## 10. 可观测性

建议增加：

```text
buzz_agent_capability_reconcile_total{trigger,result}
buzz_agent_capability_reconcile_pending
buzz_meeting_create_capability_rejected_total{capability}
```

结构化日志至少包含：

- Community authority/id；
- Agent pubkey 与本地显示名；
- exact harness path/version；
- trigger；
- desired capability hash；
- published event id；
- canonical read-back 结果；
- retry 次数和稳定错误类别。

日志不得输出 Agent private key、NIP-OA 私密材料、provider token 或其他 secret。

Desktop Doctor 增加 `Meeting protocol capability` 检查，并提供 fleet 级别汇总：

```text
Compatible 5 / Pending 0 / Unsupported 0 / Failed 0
```

## 11. 交付顺序

1. 实现类型化 snapshot 与完整 kind `10100` merge publisher；
2. 更新 `set-add-policy`，消除第一方 writer 字段覆盖；
3. 接入 `buzz-acp` 启动/reconnect self-advertisement；
4. 接入 Desktop 新建、启动、恢复、升级、导入和 provider 生命周期；
5. 加入启动时全 fleet backfill 和失败重试；
6. 完成 Meeting roster picker capability UX 与 live refresh；
7. 稳定 Relay 错误分类并补齐 metrics；
8. 运行 unit、DB integration、Desktop/Tauri 与 E2E；
9. 在保留当前本地数据的前提下重建并启动，执行真实验收。

Relay 的严格 gate 在整个交付中保持启用。不能通过临时删除 gate、把 `NULL` 当作支持或为全部
Agent 直接写数据库来获得表面通过。

## 12. 完成定义

本问题只有同时满足以下条件才算修复完成：

- 当前 Community 中所有受本 Desktop 管理且实际支持该协议的存量 Agent 自动完成声明；
- 后续新建 Agent 自动完成声明，无需用户执行 CLI 或 SQL；
- Agent 启动、恢复、升级、切换和降级均会使声明最终收敛；
- 其他设备、provider 和外部 Agent 有明确的自声明边界；
- Meeting Desktop 在提交前能指出不兼容 Agent；
- Relay 仍对完整 roster 做最终事务校验；
- channel policy 更新不会擦除 capability 的事件投影；
- 同步失败可见、可重试，不会在下一次建会时才首次暴露；
- 修复和回填不删除任何现有 Community 数据。

## 13. 实现交付记录

实现日期：2026-08-05。

本次已落地以下闭环：

1. `buzz-sdk` 提供完整 kind `10100` controls builder，对 policy、capability 数量、长度、
   控制字符、唯一性和稳定排序进行统一校验；
2. `buzz-acp` 在启动和 Relay reconnect 后，以当前 Agent 身份执行幂等自声明；失败采用有界退避，
   不阻断普通聊天；
3. Desktop 对本地 Agent 探测其实际配置且实际解析到的 `acp_command`，只有有效 probe 才新增或
   撤回 Meeting capability；probe unknown 时保留 canonical 状态，不进行猜测；
4. provider/远端 Agent 不由本机 Desktop 代为声明，而由真正运行的 `buzz-acp` 自声明；
5. Desktop 启动时遍历当前 managed Agent fleet，且新建、手动启动、自动恢复、单 Agent 快照导入、
   Team 快照导入均接入同一对账路径；该逻辑不匹配名称或固定公钥；
6. 对账使用创建任务时捕获的原始 Community authority，`localhost` 不会被替换成
   `127.0.0.1` 作为实际连接地址；
7. kind `10100` capability writer 保留 channel policy 和未知 capability，支持并发覆盖后的
   canonical 回读与有界重试；`buzz channels set-add-policy` 也会保留完整 capability 列表；
8. Desktop Meeting Create 在提交前重新读取所有非-compatible Agent；Relay 最终拒绝时会把
   缺失公钥映射回 Agent 名称；
9. Relay 保留事务内 fail-closed gate，并返回稳定错误码、缺失 roster Agent 公钥、结构化日志和
   rejection metric；
10. 未增加 SQL migration、数据库旁路补值或 test-1/test-2/test-3 特例，也未删除或重建任何
    Community 数据。

本地验收结果：

- Rust compile/check 与严格 Clippy 通过；
- SDK、ACP、Desktop capability probe/reconcile 单元测试通过；
- Desktop 3553 项前端测试、TypeScript typecheck 与相关 Biome 检查通过；
- Postgres ignored integration test 验证了“检查所有 Agent、忽略 Human、准确返回缺失 Agent”；
- 在保留 Local Dev 数据的情况下，Desktop fleet reconciliation 为 `test-1` 至 `test-5` 自动发布
  Agent 自签名 kind `10100`，canonical `users.capabilities` 回读成功；Community 内当时可见的
  11 个 Agent 均声明了 `meeting-v2-action-finalization-v2`。

Desktop Doctor 的 fleet 汇总面板和更细的 per-Agent pending/failed 持久状态属于后续可观测性
增强，不是本次防止 Meeting Create 系统性失败所需的协议闭环。
