# Project View 变更记录

## 2026-07-30 — Community 展示页承载 Project View

- 新增
  [Community 展示页中的 Project View 前端设计](./community-project-view-frontend-design.md)，
  修正客户端 v0 将 `View` 与 Inbox、Pulse、Projects 等入口并列所造成的信息架构偏差。
- 前端产品层级调整为“Community / Project Space 持有 Project View”：Human 主动选择
  Community 后直接进入其展示页，默认看到 Project Profile、Current Focus、Role 与明确
  注意事项的可信摘要，再按需展开完整 Project Map、未规划对象、Resources 和 Inspector。
- Role 从完整 View 底部的 Supporting Object 提升为 Community 展示页的一等协作摘要；
  owner、Leader Role、普通 Role、Assignment 与 Runtime 继续保持不同语义，完整 Role
  Brief、Checkpoint、Handoff 和治理操作仍在 Inspector 中按需展开。
- 独立 `View` 主侧栏入口不再是最终信息架构的一部分。现有 `/view` 与
  `/view?object=<id>` 保留为完整展开状态和深链接兼容入口，现有 `Projects` 名称、路由
  与 Git/NIP-34 语义保持不变。
- 默认摘要与完整 View 必须来自同一份 native 验证后的 Project View + Role Continuity
  snapshot，不增加第二份 Profile、Role Directory、Markdown 摘要或未验证缓存；既有
  revision、live refresh、conflict、Community 隔离和 integrity fail-closed 边界继续
  有效。
- 本次只形成一份完整设计，不拆分新的开发阶段；具体组件、路由组合和迁移接缝在实现时
  根据现有 Desktop 架构决定。

### 实际页面复核后的设计修正

- 复核确认改造前没有独立 Community 展示页；原 Community Rail 保存、验证并恢复每个
  Community 的 Inbox/Channel destination。新增 Overview 不得删除这项客户端工作位置
  连续性。
- Human 主动选择 Community 后仍可先进入 Overview，但目标 Community 已保存的
  destination 必须保留，并通过 `Continue in #channel` 或 `Open Inbox` 等入口继续；
  Overview、完整 View、Inspector 和 Settings 均不覆盖该记录，失效 Channel 安全回退
  到 Inbox。
- Community 展示页被明确拆成稳定空间外壳与 Project View 项目区域。`projectView`
  preview 默认关闭时，Community 身份、成员身份、继续上次工作及既有导航仍然可用；
  启用提示只能是紧凑的次要区域，不能用大面积空状态接管整页。
- 宽屏首屏优先 Project identity、当前方向、Current Focus、关键 Role 和显式注意事项；
  verified/stale 等可信状态只保留一个主要标识，不以重复 badge 挤占内容。
- destination 保存、目标 Channel 验证、不可用回退和同 Community 恢复不改路由的既有
  测试语义必须保留，或按 Overview + Continue 的新交互改写，不能直接删除。

### Desktop 初版实现（待按复核修正）

- 新增 `/community` Community 展示页；侧栏固定区以当前 Community 名称提供
  Overview 入口，独立 `View` 菜单项移除。即使只有一个 Community，Human 也可以从
  当前空间名称返回 Overview。
- 默认摘要直接展示 Project Profile、Goal 方向、显式状态派生的 Current Focus、Role
  Assignment、Needs Attention、Resources，以及 project revision、projection
  generation 和更新时间；Role 排序只用于导航便利，不写入新的领域优先级。
- 摘要继续复用 `get_project_view` 可信读取、Community-scoped React Query key 与
  projection live invalidation。新增共享 actor resolver，使 Overview 与完整 View
  使用同一 snapshot 和相同 Human/Agent 身份解析，没有新增摘要表、Markdown 或
  module-level Community cache。
- 摘要对象通过 `/view?object=<id>` 进入现有完整 View 和 Inspector；`/view` 的
  初始化、Add/Edit/Delete、Role Continuity、冲突恢复和深链接能力完整保留。完整 View
  页头显示当前 Community，并提供返回 Overview 的可访问入口。
- Human 从 Community Rail 选择另一个 Community 时，初版实现会进入 `/community`，
  但同时删除了目标空间的 Channel 恢复路径；实际页面复核后，这一实现不再视为最终
  设计。后续需要保留 Home teardown barrier 和 destination 校验，并在 Overview 提供
  “继续上次工作”。
- Unsupported、Uninitialized、Forbidden、完整性失败、普通读取失败、Loading、
  syncing 与 offline-stale 均在 Community 项目区域内显式表达；但初版在 Project View
  preview 关闭时形成了大面积空状态，尚未满足修订后的稳定 Community 外壳要求。
- Desktop typecheck、Biome/文件大小/可缩放文本/公钥守卫、3508 项 unit test 和
  Project View + Community Rail 的 46 项 Playwright 场景完成验证；其中一次切换等待
  的测试时序缺陷修正后定向复验通过。

## 2026-07-28 — Client Slice 4：验收与体验收口

### 完整状态与失败收口

- 首次可信读取使用与 Profile、统计和地图结构接近的稳定骨架；Loading 不再表现成容易与
  空 View 混淆的单个转圈，也不会在验证完成前提前渲染项目正文。稀疏但合法的 View 会
  分别说明空 Goal、Role、Resource 以及未关联/未规划区域，不把“没有内容”和读取失败
  混为一谈。
- Desktop TypeScript command 边界增加防御性 DTO 完整性检查，拒绝重复对象、非法
  revision、active object count 不一致、规范位置与关系不一致、失效 Issue 引用等
  自相矛盾结果。签名、Relay signer、projection generation 和领域不变量的权威验证仍
  完全由 native Rust 边界负责；前端不验签、不解释原始 projection，也不建立第二套
  权威模型。
