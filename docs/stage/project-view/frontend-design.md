# Project View 客户端 v0 设计

> 版本说明（2026-08-07）：本文保留客户端 v0 的产品与交互背景；其中旧 schema 的读取、
> 写入和 fallback 不再适用于当前实现。Desktop 普通运行时只接受 Project View v3。见
> [Project View 普通运行时全面收敛到 v3](../bug/project-view-v3-only-runtime-migration-fix-design.md)。

> 本文定义 Project View 首个 Human 客户端的产品边界、信息结构、主要交互和交付标准。
> 首版以 Buzz Desktop 为目标客户端，不深入规定组件拆分、状态管理、接口函数、缓存实现
> 或具体视觉参数；这些细节在实现阶段结合现有客户端架构决定。

## 1. 文档目的

[项目视图定义](project-view.md)已经说明 Project View 要让 Human 或 Agent
直接回答哪些一阶问题，[基本对象与关系设计](object-relation-design.md)固定了首版
对象、关系与基数，[后端实现设计](backend-implementation-design.md)则已经交付同一份
状态的事件协议、Relay 当前态投影、SDK 和 Agent CLI。

Agent 目前可以通过 `buzz project-view` 读取和修改 Project View，但 Human 尚无可用的
图形客户端。本文继续回答：

1. Human 应从哪里进入 Project View；
2. 一幅完整 Project View 应如何组织和阅读；
3. Human 如何初始化、创建、编辑、移动和删除对象；
4. Human 与 Agent 并发工作时，客户端如何表达变化和冲突；
5. 哪些能力属于客户端 v0，哪些继续留待后续。

本文不重新定义领域对象，也不建立独立于现有后端的客户端状态模型。

## 2. 客户端 v0 的目标

客户端 v0 需要让当前 Buzz Community 的成员能够：

- 在进入页面后快速理解项目是什么、为什么存在以及当前目标；
- 顺着 Goal、Plan、Stage、Requirement、Issue 和 Work 阅读项目的规划与执行结构；
- 明确看到未关联 Goal 的 Plan，以及尚未规划的 Requirement 和 Issue；
- 查看项目的 Role 与 Resource；
- 查看任一对象的完整内容、直接关系、状态、优先级和修改来源；
- 通过类型化表单初始化和维护 Project View；
- 在 Agent 或其他成员同时修改项目时看到变化，并安全处理 revision conflict；
- 始终读取经过 Relay 签名验证且 revision 一致的项目状态。

客户端 v0 的价值不是复刻一个通用项目管理工具，而是让 Human 与 Agent 第一次可以
共同查看和维护同一幅项目直接状态。

## 3. 首版不做什么

以下能力不属于客户端 v0：

- 项目上下文、上下文检索或上下文编译；
- Human 或 Agent 对 Role 的承担与 assignment；
- Role 与权限、Work 或 Executor 的关系；
- Goal、Plan 或 Stage 的自动完成判断；
- 状态自动级联；
- Goal 优先级、Goal 依赖或跨对象通用关系图；
- Stage 排序、拖拽编排或唯一“当前阶段”；
- Kanban、甘特图、时间线和容量规划；
- 批量修改、批量删除或批量导入；
- 完整变更历史与审计浏览器；
- 自动把现有 Buzz `Projects` 同步为 Project View Resource；
- Web 和 Mobile 客户端实现。

这些能力只有在真实使用 Project View 后出现明确需求时，才进入后续设计。

## 4. 核心设计结论

客户端 v0 采用以下结论：

1. **首个 Human 客户端是 Buzz Desktop。**
   Desktop 已经持有成员身份、Community 边界和 Relay 连接，也能够复用现有 Rust
   domain 与 SDK 验证逻辑。
2. **用户入口名称为 `View`。**
   `View` 表示当前 Community 的 Project View，首版路由为 `/view`。
3. **现有 `Projects` 保持不变。**
   当前 `Projects` 继续表示 Buzz 已有的 Git/NIP-34 项目能力，不改名、不迁移、不复用
   其页面和路由。
