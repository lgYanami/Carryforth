# Managed Agent 运行时标记冲突修复设计

> 状态：已实现，并于 2026-07-31 完成 Linux 本地运行验收。
>
> 2026-08-07 协议覆盖：本文中出现的 Project View v2 capability/写入描述只记录事故发生时
> 的历史实现。当前普通 CLI、Desktop、ACP 与 Relay 运行时均为 Project View v3-only；v2
> 只能存在于显式 operator migration/recovery 维护边界，不能作为 fallback。运行时模式的
> 当前唯一标记仍为 `BUZZ_MANAGED_RUNTIME=1`。
>
> 本文修复 Desktop 进程所有权标记与 Role/Project View managed 模式标记复用所造成的
> 活跃 Codex/ACP 子进程误杀。修复不改变 Project View、Role Continuity、Assignment、
> Runtime fence 或 Relay 授权协议，也不通过降低 Agent 并行度规避问题。

## 0. 实施结果

本次交付按本文契约完成：

- 在 `buzz-core::agent_process_env` 中集中定义 Desktop owner、managed runtime mode 和
  start nonce 三个变量名；
- `buzz-acp` 的首次启动、lazy wake、slot refill、panic recovery、respawn 和 developer
  MCP 全部通过同一环境组装边界，保留精确 Desktop owner 并独立注入 mode；
- `buzz-cli` 不再解释 `BUZZ_MANAGED_AGENT`，只接受精确的
  `BUZZ_MANAGED_RUNTIME=1`；
- Desktop 保存/合并 Agent env 时保留三个 harness-owned key，Linux/macOS
  dead-instance sweep 只清理“foreign owner 且 ancestry 明确 untracked”的进程；tracked
  与 unknown 都 fail safe；
- 纯 ancestry/候选分类放在 `runtime/sweep.rs`，没有放宽文件大小门禁或加入例外。

验证结果：

1. `buzz-core` 232、`buzz-acp` 632、`buzz-cli` 265 项 library test 通过；
2. Desktop `managed_agents` 741 项通过，0 失败；
3. workspace 与 Desktop 全目标 clippy 在 `-D warnings` 下通过，Rust/Tauri fmt、Desktop
   文件大小门禁通过；
4. 持久化 4-slot 的真实 `test-1` pool 成功初始化；所有 adapter、app-server、Codex 和
   MCP 后代都同时具有精确 Desktop owner 与 mode marker；
5. 子进程连续存活超过 140 秒，跨过至少两轮 60 秒 sweep，PID 稳定且无僵尸、误杀、
   `Broken pipe` 或意外 respawn；
6. 本地 Channel 端到端消息得到 Agent 的明确 `OK` 回复。

完整 `just ci` 还确认 workspace clippy、Desktop/Web 静态检查和 Flutter analyze 通过，
但被既有 `buzz-db::replica_fence::tests::fence_starts_closed_and_opens_on_advance` 阻断：该
测试把纳秒精度 `Utc::now()` 与内部微秒精度 round-trip 做精确相等比较。独立复跑可稳定
复现，代码来自历史提交 `29c48883d3`，不在本次修复范围。macOS 的
`KERN_PROCARGS2`/`proc_pidinfo` 实现已编译时保持同一决策函数，仍需对应 CI runner
完成平台执行验证。

## 1. 文档目的

Project View 阶段 4 为 managed Agent 增加动态 Role Brief 与写入 fencing。为了让 Agent
通过 shell 或 developer MCP 调用 `buzz` 时进入 managed fail-closed 模式，`buzz-acp`
会给模型与 MCP 子进程注入 `BUZZ_MANAGED_AGENT=1`。

这项注入来自提交 `1f8b773cf`（`feat(project-view): bind managed agents to role briefs`）；
它单独满足了 CLI managed 模式需求，但没有保留 Desktop 已存在的同名所有权契约。

但是 Desktop 在该变更之前已经使用同名变量承载另一个契约：

```text
BUZZ_MANAGED_AGENT=<desktop-instance-id>
```

Desktop 的孤儿进程清理器把变量值当作进程所属的 Desktop instance ID。两个独立含义在
真实 Desktop → `buzz-acp` → `codex-acp`/Codex/MCP 进程树中相遇后，清理器会把值为 `1`
的活跃子进程识别成死亡实例的孤儿并终止。