- 普通读取失败与完整性失败使用不同界面。完整性失败 fail closed，不展示任何可疑的
  Profile 或部分地图，只提供重新执行完整可信读取和可展开的安全诊断原因；诊断不包含
  event content、Resource locator 或数据库内容。

### 键盘、可访问性与窄窗口

- Project Map 支持方向键循环遍历对象卡片，`Home`/`End` 跳到首尾，`Enter` 使用按钮
  原生语义打开 Inspector；关闭 Inspector 后焦点返回来源对象。地图、加载状态和宽屏
  Inspector 增加可识别的区域/状态语义，状态与优先级继续同时显示文字而不只依赖颜色。
- 初始化与对象修改都使用原生 form submit，键盘可从字段直接进入 Review 或提交；
  required 字段同时具有原生 `required`、`aria-required`、关联 label 与说明文本，
  Role active switch 具有可读名称。conflict 出现后提交键保持禁用，Human 必须先显式
  选择最新可信 revision，避免 Enter 绕过冲突确认。
- 窄于共享辅助面板断点时 Inspector 使用 modal Sheet：遮罩、焦点约束、Escape 关闭和
  焦点恢复由同一可访问组件负责；宽屏仍保留固定侧栏，并补齐 Escape 关闭。标题、动作和
  页面间距在窄窗口收缩，文本继续使用现有 rem 尺度。

### Community 隔离与 Human/Agent 验收

- 浏览器测试桥只在 E2E 环境按已 apply 的 Relay URL 提供独立 Project View fixture，
  从而真实经过 Community apply、keyed remount、React Query key 和 `/view` 导航，
  验证 A→B→A 不携带对象选择、URL 定位或 Human 草稿，返回 A 后重新读取其可信状态。
  该接缝不进入发布 bundle，也没有新增生产 module-level cache。
- Desktop 场景覆盖 Human 保存后 Agent projection 信号触发下一份完整可信快照，并同时
  检查 revision、正文和修改来源。真实 Relay E2E 扩展为 Human 初始化、Agent 通过
  HTTP 写入、Human 通过真实 `buzz` CLI 写入、Agent 再写入的交替序列；每一步都重新
  确认 Relay revision，HTTP/CLI 两次交接后的 object projection 分别验证 Agent/Human
  actor，最后一次 Agent 写入继续验证成员撤权后 live fan-out fail closed。

### 客户端发布、兼容与运维

- 发布继续采用 server-first：先完成 Relay binary、migration 25、稳定 signer、
  capability/readiness 与 Community checked enable，再发布包含 View UI 的 Desktop。
  新 Desktop 连接旧版、未迁移或未开启 Relay 时只显示 Unsupported，不发送未知
  mutation；现有 `Projects` 与其他 Buzz 功能保持不变。旧 Desktop 会忽略新增入口，
  Project View 数据与 additive migration 保留在 Relay 侧。
- signer rotation 继续遵循后端 runbook 的 disable → reproject → checked enable。
  generation 变化或 Community/连接恢复只触发客户端重新读取完整快照，不增量拼接旧、新
  signer 的 projection；维护窗口内 capability 不可用时按 Unsupported/重试处理。
- 若单个客户端出现完整性失败，先重试完整读取并检查 Desktop/Relay 安全日志；若多个
  客户端持续失败，operator 应停止该 Community 的 Project View 写入/宣告，按既有
  `docs/project-view-operations.md` 检查 schema、meta/object revision、active count、
  signer 与 projection generation，再执行 repair/reproject。不得用未验证 JSON、
  本地缓存或数据库直读绕过失败界面。
- 客户端 v0 不持久化权威快照，也不跨应用重启保存编辑草稿；回滚 Desktop 不删除 Relay
  数据，回滚 Relay 继续遵守“应用可回滚、数据库只前进”和已有数据后的 compatible
  rollback 边界。

### 验证

- 新增 Project View 键盘导航与 native DTO 完整性定向测试，并保留 live filter、刷新
  合并和断线恢复测试。
- Project View Playwright smoke 扩展到 19 项，覆盖稳定 Loading、稀疏合法 View、
  普通可信读取失败、完整性 fail-closed、键盘遍历/焦点恢复、窄窗口焦点约束抽屉、
  Human/Agent 交替修改和跨 Relay Community 隔离；既有初始化、类型化创建、conflict、
  实时刷新、删除保护与 capability 状态继续覆盖。
- 真实 Relay/HTTP/CLI Project View E2E 保留为发布门禁；Desktop typecheck、unit、
  Biome、文件大小、可缩放文本、公钥展示守卫和 E2E build 共同作为客户端 v0 验收门。

### 范围边界

- Client Slice 4 没有增加对象类型、关系、event kind、HTTP endpoint、数据库读取、
  TypeScript 签名验证、CLI 子进程调用、跨 Community cache 或 `Projects` 迁移。
- 至此 Desktop Client v0 的四个 Slice 完成：Human 与 Agent 可以从各自操作面读取和
  修改同一幅 Relay 权威 Project View；Web、Mobile、完整历史/diff、跨重启草稿和大型
  View 搜索/虚拟化仍属于后续范围。

## 2026-07-28 — Client Slice 3：实时协作与冲突恢复

### 可信实时刷新

- Desktop 订阅当前 Relay signer 发布的 `40903` Project View Object 与 `40904`
  Project View Meta。投影事件只作为“权威状态可能变化”的失效信号，事件正文不进入
  React 状态；每次变化仍通过既有 native `get_project_view` 边界重读、验签并组装完整的
  revision 一致快照。
