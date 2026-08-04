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
3. Agent 主持人在同一 Meeting slot 和同一 ACP Session 中直接使用普通业务工具，Human
   主持人直接使用现有业务界面；
4. 主持人确认 Board 记录的行动产出已经处理；
5. 一次带 `attestation=actions-recorded` 及精确 run/window/Board fence 的 End 原子关闭
   Meeting。

Meeting 不生成 Plan 或 Step，不代理 Project View 写入，也不检查外部业务状态的语义。

现行后端设计与迁移方案见：
[Meeting V2 直接行动收口后端修正方案](../fix/meeting-v2-direct-action-finalization-backend-plan.md)。

本文仅作为文件路径兼容和设计决策记录，不再是实现依据。
