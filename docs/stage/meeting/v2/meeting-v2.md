# Meeting V2：主持人维护的共享会议看板

> 状态：产品语义规格
>
> 范围：定义 Meeting V2 的共享会议看板、主持流程、Agent 上下文和闭会语义。
>
> 非范围：事件 kind、wire schema、数据库、API、CLI、Desktop UI、超时数值和代码组织；
> 这些内容在实现设计中单独讨论。
>
> 基线：Meeting V2 建立在
> [Meeting V1：主持式发言权接力协议](../v1/meeting-v1.md) 之上。

## 1. 文档目的

Meeting V1 已经解决 Human 与 Agent 在同一私有会议中如何表达发言意图、取得唯一发言权、
定向接力、由主持人选择下一位发言者，以及如何在超时和故障后恢复。

V1 不负责表达会议为什么召开、当前讨论到哪里、主持人认为已经形成了什么结论。缺少这些
信息时，Human 只能从完整消息历史中自行归纳，Agent 也容易在不同 Turn 中失去会议方向。

Meeting V2 在 V1 之上增加一份由主持人维护、所有参会者可查看的共享会议看板。看板记录
当前会议目标、议程、进展、讨论结果和可选上下文；主持人在安排下一席之前先维护看板，
参会 Agent 在需要理解会议的语义节点按需读取当前看板。

V2 的核心取舍是：

> 看板保存会议当前方向，主持人理解并推进会议，V1 继续保证发言权协议。

看板不是会议模板，不是通用工作流，也不是 Project View 的附属对象。

## 2. 核心流程

一场 V2 会议的最小产品生命周期为：

```text
发起会议并写入初始看板
        ↓
active
        ├── 主持与发言循环
        ├── 主持人认为目标已达成并形成有效结论 → closed
        └── 无法正常继续或安全终止             → aborted
```

V2 初版不把 proposed、preparing、convening、ready、closing 等概念实现为独立的权威
阶段。需要表达的准备事项、讨论步骤和收尾事项直接写在看板议程中，由主持人推进。

当主持人取得新的主持行动机会时，一次主持控制周期为：

```text
主持人取得或继续持有 Control Token
        ↓
读取当前看板
        ↓
Board Maintenance
  ├── 需要修改：更新看板
  └── 无需修改：保持看板不变
        ↓
重新取得当前看板
        ↓
Floor Decision
  ├── 选择一名参会者
  ├── 主持人申请自己发言
  ├── 暂不安排发言
  └── 正常关闭会议
```

Board Maintenance 与 Floor Decision 是两个连续但独立的主持工作。二者不得共享同一
时间预算；Floor Decision 只能在 Board Maintenance 结束后取得完整的独立时间预算。

## 3. 目标

Meeting V2 要做到：

1. 为所有参会者提供同一份当前会议目标、议程和进展；
2. 让主持人在不引入流程引擎的前提下持续维护会议方向；
3. 保证主持人需要修改看板时，修改先于其下一次发言权安排；
4. 让 Human 和 Agent 都可以担任主持人或普通参会者；
5. 让 Agent 在 Intent、正式发言和主持判断时使用当时的当前看板；
6. 避免看板处理耗时侵占主持人的发言权决策时间；
7. 由主持人判断目标是否达成、结论是否有效，并据此正常关闭会议；
8. 允许看板引用 Project View 或其他上下文，但不要求任何外部系统存在；
9. 保持 V1 已有的私有名单、唯一 Grant、Human 优先、定向接力和恢复语义。

## 4. 非目标

Meeting V2 初版不提供：

- 会议模板或模板实例化；
- 必填的会议类型；
- 可编程的会议流程或任意状态图；
- 系统自动理解、排序或推进议程；
- 法定人数、投票、表决或多人确认；
- 由系统判断讨论是否充分、结论是否正确；
- 看板业务版本、版本比较或 revision fencing；
- 看板修改历史的产品语义；
- 看板变更通知或 Agent 长期订阅；
- 多人协同编辑看板；
- 动态参会名单或正式主持权转移；
- 因会议发言、看板更新或闭会而自动修改 Project View、Workflow、Git 或其他外部系统；
- 用看板替代完整 speech timeline 和 floor control log。