- live filter 从最后一份可信快照时间向前保留短窗口，订阅建立后主动再确认一次快照，
  关闭初始读取与订阅建立之间的竞态。一次 mutation 产生的多个 projection event 会被
  合并为一次刷新；刷新期间再次到达的信号会保留一次 trailing refresh，不会因并发请求
  丢掉更晚 revision。
- Project View query 纳入 Desktop 既有 Relay auto-heal：断线恢复后统一失效重取。
  订阅沿用 RelayClient 的重连重放；进入或返回 `/view` 时重新确认快照，Community
  切换则由既有 keyed QueryClient/remount 与 `relayClient.disconnect()` 清理旧查询、
  订阅、表单和对象选择，不增加 module-level Community cache。
- Refreshing 时保留上一份已验证内容并说明正在验证新快照；连接断开、订阅重试或后台
  重取失败时仍保留已验证 revision，但明确标记可能过期。后台刷新错误不再用全屏错误
  覆盖已有可信内容，也不会把 projection event 直接显示成部分成功状态。

### revision conflict 与草稿

- Create/Edit/Delete 在打开时固定 `baseRevision`；实时刷新只更新旁边的最新可信 View，
  不会静默替换写入基线。服务端 conflict 后不自动重试 mutation，而是保留表单、自动
  获取最新完整快照，并说明项目从哪个 revision 变化到哪个 revision。
- Edit/Delete 会对比目标的旧、新 object revision，区分“目标未变、项目其他对象变化”、
  “目标本身变化”和“目标已删除”。只有最新可信 project revision 已达到 conflict
  revision 时，Human 才能显式选择新基线；选择后仍需再次点击 Save/Delete 才会提交。
  conflict 中关闭弹窗不会静默丢弃输入，放弃草稿是显式动作。
- 初始化表单提升到 View 页面生命周期保存。若 Agent 或其他成员先完成初始化，页面会
  切换到最新 Ready View，同时保留未写入的 Profile/Goal 草稿并展示恢复区；由于
  Initialize 只能执行一次，旧草稿不能覆盖新 View，只能供 Human 检查后通过普通对象
  编辑选择性应用。Community 切换会随页面边界清除该草稿，避免跨 Community 泄漏。

### 修改来源

- Project Profile、地图卡片、Role 与 Resource 增加最近修改者和时间的轻量提示；
  Inspector 继续展示完整 `created_by`/`updated_by` 与时间、公钥。
- actor 首先通过 Buzz profile 解析；本地 managed Agent 或 Relay Agent 即使还没有可用
  profile，也会使用其已知名称并标为 Agent。无法解析时只显示缩略公钥，不伪造身份。

### 验证

- 新增 Project View live filter、projection burst 合并、刷新失败后恢复以及 Relay
  auto-heal query 分类的定向测试。
- Project View Playwright smoke 扩展到 11 项，新增 Agent 名称来源、projection event
  驱动完整快照刷新、初始化草稿恢复、旧 revision 显式重基线再提交和离线可信快照保留；
  既有初始化、类型化创建、删除保护与 capability 状态继续覆盖。
- Desktop typecheck、Biome、文件大小、可缩放文本和公钥展示守卫通过。

### 范围边界

- 本 Slice 不在 TypeScript 中验签或拼接 projection，不增加数据库读取、专用 HTTP
  endpoint、CLI 子进程、对象增量缓存或第二份权威状态。
- 完整历史/diff、跨应用重启持久化草稿、批量协同操作和 Web/Mobile 实时客户端仍不属于
  Client v0；键盘、可访问性、窄窗口与真实 Relay 交替修改的最终验收属于 Client
  Slice 4。

## 2026-07-28 — Client Slice 2：初始化与类型化修改

### Human 写入边界

- Desktop 新增单一 `mutate_project_view` Tauri command，只接收
  Initialize/Create/Update/Delete 四类 closed typed intent。UI 不构造事件、不提供原始
  JSON 编辑，也不调用 CLI；Rust 使用既有 `buzz-sdk::project_view` builder 生成
  `44300` mutation，并为初始 Goal 与新对象生成 UUID v4。
- command 在首次异步等待前固定当前 Relay URL 与成员签名身份，随后验证 NIP-11
  capability/signer，以同一身份签名 mutation 和 NIP-98 请求，避免已开始的意图被后续
  Community 切换重定向。
- 成功写入不只相信 HTTP 状态：客户端要求 event ID 相同且回执使用规范
  `response:<json>`，再重读 Relay 签名的 meta；Create/Update/Delete 还按对象坐标重读
  projection，验证 signer、generation、revision、对象身份与删除状态。若确认期间已有
  更晚 revision，则接受能证明该回执已被后续规范状态覆盖的结果，不要求最新 meta
  仍指向旧 mutation。
- HTTP 409 转为 typed revision conflict，不自动重试旧 Human intent。只有 Applied
  才使 React Query 中当前 Community 的可信快照失效并重取；冲突时当前打开表单中的输入
  保持不变。

### 初始化与对象维护

- Uninitialized View 新增专用初始化流程，一次收集完整 Project Profile 与 1–32 个
  初始 Goal，并在提交前提供整体 Review；一次 mutation 原子建立合法 View，不产生只有
  Profile 或没有 Goal 的中间状态。
- Ready View 新增全局 Add、Inspector Edit/Delete，以及 Goal→Plan、Plan→Stage、
  Stage→Requirement/Issue、Requirement/Issue→Work、Roles 和 Resources 的上下文 Add。
  上下文只预填关系，Human 提交前仍可检查和修改。
- 九类对象均使用业务字段表单；Plan、Stage、Requirement、Issue、Work 使用闭集
  status，Requirement/Issue/Work 使用闭集 priority。关系选择器只列出合法 active
  类型，并显示规范结构路径；Stage 与 Work 的必选关系不能清空，Plan、Requirement、
  Issue 的可选关系可显式解除。
