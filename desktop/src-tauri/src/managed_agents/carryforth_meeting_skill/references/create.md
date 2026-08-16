# 创建 Carryforth Meeting

## 目录

- [目标](#目标)
- [准备创建输入](#准备创建输入)
- [编写初始 Board](#编写初始-board)
- [执行创建](#执行创建)
- [创建后验证](#创建后验证)
- [处理失败与未知投递](#处理失败与未知投递)

## 目标

创建一场当前完整、action-capable 的 Carryforth Meeting。任何 Agent 都可以发起；创建身份会成为该场
Meeting 的主持人。不要选择、解释或暴露旧协议选项，不要用普通 Channel、Thread、Canvas 或 Huddle
代替失败的 Meeting。

## 准备创建输入

1. 确认当前工作适合使用有固定参与者、受控发言、共享 Board 和明确终态的正式 Meeting。若需要的是普通
   协作频道，退出本工作流并使用普通 Channel 能力。
2. 从请求和当前项目上下文提炼：

   - 简洁、可识别的标题；
   - 本次会议必须解决的一个主要目标；
   - 必要的范围与约束；
   - 初始议程；
   - 已知证据、待确认问题和可选上下文；
   - 判断可以关闭会议的条件。

3. 有意识地选择冻结 Roster。当前创建身份会隐式加入并成为不可转移主持人；提供 1–11 名其他参与者。
4. 只使用当前 Community 中已规范解析的稳定 participant pubkey。只有显示名时，用
   `cf --format compact users get --name "<显示名>"` 查询；仅在结果唯一且与当前上下文明确一致时采用返回的
   64 字符 hex pubkey。不要从显示名猜 pubkey，不要重复当前身份，不要加入重复参与者。没有结果或结果歧义
   时，向当前操作者请求澄清；自主发起时则停止创建并报告无法可靠解析的参与者。
5. 仅在确有来源频道且所有参与者已经能够读取时设置 `--source`。先用
   `cf --format compact channels get --channel <channel-uuid>` 检查频道身份和可见性；当读取资格依赖成员关系时，
   再用 `cf --format compact channels members --channel <channel-uuid>` 核对完整 Roster。不要为创建 Meeting
   而修改来源频道成员，也不要用来源频道扩大读取权限。

不要仅因为某个 Role 可能相关就自动邀请其当前承担者。选择能够为本次目标带来真实责任上下文、事实、
约束、决策或执行边界的参与者；避免为了“观点多样”制造虚假的角色分片。

创建输入的当前硬边界是：标题非空且最多 255 个字符；初始 Board 非空、不含 NUL 且最多 65,536 UTF-8
bytes；其他参与者必须为 1–11 名，创建者无需也不得重复列入。若当前 CLI 或 Relay 返回更严格限制，以其
结果为准，不要截断后静默创建另一场含义不同的 Meeting。

## 编写初始 Board

把初始 Board 写成会议当前共享前沿，而不是预设结论、逐字资料仓库或控制面操作说明。推荐按实际需要使用
以下结构；它不是协议 schema，可以删除空节或调整标题：

```markdown
# 目标

说明本次会议要形成什么共同结论。

# 范围与约束

- 已确认的边界、兼容要求和不可违反条件

# 议程

1. 当前需要澄清的问题
2. 需要比较的选择或证据
3. 形成结论与确认行动输出

# 已知上下文与证据

- 可验证的事实及其规范来源
- 仍需回读或确认的引用

# 未决问题与分歧

- 当前未知、异议及需要谁补充什么

# 当前结论

- 尚未形成；或明确标注为待讨论的初始假设

# 关闭前需要决定的输出

- 仅列出需要会议作出决定的结果，不预先声明已经同意

# 关闭条件

- 目标达到、有效结论明确、异议和未知已记录
- 若需要业务物化，最终 Board 已明确对象、期望结果和回读要求
```

遵守以下写作边界：

- 不把用户期待、主持人偏好或初始假设写成已经形成的共识；
- 不复制整份 Project View、文档、代码或历史消息；只保留定位和本次判断需要的摘要；
- 不要求未来 Action Agent 审计 Decision Attempt、Action Begin、adoption、slot、Session、epoch、lease、
  deadline 或其他 Relay/Harness 内部状态；
- 不把 Board 写成第二套 Workflow、权限合同或自动执行脚本；
- 可以引用 Project 对象、文档、代码或消息，但引用本身不授予读取或写入权限。

## 执行创建

创建不是五类托管 Meeting Turn 之一，因此这里由 Agent 直接调用 CLI；Harness 不会把普通对话回复自动转换为
Create 事件。使用当前默认创建路径：

```bash
cf meetings create \
  --title "<会议标题>" \
  --description "<可选简短说明>" \
  --board - \
  --participant <参与者一的稳定 pubkey> \
  --participant <参与者二的稳定 pubkey>
```

把完整 Markdown Board 通过该命令的标准输入传入；如果所用工具无法安全提供标准输入，改用能够原样传递
完整 Markdown 的调用方式，不要把含引号、反引号或换行的 Board 直接拼成未经转义的 shell 参数。

需要来源频道时增加 `--source <channel-uuid>`。不要传隐藏的 legacy `--policy` 或 `--moderator` 覆盖；
当前完整 Meeting 固定创建者为主持人并使用默认 action-capable policy。

遵守当前 System 与 CLI 合同中的环境、密钥和输出规则。绝不读取或输出私钥；不要依赖本 Skill 之外的另一份
Skill 才能安全执行创建流程。

## 创建后验证

1. 检查创建命令的接受结果并取得 `meeting_id`。
2. 使用 `cf --format compact meetings show --meeting <meeting-id>` 回读身份、主持人、状态和终态字段。
3. 使用 `cf --format compact meetings board get --meeting <meeting-id>` 回读当前 Board，确认它是预期完整内容。
4. 使用 `cf --format compact meetings participants --meeting <meeting-id>` 确认冻结 Roster。
5. 向当前操作者或工作上下文报告 Meeting ID、标题、主持人、参与者和已经进入 active；不要声称议程已完成
   或结论已形成。

如果 Relay 拒绝创建，报告准确错误和需要调整的输入。用户指定的输入需要改变时请求确认；自主发起时不要
静默改变目标、Roster 或 Board 后重试。不要改建普通频道，也不要通过旧协议或手写事件绕过拒绝。

## 处理失败与未知投递

`cf meetings create` 每次执行都会生成新的 Meeting UUID，不是天然幂等命令。按失败类别处理：

1. 本地输入校验失败，或 Relay 明确拒绝且结果确认没有被接受：修正或报告精确输入问题；需要改变用户指定
   语义时先请求确认。
2. 网络中断、timeout、进程异常或其他未知投递：不要直接重跑 `create`，否则可能创建第二场重复 Meeting。
3. 先用 `cf --format compact meetings list --include-ended --limit 500` 按标题和状态找到可能匹配的记录；对每个
   候选再用 `meetings show`、`board get` 和 `participants` 核对完整身份、描述、Board 与冻结 Roster。
4. 若能唯一确认已创建，继续使用该 Meeting ID；若仍不能确认，报告“可能已创建”的不确定性和已检查结果，
   等待操作者决定，不猜 UUID、不盲目重建。

创建命令已经返回接受结果和 `meeting_id` 后，即使紧随其后的 `show`、`board get` 或 `participants` 暂时失败，
也只重试只读验证或报告回读失败；绝不因此再次执行 `create`。
