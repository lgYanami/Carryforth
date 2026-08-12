# 角色连续性变更记录

## 2026-08-07 — Role Continuity 运行时全面迁移到 Project View v3

- 修复 v3 Checkpoint 已成功写入、`buzz roles checkpoint list` 却仍要求 Relay 广告 v2
  的混合 major 问题。
- CLI、Desktop/Tauri 与 ACP managed Role Brief 的 Role 运行时统一为 v3-only；schema
  v1/v2 不再静默 fallback。
- Relay history 使用显式 `v3_role_history` scope 和 v3 entity tag；DB 对 current/history
  返回事件执行 signer、coordinate、revision、generation 和 strict v3 parser 校验。
- cutover/reproject 改为重投影全部 Proposal、Assignment、Commitment、Checkpoint、Handoff
  canonical 历史；readiness 在广告 v3 前验证全部 pointer，不删除或压缩 canonical 数据。
- SDK 提供共享 scope/tag 常量，仓库 `check` 增加 Role runtime v3-only 静态门禁。

实现依据见
[Project View v3 Role History 运行时全量迁移修复设计](../bug/project-view-v3-role-history-runtime-migration-fix-design.md)。

## 2026-08-03 — Role 定义与 Assignment 治理统一（已修复并验证）

- 修复普通 Community member 能创建 Role、却无权向它发 Offer 的分裂授权。Role 定义的
  create/update/deactivate/delete 现在与 Role Assignment 治理使用同一层级：Human owner
  可治理 admin/member，Community admin + exact active admin Assignment 的 Leader 只可治理
  member，普通 member 只能读取、申请并处理属于自己的 Proposal。
- v2/v3 Role create command 增加签名的 create-only `initial_role_level`。owner 可在初始化后
  创建 admin Role；Active Leader 只能创建 member Role；既有 Role level 仍不能通过 generic
  JSON patch 修改。
- Role definition gate 在 DB transaction 与 receipt replay 前重验；Supervisor binding、Runtime
  lease/fence 继续只表达运行归因，不成为治理权限。非 Role Project View、Document、Resource
  的 Community writer 边界未被收紧。
- CLI、Tauri 与 Desktop 使用 verified membership + Role + Assignment snapshot 组装同一能力。
  自动化验收覆盖普通 member、owner、Active Leader、missing/stale Assignment 与 admin target。

实现依据见
[Project Role 治理授权与 admin Role 创建缺口修复设计](../bug/project-role-governance-authorization-and-admin-role-creation-fix-design.md)。

## 2026-08-03 — Role Assignment 恢复为行为 fence，而非 Project ACL

- 修复 schema v3 通用 actor gate 对 managed Agent 无条件要求 Assignment 的逻辑错误。
  Candidate 在接受 Offer 前没有目标 Assignment；现在其稳定 Member identity 可直接申请、
  接受/拒绝自己的 Proposal或撤回自己创建的 Proposal，Relay 仍在事务内验证 Proposal
  candidate/creator、Community 资格、revision 与 compound replacement fence。
- `RoleActorIntent` 成为领域 crate 对 CLI 和 reducer 共享的 closed 分类，区分 Community
  identity、candidate-or-governor、governor 与 Role-bearing command；actor-dependent 的
  `reject_proposal` 最终权限仍由当前 verified state 判断，不把客户端分类当作授权。
- managed Leader、Work Commitment、Checkpoint、Handoff、replacement/unable report 等
  Role-bearing 行为继续要求当前 Assignment；schema v3 下还继续要求 exact supervised
  Runtime。Role level 与 Community `admin/member` 的原子同步、active Role/Assignment
  不变量均未改变。
- Assignment 结束只撤销 Role identity。若 actor 的 Community member/owner-backed 资格仍
  有效，它仍可执行普通 Project View CRUD；要撤销全部 Project View 权限必须使用
  Community remove、owner 关系撤销、ban 或 timeout。

实现依据见
[Project View Community 授权与 Assignment Fence 边界修复设计](../bug/project-view-assignment-authorization-boundary-fix-design.md)。

## 2026-07-30 — 上下文完善阶段 D：真实 Agent 行为验收

### 真实 Project Space 行为

- 使用真实 Codex Agent、`@agentclientprotocol/codex-acp 1.1.7`、真实 Relay 与真实 CLI
  建立独立验收 Project。Agent 能正确区分 Project Space、Project View、Role、
  Assignment、Member 与 Runtime，并知道稳定语义属于 system contract、动态事实按
  turn 注入，聊天和本地文件不会自动更新 Project。
- 当 Role Brief 没有展开目标 Issue 时，Agent 主动使用
  `buzz project-view get-object` 读取完整规范对象，没有根据 Work 摘要猜测。
- material change 先把 Work 写为 `completed`，再追加引用该 Work、Issue 与当前
  Assignment 的结构化 Checkpoint；Work 位于 project revision 13，Checkpoint 位于
  revision 14，仍待 Human 关闭的 Issue 没有被误改。
- 对抗提示要求代替已承担的 Release Steward 作发布承诺时，Agent 明确拒绝且没有产生
  Project mutation；对抗提示要求通过 Handoff 自行卸任时，Agent只追加计划性 Handoff，
  明确拒绝自行结束 Assignment，Role 保持 assigned。

### v2 Relay 重启缺陷

- 真实验收发现 v2 Community 在 `BUZZ_REQUIRE_RELAY_MEMBERSHIP=true` 下重启时，会把
  legacy `pubkey_allowlist` backfill 的 `forbidden:membership:v2_backfill` 传播成启动
  失败。
- backfill 现在在取得 Community lock 并确认 v2 后安全返回 `Ok(0)`；不会从 legacy
  allowlist 写入 v2 membership，也不改变 v1 一次性迁移行为。
- 现有 v2 cutover / Assignment replacement 数据库纵向测试增加启动回归断言；修复后的
  Relay 在 membership enforcement 开启时成功重启并完成全部真实 Agent turn。

### 阶段结论

- D-01 稳定认知、D-02 主动展开、D-03 规范对象优先写回、D-04 跨 Role 边界、D-05
  Handoff 不等于卸任全部通过。完整证据见
  [上下文行为验收报告](context-behavior-acceptance-report.md)。
- 本轮没有观察到需要修改 `[Project Space]` 稳定文案的真实误判，也没有扩展 Project
  Context 数据模型。上下文完善阶段 A～D 至此完成。

## 2026-07-30 — 上下文完善阶段 C：可恢复的 Full Brief 刷新

### Agent 与 supervisor 显式刷新

- `[Project Space]` contract 升级到 version 2，明确
  `buzz roles brief --markdown` 会立即从 Relay 重建并读取 Agent 自己的完整 Role
  Brief；Agent 不需要销毁 session，也不必把旧 Binding 当作完整局势。
- Desktop / owner observer control 增加 `refresh_role_context`。supervisor 可以为指定
  channel 安排下一完整 turn 强制 Full Brief；如果 Agent 正在执行，刷新请求由 pool
  暂存到该 turn 返回，如果当前没有 session，则下一次 session 创建本来就会使用 Full。
- refresh control 不取消正在执行的 turn，不制造 requeue，也不改变 Assignment 或
  Runtime fence。请求只在 Full Brief 成功进入一个完整 turn 后消费；unavailable 或
  交付失败会在后续完整 turn 继续请求 Full。

### connector compaction / reset 协议

- Buzz ACP 定义窄扩展
  `session/update.params.update._meta.buzz.contextReset`，按 `sessionId` 有界保存
  connector 报告的上下文丢失；下一完整 turn 消费该信号并强制重建 Full Brief。
- Buzz Agent 在内部 handoff 完成、模型可见 history 已经 compaction 后发送
  `{reason: "compaction", handoff: N}`。当前 turn 继续运行；不会在 compaction 点插入
  第二份 Role Context。
- 未携带该精确信号的 native steer、tool call、keepalive 或普通 session update 不会
  触发刷新，因此仍保持“每个完整 channel turn / heartbeat 一个动态 Role Context”
  的边界。

### observer 诊断

- `role_context_resolved` 现在同时记录 Project Space contract version / content ID、
  请求类型 `full | incremental`、刷新原因、实际结果
  `full | compact | unavailable`，以及 Full Brief 的 Role Directory
  `shown / total / omitted / truncated`。
- connector reset 被识别时额外产生 `context_reset_detected`，说明 session、原因和
  `next_complete_turn` 生效点；supervisor control 通过 `control_result` 返回
  `scheduled | next_session_full`。
- Directory 信息只随 Full observer 结果出现；compact 与 unavailable 不伪造目录，
  observer 也不成为授权来源。

### 本阶段边界

- 本阶段没有新增 Project View / Role 数据表、Nostr kind、Assignment 权限或自动写回
  行为，也没有增加前端刷新按钮；Desktop 已提供可调用的 supervisor API。