- 删除前从当前可信 View 扫描全部入向关系并列出引用来源；有引用时禁止提交且不级联。
  Project Profile 永不可删除，最后一个 Goal 也在客户端直接阻止；无引用对象需要显式
  确认后才提交 tombstone mutation。

### 验证

- Tauri 新增 7 个 mutation 定向测试，其中真实 HTTP fixture 覆盖原子初始化、对象
  Create 后的签名 projection 确认、规范回执和 409 单次提交；Tauri 全量测试
  1,639 项通过、14 项真实系统钥匙串测试按约定忽略，另有 3 项 diagnostic test 通过。
- Desktop 3,493 个 unit test 全部通过；Project View model/serialization 定向测试覆盖
  规范路径、入向引用扫描、typed writable object 和可选关系的显式 clear。
- Project View 8 个 Playwright smoke 全部通过，覆盖原子初始化、上下文 Stage 创建、
  冲突不重试且保留当前表单输入、引用阻断删除、无引用确认删除，以及 ready、
  unsupported、forbidden 状态。
- Desktop typecheck、Biome、文件大小、可缩放文本、公钥展示守卫和 Tauri
  `clippy --all-targets -D warnings` 均通过。

### 范围边界

- 本 Slice 完成初始化和全部 v0 对象的类型化 Create/Edit/Delete；没有增加数据库读取、
  UI 专用 HTTP endpoint、CLI 子进程或第二份权威状态。
- projection 实时订阅、断线恢复、Community 切换后的主动重确认、跨刷新/关闭的草稿
  保存、最新 revision 对比与显式重基线提交，以及 Human/Agent 身份名称解析仍属于
  Client Slice 3。

## 2026-07-27 — Client Slice 1：可信读取与 View 页面

### 客户端可信边界

- Desktop 新增单一 `get_project_view` Tauri 读命令。命令先读取 NIP-11
  `supported_extensions` 与规范小写 `self`，只信任该 Relay identity 签名的
  `40903`/`40904` projection；前端不接收或自行解释原始事件。
- 快照读取复用 CLI 已建立的协议约束：先读 meta，再以
  `project_revision + projection_generation` 扩展过滤器分页读取 active object，
  逐事件验证签名、signer、project、generation、revision、严格游标顺序、对象 ID
  唯一性与 active count，最后重读同一 meta。快照竞争最多有界重试三次，仍不稳定则
  不展示混合 revision。
- 验证后的对象先经 `ProjectViewState::from_snapshot()` 复核完整领域不变量，再由
  `ProjectView::assemble()` 生成唯一规范层级。错误 signer、tombstone 混入、重复 head、
  缺失关系目标和不一致计数都 fail closed。
- Native command 明确返回 `unsupported`、`forbidden`、`uninitialized` 和 `ready`
  四态；TypeScript 边界只负责 snake_case DTO 到 camelCase UI model 的机械转换，并再次
  拒绝 outer/inner object type 不一致。
- React Query key 包含当前 Community ID；本 Slice 只在窗口重新获得焦点时重取完整可信
  快照，没有增加 module-level Community cache，也没有直接访问数据库或调用 CLI
  子进程。

### View 页面

- 新增预览入口 `View` 与 `/view` 路由，位置在 `Pulse` 和现有 `Projects` 之间。
  `Projects` 的名称、路由、目录和行为保持不变，避免本阶段引入 Git Repository
  概念迁移。
- 只读页面呈现 Project Profile、由明确状态推导的 Current Focus、唯一规范
  `Goal → Plan → Stage → Requirement/Issue → Work` 地图、Unbound Plan、
  Unplanned Requirement/Issue、Roles 和 Resources；未归属对象不会因无法放进主树而
  消失。
- Issue 完整对象只出现在其规范 planned/unplanned 位置；`about` 关系仅在目标对象卡片和
  Inspector 中显示轻量引用，没有复制第二份 Issue 状态。
- 任意对象可打开响应式 Object Inspector；选择写入 `/view?object=<uuid>`，因此前进、
  后退和页面内关系跳转可恢复。Inspector 展示 typed 正文、关系目标、object/project
  revision、规范时间和 actor 公钥。
- 不支持、无权限、未初始化、加载和完整性/网络失败均有独立状态。未初始化页不提供尚未
  交付的初始化按钮；错误页只允许重新执行可信读取。

### 验证

- Tauri Rust 新增 3 个真实 HTTP fixture 测试：完整签名 projection 能验证并组装，NIP-11
  未声明的 signer 会被拒绝，并覆盖 unsupported、uninitialized、forbidden
  状态映射；`cargo clippy --all-targets -D warnings` 通过。
- Desktop 3,492 个 unit test 全部通过；新增 normalization、规范对象索引、Current Focus
  与 `/view`/`/projects` 路由区分回归测试。
- 新增 4 个 Playwright smoke 测试，覆盖侧栏进入 View、规范地图、未规划区域、URL
  Inspector、现有 Projects 入口保留，以及 unsupported、uninitialized、forbidden
  Community 状态。
- Desktop typecheck、Vite E2E build、Biome、文件大小、可缩放文本和公钥展示守卫通过。

### 范围边界

- 本 Slice 完成的是 Human 可用的可信只读 Project View，不包含初始化、Create/Update/
  Delete、关系选择器、引用感知删除或 revision conflict 表单；这些属于 Client Slice 2。
- 本 Slice 不订阅 projection 实时变化，也不处理编辑草稿与并发冲突；Human/Agent
  修改后的实时恢复、断线重连与来源展示属于 Client Slice 3。
