# Meeting Desktop 产品规格

> 状态：主体产品已实现；逻辑主持 runtime v4 / Contract `4/7` 适配实施中，current 验收待确认
>
> 目标客户端：Buzz Desktop。
>
> 依赖协议：[Meeting V2：主持人维护的共享会议看板](../v2/meeting-v2.md)与
> [Meeting Action Finalization 逻辑主持人 ACK 与同步简化实现设计](../fix/meeting-action-finalization-logical-host-ack-simplification-implementation-design.md)。
> [主持人直接完成行动收口的后端修正方案](../fix/meeting-v2-direct-action-finalization-backend-plan.md)
> 仅保留 Plan/Step 退役的历史背景。
>
> 本文定义 Meeting 在 Desktop 中的产品语义、信息架构、页面状态和 Human 交互，不重新
> 定义后端协议，不规定组件拆分、Tauri command、缓存实现、具体视觉尺寸或阶段开发计划。

## 1. 文档目的

Meeting V2 后端已经提供从创建会议、冻结参会名单、主持式发言权接力、会议看板维护，
到正常关闭、异常中止和可选行动收口的完整生命周期。Agent 可以通过 ACP 和 CLI 使用这些
能力，但 Desktop 尚未提供 Human 可以使用的图形界面。

本文回答：

1. Human 从哪里发起、发现和打开会议；
2. Desktop 如何同时呈现正式发言时间线与当前会议看板；
3. Human 参会者如何申请、接受、使用和交还发言权；
4. Human 主持人如何查看 Intent、维护看板、安排下一席和结束会议；
5. Agent 主持或参会时，Desktop 如何提供可信的观察界面而不冒充 Agent 操作；
6. 行动收口如何继续作为会议生命周期的一部分呈现；
7. 哪些现有 Desktop 能力可以复用，哪些普通频道行为不得带入会议。

## 2. 产品基础与当前边界

### 2.1 会议是特殊私有频道

每场 Meeting 已经拥有一个私有会议房间。Desktop 应把它表现为一种特殊私有频道，而不是
Project View 的子页面，也不是脱离 Buzz 导航体系的独立应用。

会议房间继续继承：

- 当前 Community 和 Relay 边界；
- 固定成员可读的私有 Channel；
- 可复制、可恢复的房间定位；
- 现有身份、Profile、Agent 标识和 Community 切换能力。

会议房间同时覆盖普通频道行为：

- 主内容只能由 Meeting 协议接受的正式 Speech 构成；
- 普通消息输入、自由 Reply 和 Thread 不得形成绕开发言权的讨论路径；
- 普通频道成员管理不能改变冻结的会议参会名单；
- 会议终态使房间永久只读。

协议元数据中的 `room_kind=meeting` 是会议识别依据。它是 Channel 之上的房间语义，不应把
现有 `stream | forum | dm` 的普通 Channel 类型直接改写成 `meeting`。

### 2.2 复用现有 Desktop 空间结构

Meeting 页面沿用 Desktop 已有的三栏结构：

```text
左侧应用与房间导航
    + 中央频道式内容时间线
    + 可调整宽度的右侧辅助面板
```

在 Meeting 中分别映射为：

```text
Meetings 导航
    + 正式 Speech 时间线与 Floor 控制区
    + 默认打开的 Meeting Board
```

页面不复制第二套 Community Rail、Profile、消息渲染基础设施或右侧面板框架。

### 2.3 与 Project View 的边界

Project View 是会议可选的上下文和行动目标，不是会议存在的前提：

- 发起会议时可以不选择任何 Project View 对象；
- Board 可以包含 Project View、消息、文档、代码或普通 URL；
- 引用 Project View 不表示会议必然修改它；
- Action Finalization 不建立 Meeting 专用 Project View 写入协议；
- Agent 主持人使用普通 CLI，Human 主持人使用现有 Project View 管理界面；
- 这些写入仍是 Project View 自身的普通业务操作，不由 Meeting 代理、限制或验证；
- 主持人可以在没有任何 Project View 写入时完成行动收口；
- 普通 Speech、Intent、Board Maintenance 和直接关闭都不能隐式修改 Project View。

### 2.4 与 Huddle 的边界

Huddle 是实时音频能力，Meeting 是结构化的 Board 与 Floor 协议。二者当前没有语音转写、
身份同步或发言权同步关系。

Desktop 初版不得：

- 创建 Meeting 时自动创建 Huddle；
- 把 Huddle 中的语音当作正式 Speech；
- 用 Huddle 的加入和离开表达 Meeting 入会状态；
- 在 Meeting 标题栏直接复用普通频道的 Huddle 主操作，造成两套发言语义混淆。

## 3. 设计目标

Meeting Desktop 必须达到：

1. Human 可以发起一场由自己主持的 Meeting V2。
2. 发起者可以从 Human 和 Agent 中选择固定参会名单。
3. 每个参会者都能持续看到正式 Speech 时间线和当前 Board。
4. Human 参会者可以完整使用 Human Floor Request、Offer、Grant、Speech、Yield 和
   Directed Handoff。
5. Human 主持人可以完整维护 Board、查看和管理 Intent、安排自己或其他 Agent 发言、管理
   open Handoff、保持 idle、正常关闭或异常中止。
6. 界面从交互顺序上保证 Board Maintenance 先于主持人的 Floor Decision。
7. Board Maintenance 和 Floor Decision 使用两个明确分离的界面阶段，不共享一个客户端
   倒计时或提交动作。
8. Agent 主持或参会时，Human 可以理解 Agent 当前正在做什么，但不能代替 Agent 身份签名。
9. Action Finalization 是 Meeting 生命周期内的阶段；即使 Human 临时打开外部业务页面，
   Meeting 也只有到 `closed` 或 `aborted` 才结束。
10. `closed` 与 `aborted` 在产品语义和视觉结果上明确区分。
11. Board 不建立用户可管理的版本历史、Diff 或更新通知。
12. Project View、来源频道和其他外部上下文保持可选。
13. 所有可操作状态以 Relay 权威状态为准，重连或命令失败不能产生虚假发言权。

## 4. 非目标

Desktop 初版不包含：

- 会议类型、会议模板和流程模板；
- RSVP、接受邀请、法定人数和正式入会状态；
- 会议期间动态增加或移除参会者；
- 主持权转移、副主持人、主持人选举或主持接管；
- 多人协同编辑 Board；
- Board 业务版本、历史、Diff 或变更通知；
- 自动解析、理解、排序或推进 Board 中的议程；
- 投票、表决、决议签署或多人确认；
- 自动生成会议纪要以替代 Board 和 Speech timeline；
- 音频、视频、语音转写或 Huddle 联动；
- 在 Meeting 中嵌入完整 Project View 画布；
- 在 Meeting 中提供 Human 专用的行动物化表单、Plan/Step 编辑器或受限业务写入器；
- 由 Meeting 自动证明 Board 与外部业务状态语义一致；
- 让 Human 通过 Desktop 冒充 managed Agent 主持或发言；
- Web 和 Mobile 的具体页面设计；
- 修改 Meeting V2 的事件、状态机、优先级和权限语义。

## 5. 核心设计结论

### 5.1 中央讨论，右侧看板

Meeting 的默认宽屏布局是：

```text
中央：正式发言和当前 speaker
右侧：主持人维护的当前 Board
底部：与当前身份和 Floor 状态匹配的操作区
```

Board 默认打开，因为它是参会者理解目标、议程、进展和结论的主要入口。用户可以收起它，
但 Meeting 标题栏必须始终提供明确的 `Board` 入口。

### 5.2 Human 主持是完整主持路径

Human 主持的 Meeting 不是只读或降级模式。Human 主持人必须可以在 Desktop 中完成全部
需要主持身份签名的正常操作。

Agent 主持与 Human 主持共享同一会议语义，但控制来源不同：

- Human 主持：Desktop 提供主持人控制台并由 Human 操作；
- Agent 主持：ACP 自主操作，Desktop 对其他身份只提供状态观察；
- 当前身份不是主持人时，客户端不得显示可提交的主持命令。

### 5.3 Board 与 Floor 是两个连续界面阶段