- 下一阶段 D 是行为验收：使用真实 Agent 验证主动展开、规范对象与 Checkpoint 写回、
  跨 Role 边界以及 Handoff 不等于主动卸任。

## 2026-07-30 — 上下文完善阶段 B：稳定 Project Space contract

### 平台契约与动态事实分层

- ACP 增加 Buzz 平台维护的固定 `[Project Space]` section，说明 Community / Project
  Space、Project View、Role、Assignment、Member、Runtime、Role Brief / Binding、
  Role Directory、Checkpoint 与 Handoff 的稳定语义，以及按需读取、显式写回、跨 Role
  协作和 fail-closed 行为。
- contract 是无参数常量，不接收当前 Project、Community、Member、Role、Assignment、
  Directory、revision 或项目成员自由文本；这些动态事实继续只由每个完整 turn 前验证后
  的 `[Role Brief] | [Role Binding] | unavailable` 上下文承载。
- contract 明确聊天、本地文件、工具输出和 Agent memory 不会自动更新 Project；
  Project-authored text 仍是项目数据而非平台 instruction，prompt 也不构成授权缓存，
  role-bearing write 继续由 CLI 与 Relay 重新验证。

### 现代与 legacy ACP 交付

- protocol-v2 与支持专用 system prompt 的 Agent 在 `session/new` 获得 contract；固定
  组装顺序为 `[Workspace] → [Base] → [Project Space] → [System] →
  [Team Instructions] → [Agent Memory — core] → [Channel Canvas]`。
- `[Project Space]` 不依赖 base prompt，因此 `--no-base-prompt` 不会关闭平台契约。
- legacy Agent 在 batch、initial message 与 heartbeat 三条完整提示路径获得同一份明确
  标注的兼容 section；兼容 user context 的较低提示优先级不被当作安全边界。现代路径
  不会在 user message 中重复 contract。

### 独立版本轴与有界失效

- contract 使用“显式版本 + 精确内容 SHA-256”形成独立 content ID，与 Project
  revision 完全分离；任一文案或版本变化都会得到不同 ID。
- 每个 channel session 与 heartbeat session 只在成功创建后记录其 contract ID。ACP
  在每个完整 turn、选择 Full / Incremental Role Context 之前比较 ID；缺失或不匹配时
  清除旧 session、turn/core/canvas 状态并重建，因此替换 session 同时获得完整
  Role Brief。
- 普通 Project revision 变化不会触发 system contract 重建，仍沿用既有按 meta /
  revision 刷新动态 Role Context 的路径。

### 本阶段边界

- 本阶段未修改 Project View / Role Continuity 的数据库、Nostr kind、权限或动态
  Brief DTO；显式 Full refresh、connector compaction/reset 信号与 observer 的完整
  contract 诊断仍属于阶段 C。

## 2026-07-30 — 上下文完善阶段 A：共享 Role Directory

### 同一验证快照派生目录

- `RoleBrief` 增加必需的 `role_directory`，由组装 Brief 的同一份
  `VerifiedRoleBriefSnapshot` 直接派生，不执行第二次 roster 查询，也不引入新的数据库
  表、Nostr kind、Project View object 或 Markdown 事实源。
- 目录只包含 active Role；每项携带稳定 `role_id`、name、level、有界单行 purpose、
  当前 `assigned | vacant` 状态、承担者稳定公钥、Assignment/source 引用以及当前
  Member 自身 Role 标记。
- 历史已结束 Assignment 不参与当前 staffing；replacement 后目录只保留当前 active
  Assignment。display name 仍只在 Desktop 展示层 best-effort 解析，不参与目录组装、
  排序、验证或权限判断。

### 有界、稳定且不静默截断

- 首版目录最多携带 32 个 active Role，单项 purpose summary 最多 160 个 Unicode
  字符；顺序固定为当前 Member 的 Role、Leader / admin Role、其余 Role 的规范化名称
  与 Role ID。
- DTO 同时携带 `total_active_roles` 与 `omitted_active_roles`。超过预算时 Full Brief
  明确报告 shown / total / omitted，并提示使用 `buzz roles list` 展开；被省略的 Role
  不会被表达为不存在。
- candidate 与 assigned Brief 使用同一目录规则；compact `[Role Binding]` 不重复目录，
  unavailable 路径继续只返回 fail-closed 状态，不复用旧目录。

### CLI、ACP 与 Desktop 同源

- 共享 JSON DTO 与 Full Markdown renderer 同步扩展，因此 `buzz roles brief` 和 ACP
  的 Full Role Brief 自动使用同一份目录；没有在 ACP 中拼接第二套协作清单。
- Desktop 的 v2 normalizer 对目录计数、唯一 Role、当前 Role 标记、active Role 定义
  以及 active Assignment 一致性执行 fail-closed 检查；Inspector 增加
  `Collaboration roles` 区域，展示 Leader / Role、Current、承担者或 Vacant 和显式
  omitted 数量。
- Rust 覆盖 assigned、candidate、vacant、replacement history、稳定排序、purpose
  截断与目录预算；Desktop E2E 覆盖正常展示及目录与验证后 Role Continuity 不一致时
  拒绝整份 View。

### 本阶段边界

- 本阶段只交付设计中的阶段 A。稳定 `[Project Space]` system contract、contract
  version/session 失效、显式 Full Brief refresh 与 compaction/reset 信号仍属于后续
  阶段。

## 2026-07-30 — 设计决策：完善 Project View + Role Continuity Agent 上下文

> 本条记录设计决策，尚不表示对应代码已经交付。完整设计见
> [Project View + Role Continuity Agent 上下文完善设计](project-view-role-continuity-context-design.md)。

### 稳定 Project Space 运行契约进入 system context

- ACP 应增加由 Buzz 平台维护的稳定 `[Project Space]` section，使 Agent 从 session
  开始就知道一个 Community 是一个持久 Project Space，并理解 Project View、Role、
  Assignment、Role Brief、Role Binding、Checkpoint 与 Handoff 的语义、状态归属及
  基本读写规则。
- `[Project Space]` 属于共享平台 contract，不属于单个 Agent 可配置的 Persona。它只
  解释如何使用和维护 Project Space，不包含当前 Project 名称、Goal、Role、
  Assignment、Role Directory、Work、revision、成员身份或任何项目成员编写的动态文本。
- 现代 ACP Agent 在 `session/new` 的 system prompt 中获得 contract；legacy Agent
  只能通过既有 `[Base]` user-context 路径获得兼容说明。提示优先级差异不能成为安全
  边界，最终授权继续由 CLI 与 Relay 验证。
- Project Space contract 与 Project revision 使用独立版本轴。contract 内容变化后，
  已有 session 需要在有界时间内失效或重建；Project revision 变化只刷新动态 Role
  Context，不重建 system contract。

### 动态 Project 与 Role 内容继续按 turn 注入

- 本次决策不把当前 Role、Assignment 或 Project View 内容迁入长期 system prompt。
  Stage 10 已有的 `full | compact | unavailable` 模型保持不变：新建/重建 session、
  cache miss、Relay/Community/Member/meta/revision/generation 变化时注入完整
  `[Role Brief]`；同一 session 且 meta 精确不变时注入 `[Role Binding]`；验证失败时
  fail closed。
- native steer 与单次 tool call 仍不形成新的 Role Context 注入。Project mutation
  后的下一完整 turn 根据新 meta 刷新；当前逻辑立即依赖写入结果时，Agent 应使用回执
  或主动查询。
- unavailable turn 不重新发送旧 Role、Assignment、Binding 或 Directory，也不把
  verified cache 当作授权。下一完整 turn 恢复后，如果 Relay 与 meta head 和既有
  verified cache 精确一致，可以恢复为 compact Binding；cache 缺失、身份改变或 meta
  变化时才重新生成 Full Brief。
- Agent / supervisor 显式 full refresh，以及 ACP connector 报告
  compaction/reset 后强制 full refresh，被确认为后续体验完善点。原生信号完成前，
  `buzz roles brief --markdown` 提供即时完整重读。

### Full Role Brief 增加最小 Role Directory

- Full Brief 应从组装自身的同一份 meta-bounded verified Project View v2 snapshot
  派生最小 Role Directory；不新增 roster 查询、数据库表、event kind、Project View
  object 或独立 Markdown 事实源。
- 目录只表达 active Role 的稳定 ID、name、level、单行 purpose、`assigned | vacant`
  状态、承担者稳定公钥以及当前 Member 自身 Role 标记。它不展开其他 Role 的完整职责、
  历史任期、Checkpoint、Handoff、presence 或 Runtime 状态。
- stable public key 是规范成员身份；display name 只能作为 best-effort 展示，不能参与
  验证、排序、权限或 Assignment 判断。