- Current Focus 只汇总领域对象的显式 status，不引入“当前 Stage”、隐式状态推进、排序或
  Kanban 语义。

## 2026-07-27 — Slice 5：CI、可观测性与发布

### 专用质量门

- 新增 `project-view-test-unit`、`project-view-test-db`、
  `project-view-test-e2e`、`project-view-test` 与 `test-migrations` Just
  recipes。`test-unit` 和无 nextest 的 `scripts/run-tests.sh unit` 都显式覆盖领域
  crate、kind registry、SDK、Relay adapter 与 CLI；领域 crate 的 property、关系和
  wire integration targets 不会被 `--lib` 误过滤。
- 新增隔离 PostgreSQL 脚本。DB 测试各自创建/删除 scratch database；migration gate
  使用另一个精确命名的临时数据库，执行 fresh、0024→0025、ledgerless schema、
  populated upgrade、并发 migrator，并用 `pgschema plan` 阻断 migration 25 与
  `schema/schema.sql` 的 Project View 漂移。
- Project View E2E 现在启动独立数据库和 Relay，使用 packaged
  `buzz-admin project-view enable` 开启中心 DB flag，并由真实 `buzz` 子进程完成一次
  typed create。测试继续覆盖 NIP-11 signer/capability、WS/HTTP、COUNT、revision-pinned
  pagination、冲突、projection 签名、mixed query 隐藏和 live membership 撤权。
- 新增真实 pre-feature rollback smoke：CI 固定从 `ab3af828` 构建 Project View
  出现前的 Relay，在已由当前 `buzz-admin` 迁移到 25、全部开关为 false 的数据库上以
  `BUZZ_AUTO_MIGRATE=false` 启动，验证 readiness 与既有 NIP-11 路径。该测试不会用
  当前 Relay 模拟旧版本。
- 新增 post-mutation compatible rollback smoke：先由当前 Relay 接受初始化，再用同一
  migration 25 数据库启动固定 `8ef125c1`（Slice 4）Relay，验证 capability、稳定
  signer、revision 1 完整快照和 Project View 专属非成员拒绝。由此分别覆盖首次
  mutation 前的 pre-feature 回滚与已有数据后的兼容回滚边界。

### CI 与制品

- backend nextest archive 加入 `buzz-db`、`e2e_project_view`、`buzz` 和
  `buzz-admin`；独立 Project View integration job 顺序执行 DB transaction、
  migration/schema drift 与真实 Relay/CLI E2E。rollback job 使用固定 pre-feature
  与 compatible 源码 binary cache，避免用当前 binary 模拟旧版本，也避免把 ignored
  migration tests 留在非必经路径。
- Docker PR path filter 新增 `migrations/**` 与 `schema/**`。Relay release
  `LOG_PATHS` 新增 Project View crate、CLI/admin、schema、协议/运维文档、Chart、
  Compose 和专用测试脚本。
- Sprig archive 新增实际 `buzz -> sprig` symlink 与 manifest entry；Sprig workflow
  增加 CLI/SDK/Project View 相关 PR 构建路径，使 managed Agent 拿到与 Relay
  capability 对应的 typed CLI。
- 新增 package/deploy contract，静态验证 Relay image 含 `buzz-admin`、CI archive
  含真实 CLI/admin/E2E、完整 metrics 名称、Chart 使用稳定 signer，并禁止 Chart 或
  Compose 引入 Pod-local `BUZZ_PROJECT_VIEW_ENABLED`。

### 可观测性与运维

- 新增八组低基数指标：mutation count/duration/conflict、snapshot
  duration/revision retry、按闭集 type 的 active object gauge、projection dispatch
  error 与 schema readiness。operation、result、type、reason 都来自闭集，不使用
  Community/object/event ID 作为 label。
- mutation 结果日志包含 `community_host`、command/actor 坐标、operation、
  object type/id、expected/committed revision 与 result code；正文、patch、title 和
  Resource locator 不进入普通日志。
- 新增 `docs/project-view-operations.md`，固化 server-first 顺序：全部 Pod 先以
  auto-migrate false 升级，再运行 migration 25，验证 schema/signer/read gate 后由
  admin 开启。Runbook 同时记录 disable、诊断 SQL、告警信号、signer rotation、
  首次 mutation 前与之后两种不同的回滚边界；Chart/Compose 文档与 Helm NOTES
  指向同一流程。

### 验证

- `just project-view-test-unit`：59 项 Project View 定向测试通过；其中领域层 36 项，
  关系设计 21 条清单全部自动测试。
- `just project-view-test-db`：14 项隔离 PostgreSQL 测试通过。
- `just test-migrations`：6 项 migration 测试与 Project View schema drift gate
  通过。
- `just project-view-test-e2e`：真实 Relay、admin 与 CLI E2E 通过。
- 固定 `ab3af828` pre-feature Relay + migration 25 rollback smoke，以及当前 Relay
  写入后切换到固定 `8ef125c1` compatible Relay 的 post-mutation smoke 均通过。
- 实际构建 Sprig archive，确认包含 `buzz -> sprig` 和 manifest entry。
- `just test` 13 组 unit/integration tests 全部通过；仓库级 `just ci` 全部通过。
- 受影响 Rust packages 的 `cargo check --all-targets`、workflow YAML 解析、
  shell syntax、release contract 与 `git diff --check` 通过。

## 2026-07-27 — Slice 4：Typed SDK 与 Agent CLI

### SDK command 与 projection 契约

- `buzz-sdk::project_view` 新增 `build_initialize`、`build_create`、`build_update`、
  `build_delete`。Builder 只接受领域层 typed input，不接收 project/community ID，
  并固定生成 `44300` 的精确 `-`、`t` tags。
