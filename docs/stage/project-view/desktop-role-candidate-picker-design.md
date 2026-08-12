# Desktop Role 候选身份选择器修改设计

> 状态：已实现，自动化验证通过；真实 Community 的最终交互验收由 Human 执行。
>
> 本文把 Project View Role 的 Assignment candidate 从“以公钥为主要输入”改为
> “以 Community 身份目录为主要入口、以公钥为稳定提交值”。协议、Role Continuity
> 双边确认和 Relay 最终授权不变。

## 1. 修改目的

Project View v2 使用公钥标识 Human、Agent、Role proposal 和 Assignment。这是正确的
协议边界：名称和头像可以变化或重复，公钥才是稳定身份。

但是 Desktop 当前把协议标识直接暴露为主要 Human 操作：owner 或 Leader 点击
`Assign` 后，需要在 `Candidate` 输入框输入公钥。即使 Buzz 已经知道 Agent 的名称、
头像、owner 和运行状态，委派者仍无法直接选择 `test-1`、`Honey` 或其他 Community
Agent。

本次修改固定以下产品边界：

```text
Human 选择：Community 中的可读身份
Desktop 提交：该身份的规范化公钥
Relay 决定：候选资格与 Role/Assignment mutation 是否成立
```

## 2. 已确认的当前实现

### 2.1 当前 Candidate 控件

`../../../desktop/src/features/project-view/ui/ProjectRoleInspector.tsx` 当前使用：

```text
Input
└── datalist
    └── continuity.members
```

输入框 placeholder 是 `Public key or npub`。`datalist` option 的 value 仍是完整公钥，
名称只作为浏览器可能显示的附属 label。它不是一个能够稳定呈现头像、类型、状态和重名
消歧信息的身份选择器。

### 2.2 为什么未分配 Agent 不在 `continuity.members`

Role Continuity snapshot 中的 `members` 来自 Relay 签名的当前 NIP-43 membership
projection。一个 owner-backed managed Agent 在首次 Assignment 激活之前，可以没有直接
Community role，因此不会出现在这份 membership projection 中。

这不表示 Agent 不能成为 candidate。Relay 的 v2 写事务会从 Community-scoped
`users.agent_owner_pubkey` 加载 managed ownership，并要求：

- Agent 的 owner 是 eligible direct Community member；
- Agent 与 owner 均未被当前 ban 排除；
- proposal 创建和接受时 candidate 仍然 eligible。

offer 被 candidate 接受时，Relay 会在同一原子提交中激活 Assignment，并将 eligible
owner-backed Agent materialize 为 Community member。因而现有 UI 存在一个真实的引导
缺口：

```text
创建 Assignment 前：Agent 合法但不在 continuity.members，只能手输公钥
接受 Assignment 后：Agent 才出现在 membership projection
```

### 2.3 已存在的可复用目录

Desktop 已经有以下数据和表现能力：

- `useManagedAgentsQuery()`：本机保存的 managed Agent 名称、头像、relay URL、运行状态；
- `useRelayAgentsQuery()`：当前 Relay 发布的 Agent directory；
- `useUsersBatchQuery()`：display name、头像、Agent 标识与 owner profile；
- `useRelayMembersQuery()` / Role Continuity membership：当前 Community 直接成员及角色；
- `ProjectViewRoleContinuity`：Roles、Assignments 与 Proposals；
- `UserAvatar`、`PubKey` 和现有 People/Agent 搜索结果行。

因此第一版不需要新的 Project View event、数据库表或 HTTP endpoint。需要的是一个
Community-scoped 候选合并层和明确的选择器组件。

## 3. 目标与非目标

### 3.1 目标

1. Human 可以按名称搜索并选择当前 Community 的 Agent，而不必复制公钥。
2. 首次尚未承接 Role 的 owner-backed managed Agent 也能出现在候选列表。
3. People 与 Agents 使用一个一致的 Candidate 选择语义。
4. 重名身份始终可通过类型、owner 和短公钥安全消歧。
5. 目录严格跟随 active Community，不能混入其他 relay 的本地 Agent。
6. 目录不完整或读取失败时，Human 仍可使用公钥完成合法委派。
7. 最终 command 继续提交规范化公钥，Relay 继续做全部权威资格校验。
8. 键盘、屏幕阅读器和窄窗口可以完成整个选择流程。