本文固定以下内容：

1. 不同环境变量分别承担生命周期所有权、managed 行为选择和运行时授权；
2. Desktop、ACP、CLI 与 MCP 之间如何传递这些值；
3. 孤儿清理器如何在保留真实回收能力的同时避免误杀活跃进程树；
4. 如何兼容已有本地进程、发布和回滚；
5. 哪些自动化测试与真实运行观察构成修复完成的证据。

## 2. 事故摘要与证据

### 2.1 用户可见行为

一次受影响的运行通常表现为：

1. Agent 启动并成功回复一到两条消息；
2. 后续 turn 已被 Relay 接收，Desktop 也能显示 Human 消息；
3. Agent 可能短暂显示工作中，但没有发布最终回复；
4. 同一条消息被 `buzz-acp` 重试，最终达到重试上限；
5. 外层 managed Agent 仍显示为运行中，因此 UI 状态与真实模型 worker 状态不一致。

这解释了为什么问题看起来像 Channel session 卡住或 Agent 忽略消息，实际上是处理 turn
的 ACP transport 在运行中消失。

### 2.2 进程与日志证据

已观察到的证据链为：

```text
Desktop tracked runtime: buzz-acp                    仍存活
    └── codex-acp / Codex app-server                 被清理
            └── buzz-dev-mcp                         被清理或随父进程退出

Desktop log:
  reaping 49 orphaned agent(s) from dead instance '1'
  reaping 2 orphaned agent(s) from dead instance '1'

buzz-acp log:
  agent_returned — respawning
  IO error: Broken pipe (os error 32)
```

Desktop 周期清理间隔为 60 秒。首次 24-slot pool 会形成约 24 个 adapter 和 24 个 Codex
app-server，因此一次清理约 49 个相关进程；之后单个 slot respawn 通常重新产生 2–3 个
候选进程，并在下一轮再次被清理。

以下可能原因已经被现场证据排除为本次主因：

- Codex 登录、模型额度或服务端限流；
- session context 达到上限或跨 Channel 污染；
- Linux OOM、cgroup memory/pid 上限或文件描述符耗尽；
- `codex-acp` 或 Node 版本启动后自行退出；
- Relay 或 Desktop 主进程失效。

Project View v2 capability 不可用、Role Brief unavailable 和 Codex sandbox 无法访问本地
Relay 是独立问题。它们应产生明确的工具或权限失败回复，不能导致 ACP pipe 被外部关闭。

## 3. 当前实现与根因

### 3.1 Desktop 的已有所有权契约

`desktop/src-tauri/src/managed_agents/runtime.rs` 在启动 harness 时写入：

```text
BUZZ_MANAGED_AGENT=xyz.block.buzz.app.dev
BUZZ_MANAGED_AGENT_START_NONCE=<random-nonce>
```

其中第一个值是稳定的 Desktop instance namespace。release、dev 和 worktree build 使用
不同标识，使它们可以并存而不互相清理进程。Desktop 的以下路径依赖这个语义：

- 判断一个 PID 是否属于当前实例；
- 清理当前实例未被 runtime map 跟踪的孤儿；
- 查找已停止 Desktop 实例留下的进程；
- 退出时回收 harness 及其进程组；
- 验证 PID receipt 是否仍属于当前 Desktop。

该变量不是布尔值，也不是 Role 授权凭据。

### 3.2 `buzz-acp` 引入的第二种语义

`crates/buzz-acp/src/lib.rs::managed_agent_env()` 当前会：

1. 从 persona env 中删除 `BUZZ_MANAGED_AGENT`；
2. 忽略 `buzz-acp` 父进程已经携带的 Desktop instance ID；
3. 给每个模型子进程强制写入 `BUZZ_MANAGED_AGENT=1`；
4. 给 developer MCP server 同样写入值 `1`。

`crates/buzz-cli/src/commands/project_view_snapshot.rs::is_managed_runtime()` 又要求该值
严格等于 `1`，以决定 Role 和 Project View 写入是否需要 managed snapshot 与 Assignment
fence。

因此，同一个变量同时表达：