- 领域层新增不依赖当前服务端状态的 submission validation：在签名前检查 revision safe
  range、UUID v4、初始 Goal 基数与去重、字符串/list/locator 限制、required-field
  `null`、空 patch、Issue 自引用和 Work target 类型；Relay 仍在归约时重复校验当前状态、
  CAS、对象存在性和关系目标。
- 新增 `parse_meta_projection`、`parse_object_projection` 与
  `verify_projection`。读取端验证事件签名、NIP-11 Relay signer、kind、精确 tag 顺序、
  规范 UUID/hex/RFC3339/decimal、content/tag/coordinate 一致性、revision/generation
  范围，以及 reset/source 和 active/tombstone 互斥；未知 projection 可选 content
  字段保持向前兼容，tombstone 明确拒绝业务正文。

### `buzz project-view` Agent 操作面

- 新增 `get`、`get-object`、`init`、`create`、`update`、`delete` 六个子命令。
  Profile、Goal、create data 与 update patch 均从 JSON file/stdin 进入 typed
  deserialization；Create 的对象 UUID 在签名前由 CLI 生成，调用者不能覆盖
  `id`/`object_type`。
- 所有命令先读取 NIP-11 并要求 `buzz-project-view-v1` 与规范 `self` signer。Project
  View mutation 使用 closed-tag 精确签名；NIP-OA 仍通过独立 `x-auth-tag` header
  传递，不污染 mutation tags。网络重试复用同一 signed event bytes。
- `get` 先验证 meta，再按 `(generation, revision)` 使用 `/query` 扩展分页读取 active
  heads，检查排序、唯一 ID、数量、generation/revision 和全部 projection，最后重读
  同一 meta 后才交给 `ProjectView::assemble`。并发变化触发有界重试，无法取得一致快照
  时返回 conflict，不输出混合 revision。
- `get-object` 使用规范 `d` coordinate、`limit:2` 和 Relay signer 做 point read，
  同时支持 active object 与 tombstone。写命令保留调用者显式
  `expected_project_revision`，成功后回读 meta/object 确认；HTTP `409` 映射
  `CliError::Conflict` 与 exit code `5`。
- 默认 `get` JSON 一次返回 project、Goals/Plans/Stages、未规划 Requirements/Issues、
  Roles、Resources 和 Issue reverse references；未初始化状态明确输出
  `initialized:false`、revision `0` 与空集合。全局 `--format compact` 保留同一逻辑
  结构，但移除每个对象的 provenance/revision 冗余字段。

### 验证

- SDK 协议测试覆盖四类 mutation builder、projection round-trip、错误 signer、未知可选
  字段和 tombstone 正文拒绝。
- CLI HTTP integration test 使用真实 `BuzzClient` 与本地 Axum bridge，覆盖
  meta → revision-pinned page → meta 的一致快照组装，以及 revision conflict 的进程
  exit `5`；命令 inventory 与 typed input/null semantics 同步受单测保护。
- `cargo clippy -p buzz-project-view -p buzz-sdk -p buzz-cli --all-targets -- -D warnings`
  通过；`cargo test -p buzz-cli commands::project_view --lib` 9 项测试全部通过；
  仓库级 `just ci` 全部通过。

## 2026-07-27 — Slice 3：Relay 原生协议接入

### 写入、签名与原子提交

- 将 kind `44300` 接入 WebSocket `EVENT` 与 HTTP `POST /events` 的统一 ingest
  管线。命令必须具备协议规定的精确 tags，并依次通过全局凭证、`MessagesWrite`
  scope、当前 Community 成员身份与 ban 检查。
- Relay 在回执前完成 mutation 解析、领域归约、projection 规划与稳定密钥签名，再由
  `ProjectViewWriteTx` 原子提交 command、receipt、规范状态和全部新 head；幂等重放
  返回既有 receipt，不重复分配 revision 或 fan-out。
- 新增 SDK projection builder，严格构造 object/meta projection 的 kinds、坐标、tags
  与 content；数据库边界再次验证覆盖集合、事件签名、稳定 signer、revision 和
  generation，避免把内部自洽但不对应命令的投影写入规范状态。
- Project View command 只进入 command audit，不触发通用 workflow；Relay 生成的
  projection 不重复进入 audit/workflow。kind `40903`、`40904`、`44300` 使用显式的
  有界 metrics 标签。

### 统一读取、分页与实时撤权

- WS `REQ`/`COUNT` 与 HTTP `/query`/`count` 共用严格 reader gate：允许当前 Relay
  member，或 owner 仍是当前 member 的 persisted managed agent；actor ban 总是拒绝，
  owner ban 在 managed-agent 路径拒绝，且全部查询以 Community 为租户边界。
- 未授权的 Project View-only filter 明确拒绝；mixed filter 在 SQL `LIMIT`/COUNT 和
  普通查询前排除 Project View kinds，防止分页与数量侧信道；NIP-50 的现有正向索引
  allowlist 不包含这些 kinds，并在返回端继续 fail closed。授权后的返回结果仍执行末端
  防御检查。
- HTTP `/query` 支持 revision/generation 固定的 active snapshot 分页。首屏返回规范
  游标，续页必须携带相同 revision、generation 与 canonical cursor；并发 mutation
  后的旧游标返回 `409 Conflict`，不会拼接跨 revision 页面。
- 本机与 Redis 跨 pod 的 live fan-out 都在实际发送 chokepoint 重新查询当前授权。
  reader 被移出 Community 或被 ban 后，无需重连即可停止接收后续 projection。

### Capability 与 signer rotation

