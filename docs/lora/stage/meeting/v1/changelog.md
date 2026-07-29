# Meeting V1 决策变更记录

本文记录在概念设计和后端实现设计冻结后，经讨论确认的语义调整。新决策优先于旧文档中
与其冲突的描述；实现文档应同步更新为当前语义。

## 2026-07-29：Meeting Turn 工具策略改为 advisory

### 状态

已确认，纳入 Stage 3 交付。

### 原设计

Meeting Turn 强制使用 Agent 的 `Plan` permission mode，只向 Agent 暴露带
`BUZZ_DEV_MCP_READ_ONLY=1` 的 `buzz-dev-mcp`。Harness 要求目标 ACP Agent 明确支持并
成功切换到 Plan；否则 Meeting session 创建失败。

该方案试图把“会议只做讨论，不执行任务”实现为代码级只读边界。

### 变更原因

会议发言可能需要从任务、工作流、项目状态、Buzz CLI、第三方 MCP、HTTP 或 Agent 原生
工具中获取上下文。只允许一个专用只读 MCP 会显著限制 Agent 的调查能力。

此外，Plan 和 Agent 原生工具权限属于具体 ACP Runtime 的实现语义。Buzz 的 MCP 配置只能
约束经该 MCP 发起的调用，无法统一约束 Codex 等 Runtime 自带的文件、Shell、网络和其他
工具。对不支持 Buzz 专用权限策略的 Runtime 强行建立通用硬限制，会造成兼容性失败，也
不能形成所宣称的完整安全边界。

### 新决策

Meeting V1 初版采用 **advisory 工具策略**：

1. Meeting Turn 不再强制切换 Plan mode；
2. 不再要求 ACP Agent 支持特定 permission mode；
3. Meeting Turn 继承该 Agent 的正常 MCP、CLI、HTTP 和原生工具能力；
4. Meeting system prompt 明确要求工具只用于获取发言所需证据，不执行任务，不产生持久
   写操作或会议外部副作用；
5. 如果发现需要执行的事项，Agent 应把它作为结论、问题或后续行动建议写入发言，而不是
   在 Meeting Turn 中直接执行；
6. Meeting prompt 要求 Agent 不得通过工具自行发布 Meeting speech 或控制事件。Harness
   管理的自动路径仍只根据结构化模型结果构造、签名并提交 Intent、ACK、Progress、SAY、
   YIELD 和 Handoff；
7. 工具输出、会议内容和项目内容继续按不可信证据处理，不能覆盖 Meeting system prompt、
   Grant、deadline 或输出 schema。

该变更只适用于 Meeting V1。Meeting V0 保留原有的强制 Plan mode 和
`BUZZ_DEV_MCP_READ_ONLY=1` 行为；ACP Harness 根据 turn 的协议版本选择独立运行上下文，
避免 V1 的工具策略隐式改变 V0。

### 保证边界

本次变更只放宽 Agent 的上下文获取能力，不放宽 Meeting 协议。

Relay 与 Harness 仍硬性保证：

- 同一时间至多一个有效 Offer/Grant；
- 只有当前 Grant holder 可以消费 Grant；
- revision、epoch、deadline、名单、mention 和 Handoff 目标必须有效；
- Agent 原始输出不能直接发布，只有通过严格 schema 和最新权威 State 校验的结果才能
  进入 Harness 管理的自动发布路径；
- Relay 对来自 Harness、CLI 或其他客户端的协议事件执行相同的授权、revision、Grant 和
  deadline 校验；
- 迟到、过期、重复或格式错误的结果不会形成有效 speech。

Meeting V1 初版**不保证**模型在受到提示注入或行为失控时无法通过自身工具产生副作用。
“只调查、不写入”是参会 Agent 的行为约定，不是 OS、Runtime 或 MCP 层的安全隔离。
同理，若普通工具面包含带参会身份凭据的 Buzz CLI，V1 也不保证模型无法绕过 Harness
自动路径尝试提交协议事件；prompt 负责禁止这种行为，Relay 负责拒绝不满足协议条件的事件。

### 后续方向

如果后续需要代码级工具限制，可以设计 MCP Gateway、受信 Agent Runtime 的逐调用权限
策略或独立沙箱。其中 MCP Gateway 只能覆盖 MCP 调用；对于 Codex 等 Runtime 的原生工具，
还必须结合 Runtime 权限策略或进程级隔离，不能把 Gateway 单独描述为完整方案。
