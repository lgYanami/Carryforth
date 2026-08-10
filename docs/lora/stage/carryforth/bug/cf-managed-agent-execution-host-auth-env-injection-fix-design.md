# `cf` Managed Agent 执行宿主身份环境注入缺口修复设计

> 状态：代码实现与自动化回归完成；重新构建后的真实 managed Agent / Meeting 验收待执行
>
> 日期：2026-08-10
>
> 范围：Carryforth `cf` CLI、Desktop managed Agent 启动、`buzz-acp` Agent Pool、
> Codex/Claude ACP 子进程、Code Mode 执行宿主与 `buzz-dev-mcp`
>
> 关联：
> [`cf` CLI 去 Buzz 化实现计划](../cli-cf-cutover-implementation-plan.md)、
> [Meeting Action Finalization 提示词认知边界优化设计](../../meeting/fix/meeting-action-finalization-prompt-boundary-optimization-design.md)

## 1. 结论

“可信召回异常态呈现与安全恢复”验收 Meeting 没有因为 Action lease、Meeting Coordinator、
Candidate-Cohort adoption 或提示词边界再次失败。Action Finalization 已正常派发，lease 也正常续约；
最终 `BLOCK(external_operation_failed)` 是主持 Agent 在真实业务命令全部认证失败后，按照现行提示词做出的
正确 fail-closed 结果。

直接故障是 `cf` 切换只完成了 **MCP 子进程**的 `CARRYFORTH_*` 凭据映射，没有完成
**模型 Agent / Code Mode 执行宿主**的凭据映射：

```text
Desktop
  └─ buzz-acp                         BUZZ_*：有；CARRYFORTH_*：无
       ├─ codex-acp / codex           BUZZ_*：有；CARRYFORTH_*：无
       │    └─ codex-code-mode-host   BUZZ_*：有；CARRYFORTH_*：无
       └─ buzz-dev-mcp                BUZZ_*：无；CARRYFORTH_*：有
```

本次 Action Turn 使用的是 Codex 内置 `functions.exec` / Code Mode shell，而不是
`buzz-dev-mcp` shell。`cf --help` 不需要身份，所以帮助命令成功；真正读取 Project View、Document、
Project Context、Meeting 等 canonical 状态时，`cf` 只读取新的公开合同 `CARRYFORTH_*`，因而全部以
exit code 3 失败：

```text
auth_error: CARRYFORTH_PRIVATE_KEY is required
```

这属于 **Carryforth CLI cutover 的执行宿主环境注入缺口**。它不是 Meeting 提示词缺陷，也不是主持
Agent 的另一个槽越权调用 `actions begin`。修复应覆盖 managed Agent 的所有真实执行面，不得恢复
`cf` 对 `BUZZ_*` 的 fallback，也不得修改 Meeting 状态机来掩盖认证失败。

## 2. 用户可见影响

- `cf --help`、子命令帮助和参数校验可成功，造成 CLI 已经可用的假象；
- managed Agent 通过 Code Mode、内置 shell 或其他模型原生工具执行任何需签名的 `cf` 命令时失败；
- Channel / DM 中的 Agent 只要走同一内置执行宿主，也可能受影响，不限于 Meeting；
- `buzz-dev-mcp` 路径因为已经收到 `CARRYFORTH_*`，可能正常，从而形成同一个逻辑 Agent 内“某些工具
  可用、某些工具认证失败”的不一致；
- Meeting Action Finalization 会在实际物化前返回 `BLOCK(external_operation_failed)`，Meeting 按设计
  保持 `active / finalizing_actions`，不会伪造 `completed_closed`；
- 本次失败没有产生 Project View、Document 或 Project Context 半写入，也没有造成数据删除。

如果 Human 在终端中自行加载了 `CARRYFORTH_*`，Human 的 `cf` 可能正常；这不能证明 Desktop managed
Agent 的真实启动链已经完成切换。

## 3. 事故证据

### 3.1 Meeting 与 Action Run

```text
Meeting:    4277aaeb-e9a1-44e7-9d7d-62417bcc8f0b
Action Run: cf77364f-1e11-488e-81d3-2ddb89e0ad94
Epoch:      1
Condition:  blocked
Reason:     external_operation_failed
Progress:   progress_seq = 3
Terminal:   无 completion event，Meeting 未关闭
```

Action Begin 成功后，Relay 接受了 `progress_seq=1..3` 的 renewal。最终 BLOCK 发生在可用 lease 内，因此
不能归因于 `action_lease_expired`、首次 dispatch permit 或 provider 未启动。

