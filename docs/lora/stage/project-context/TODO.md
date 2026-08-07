# Project Context 后续 TODO

> 状态：方案已确认，待按 [Meeting Coordinate 实现设计](./meeting-coordinate-implementation-design.md) 交付
>
> 日期：2026-08-07

## 1. 将终态 Meeting 作为 Project Context 坐标

### 背景

Meeting 的正式 Speech、最终 Board、终态和 Action Finalization 记录具有长期上下文价值。
会议物化产生的 Goal、Plan、Stage、Requirement、Issue、Work、Role、Resource 和 Document
已经可以作为 Project Context 坐标，但当前无法直接表达“这些项目对象来源于哪场会议、为何在
该会议中共同形成”。

概念上建议未来增加稳定坐标：

```text
MeetingCoordinate {
  meeting_id
}
```

Meeting 只作为来源证据坐标。Edge 的解释性正文仍由普通 Project Document 承载，不把完整
Speech timeline 或 Board 正文复制进 Edge。

### 已确认权限模型

Meeting 与 Project View、Project Document 一样，是 Community / Project 范围的治理资产：

- 所有当前有效 Community member 均可发现和读取 Meeting；
- frozen roster 只控制正式参与、Floor、Speech、主持、Board 和 Action Finalization；
- 普通非 roster member 是只读 observer，不获得会议行动权限；owner/admin 仅保留既有 administrative
  Meeting End 应急治理能力，不因此获得主持、Speech、Board 或 Action Finalization 权限；
- Community 外、已移除、停用或封禁身份不能观察 Meeting 是否存在；
- 新 Meeting 只能绑定 Community-wide 可读 source，不能静默发布 private Channel / DM 内容；
- Meeting、Project View、Document 和 Context Edge 因此复用同一 Community read boundary，
  不需要为 Context 查询增加逐 roster 权限交集。

这是一次 Meeting 领域读取语义调整，而不只是 Project Context hydration 特判。REQ、COUNT、HTTP、
live fan-out、Meeting directory、Board/Speech point read 与 Desktop 都必须一起迁移；frozen roster 写入
校验保持不变。

### 建议的首版边界

- 仅允许 `closed` 或 `aborted` 的终态 Meeting 建立新的 Context 关联；
- 坐标身份只使用稳定、Project-scoped `meeting_id`；
- hydration 默认只返回有界轻量元数据，例如标题、终态、主持人、participant count / preview 与结束时间；
- 最终 Board 与正式 Speech 按需从 Meeting 领域读取，不自动注入 Agent Turn；
- Meeting 归档后坐标与既有 Edge 保留，并继续执行当前 Community membership 读取授权；
- 不把 Board revision、单条 Speech 或 Nostr event ID 设计成独立坐标；
- 不因 Action Finalization 或 Project View 物化自动推断、创建或扩展 Edge；
- Human 或 Agent 必须显式确认准确坐标集合并写回 Context Document。

### 当前可行的过渡方案

在 Meeting Coordinate 落地前，Action Finalization 可以显式创建或更新一份适合 Community
范围共享的 Project Document，记录：

- Meeting deep link；
- 讨论目标与最终结论；
- 关键分歧、采用方案及适用边界；
- 受影响或新产生的 Project View / Resource / Document 坐标。

随后以物化出的项目对象作为 Edge 坐标，以该 Document 作为 Context Document。Meeting Community-read
迁移完成前，不得把按旧合同产生的 legacy Speech 全文直接复制到 Community-wide Document；写入者必须
先整理出适合目标可见范围的内容。

### 后续交付条件

1. 定义 Meeting Coordinate 的 wire、canonical ordering、Edge key variant byte 与 CLI token；
2. 将 Meeting read authorization 迁移为 Community member，并保持 roster action authorization；
3. 将 Project Context Edge 升级到 schema/capability v2，非破坏迁移现有投影；
4. 定义 Meeting identity resolver、终态校验、归档和安全撤权后的生命周期语义；
5. 为 CLI、ACP 和 Desktop 增加 metadata-first、Board/Speech-on-demand hydration；
6. 验证普通非 roster Community member 可读但不可执行会议行动、owner/admin 应急 End 边界不扩大，
   非 member 无侧信道；
7. 验证现有 Project View / Document 坐标、Edge key、Context Revision 和三类集合查询语义不变。

### 当前决定

Project Context Edge v1 不原地扩展 closed union；下一版完整升级到 schema/capability v2，并加入终态
Meeting Coordinate。Meeting 使用追加 family rank `0x02`，既有 Project View / Document-only Edge key
保持不变。实现完成前继续使用 Project Document 承接经过整理的会议结论和来源说明。