## 5. 与 Meeting V1 的关系

### 5.1 继承的协议语义

除本文明确调整的产品语义外，V2 继续采用 V1 的主持式发言权接力协议，包括：

- `ControlToken` 与一次性 `SpeechGrant` 是不同对象；
- 主持人也不能绕过 Grant 直接发表正式 speech；
- 同一时刻至多存在一个活动 Offer 或一个活动 Grant；
- 每个 Grant 最多接受一条正式 speech；
- Human Floor Request 保持 V1 的下一席优先级；
- Directed Handoff、Recall、接力深度和强制归还主持人的规则保持不变；
- Relay 权威校验、确定性 fallback 的候选顺序、幂等和恢复语义保持不变；
- 会议仍使用私有固定名单，非参会者不能读取会议内容；
- End 仍是高于所有 Floor 状态的终态；
- Community 管理和安全撤权仍可异常终止会议。

V2 不重新定义 V1 的 Floor 状态机，只在主持人拥有 Control Token 的窗口中加入看板维护和
闭会判断。

### 5.2 V2 的明确调整

V1 允许创建者显式指定另一名主持人。V2 初版收紧为：

> 发起会议者就是主持人。

创建者、Meeting Owner 和 moderator 在 V2 初版中是同一身份。V2 不提供创建时指定其他
主持人，也不提供会中主持权转移。

V2 还调整主持决策时间预算的起点：主持人的 Floor Decision 时间只能在本轮 Board
Maintenance 完成或超时后开始。Board Maintenance 不得提前消耗或触发 V1 的主持决策
fallback；其他 Offer、Grant 和发言相关 deadline 继续遵循 V1。

### 5.3 产品代际与协议版本

“Meeting V2”是本文使用的产品代际名称。Meeting V1 的现有 wire schema 已经使用过
`v=2`；本文不指定新的 wire schema 数值，也不允许据产品名称推断协议版本。具体版本和
兼容方式属于后续实现设计。

## 6. 角色

### 6.1 发起者与主持人

V2 发起者自动成为唯一主持人。主持人负责：

- 创建初始会议看板；
- 维护会议目标、议程和当前进展；
- 把讨论中形成的共识、问题和结论整理到看板；
- 在 Control Token 返回时先处理看板，再安排下一席；
- 按 V1 规则选择、拒绝或延后发言意图；
- 判断会议是否已经达到讨论目标并形成有效结论；
- 在满足上述判断后正常关闭会议。

主持人可以是 Human，也可以是 Agent。主持权不赋予绕过 Speech Grant 的发言能力，也不
赋予会议之外的外部系统权限。

### 6.2 普通参会者

Human 与 Agent 普通参会者继续拥有 V1 规定的参会能力，并增加：

- 在需要时读取当前会议看板；
- 根据当前目标、议程和进展形成 Intent 或正式发言；
- 在发言中建议主持人调整议程、记录共识或修改结论。

普通参会者不能直接修改看板。对看板的建议必须通过正常会议发言表达，是否采纳由主持人
决定。

## 7. 会议看板

### 7.1 定位

Meeting Board 是会议范围内的一份当前共享文档。它是参会者理解“为什么开会、当前讨论
什么、已经推进到哪里、主持人记录了什么结论”的入口。

看板不替代 speech timeline：

- speech timeline 保存实际发生的正式发言；
- floor control log 保存发言权和主持控制过程；
- 看板保存主持人维护的当前目标、议程、进展和归纳结果。

看板内容是主持人的会议归纳，不自动成为外部系统中的事实、任务、决策或授权。

### 7.2 逻辑内容

初始看板应能够表达：

- 会议讨论目标；
- 有序会议议程；
- 当前正在推进的议程项或进度；
- 讨论记录、当前共识和未决问题；
- 最终结论；
- 可选相关上下文。

