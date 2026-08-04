# Meeting V2 阶段一：最小协议与数据设计

> 状态：已完成（2026-08-02）
>
> 产品语义基线：[Meeting V2](./meeting-v2.md)
>
> 阶段规划基线：[Meeting V2 后端分阶段实现计划](./meeting-v2-implementation-plan.md)

## 1. 阶段目标

阶段一只交付以下纵向链路：

```text
签名 Create
  → 原子创建私有 Session、固定名单和初始当前看板
  → 主持人或参会者按需查询当前看板
```

阶段一不交付 Board Update、Floor、speech、End、ACP Turn 或会议恢复。V2 Create 只能在
可丢弃的隔离测试环境开启；其他 V2 mutation 必须 fail closed。

## 2. 持久协议身份

- wire schema：`v=3`；
- floor policy：`moderated-board-v1`；
- 产品名称“Meeting V2”不用于推导 wire 数值；
- `v=1 + uniform-v0` 和 `v=2 + moderated-baton-v1` 的既有行为保持不变；
- Session 创建后，schema、policy 和 moderator 不可改变。

V2 Create 使用既有 kind `42100`，严格标签为：

```text
h       exactly one，非 nil Session UUID
name    exactly one，规范化后非空且不超过 255 个字符
v       exactly one，固定为 3
policy  exactly one，固定为 moderated-board-v1
about   zero or one
source  zero or one，且不能等于 Session UUID
p       1..11，表示创建者之外的固定参会者
```

V2 Create 不允许 `moderator` 标签。event author 自动成为 creator、Channel owner 和唯一
moderator；author 必须恰好一次出现在 Relay 形成的完整固定名单中。

## 3. 初始看板 envelope

Create event 的 content 是严格 JSON：

```json
{
  "format": "markdown",
  "body": "# 会议目标\n..."
}
```

约束：

- 只接受上述两个字段，拒绝未知字段；
- `format` 固定为 `markdown`；
- `body` 去除首尾空白后必须非空；
- `body` 按 UTF-8 字节计不超过 65,536 bytes；
- 拒绝 NUL 字符；
- 不要求目标、议程、Project View 或其他引用具有固定字段；
- 看板内容只是会议上下文，不产生外部效果。

Markdown 单文档让主持人可以自由组织目标、议程、进展、共识、未决问题、结论和可选引用，
不会在阶段一引入模板或流程模型。

## 4. 当前看板投影

Relay 在同一创建事务内生成并签名 kind `42110` 当前看板事件：

```text
kind       42110（relay-only）
h          Session UUID
v          3
policy     moderated-board-v1
format     markdown
moderator  创建者 pubkey
content    与严格初始看板 envelope 等价
```

该 kind：

- 客户端提交时必须被拒绝；
- 以 Meeting Session UUID 作为 `channel_id` 存储；
- 不写入 `meeting_event_outbox`；
- 不进入普通 live fan-out；
- 不携带 board revision；
- 通过现有 Nostr REQ/HTTP query 读取，不增加 Meeting 专用 HTTP endpoint。

阶段一每场 V2 Session 恰好存在一个当前看板投影。阶段二更新当前看板时仍只保留当前产品
投影；具体更新命令、幂等和 fencing 在阶段二设计中冻结。

## 5. 数据一致性

一个事务必须共同提交：

- 已验签的 Create command event；
- 私有 Meeting Channel；
- 固定 `channel_members`；
- `meeting_sessions` 的 V2 协议身份与 moderator；
- 冻结的 `meeting_participants`；
- 当前看板正文和 relay-signed 投影 event；
- 阶段一锁定的 V2 bootstrap runtime row；
- Create event 的既有 Meeting outbox 记录。

任一步失败都回滚整个事务，不能出现 active 但没有当前看板、名单或协议身份的 Session。
当前看板事件本身不进入 outbox。

## 6. 读取授权

当前看板沿用 Meeting 的现有 Channel 和 reader fence：

- 主持人可读；
- 固定普通参会者可读；
- 非参会者不可读；
- token 的 Channel 范围仍然生效；
- Community 移除、封禁、身份停用或 Meeting 安全撤权后不可读；
- 可选外部引用不可用不影响返回原始当前看板。

调用方只查询 kind `42110 + #h=<session>`，不管理 revision，也不订阅变更。

## 7. 灰度和 fail-closed 边界

- 新建开关：`BUZZ_MEETING_V2_CREATE_ENABLED`，默认 `false`；
- 该开关在阶段一只可用于可丢弃的隔离测试环境；
- V2 Floor Claim、Floor Signal、V1 Baton command、kind 9 speech 和 End 全部拒绝；
- ACP 不识别阶段一 bootstrap runtime，不启动 V0/V1 Turn；
- 测试创建的 active V2 Session 由 fixture teardown 清理，不跨测试保留；
- 阶段二完成前不得在共享或实际运行环境启用 Create。

## 8. CLI 面

创建：

```text
buzz meetings create \
  --policy moderated-board-v1 \
  --title <title> \
  --board <markdown-or-dash> \
  --participant <pubkey> ...
```

`--board -` 从 stdin 读取。V2 禁止 `--moderator`；V0/V1 禁止 `--board`。

读取：

```text
buzz meetings board get --meeting <uuid>
```

成功时输出规范化 JSON，至少包含 Meeting ID、format、body、moderator、投影 event ID 和
created-at。会议不存在、协议不是 V2、当前看板缺失和读取无权分别返回既有 CLI 错误类别。

## 9. 阶段验收

阶段一必须证明：

1. SDK fixture 精确锁定 V2 Create wire；
2. migration fresh、upgrade、schema snapshot 和 drift 检查通过；
3. V2 创建原子地形成 Session、名单、bootstrap row 和唯一当前看板；
4. creator = owner = moderator，且不能通过 wire 指定另一主持人；
5. 主持人与普通参会者可读取同一当前看板，非参会者被拒绝；
6. 当前看板事件没有 outbox row；
7. V2 mutation、speech、End 和 ACP 路径保持 fail closed；
8. V2 Create gate 默认关闭且严格解析；
9. V0/V1 builders、fixtures、Relay 路由和后端回归保持不变。