4. **一个 Community 只有一幅 View。**
   用户不在 View 中选择 Project；切换 Community 就是切换 Project。
5. **Human 与 Agent 使用同一份后端状态。**
   客户端不建立第二份项目文档，也不通过 CLI 子进程维护旁路状态。
6. **主界面是一幅可阅读的项目地图。**
   页面按照项目意图到实际执行的顺序组织，而不是把九种对象拆成九个管理后台。
7. **修改使用对象类型专用交互。**
   Human 不直接编辑 JSON，也不需要了解 event kind、projection 坐标或 UUID。
8. **所有状态和关系变动保持显式。**
   客户端不根据 Work、Issue 或 Stage 的变化自动修改其他对象。
9. **并发冲突必须对 Human 可见。**
   客户端不得静默替换用户编辑所基于的 project revision，也不得在冲突后自动覆盖
   Agent 或其他成员的修改。
10. **客户端不展示未验证或混合 revision 的状态。**
    Relay projection 的到达顺序不能成为 UI 正确性的依据。

## 5. 命名与现有 Buzz 功能

### 5.1 用户可见名称

首版侧栏保持以下概念：

```text
Inbox
Pulse
View          当前 Community 的 Project View
Projects      现有 Git/NIP-34 项目
Agents
Workflows
```

进入 `View` 后，页面主标题使用 Project Profile 的项目名称。`View` 是产品入口，不应
取代项目自身的名称。

例如：

```text
View / Buzz
```

页面空间不足时，也可以只显示项目名称，并通过导航选中状态表达用户当前位于 View。

### 5.2 内部概念

产品入口虽然显示为 `View`，领域、协议和内部代码概念仍称为 `Project View`，避免与
普通 UI view 或现有 `Projects` 混淆。

现有 `Projects`：

- 不参与 Project View 的身份判断；
- 不作为 Project View 的隐含根对象；
- 不因本阶段发生路由、数据模型或页面行为变化；
- 未来可以通过明确的 Resource locator 与 Project View 关联，但首版不自动建立关系。

## 6. 体验原则

### 6.1 先让项目可理解

用户进入 View 后，应首先看到项目名称、定位、目的、问题、范围和目标，而不是先看到
对象类型选择器、空表格或技术元数据。

### 6.2 直接状态优先

View 只呈现已经明确登记的对象、关系、状态和优先级。客户端可以对这些字段进行汇总，
但不能把推断结果伪装成规范项目状态。

### 6.3 保留规范位置

每个对象只在 read model 规定的规范位置完整出现：

- Plan 出现在关联 Goal 下，或“未关联 Goal 的 Plan”区域；
- Requirement 和 Issue 出现在其 Stage 下，或对应的“未规划”区域；
- Work 出现在它实际处理的 Requirement 或 Issue 下；
- Issue 的 `about` 只形成交叉引用，不改变 Issue 的规范位置。

### 6.4 不把视觉结构变成新语义

首版模型没有定义 Goal、Plan 或 Stage 的人工排序，因此客户端：

- 不显示“Stage 1、Stage 2”之类隐含顺序；
- 不提供拖拽排序；
- 不把列表第一项解释为优先项；
- 不假设只能存在一个 Active Plan 或 Active Stage。

### 6.5 显式处理异常状态

未规划、未关联、冲突、离线、无权限和完整性失败都必须具有明确界面，不得退化为空白
页面、静默忽略或显示部分结果。

### 6.6 Human 与 Agent 对等

Human 创建的修改和 Agent 创建的修改使用相同领域约束和并发规则。客户端可以把公钥
解析为 Human 或 Agent 的显示身份，但不能因修改来源不同而改变状态含义。

## 7. 导航与页面边界

### 7.1 Community 范围

`View` 始终属于当前激活的 Community：

- 切换 Community 后加载目标 Community 的 View；
- 不在页面中提供 Project 切换器；
- 不允许旧 Community 的对象、草稿或实时事件泄漏到新 Community；
- 返回原 Community 时重新确认其最新 revision。

### 7.2 Capability 与初始化

客户端根据当前 Relay 的 Project View capability 区分：

