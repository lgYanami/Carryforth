# Meeting V2 旧行动物化设计（已废止）

本文原先定义的 Meeting 内部 `Action Plan`、`Step`、Project View 专用 materializer、
逐步 receipt 和 `ready_to_close` 状态已经废止，并已从后端实现中移除。

废止原因：

- 主持 Agent 本来就能通过普通 `buzz` CLI 操作 Project View 和其他业务系统；
- Human 主持本来就能通过现有管理界面完成同类操作；
- Meeting 专用 Plan/Step 把行动范围错误地限制为少量 Project View 对象；
- 编译器和逐步执行器形成了第二套业务写入协议，增加恢复复杂度，却无法证明 Board 与外部
  状态在语义上一致；
- Human 路径被迫编辑一套内部执行结构，而不是使用已有业务界面。

现行规范只有一条路径：

1. 最终 Board 冻结；
2. 主持人进入 `finalizing_actions`；
3. Agent 主持人以同一逻辑主持身份在一个健康工作槽中直接使用普通业务工具；Harness 优先复用
   已有 Meeting channel Session，但物理槽和 ACP Session 不再是正确性或授权条件。Human 主持人
   直接使用现有业务界面；
4. 主持人确认 Board 记录的行动产出已经处理；
5. 一次带 `attestation=actions-recorded` 及精确 run/window/Board fence 的 End 原子关闭
   Meeting。

Meeting 不生成 Plan 或 Step，不代理 Project View 写入，也不检查外部业务状态的语义。

Agent Action Turn 必须从当前 canonical State 取得 frozen Board、主持身份与 run/window/Board
fence，并在同一 Turn 内完成业务写入、canonical readback 和显式 `COMPLETE`。Action lease 只表示
逻辑主持 Harness 仍在线工作；真正完成只由 Relay 接受的 `actions-recorded` ACK 决定。Retry、
Return-to-Board、Abort 和迟到结果继续受 current fence 与单执行 Turn 屏障约束。

现行后端设计与迁移方案见：
[Meeting Action Finalization 逻辑主持人 ACK 与同步简化实现设计](../fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)。

[Meeting V2 直接行动收口后端修正方案](../fix/meeting-v2-direct-action-finalization-backend-plan.md)
保留从旧 Plan/Step materializer 迁移到 direct action 的历史背景；其中要求 exact slot/session
affinity 的条款已被上述现行设计取代。

本文仅作为文件路径兼容和设计决策记录，不再是实现依据。