- NIP-11 仅在 Community 显式开启、数据库 schema 完整、稳定 signer 已配置且规范状态
  可读取时声明 `buzz-project-view-v1`；deployment readiness 只在至少一个 Community
  开启 Project View 时要求全局前置条件。
- `buzz-admin project-view enable` 改为 checked enable，在持锁状态下验证 schema、
  signer 与完整性；`disable` 保持无需私钥。
- 新增 `project-view reproject --community|--all --expected-pubkey
  [--relay-key-file]`。operator 轮换顺序固定为先 disable，再运行只允许 disabled 状态的
  重签，验证后显式执行 checked enable；重签只递增 projection generation，不改变
  project revision，并原子退休全部旧 head。
- 私钥不接受 argv 传入；key file 必须是普通文件，Unix 下拒绝 group/world 权限。
  `--all` 在写入前先验证全部目标均 disabled，避免只完成部分 Community。

### 验证

- `cargo test -p buzz-sdk -p buzz-project-view`：SDK 237 个测试、领域层 36 个测试通过。
- Relay Project View handler/filter 测试与 admin 测试通过；两个真实 PostgreSQL
  integration test 通过，覆盖成员/managed-agent reader gate、ban、tombstone 重签、
  generation CAS、旧 signer 拒绝和新 signer checked enable。
- 新增真实 Relay 协议 E2E，并在本地 Postgres/Redis 上显式运行通过：覆盖 WS/HTTP
  写入与读取、projection 签名、NIP-11 capability、COUNT、revision-pinned pagination、
  stale mutation/page `409`、mixed historical 隐藏，以及 membership 撤销后的 live
  fail-closed。

### 范围边界

- 本阶段完成 Relay、数据库、SDK projection 与 operator rotation 闭环；面向人和 Agent
  的 typed `buzz project-view` CLI 读写命令、客户端 read-model 组装及契约化输出属于
  Slice 4。

## 2026-07-27 — Slice 2：数据库规范状态与原子写事务

### Migration 与规范状态

- 新增 additive migration `0025_project_view.sql`，并同步
  `schema/schema.sql`；已有和新建 Community 的
  `project_view_enabled` 均默认 `false`，迁移不回填 Project View 对象，也不改写
  `events` 大表。
- 新增 `project_view_state`、`project_view_objects` 和
  `project_view_mutations`。全部领域表、主键、唯一约束、外键和索引都以
  `community_id` 为租户前缀，没有加入 operator-global allowlist，也没有
  `ON DELETE CASCADE`。
- 对 revision safe range、schema version、32-byte ID/pubkey、Profile identity、对象类型、
  relation shape、active body 与 tombstone 空正文增加数据库 CHECK；对象行禁止 hard
  delete 和 tombstone 复活。
- ordinary trigger 只按 active insert / active→tombstone 对
  `active_object_count` 做机械 `+1/-1`。deferred trigger 使用主键和关系索引验证最终
  Profile/Goal 聚合、变化对象的 typed active target，以及 tombstone 的 active 入向引用；
  mutation 提交不运行全表 `COUNT(*)`。
- migration 固定数量更新为 `25`，同时增加 tenant-key 静态断言、0024→0025 实际升级测试，
  以及不含 `_sqlx_migrations` ledger 的 `schema/schema.sql` 从零建库测试。

### 原子写路径

- 新增 `buzz-db::project_view::ProjectViewWriteTx`。写事务先取得按 Community 派生的
  exclusive advisory lock，再从 writer DB 读取中心开关；未开启或已归档 Community
  fail closed。
- 状态行使用 `FOR UPDATE`，project revision 使用 CAS；canonical time 来自数据库，并以
  `max(clock_timestamp(), previous + 1µs)` 保证随 revision 单调。
- `load_current()` 的纯领域基线保存在事务内部。提交前 DB 层重新运行同一 typed mutation，
  要求得到的 next state 和 changed entries 与 prepared bundle 完全一致，避免调用方把
  “内部自洽但不对应签名命令”的状态写入数据库。
- accepted command event、幂等 receipt、state、canonical object/tombstone、object
  projection、meta projection 和旧 head 退休全部进入同一个 SQL transaction；任一步
  失败即整体回滚。
- event store 抽取 caller-owned transaction helper；普通 `insert_event()` 继续走同一
  字段与校验实现。Project View head 只按 state/object 保存的精确 event ID 和预期 kind
  soft-retire，不复用 NIP-33 的作者/时间戳 replacement。
- 重试先命中 `(community_id, event_id)` durable receipt，不分配新 revision，也不重复
  fan-out；同 expected revision 的并发写在 Community lock/CAS 下恰好一个成功。

### Operator 控制面

- 新增 `buzz-admin project-view status [--community <host>]`，展示中心开关、归档状态、
  revision、projection generation 和 signer pubkey。
- 新增 `enable|disable --community <host>|--all`。单 Community 和全量操作复用 mutation
  advisory lock；`--all` 按 UUID 稳定顺序取锁，只更新非归档 Community，且不在 argv
  接受任何私钥。

### 验证

- `cargo test -p buzz-db`：87 个无基础设施测试通过，132 个基础设施测试保持 ignored。
- 8 个 Project View 临时数据库 integration test 已显式执行并通过，覆盖初始化/幂等、
  projection 失败全回滚、prepared bundle 推导校验、并发 CAS、tenant key 与跨
  Community 引用、tombstone/count/head retirement、最后 Goal deferred guard 和中心开关。
- 0024→0025 升级与 ledger-less `schema/schema.sql` 建库测试均在独立临时数据库通过；
  临时数据库已清理。
- `cargo test -p buzz-project-view`：36 个领域、关系、wire 与属性测试通过。
- `cargo test -p buzz-admin` 和
  `cargo clippy -p buzz-db -p buzz-admin --all-targets -- -D warnings` 通过。
