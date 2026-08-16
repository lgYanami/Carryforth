# Project Context Coordinate 语义检索结构过滤方案

> 状态：实现完成；确定性与 disposable real-pgvector 验收通过；待真实 Agent / Provider 验收
>
> 日期：2026-08-16
>
> 范围：全图 Coordinate 起点语义检索、Edge 成员 Coordinate 语义检索
>
> 关联文档：
> [Agent 自主上下文图检索 Skill 中文设计](../../agent-context-search/skill-prompt/skill-prompt.md)、
> [统一语义检索引擎 TODO](../%20unified-engine/TODO.md)、
> [统一语义检索引擎规范](../%20unified-engine/project-context-unified-semantic-retrieval-engine-spec.md)

## 1. 背景与结论

`coordinate-search` 当前把一段自然语言编码为一个向量，并在所有 eligible、current、位于 active Edge
上的 Coordinates 中执行 direct cosine top-K。真实验收表明：把完整问题、Role 环境、目标对象和后续
遍历要求都放进一个 query 会稀释起点定位信号；使用聚焦的目标职责 query 后，目标 Work 可以回到
rank 1。

这不是 fixed-point score 或 DESC 排序实现错误。它说明纯语义排序不适合同时承担：

- 理解 Agent 所处的上下文环境；
- 判断 Agent 当前真正需要哪类对象；
- 从所有 Coordinate 类型中完成候选召回；
- 替 Agent 选择最终起点。

Carryforth 已采用 Agent 自主渐进检索：Agent 确认自己的上下文环境，构造当前 hop 的聚焦 query，读取
候选的 canonical 轻量观察，再自行选择起点或下一 Coordinate。因此当前相关性修正收敛为：

1. 继续通过 `search-project-context` Skill 约束 query，不增加 target/context 融合权重；
2. 查询引擎提供确定性的 Coordinate 类型过滤；
3. Agent 只在明确知道所需对象类型时使用过滤；
4. 引擎不根据自然语言自动推断硬过滤，也不替 Agent 判断上下文相关性。

一句话目标：

> Agent 决定“当前环境下要找什么”，查询引擎可靠地限定“只在哪些 Coordinate 类型中找”。

## 2. 目标与非目标

### 2.1 目标

- 只查询 `work`、只查询 `document`，或查询多个明确类型；
- 不指定类型时继续查询全部 Coordinate；
- 过滤在 distance、top-K 和 K+1 truncation 之前完成；
- 相同 query、向量、候选和快照下，过滤不改变候选的基础相似度分数；
- 全图起点搜索和 Edge 成员 Coordinate 搜索使用同一 closed 类型集合；
- 结果明确绑定并回显实际应用的过滤范围；
- 不增加 Provider 调用，不修改 semantic source 或重建索引。

### 2.2 非目标

- 不新增 target/context 双向量、conditioned gain、融合权重或 neutral quota；
- 不新增服务端意图识别、LLM query rewrite 或自动类型分类；
- 不根据当前 Role 执行硬过滤；
- 不增加图邻域 anchor、路径 coherence 或 MMR；
- 不改变 `coordinate edge-search` 的 Edge / relation Document 排名；
- 不改变完整路径型 `semantic-query`；
- 不在本方案中增加 lifecycle、status、priority 或任意字段过滤 DSL；
- 不保证指定类型内的某个对象必然进入 top-K，也不把 score解释为置信概率。

## 3. Closed Coordinate 类型集合

结构过滤使用 canonical Coordinate 类型，而不是自由字符串。允许值为：

```text
project_profile
goal
role
plan
stage
requirement
issue
work
resource
document
meeting
```

前九项对应 `ProjectViewObjectType`；`document` 与 `meeting` 对应另外两个 canonical Coordinate family。

`document` 只表示作为图 Coordinate 的 Project Document。它不表示一条 Edge 绑定的 relation Document；
relation Documents 仍通过 `coordinate edge-search` 与 `edge documents` 观察。如果同一个 Document 同时
作为真实 Coordinate 出现在 active Edge 上，它可以被 `document` 过滤召回。

多个类型采用 OR 语义：

```text
work + issue = Work Coordinates ∪ Issue Coordinates
```

不提供 `all` 作为列表成员。省略整个过滤字段就是全量，避免 `all + work` 一类含混组合。

## 4. CLI 合同

所有返回 Coordinate 排名的 Agent-facing 语义操作使用同一个可重复参数：

```text
--coordinate-type <TYPE>
```

### 4.1 全图选择起点

只查询 Work：

```bash
cf project-context coordinate-search \
  --query "当前客户端重试实现责任" \
  --coordinate-type work \
  --limit 8
```

查询 Work 或 Issue：

```bash
cf project-context coordinate-search \
  --query "本次发布的回滚责任位置" \
  --coordinate-type work \
  --coordinate-type issue \
  --limit 8
```

不传 `--coordinate-type` 时保持当前全量行为。