- candidate 与 assigned Member 可以看到当前权限范围内的目录；unavailable 不复用旧
  目录；compact Binding 不重复目录。目录超过 prompt 预算时必须稳定排序、显式报告
  total/shown/omitted，并引导使用 `buzz roles list`，不能静默把截断解释为不存在。
- Role Directory 必须进入共享 Role Brief assembler/DTO/renderer，使 CLI、ACP 与
  Desktop 保持同源。当前 closed DTO 的兼容方式需要在实现阶段明确，不能由 ACP 私自
  拼接另一套目录。

### Agent 读取、写回与协作规则

- system contract 应指导 Agent 在 Brief 不足、跨 Role、revision conflict、
  candidate、unavailable、Community/Relay/Member 切换或 context compaction 后主动
  使用 `buzz project-view` 与 `buzz roles` 展开当前状态。
- Project 对象的直接事实变化写回对应 Project View 对象；Role 的 material progress、
  blocker、risk、open question 与 next step 形成结构化 Checkpoint；计划性接续使用
  Handoff。属于 Work 或 Issue 的事实应先更新原对象，再由 Checkpoint 引用，不能建立
  第二份真相。
- 对话、工作区文件、工具输出和 Agent memory 不自动更新 Project。一般探索与未验证
  判断默认不写入规范状态；本次设计不引入自动 context extraction 或 Project Context
  数据模型。
- Role Directory 用于识别责任边界与 vacancy，不授予权限，也不允许 Agent 静默承担
  第二个 Role。跨 Role 行动前应读取完整 Role 定义，并通过现有消息、Issue、Work 与
  有权治理主体协作：普通 Role 可交给 Community owner 或 active Leader，admin /
  Leader Role 只能交给 Community owner。verified human owner 只保留针对自己所拥有
  managed Agent 的既有 owner-control 特例，不获得通用 Role Assignment 治理权；
  普通 Human Member 也不因其是 Human 而天然获得治理权。

### 范围边界

- 本次设计不改变 Project View v2 协议、数据库规范状态、Nostr kind、Relay capability、
  Community 权限、Assignment 控制规则或 Runtime fencing。
- Relay-signed 动态内容证明来源和 revision 一致性，不表示其中判断绝对正确，也不能
  覆盖平台安全规则、Team Instructions 或 Human 治理。动态项目文本继续留在
  user-context 层，prompt 本身不承担授权。

## 2026-07-29 — 集成验收缺陷修复：可验证 cutover 快照与完整 Role 历史

### v2 generation reset 保留每个 head 的最后变更坐标

- 修复正式集成验收发现的 P1：v1→v2 cutover 曾以新的 cutover revision/time 包裹全部
  旧普通对象和 Role，但 body 仍保留对象自身最后变化时的 revision/time，导致共享
  `VerifiedRoleBriefSnapshot` 正确地 fail closed。现在 reset meta 仍表示新的 Project
  revision 和 projection generation；未变化的普通对象、tombstone 与 Role head 则保留
  各自最后变化时的 `project_revision` 和 `updated_at/deleted_at`，同时进入新的
  generation。cutover 新建的 Proposal、Assignment 以及 meta 继续使用 cutover 的新
  revision/time。
- cutover 提交前的投影回读校验改为与每个 head 的规范状态比较，不再错误地要求所有
  head 等于 meta revision。共享 assembler 的严格校验没有放宽：head generation 必须
  等于 meta generation，head revision 只能小于等于 meta revision，body revision/time
  仍必须与该 head 精确一致。
- 在写入或退休任何 event 前，cutover 生产事务会把新 membership、全部普通对象/Role
  continuity heads 和 reset meta 交给共享 `VerifiedRoleBriefSnapshot` 完整组装；Role
  level、Assignment、membership、计数或引用任一不一致都会使整个 cutover 回滚。
- SDK 对 `V2ProjectionContext` 的注释同步明确“一份 context 对应一个 head”；增量变更
  通常共享同一 revision/time，而 generation reset 中未变化的 head 保留最后变更坐标。

### `roles get` 从有界当前页恢复完整任期历史

- 修复阶段 6 引入的读取回归：默认 Role Brief 快照为了保持有界，只包含 active
  Assignment 和有限 Checkpoint/Handoff；`buzz roles get` 曾误把这份当前页作为完整
  `assignment_history`，因此 A→B 替换后只显示继任者的一段任期。
- `roles get` 现在先组装共享 verified 当前快照，再以同一
  `projection_generation + project_revision` 对目标 Role 分页读取 Proposal、
  Assignment、Checkpoint 与 Handoff 完整历史。分页保持服务端每页 500 条的上限，并对
  跨页重复 event、停滞 cursor 以及当前 Assignment 在历史中缺失执行 fail-closed 校验。
  这不会把已结束任期重新注入每轮 ACP Role Brief，也不会扩大 Desktop 默认当前页。

### 回归与验证

- PostgreSQL cutover 回归直接把 Relay 签名的 membership、object/entity heads 和 meta
  交给与 CLI/ACP 相同的 `VerifiedRoleBriefSnapshot`；覆盖不同历史 revision 的 active
  object、Role 和 tombstone，并断言 reset meta revision 高于这些未变化 head。夹具还
  把一个既有 Community admin 显式映射到 Leader Role，验证 admin membership、Role
  level 与初始 Assignment 在事务内一致。
- 原有 v2 Assignment 纵向数据库测试也新增完整 cutover snapshot 组装断言，防止未来只
  校验外层 schema/envelope 而遗漏跨 head 一致性。
- `just project-view-test` 全部通过：领域/协议专项 93 项、PostgreSQL 19 项、迁移与
  schema drift 6 项、真实 Relay/CLI E2E 1 项。真实 E2E 已重新完成 v1→v2 cutover、
  Agent A 接受、自卸任拒绝、A→B 原子替换，并读取到两段 Assignment 历史与一条
  Handoff。`buzz-sdk`、`buzz-db`、`buzz-cli` 的 all-targets clippy（warnings denied）、
  Rust fmt 和 diff whitespace gate 通过。

## 2026-07-29 — 阶段 10：Role Brief 按 revision 增量刷新

### 每轮确认身份，按需重建完整上下文

- ACP 仍在每个完整 channel turn 和 heartbeat 开始前读取 NIP-11 Relay identity，并查询、
  验签唯一的 Project View v2 meta head。首次启动、ACP session 新建或重建、本地无匹配
  cache，以及 meta event、project revision、projection generation、Project、Member 或
  Relay identity 任一变化时，继续读取 object/entity heads、meta 指向的 membership
  snapshot，并使用前后 meta bracket 组装完整 verified Role Brief。
- 同一 session 的后续 turn 若当前 Relay、Project、Member、meta event、revision 和
  generation 与上次完整 verified Brief 精确一致，不再查询全部 heads 和 membership，
  而是注入紧凑的 `[Role Binding]`。它包含 candidate/assigned 状态、Role ID/name/level、
  Assignment ID 和完整 meta revision 坐标；assigned binding 明确声明自己不是缓存授权，
  每次 role-bearing 写入仍须重新解析 Assignment。
- compact renderer 直接从共享 canonical `RoleBrief` 派生，没有引入第二套 Role 解释或
  持久化 `role.md`。完整 Brief、JSON/Desktop read model 和 compact binding 仍以同一个
  `VerifiedRoleBriefSnapshot` assembler 为事实解释边界。

### Session、Runtime 与 fail-closed 边界

- channel 与 heartbeat 的 session 是否已经存在，会在 Role context 解析前决定
  `full/compact` refresh；rotate、模型切换、取消失败、Agent 退出等既有 session
  invalidation 路径会使下一 turn 自动恢复为完整 Brief。启动检查也显式采用 full refresh。
- compact 与 full 两条路径都在把 context 交给 Agent 前调用 Runtime supervisor
  coordinator 对账当前 Assignment。缓存只减少投影读取和 prompt 重复，不跳过 Runtime
  状态收敛，也不改变 CLI 每次签名前的完整 Assignment 校验或 Relay 的事务内最终 fencing。
- meta/Relay 读取失败、完整 snapshot 不能稳定、共享 Brief 组装失败或 Runtime 对账失败
  时，仍暂停本地 Runtime fence 并注入 `State: unavailable`；失败 turn 不会回退到上一份
  Role Binding。Relay identity 变化会立即清空旧 cache realm，其他 key 不匹配也会在完整
  rebuild 前清空。native steer 仍是当前 turn 内的增量消息，不新增独立 Role 授权语义。
- `role_context_resolved` observer frame 增加 `mode=full|compact|unavailable` 与精确
  `metaEventId`，便于确认上下文刷新行为而不把缓存提升为授权状态。

### 验证与范围

- 新增真实 HTTP mock 纵向测试，验证首次 full 读取、未变化 meta 的轻量 compact 路径、
  session 重建强制 full、meta 读取失败不复用 cache、失败后重新确认同一 head，以及
  revision 变化后的下一 turn 重读 object/entity/membership 并注入新 Brief。