当主持人获得 Control Token 时，界面顺序固定为：

```text
Board Maintenance
    ↓ 完成、确认不变或协议超时
Floor Decision
```

客户端不得把“保存 Board”和“选择下一位 speaker”合并成一个按钮、一次提交或一个倒计时。

### 5.4 协议权威，产品语言优先

Desktop 不自行分配发言权，也不根据本地点击直接宣布状态改变。所有状态变化必须等待 Relay
接受并返回权威结果。

同时，主界面不要求普通用户理解 event kind、revision、epoch、lease、action run 或
action window。低层信息只在诊断或活动记录中按需展示。

### 5.5 没有正式 Join

发起会议时确定的 roster 就是会议完整参会名单。创建成功后，成员已经拥有会议房间访问权：

- 不发送 RSVP；
- 不要求接受邀请；
- Human 打开房间只是查看会议，不改变权威生命周期；
- Agent 由 ACP 在其 Community 槽池中参与，不依赖 Desktop 是否打开；
- Presence 只可作为辅助状态，不参与 quorum、Floor 或关闭判断。

## 6. 用户可见术语

主界面采用下表中的产品用语：

| 协议概念 | Desktop 主要文案 |
|---|---|
| Meeting Board | 会议看板 / Board |
| Control Token | 主持控制权；通常不直接展示 Token 名称 |
| Board Maintenance | 整理看板 |
| Floor Decision | 安排下一步 |
| SpeechIntent | 发言意图 / Intent |
| Human Floor Request | 申请发言 |
| Offer | 邀请发言 |
| Speech Grant | 轮到你发言 / 已获得发言权 |
| Yield | 放弃本轮发言 |
| Directed Handoff | 请某位参会人继续回应 |
| Recall | 本轮结束后收回主持控制权 |
| HOST_IDLE | 等待新的发言意图 |
| finalizing_actions | 行动收口中 |
| runnable | 可以继续记录或确认行动产出 |
| blocked | 行动收口被阻塞 |
| actions-recorded | 行动产出已完成记录；不显示协议 attestation 名称 |
| closed | 已正常结束 |
| aborted | 已中止 |

`closed` 不得只显示成含义模糊的“结束”。它声明主持人认为目标已经达到并形成有效结论。
`aborted` 必须使用“中止”，避免暗示会议成功完成。

## 7. 信息架构与导航

### 7.1 Meetings 侧栏分组

Desktop 左侧导航增加 `Meetings` 分组。它属于当前 Community，不跨 Community 聚合。

分组包含：

- `+`：发起新会议；
- 当前 active Meeting；
- 正在 Action Finalization 的 Meeting；
- `会议历史`入口。

会议房间不得同时重复出现在普通 Channels 分组中。

### 7.2 会议条目

进行中的会议条目至少显示：

- 会议名称；
- 当前状态点；
- 当前身份需要操作时的 attention 标识；
- 正在发言的 participant 或简短状态，可在空间允许时显示。

排序优先考虑：

1. 当前身份需要立即操作；
2. 正在进行；
3. 最近发生权威生命周期活动。

原始 Floor 控制事件不能逐个制造普通未读角标。

### 7.3 会议历史

正常关闭和异常中止的 Meeting 从 active 列表移入会议历史。历史至少支持：

- 按最近结束时间浏览；
- 区分 `closed` 与 `aborted`；
- 显示主持人和参会者；
- 重新打开只读 Speech timeline、最终 Board，以及是否经行动产出确认关闭。

初版不要求高级搜索、标签、会议类型筛选或归档管理。

### 7.4 来源频道入口

普通 Channel 的更多菜单可以提供 `发起会议`：

- 仅把当前 Channel 预填为可选 source；
- 不要求 Meeting 必须关联该 Channel；
- 不自动把 Channel 全体成员加入 roster；
- 所选每位参会者都必须能够读取 source；
- 无法满足读取条件时，用户应删除 source 或调整 roster，客户端不得静默扩大权限。

这里的 source 是 Meeting Create 已有的可选 `source_channel_id` 导航元数据，不是客户端从
Board Markdown 反向解析出来的链接。只有用户明确选择 source 时才执行 roster 可读校验；
没有 source 永远是合法会议。Board 中其他可选引用失效或不可访问时，仍按 11.8 的通用引用
规则处理，不阻止会议创建或继续讨论。

### 7.5 房间定位

Meeting 可以复用现有 Channel 路由和窗口恢复能力，只需按 `room_kind=meeting` 切换产品模式。
这是推荐的内部实现方向，不构成用户可见 URL 的永久承诺。

复制会议链接只复制定位，不授予非 roster 身份读取权限。

## 8. 发起会议

### 8.1 发起入口

Human 可以从：

1. `Meetings` 分组的 `+`；
2. 普通 Channel 的 `发起会议`；
3. 后续可选的全局 Quick Action；

打开同一个创建界面。

### 8.2 创建字段

创建界面按以下顺序组织：

1. **会议名称**：必填；
2. **讨论目标**：必填，用于建立初始 Board；
3. **议程**：可添加、删除和调整顺序，初版允许暂不填写；
4. **参会人**：必填，创建者之外至少一人；
5. **背景与上下文**：可选；
6. **来源频道**：可选；
7. **Project View、消息、文档或 URL 引用**：全部可选。

客户端不得要求用户选择会议类型、模板、主持策略或底层 policy。

新创建的 Desktop Meeting 应使用当前可创建的行动能力版 Meeting V2。兼容旧 policy 是读取
与呈现问题，不是普通用户的创建选项。

### 8.3 初始 Board 的生成

创建界面的目标、议程和上下文字段只是首次编辑辅助。提交时，客户端把它们组合成一份
完整 Markdown Board。

这不构成会议模板：

- 不存在模板 ID、模板名称或可复用模板库；
- 不产生 Board 的结构化字段协议；
- 创建后主持人维护的是一份自由 Markdown 文档；
- 系统不反向解析 Markdown 来自动判断议程完成状态；
- 主持人可以删除、重排或完全重写首次生成的结构。

创建界面应提供最终 Board 预览或直接编辑入口，避免生成主持人未看见的权威内容。

### 8.4 参会人选择

参会人选择器同时检索：

- 当前 Community 可发现的 Human；
- 当前身份管理的 Agent；
- 当前 Community 中可参与的其他 Agent。

候选项至少显示：

- 显示名称和头像；
- Human 或 Agent 标识；
- Agent owner 或 managed-by 信息，在当前产品已能解析时显示；
- 公钥的按需识别信息。

约束为：

- 创建者自动加入且不能移除；
- 创建者之外选择 1 至 11 人；
- 总 roster 为 2 至 12 人；
- 同一身份不能重复；
- Meeting 创建后名单冻结。

新建 action-capable Meeting V2 时，完整 Agent roster 还必须满足 runtime capability gate：

- 每个选中的 Agent runtime 都声明 `meeting-v2-action-finalization-v4`；
- Human participant 不需要该 Agent runtime capability；
- 已知不兼容的 Agent 在选择器中明确标记，并阻止提交；
- capability 暂时未知时，提交前重新确认，Relay 拒绝时保留完整创建草稿；
- 客户端不得为了容纳不兼容 Agent 而静默降级为不支持行动收口的旧 policy。

### 8.5 主持人身份

发起者自动成为唯一主持人。创建界面不提供主持人选择器。

- Human 从 Desktop 创建：当前 Human 是主持人；
- Agent 通过 ACP/CLI 创建：该 Agent 是主持人；
- Human 不能在 Desktop 中选择“由某个 Agent 主持”并以 Human 身份代为创建；
- 若产品未来提供“请 Agent 发起会议”，它必须触发 Agent 自己的独立创建行为，不改变本条
  身份规则。

### 8.6 创建结果

Relay 接受 Create 后：

- Meeting 出现在所有 roster 成员的 Meetings 分组；
- 发起者进入会议房间；
- 初始 Board 默认打开；
- 主持人拥有初始控制机会；
- 其他 participant 不需要接受邀请或执行 Join。

创建失败时，输入草稿应保留；客户端不得展示一个尚未被 Relay 接受的临时会议房间。

## 9. Meeting 房间布局