### 3.2 非目标

- 不用可变名称替代公钥成为协议身份；
- 不改变 `offer_role`、`request_role`、`accept_proposal` 或 Assignment compound fence；
- 不允许 owner 代替 candidate 接受 offer；
- 不把 Agent 运行状态解释为 Role 身份是否有效；
- 不从当前 Channel 成员推断 Community candidate；
- 不扩大谁能读取 Community Agent/Profile 数据；
- 不在本次修改中重做通用全站 People Picker。

## 4. 目标交互

### 4.1 打开选择器

owner 或有权治理目标 Role 的 Leader 在 Role Inspector 点击：

```text
Assign
```

已有当前 Assignment 时按钮仍显示 `Replace`。两者打开同一个对话框，并保持现有说明：
创建的是 72 小时 offer；candidate 必须自行接受，replacement 在接受时原子提交。

### 4.2 Candidate 主入口

现有自由文本框替换为单选、可搜索的身份选择器：

```text
Candidate
┌──────────────────────────────────────────────────────┐
│ Search people or agents…                             │
├──────────────────────────────────────────────────────┤
│ Agents                                               │
│  [avatar] test-1   Agent · managed by you · f06d…b204│
│  [avatar] Honey    Agent · offline · 1a13…0317       │
│                                                      │
│ People                                               │
│  [avatar] Alice    member · 953d…42aa                │
└──────────────────────────────────────────────────────┘

Use a public key manually…
```

选择后，搜索框收起为选中身份摘要。摘要继续显示短公钥，确保 Human 能在提交前确认稳定
身份，而不是只看到可能重复的 display name。

### 4.3 每一项显示的信息

每个候选结果至少显示：

- avatar；
- 优先解析出的 display name；
- `Agent` 或 Community member role；
- managed ownership（例如 `managed by you`），可解析时显示 owner；
- 短公钥；
- 与本次委派相关的状态。

相关状态包括：

- `Available`：没有 active Assignment 或 open proposal 冲突；
- `Assigned to <Role>`：已有其他 active Assignment，接受新 offer 会按现有 compound
  replacement 语义处理；
- `Offer pending` / `Request pending`：当前已有 open proposal；
- `Current assignee`：已经承担目标 Role；
- `Offline` / `Stopped`：仅为运行提示，不禁止选择。

Role 是持久身份职责，Runtime 是短生命周期执行器。停止运行的 Agent 仍可合法接收 Role
offer，因此运行状态不能成为 eligibility gate。

### 4.4 手动公钥入口

`Use a public key manually…` 是次级入口。展开后保留现有 hex/npub 输入能力，并在本地做
格式校验。

该入口用于：

- Relay directory 尚未同步；
- 合法远端 candidate 没有完整 profile；
- 目录读取发生可恢复失败；
- operator 明确知道一个尚未被 Desktop 发现的身份。

手动输入不能绕过 Relay eligibility。失败时继续显示 Relay 返回的
`candidate_ineligible`、revision conflict 或其他类型化错误。

## 5. Candidate 数据模型

新增纯前端 view model，建议放在：

```text
desktop/src/features/project-view/projectRoleCandidates.ts
```

建议结构：

```ts
type ProjectRoleCandidate = {
  pubkey: string;
  displayName: string;
  avatarUrl: string | null;
  identityType: "agent" | "person";
  communityRole?: "owner" | "admin" | "member";
  ownerPubkey?: string;
  managedByCurrentUser: boolean;
  runtimeStatus?: "online" | "away" | "offline" | "running" | "stopped";
  activeAssignment?: {
    assignmentId: string;
    roleId: string;
    roleName: string;
  };
  openProposal?: {
    proposalId: string;
    proposalType: "offer" | "request";
    roleId: string;
  };
  source: "managed" | "relay_agent" | "member";
};
```

`source` 只用于合并优先级和诊断，不展示为权限证明。若同一公钥来自多个来源，合并为
一个 candidate。