```text
生命周期所有权：BUZZ_MANAGED_AGENT=<desktop-instance-id>
行为模式开关：BUZZ_MANAGED_AGENT=1
```

这是根本的契约冲突。

### 3.3 为什么清理器会杀死活跃子进程

Desktop 的 periodic sweep 每 60 秒取得当前 runtime map 中的 harness PID 作为
`skip_pids`。外层 `buzz-acp` 在这个列表中，所以不会被直接清理。

模型 adapter 和 Codex app-server 使用自己的进程组，且不作为独立 runtime 存入
Desktop map。dead-instance sweep 读取它们的环境后得到 instance ID `1`：

```text
candidate instance = "1"
current instance   = "xyz.block.buzz.app.dev"
candidate != current
desktop_is_alive_for_instance("1") == false
=> resolve process groups and kill
```

同实例 sweep 已经使用父 PID、进程组和有界祖先遍历豁免 tracked harness 的活跃后代；
dead-instance sweep 没有应用这项保护。因此，即使父 harness 明确处于 tracked set，带错误
marker 的子进程仍会直接进入 foreign dead-instance 清理。

### 3.4 为什么问题不是每次立即出现

清理按周期执行，而不是在每个 turn 开始时执行：

- 短 turn 可能在下一次 sweep 前完成并发布回复；
- 长 turn、工具调用和 Project View 读取更容易跨越 sweep 边界；
- slot respawn 后存在另一个短暂可工作窗口；
- 多次清理后 adapter stdin/stdout 已关闭，后续 turn 直接得到 `Broken pipe`。

因此成功与失败取决于时序，不能用“已经成功回复过”证明进程池健康。

## 4. 设计目标与非目标

### 4.1 目标

1. Desktop 可以准确识别 release、dev 和 worktree 实例各自拥有的整个 Agent 进程树。
2. `buzz-cli` 可以明确判断自己是否由 managed Agent harness 调用，并保持 Role/Project
   View 写入 fail closed。
3. 模型 adapter、Codex app-server 和 MCP 不得修改或丢失 Desktop 所有权标记。
4. 活跃 tracked harness 的任何正常后代都不会被 periodic dead-instance sweep 清理。
5. 父 Desktop 或 harness 真正死亡后，现有孤儿回收能力仍然有效。
6. 独立从终端启动的 `buzz-acp` 不会被任意正在运行的 Desktop 认领或终止。
7. 修复对 1、4、24 或其他合法并行度均成立。

### 4.2 非目标

- 不改变 Channel 到 ACP session 的 affinity 或上下文隔离策略；
- 不用降低并行度作为修复手段；
- 不修改 Project View/Role event kind、数据库 schema 或 Relay API；
- 不放宽 Assignment fence、verified snapshot 或 Relay 最终授权；
- 不在本修复中解决 Relay 未宣告 Project View v2、Role Brief unavailable 或 Codex
  sandbox 网络策略；
- 不重构 Desktop 全部 PID sweep 为新的进程监督框架。

## 5. 新的环境变量契约

修复后固定三类互不重叠的状态：

| 环境变量 | 值 | 唯一职责 | 是否授予权限 |
|---|---|---|---|
| `BUZZ_MANAGED_AGENT` | Desktop instance ID | Desktop 进程生命周期所有权 | 否 |
| `BUZZ_MANAGED_RUNTIME` | 严格为 `1` | 启用 CLI managed fail-closed 行为 | 否 |
| `BUZZ_RUNTIME_FENCE_PATH` | 绝对文件路径 | 提供动态 Runtime ID/epoch fence | 否，Relay 仍最终验证 |

约束如下：

1. `BUZZ_MANAGED_AGENT` 是 opaque instance namespace。除 Desktop 清理器外，其他组件
   不解释其格式，也不能把它归一化为 `1`、`true` 或其他布尔值。
2. `BUZZ_MANAGED_RUNTIME` 只接受精确值 `1`。缺失、空字符串和其他值都按 unmanaged
   处理，避免宽松 truthy 解析产生漂移。
3. `BUZZ_MANAGED_RUNTIME=1` 只会启用更严格的本地检查，不是授权证明。Agent 即使伪造
   该值也不能获得 Role 权限；verified snapshot、Assignment 和 Relay handler 继续执行
   实际授权。