本文只规定这些逻辑能力，不规定具体字段、排版、编辑器或序列化形状。主持人可以在会议
期间增加、删除、调整或重新组织议程，也可以更新目标、记录和结论。

### 7.3 单写者语义

看板采用单写者模型：

- 只有当前主持人可以正式修改看板；
- 所有当前参会者都可以读取当前看板；
- 非参会者不能读取；
- 看板不是多人协作文档；
- 参会者同时提出不同建议不会产生看板写冲突，由主持人统一归纳。

### 7.4 当前状态语义

V2 只要求提供“当前看板”，不要求参会者管理看板版本：

- 不暴露业务 revision；
- 不要求参会者比较版本；
- 不要求从一组 patch 重建当前看板；
- 不要求提供看板版本历史；
- 不要求看板更新时通知所有参会者；
- 不要求 Agent 订阅看板变化或长期维护本地副本。

看板是否具有底层审计、持久化或幂等信息属于实现设计，不构成 V2 产品语义。

### 7.5 可选上下文

主持人可以在看板中加入与会议有关的上下文，例如：

- Project View 中的对象或视图；
- 文档；
- 代码或仓库位置；
- 消息和历史讨论；
- URL；
- 其他参会者可访问的资源。

这些引用全部可选。没有关联对象、Project View 未初始化、引用失效或外部系统暂时不可用，
都不得阻止通用会议继续进行。看板引用某个对象也不表示会议有权修改该对象。

## 8. 发起会议

V2 不经过模板选择或模板展开。发起者直接：

1. 创建会议；
2. 确定固定参会名单；
3. 自动成为主持人；
4. 写入初始看板；
5. 进入可讨论的 active 状态。

发起者可以在初始看板中写入会议目标、议程、已有背景、待确认问题和可选上下文。初始看板
不要求关联 Project View，也不要求预先存在候选结论。

V2 初版不增加 RSVP、法定人数或独立 ready gate。参会身份和访问范围继续使用 V1 的固定
名单语义。

## 9. 看板的按需读取

### 9.1 Human

所有当前参会 Human 都可以在需要时查看当前看板。打开、返回或刷新看板时读取当前内容
即可；是否自动刷新以及如何呈现属于 UI 设计，不属于本规格。

### 9.2 Agent

Agent 不因看板更新而启动 Turn，也不订阅看板变化。会议代码在以下语义节点先获取当前
看板，再把它注入对应 Agent Turn：

1. `Participant Intent Turn`；
2. `Granted Speech Turn`；
3. `Moderator Board Maintenance Turn`；
4. `Moderator Floor Decision / Close Turn`。

一次读取只服务于当前 Turn。特别是：

- Intent 时读取过看板，不表示获 Grant 后可以复用同一份内容；
- Intent 和正式发言之间允许主持人修改看板；
- 获得 Grant 后必须再次取得当前看板；
- Agent 应按照发言时的当前看板调整内容，若原 Intent 已不再适合，可以 Yield；
- Board Maintenance 完成后，Floor Decision 使用处理完成后的当前看板；
- 不得在获取当前看板失败时静默使用一份已知旧副本启动语义 Turn。

看板读取失败后的重试、等待和最终回退方式属于实现设计，但必须保持“旧看板不冒充当前
看板”的产品边界。

## 10. 主持控制周期

### 10.1 触发

以下任一情况都会让主持人获得一次新的看板维护和闭会判断机会：

- speech、Yield、Expiry 或其他 V1 路径使 Control Token 真正返回主持人；
- 主持人已经持有 Control Token 并保持 idle，后来因新 Intent 或其他可处理工作需要重新
  作出 Floor Decision。

第二种情况不要求 Control Token 先离开再返回。即使没有新的 speech，主持人重新开始一次
选择之前也要先处理当前看板；若没有新进展，可以直接保持看板不变。

即使当前没有 pending Intent 或 open Handoff，主持 Agent 也需要有机会：

- 更新刚刚形成的会议进展；
- 记录结论；
- 判断目标是否已经达成；
- 决定正常关闭或保持等待。