### 9.1 宽屏结构

```text
┌──────────────┬──────────────────────────────────┬──────────────────────┐
│ Meetings     │ 会议标题 · 状态 · 主持人 · 成员 │ Meeting Board        │
│              ├──────────────────────────────────┤                      │
│ ● 需求讨论会 │ 当前阶段 / speaker 状态          │ 讨论目标             │
│ ○ 架构评审会 │                                  │ 当前议程             │
│              │ 正式 Speech timeline             │ 记录、共识、问题     │
│ Channels     │                                  │ 最终结论             │
│ # general    │                                  │ 可选引用             │
│ # agents     ├──────────────────────────────────┤                      │
│              │ Floor Dock / 主持人控制台        │ 查看或主持人编辑     │
└──────────────┴──────────────────────────────────┴──────────────────────┘
```

### 9.2 会议标题栏

标题栏至少显示：

- Meeting 图标和名称；
- `进行中 / 行动收口中 / 已正常结束 / 已中止`；
- 主持人身份；
- 参会者头像与人数；
- Board 展开或收起操作；
- 更多菜单。

更多菜单根据身份和状态提供：

- 查看完整参会名单；
- 查看来源上下文；
- 复制会议链接；
- 查看会议活动记录；
- 主持人可用的中止操作；
- 终态下的只读信息。

会议进行中不得沿用普通 Channel 的成员增加、移除、归档、可见性转换和 Channel 类型修改
操作。

### 9.3 状态条

标题栏下方使用一条紧凑状态条表达当前最重要的会议状态，例如：

- 主持人正在整理看板；
- 主持人正在安排下一位发言人；
- Alice 正在决定是否接受发言邀请；
- Bob 正在发言；
- 等待新的发言意图；
- 主持人正在记录会议行动产出；
- 行动收口被阻塞；
- Meeting 已正常结束；
- Meeting 已中止。

状态条不展示原始 epoch、revision 或 event ID。

### 9.4 中央内容区

中央内容区保持稳定的正式 Speech timeline。Board Maintenance、Intent 和 Floor 命令不作为
普通消息混入其中。

进入 Action Finalization 时，中央时间线仍可向上浏览；底部控制区切换为行动收口卡片，
不跳转到会议外的独立工作页面。

### 9.5 右侧 Board

宽屏默认打开，可调整宽度。收起后，标题栏中的 Board 操作持续可见。

中窄窗口使用现有右侧 Sheet 或覆盖面板模式。打开 Board 时不得丢失 timeline 滚动位置或
当前 Floor 操作草稿。

### 9.6 底部控制区

普通 Channel composer 在 Meeting 中被替换为：

- Human participant 的 Floor Dock；
- Human host 的主持人控制台；
- Agent host 或非当前操作身份的只读状态条；
- Action Finalization 控制卡；
- 终态只读说明。

底部区域只显示当前状态合法的操作，不通过提交后再报错的方式暴露大量无效按钮。

## 10. 生命周期与页面状态

Desktop 不创建新的权威会议阶段。创建表单只是本地前置界面，其他状态全部映射自 Relay。

| 权威状态 | Desktop 主要界面 | 是否可发言 | Board |
|---|---|---:|---|
| active，主持人 Board 窗口 | 主持人正在整理看板 | 否 | 主持人可编辑，其他人只读 |
| active，Floor Decision | 主持人正在安排下一步 | 否 | 只读 |
| active，HOST_IDLE | 等待新的发言意图 | 否 | 只读 |
| active，Offer | 邀请目标接受或拒绝 | 否 | 只读 |
| active，Grant | 当前 speaker 发言 | 仅 holder | 只读 |
| finalizing_actions/runnable | 正在记录行动产出 | 否 | 冻结只读 |
| finalizing_actions/blocked | 行动收口被阻塞 | 否 | 冻结只读 |
| ended/closed | 已正常结束 | 否 | 最终只读 |
| ended/aborted | 已中止 | 否 | 最终只读 |

Action Finalization 不再细分 `planning | applying | ready_to_close`。`runnable | blocked` 是
当前 action run 的唯一用户相关 condition：前者允许主持人继续使用普通业务入口或提交完成
确认，后者要求先恢复为新的 runnable window，不能直接正常关闭。

Human Floor Request 和 Directed Handoff 可以让状态绕过主持人的普通 Floor Decision 路径。
Desktop 必须按权威状态切换，不得强行插入本地“先整理看板”页面阻塞这两条直接路径。

Board Maintenance 只有以显式 `updated | unchanged` 完成，才构成正常关闭或进入 Action
Finalization 所需的最终 Board 结果。`timed_out | preempted` 可以让会议继续沿合法 Floor
路径推进，但不能被 Desktop 当作主持人确认了最终 Board。

## 11. Meeting Board

### 11.1 定位

Board 是主持人维护的一份当前共享会议文档，用于表达：

- 讨论目标；
- 有序议程；
- 当前进度；
- 讨论记录、共识和未决问题；
- 最终结论；
- 可选外部上下文。

Board 不替代 Speech timeline，也不自动成为 Project View 或其他系统中的事实。

### 11.2 普通参会者阅读

所有 roster participant 都可以打开当前 Board。普通参会者：

- 只能查看；
- 可以复制内容和打开合法引用；
- 不能直接评论、建议修改或提交 patch；
- 若希望修改 Board，只能在获得 Grant 后通过正式 Speech 提出建议。

### 11.3 主持人编辑条件

Human 主持人只有在以下条件同时成立时才看到可编辑 Board：

- Meeting 仍为 active；
- Control Token 属于主持人；
- 当前处于合法 Board Maintenance 窗口；
- 不存在活动 Offer 或 Grant；
- 当前 Board 已成功读取并确认是权威当前内容。

其他任何状态下，主持人看到的也是只读 Board。

### 11.4 Board Maintenance 操作

Board 进入编辑状态后，主持人控制台仅提供：

- `保存并继续`：提交完整的新 Board；
- `看板无需修改`：显式保持当前 Board；
- 本地取消编辑：只退出本地编辑器，不代表协议上的 `UNCHANGED`。

在 `保存并继续` 或 `看板无需修改` 得到 Relay 接受之前，不得出现可执行的下一席选择。

### 11.5 不提供版本产品

Desktop 不提供：

- Board revision；
- 版本列表；
- Diff；
- 回滚；
- 更新通知；
- 未读 Board 标记。

底层 event ID、幂等 fence 和审计数据可以存在，但不成为普通用户维护的 Board 版本。

### 11.6 获取最新 Board

Desktop 至少在以下节点读取或重新确认当前 Board：

- 首次打开 Meeting；
- 打开 Board 面板；
- Human 主持人进入编辑窗口；
- Meeting 进入或离开 Action Finalization；
- 重连并恢复会议；
- Relay 通知当前 Board 已失效后。

这属于获取当前状态，不构成 Board 变更通知产品。

若无法确认最新 Board：

- 可以展示明确标记为“未确认最新状态”的缓存内容；
- 不得把缓存内容标记为当前 Board；
- 禁止主持人基于它提交修改；
- 提供重试。

### 11.7 编辑被 Human Request 抢占

Human Floor Request 可以在 Board Maintenance 期间取得直接优先路径。发生时：

- 尚未被 Relay 接受的 Board 修改不得迟到提交；
- 编辑器立即变为不可提交；
- 客户端可以保留本地草稿，但必须标记“尚未写入会议看板”；
- 当前 Human speech 流程不等待该草稿；
- 控制权最终返回后，主持人重新读取权威 Board，再决定复制、合并或丢弃本地草稿。

本地草稿不是 Board 版本，也不能自动覆盖后来产生的权威内容。

### 11.8 外部引用

Board 中的链接使用普通 Markdown 链接语义。客户端可以为已知 Buzz deep link 提供更清晰的
标题或图标，但不得：

- 把任意 Markdown 解析成外部写命令；
- 因引用不可访问而阻止通用 Meeting；
- 把链接存在解释成 Action Finalization 决定。

## 12. 正式 Speech timeline

### 12.1 正式内容

主时间线只把成功消费 Speech Grant 的 canonical Speech 表现为完整消息行。每条 Speech 至少
显示：