### 4.2 从 Edge 选择下一 Coordinate

```bash
cf project-context edge coordinate-search <EDGE_KEY> \
  --query "下一步需要处理的工作" \
  --coordinate-type work \
  --limit 8
```

过滤只作用于该 Edge 的完整 canonical member set，不改变 Edge，也不返回或修改 relation Documents。

### 4.3 不适用的操作

- `coordinate edge-search` 返回 Edge，不接受 Coordinate 类型过滤；
- `edge coordinates` 返回完整 canonical member set，不能因过滤而隐藏成员；
- `coordinate show` 是精确读取，不接受候选过滤；
- `semantic-query` 保持现有独立 lifecycle、root 与 path 合同。

## 5. Request 与结果语义

逻辑请求增加可选 closed 字段：

```json
{
  "query": "本次发布的回滚责任位置",
  "coordinate_types": ["work", "issue"],
  "limit": 8
}
```

合同如下：

1. `coordinate_types` 省略表示全部类型；
2. 提供时必须非空，数量不能超过 closed 类型总数；
3. CLI 将重复 flag 规范化为排序、唯一的 canonical 列表；
4. 未知类型在本地解析阶段返回不可重试的 `user_error`；
5. 类型列表属于 request binding、结果签名和 snapshot observation；
6. 结果必须回显 canonical applied types，SDK 验证每个 Coordinate 均匹配；
7. `truncated=true` 只表示过滤后范围中存在第 K+1 个 eligible current 候选；
8. 候选 identity、rank、score 与 canonical observation 保持现有含义；filtered Edge member结果使用下述
   versioned coverage，不能篡改v1计数。

过滤条件是本地执行范围，不进入 Provider 输入文本。相同自然语言仍只产生一次 query encoding 和一次
Provider 调用。

### 5.1 Filtered Edge member coverage

现有one-hop `EdgeCoordinates` coverage按完整Edge成员定义：`scorable_coordinates + omissions`必须等于
完整成员数。过滤后不能把错误类型算作semantic omission，也不能用全量scorable数量验证过滤后的
`truncated`。

因此versioned filtered结果必须明确区分：

- `edge_coordinate_count`：完整canonical Edge成员数，永不被过滤改变；
- `type_matched_coordinate_count`：属于请求类型集合的完整成员数；
- `type_filtered_out_coordinates`：仅因类型不匹配而排除的成员数；
- `scorable_coordinates`：type-matched成员中具有current scorable semantic head的数量；
- `omitted_coordinates`：仅统计type-matched成员中的现有closed semantic omission原因；
- `title_only_scorable_coordinates`：type-matched scorable池中的title-only数量。

必须满足：

```text
type_matched_coordinate_count + type_filtered_out_coordinates
  = edge_coordinate_count

scorable_coordinates + omitted_coordinates.total
  = type_matched_coordinate_count
```

返回数量和`truncated`只对`scorable_coordinates`校验。未过滤v1继续使用原有coverage DTO；不能为了复用
filtered结果而改变v1字段含义。过滤类型覆盖全部成员类型时，新增的matched/scorable/omission统计必须
退化为v1同值，`type_filtered_out_coordinates=0`。

## 6. 查询执行语义

执行顺序必须是：

```text
host-derived Project / caller authorization
  → active generation 与 current semantic sources
  → closed Coordinate type scope
  → exact similarity
  → stable score / identity ordering
  → filtered K+1
  → result binding 与 release verification
```

类型过滤必须在 exact scorer 的候选 scope 中完成，不能先取全量 top-K 再丢弃错误类型。否则高分的
Meeting、Issue 或 Role 仍会消耗 Work 查询的 K 个名额，并产生错误的 `truncated`。

以下不变量保持不变：

- Host / Community / Project 边界；
- caller authorization、membership、ban 与 query gate；
- active Edge、eligible current source 与 generation/currentness；
- query-vector model space 与 exact score；
- fixed-point score、稳定 tie-break 和结果签名；
- Provider admission、deadline、并发与 release-time verification。

过滤只能缩小已有授权范围，不能扩大候选集合或绕过 currentness。

## 7. Agent 与 Skill 使用规则

CLI 交付后，`search-project-context` Skill 增加以下规则：

1. 明确要找 `Work`、`Issue`、`Document` 等对象时，使用对应 `--coordinate-type`；
2. 明确存在多个可能类型时，重复传入参数；
3. 不确定类型时省略过滤，不从候选文本或 score 猜测硬范围；
4. 不为每种类型机械重复执行一次查询；
5. 当前 Role 与相关环境继续放在聚焦 query 中，但 Role 不转换为类型过滤；
6. 已有明确且相关 Coordinate 时仍直接起步，不执行全图搜索；
7. 最终起点仍由 Agent 读取 lightweight canonical observation 后选择，不能直接采用 rank 1。

结构过滤解决的是候选范围，不替代 Agent 的上下文判断。