## 6. Community-scoped 数据合并

### 6.1 数据来源

选择器合并：

1. 当前 Role Continuity `members`；
2. 当前 Relay 的 `useRelayAgentsQuery()`；
3. `useManagedAgentsQuery()` 中属于 active Community relay 的记录；
4. 上述公钥的 batch profile；
5. 当前 `assignments` 与有效 `proposals`；
6. current identity 与 active Community 信息。

不能使用当前 Channel member list 作为基线。Role 与 Assignment 是 Community-global
Project View 状态，一个 Agent 不必先加入当前 Channel 才能承担 Role。

### 6.2 本地 managed Agent 的 relay 过滤

`useManagedAgentsQuery()` 返回本地保存的全部 managed Agents，记录本身带 `relayUrl`。
候选层必须过滤到 active Community：

```text
canonicalRelayUrl(agent.relayUrl)
  == canonicalRelayUrl(activeCommunity.relayUrl)
```

必须复用已有 canonical relay identity helper；不得在选择器里重新实现 localhost、默认
端口、大小写或尾部斜杠规则。规范化只用于匹配和去重：

- 实际连接仍使用记录中的原始 URL；
- 不把 `ws://localhost:3000` 改写成 `ws://127.0.0.1:3000`；
- 不修改 managed Agent 配置或 runtime key。

无法规范化或无法匹配 active Community 的记录不进入默认列表。

### 6.3 合并优先级

按规范化公钥去重，字段优先级固定为：

1. 本地 managed Agent：名称、avatar、精确本地 runtime status；
2. 当前 Relay Agent directory：Community 可见 Agent 名称与 presence；
3. verified/profile 数据：display name、avatar、owner；
4. fallback：短公钥。

身份类型采用可信来源的并集：managed record、Relay Agent record、profile `isAgent` 任一
成立即为 Agent。普通 profile 文本不能把一个已知 Agent 降级为 Person。

### 6.4 名称与排序

名称解析顺序：

```text
managed Agent name
→ Relay Agent name
→ profile display name
→ NIP-05 handle
→ shortened pubkey
```

搜索匹配 display name、NIP-05、完整公钥和短公钥。结果先分为 Agents、People，再按：

1. 当前搜索的 exact/prefix match；
2. current target Role assignee；
3. 可用 candidate；
4. display name localeCompare；
5. pubkey。

重名不合并；只有相同规范化公钥才合并。所有重名项都显示短公钥。

### 6.5 Candidate eligibility 的边界

Desktop 目录只做安全的候选发现，不复制 Relay 的完整治理内核。

- direct Community members 来自 verified Role Continuity membership；
- 本地 managed Agent 只有在 active identity 是其 owner，且该 owner 是当前 eligible direct
  member时才进入默认候选；
- Relay Agent 只有在 profile owner 能映射到当前 direct member时才标为可直接选择；
- 无法确认 managed ownership 的 identity 不作为“已验证 eligible”展示，可通过手动公钥
  入口提交；
- archive/已知失效身份从默认结果排除；
- ban、最新 owner eligibility、proposal/assignment fence 和 revision 始终由 Relay 在
  mutation 时重新验证。

UI 文案不得把“出现在目录里”表达为授权保证。

## 7. 组件与代码接缝

### 7.1 纯合并层

新增：

```text
desktop/src/features/project-view/projectRoleCandidates.ts
desktop/src/features/project-view/projectRoleCandidates.test.mjs
```

纯函数负责：

- canonical Community 过滤后的输入合并；
- 公钥规范化与去重；
- 名称/头像优先级；
- Assignment/Proposal 状态装饰；
- 分组、搜索与稳定排序。

纯函数不调用 React Query、不读取 localStorage、不提交 mutation，便于覆盖全部边界。

### 7.2 查询组合 Hook

新增：

```text
desktop/src/features/project-view/useProjectRoleCandidates.ts
```

职责：

- 读取 active Community、current identity；
- 订阅 managed Agents、Relay Agents 和必要 profiles；
- 只在 Role Inspector/offer 对话框需要时启用额外 profile batch；
- 调用纯合并层；
- 返回 `loading`、`partial`、`ready`、`error` 与 candidates。