- Relay 不支持 Project View；
- 当前 Community 支持但尚未初始化；
- 当前 Community 已初始化；
- 当前身份无权读取。

`View` 在不支持 Project View 时究竟隐藏还是显示为不可用入口，可以在实现阶段结合
Feature Gate 和升级体验决定；直接访问 `/view` 时必须返回明确的不可用说明。

### 7.3 对象定位

从列表、Issue 引用或未来其他 Buzz 页面打开一个 Project View 对象时，应进入同一
View 页面并选中该对象，而不是创建另一套对象详情页面。

具体 URL 查询参数和 deep link 形式在实现阶段决定，但浏览器式前进、后退和可恢复
定位应当成立。

## 8. 页面信息结构

首版采用“一幅主视图 + 对象详情面板”的结构：

```text
┌──────────────────────────────────────────────────────────────────┐
│ Project name                         revision / connection state │
│ positioning · purpose · problem · scope              Edit · Add │
├──────────────────────────────────────────────────────────────────┤
│ Current focus：Active Plan / Stage、进行中 Work、开放 Issue 等  │
├───────────────────────────────────────────┬──────────────────────┤
│ Project map                               │ Object inspector     │
│                                           │                      │
│ Goal                                      │ 完整正文             │
│ └── Plan [Active]                         │ 状态与优先级         │
│     └── Stage [Active]                    │ 直接关系             │
│         ├── Requirement [Ready]           │ 修改者与时间         │
│         │   └── Work [In progress]        │ Edit / Delete        │
│         └── Issue [Open]                  │                      │
│             └── Work [Pending]            │                      │
│                                           │                      │
│ Unbound plans / Unplanned items           │                      │
├───────────────────────────────────────────┴──────────────────────┤
│ Roles                                                            │
│ Resources                                                        │
└──────────────────────────────────────────────────────────────────┘
```

这张结构图表达内容层级，不固定最终像素布局。详情面板可以根据窗口宽度变成抽屉或独立
内容区域。

### 8.1 Project Profile

页面顶部展示：

- 项目名称；
- 定位；
- 存在目的；
- 所解决的问题；
- 范围。

Profile 是 Project 的描述面，不应被表现成可删除的普通卡片。

### 8.2 Current Focus

页面可以从显式状态中汇总：

- Active Plan；
- Active Stage；
- In-progress Work；
- Open 或 In-progress Issue；
- Ready 或 In-progress Requirement；
- High 或 Urgent 的 Requirement、Issue 和 Work。

Current Focus 只是帮助 Human 快速定位的读取摘要：

- 不形成新对象；
- 不保存独立状态；
- 不选择唯一当前 Plan 或 Stage；
- 点击摘要项应回到对象的规范位置。

### 8.3 Project Map

Project Map 是页面主体，按以下顺序组织：

```text
Goal
└── Plan
    └── Stage
        ├── Requirement
        │   └── Work
        └── Issue
            └── Work
```

对象的卡片或行首先呈现：

- 易识别的名称或标题；
- 对象类型；
- 显式状态；
- Requirement、Issue、Work 的优先级；
- 是否存在相关 Issue；
- 最近修改来源和时间的轻量提示。

长描述、全部关系和 provenance 放在 Object Inspector 中，避免主地图失去可扫描性。

### 8.4 未关联与未规划区域

以下对象必须作为一等内容出现：

- 未关联 Goal 的 Plan；
- 未规划的 Requirement；
- 未规划的 Issue。

区域应显示数量，并允许直接打开对象或把对象放入合法位置。它们不是错误数据，也不能
被自动分配到某个 Goal、Plan 或 Stage。

### 8.5 Roles

Role 区域展示：

- 名称；
- 存在目的；
- 直接职责；
- 责任边界；
- 是否生效。

Role 不显示成员承担者，也不控制编辑权限。界面文案必须避免把 Project Role 与
Community owner、admin、member 等权限角色混为一谈。

### 8.6 Resources

Resource 区域展示：

- 资源名称和类型；
- 描述；
- locator；
- 在客户端能够安全识别时提供打开入口。