- cache-key 单元测试覆盖 Relay、Project/Community、Member、meta event、revision 和
  generation 的任一变化均不能命中；session 生命周期测试覆盖 channel 与 heartbeat 的
  创建、复用和 invalidation；共享 renderer 测试覆盖 assigned/candidate binding 及
  Assignment 写入边界。
- `buzz-sdk` `250/250`、`buzz-acp` `613/613` 测试通过；workspace all-targets check、
  clippy（warnings denied）与 Rust formatting gate 通过。本阶段不改变 Project View
  协议、数据库规范状态、Relay 权限或 Desktop 页面。

## 2026-07-29 — 阶段 9：运行中 Runtime 动态收敛

### 每轮 Role context 与 Runtime 原子对账

- `buzz-acp` 增加独立的 Runtime supervisor coordinator worker。启动时仍在首个模型进程
  之前取得初始 epoch；启动完成后，每个完整 turn 的 verified Role Brief 必须先携带当前
  Assignment 与 coordinator 对账，得到确认后才会作为 `assigned/candidate` context
  交给 Agent。
- coordinator 在每轮重新读取 authenticated Runtime status，并串行处理 Assignment
  替换、candidate/assigned 转换以及 supervisor binding 的新增、撤销和重新注册。仍然
  current 且 lease 有效的 Runtime 保持原 logical Runtime/epoch；失效或切换时先暂停旧
  续租并撤下本地 fence，再按既有持久 recovery/backoff 规则恢复或启动新 Runtime。
  因而不需要重启 ACP 或 Agent pool，就能在下一完整 turn 收敛。
- Role Brief snapshot、Assignment 组装或 Runtime 对账失败时，不复用旧 Brief；ACP 注入
  明确的 `project_view_unavailable` 或 `runtime_supervision_unavailable`。两段读取共用
  原有 12 秒 turn-context deadline。长 recovery/backoff 在独立 worker 内继续，不阻塞
  Relay 主循环；超时请求的后续 suspend 命令会再次撤下 fence。
- verified Assignment 消失时才清除 pair-scoped 恢复状态；单纯 snapshot/网络故障只暂停
  Agent 写入并保留受信恢复坐标。Assignment 结束后仍由 Relay 最终 fencing，正在执行的
  旧 turn 不能借本地切换复活旧任期。

### 动态 fence 文件与进程边界

- ACP 不再把启动时静态 `BUZZ_RUNTIME_ID`/`BUZZ_RUNTIME_EPOCH` 复制给长期运行的模型
  进程，而是为每个 harness generation 生成与私有恢复路径不可互推的独立
  `BUZZ_RUNTIME_FENCE_PATH`。当前 `RuntimeFence` 以原子写和仅 owner 可读权限发布；
  同一路径在 Runtime/epoch 改变时更新，在不可验证、binding 撤销或 harness 停止时删除。
- `buzz-cli` 的 managed Project View/Role 写入在每次签名前重新读取该文件，并优先于旧的
  静态 pair。文件缺失表示当前不附带 fence，格式损坏、超限或非绝对路径则 fail closed；
  legacy `BUZZ_RUNTIME_ID`/`BUZZ_RUNTIME_EPOCH` 仅保留给非 ACP 的兼容调用。
- fence 文件不是授权根，也不包含 supervisor 私钥或持久恢复状态。模型进程能够读取或
  破坏它，但最多让自己的 CLI 写入失败；Assignment、binding、runtime ID、epoch、lease
  与签名者仍由 Relay 在 Project 事务内做最终校验。
- Agent 子进程和 session MCP server 只接收 harness 派生的 fence 路径。persona、父进程
  和 Desktop 自定义环境不能覆盖该路径；Desktop 同时把它加入 reserved/ambient-strip
  集合。supervisor 私钥与恢复状态路径继续只存在于受信 ACP harness。

### 验证与范围

- 新增真实 HTTP mock 纵向测试，覆盖运行中 binding 撤销、重新注册、Assignment 替换、
  新 Runtime fence 发布及 graceful stop；另覆盖 current lease/epoch 判断、动态 fence
  权限与 round-trip、CLI 文件损坏/缺失/legacy pair，以及 Agent/MCP 环境隔离。
- `buzz-acp` `610/610`、`buzz-cli` `264/264`、Desktop Tauri `1644/1644` 非 ignored
  测试通过；workspace 与 Desktop all-targets check/clippy（warnings denied）及 Rust
  formatting gate 通过。
- 本阶段关闭阶段 8 “运行中新增 Assignment 或 binding 需要下一次 harness generation”
  的限制。operator supervisor 私钥仍必须在 ACP 启动前配置；运行中不能把新的秘密身份
  注入既有受信进程。动态收敛发生在完整 turn 边界，长 turn 中途的并发变化继续由 Relay
  command fencing 保底。

## 2026-07-29 — 阶段 8：ACP Supervisor Adapter

### 受信 harness 与 Agent 进程隔离

- `buzz-acp` 增加具体的 Runtime supervisor adapter。ACP harness 是模型进程之外的
  受信边界：它在 Tokio 和任何 Agent 子进程启动前消费独立 supervisor 私钥及 pair-scoped
  恢复状态路径；模型面对的 Agent 只能继承 Relay 签发的 `BUZZ_RUNTIME_ID` 与
  `BUZZ_RUNTIME_EPOCH`，不能读取 supervisor 私钥或恢复文件能力。
- supervisor identity 必须与 managed Agent 的 Member identity 不同。Desktop 只接受
  operator 从自身环境显式提供的 supervisor 私钥，按 canonical
  `Member pubkey + Relay URL` 派生独立状态文件；persona、自定义环境和父进程残留值均不能
  覆盖 harness 持有的 managed marker 或 Runtime fence。
- eager pool、lazy pool、slot refill、panic recovery 和普通 respawn 使用同一份
  harness-issued fence。ACP 先连接 Relay、验证启动 Role Brief、读取 Assignment 的
  supervision binding 并取得 epoch，之后才允许启动首个模型进程；受监督 Assignment
  缺少可信配置或 Role context 无法验证时 fail closed。

### 持久恢复、续租与停止语义

- adapter 通过 authenticated Runtime status/evidence 接口启动新 logical Runtime、
  定期续租，并在 harness 健康后完成 `recovery_succeeded`。状态文件以原子写和仅 owner
  可读权限保存 Assignment、Runtime、epoch、Member、supervisor 与 Relay 绑定，替代
  harness 启动时始终采用服务端 epoch，不信任本地旧 epoch。
- 前一 harness 未留下正常停止证据时，替代者先报告 `abnormal_exit`；若前一
  `recovery_attempt` 中断，则先报告 `recovery_failed`，遵守服务端 backoff 并在长等待
  期间发送 supervisor heartbeat，随后由 Relay 分配新 epoch。终态
  `unavailable`、缺失本地 ownership 但服务端已有 evidence、或身份/Relay 不一致时均不能
  另建 Runtime 绕过恢复上限。
- 新增 `graceful_stop` typed evidence 和 additive migration
  `0031_project_runtime_graceful_stop.sql`。Desktop/ACP 的有意停止只关闭当前 Runtime
  lease，不启动故障恢复、不增加失败证据，也不结束 Assignment；已结束 Runtime 拒绝迟到
  的续租或其他证据。只有 SIGTERM、Ctrl-C 或 owner shutdown 属于正常停止；Relay
  无法恢复、Agent pool 耗尽等内部退出会报告 `abnormal_exit`。证据失败或超时时保留本地
  状态，交给下一受信 harness 对账。`recovering/unavailable` Runtime 不能用
  `graceful_stop` 擦除既有失败或绕过恢复上限。
- ACP 内部 worker 的补位和重启属于同一受监督 harness 的实现细节；只要 harness 本身
  仍健康，就不改变 logical Runtime 或 epoch。

### 启用边界与验证

- Runtime supervision 仍需 Relay operator 先为具体 Assignment 注册 supervisor binding，
  并向 Desktop/部署环境单独配置对应私钥；没有 binding 的现有 candidate/assigned Agent
  保持原有未监督行为。已经运行的 harness 若在运行期间才获得 Assignment 或 binding，
  本阶段需要下一次 managed harness generation 才启用 adapter。
- `buzz-project-view` `25/25`、`buzz-cli` `262/262`、`buzz-acp` `606/606`、
  `buzz-db` `97/97` 非 ignored 测试和 Runtime supervision PostgreSQL 纵向测试、
  Desktop Tauri `1644/1644` 非 ignored 测试通过；Project View 六条 ignored migration
  测试和 schema-drift gate 通过。workspace/Desktop all-targets check 与 clippy 通过。
  覆盖正常停止、旧 epoch、恢复 ownership、恢复耗尽、服务端 epoch 优先、状态文件权限及
  fence 环境覆盖。