不得增加 module-level Community cache。现有 Community remount boundary 应自然销毁该
Hook；若实现引入新的 singleton，必须接入 `resetCommunityState()`。

### 7.3 选择器组件

新增：

```text
desktop/src/features/project-view/ui/ProjectRoleCandidatePicker.tsx
```

优先复用现有 `UserAvatar`、`PubKey`、Dialog/Input/Button 样式，以及已有 People/Agent
搜索结果行的视觉语言。不要依赖原生 `datalist`，因为不同 WebView 对 label、键盘行为和
结果样式的支持不一致。

组件输入为 candidates 与 selected pubkey，输出始终是 pubkey。它不构造 Role command。

### 7.4 Role Inspector 集成

`ProjectRoleInspector.tsx` 保留现有：

- `canGovernRole`；
- `offer_role` intent；
- expected project revision；
- current/replacement Assignment fence；
- 72 小时期限；
- optional context；
- candidate acceptance 边界。

只替换 `candidatePubkey` 的采集与展示方式。`submitOffer()` 最终仍收到一个经过本地格式
验证的公钥字符串。

## 8. Loading、失败与并发行为

### 8.1 Loading

打开 Assign 对话框时：

- 已有 cached managed/Relay Agent 先显示；
- profile 仍加载时使用已知名称或短公钥，不阻塞列表；
- 首次完全没有目录数据时显示紧凑 loading row；
- 不阻塞 Context 编辑和 Cancel。

### 8.2 部分失败

某一目录来源失败时：

- 继续显示其他可信来源；
- 显示“部分候选可能尚未同步”的非阻断说明；
- 保留 Retry；
- 手动公钥入口始终可用。

不得因为 Agent directory 失败而把整个 verified Project View 或 Role Inspector 标成
integrity failure。目录是辅助发现数据，不是 Project View canonical snapshot。

### 8.3 Revision conflict

候选搜索期间 Project revision 可能变化。提交继续使用现有
`expectedProjectRevision`，发生 conflict 时：

- 不自动在新 revision 上重放 offer；
- 刷新 verified snapshot；
- 保留选中 candidate 与 Context 草稿；
- 提示 Human 复核 Role/Assignment 最新状态后再次提交。

### 8.4 Community 切换

切换 Community 时必须关闭正在打开的 Assign 对话框或随 `/view` 子树卸载。旧
Community 的 selected candidate、搜索词和 profile 结果不能出现在新 Community。

## 9. 安全与隐私

1. Candidate picker 不授予权限；Relay 是唯一授权者。
2. 只展示当前 Human 已能通过现有 Community/Agent/Profile API 读取的数据。
3. 不把 private key、agent env、token、ACP config 或 runtime fence 放入 candidate model。
4. 完整公钥仅在选择详情或手动模式按需展示；默认列表使用短公钥消歧。
5. 名称、头像、Persona 和 Agent 状态均是展示数据，不能替代公钥参与 mutation。
6. 不把 online/running 状态当作 eligible，也不把 offline/stopped 当作 ineligible。
7. 当前 Relay 与 active Community 的边界必须在数据合并前确定，不能先合并全局列表再靠
   UI 分组隐藏。

## 10. 可访问性与键盘

- 搜索输入具有可读 label：`Candidate`；
- 结果列表使用 combobox/listbox 或等价的正确 ARIA 关系；
- `ArrowUp`/`ArrowDown` 移动 active option，`Enter` 选择，`Escape` 关闭；
- 每一项的 accessible name 包含 display name、Agent/Person 类型和短公钥；
- 状态不能只用颜色表达；
- 选中项可用键盘清除并重新选择；
- 200% zoom 与窄窗口下名称可以截断，但类型和短公钥仍可辨认。

## 11. 测试计划

### 11.1 纯函数测试

至少覆盖：