Resource 是稳定导航入口，不在 View 中复制仓库、文档、服务或产物的完整内容。

### 8.7 Object Inspector

点击任一对象后，Inspector 展示：

- 完整业务字段；
- 所有直接关系；
- Issue 的 `about` 目标或指向当前对象的 Issue；
- object revision 与 project revision；
- 创建者、最近修改者和相应时间；
- 当前允许的编辑和删除操作。

关系目标应使用对象名称和结构位置表达，UUID 只作为必要时的辅助技术信息。

## 9. Human 操作

### 9.1 初始化

未初始化的 View 显示专用初始化流程，一次收集：

- 完整 Project Profile；
- 至少一个初始 Goal。

提交前允许 Human 检查整体内容。初始化必须作为一个原子动作完成，不能先产生缺少 Goal
的 Profile，也不能在中途留下部分 View。

如果另一个 Human 或 Agent 已经完成初始化，当前流程保留未提交输入并切换到冲突处理，
不得再次初始化或覆盖新状态。

### 9.2 创建对象

页面提供全局 Add 入口，也允许从当前结构位置发起上下文创建：

- 在 Goal 下创建 Plan；
- 在 Plan 下创建 Stage；
- 在 Stage 下创建 Requirement 或 Issue；
- 在 Requirement 或 Issue 下创建 Work；
- 在 Roles 或 Resources 区域创建对应对象；
- 从全局入口创建未关联 Plan、未规划 Requirement 或未规划 Issue。

上下文创建只负责预填合法关系，Human 仍可在提交前检查和修改。

### 9.3 类型化表单

每种对象使用符合其字段的表单：

- Goal：标题、期望结果和方向；
- Role：名称、目的、职责、边界和 active 状态；
- Plan：标题、描述、状态和可选 Goal；
- Stage：标题、描述、状态和必选 Plan；
- Requirement：标题、描述、状态、优先级和可选 Stage；
- Issue：标题、描述、状态、优先级、可选 Stage 和可选 `about`；
- Work：标题、描述、状态、优先级和必选处理目标；
- Resource：名称、类型、locator 和描述。

关系选择器只显示领域允许的目标类型，并以名称、类型和结构路径帮助 Human 判断。客户端
不提供原始 JSON 编辑作为常规交互。

### 9.4 编辑与移动

正文、状态、优先级和关系都通过显式编辑完成。所谓“移动”只是改变已有关系：

- Plan 关联或解除 Goal；
- Requirement 或 Issue 进入或离开 Stage；
- Issue 改变或清除 `about`；
- Work 改变它处理的 Requirement 或 Issue。

Stage 必须始终属于一个 Plan，因此不能被移动到“无 Plan”状态。客户端不通过拖拽隐式
产生关系更新。

### 9.5 删除

删除前显示对象类型、标题和直接影响。客户端应根据当前 View 尽可能列出入向引用：

- 有 Plan 引用的 Goal；
- 有 Stage 引用的 Plan；
- 有 Requirement 或 Issue 引用的 Stage；
- 被 Issue `about` 引用的对象；
- 被 Work 处理的 Requirement 或 Issue。

存在引用时，引导 Human 先移动、解除或删除引用来源。客户端不提供级联删除。

Project Profile 永远不可删除；最后一个 Goal 不可删除。这些约束应直接体现在界面中，
而不是只在提交失败后暴露。

## 10. Human 与 Agent 的实时协作

### 10.1 变化可见

当 Agent 或其他成员修改 Project View 时，已打开的 View 应在合理时间内刷新，并显示
新的 project revision。实时事件只表示权威状态可能变化；客户端最终展示的仍必须是
经过验证的一致快照。

首版不要求把每个 projection event 直接增量应用到 UI。具体采用整幅快照刷新还是经过
验证的原子增量更新，在实现阶段根据复杂度和规模决定，但不得显示混合 revision。

### 10.2 编辑基线

Human 开始一次编辑时，客户端记录这次意图基于的 project revision。即使后台已经收到
更新，也不能在提交前静默把该基线替换为最新 revision。