4. Runtime fence 仍通过动态文件传递。静态 `BUZZ_RUNTIME_ID`/
   `BUZZ_RUNTIME_EPOCH` 只保留现有兼容读取，不重新成为首选路径。
5. `BUZZ_MANAGED_AGENT_START_NONCE` 保持现有 harness generation 用途，不替代稳定的
   instance namespace。

为避免字符串再次漂移，实现时应在所有相关 crate 中使用具名常量。若放入共享
`buzz-core` 模块，只导出零 I/O 字符串常量并补齐公共 API 文档；该模块不得读取环境或
引入 Desktop/ACP 依赖。

## 6. 详细修改设计

### 6.1 Desktop 启动边界

`desktop/src-tauri/src/managed_agents/runtime.rs::spawn_agent_child()` 保持现有写入：

```text
BUZZ_MANAGED_AGENT=<current_instance_id(app)>
BUZZ_MANAGED_AGENT_START_NONCE=<uuid>
```

Desktop 不写入 `BUZZ_MANAGED_RUNTIME`。是否为 managed Role/Project View runtime 由
`buzz-acp` 这一可信 harness 边界决定，避免普通 Desktop 子进程无差别进入 managed 模式。

`desktop/src-tauri/src/managed_agents/env_vars.rs::RESERVED_ENV_KEYS` 增加：

```text
BUZZ_MANAGED_AGENT
BUZZ_MANAGED_RUNTIME
BUZZ_MANAGED_AGENT_START_NONCE
```

虽然 Desktop 当前在用户 env 之后覆盖所有权和 nonce，保存时拒绝、运行时过滤仍能形成
清晰的防御边界，并处理旧的 on-disk persona/agent record。

### 6.2 ACP 模型子进程环境

`managed_agent_env()` 不再自行发明所有权值。它接收或捕获父 `buzz-acp` 的 Desktop
owner marker，并构造确定性的子进程环境：

```text
始终注入：
  BUZZ_MANAGED_RUNTIME=1

父进程存在非空 owner marker 时原样注入：
  BUZZ_MANAGED_AGENT=<exact-parent-value>

父进程没有 owner marker 时：
  不向子进程增加 BUZZ_MANAGED_AGENT
```

persona env 中以下键必须先被过滤：

- `BUZZ_MANAGED_AGENT`；
- `BUZZ_MANAGED_RUNTIME`；
- `BUZZ_RUNTIME_ID`；
- `BUZZ_RUNTIME_EPOCH`；
- `BUZZ_RUNTIME_FENCE_PATH`。

建议把 owner marker 作为显式参数传给纯 helper，而不是让单元测试修改进程全局环境：

```text
managed_agent_env(
    persona_env,
    runtime_fence_path,
    desktop_owner_marker,
)
```

首次 pool 初始化、lazy wake、slot refill、panic recovery 和所有 respawn 必须统一调用这一个
helper，禁止出现只修首次启动、respawn 又回退到旧值的分支。

### 6.3 `AcpClient::spawn` 强制项

`crates/buzz-acp/src/acp.rs` 当前把 `BUZZ_MANAGED_AGENT` 和 runtime fence 视为
harness-owned 值。修复后强制列表扩展为：

```text
BUZZ_MANAGED_AGENT       # 仅在父 harness 有 owner marker 时传入
BUZZ_MANAGED_RUNTIME     # 始终为 1
BUZZ_RUNTIME_FENCE_PATH  # 有动态 fence 时传入
```

这些键不能使用普通的“父环境存在则 operator wins”逻辑；最终值必须来自 harness 组装的
`extra_env`。同时仍需移除 supervisor private key、supervision state path 和静态 runtime
坐标，防止可信 supervisor 能力越过模型进程边界。

### 6.4 Developer MCP 环境

`build_mcp_servers()` 必须与模型子进程使用同一份 owner/mode 契约：

- 始终传递 `BUZZ_MANAGED_RUNTIME=1`；
- 父 harness 有 Desktop owner marker 时原样传递；
- standalone harness 没有 owner marker 时不构造伪值；
- 动态 runtime fence path、Relay URL、Agent private key 和 auth tag 保持现有逻辑。