完成上述处理且没有下一步动作后，会议可以安静地保持 idle，不产生空循环。

### 10.2 Board Maintenance

主持人先读取当前看板，并基于最新 speech 和控制状态判断：

- 看板需要变化：先完成修改；
- 看板无需变化：保持当前内容；
- 当前信息不足：也可以保持当前内容，继续讨论。

看板更新不是正式 speech，不消费 Speech Grant，也不出现在 speech timeline 中。

主持人只有在 Control Token 属于自己、且不存在活动 Offer 或 Grant 时，才能正式修改看板。
一旦下一份 Offer 或 Grant 已经产生，看板在该次发言机会结束前保持不变。

### 10.3 Floor Decision

Board Maintenance 完成后，主持人才进入本轮 Floor Decision。主持人可以按照 V1 规则：

- 选择 pending SpeechIntent；
- 选择需要重新处理的 open Directed Handoff；
- 安排自己发言；
- 保持 idle；
- 正常关闭会议。

主持人作出选择时，以 Board Maintenance 完成后的当前看板作为会议方向。

### 10.4 独立时间预算

Board Maintenance 与 Floor Decision 必须拥有独立的时间预算：

- Board Maintenance 的读取、判断、修改和重试不消耗 Floor Decision 的时间；
- Floor Decision 的时间只在 Board Maintenance 结束后开始；
- Board Maintenance 超时不能导致 Floor Decision 只剩余部分时间或立即超时；
- Board Maintenance 超时时，看板保持原状，然后进入拥有完整时间预算的 Floor Decision；
- 该超时只表示“本轮没有完成看板修改”，不能表述为主持人主动确认看板无需变化。

具体时长、重试次数和调度策略不在本规格中定义。

### 10.5 Human Request 与 Directed Handoff

“先维护看板，再传递发言权”约束的是主持人拥有 Control Token 时作出的下一席决定。它不
改变 V1 的两条直接推进路径：

1. Human Floor Request 继续按 V1 优先级取得下一席；
2. 当前 speaker 发起的合法 Directed Handoff 继续按 V1 直接产生下一次 Offer。

这些路径没有经过主持人的 Floor Decision，因此不插入 Board Maintenance。结果是：

- Human 队列连续发言期间，看板可以暂时不变；
- Directed Handoff 链期间，看板可以暂时不变；
- 看板不承诺在每条 speech 后立即更新；
- 每个 Agent speaker 仍在自己的 Granted Speech Turn 开始前读取当时的当前看板；
- Control Token 最终返回主持人后，主持人统一归纳这段讨论并推进看板；
- 主持人认为必须尽快收束到看板时，可以使用 V1 已有的 Recall 语义阻止后续 Directed
  Handoff，并在 Human 优先级允许后让控制权返回；已经排队的 Human 仍可能先按 FIFO
  发言。

若 Board Maintenance 进行期间到达 Human Floor Request，Human 不等待看板工作完成，
继续按 V1 直接取得优先路径。主持人因此失去当前控制窗口时，尚未生效的看板修改不能随后
迟到落地；Control Token 再次返回后，主持人获得新的 Board Maintenance 机会。

## 11. 议程推进

会议议程直接写在看板上，由主持人维护和推进。

主持人可以根据讨论情况：

- 标明当前议程项；
- 将议程项视为完成或跳过；
- 增加新议题；
- 调整后续顺序；
- 重新打开已经讨论过的问题；
- 记录仍未解决的问题；
- 整理阶段性或最终结论。

系统不解释议程内容，不根据 speech 自动修改进度，也不判断某个议程项是否真正完成。普通
参会者可以提出建议，但只有主持人可以把建议反映到当前看板。

因此，V2 的会议进程不是由模板或程序状态机驱动，而是由主持人的显式看板维护驱动。

## 12. 正常关闭

是否正常关闭会议由主持人判断。正常关闭表达以下语义声明：

> 主持人认为会议讨论目标已经达到，并且讨论形成了足以结束本次会议的有效结论。