### 3.2 Action Turn 的命令证据

主持 Agent 在 Action Turn 中：

1. 成功读取 Carryforth CLI skill；
2. 成功执行 `cf --help` 和相关 subcommand help；
3. 使用 `cf` 连续执行 Project View、Role、Stage、Requirement、Resource、Guide、Document、Meeting、
   Project Context 等 14 个 canonical 读取；
4. 14 个业务读取均以 exit code 3 和相同 `CARRYFORTH_PRIVATE_KEY is required` 失败；
5. 没有执行 Project View、Document、Context 业务写入；
6. 没有自行调用 `cf meetings actions begin/retry/return/block/close`；
7. 最终向 Harness 返回 `BLOCK(external_operation_failed)`。

因此，唯一的 Meeting 状态写入是 Harness 根据当前权威 Action Turn 输出提交的合法 BLOCK，不是 DM 槽、
其他工作槽或验收观察器旁路修改 Meeting。

完整脱敏验收记录：
`RESEARCH/AGENT_MEMORY_CF_CLI_END_TO_END_ACCEPTANCE_2026_08_10.md`。

### 3.3 进程环境证据

对事故时仍存活的 exact 主持 rollout 进程树只检查变量是否存在，不读取或输出私钥值，结果为：

| 进程层 | `BUZZ_PRIVATE_KEY/RELAY_URL` | `CARRYFORTH_PRIVATE_KEY/RELAY_URL` |
| --- | --- | --- |
| `buzz-acp` | 有 | 无 |
| `codex-acp` | 有 | 无 |
| Codex app-server | 有 | 无 |
| Code Mode host | 有 | 无 |
| `buzz-dev-mcp` | 无 | 有 |

该矩阵与代码路径一致：

- Desktop 在 `desktop/src-tauri/src/managed_agents/runtime.rs` 中仍以内部配置名向 `buzz-acp` 注入
  `BUZZ_PRIVATE_KEY`、`BUZZ_RELAY_URL` 与 `BUZZ_AUTH_TAG`；
- `crates/buzz-acp/src/lib.rs::build_mcp_servers()` 已把权威配置映射为 `CARRYFORTH_*`，但只用于 MCP；
- `managed_agent_env()` 只处理 persona、managed runtime 和 runtime fence，没有给模型 Agent 进程注入
  `CARRYFORTH_*`；
- Codex Code Mode host 继承模型 Agent 进程环境，所以同样缺少新 CLI 合同。

## 4. 根因

### 4.1 同一个 Agent 存在两条不同的命令执行面

当前实现隐含了一个错误假设：Agent 执行 `cf` 时总会经过 `buzz-dev-mcp`。实际至少有两条路径：

```text
路径 A：Agent -> buzz-dev-mcp shell/private shim -> cf
路径 B：Agent -> provider built-in tool / Code Mode host -> shell -> cf
```

阶段三只为路径 A 映射了新凭据。路径 B 直接继承 Agent 子进程环境，仍只有 ACP 内部使用的旧变量。
`cf` 又按计划严格拒绝旧变量，因此路径 B 必然认证失败。

### 4.2 环境转换逻辑被写成 MCP 专用实现

`build_mcp_servers()` 已经知道：

- canonical Relay URL；
- managed Agent Nostr private key；
- owner auth tag；
- runtime mode 与 fence。

但这些值被直接拼装进 `McpServer.env`，没有形成一个可同时供模型 Agent spawn 与 MCP spawn 使用的
typed、单一来源的 Carryforth CLI 环境。结果是两个执行面的公开合同发生漂移。

### 4.3 验收把无需认证的 help 当成了可用性证明

静态扫描、binary 名称、`cf --help`、MCP env unit test 都通过，却没有验证：

```text
真实 managed Agent
  -> 真实模型内置工具宿主
  -> cf authenticated read/write
  -> localhost Relay receipt
  -> canonical readback
```

所以“命令存在”和“Agent 能使用命令”被错误地视为同一个完成条件。

### 4.4 `external_operation_failed` 是正确外层分类

当前 Action prompt 已明确：只有具体业务命令、权限、CAS 或 canonical readback 真实失败时才允许 BLOCK。
本次所有业务命令都返回明确认证错误，Agent 没有因为隐藏的 Decision Attempt、adoption、slot/session 或
`mode=host_direct` 自行推断冲突。