这样 Agent 通过 ACP shell、Codex 内部命令或 developer MCP 间接执行 `buzz` 时，managed
模式与 Desktop 所有权都保持一致。

### 6.5 CLI managed 模式

`crates/buzz-cli/src/commands/project_view_snapshot.rs::is_managed_runtime()` 改为只检查：

```text
BUZZ_MANAGED_RUNTIME == "1"
```

Role command 和 Project View v2 写入继续复用该单一 helper。任何 CLI 路径都不得再根据
`BUZZ_MANAGED_AGENT` 的存在或内容推断 Role managed 状态。

该修改需要与 `buzz-acp` 一起发布：仅更新 ACP 而保留旧 CLI，会使旧 CLI 看不到值 `1`；
仅更新 CLI 而保留旧 ACP，则新变量不存在。Desktop 开发构建和发布 bundle 必须把
`buzz-acp`、bundled `buzz` CLI 与 Desktop 作为一个兼容单元重建。

### 6.6 dead-instance sweep 防误杀

`reap_dead_instance_agents()` 的 macOS 和 Linux 实现都必须在把候选 PID 加入
`foreign_agents` 前应用 tracked-descendant 保护。

每个候选进程的决策顺序固定为：

```text
1. PID 有效、属于当前用户且不是 Desktop 自身
2. PID 不在 tracked harness skip_pids 中
3. 进程名属于 Buzz 已知 Agent/adapter/MCP 范围
4. 如果它是任意 tracked harness 的活跃后代：跳过
5. 读取 BUZZ_MANAGED_AGENT owner marker
6. owner == current instance：交给同实例 sweep，不在这里处理
7. 对 foreign owner 检查对应 Desktop 是否仍存活
8. 只有确认 Desktop 不存活时才按已解析 PGID 清理
```

第 4 步必须复用已有的直接父 PID、PGID 和有界祖先链逻辑，而不是只检查直接父进程。
`buzz-acp → node shim → codex-acp → app-server` 可能跨越多层，也可能由 adapter 创建新的
进程组。

若读取 `/proc/<pid>/stat`、macOS process info 或祖先链时出现瞬时失败，清理器应按
“归属未知”处理：本轮跳过，下一周期重试。回收延迟 60 秒优于误杀一个正在处理消息的
Agent。

这项保护不削弱正常关闭：shutdown 路径使用空 `skip_pids`，因此所有者明确且父 Desktop
已经停止的进程仍可被清理。真正退出的 harness 也不再是 tracked live ancestor，其遗留
子进程会在后续 sweep 被回收。

### 6.7 可观测性

保留现有清理日志，并增加足以区分决策路径但不包含 secret 的字段：

- sweep 类型：same-instance、dead-instance、shutdown；
- instance namespace；
- candidate 数量和最终清理数量；
- 因 tracked ancestor 被豁免的数量；
- ancestry unknown 而延迟处理的数量。

`buzz-acp` 启动日志可以记录 `managed_runtime=true` 和
`desktop_owned=true|false`，但不能打印 private key、auth tag、runtime fence 内容或
完整 persona env。

## 7. 兼容、发布与回滚

### 7.1 持久化兼容

本修复只改变进程环境和清理判定：

- 不新增数据库 migration；
- 不修改 Agent record schema；
- 不修改 Project View 或 Role wire schema；
- 不删除 Community、Channel、session、Project View 或 Docker volume 数据。

### 7.2 运行中旧进程

当前值为 `BUZZ_MANAGED_AGENT=1` 的模型进程不能原地修复环境变量。部署修复时必须：

1. 正常停止 Desktop 和全部 managed Agent；
2. 等待旧 harness、adapter、app-server 与 MCP 退出；
3. 同时重建 Desktop、`buzz-acp`、`buzz-cli` 和 `buzz-dev-mcp`；
4. 启动 Desktop 并重新生成 Agent pool；
5. 不复用旧 ACP session 进程，只复用正常持久化的数据和配置。

正常停止已经能够按进程组回收旧树；若旧版本异常退出，新版本启动 sweep 可以继续清理
带旧 marker 的残留进程。实施时不得通过删除 app data 或 Docker volume 清场。