- speaker 身份和 Human/Agent 类型；
- 主持人标识，在 speaker 同时是主持人时显示；
- 正文；
- 正式发生时间；
- Directed Handoff 的目标和原因，在存在时显示。

Speech 继续复用 Desktop 现有的 Markdown、链接、代码块和身份展示能力，但发送路径必须是
Meeting Speech，而不是普通 Channel message。

### 12.2 控制过程的呈现

以下内容不作为普通 Speech：

- Intent submit、refresh、withdraw、select、reject；
- Human Floor Request；
- Offer、ACK、Decline；
- Grant、Progress、Yield、Expiry；
- Board update 或 unchanged；
- Recall；
- Action command。

主时间线可以把重要转换压缩成轻量系统行，例如：

- Alice 放弃了本轮发言；
- Bob 将问题交给 Carol；
- 主持人收回了后续接力；
- Meeting 进入行动收口；
- Meeting 已正常结束。

完整控制记录放在 `会议活动记录`中按需查看。

### 12.3 不建立旁路讨论

Meeting Speech 初版不提供普通 Channel 的：

- Reply composer；
- Thread panel；
- 绕过 Grant 的快捷回复；
- 普通消息编辑；
- 产生新讨论内容的 Reaction 语义。

复制、选择文本、打开 Profile 和合法链接可以继续使用。

## 13. Human 参会者操作

本节的 Human 参会者指非主持 Human。Human 主持人的自发言走主持人 self Intent 和选择路径，
不能使用 Human Floor Request 绕过主持约束。

是否属于 Human、Agent 或主持人，必须读取 Meeting Create 时由 Relay 冻结的 participant type
与 moderator identity。Desktop 可以使用当前 Profile 和 Channel 信息改善头像、名称与说明，
但不能根据后来变化的 Profile、Channel role、managed-by 或本地 agent discovery 重新推断
Floor 权限。只有冻结类型为非主持 Human 的身份才显示 Human Floor Request 操作。

### 13.1 申请发言

未持有 Request、Offer 或 Grant 时，Floor Dock 显示：

```text
[申请发言]
```

Human Floor Request 协议不包含 Intent 摘要，因此 Desktop 不要求 Human 在申请时填写一句
发言主题。提交后显示：

```text
已申请发言 · 等待当前发言结束
[撤回申请]
```

Human Request：

- 按 Relay FIFO 和协议优先级处理；
- 不打断已经 GRANTED 的 speaker；
- 可以抢占协议允许抢占的尚未 ACK Offer；已经服务于更早 Human Request 的 Offer 继续保持
  FIFO 优先级；
- 主持人不能拒绝、重排或撤销；
- 只能由申请者撤回。

### 13.2 接受或拒绝 Offer

Offer 指向当前 Human 时，Floor Dock 使用高注意力状态显示：

- `轮到你发言`；
- Offer 来源，例如 Human Request、主持人选择或 Directed Handoff；
- Directed Handoff 原因，在存在时显示；
- `接受`；
- `放弃`，可填写协议允许的简短原因；
- 权威截止状态，在 Relay 提供时显示。

点击接受不立即本地打开 composer。只有 Relay 确认 Grant 后才进入发言状态。

### 13.3 使用 Grant 发言

当前 Human 获得 Grant 后，Floor Dock 变为唯一可发送的 Speech composer：

- 支持一条正式 Speech；
- 支持合法 participant mention；
- 可选指定 Directed Handoff 目标、类型和必填原因；
- 默认提供 `发表并结束本轮`；指定 Directed Handoff 时改为`发表并请目标回应`；
- 尚未发表时提供 `放弃本轮发言`；
- Grant 过期或被终结后立即禁止提交。

每个 Grant 最多接受一条 Speech。客户端不得在发送成功后继续保留可再次发送的 composer。

### 13.4 非当前 Human 状态

当其他 participant 操作时，当前 Human 的 Floor Dock 只显示：

- 当前 speaker；
- 自己是否已申请；
- 是否存在需要自己接受的 Offer；
- Meeting 是否正在等待主持人；
- Meeting 是否已经进入行动收口或终态。

## 14. Human 主持人控制台

### 14.1 总体结构

Human 主持人的底部区域不是普通 composer，而是状态驱动的主持人控制台。

```text
主持人获得 Control Token
        ↓
Board Maintenance
        ↓
Floor Decision
        ├── 选择 pending SpeechIntent
        ├── 重新处理 open Handoff
        ├── 安排自己发言
        ├── 保持 idle
        ├── 直接正常关闭
        └── 进入 Action Finalization
```

主持人同时仍是普通 participant。Human 主持人唯一不能使用的 Human 专属普通能力是 Human
Floor Request；任何合法指向主持人身份的 Offer 仍必须可响应。无论 Offer 来自 self Intent、
另一个 speaker 的 Directed Handoff 或协议 fallback，主持人控制台都复用 13.2 和 13.3 的
`接受 / 放弃 → Grant composer / Yield`路径，并显示真实 allocation source。获得 Grant 后
主持人只能发表一条正式 Speech；Board 要等 Control Token 后续真正返回并创建新 Board 窗口
时才能编辑。

### 14.2 Intent 列表

主持人可以查看共享 pending SpeechIntent 池。每个 Intent 至少显示：

- 作者身份；
- 一句话摘要；
- addressed participant，在存在时显示；
- pending、selected、rejected、withdrawn 或 consumed 等产品可理解状态；
- 是否基于较早 speech revision，客户端能够确认时以“可能已过时”表达；
- 可用主持操作。

Intent 不包含完整候选 Speech、隐藏推理或 Agent 工具结果，Desktop 不得暗示可以预览 Agent
完整发言。

### 14.3 异步整理 Intent

主持人可以在其他 speaker 发言期间查看并拒绝明显不再适用的 pending Intent。拒绝界面必须
收集协议要求的稳定原因类别和 Human 可读说明，例如：

- 偏离主题；
- 重复；
- 已被新内容替代；
- 当前 Meeting 无法支持；
- 与当前议程不符。

拒绝必须等待 Relay 接受，不能只从本地列表删除。

主持人只有在合法 Floor Decision 窗口中才能选择 Intent 并产生下一份 Offer。

### 14.4 Board Maintenance 界面

进入主持人的 Board 窗口时：

- 右侧 Board 自动打开并进入编辑；
- 状态条显示“请先整理会议看板”；
- Intent 可以继续查看，但 `邀请发言`不可操作；
- 底部只提供 `保存并继续`和`看板无需修改`；
- Board 阶段的期限只属于本阶段。

Board Maintenance 超时时：

- Board 保持原状；
- 客户端不能把超时显示成主持人主动选择“无需修改”；
- 未提交编辑失去提交资格；
- Floor Decision 进入一份完整、未被 Board 阶段消耗的独立时间预算。
- 本轮可以继续安排讨论或保持 idle，但不得正常关闭或进入 Action Finalization；关闭前必须
  在后续合法窗口取得一次显式 `updated | unchanged` 的最终 Board 结果。

### 14.5 Floor Decision 界面

Board Maintenance 以显式 `updated | unchanged` 被 Relay 接受后，主持人控制台切换为
“安排下一步”。此时可以：

1. 选择一个 pending SpeechIntent，并向目标发送 Offer；
2. 选择需要重新尝试的 open Directed Handoff；
3. 安排主持人自己发言；
4. 保持 idle，等待新的 Intent；
5. 在满足条件时直接正常关闭；
6. 在满足条件时进入 Action Finalization。

Board 在本阶段只读。Floor Decision 的期限从本阶段开始，不继承 Board Maintenance 已消耗的
时间。

这些选项不是无条件并列：Human Request 始终先走直接优先路径；主持人存在有效 pending
self Intent 时，必须先选择自己或撤回该 self Intent，普通 Intent 和 open Handoff selection
在此之前不可操作。拒绝不再适用的普通 pending Intent 仍可按 14.3 异步进行。