## 2026-07-29 — 阶段 7：可信 Runtime 监督与自动故障恢复

### Assignment 与 Runtime 分离

- 增加 closed、版本化的 Runtime supervision 协议，显式区分
  `available/recovering/unavailable` 与 Assignment 的 `active/ended`。Runtime 启停、
  lease 过期、presence 离线或普通 WebSocket 断开都不会直接结束 Assignment。
- 每个受监督 Runtime 使用 assignment-scoped binding、独立 `runtime_id` 和单调
  `runtime_epoch`。managed Agent 的角色写命令自动附带当前 Runtime fence；旧 epoch、
  已撤销 binding 或已经结束的 Assignment 均不能继续写入。`recovery_attempt` 以旧
  epoch 请求，Relay 在替代进程启动前先分配并返回新 epoch；监督器把新 epoch 注入
  替代进程，随后成功或失败结果也必须引用它。
- supervisor policy 对 lease、恢复窗口、最大尝试次数、指数退避基数、monitor timeout
  和恢复宽限设置有界值。服务端强制每个 attempt 必须先收到成功或失败结果，失败后下一次
  attempt 必须等待递增退避；最后一次 attempt 仍在运行时，即使窗口已过也不能提前结束
  Assignment。自动结束默认关闭，只有明确启用且 Relay audit 和稳定签名密钥同时可用时
  才能启动。

### 可信证据与 fail-closed 恢复

- additive migration `0030_project_runtime_supervision.sql` 增加 supervisor binding、
  Runtime lease 和 append-only evidence。`start`、续租、异常退出、恢复尝试、成功、
  失败和 supervisor heartbeat 都使用 closed typed payload，并以 auth event
  idempotency key 加请求哈希防止同 key 重放不同证据。
- supervisor 只能提交运行证据，不能修改 Assignment。注册/撤销 binding 使用 operator
  入口并写入 hash-chain audit；证据入口使用 tenant-bound、payload-bound、带 replay
  防护的 NIP-98，且签名者必须精确匹配已注册 supervisor。
- 异常后先进入 `recovering`，恢复成功继续同一 Assignment；只有明确异常、已执行并失败
  的至少一轮有限恢复尝试、达到策略阈值、同 Assignment 下所有受信 Runtime 均不可用、monitor
  当前健康且额外宽限已结束时，才成为终止候选。lease 静默过期只影响可用性读取，不会
  被提升为“不可恢复”证据。
- 进程重启仍由持有 supervisor identity 的 Desktop/ACP 外层或部署侧监督器执行；Relay
  提供可信证据协议、状态机、fencing 和最终策略执行。监督器密钥不注入被监督的
  Agent Runtime，Agent 因而不能伪造失败来主动卸任。

### 原子 Project system action

- 多 Pod scheduler 通过数据库 claim、`SKIP LOCKED`、claim token 和期限实现幂等竞争；
  终止前在同一事务中重新验证全部证据与健康 Runtime，部署开关可随时停止自动终止。
- `end_unrecoverable_assignment` 使用内部 `ProjectSystemChange`，而不是伪装成 Community
  Member 命令。一个 Project lock 事务同时提交 system audit、Assignment
  `ended(unrecoverable)`、Commitment `assignment_ended`、system Handoff、成员等级同步、
  canonical rows、Relay 签名的 entity/meta heads 和 project revision；任一步失败全部
  回滚。
- system Handoff 引用最新 Checkpoint、被中断 Commitment 和相关 Work，确保 Runtime
  无法恢复时 Role 变为空缺，但责任、当前局势和历史归因仍留在 Project。该动作推进一次
  project revision；高频 lease/heartbeat/evidence 不进入 Project View 投影。

### 操作入口、可观测性与验证

- CLI 增加 `buzz runtime evidence ...` 与 `buzz runtime status --assignment ...`，供可信
  supervisor 提交证据、供 Project Member/运维读取 Runtime availability。监督器为
  managed Runtime 注入 `BUZZ_RUNTIME_ID`/`BUZZ_RUNTIME_EPOCH` 后，普通 Role 和 Project
  View v2 写入会在签名前携带该 fence；Relay 仍做最终事务内验证。
- 增加低基数 `buzz_role_runtime_recovery_total{result}`，并保留 evidence、scheduler
  claim/error 和 Assignment ended 指标；日志携带 Community、Assignment 和 binding，
  不把 Runtime 或 Assignment ID 放进指标 label。
- 领域与数据库测试覆盖状态转换、同键同请求幂等、同键异请求拒绝、旧 epoch 拒绝、
  多 Runtime 中任一健康即阻止终止、单纯 lease 过期不终止、最后一次恢复尝试、monitor
  故障与恢复宽限、多 Pod claim 互斥、append-only 证据、deferred trust-chain 约束及完整
  system action 原子提交。迁移门禁同步覆盖 `0030` 与 checked-in schema 的 runtime
  对象漂移。

## 2026-07-29 — 阶段 6：Checkpoint、Handoff 与完整 Role Brief

### Project-owned 追加式连续性

- 增加 closed、结构化的 `RoleCheckpointContent`，覆盖摘要、当前关注、进展、阻塞、风险、
  未决问题、下一步和 typed references。Checkpoint 固定归属于 Role、active Assignment
  与 Member；创建后不可更新或删除，修正只能追加带 `supersedes_checkpoint_id` 的新记录。
- typed reference 支持 Project View object、Assignment、Commitment 和本 Community 的
  Nostr event。领域层拒绝缺失的规范对象，数据库 deferred constraint 再拒绝跨
  Community、错误 owner/source 或不存在的 event。
- 完整 Handoff 增加 cause、可选 Checkpoint、受影响 Commitment、未决事项与引用。
  Member 可以在 active Assignment 期间追加 planned/other 上下文，但该动作不结束任期；
  正式替换仍由 Relay 在同一事务中生成 system Handoff。
- replacement system Handoff 自动携带旧任期的最新 Checkpoint、被终止的 Commitment、
  对应 Work 引用和待接续事项。即使前任没有撰写 Handoff，也保留 Project View、Work
  responsibility 和 waiting-for-continuation 作为恢复路径，不改写 Work/Issue 或前任归因。

### 规范存储与可信投影

- additive migration `0029_project_role_history.sql` 补齐 Checkpoint revision basis、
  supersedes、Handoff Checkpoint 关系、append-only entity revision 和按 Role/revision 的
  历史索引；结构化引用继续进入 `project_role_continuity_references`，不塞回 Role 行。
- append-only trigger 禁止语义更新和删除，只允许可信 reprojection 重绑
  `projection_event_id`。deferred validation 绑定 source change 的 project revision、
  operation、actor、Assignment/Role/Commitment 归因与同 Project reference。
- Checkpoint/Handoff、引用、canonical rows、`40903` signed entity heads、旧 projection
  retirement、`40904` meta count/revision 和 write receipt 在同一 Project lock 事务中提交；
  任一步失败均不会留下部分连续性状态。

### 有界可信读取与历史分页

- 现有 `POST /query` 的 `buzz_project_view` extension 增加
  `v2_migration_current_entities` 与 `role_history` scope，不增加 Role 专用 HTTP endpoint。两者继续
  返回普通、可独立验签的 `40903` projection event。
- 默认 snapshot 精确读取当前 Role、open Proposal、active Assignment、active
  Commitment 及其必要依赖，并为每个未删除 Role 只附带最新 Checkpoint 和最近 3 条
  Handoff。默认 View 与每轮 Role Brief 的读取量不再随已结束任期和连续性历史线性增长。
- 历史页固定 `projection_generation + project_revision`，按
  `project_revision DESC + entity type + UUID DESC` 做 keyset pagination，并支持
  Role、Assignment、Member 和实体类型过滤。revision 改变返回 conflict；伪造、跨 Role
  或跨类型 cursor fail closed，客户端不能拼接不同 snapshot 的页面。
- 共享 `VerifiedRoleBriefSnapshot` 区分完整历史与有界历史覆盖：当前 object、Proposal、
  Assignment 和 Commitment count 仍与 signed meta 精确一致；Checkpoint/Handoff
  slice 只能小于等于 meta 总数，已携带的历史依赖仍逐项验证，不把部分历史伪装成完整
  snapshot。

### 完整 Role Brief、Agent 与 Human 入口

- 共享 `VerifiedRoleBriefSnapshot` 验证有界、可接续的 append-only heads；Role Brief 按
  `project_revision + UUID` 选择最新 Checkpoint，携带最近 Handoff、完整结构化字段及每段
  signed source reference。没有 Handoff 时仍从 Profile/Goal、responsible Work、
  Commitment 与最新 Checkpoint 组装可接续上下文。