正常关闭发生在 Control Token 已经返回主持人之后。主持人应当：

1. 读取当前看板；
2. 完成本轮最后一次 Board Maintenance；
3. 把最终目标进展和有效结论写入看板；
4. 再作出关闭决定。

系统只确认这是主持人的决定，并保持程序顺序；系统不判断结论的事实正确性、质量、完整性
或外部效力。普通参会者、投票结果和 Project View 状态都不是 V2 正常关闭的必要条件。

关闭后继续继承 V1 的终态语义：活动 Intent、Request、Offer、Grant 和主持控制全部终结，
会议历史转为只读。

## 13. 异常终止

以下情况不应冒充“目标达成并形成有效结论”的正常关闭：

- 主持人认为会议无法继续或无法形成有效结论；
- 主持人长期不可用且无法恢复；
- 参会身份被安全撤权；
- Community 管理员因安全或运营原因强制终止；
- 其他使会议无法正常完成的故障。

这些情况在产品语义上记为 `aborted`。它仍然进入 V1 的不可恢复终态，但不宣称会议目标
已经达成。

V2 初版不支持正式主持权转移。主持人暂时不可用时等待其恢复；确定无法恢复时，由已有
授权主体异常终止会议。主持权接管或副主持人属于后续版本。

## 14. Agent 语义

### 14.1 主持 Agent

主持 Agent 的看板维护和发言权决策是两个不同的语义工作：

```text
Board Maintenance
  → UPDATE BOARD | LEAVE UNCHANGED

Floor Decision
  → SELECT | MODERATOR SPEAK | IDLE | CLOSE
```

主持 Agent 在 Board Maintenance 中归纳进展，在 Floor Decision 中基于处理后的当前看板
决定下一步。模型对看板或流程的理解不是 Relay 权威状态；它不能绕过 V1 权限、Human
优先级、Grant 或 deadline。

### 14.2 参会 Agent

参会 Agent 在 Intent Turn 中用当前看板判断自己是否有内容值得表达，在 Granted Speech
Turn 中重新读取看板并形成正式发言。

看板中的文字不能授予 Agent 新权限，也不能覆盖其系统指令、会议协议、当前 Grant、工具
边界或外部系统授权。看板引用某项工作，不表示 Agent 已经获得执行该工作的权限。

### 14.3 全 Agent 会议

V2 允许主持人和所有参会者都是 Agent。全 Agent 会议仍遵循同样的看板访问、主持顺序、
Speech Grant、超时和闭会语义，不因为缺少 Human 而自动增加表决或确认要求。

## 15. 外部上下文与外部效果

看板可以引用任意可选上下文，但 Meeting 本身保持独立：

```text
Meeting Board
    ├── Project View reference?   optional
    ├── document reference?       optional
    ├── message reference?        optional
    ├── repository reference?     optional
    └── no external reference     valid
```

会议中的 speech、看板内容和正常关闭都只表达会议内部记录。它们不会自动：

- 修改 Project View；
- 创建或关闭 Work/Issue；
- 修改代码或 Git；
- 触发 Workflow；
- 向第三方系统写入；
- 把主持人的结论提升为外部治理决定。

未来若需要外部效果，必须由独立、显式、另行授权的机制完成，不属于本规格。

## 16. 安全与活性不变量

Meeting V2 必须始终满足：