- `just test`：core、auth、db、conformance、project-view、push-gateway 和 workspace
  integration 共 9 个测试组通过。

### 范围边界

- kind `44300` 仍未接入 Relay ingest；成员写安全门、统一读门禁、实际 projection
  builder/signing、NIP-11 capability、post-commit fan-out 和 reproject 属于 Slice 3。
- 当前写事务用一次 set-based object query 重建完整纯领域状态，因此没有 N+1，并能在
  DB 边界复核全状态；设计中的 mutation-targeted loader 和 10k 规模性能门在 Relay
  接入前继续收敛。

## 2026-07-27 — Slice 1B：协议与 indexed d-tag 基础

### 协议

- 新增 `docs/nips/NIP-PV.md`，冻结 mutation、object projection 和 meta projection
  的 kind、签名者、精确 tags、content、revision/generation、读取与实时一致性语义。
- 固定 `44300` 为成员签名 append-only mutation，`40903`/`40904` 为 Relay 签名的
  object/meta current-state projection。
- 实现前复核 NIP-01 和官方 `registry-of-kinds`；截至本次交付，三个值均未发生外部登记
  冲突。
- 明确 signer rotation 产生的 `reset: true` meta 没有成员 command source，因此省略
  source `e` tag 和 `source_event_id`；普通 `reset: false` meta 两者仍为必填且必须一致。

### 实现

- 在 `buzz-core::kind` 注册三个 kind 并纳入重复值检查；`40903`/`40904` 同时进入
  Relay-only classifier，客户端在专用 handler 尚未实现时也不能写入 projection。
- 新增共享 `has_indexed_d_tag()`：保留 `30000..=39999` 的既有行为，并只额外识别
  `40903`/`40904`。没有扩大 `is_parameterized_replaceable()`，因此 Project View
  projection 不会误走 NIP-33 的作者/时间戳替换规则。
- `buzz-db::event::extract_d_tag()` 改用共享 classifier；Project View object/meta 坐标会
  写入 `events.d_tag`，mutation 和其他普通 40xxx kind 仍保持 `NULL`。
- WS REQ、HTTP `/query` 与 WS/HTTP COUNT 共用的 filter builder 改用同一 classifier。
  只有显式、非空且全部可索引的 kinds 才会在 SQL `LIMIT` 前下推单值或多值 `#d`；
  mixed-kind 与 kindless filter 继续安全回退。

### 范围边界

- `44300` 仍是 Relay ingest 的 unknown/rejected kind；本阶段没有接入 mutation handler、
  scope 或 capability。
- 没有新增 migration、Project View 状态表、projection transaction 或 NIP-11 宣告。
- 新增一个需 Postgres 的 ignored 回归测试，覆盖“同 kind 新行超过 limit、目标 head
  较旧”时 point read 仍能由 SQL `d_tag` 谓词精确命中；默认测试集继续忽略该用例，
  本次交付已在迁移后的真实 Postgres 上显式执行。

### 验证

- `cargo test -p buzz-core`：231 个单元测试与 2 个文档测试通过。
- `cargo test -p buzz-db`：84 个无基础设施测试通过；Postgres point-read 回归测试另行
  显式执行并通过。
- `cargo test -p buzz-relay handlers::req::tests`：47 个 REQ/filter 测试通过。
- `cargo clippy -p buzz-core -p buzz-db -p buzz-relay --all-targets -- -D warnings`
  通过。
- `just test`：core、auth、db、conformance、project-view、push-gateway、数据库及
  workspace integration 共 9 个测试组通过。

## 2026-07-27 — Slice 1A：纯领域模型

### 实现

- 新增零 I/O crate `buzz-project-view`，并接入 workspace 与 infra-free unit test gate。
- 实现九类 Project View 对象、闭集状态枚举、强类型关系、统一 active object 信封与
  tombstone。
- 实现 schema v1 的 Initialize、Create、typed Update、Delete mutation；所有写入使用
  project revision CAS，失败时内存状态保持不变。
- 实现字段、UUID、revision、关系目标、删除保护和最终聚合状态校验。
- 实现完整确定性 read model，包括 Unbound Plan 子树、Unplanned
  Requirement/Issue 下的 Work，以及按目标分组的 Issue 反向引用。
- 新增 mutation JSON 大小、嵌套深度和封闭 schema 解析入口。

### 首版解释

- 延续后端实现设计：Stage 不承载业务顺序；无显式顺序时仅按
  `(created_at, id)` 稳定输出。
- 延续后端实现设计：Work 的 `handles` 可以显式替换，但不能清空。
- 延续后端实现设计：Delete 生成 tombstone，ID 永久不可复用；删除前仍须先解除全部
  active 入向引用。
- 必填字符串以 `trim()` 结果判断是否为空，但规范值保留调用方原始文本，不隐式改写。
- 在 Nostr address/event 与 Buzz deep-link 的更细语法尚未由协议文档固定前，领域层只
  校验它们非空、长度与控制字符；URL 另外拒绝 userinfo 和密码。

### 验证

- 关系设计中的 21 项清单使用一一对应的具名契约测试。
- mutation wire、patch 三态、字段边界、ID/tombstone 与 safe revision 使用示例和边界
  测试。
- 属性测试覆盖合法 mutation 序列不变量、非法 mutation 原子性、旧 revision 重放、
  snapshot 输入排列无关、Issue `about` 环和确定性重放。

### 尚未包含

- `docs/nips/NIP-PV.md`、event kinds 与 indexed `d` tag classifier。
- PostgreSQL、Relay、projection、SDK、CLI 与发布接入。