因此不应通过弱化提示词、忽略 exit code 3 或把认证错误当成成功来修复。正确位置是 CLI 执行宿主的
启动环境边界。

## 5. 修复不变量

1. `cf` 仍是唯一正式 Agent-first CLI，不恢复 `buzz` alias；
2. `cf` 仍只读取 `CARRYFORTH_RELAY_URL`、`CARRYFORTH_PRIVATE_KEY`、`CARRYFORTH_AUTH_TAG`；
3. 不在 CLI 内增加 `BUZZ_*` fallback；
4. ACP / Desktop 内部暂时保留的 `BUZZ_*` 配置只能作为内部输入，在 managed Agent spawn 边界显式转换；
5. 同一个逻辑 Agent 的 MCP、Code Mode、内置 shell 与其他 provider 原生工具必须看到同一份权威
   `CARRYFORTH_*`；
6. persona、用户自定义 env 或父 shell 中的同名值不得覆盖 managed Agent 的权威身份；
7. 旧 `BUZZ_*` CLI 变量不得继续暴露给模型 Agent 子进程；
8. 私钥和 auth tag 不得写入 prompt、observer、结构化日志、错误详情或测试快照；
9. initial pool、lazy wake、refill、crash respawn 与 generation replacement 必须使用相同环境构造器；
10. 不修改 Meeting prompt、lease、Coordinator、Action Begin、ACK 或 BLOCK 状态机；
11. 不新增数据库 migration，不 reset、truncate、drop、删除 volume 或重建 Desktop identity；
12. 本修复不自动 Retry、Abort 或关闭现有 blocked Meeting。

## 6. 修复方案

### 6.1 在 ACP 建立单一的 Carryforth CLI 环境合同

在 `buzz-acp` 中增加一个范围明确的 typed builder，例如：

```text
ManagedCliEnvironment {
  relay_url,
  private_key,
  auth_tag?,
}
```

它只从 ACP 已解析的权威运行配置生成：

- `Config.relay_url`；
- `Config.keys` 对应的 managed Agent identity；
- 已验证且非空的 owner auth tag。

builder 输出三个公开变量：

```text
CARRYFORTH_RELAY_URL
CARRYFORTH_PRIVATE_KEY
CARRYFORTH_AUTH_TAG（可选）
```

不得从 persona 或任意父进程中的 `CARRYFORTH_*` 反向推断权威身份。auth tag 不存在时必须显式移除，
不能继承陈旧值。

### 6.2 同时注入模型 Agent 与 MCP 子进程

同一个 typed builder 必须被以下两处复用：

1. `PoolStartup.extra_env` / `managed_agent_env()`：注入 Codex、Claude 等模型 Agent 子进程；
2. `build_mcp_servers()`：注入 `buzz-dev-mcp`。

这不是复制三行 env 设置。两个入口必须消费同一份结构化结果，避免以后 CLI 改名或身份来源调整时再次
只更新一个执行面。

模型 Agent 进程拿到新变量后，其 Code Mode host 和内置 shell 会自然继承。因此无需为每一种 provider
工具单独增加 Carryforth 逻辑。

### 6.3 将 CLI 身份变量设为 Harness-owned

模型 Agent spawn 当前会合并父环境、persona env 与 Harness env。三个 `CARRYFORTH_*` 必须和 managed
owner/runtime fence 一样被视为 Harness-owned：

- persona 中出现这些名称时过滤或拒绝；
- 父 shell 已存在同名变量时由 Harness 权威值覆盖；
- auth tag 缺失时显式 `env_remove`；
- 不允许 `extra_env` 的普通“仅缺失时设置”策略让陈旧父环境获胜。

Desktop 当前已有 reserved env 门禁；ACP 仍需做 defense-in-depth，以覆盖直接启动 ACP、旧 persona 与未来
其他 Desktop provider 路径。

### 6.4 从模型 Agent 环境移除退役的公开 CLI 变量

Desktop 可以继续用内部 `BUZZ_*` 启动 ACP，但 ACP 在 spawn 模型 Agent 时应：

1. 先从自身已解析配置生成权威 `CARRYFORTH_*`；
2. 对模型 Agent command 显式移除 `BUZZ_PRIVATE_KEY`、`BUZZ_RELAY_URL`、`BUZZ_AUTH_TAG`；
3. 再写入 Harness-owned `CARRYFORTH_*`。

这样既不要求本阶段同时重命名 Relay/ACP 内部配置，也不会向 Agent 暴露旧 CLI 合同。现有 Git credential、
provider API key 与 runtime supervision env 不在本次清理范围，不能顺带删除。

