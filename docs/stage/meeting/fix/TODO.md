# Meeting 后续 TODO

> 状态：已记录，暂不纳入当前 Meeting Agent 上下文优化交付
>
> 日期：2026-08-04
>
> 本轮新增的 Speech 历史修正边界：只立即补齐 Board Maintenance 的 canonical Speech
> 历史完整性 gate。

## 1. 统一 Speech 窗口元数据

### 当前情况

participant Intent、Granted Speech 和带 Candidate Cohort 的主持 Floor Turn 会注入
recent_shared_conversation_window，包含权威 revision、包含/省略数量、截断状态和
meeting_read history 提示。

Board Maintenance、无候选 Floor Decision 和 Action Finalization 当前虽然注入有界
recent_shared_conversation，但没有同样的窗口元数据。

### 后续目标

- 所有会读取 recent shared conversation 的 Meeting Turn 使用统一窗口元数据；
- 明确 authoritative revision、included/omitted count 和 is_truncated；
- 不改变各 Turn 的业务 Schema 或 deadline；
- 不把控制事件混入 canonical Speech。

### 当前决定

暂不处理。Board Maintenance 本次只保证生成其有界窗口所依据的本地 canonical Speech
projection 已连续覆盖 Relay 当前权威 speech revision。

## 2. meeting_read history 游标

### 当前情况

meeting_read history：

- 默认返回最近 100 条；
- 单次最多返回最近 500 条；
- 当前没有 before revision、before event ID 或 continuation cursor。

超过 500 条正式 Speech 的会议中，Agent 无法通过该工具继续读取更早内容，虽然 Relay 和 ACP
同步器仍可以保存完整历史。

### 后续目标

- 增加稳定复合游标，避免同秒事件产生分页缺口或重复；
- 支持从当前 prompt 窗口继续向更早 revision 分页；
- 返回明确的 next cursor、included range 和 remaining/truncated 信息；
- 保持 roster read authorization、只读工具边界和输出预算。

### 当前决定

暂不处理。当前交付继续保留默认 100、最大 500 的既有行为。

## 3. 单条超大 Speech 的 Prompt 预算

### 当前情况

Meeting Speech 合法正文上限可达到 256 KiB，而自动 recent shared conversation 预算约为
128 KiB。当前选择器从最新 Speech 向前累计；如果最新单条 Speech 自身已经超过预算，选择会
立即停止，可能导致自动注入窗口为空，也不会继续选择更早的小 Speech。

### 后续目标

需要单独决定并实现确定性策略，例如：

- 对超大单条 Speech 做 UTF-8 安全的 head/tail 截断；
- 明确提供 original bytes、truncated 和 revision；
- 决定截断后是否继续选择更早 Speech；
- 保证 Agent 能使用 history 工具读取原文或获得明确的不可读取提示；
- 避免工具输出自身超过 stdout budget。

### 当前决定

暂不处理。当前实现和既有 Speech/Prompt 大小上限保持不变。