- ACP 继续在每个完整 turn 前重建同一个 verified Brief。assigned Brief 现在明确要求
  Agent 在进展、阻塞、风险、未决问题或下一步发生重要变化后追加结构化 Checkpoint；
  读取使用默认有界实体页；写入仍由 managed CLI 在签名前刷新 Assignment fence，Relay
  做最终 fencing。
- CLI 增加 `buzz roles checkpoint append|list` 与
  `buzz roles handoff append|list`；Proposal、ended Assignment、Checkpoint 和 Handoff
  列表均提供真实服务端 newest-first limit/cursor 页面。JSON 文件或 stdin 先进入 typed
  schema；ended Assignment、错误 supersedes、错误 reference 或伪造 system cause 均不能
  形成写入。
- Desktop Role Inspector 展示最新 Brief 和有界 Checkpoint/Handoff 合并时间线；“Load
  more” 通过 infinite query 调用原生 revision-pinned 历史页并按实体 ID 去重。当前 Human
  assignee 可以填写结构化 Checkpoint 或 context-only Handoff。React 只提交
  revision-fenced intent，Tauri 使用 SDK builder 签名，冲突后刷新但不重放旧意图。

### 验证与范围

- 领域测试覆盖连续 Checkpoint、追加式纠正、ended Assignment 拒绝、缺失 reference、
  member Handoff 不卸任，以及 replacement 自动携带 Checkpoint/Work/Commitment。
- PostgreSQL 纵向测试覆盖从 `0024` 升级、checked-in schema 冷建库、规范行/引用/投影/meta
  原子提交、错误 Nostr reference 拒绝、history UPDATE/DELETE 拒绝、current/history
  翻页无重复，以及跨 Role cursor 拒绝。
- `buzz-project-view` `18/18`、`buzz-sdk` `250/250`（含 Role Brief 定向 `6/6`）、
  `buzz-cli` `261/261`、`buzz-acp` `600/600`、Desktop Tauri `1644/1644` 非 ignored
  测试、Desktop `3507/3507` 以及 Project View Playwright `28/28` 通过；`buzz-db`
  `87/87` 非 ignored 测试、新增 Relay bridge parser 测试和 PostgreSQL 纵向测试通过。
  workspace/Desktop all-targets check 与 clippy、TypeScript、Biome 和各 Desktop guard
  均通过。
- 阶段 6 至此形成 Role Continuity v0 的人工指派、替换和接续闭环。可信 runtime lease、
  epoch、supervisor evidence 与自动 `unrecoverable` 仍属于阶段 7。

## 2026-07-29 — 阶段 5：Work Responsibility 与 Commitment

### Project-owned Work 责任

- v2 Work head 增加独立于既有 v1 object body 的 `responsible_role_id` 关系；普通 Project
  View Work 内容仍复用原模型，Role 责任则由 Relay 签名的同一 Work projection 携带。
- `set_work_responsibility` 只允许 Community owner 或携带当前 Leader Assignment fence
  的 admin 执行；目标必须是 active Role。有 active Commitment 时不能单独改派或清除
  责任，Role 仍被 Work 引用时也不能停用或删除。
- Work responsibility 的改变与 Work object revision、全局 project revision、旧 head
  retirement、receipt 和 v2 meta head 在同一事务中推进，不产生第二份旁路状态。

### Assignment-owned Commitment

- 增加 first-class `WorkCommitment` 生命周期实体，固定记录 Work、Assignment、Member、
  接受者、开始时间、终止者、终止时间与原因。Member 归因在创建后不可改写，继任者只能
  创建自己的新 Commitment。
- 增加 `accept_work`、`end_commitment` 和原子 `replace_commitment` command。只有
  Work responsible Role 的当前 assignee 能接受；释放与替换必须携带该 Member 的精确
  active Assignment fence，并以 observed Commitment ID 防止覆盖并发状态。
- Assignment 结束会在同一 project revision 中结束其全部 active Commitment，但不修改
  Work status 或 responsible Role。Work 进入 completed/cancelled 或被删除时，也会在
  普通对象事务内以 `work_closed` 结束 active Commitment。
- additive migration `0028_project_work_commitments.sql` 补齐 immutable
  `member_pubkey`、entity revision、latest change 和更新时间，并用 deferred constraint
  校验 Assignment/Member/签名者一致、active Commitment 只指向非终态 Work。materialized
  active Commitment count、canonical row、signed entity head 和 meta count 同事务提交。

### 同一 verified state 的 Agent 与 Human 入口

- 共享 `VerifiedRoleBriefSnapshot` 现在验证 Work responsibility、Commitment、
  Assignment 和 meta count 的一致性，并把当前 Role 的非终态 Work 派生为
  `committed` 或 `waiting_for_continuation`。JSON、共享 Markdown renderer、ACP 动态
  Role Brief 与 Desktop 继续来自同一个 assembler。
- CLI 增加 `buzz roles work assign|unassign|accept|release|recommit`；managed Agent
  继续在签名前刷新完整 verified snapshot，并自动携带当前 Assignment fence。
- Desktop Role Inspector 展示 responsible Work 与接续状态；Work Inspector 允许 Human
  owner/Leader 分配责任，允许当前 assignee 接受或释放 Work。React 只提交 revision-fenced
  intent，Tauri/Relay 执行最终校验并在冲突时要求 Human 查看新状态。

### 验证与范围

- 纯领域测试覆盖跨 Role 接受拒绝、active Commitment 改派拒绝、Assignment 结束不改变
  Work、等待接续，以及 recommit 保留前任归因；SDK 测试覆盖责任 projection、
  Commitment verified snapshot 和 Role Brief 派生。
- migration/schema 静态门禁、workspace all-targets check、Desktop Tauri all-targets
  check、TypeScript 与 Biome 检查通过；Desktop E2E 覆盖 verified Commitment 展示和
  owner 的 revision-fenced Work 分配。
- 本阶段不实现结构化 Checkpoint、成员撰写的完整 Handoff、Role 历史分页或完整 Role
  Brief 时间线；这些属于阶段 6。

## 2026-07-29 — 阶段 4：Managed Agent 绑定与最小 Role Brief

### 共享 verified Role Brief

- 在 `buzz-sdk` 增加唯一的 `VerifiedRoleBriefSnapshot` 组装边界。CLI、ACP 和 Desktop
  先使用既有 v2 parser 验证 Relay 签名，再由共享组装器统一校验 project/generation/
  revision、meta counts、membership pointer、Project View 关系、Role/Assignment 唯一性
  以及 Leader 与 Community `admin` 的一致性；任一部分不一致都不会产出 Brief。
- 增加 canonical `RoleBrief` JSON DTO 和共享 Markdown renderer，区分
  `candidate` 与 `assigned`。最小 Brief 包含 Project Profile、全部 active Goal、Role
  定义与 level、当前 Assignment fence、与 Role 直接相关的 Issue/Work 切片，以及
  meta、membership、对象和实体各自的 signed source reference。
- Role Brief 仍是从当前投影即时派生的 read model，不新增表、不保存 `role.md`，也不成为
  第二份规范状态。相同 verified snapshot 供 JSON、CLI Markdown、ACP prompt 和 Desktop
  展示共同使用。

### Managed Agent 动态绑定与写入 fencing

- ACP 在 Relay 连接后的启动阶段解析一次 Role 状态，并在每个 channel turn 和 heartbeat
  创建 session 前重新读取 NIP-11 Relay identity、meta、完整 v2 heads 和 meta 精确指向的
  membership snapshot。读取使用前后 meta bracket；Brief 不能验证或超时时只注入
  `State: unavailable`，不缓存、不回退到旧 Assignment。
- 最新 Brief 作为动态 `[Role Brief]` user-context section 注入。它不进入长期
  `systemPrompt`；slash command 仍保持首 block，Brief 紧随其后。ACP 同时发出
  `role_context_resolved` observer frame，记录状态、Assignment、project revision、
  projection generation 和失败类别。
- 所有 managed Agent 的首次启动、lazy wake、slot refill、panic recovery 和 respawn
  都由 harness 强制携带 `BUZZ_MANAGED_AGENT=1`；persona 和父进程环境不能覆盖。该标记也
  传入 developer MCP，使 Agent 通过 shell 或 MCP 调用 `buzz` 时进入同一 managed 模式。
- managed CLI 在每次 Role 或 Project View v2 写入签名前重新读取 verified snapshot。
  已分配 Agent 自动携带当前 `acting_assignment_id`；显式提供的旧 fence 会在本地拒绝，
  Relay 继续执行最终 Assignment fencing。未分配 Agent 可以读取、请求或处理属于自己的
  Proposal，但不能进行普通 Project View 角色身份写入。
- `buzz project-view` 补齐 schema-v2 read/create/update/delete，使 Agent 与 Desktop
  修改同一份 Project View；v1 路径保持兼容。v2 写入后的 receipt 使用完整 verified
  snapshot 确认，不把数据库或未验证事件作为客户端事实源。