Board Maintenance 若以 `timed_out` 结束，Desktop 仍按 Relay 进入完整 Floor Decision，
但只提供继续讨论或 idle 所需的合法操作，隐藏 Close 和 Finalize Actions。若 Board 窗口以
`preempted` 结束，则直接跟随 Human Request 等权威优先路径，不先展示主持人 Floor Decision；
控制权返回主持人后再创建新的 Board Maintenance 机会。

### 14.6 选择 SpeechIntent

用户可见主操作为：

```text
邀请 Alice 发言
```

其权威过程仍为：

```text
主持人选择 Intent
    → Relay 创建 Offer
    → Alice ACK 或 Decline
    → ACK 后 Relay 创建唯一 Grant
    → Alice 发表一条 Speech
```

Desktop 不得在选择 Intent 后立即显示 Alice 已获得发言权。

Offer 等待期间，控制台显示目标、来源 Intent 和等待状态。Offer 失败或超时后，对应 Intent
仍可能保持 pending，界面必须按 Relay 结果恢复，不能静默消费。

### 14.7 主持人自发言

`我要发言`是主持人的产品级入口，但必须保持 V1 的 self Intent、selection、Offer 和 Grant
约束。完整交互为：

1. 主持人填写必需的一句话发言意图；
2. Relay 接受 self SpeechIntent 后，它进入主持人的 pending Intent 状态；
3. 主持人可以在 selection 前刷新摘要或撤回该 Intent；
4. 主持人在合法 Floor Decision 中选择自己的 self Intent；
5. 若协议要求说明为什么延后其他 pending Intent，界面同时收集对应原因；
6. Relay 创建指向主持人的 Offer；
7. 控制台显示`接受并开始发言`和`放弃`，由主持人本人响应；
8. Relay 确认 ACK 并创建 Grant 后，才打开正式 Speech composer。

客户端可以把连续的 self Intent submit 与 selection 组织在一个紧凑流程中，但不能省略一句
摘要、在 Intent 尚未被 Relay 接受时提前选择，或把“我要发言”点击本身冒充后来 Offer 的
ACK。

其他约束为：

- 主持人不能绕过 Grant 直接发送 Speech；
- Human Request 优先级高于主持人自发言；
- 存在其他有效 Intent 且协议要求说明延后时，界面收集必要说明；
- 默认不得让主持人无限连续自发言。

Self Offer 被拒绝、抢占或超时后不产生 Grant，self Intent 按 Relay 结果仍可保持 pending。
控制台据此提供重新选择、刷新或撤回，不能静默消费，也不能直接复用已经失效的 Offer。

### 14.8 Human Floor Request

Human Request 不显示在主持人可排序的 Intent 列表中。控制台将它表现为独立的高优先状态：

```text
Bob 已申请发言，将在当前有效发言结束后优先获得下一席
```

主持人不能拒绝或重排。若它抢占 Board 或未 ACK Offer，Desktop 直接跟随 Relay 权威状态。

### 14.9 Directed Handoff 与 Recall

主持人控制台单独列出 open Directed Handoff：

- 来源 Speech；
- 目标 participant；
- 提问或交接原因；
- 当前是否正在 Offer/Grant；
- 已失败尝试，在协议提供时显示。

主持人可以在合法状态下：

- 重新选择 open Handoff；
- 以必填原因 dismiss 未处于活动 Offer/Grant 的 open Handoff；
- 设置 `本轮结束后收回控制权`；
- 在 OFFERED 阶段按协议取消尚未 ACK 的非 Human Offer。

Recall 不能打断已经 GRANTED 的当前 Speech，也不能越过已经排队的 Human Request。

### 14.10 保持 idle

没有合适的下一步时，主持人可以选择等待。界面显示：

```text
等待新的发言意图
```

Meeting 不产生空轮询 Speech。新的 Intent 或其他可处理工作到达后，主持人再次进入一轮
Board Maintenance，再进行 Floor Decision。

### 14.11 主持人不可用和 fallback

若 Human 主持人未在权威期限内操作：

- Desktop 显示 Relay 已采用的确定性结果；
- 不伪造本地主持决定；
- 不能在 deadline 后迟到提交旧 Board、Intent selection 或关闭命令；
- 主持人恢复后继续保有主持身份，但不重新执行已过期窗口。

## 15. Agent 主持与 Agent 参会的呈现

### 15.1 Agent 主持

当主持人是 Agent 且当前 Desktop 身份不是该 Agent 时：

- Board、Intent、Floor、Speech 和 Action 状态均可按 roster 权限查看；
- 主持人控制台变为只读状态说明；
- 不显示可签名的 Board update、Intent selection、Recall、Close 或 Action 操作；
- Human 不能借 managed-agent owner 关系冒充主持 Agent；
- Agent 的 Board Maintenance、Floor Decision 和 Action Finalization 由 ACP 继续完成。

状态文案例如：

- 主持 Agent 正在整理看板；
- 主持 Agent 正在选择下一位发言人；
- 主持 Agent 正在记录会议行动产出；
- 主持 Agent 的行动收口被阻塞。

Desktop 不展示 Agent 隐藏推理、模型草稿或工具内部输出。

### 15.2 Agent 参会者

Agent participant 的 Intent、Offer、Grant 和 Speech 由其 ACP runtime 处理。Desktop 对 Human
只显示：

- 是否有 pending Intent；
- 是否正在等待 Offer；
- 是否被邀请、已接受或正在发言；
- 正式 Speech；
- runtime 不可用等稳定可观察状态，在后端确实提供时显示。

Desktop 不为 Agent participant 显示由 Human 点击的 ACK、Speech 或 Yield 按钮。

Desktop 也不负责把 Board 主动推送给 Agent。ACP 继续在 Intent、Granted Speech、主持人的
Board Maintenance 和 Floor Decision 等协议节点取得并注入当前权威 Board；前端不新增
Board 订阅、版本协商或通知机制。

### 15.3 槽与 Session

Agent 的槽分配和 ACP Session 是 runtime 实现细节，不成为 Human 手动选择项或 Meeting
correctness gate：

- Human 不选择某个具体 Agent 槽入会；
- Human 不把一个普通 Agent Turn“拖入会议”；
- 后端优先复用持有 Meeting channel Session 的健康槽，但可由同一逻辑 Agent 的其他健康槽执行；
- 最终 Board、Floor 与 Action Finalization 均以 current canonical envelope、frozen Board 和
  run/window/Board fence 自包含，不继承物理槽授权；
- Desktop 只呈现结果与稳定故障状态。

## 16. 参会者面板

会议标题栏的成员按钮打开 Meeting participant 面板。面板按以下信息组织：

- 主持人；
- Human participants；
- Agent participants。

每位 participant 可以显示：

- Profile、Human/Agent 类型；
- 是否为主持人；
- 当前 Floor 状态；
- 是否有 pending Intent 或 Human Request；
- 是否正在被 Offer、持有 Grant 或发言；
- Presence 或 Agent runtime 状态，在可靠可用时作为辅助信息显示。

面板不得提供：

- 添加 participant；
- 移除 participant；
- 修改 Channel role 以改变 Meeting 身份；
- 指定新主持人；
- 把离线解释为退出 Meeting。

## 17. Action Finalization

### 17.1 进入方式

主持人在最后一次 Board Maintenance 完成后的 Floor Decision 中，可以选择：

- `直接结束会议`；
- `记录行动产出后结束`。

两者必须是明确的不同选择。Board 中出现 Project View 链接或“行动”文字，不会自动触发
Action Finalization。只有主持人认为正常闭会前仍需把 Board 中已经形成的行动产出登记到
Project View 或其他承载系统时，才需要进入该阶段。若行动已经登记、无需外部登记，或者
Board 本身就是足够的产出，主持人可以直接结束会议。

### 17.2 进入后的会议状态

Relay 接受 Action Finalization 后：

- Meeting 仍未结束；
- Board 冻结并绑定最终 Board；
- action run 进入 `runnable`，并获得独立于 Board 和 Floor 的截止时间；
- 新 Intent、Human Request、Offer、Grant、Speech 和普通 Floor 操作被禁止；
- 中央 timeline 继续可读；
- 底部区域切换为行动收口；
- 直到带“行动产出已记录”声明的 `End(outcome=closed)` 成功，Meeting 才进入正常终态。