## 8. 兼容与发布边界

当前全图coordinate-search和one-hop search都是closed DTO，旧Relay会拒绝未知字段。其中
Edge→Coordinate还与Coordinate→incident Edge共享一个tagged one-hop family。因此过滤不能由新客户端
静默发送给旧Relay，也不能含混地扩展整个one-hop family。

兼容方案冻结为：

1. 全图检索未提供`--coordinate-type`时，CLI继续使用现有whole-graph v1 surface与原request bytes；
2. Edge member检索未提供过滤时，CLI继续使用现有one-hop v1 `EdgeCoordinates` variant与原request bytes；
3. 提供过滤时，两条操作分别检查自己明确广告的versioned filtered capability/result，不能用一个含混
   capability代表两个现有surface；
4. one-hop filtered字段只允许出现在versioned `EdgeCoordinates` variant；`IncidentEdges`没有该字段并
   必须拒绝错误variant组合；
5. 任一filtered surface不受支持时，在Provider前返回不可重试的unavailable / unsupported；
6. 两条v1路径都保留，不能通过静默改变其query bytes、scope、coverage或排序来实现过滤；
7. 两个filtered surface复用各自既有response-only Event kind，并冻结以下独立标识：
   - whole-graph extension：`carryforth_project_context_coordinate_search_v2`；
   - whole-graph capability：`carryforth-project-context-coordinate-search-v2-http`；
   - whole-graph result marker：`carryforth-project-context-coordinate-search-result-v2`；
   - one-hop extension：`carryforth_project_context_one_hop_semantic_search_v2`；
   - one-hop capability：`carryforth-project-context-one-hop-semantic-search-v2-http`；
   - one-hop result marker：`carryforth-project-context-one-hop-semantic-search-result-v2`。

在相同快照、query vector、limit且filtered types覆盖全部eligible Coordinate类型时，全图与Edge member
两条filtered surface都必须与各自v1产生相同的候选身份、score、顺序和truncation；Edge member新增
coverage还必须满足§5.1的退化关系。

## 9. 验收要求

### 9.1 确定性合同

- 单类型、组合类型、省略过滤、空列表和未知类型；
- canonical 类型顺序与去重；
- request/result binding 和 applied-types 回显；
- 返回错误类型 Coordinate 时 SDK fail closed；
- 无过滤 CLI 仍分别走whole-graph v1与one-hop v1；
- filtered CLI必须分别检查对应capability；
- filtered字段不能进入one-hop `IncidentEdges` variant；
- filtered Edge member coverage满足完整成员、type-matched、filtered-out、scorable与omission恒等式。

### 9.2 DB 与排名

- 全量 rank 1 为 Meeting、Work rank 2 时，`work` 查询必须返回该 Work 为 rank 1；
- `work + document` 只返回这两类，并保持各自 exact score；
- relation-only Document 不得被 `document` 过滤误当 Coordinate；
- 同一 Document 真实作为 Coordinate 时可以返回；
- inactive Edge、missing/stale head、错误 generation 和未授权 Project 继续排除；
- K+1 与 `truncated` 只基于过滤后的 eligible 集合；
- Edge member 搜索只在完整 Edge 成员中按相同类型规则过滤；
- 过滤前执行 top-K 的错误实现必须被回归测试捕获。

### 9.3 Agent / Live 验收

- 明确寻找 Work 时，Agent 使用 `--coordinate-type work`；
- 不确定类型时，Agent保持全量查询；
- 聚焦 query 与类型过滤组合后，可接受起点进入 Top 8；
- Agent仍通过 `coordinate show`筛选，不直接采用最高分；
- 不出现额外 Provider call、Context revision变化或 canonical readback失败；
- 不能把目标 rank 1 当成通用发布保证，只验证标注候选集合与 Agent选择结果。

## 10. 建议交付顺序

1. 冻结 closed Coordinate 类型、CLI 参数和 v1/filtered兼容合同；
2. 实现 pure request/result、canonicalization 与 SDK verifier；
3. 在共享 exact scorer scope 中实现全图与 Edge member过滤；
4. 接通 Relay capability、request binding与发布门；
5. 接通两个 CLI入口并更新Skill；
6. 完成确定性、real-pgvector与真实Agent验收；
7. 保持 v1 可回滚，资格通过后再决定是否改变默认Agent调用面。

## 11. 完成定义

本方案完成必须同时满足：

- Agent 可以对 Work、Document、任意 closed 类型组合或全量 Coordinates执行语义候选检索；
- 类型过滤在 top-K 前执行，结果可验证且不会扩大授权范围；
- 无过滤 v1 行为不变；
- filtered请求有独立可发现的兼容能力，不会随机落到旧Relay；
- Skill明确何时使用和何时省略结构过滤；
- 没有引入新的融合权重、上下文硬过滤、自动意图分类或图邻域相关性；
- 真实验收证明Agent仍负责最终上下文判断与渐进遍历。