1. 不在 `continuity.members` 的同 Community managed Agent 仍出现在 Agents 分组；
2. 其他 relay 的 managed Agent 被排除；
3. `localhost`/`127.0.0.1` 只按共享 canonical helper 比较，不改写连接 URL；
4. managed、Relay Agent、profile 和 member 同公钥合并为一项；
5. 同名不同公钥保持两项并显示不同短公钥；
6. managed name/avatar 覆盖较弱来源，缺失时正确 fallback；
7. active Assignment 与 open Proposal 状态正确装饰；
8. offline/stopped Agent 仍可选择；
9. archive/无法确认 Community ownership 的身份不进入默认 eligible 列表；
10. 搜索可匹配名称、NIP-05 和公钥，排序稳定且与输入顺序无关。

### 11.2 组件测试

覆盖：

- 默认显示 Agent 名称而非完整公钥输入框；
- 搜索 `test-1` 后通过键盘选择；
- 结果行展示 Agent badge、owner/status 和短公钥；
- 选中后 `Create offer` 提交精确 candidate pubkey；
- 手动模式接受 hex/npub、拒绝非法值；
- partial/error 状态不丢失手动入口；
- 重名项可被辅助技术区分。

### 11.3 Desktop E2E

扩展 `../../../desktop/tests/e2e/project-view.spec.ts`：

1. seed 一个尚未在 Role membership 中出现的 managed Agent `test-1`；
2. owner 打开 vacant Role，点击 `Assign`；
3. 列表按名称展示 `test-1`；
4. 选择后创建 offer，断言 Role mutation payload 中是对应公钥；
5. mock candidate identity 接受后，Role 卡片显示 `Assigned`；
6. owner 身份看不到替 candidate 执行 `Accept` 的操作；
7. 切换 Community 后旧 Agent 不再出现；
8. Agent directory 失败时仍可手动输入公钥。

如需截图，必须按仓库约定在 mock Tauri bridge 下运行，并在截图前等待动画完成。

## 12. 验收标准

本项只有同时满足以下条件才算完成：

1. Human 可以在不复制公钥的情况下，把一个首次未分配 Role 的 Community managed Agent
   选为 candidate；
2. `test-1` 等 Agent 以名称、头像、类型和短公钥呈现；
3. 创建的 offer 仍精确绑定选中 Agent 公钥；
4. candidate 仍必须以自己的身份接受，owner 无代接受入口；
5. 跨 Community Agent 不出现在结果中；
6. 目录失败时公钥 fallback 可用；
7. Relay 对 eligibility、revision 和 Assignment fence 的行为完全不变；
8. targeted unit/component/E2E、Desktop typecheck、Biome 和相关 Tauri tests 通过。

## 13. 实施顺序

建议一次修改完成，不拆成长期双轨 UI：

1. 实现并测试纯 candidate 合并函数；
2. 增加 Community-scoped candidate Hook；
3. 实现可搜索单选组件与手动公钥 fallback；
4. 替换 Role Inspector 的 `Input + datalist`；
5. 补齐组件和 Project View E2E；
6. 运行 Desktop 定向检查与完整质量门禁；
7. 用真实本地 Community 验收 `test-1` 首次委派和接受流程。

## 14. 实施结果

- `projectRoleCandidates.ts` 实现多来源合并、canonical relay 过滤、按公钥去重、状态装饰、
  搜索分组及 hex/npub 规范化；真实 managed Agent 连接 URL 保持不变。
- `useProjectRoleCandidates.ts` 只在 offer 对话框打开时读取当前 Community 的 managed
  Agents、Relay Agent directory、profiles 与 archived identities，没有新增 module-level
  Community cache。
- `ProjectRoleCandidatePicker.tsx` 提供 Agents/People 分组、名称与公钥搜索、键盘导航、
  重名消歧、当前 assignee 禁选，以及显式展开的手动公钥 fallback。
- `ProjectRoleInspector.tsx` 继续复用原有 offer、replacement、revision 与 mutation 语义，
  仅把 Human 的 candidate 选择转换为规范化公钥后提交。
- 验证结果：candidate 纯函数测试 5/5、全部 Project View 单元测试 16/16、Desktop 单元
  测试 3518/3518、Project View E2E 31/31、typecheck、production E2E build、Biome、文件
  大小、文本缩放与公钥截断门禁全部通过。