Desktop 不把行动执行调度为一个会议外的新 Agent Turn，也不导航到另一个“会后任务”页面
来替代会议生命周期。Human 可以暂时导航到已有业务页面，但 Meeting 本身持续处于行动收口，
返回后仍恢复同一会议上下文。

### 17.3 行动收口卡片

行动收口卡片只展示完成主持判断所需的信息：

- 冻结的最终 Board；
- 当前主持人；
- `正在记录行动产出`或`行动收口被阻塞`；
- runnable window 的权威截止时间；
- 当前 Community 的现有 Project View 入口，以及 Board 已有的来源频道和其他上下文入口；
- 当前身份合法可用的确认、报告阻塞、重试、返回 Board 或中止操作。

只有 Create 时冻结的主持人看到可提交控制。其他 Human 和 Agent participant 只能读取最终
Board 与行动收口状态；他们不会因为 Board 中被记录为承接人，就在这一阶段获得 Meeting
操作或新的 Agent Turn。

Meeting 后端不接收普通业务操作清单，也不投影目标对象、操作数量或外部验证结果。因此
Desktop 不得在行动收口卡片中伪造“Requirement 已创建”“所有 Work 已验证”等进度。Human
可以在 Project View 自己的页面看到该系统的权威业务状态，但这不是 Meeting 的执行进度面。

普通界面也不展示：

- `action_run_id`；
- `action_window_epoch`；
- Board event ID；
- ACP lease。

技术活动记录可以在诊断入口中按需显示，但不能要求普通 Human 理解它们才能完成会议。

### 17.4 Agent 主持的行动收口

Agent 主持时：

- ACP 为同一逻辑主持 Agent 调度唯一 Action Turn，优先复用已有 Meeting channel Session，必要时
  使用其他健康槽；
- ACP 向实际取得 Turn 的槽注入精确冻结的 Board、current action window、主持身份和完整业务工具边界；
- Agent 使用自己原本拥有的普通 CLI 和业务工具直接读取、创建、更新或删除目标系统对象；
- Agent 不生成 Meeting 专用 Plan，Harness 也不编译或重放业务操作；
- Desktop 只读展示主持 Agent 正在记录行动产出及最终控制结果；
- Human roster participant 不能接管 Agent 主持身份进行修正或重试。

Desktop 可以使用以下稳定文案：

```text
主持 Agent 正在根据最终会议看板记录行动产出
```

Agent 返回 `COMPLETE` 时，ACP 直接提交带 current fence 的 `actions-recorded` End；Relay 原子接受后
页面进入 `closed`。Action lease 只表示逻辑主持 Harness 仍在线工作，不代表业务完成。
`BLOCK`、`RETURN_TO_BOARD` 和 `ABORT` 分别映射到 blocked、新 Board window 和 aborted。
Desktop 不展示模型隐藏推理、工具调用日志，也不根据工具调用猜测行动是否完成。

### 17.5 Human 主持的行动收口

Human 主持不依赖 ACP slot，也不填写 Meeting 专用行动表单。行动收口卡提供：

- 冻结的最终 Board；
- `打开 Project View`或 Board 中已有的其他上下文入口；
- `确认行动产出已完成并结束会议`；
- `暂时无法完成`，用于主动报告稳定阻塞原因；
- `返回会议看板`；
- `中止会议`。

Human 从 Meeting 打开现有 Project View 管理页面后，使用该页面原本提供的完整业务能力。
可以创建、更新或删除当前系统支持的任何合法对象和关系，不受 Requirement/Work 子集限制；
也可以改用 CLI 或 Meeting 无法观察的其他系统。完成后回到 Meeting，再作出最终确认。

Human 也可以不发生任何外部写入。例如目标状态已经存在、行动应由其他系统承载，或者主持人
判断最终 Board 本身已经足够。Desktop 不要求操作数量，不把 Project View revision 增长作为
完成条件，也不自动比较 Board 与外部状态。

确认前应使用以下明确文案：

> 我确认，最终会议看板中需要在正常闭会前登记的行动产出，已经完成登记，或确认无需新增
> 登记。此确认不表示相关 Work 已经执行完成。

### 17.6 外部业务操作边界

Meeting 不校验 Requirement、Work、Issue、Role、Assignment 或承接人关系。Project View
管理界面和 typed domain handler 继续负责自身字段、revision、引用与角色约束；其他系统也
继续使用自己的规则和权限。

冻结 roster 不自动成为 Project View 的 assignee 白名单，Meeting 也不自动创建缺失的 Role
或 Assignment。若 Board 中的决定无法在目标系统合法表达，主持人可以在行动阶段继续处理、
进入 blocked、返回 Board 修正会议决定，或中止会议。

行动收口只负责登记会议产出，不表示被分配的参与者已经完成后续 Work。Work 的实际执行属于
其自身生命周期。

### 17.7 Runnable 与截止时间

`runnable` 时，Human 主持人可以继续使用外部业务界面，也可以提交完成确认。Desktop 显示
当前权威截止时间，但不能仅凭本地倒计时自行宣布 blocked；截止后重新读取 Relay 状态。

Human 主持人确认当前窗口无法继续时，可以选择`暂时无法完成`，填写用户可理解的稳定原因
类别和可选简短说明。Relay 接受 block 后才显示 blocked。Agent 主持的 block 由当前逻辑主持 Turn
提交；deadline 到期也可以由 Relay 自动收敛为 blocked。

Meeting 不追踪普通业务命令的进度、成功数量或 receipt。已经提交的外部操作可能在 action
deadline 后才返回或才被观察到，Meeting 不撤销这些效果。旧 action window 的完成确认必须
被拒绝，主持人需要按权威状态恢复。

### 17.8 Blocked、Retry、返回 Board 与中止

被阻塞时，行动卡显示：

- 稳定的用户可理解原因；
- 外部效果不会被 Meeting 自动回滚的提示；
- 主持人当前可用的恢复操作。

恢复规则为：

- Human 主持人在 blocked 状态使用`重试`取得新的 runnable window 和独立 deadline；
- retry 后必须先重新读取目标系统权威状态，再决定补充操作或直接确认完成；
- `返回会议看板`可从 runnable 或 blocked 使用，但必须再次确认“可能已有的外部效果将保留”；
- 返回成功会终结当前 action run 并打开新的 Board Maintenance，主持人再修改 Board、继续讨论、
  直接关闭或重新进入行动收口；
- 返回 Board 不以“零外部效果”为前提，也不表示 Relay 已确认存在外部效果；
- Agent provider/process 真实失败或 lease 到期时，Desktop 只展示 canonical blocked，不允许 Human
  参会者替 Agent 重试或返回 Board；单纯槽或 Session 变化不应生成 blocked；
- 主持人随时可以明确中止，并保留已经发生的外部效果。

### 17.9 确认完成并原子关闭

Human 主持人的`确认行动产出已完成并结束会议`只在当前 action condition 为 runnable 时可用。
一次提交同时携带当前 Board、action run/window 和完成声明：

- Relay 接受前，界面保持“正在确认”，不得先显示 closed；
- Relay 接受后，action run 完成与 Meeting `closed` 在同一事务中成立；
- 不存在先 complete、再 ready、最后 close 的中间产品状态；
- 响应丢失时重放同一已签名 End，不重新创建完成声明；
- stale Board、run/window、blocked condition 或过期 deadline 被拒绝时，Desktop 重新读取权威
  状态并显示合法恢复操作。

## 18. 正常关闭与异常中止

### 18.1 正常关闭

Human 主持人有两条不同的正常关闭路径，Desktop 不得把两者的 gate 混成同一组条件。

**直接关闭**只发生在 discussion 的 Floor Decision 中，要求：

- Control Token 已返回；
- 主持人当前确实控制 Floor，且没有活动 Offer、Grant 或待优先处理的 Human Request；
- 最后一次 Board Maintenance 已以显式 `updated | unchanged` 完成；
- Board 已表达最终进展和有效结论；
- Meeting 尚未进入 Action Finalization。

**行动收口后关闭**只发生在 `finalizing_actions/runnable`，要求：

