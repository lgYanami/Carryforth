# 角色连续性待办

## Role Brief 按 revision 增量刷新

### 当前状态

阶段 4 采用正确性优先的实现：

- 每个完整 ACP channel turn 和 heartbeat 开始前，重新读取 NIP-11 Relay identity、
  Project View v2 meta、全部 object/entity heads，以及 meta 精确指向的 membership
  snapshot；
- 使用前后 meta bracket 和共享 `VerifiedRoleBriefSnapshot` 验证完整快照；
- 每个完整 turn 都把完整最小 Role Brief 作为动态 user-context block 发送；
- native steer 是当前 turn 内的增量消息，不重新解析或重复携带 Brief；工具调用也不形成
  新的 Brief 注入；
- 读取失败时注入 `State: unavailable`，不回退到上一份 Assignment。

该方案保证 Assignment 替换在下一完整 turn 生效，但会重复查询未变化的完整投影，并在
长 session 中重复发送大体相同的 Brief。

### 优化目标

把“每轮确认当前角色身份”和“每轮重复完整项目上下文”拆开：

```text
每个完整 turn
    └── 读取并验证轻量 meta head
            ├── generation/revision/meta event 未变化
            │       └── 注入 compact Role Binding
            └── 发生变化、session 新建或缓存缺失
                    └── 读取完整 verified snapshot，重新生成完整 Role Brief

每次 managed 写入
    └── 仍重新读取完整 verified snapshot，并验证当前 Assignment fence
```

compact Role Binding 至少包含：

- `candidate | assigned | unavailable`；
- Role ID、Role name 和 level（assigned 时）；
- 当前 Assignment ID（assigned 时）；
- project revision、projection generation 和 meta event ID；
- 明确声明写入前仍须重新验证 Assignment。

完整 Role Brief 在以下时机重新注入：

- ACP session 新建或重建；
- meta event、project revision 或 projection generation 改变；
- Assignment、Role 或 membership 改变；
- 本地没有匹配当前 Relay/Community/Member 的 verified cache；
- Agent 或 supervisor 显式请求上下文刷新；
- 后续 ACP connector 能报告 context compaction/reset 时。

### 安全边界

- 缓存只用于减少读取和 prompt 重复，不能成为授权事实源。
- 轻量 meta 读取失败、Relay identity 改变或 revision 无法确认时，必须注入
  `State: unavailable`；不能把旧 Brief 继续标记为当前授权。
- 缓存键至少包含 Relay identity、Community/Project、Member pubkey、projection
  generation 和 meta event ID；Community 切换必须清空。
- managed CLI 的 Role 与 Project View 写入继续在签名前读取完整 verified snapshot；
  Relay 继续执行最终 Assignment fencing。
- native steer 不因该优化获得独立授权语义。若 Role 在长 turn 中途变化，下一完整 turn
  刷新上下文；该 turn 内的任何写入仍由 CLI/Relay 的最新 Assignment fence 拦截。

### 验收条件

- meta 未变化时，不再查询全部 object/entity heads 和 membership，也不重复发送完整
  Brief；
- meta 变化后的下一完整 turn 必须读取并注入新的完整 Brief；
- Assignment 替换后旧 Runtime 仍无法写入；
- 查询失败不会复用旧 Assignment 授权；
- candidate、assigned、unavailable 的行为与阶段 4 保持一致；
- JSON、Markdown、ACP 和 Desktop 仍由同一个共享 verified assembler 产生；
- 增加覆盖 session 重建、Community 切换、revision 改变、缓存失效和长 turn steer 的
  测试。

### 非目标

- 本优化不改变 Role、Assignment 或 Project View 的规范数据模型；
- 不用缓存替代 Relay 投影或 PostgreSQL 规范状态；
- 不在本项中实现 Work Commitment、Checkpoint、完整 Handoff 或 Runtime lease。