### 6.5 覆盖所有 Agent 生命周期入口

环境修复不能只作用于首次 pool 初始化。应逐项确认：

- eager pool 初始化；
- lazy pool 首次唤醒；
- 空闲槽 refill；
- provider/transport failure 后 respawn；
- process generation replacement；
- 并行度 1 与并行度 4；
- Codex ACP 与当前支持的其他 ACP Adapter。

任何路径创建出的新模型进程都必须获得相同 Carryforth CLI 环境。旧进程不会被在线变更环境，因此交付后
必须重启 managed ACP/Agent 进程，而不是只刷新 Desktop 页面。

### 6.6 增加脱敏启动诊断

为避免以后只能从模型输出反推环境，ACP 可在 Agent spawn 时记录低敏结构化状态：

```text
managed_cli_env_ready=true
relay_url_present=true
private_key_present=true
auth_tag_present=true|false
legacy_cli_env_removed=true
```

日志不得包含 Relay auth tag、private key、nsec、环境变量值或其可逆变形。URL 若需记录，也只使用现有
安全的 canonical relay identifier，不新增完整敏感 query 输出。

`external_operation_failed` 继续作为 Relay 的低基数 Action reason；详细 CLI stderr 只留在受控 Agent
activity/脱敏验收记录中，不扩大 wire error vocabulary。

## 7. 自动化测试

### 7.1 环境构造单元测试

覆盖：

1. 权威 Config 生成三个正确命名的新变量；
2. 缺少 auth tag 时输出中没有该变量，并要求 child 显式移除继承值；
3. persona 无法覆盖 `CARRYFORTH_*`；
4. 父环境中的伪造或陈旧 `CARRYFORTH_*` 被权威值覆盖；
5. 模型 Agent child 不再看到三个旧 `BUZZ_*` CLI 变量；
6. MCP 与模型 Agent env 来自同一 builder，名称与值一致；
7. 日志和 debug 表达不包含 secret 值。

测试断言 secret 时只在进程内比较，不将值打印到失败消息。

### 7.2 子进程环境集成测试

使用不连接真实 provider 的 probe child，分别模拟 Agent process 与 MCP process，只返回变量的
**存在性和名称**：

```text
Agent child: CARRYFORTH_*=present, BUZZ_*=absent
MCP child:   CARRYFORTH_*=present, BUZZ_*=absent
```

覆盖首次 spawn、lazy wake、refill 与 crash respawn，证明不是只有 constructor unit test 为绿色。

### 7.3 `cf` 真实命令回归

`cf --help` 只能作为安装检查，不能作为认证验收。至少覆盖：

- managed Agent 通过 Code Mode / built-in shell 执行 `cf project-view get`；
- 同一 Agent 通过 MCP shell 执行相同读取；
- `cf documents list`、`cf project-context contains-all`、`cf meetings show` 成功；
- 一次 revision-fenced 写入与 canonical readback 成功；
- 只设置旧 `BUZZ_*` 直接运行 `cf` 仍稳定以 exit code 3 失败；
- 缺失新 key 的错误仍只指向 `CARRYFORTH_PRIVATE_KEY`。

live test 只能使用独立 scratch Community 或明确的非破坏性对象，禁止 reset localhost 主开发库。

### 7.4 Agent 与 Meeting 端到端

1. 创建全新 5 人冻结 roster Meeting；
2. 创建成功后验收 DM 零干预，不调用 Board、Floor、Action 或三域写命令；
3. Coordinator 自然完成 4–6 条 Speech 和 Action Begin；
4. Action epoch 1 通过内置执行宿主使用 `cf` 完成 Project View、Document、含当前 Meeting 的 Context
   Edge 写入与 canonical 回读；
5. Agent 返回 `COMPLETE`，Harness 生成 actions-recorded ACK；
6. Action Run 为 `completed_closed`，Meeting 为 `ended / closed`；
7. observer 中无认证错误、无旧 `buzz` CLI 调用、无 DM/其他槽会议控制写入；
8. 全流程日志不包含 secret。

再以并行度 1 和 4 各覆盖一次，确认修复不依赖特定工作槽。

## 8. 实施顺序