- Board 和 action run 仍绑定进入 Action Finalization 时冻结的权威结果；
- action run/window 和 Board fence 均为当前权威值；
- runnable deadline 在 Relay 校验时仍有效；
- End 携带“行动产出已记录”声明；
- 同一冻结主持身份提交 End。

Action Finalization 已经冻结 speech-floor，因此该路径不再要求重新取得 discussion 的 Control
Token，也不检查已经终结的 Offer、Grant 或 Human Request。Desktop 不查询 Project View，
不要求至少一次外部写入，也不等待一个不存在的 `ready_to_close` 状态。

两条路径的关闭确认界面都应明确说明：

> 正常结束表示主持人认为会议目标已达到，并已形成足以结束本次会议的有效结论。

行动收口路径还必须明确说明：确认代表主持人判断闭会前所需的行动产出已经完成登记或无需
新增登记，不代表被登记的后续 Work 已经执行完成。

### 18.2 异常中止

主持人中止时必须填写或选择：

- 稳定的 reason code 对应类别；
- 可选或按具体原因要求的简短说明。

中止确认必须说明：

- Meeting 不会被标记为目标已达成；
- 当前 Speech、Intent、Request、Offer、Grant 和控制流程都会终止；
- 已产生的外部系统效果不会自动回滚；
- 中止后不可恢复 Meeting。

Operator 或安全系统触发的 abort 使用相同终态展示，但标明终止来源。

### 18.3 终态页面

`closed` 页面显示：

- 已正常结束；
- 主持人和结束时间；
- 最终 Board；
- 完整正式 Speech timeline；
- 是否经“行动产出已记录”确认关闭；
- Board 原有的 Project View 与其他上下文链接，在存在时显示。

`aborted` 页面显示：

- 已中止；
- 原因和终止来源；
- 最后 Board；
- 完整正式 Speech timeline；
- 若曾进入行动收口，显示“外部系统中可能已有保留效果”的提示。

Meeting 不掌握普通业务操作清单，因此终态页不得声称列出了全部行动结果、部分成功项或未完成
项。目标系统中的实际状态应通过其现有页面查看。

两种终态都永久只读，不提供 reopen。

## 19. Attention、未读与通知

Meeting 需要区分“有新内容”和“当前身份必须操作”。

### 19.1 可以产生普通未读的内容

只有新的 canonical Speech 可以按普通会话内容计算未读。

### 19.2 产生 attention 的状态

以下情况可以产生高优先 attention：

- 当前 Human 收到 Offer；
- 当前 Human 获得 Grant；
- 当前 Human 主持人进入 Board Maintenance；
- 当前 Human 主持人需要作出 Floor Decision；
- Human 主持的 Action Finalization 需要确认、重试、返回 Board 或中止；
- Meeting 被阻塞或异常中止。

Attention 在权威状态解除后消失，不等同于消息未读计数。

### 19.3 不产生通知的内容

以下内容不单独产生普通未读或 Board 通知：

- Board update；
- Intent revision；
- Offer/Grant 的底层进度事件；
- ACP heartbeat；
- action run/window 的底层控制事件；
- 当前身份无需处理的 Floor 状态变化。

Desktop 可以使用实时事件使当前页面失效并重新读取权威状态，但这不构成用户可管理的 Board
订阅或变更通知产品。

## 20. 外部上下文

### 20.1 来源频道

存在 source 时，标题栏或 Board 引用区显示紧凑入口。打开 source 使用现有 Channel 体验，
不在 Meeting 页面复制完整 Channel timeline。

### 20.2 Project View

存在 Project View 引用时：

- 以链接或紧凑引用卡显示；
- 点击进入当前 Community 的现有 Project View 页面或 Inspector；
- 返回 Meeting 后恢复 Meeting timeline、Board 和 Floor 状态；
- Project View 不可用时，只显示引用不可用，不阻止 Meeting。

Human 主持进入 Action Finalization 后，也通过同一个现有 Project View 页面完成普通业务操作；
Meeting 不提供专用 Requirement/Work 表单，不限制可操作的对象类型，也不会自动收集本阶段
创建或修改的对象作为 Meeting 结果。Board 没有 Project View 引用、Project View 功能不可用
或主持人没有发生 Project View 写入，都不阻止其按判断完成行动收口。

### 20.3 其他引用

消息、文档、代码、仓库位置和 URL 继续使用各自已有的安全打开方式。Desktop 不保证所有
participant 对任意外部 URL 都有权限，也不因此改变 Meeting roster。

## 21. 响应式与窗口行为

### 21.1 宽屏

- timeline 与 Board 同时可见；
- Board 默认打开且可调整宽度；
- 主持人控制台固定在中央内容底部；
- 参会者面板使用右侧辅助面板或 Sheet。

### 21.2 中窄窗口

- timeline 保持主页面；
- Board 作为可覆盖的右侧 Sheet；
- Host Console 和 Floor Dock 不被 Board 覆盖后永久丢失；
- 收起 Board 后仍可从标题栏一键返回；
- Offer、Grant 和主持人 deadline 等需要立即操作的控件优先于低优先信息。

### 21.3 状态恢复

窗口刷新、应用重启或从其他 Community 返回后：

- 重新查询当前 Meeting 权威状态；
- 恢复 timeline 位置可以是本地体验能力；
- 不恢复已失效的 Offer、Grant 或主持控制按钮；
- 本地 Board 草稿只有在原 Board 窗口仍有效时才可继续提交，否则只能作为不可提交草稿查看；
- Community 切换必须清除前一个 Community 的 Meeting 单例状态和待提交操作。

## 22. 可访问性与输入体验

Meeting 的关键操作必须支持键盘和辅助技术：

- Board、timeline、Floor Dock 和 Host Console 具有清晰区域标题；
- Offer、Grant、Board 超时和 Action blocked 使用可读状态，不只依赖颜色；
- 状态变化使用克制的 live region，避免每个底层事件连续朗读；
- destructive 的 Abort 与普通 Close 视觉和文案明确区分；
- focus 在状态切换后移动到当前首要操作，但不得打断正在阅读的 timeline；
- Speech composer、Board editor 和行动收口卡片的可读文本使用 Desktop 既有 rem 字号体系；
- Agent、Human、主持人和当前 speaker 不只用头像外观区分。

快捷键不得绕过确认或协议状态。具体快捷键在实现阶段确定。

## 23. 权威状态、失败与恢复

### 23.1 不乐观宣布控制结果

以下操作提交期间显示 pending，但在 Relay 接受前不改变权威界面状态：

- 申请或撤回 Human Request；
- 接受或拒绝 Offer；
- 提交 Speech 或 Yield；
- 更新或确认 Board 不变；
- 选择、拒绝或 dismiss Intent/Handoff；
- Recall；
- Close 或 Abort；
- Action begin、block、retry、return-to-board，以及带完成声明的 End。

### 23.2 命令过期或冲突

命令因 epoch、window、revision 或当前状态不匹配被拒绝时：

- 停止 pending；
- 重新读取 Meeting、Floor、Board 或 Action 权威状态；
- 说明原操作已过期；
- 不自动对新状态重放具有不同语义的操作；
- Board 草稿或 Speech 草稿可按安全边界保留为本地文本，但不能自动提交。

### 23.3 断线

断线时可以显示最后一次已验证状态，但必须标明：

```text
连接已中断，无法确认当前会议状态
```

断线期间：

- 禁止依赖当前 Offer、Grant、Board window 或 Action state 的写操作；
- 不继续显示本地倒计时后自行判定超时结果；
- 重连后先同步权威状态再恢复操作；
- 如果当前 Human 已经失去 Grant，不恢复旧 composer 的发送能力。

### 23.4 Capability

Desktop 区分：

- Relay 不支持 Meeting；
- Relay 支持读取已有 Meeting，但当前 create gate 关闭；
- Relay 支持普通 V2，但某个兼容会议不支持 Action Finalization；
- 新建 action-capable V2 时，某个 roster Agent 缺少
  `meeting-v2-action-finalization-v4` runtime capability；
- 当前 Community 的 Project View 页面不可用，但 Action Finalization 本身仍可继续。