### 7.3 版本组合

| Desktop | `buzz-acp`/CLI | 结果 |
|---|---|---|
| 旧 | 旧 | 保留本缺陷 |
| 新 | 旧 | descendant guard 可防止活跃树误杀，但变量冲突仍存在，仅作过渡保护 |
| 旧 | 新 ACP + 新 CLI | 子进程继承旧 Desktop instance ID，可正常工作 |
| 任意 | ACP 与 CLI 版本不匹配 | 不支持；managed 模式可能判断错误 |
| 新 | 新 ACP + 新 CLI | 目标组合 |

因此发布门禁必须检查 bundled CLI 与 harness 来自同一构建版本。Desktop 对旧 ACP 的
descendant guard 是容错，不是长期支持承诺。

### 7.4 回滚

回滚不涉及数据恢复：

1. 先正常停止使用新变量契约的全部 Agent；
2. 整体回滚 Desktop、ACP 与 CLI，不能只回滚其中一个；
3. 重新启动后验证进程树与 marker；
4. 保留所有数据库、Community 配置和本地 Agent record。

旧版本仍包含本缺陷，因此回滚仅用于其他严重回归，不能被视为该问题的稳定运行方案。

## 8. 测试设计

### 8.1 `buzz-acp` 单元测试

新增或改写以下测试：

1. 父 owner marker `xyz.block.buzz.app.dev` 被逐字传给模型子进程；
2. owner marker 不会变成 `1`；
3. standalone harness 缺少 owner marker 时，子进程也不获得伪造 owner；
4. 所有场景均获得且只获得一个 `BUZZ_MANAGED_RUNTIME=1`；
5. persona 试图覆盖 owner、managed mode、static runtime coordinates 或 fence path 时被过滤；
6. 首次启动、lazy wake、slot refill、panic recovery 和 respawn 使用相同结果；
7. MCP server 获得与模型子进程相同的 owner/mode，并继续获得动态 fence path。

测试 helper 接受显式 owner 参数，避免并行 Rust 测试通过 `set_var` 竞争全局环境。

### 8.2 `buzz-cli` 单元测试

至少覆盖：

| 环境 | `is_managed_runtime()` |
|---|---:|
| 无两个 marker | false |
| `BUZZ_MANAGED_AGENT=xyz.block.buzz.app.dev` | false |
| `BUZZ_MANAGED_AGENT=1`，无新 marker | false |
| `BUZZ_MANAGED_RUNTIME=1` | true |
| `BUZZ_MANAGED_RUNTIME=0/true/空` | false |

随后保留 Role/Project View managed 写入测试，确认未分配 Agent 仍 fail closed、已分配 Agent
仍自动附带最新 Assignment fence、普通 Human CLI 不受影响。

### 8.3 Desktop 清理器测试

把 foreign-process 分类尽可能提取为纯决策函数，并覆盖：

1. 当前实例进程不会进入 dead-instance 清理；
2. 另一个仍存活 Desktop 的进程不会被清理；
3. 真正死亡 foreign Desktop 的孤儿会被清理；
4. marker 错误但仍是 tracked harness 活跃后代的进程不会被清理；
5. 多层祖先和新进程组仍能识别为活跃后代；
6. 祖先信息不可读时跳过而不是清理；
7. shutdown 使用空 skip set 时可以回收同一进程；
8. dev、release 与 worktree instance namespace 继续保持精确边界。

Linux 使用 `/proc` 解析定向测试；macOS 保留纯 buffer/ancestor 测试，并在对应 CI runner
执行平台实现。

### 8.4 真实进程集成测试

增加一个不依赖真实模型的确定性 harness 测试：

1. 启动伪 Desktop owner、真实 `buzz-acp` 和可控 ACP child；
2. child 再启动一层 worker，并创建独立进程组；
3. 以 4 slots 初始化；
4. 连续执行至少两轮 60 秒 sweep；
5. 确认 tracked harness、adapter 和 worker 均存活；
6. 停止 harness 后再次 sweep，确认遗留 worker 被回收；
7. 检查没有僵尸进程和跨实例误杀。