### CLI 与 Desktop 展示

- 增加 `buzz roles brief [--member <pubkey>] [--markdown]`。默认输出 canonical JSON，
  `--markdown` 输出与 ACP prompt 相同的共享 renderer 结果。
- Desktop Tauri 从同一个共享组装器为每条 active Assignment 生成 Brief；React 只消费
  该 verified DTO。Role Inspector 展示 Project 摘要、Goals、相关 Issue/Work 数量、
  project revision 和 projection generation，并继续与当前任期、Proposal、Handoff
  共用同一 v2 snapshot。

### 验证

- `buzz-sdk` 完整测试 `246/246`、`buzz-cli` `259/259`、`buzz-acp` library
  `600/600` 和 pool lifecycle `9/9` 通过。
- Desktop 单元测试 `3507/3507`、Biome/file-size/px-text/pubkey guards、TypeScript
  production build 和 E2E build 均通过。
- Project View Playwright smoke `24/24` 通过；新增断言覆盖 Inspector 的 verified
  Role Brief。Desktop Tauri Project View 定向测试 `12/12` 通过。
- `buzz-sdk`、`buzz-cli`、`buzz-acp` 和 Desktop Tauri 的
  `clippy --all-targets -D warnings` 均通过；workspace Rust fmt 与差异 whitespace
  检查通过。

### 范围边界

- 本阶段完成的是 Assignment 身份绑定和最小上下文，不包含 Work
  `responsible_role_id`、Commitment 或 `waiting-for-continuation`；这些属于阶段 5。
- 结构化 Checkpoint、成员撰写的完整 Handoff、Role 时间线和完整 Role Brief 属于阶段
  6；runtime lease、epoch、可信 supervisor 和自动 `unrecoverable` 属于阶段 7。

## 2026-07-28 — 阶段 3：Desktop Human 治理

### v2 普通 Project View 对象写入

- 补齐 schema-v2 `initialize/create/update/delete` command 信封，复用既有对象 reducer，
  但显式携带 `schema_version = 2`、`expected_project_revision` 和可选
  `acting_assignment_id`。v1 wire command 不会被当成 v2 接受。
- Relay 按 v2 command 的 closed shape 区分普通对象写入与 Role Continuity command；
  普通对象同样经过 Community Project lock、全局 project revision、幂等 receipt、
  canonical row、旧 head retirement、`40903` object/Role head 和 `40904` meta 的单事务
  提交。
- v2 新建 Role 默认是普通 `member` level；通用 Role 更新保留既有 level，不能借对象
  patch 升降级。Role 仍有 active Assignment 时，通用停用和删除在服务端准备阶段拒绝，
  不依赖 Desktop 按钮守卫。
- Desktop native mutation 根据 NIP-11 capability 生成 v1 或 v2 command，并对返回的
  object/meta projection 做 schema-aware 签名与 source/revision 确认；revision conflict
  返回 typed result，不自动重放 Human intent。

### Desktop verified read 与 Role 治理

- Tauri `get_project_view` 支持 verified v2 snapshot：固定 NIP-11 Relay identity，校验
  meta、普通 object、Role/Proposal/Assignment/Handoff entity head、membership snapshot、
  counts、generation、revision 与 source pointer，再把组装后的 typed DTO 交给 React。
- `View` 的 Role 卡显示 `Leader/Role` level、当前承担者、vacant 与 inactive 状态；Role
  Inspector 显示当前任期、open Proposal、历史任期和最小 Handoff，承担者名称与普通成员
  profile 共用同一解析结果。
- 增加 typed Tauri Role mutation，覆盖 request、offer、accept、reject、withdraw、
  authorize 和治理性 end。owner 可以治理全部 Role，active Leader 只能治理普通 Role；
  普通成员可请求 Role 或处理属于自己的 Proposal，assignee 不获得主动结束自己任期的入口。
- 指派/替换候选人只来自 verified Community membership；写入带当前 project revision。
  409 conflict 会使当前 Community 的 Project View query 失效并读取最新快照，旧表单动作
  不会在新 revision 上自动执行。
- 普通 Role 编辑器在有 active Assignment 时禁用 deactivate，Inspector 禁用通用 delete；
  即使绕过 UI，Relay/DB 仍执行相同硬约束。

### Community Members 设置与 Community 隔离

- v1 Community 保留原 invite、promote、demote、remove 行为；v2 Community 的 Settings
  fail closed，隐藏直接邀请与等级/删除 mutation，并统一引导到 `View` 的 Role 治理流程。
- v2 成员行显示当前 Role，不能从 Settings 直接制造没有 Leader Assignment 的 admin，
  也不能直接删除有任期的 Member；Relay 继续作为最终权限边界。
- Role continuity 不增加 module-level Community cache，沿用包含 Community identity 的
  Project View query key 和现有 key-based remount。切换 Community 后会重新读取对应
  snapshot，不复用上一项目的 Assignment。
- 将 Role continuity API、integrity error 与大型 Tauri 测试夹具拆为独立模块，所有
  Desktop 源文件继续满足 1000 行硬上限。

### 验证

- Desktop 单元测试 `3507/3507`、Biome/file-size/px-text/pubkey guards、TypeScript
  `--noEmit` 均通过。
- Project View Playwright smoke `24/24` 通过；新增场景覆盖 v2 Role 卡与 Inspector、
  owner 携 revision 发起 Offer、conflict 刷新且不重放、Settings 不直接改 admin，以及
  Community 切换后 assigned→vacant 不泄漏。
- Tauri Project View 定向测试 `12/12` 通过，覆盖 verified snapshot、capability 状态、
  typed mutation、签名投影确认、revision conflict 不重试和 Role command 输入边界。
- `buzz-project-view` v2 object command 测试、`buzz-sdk` v2 projection 测试与
  `buzz-db` PostgreSQL 纵向测试通过；实库测试验证 cutover 后普通 Goal 写入进入同一
  revision/projection 流，active Assignment Role 的通用停用不会提交。
- `buzz-project-view`、`buzz-sdk`、`buzz-db`、`buzz-relay` 和 Desktop Tauri 的
  `clippy --all-targets -D warnings` 均通过。

### 范围边界

- 本阶段交付 Human 对 Assignment 的完整首轮治理面，但尚未让 managed Agent Runtime
  自动解析和携带自己的 Assignment；Agent 仍通过阶段 2 的 `buzz roles` CLI 显式操作。
- Role Inspector 目前展示阶段 2 的最小 Handoff；Work Commitment、结构化 Checkpoint、
  完整 Role Brief 和可信 runtime 自动故障恢复分别留在后续阶段。
- 下一阶段是 managed Agent 绑定与最小 Role Brief：Runtime 在启动和每个 turn 前读取
  verified active Assignment，动态注入 Role 上下文，并让所有角色身份写入自动携带
  `acting_assignment_id`。

## 2026-07-28 — 阶段 2：Proposal、Assignment 与 CLI 纵向闭环

### Role Continuity 状态机

- 增加 closed `RoleCommand` 与纯领域 reducer，覆盖
  `request/offer/accept/reject/withdraw/expire/authorize` Proposal、人工结束
  Assignment、`request_replacement` 和 `report_unable_to_continue`。
- Proposal 创建时同时记录目标 Role 和候选 Member 的 active Assignment fence；最后一个
  必要确认到达时重新验证候选资格、授权者资格和两段 fence。任一条件变化都会拒绝整个
  command，不会先消费 Proposal 再留下半套状态。
- Assignment 激活或替换是一次 project revision 内的 compound transition。候选者原有
  Role 与目标 Role 的旧任期会一起结束，新任期只创建一次，并为每条被替换任期生成最小
  Handoff。
- 固定治理边界：assignee 不能主动结束自己的 Assignment；owner 可以治理所有层级；
  active Leader 只能治理普通 Role，并且必须携带与 signer 完全一致的当前
  `acting_assignment_id`；verified human owner 可以结束自己名下 managed Agent 的普通
  Assignment。
- 普通 Role 对应 Community `member`，Leader Role 对应 Community `admin`。Assignment
  变化产生期望等级，由同一事务更新 canonical membership 和 NIP-43 snapshot。

### v2 原子协调器与 cutover

- 增加 additive migration `0027_project_role_assignment_state.sql` 并同步
  `../../../schema/schema.sql`：补齐连续性实体 revision/change pointer、Assignment 的替换请求与
  unable report、最小 Handoff，以及 v2 meta 的实体计数和 membership snapshot pointer。
- 增加统一 v2 write transaction：在 Community Project lock 内完成 command receipt、
  canonical Proposal/Assignment/Handoff、Community 等级、NIP-43 snapshot、`40903`
  entity heads、`40904` meta 和旧 head retirement。签名投影在 commit 前会被重新解析并与
  canonical state 对照，任一步失败全部回滚。