不可用能力使用明确说明，不能用永久 loading 或普通网络错误代替。已有 Meeting 的读取能力不
应仅因新建 gate 关闭而消失。Project View 不可用不能被误报为 Meeting action capability
不可用，也不能自动禁止“确认无需新增登记”。

## 24. 隐私、身份与权限边界

Desktop 必须保持：

- 只有冻结 roster 可以读取 Meeting Board、Speech、Intent 和 Floor log；
- 非 roster deep link 不能泄漏标题、Board、participant 或 Speech；
- 当前身份签名所有 Human 操作；
- Human/Agent 和主持判定使用 Meeting Create 时冻结的 participant type 与 moderator identity，
  不用当前 Profile、Channel role 或 managed-by 重新推断；
- managed Agent owner 关系不自动赋予冒充 Agent 的签名能力；
- Meeting 主持权不新增会议外权限；
- Project View 写入继续经过现有 typed domain handler 和校验；
- source 或 Board 链接不扩大目标资源权限；
- 终态历史继续遵循同一 roster 可读边界。

当前 Community 内的 Agent 权限约定由既有系统负责，本规格不新增 Meeting 专属权限管理。

## 25. 关键验收场景

### 25.1 发起与发现

1. Human 从 Meetings 分组创建无 source Meeting，选择 Human 与 Agent，自己成为主持人。
2. Human 从普通 Channel 发起 Meeting，source 被预填但可以移除。
3. source 对某个 participant 不可读时，创建前明确阻止并给出调整方式。
4. 创建成功后，所有 roster 成员可以发现房间，无 RSVP 或 Join。
5. 非 roster 身份即使得到 deep link 也不能读取 Meeting 内容。
6. 选择缺少 action-finalization capability 的 Agent 时，Desktop 明确指出目标并阻止创建，
   不静默降级 policy；检查的 capability 是 `meeting-v2-action-finalization-v4`。

### 25.2 Board 与主持顺序

1. Human 主持人取得控制后，Board 自动进入合法编辑状态。
2. 未完成 Board Maintenance 前，下一席选择不可操作。
3. `保存并继续`后，Floor Decision 获得独立完整操作阶段。
4. `看板无需修改`与 Board timeout 被明确区分。
5. Board timeout 后可以继续讨论，但 Close 和 Finalize Actions 保持不可用。
6. Board 编辑期间 Human Request 抢占，旧窗口不能迟到提交。
7. 普通参会者始终只能读取 Board。
8. Board 获取失败时不能基于缓存执行主持修改。

### 25.3 Intent 与发言权

1. Agent Intent 出现在主持人 Intent 列表，但不混入正式 Speech timeline；其他客户端提交的
   合法 Human Intent 也按同一 pending SpeechIntent 语义呈现。
2. Human 主持人选择 SpeechIntent 后先显示 Offer，不能直接显示 Grant。
3. Agent Decline 或 Offer timeout 后，Intent 按权威状态保持或恢复 pending。
4. Human Request 独立显示并拥有协议优先级，主持人不能拒绝或重排。
5. Human ACK 后只有 Relay 创建 Grant 才打开 Speech composer。
6. 每个 Grant 最多成功发送一条 Speech。
7. Directed Handoff 显示目标和原因，并遵守深度、Recall 和 Human priority。
8. Human 主持人自发言先填写并维护一句 self Intent，再由本人响应 self Offer，Relay 创建 Grant
   后才能发言。
9. Self Offer 失败后 self Intent 不被静默消费，主持人可以重新选择、刷新或撤回。
10. Human Floor Request 操作只按 Create 时冻结的非主持 Human 类型开放，不按当前 Profile 或
    Channel role 推断。
11. Directed Handoff 或 fallback 指向 Human 主持人时，Host Console 同样提供 ACK/Decline、
    Grant composer 和 Yield，不要求先创建新的 self Intent。

### 25.4 Agent 会议

1. Agent 主持时，Human participant 能看到 Board、Intent 状态和主持进程。
2. 非主持 Human 看不到可提交的 Agent 主持按钮。
3. Agent participant 的 ACK、Speech 和 Yield 不由 Human Desktop 代操作。
4. Agent slot/session 变化不要求 Human 选择槽或重新 Join。

### 25.5 Action Finalization

1. 没有行动的 Meeting 可以在最终 Board 后直接 closed。
2. 进入 Action Finalization 后 Board 和 Floor 均冻结，Meeting 仍非终态。
3. Agent 主持行动由同一逻辑主持 Agent 的唯一 Turn 使用普通业务工具执行；fallback 槽也收到完整
   frozen Board 与 canonical envelope，Desktop 只读展示稳定状态。
4. Human 主持可以从 Meeting 打开现有 Project View 页面完成任意合法业务操作，再返回 Meeting。
5. Human 路径不存在 Requirement/Work 专用表单、业务预览、Plan、Step 或 Meeting materializer。
6. Human 即使没有发生任何外部写入，也可以确认产出已登记或无需新增登记。
7. Desktop 不展示 operation count、逐项 receipt 或“Relay 已验证全部行动”等误导信息。
8. `runnable` 时一次完成确认原子关闭 Meeting，不经过 complete 或 ready-to-close 中间状态。
9. Human 主持人可以主动报告阻塞，deadline 到期也会使 Meeting 进入 blocked。
10. blocked 后 Human 主持人必须 retry 取得新 window，才能确认关闭。
11. runnable 或 blocked 都可返回 Board，但必须明确确认可能已有的外部效果继续保留。
12. stale run/window/Board 的完成确认不能关闭 Meeting。
13. action abort 保留可能已经发生的外部效果并显示 `aborted`。

### 25.6 终态与恢复

1. `closed`显示目标达成语义、最终 Board、Speech，以及是否经行动产出确认关闭。
2. `aborted`显示中止原因；曾进入行动收口时提示外部效果可能保留，不冒充正常结束，也不
   声称 Meeting 掌握完整操作清单。
3. 两种终态都不可发言、改 Board 或 reopen。
4. 断线期间不允许基于过期 Grant、Board window 或 Action state 提交。
5. 重连后按 Relay 权威状态恢复，不产生双 Speech、双选择或重复 Meeting 完成声明。
6. Community 切换不泄漏另一个 Community 的 Meeting 状态和草稿。

## 26. 实现阶段再确定的细节

以下内容不改变本规格，可以在阶段实现时结合现有 Desktop 架构确定：

- 具体 route 文件和页面组件边界；
- Tauri command 与 React Query hook 的拆分；
- Meeting 右侧面板的默认宽度和响应式断点；
- 图标、颜色、动画和精确间距；
- Board editor 的具体组件和 Markdown 编辑体验；
- 状态条和活动记录的精确事件折叠规则；
- action 收口卡片、外部业务页面入口和返回 Meeting 的具体导航方式；
- Meeting history 的分页方式；
- 快捷键；
- E2E mock bridge 的 event seed 形状；
- Desktop feature gate 和 rollout 配置名称。

这些细节不得改变本文固定的身份、Board/Floor 顺序、Human priority、行动生命周期和终态
语义。

## 27. 完成定义

Meeting Desktop 可以认为产品交付完成，需要同时满足：

1. `room_kind=meeting` 能稳定驱动 Meeting 专用导航和页面模式；
2. Human 可以创建固定 roster Meeting，并理解不存在 RSVP 或 Join；
3. Human participant 可以完成完整 Floor 生命周期；
4. Human host 可以完成 Board、Intent、Floor、Close、Abort 和 Action Finalization 操作；
5. Agent 主持和参会状态可观察，但 Human 不能冒充 Agent；
6. Board 和 Speech timeline 同时可用且互不替代；
7. Board Maintenance 与 Floor Decision 在界面和时间预算上明确分离；
8. Human Request、Directed Handoff 和主持选择保持协议优先级；
9. Project View 保持可选，Meeting 不产生专用或隐式写入；Human/Agent 只使用现有业务入口；
10. Action Finalization 留在 Meeting 生命周期中直到 `closed | aborted`；
11. 断线、过期、冲突和恢复不产生虚假控制权或重复 Meeting 完成声明；普通业务操作继续遵循
    各目标系统自身的幂等与恢复语义；
12. 自动化测试覆盖本文关键验收场景，并通过 Human 手动端到端验收。