若完整 120 秒测试不适合普通 unit gate，可使用可注入 clock/纯候选分类进入默认 CI，把
真实时间进程测试放入 Desktop diagnostic integration 或专用 `just` 门禁。

## 9. 本地验收标准

修复构建必须完成以下人工/自动结合的本地验收：

1. 将测试 Agent 的持久化并行度确认设为 4，而不是仅修改当前进程环境；
2. 启动 Desktop，确认 pool 日志明确为 `agents=4`；
3. 等待至少 120 秒，覆盖两个 periodic sweep；
4. 发送一个短对话并获得回复；
5. 发送一个持续超过 60 秒、包含 `buzz`/MCP 工具调用的 turn 并获得明确回复；
6. 在至少两个 Channel 并行发送消息，确认各自 ACP session 上下文保持隔离；
7. 日志中不得出现 `dead instance '1'`、非预期 `agent_returned exited` 或
   `Broken pipe (os error 32)`；
8. `ps` 中每个 slot 的 adapter/app-server 保持存活，不积累 `<defunct>` 子进程；
9. 正常关闭 Desktop 后，相关容器数据保持不变，managed Agent 进程树全部退出；
10. 再次启动后确认旧孤儿可以回收、Agent 可以继续回复。

若 Relay 尚未宣告 Project View v2，Agent 可以明确回复 capability/Role unavailable；只要
turn 正常结束，就与本次进程误杀修复不冲突。验收不能把“功能不支持”的明确回复误判为
ACP transport 失败。

## 10. 质量门禁

实现完成后至少执行：

```bash
. ./bin/activate-hermit
cargo test -p buzz-acp --lib
cargo test -p buzz-cli --lib
cargo test --manifest-path desktop/src-tauri/Cargo.toml managed_agents
just desktop-tauri-fmt
just ci
```

若修改了 Desktop 进程扫描平台代码，还必须在 Linux 与 macOS CI runner 上分别通过相关
测试；Linux 本地通过不能替代 macOS `KERN_PROCARGS2`/`proc_pidinfo` 路径验证。

## 11. 实施顺序

1. 定义环境变量常量和契约测试；
2. 修改 ACP pool、respawn、MCP 与 `AcpClient::spawn` 环境传递；
3. 修改 CLI managed mode 判断并补齐 Role/Project View fail-closed 测试；
4. 给 Desktop dead-instance sweep 增加 tracked-descendant 与 unknown-ancestry 保护；
5. 增加跨组件和真实进程回归测试；
6. 同时重建 Desktop、ACP、CLI 和 MCP；
7. 按第 9 节完成 4-slot、跨两轮 sweep 的本地验收；
8. 验证后再把 changelog 状态从“待修复”更新为已交付，并记录实际测试计数与提交。

## 12. 未采用的方案

### 12.1 只把并行度降为 1

并行度只改变一次被杀的子进程数量。单 slot 仍会产生 adapter 和 app-server，并在下一轮
sweep 被清理，不能修复契约冲突。

### 12.2 让 CLI 把任意非空 `BUZZ_MANAGED_AGENT` 当作 managed

这可以暂时让 Desktop instance ID 同时充当布尔值，但继续把生命周期所有权与 Role 行为
耦合。standalone harness、未来 Desktop instance 格式和用户环境都会保持歧义，容易再次
出现清理或权限回归。

### 12.3 只在清理器中特判值 `1`

特判会掩盖错误传播，并使真正遗留的旧 managed worker 无法回收。以后若错误值变成
`true` 或其他字符串，同类误杀会再次出现。

### 12.4 只增加 tracked-descendant 豁免

这能阻止当前活跃 harness 下的误杀，但没有修复变量双重语义。父 harness 退出、跨版本
进程、standalone ACP 和 CLI managed 判断仍不明确，因此 descendant guard 只能作为第二道
保护，不能替代契约拆分。

### 12.5 本次直接重命名 Desktop 所有权变量

把已有所有权变量整体迁移为新名称会扩大 release/dev/worktree、PID receipt、启动清理、
退出清理和旧进程兼容范围。当前缺陷可以通过新增独立 managed-mode 标记安全修复，因此
本次保留成熟的 Desktop 所有权契约；若未来需要重命名，应另做双写、双读和移除旧值的
分阶段迁移。