1. 一场会议恰好有一名主持人，初始主持人就是会议发起者；
2. V2 初版不发生主持权转移；
3. 看板只有主持人可修改，所有当前参会者可读取；
4. 看板不向非参会者开放；
5. 看板只提供当前内容，不要求业务版本、历史或变更通知；
6. 看板更新不属于正式 speech，也不消费 Speech Grant；
7. 主持人只能在 Control Token 属于自己且没有活动 Offer/Grant 时修改看板；
8. 主持人需要修改看板时，修改必须先于其下一次 Floor Decision；
9. Board Maintenance 和 Floor Decision 不共享时间预算；
10. Board Maintenance 超时不缩短 Floor Decision 的完整时间预算；
11. Human Floor Request 和 Directed Handoff 保持 V1 的直接推进语义；
12. 看板不保证在 Human 队列或 Handoff 链的每条 speech 后更新；
13. Agent 在每个规定语义节点按需读取当前看板，不把旧读取当作长期同步状态；
14. 获取当前看板失败时，不得静默用已知旧看板启动语义 Turn；
15. 正常关闭只能由主持人在完成最后一次 Board Maintenance 后决定；
16. 正常关闭表达“目标已达成并形成有效结论”，异常终止不得冒充该语义；
17. 系统不判断看板内容或结论的正确性；
18. 看板不能覆盖 V1 安全不变量或授予任何外部权限；
19. Project View 和所有其他外部上下文均为可选项；
20. 会议不会因看板或闭会隐式产生外部系统变更。

## 17. 关键场景

### 17.1 Agent 提交 Intent 后议程变化

1. Agent A 在 Intent Turn 开始前读取当前看板并提交 Intent；
2. 在 A 获得 Grant 之前，主持人更新了议程；
3. A 获得 Grant；
4. A 在 Granted Speech Turn 前重新读取当前看板；
5. A 根据新议程发言，或认为原 Intent 已不适合而 Yield。

Intent 时的看板不会被当作发言时的固定快照。

### 17.2 主持 Agent 更新看板后选择下一位

1. Control Token 返回主持 Agent；
2. 主持 Agent 读取当前看板并归纳刚刚结束的讨论；
3. 主持 Agent 完成看板修改；
4. Board Maintenance 的时间预算结束；
5. 主持 Agent 使用独立的 Floor Decision 时间预算读取当前看板；
6. 主持 Agent 选择下一位 speaker；
7. speaker 获 Grant 后读取当前看板并开始发言 Turn。

### 17.3 看板维护超时

1. Control Token 返回主持 Agent；
2. Board Maintenance 未在自身时间预算内完成；
3. 当前看板保持原状，并记录为本轮没有完成更新；
4. Floor Decision 获得完整、未被消耗的独立时间预算；
5. 会议继续使用 V1 的主持决策和 fallback 语义。

该路径不表示主持人主动确认看板无需变化。

### 17.4 Directed Handoff 链

1. 当前 speaker 发起合法 Directed Handoff；
2. 下一位 speaker 按 V1 直接取得 Offer/Grant；
3. 中间不插入主持人的 Board Maintenance；
4. 每位 Agent speaker 在自己的 Granted Speech Turn 前读取当前看板；
5. Control Token 最终返回主持人；
6. 主持人统一把这一段讨论归纳到看板。

### 17.5 正常关闭

1. Control Token 返回主持人；
2. 主持人读取看板和最新讨论；
3. 主持人更新目标进展并记录最终结论；
4. 主持人判断目标已经达成、结论足以闭会；
5. 主持人正常关闭会议；
6. 会议进入只读终态，但不自动产生任何外部变更。

## 18. 完成标准

从产品语义上，Meeting V2 完成应满足：

1. 发起会议者自动成为主持人，并能写入初始看板；
2. 会议可以在没有模板和 Project View 的情况下正常进行；
3. 所有参会者能按需查看当前看板，只有主持人能修改；
4. Agent 的 Intent、Granted Speech、Board Maintenance 和 Floor Decision 都使用在相应
   节点取得的当前看板；
5. 主持人修改看板后再作自己的下一席决定；
6. Board Maintenance 和 Floor Decision 具有互不侵占的时间预算；
7. Human Floor Request 和 Directed Handoff 不被新的看板步骤破坏；
8. 主持人可以通过维护看板推进自由议程，而系统不自动解释议程；
9. 主持人可以在记录有效结论后正常关闭会议；
10. 无法正常完成的会议可以异常终止，且不会被表示为目标已达成；
11. 看板不要求版本管理、历史同步或变更通知；
12. Meeting V2 不隐式改变任何外部系统。

满足以上语义后，具体协议、数据和客户端实现可以在独立设计中展开。