- 增加显式 `buzz-admin project-view cutover-v2`。cutover 只接受 disabled、已初始化的
  v1 Community；现有非 owner admin 必须逐一显式映射到唯一 Leader Role 或显式降级，
  不能按名称猜测。操作绑定 hash-chain audit、稳定 idempotency hash，并原子提升 schema、
  project revision 和 projection generation。
- cutover 会把全部 v1 当前对象与 tombstone 重投影为 v2 head，而不是只迁移 Role；旧
  current heads 同事务退休。重复相同 operator idempotency key 返回原 receipt。
- `buzz-project-view-v2` 只在 feature enabled、Relay signer 一致、meta/membership/counts
  完整、全部 canonical current rows 都有同 generation 的 live projection head 时由
  NIP-11 宣告；残缺 cutover 或手工退休任一 head 都保持 fail closed。

### Relay、SDK 与 CLI

- Relay 按 Community 的 schema version 分派 v1 mutation 或 v2 Role command，并在 receipt
  lookup 前执行当前 membership、ban、feature、signer 与 Assignment fence 检查。已接受
  event 不能借重放绕过后续卸任或权限变化。
- SDK 增加 v2 command、Role continuity entity、普通 Project object、membership 和 meta
  的 typed builder/parser。parser 校验 kind、Relay signer、签名、closed content、精确
  tag 顺序、schema、project/generation coordinate 与 source pointer。
- 增加 `buzz roles list/get/current/proposals/request/offer/proposal/assignment`。CLI
  先从 NIP-11 固定 Relay identity，再验证 meta、全部 Role continuity heads 和 meta 指向
  的唯一 membership snapshot；Assignment、Role level 与 Community 等级不一致时拒绝
  展示快照。

### 验证

- 纯领域测试覆盖双旧任期原子替换、授权者在最终确认前失权不产生部分消费、assignee
  自卸任禁令与 replacement request、Leader 的精确 active Assignment fence。
- 13 项 Project View PostgreSQL 测试全部通过，包含显式 v1→v2 cutover、cutover
  idempotency、残缺 generation 启用拒绝、owner 首次指派、A→B 替换、唯一 active
  Assignment、最小 Handoff、自卸任拒绝和旧 Assignment 延迟命令拒绝。
- 真实进程 E2E 使用隔离 PostgreSQL/Redis、真实 Relay、`buzz-admin` 和 `buzz` CLI，完成
  v1 对象写入、停用、v2 cutover、NIP-11 capability 切换、Agent A 接受 Offer、自卸任
  拒绝和 A→B 替换；最终 CLI 读取到两条任期历史与一条 Handoff。
- 9 项 `buzz-project-view`、243 项 `buzz-sdk`、259 项 `buzz-cli`、6 项 Relay
  Project View、13 项 Project View 实库事务、6 项 migration/schema-drift 测试及相关
  crates 的 `clippy --all-targets -D warnings` 均通过。

### 范围边界

- cutover 是显式 operator 动作，不会自动迁移或启用任何现有 Community；阶段 2 只适合
  受控 CLI pilot。
- v2 普通 Project View 对象已有完整 cutover/read projection，但 v2
  `initialize/create/update/delete` 写入尚未开放；schema-v2 Community 会明确拒绝旧 v1
  mutation，避免被旧客户端误写。恢复普通对象写入是 Desktop 阶段开始前的后端前置项。
- 本阶段不包含 Desktop Human 治理、managed Agent 自动携带 Assignment、Work
  Commitment、完整 Checkpoint/Role Brief 或可信 runtime 自动故障恢复。

## 2026-07-28 — 阶段 1：Membership 一致性内核

### v2 边界与变更来源

- 增加独立的 `buzz-project-view::v2` 领域入口，固定 closed
  `SchemaVersion`、`RoleLevel`、admin Role 创建与生命周期的 owner-only
  治理规则，以及 Nostr、NIP-98、operator、system 四类 typed change source。
- NIP-98 request hash、operator/system idempotency-key hash 和 change ID 使用不同的
  SHA-256 domain separator，并以固定测试向量锁定算法；Nostr change 直接复用 source
  event ID。
- 增加 additive migration `0026_project_role_continuity.sql` 并同步
  `../../../schema/schema.sql`。所有既有与新建 Community 仍默认
  `project_view_schema_version = 1`，本阶段不宣告 `buzz-project-view-v2`。
- `project_view_changes` 对 source event、audit sequence、idempotency hash 和 project
  revision 建立 Community-scoped 唯一性；operator/system source 必须引用同一
  Community 的 hash-chain audit row。Proposal、Assignment、Commitment、Checkpoint、
  Handoff 与引用表只建立规范存储和约束骨架，尚未开放协议写入。
- migration 26 使用兼容触发器镜像 v1 Relay 仍只写入的 `last_event_id`，保证 additive
  migration 后旧 v1 写入形态仍可运行。迁移测试实际覆盖 fresh、0024→0026、
  ledger-less schema、并发 migrator、旧 v1 INSERT/UPDATE 形态和 schema drift。

### Community / Project 共同一致性锁

- 将原 Project View advisory lock 提取为唯一的 Community/Project lock primitive。
  Project View writer、全部 `relay_members` mutation、持久 ban/unban、timeout、
  managed Agent owner materialization、NIP-43 snapshot publisher 与 reconcile
  使用同一 lock key。
- 固定锁顺序为 Community/Project lock 在前、NIP-43 replacement lock 在后。并发测试
  证明 snapshot publisher 会等待未提交的 membership transaction，并在取得锁后读取
  最新完整成员集合。
- schema-version 查询只依赖已持有的 advisory lock，不额外获取 Community 行级
  `FOR UPDATE`，避免阻塞 event 外键所需的 `KEY SHARE`。

### v1 兼容与 v2 fail-closed

- v1 Community 继续保留既有 add/remove/change、invite、bootstrap、owner transfer、
  ban/unban 和 allowlist backfill 语义；新内核只改变事务组织和锁，不把 v2 Role
  规则提前施加给 v1。
- v2 Community 禁止通用入口直接创建 `admin/owner`、降级 active Leader、移除有 active
  Assignment 的 Member、移除仍拥有 active managed-Agent Assignment 的 Human、
  self-leave 绕过、admin invite、managed Agent 成为 Community owner，以及在转移
  ownership 前 ban 当前 owner。
- 单 Community 与批量 v1 Project View 开关都会拒绝启用 v2 Community，避免出现“v2
  不宣告 capability、却被旧开关标成 enabled”的误导状态。
- Assignment 激活后，partial unique index 保证一个 Role 和一个 Member 各自最多一条
  active Assignment；deferred constraint 在 commit 时验证 Role active/level、
  Community 等级、managed Agent owner 资格、persistent ban、Work responsibility
  和 Commitment 关系。约束触发器会同时验证跨 Community UPDATE 的旧、新两侧；直接
  SQL 提权、删/挪成员或 ban owner 同样会被拒绝。
- known managed Agent 即使已经 materialize 为 direct member，也继续依赖 verified
  owner 的有效 Community membership，且 Agent 与 owner 都必须未被 ban；direct row
  不再成为绕过入口，managed Agent 也不能被递归地用作另一个 Agent 的治理 owner。
  Relay 用一条 SQL 快照同时解析 direct membership、managed owner 和双方 ban 状态，
  避免授权读取拼接 membership/ban 变化前后的两代状态。
- owner 变更所需的“旧 owner 按 active Assignment 推导为 `admin/member`”已形成事务内
  primitive。由于完整 v2 source/audit/meta/NIP-43 projection coordinator 尚未开放，
  本阶段对会成功改变 v2 membership 的通用入口统一返回
  `unavailable:project_view_v2:membership_coordinator`，而不是提交不增加 project
  revision 的半套状态。配置中的 owner 与当前 v2 owner 相同时，startup bootstrap
  保持幂等 no-op。

### 验证

- 纯领域测试覆盖未知 schema 拒绝、change ID 固定向量、operator/system domain
  separation、非法 audit sequence，以及 admin Role 创建、升降级、停用和重新启用的
  owner-only 规则。
- PostgreSQL 测试覆盖 v2 直接提权/删除/ban 旁路、active Assignment membership
  守卫、managed Agent owner 资格、owner ban、owner role 推导、v1 capability 隔离和
  v1 writer fail-closed。
- Project View 16 项实库事务测试、relay-members 10 项实库测试、moderation 4 项实库
  测试、user 8 项实库测试，以及 migration/schema-drift gate 均通过。

### 范围边界

- 本阶段不提供 v2 cutover、不宣告 v2 capability，也不开放 Proposal、Assignment
  command、v2 projection、CLI 或 Desktop。
- 成功的 v2 membership mutation 必须等待后续 coordinator 将 typed change source、
  audit receipt、project revision、canonical membership、NIP-43 snapshot 与 v2 meta
  放入同一事务；在此之前保持 fail closed。