1. 抽取 typed Carryforth CLI env builder；
2. 将 `managed_agent_env()` 与 `build_mcp_servers()` 接到同一 builder；
3. 在 Agent spawn 层实现 Harness-owned override 与旧变量显式移除；
4. 补 initial/lazy/refill/respawn 环境测试；
5. 运行 Rust fmt、Clippy、ACP/MCP/Desktop Tauri 定向测试和 `cf` cutover 静态门禁；
6. 清理增量构建产物，重新构建并同时重启 Desktop、ACP 与 Agent 子进程；
7. 先做 managed Code Mode 与 MCP 两条 `cf` authenticated smoke；
8. 最后创建一场全新、零干预 Meeting 做三域物化和正常关闭验收。

本次没有数据库 migration。构建与测试不得执行 `reset`、`truncate`、`drop`、`docker compose down -v`
或任何会清理主开发数据的命令。

## 9. 完成标准

只有同时满足以下条件，才能把 [`cf` CLI 去 Buzz 化实现计划](../cli-cf-cutover-implementation-plan.md)
从“待真实 Agent / Meeting 验收”更新为完成：

1. 所有 managed Agent 执行宿主都只看到权威 `CARRYFORTH_*` CLI 合同；
2. 模型 Agent 子进程不再继承退役的三个 `BUZZ_*` CLI 变量；
3. MCP 与 Code Mode 两条真实路径均能完成 `cf` authenticated read/write；
4. eager、lazy、refill、respawn 和多槽测试通过；
5. `cf` 仍不接受旧变量 fallback；
6. 一场全新零干预 Meeting 在 epoch 1 完成三域写入、回读、ACK 与正常关闭；
7. 无 secret 泄露、无数据清理、无 Meeting 状态机或提示词语义回退。

## 10. 非目标

- 不在本次重命名 `buzz-acp`、`buzz-relay`、内部 crate 或内部配置变量；
- 不取消本地 Nostr 签名认证；
- 不修改 Action lease、Coordinator、Candidate-Cohort、ACK、BLOCK 或提示词合同；
- 不用 MCP 强制替代模型内置 shell；
- 不增加 `buzz` / `carryforth` CLI alias 或旧变量兼容；
- 不自动恢复、Retry、Abort 或关闭事故 Meeting；
- 不解决多个独立 Harness 使用同一 Agent key 的 active-active 协调问题；
- 不改写历史 Event、Project revision、Document revision 或 Context revision。

## 11. 实施记录

2026-08-10 已完成代码实现：

1. 在 `buzz-acp` 中新增单一的 typed Carryforth CLI environment builder，从当前 Agent 的权威
   Relay URL、Nostr key 与已验证的 owner attestation 生成 `CARRYFORTH_*`；
2. 模型 Agent / Code Mode 子进程与 `buzz-dev-mcp` 现在都消费同一 builder，不再存在 MCP 有凭据、
   模型执行宿主无凭据的分叉；
3. Agent spawn 会先移除父进程继承的 `CARRYFORTH_*` 与退役 `BUZZ_*` CLI 变量，再写入 Harness
   权威值；persona 或父 shell 不能覆盖 Relay、身份或 attestation；
4. invalid / empty owner attestation 不会进入 Agent 或 MCP；日志只记录三个字段是否存在，不记录
   Relay URL、私钥或 attestation 内容；
5. initial eager spawn、lazy wake、空槽 refill、panic recovery 与 process replacement respawn 均从
   同一 `managed_agent_env()` 取得环境，不再只有初始构造路径生效；
6. 本次未修改 Meeting Coordinator、Action lease、Candidate-Cohort、提示词、Relay、数据库或
   Desktop 数据，也未新增 migration。

已通过的自动化门禁：

- `cargo fmt -p buzz-acp -- --check`；
- `cargo check -p buzz-acp`；
- `cargo clippy -p buzz-acp --all-targets -- -D warnings`；
- `cargo test -p buzz-acp --lib`（835 项）；
- `cargo test -p buzz-acp --test pool_lifecycle_state`（9 项）；
- `cargo test -p carryforth-cli --lib`（304 项）；
- `cargo test -p buzz-dev-mcp --lib`（89 项）；
- `cargo test -p buzz-agent --lib`（267 项）；
- `scripts/check-cf-cli-cutover.sh`；
- `git diff --check`。

尚未计为通过的现场门禁：重新构建并重启 Desktop / ACP 后，分别通过 Code Mode 与 MCP 执行真实
`cf` authenticated read/write，以及创建一场全新、零干预 Meeting 验证三域物化、canonical 回读、
actions-recorded ACK 与 `ended / closed`。在这些现场门禁完成前，本文不宣称完整验收已完成。
