# Project Context 后续 TODO

> 状态：已记录，暂不纳入 Project Context Edge v1
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

### 当前阻塞：权限模型不一致

Project Context Edge v1 当前复用 Community-wide member visibility；Meeting 则由创建时冻结的
私有 roster 控制读取权限。直接加入 Meeting 坐标可能向非参会成员泄露：

- Meeting 的存在、ID、标题或参会关系；
- Meeting 与 Project View 对象之间的关系；
- Context Document 中源自私有会议的内容。

因此，增加 Meeting 坐标前必须先为 Context 查询建立逐坐标权限交集：调用者只有在能够读取
全部坐标和全部 Context Document 时，才能观察整条 Edge。授权失败时不得泄露 Edge key、计数、
其他坐标或 Meeting 是否存在。

### 建议的首版边界

- 仅允许 `closed` 或 `aborted` 的终态 Meeting 建立新的 Context 关联；
- 坐标身份只使用稳定、Project-scoped `meeting_id`；
- hydration 默认只返回调用者有权读取的轻量元数据，例如标题、终态、主持人与结束时间；
- 最终 Board 与正式 Speech 按需从 Meeting 领域读取，不自动注入 Agent Turn；
- Meeting 归档后坐标与既有 Edge 保留，并继续执行原有读取授权；
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

随后以物化出的项目对象作为 Edge 坐标，以该 Document 作为 Context Document。不得把私有
Speech 全文直接复制到 Community-wide Document；写入者必须先整理出适合目标可见范围的内容。

### 后续交付条件

1. 定义 Meeting Coordinate 的 wire、canonical ordering、Edge key variant byte 与 CLI token；
2. 定义 Meeting identity resolver、终态校验、归档和安全撤权后的生命周期语义；
3. 完成按全部坐标与 Context Document 求交的查询授权，覆盖 projection/filter 侧信道；
4. 为 CLI、ACP 和 Desktop 增加 metadata-first、Board/Speech-on-demand hydration；
5. 验证非 roster 成员无法观察 Meeting 坐标或包含它的 Edge；
6. 验证现有 Project View / Document 坐标、Context Revision 和三类集合查询语义不变。

### 当前决定

Project Context Edge v1 暂不加入 Meeting 坐标。近期使用 Project Document 承接经过整理的会议
结论和来源说明；完成对象级读取授权后，再单独设计并交付终态 Meeting Coordinate。