当基线过期时，客户端：

1. 保留 Human 的表单草稿；
2. 获取最新 View；
3. 明确说明项目由哪个 revision 变化到哪个 revision；
4. 让 Human 检查最新对象和自己的输入；
5. 由 Human 明确选择放弃、继续修改或基于新 revision 重新提交。

### 10.3 Conflict

服务端返回 conflict 时，不把它显示为普通网络错误，也不自动重试写入。界面应说明：

- 项目在当前编辑期间已发生变化；
- 本次修改没有写入；
- 用户草稿仍然保留；
- 需要检查最新状态后再次确认。

即使变化发生在另一个对象上，首版项目级 revision 仍会产生 conflict。客户端可以帮助
Human 判断目标对象是否变化，但不能替 Human 自动决定旧意图仍然安全。

### 10.4 修改来源

对象显示 Relay 已验证的 `created_by`、`updated_by`、`created_at` 和 `updated_at`。
当公钥可以解析为 Buzz Profile 或 managed Agent 时，优先显示可识别身份；无法解析时
使用缩略公钥，不伪造身份名称。

## 11. 客户端与后端边界

客户端继续遵守现有后端设计：

```text
React UI
    │ typed Human intent
    ▼
Desktop Project View client boundary
    ├── 读取并验证 NIP-11 capability 与 Relay signer
    ├── 获取 revision 一致的 Relay projection snapshot
    ├── 组装规范 Project View read model
    └── 构造、签名并提交 typed mutation
    │
    ▼
Buzz Relay 的既有事件与查询入口
```

边界原则如下：

- 客户端读取 Relay projection，不直接查询 PostgreSQL；
- 客户端写入成员签名 Project View mutation，不增加 UI 专用业务 endpoint；
- 客户端复用既有 domain 与 SDK 约束，不在 TypeScript 中重新发明一套规范模型；
- UI 不调用 `buzz project-view` CLI 子进程；
- CLI 与 Desktop 的交互形式可以不同，但签名事件和最终 Project View 必须一致；
- 客户端把 projection 事件视为不可信输入，完成 signer、签名、坐标、类型和 revision
  验证后才能展示；
- 读取失败时不退回未验证 JSON、部分对象或旧新 revision 拼接结果。

具体 Rust 模块、Tauri command、前端 query key、缓存与订阅实现留到实现阶段决定。

## 12. 页面状态与错误表达

客户端至少区分以下状态：

| 状态 | 用户体验 |
|---|---|
| Loading | 显示稳定的 View 骨架，不误报“未初始化” |
| Unsupported | 说明当前 Relay 不支持 Project View |
| Uninitialized | 显示初始化入口和 Project View 的用途 |
| Ready | 显示完整、已验证且 revision 一致的 View |
| Refreshing | 保留上一份已验证内容，并提示正在同步 |
| Offline / stale | 保留可用的已验证内容并标记可能过期；禁止制造已成功写入的假象 |
| Forbidden | 说明当前身份不是可以读取此 Community View 的成员 |
| Conflict | 保留草稿，展示最新 revision 并要求 Human 重新确认 |
| Integrity failure | 不展示可疑的部分结果，提供重试与诊断说明 |

错误信息应使用 Human 可理解的语言，同时保留足够的安全诊断信息。普通界面不需要暴露
事件正文、完整 projection payload 或内部数据库信息。

## 13. Desktop 体验要求

客户端 v0 首先服务 Desktop，但页面结构应避免阻碍未来 Web 或 Mobile 复用。

Desktop 首版要求：

- 主地图支持键盘导航；
- 状态和优先级不能只依赖颜色区分；
- 表单具有明确 label、错误位置和提交状态；
- 长文本可阅读，文本大小遵守 Desktop 的 rem 字体尺度；
- Inspector 在较窄窗口中可以转换为抽屉或完整页面；
- 加载、空状态和错误状态不会导致布局大幅跳动；
- Community 切换时清理当前 View 的订阅、草稿绑定和临时选择状态。

Mobile 的具体导航、手势和布局不在本文中定义。

## 14. 分阶段交付

### 14.1 Client Slice 1：读取基础与 View 页面

- 建立 Desktop Project View 客户端边界；
- 识别 capability、未初始化和权限状态；
- 获取并验证一致快照；
- 增加 `View` 入口和 `/view` 页面；
- 只读呈现 Profile、项目地图、未规划区域、Roles、Resources 和 Inspector；
- 保持现有 `Projects` 完全不变。

### 14.2 Client Slice 2：初始化与类型化修改

- 初始化 Project Profile 与初始 Goal；
- 创建和编辑全部首版对象；
- 使用合法关系选择器移动或解除对象关系；
- 支持显式状态与优先级修改；
- 提供引用感知的删除交互。

### 14.3 Client Slice 3：实时协作与冲突

- 响应 Human、Agent 和其他成员的修改；
- 在连接恢复和 Community 切换后重新确认一致快照；
- 保留冲突中的 Human 草稿；
- 提供 revision 变化说明与显式重新提交；
- 展示可识别的修改来源。

### 14.4 Client Slice 4：验收与体验收口

- 覆盖主要空状态、错误状态和完整性失败；
- 完成键盘、可访问性和窄窗口体验；
- 增加组件、交互、Community 隔离和真实 Relay 测试；
- 验证 Human 与 Agent 对同一 View 的交替修改；
- 固化客户端运维、兼容与发布说明。

## 15. 客户端 v0 验收标准

客户端 v0 完成交付时应满足：

1. 支持 Project View 的 Community 可以进入 `View`，现有 `Projects` 行为没有变化。
2. 未初始化 Community 可以由 Human 原子建立 Profile 和至少一个 Goal。
3. Human 可以直接回答项目是什么、目标是什么、当前有哪些规划、工作、问题、Role
   和 Resource。
4. 所有活动对象都在唯一规范位置出现，未关联和未规划对象不会丢失。
5. Human 可以创建、查看、编辑和删除全部首版对象，并只能选择合法关系。
6. Work、Requirement、Issue、Stage 和 Plan 的状态不会因其他对象变化而隐式级联。
7. Agent 修改 View 后，已打开客户端能够恢复到新的、一致的 project revision。
8. Human 基于旧 revision 提交时会得到明确 conflict，草稿不会丢失，也不会自动覆盖
   新状态。
9. Community 切换、断线重连或 projection 乱序不会产生跨 Community 数据泄漏或混合
   revision。
10. 客户端不会直接访问数据库、调用 CLI 子进程或维护第二份 Project View 权威状态。

完成上述标准后，才可以认为 Human 与 Agent 都能使用同一幅 Project View。

## 16. 实现阶段再决定的事项

以下细节不在本文中提前固定：

- React 组件边界与文件组织；
- Tauri command 和客户端 DTO 的具体形状；
- React 状态、服务端状态和表单状态所采用的库与拆分方式；
- 实时变化采用整幅快照刷新还是原子增量应用；
- 快照是否以及如何持久化到本地；
- Object Inspector 的最终宽度、动画和响应式表现；
- 对象定位所用的 URL search 参数与 deep link 格式；
- 大型 View 的搜索、过滤、折叠和虚拟列表策略；
- Current Focus 的最终视觉形式；
- 现有 `Projects` 与 Resource 的未来手动或自动关联体验；
- 完整活动历史、diff 和审计浏览体验；
- Web 与 Mobile 的交付顺序。

这些选择必须继续遵守本文的产品语义、领域约束和一致性要求，但可以在具体实现时根据
现有 Buzz Desktop 架构、性能测量和用户反馈调整。

## 17. 当前结论

客户端 v0 将 Project View 从 Agent 可使用的协议能力，推进为 Human 与 Agent 共同使用
的项目工作面：

> `View` 让当前 Community 的成员直接认识项目、阅读项目结构、维护明确状态，并在
> Human 与 Agent 并发工作时安全地共享同一份项目现实。

它首先保证项目状态清楚、完整、可操作和可恢复；不在首版把 Project View 扩展为通用
任务管理系统，也不提前实现项目上下文。
