# `cf` Action Finalization Context 归因误用与 Help 敏感值暴露修复设计

> 状态：代码与自动化完成，待全新 Meeting 现场验收
>
> 日期：2026-08-10
>
> 范围：Carryforth `cf` CLI、Meeting Action Finalization 提示词与 trusted Turn envelope、
> Project Context 写入参数、CLI help 安全输出
>
> 关联：
> [`cf` Managed Agent 执行宿主身份环境注入缺口修复设计](./cf-managed-agent-execution-host-auth-env-injection-fix-design.md)、
> [Meeting Action Finalization 提示词认知边界优化设计](../../meeting/fix/meeting-action-finalization-prompt-boundary-optimization-design.md)、
> [Meeting Action Finalization 中 Project Context 写回实现设计](../../project-context/meeting-action-finalization-context-write-implementation-design.md)

## 1. 结论

“授权范围缺失召回的失败关闭最小缺口评审”验收 Meeting 中，上一轮 `CARRYFORTH_*` 环境注入修复已经
生效：主持 Agent 使用 `cf` 成功创建并权威回读了 Issue 和 Project Document。Action Finalization
并没有因为认证、租约、Coordinator、工作槽或 ACP Session 失败。

真正阻塞发生在 Project Context attach。主持 Agent 把普通 Community Context 写入误写成显式
supervised Runtime attribution：

```text
cf project-context attach \
  --context-document <document-id> \
  --coordinate meeting:<meeting-id> \
  --coordinate issue:<issue-id> \
  --coordinate document:<document-id> \
  --acting-assignment <assignment-id>
```

CLI 按现有严格合同拒绝了不完整 attribution：

```text
exit 1 / user_error
--runtime-id and --runtime-epoch are required
```

Project Context 的普通写权限来源是 Community membership。它可以且默认应当同时省略
`--acting-assignment`、`--runtime-id` 和 `--runtime-epoch`。只有调用者明确声明 supervised Runtime
attribution 时，三者才必须同时出现并由 Relay 严格验证。

因此，本次不删除 `BLOCK(external_operation_failed)`，也不放宽 Relay 对显式 attribution 的校验。
修复位置是：

1. 在 Action Finalization trusted envelope 和系统提示中明确普通 Context 写入的正确权限与命令形态；
2. 允许 Agent 对“尚未提交任何 Event 的本地 attribution 参数错误”做一次安全纠正；
3. 改善 CLI 对不完整 attribution 的错误提示，但继续 fail closed；
4. 修复 `cf --help` 回显当前环境变量值的问题，保证只显示变量名或 redacted 状态。

本次不修改 Meeting 状态机、Action lease、Coordinator、Project Context 数据模型、数据库权限或既有数据。

## 2. 事故记录

### 2.1 Meeting 与 Action Run

```text
Meeting:       bde045b7-8183-4052-95a8-4d85c7c4812d
Action Run:    b33cfba7-2925-4d65-81a8-024cb487a1d6
Final Board:   6bd076a57bb684fe295ece1c6a740f281bc1875efdc7b6eebb22f65abe80961d
Epoch:         1
Progress:      14
Condition:     blocked
Reason:        external_operation_failed
Meeting state: active / finalizing_actions
```

Action Begin 后约 25.458 秒产生首次 progress，随后持续续租到 `progress_seq=14`。不存在
`action_lease_expired`、`provider_failure`、`affinity_lost` 或旁路 Action Begin。

### 2.2 已成功的业务写入

```text
Project View: 95 -> 96
Issue:        bb6e696c-add1-471f-8569-792ad121e0b2
Document:     e59d1b63-2406-46a5-b0b8-506c78338203
Catalog:      27 -> 28
```

Issue 和 Document 均由 Action Turn 使用 `cf` 成功写入并 canonical 回读。这直接证明：

- 模型 Agent / Code Mode 已收到有效 `CARRYFORTH_RELAY_URL` 与 `CARRYFORTH_PRIVATE_KEY`；
- `cf` sidecar、签名、Relay 连接与 Community 写权限正常；
- 上一份执行宿主环境注入修复已经覆盖真实 Action Turn。

### 2.3 失败的 Context 命令

Action Turn 的真实工具输出为：

```text
cf project-context attach ... --acting-assignment <assignment-id>

exit_code = 1
error = user_error
message = missing --runtime-id and --runtime-epoch
```

该错误发生在 Clap / CLI 本地参数校验阶段，尚未签名或提交 Project Context command。随后 exact 与三个
incident canonical 回读均返回空 Edge，Context revision 保持 25。Agent 因冻结 Board 明确要求该 Edge，
所以返回 `BLOCK(external_operation_failed)` 是正确的 fail-closed 行为。

完整脱敏记录：
`RESEARCH/AGENT_MEMORY_CARRYFORTH_E2E_ACCEPTANCE_2026_08_10.md`。

### 2.4 `cf --help` 的独立安全缺陷

现在 managed Agent 子进程按设计持有 `CARRYFORTH_*`。Clap 的 env-backed 参数如果不隐藏 env value，
`cf --help` 可能把当前环境中的 private key 或 auth tag 值渲染进工具输出。

即使这些值没有被写入消息、Document 或 Context，进入模型工具 transcript 本身也扩大了敏感信息暴露面。
CLI help 只应说明变量名、是否 required / optional 和参数用途，不得输出当前值。

该问题与 Context attach 失败没有因果关系，但由同一轮真实验收发现，且必须在下一次验收前一起收口。

## 3. 根因

### 3.1 Community authority 与 supervised attribution 被混淆

Project Context 当前有两种合法写入形态：

| 写入形态 | 权限来源 | CLI 参数 | Relay 行为 |
| --- | --- | --- | --- |
| 普通 Community 写入 | 当前 Community member / managed Agent owner membership | 三个 attribution 参数全部省略 | 按 Community ACL 验证 |
| 显式 supervised attribution | Community ACL + active Assignment + exact Runtime fence | `acting-assignment`、`runtime-id`、`runtime-epoch` 全部提供 | 严格验证完整 attribution |

Assignment 不是普通 Context 写权限的来源。显式 attribution 只是更强的来源声明；一旦声明就不能在 fence
缺失时静默降级，否则 receipt 会记录无法证明的 Role / Runtime provenance。

### 3.2 Action 提示词只说“attach”，没有说明使用哪条写入形态

当前 Action Finalization prompt 要求：

- 读取 Role / Assignment；
- 创建普通解释 Document；
- 通过 `cf project-context` attach Meeting 与物化坐标；
- 具体业务命令失败时 BLOCK。

trusted `project_context_policy` 只声明本 Turn 允许 Context 写入、需要 Context Document 和 canonical
readback，没有声明：

- 普通写入使用 Community authority；
- Runtime fence 对普通 attach 不需要；
- 默认命令必须省略三项 attribution 参数；
- 当前 Role Binding 不会自动把 Context Edge 变成 role-bearing Runtime write。

Agent 因此把“当前正在承接 admin Role”错误推导为“每个 Context write 都应传 acting Assignment”。

### 3.3 CLI 的错误虽然正确，但缺少安全纠正提示

`ProjectContextAttributionArgs` 目前在 Clap 层使用 `requires_all`。这能拒绝部分 attribution，却只告诉
调用者还缺两个参数，没有告诉调用者：普通 Community Context 写入的正确修复通常是省略全部三项，
而不是尝试寻找或伪造 Runtime fence。

Action prompt 又要求具体命令失败后 BLOCK，Agent 因而没有尝试安全的无 attribution 命令。

### 3.4 Help 没有隐藏 env value

CLI 的全局 Relay、private key 和 auth tag 参数使用 Clap `env`。当前参数没有启用
`hide_env_values`，导致 help 渲染可能包含进程环境中的当前值。此前模型子进程没有
`CARRYFORTH_PRIVATE_KEY`，所以真实 managed Agent 验收没有暴露这一问题；统一环境注入后该缺陷成为
确定的安全风险。

### 3.5 `external_operation_failed` 不是根因

删除该 reason code 只会产生两种错误结果：

1. Agent 换成另一个 BLOCK reason，Context Edge 仍不存在；
2. Harness 把未完成的部分物化错误当作 COMPLETE，会议在缺少冻结产物时关闭。

现行原则“具体业务写入或 canonical readback 失败才允许 BLOCK”在本次被正确执行，应继续保留。

## 4. 修复不变量

1. 普通 Project Context attach / detach 继续使用 Community authority；
2. 普通写入默认同时省略 Assignment 与 Runtime fence；
3. 显式 supervised attribution 继续要求 Assignment、Runtime ID 和 Runtime epoch 三者完整且有效；
4. 不把缺失 Runtime fence 自动解释为普通写入，也不伪造 attribution；
5. Role Binding 继续约束真正 role-bearing 的治理写入与 Role Checkpoint，不扩散到普通 Context Edge；
6. `external_operation_failed`、`external_state_conflict`、`tool_unavailable` 等现行 BLOCK 语义保留；
7. 只允许对无外部副作用、可证明发生在本地参数解析阶段的错误纠正一次；
8. Relay 错误、权限错误、CAS conflict、未知结果或响应不确定不得按本规则自动重试；
9. private key 与 auth tag 不得出现在 help、错误、prompt、日志、observer 或测试快照；
10. Agent 仍可通过环境使用 `cf`，不能以隐藏 help value 为由移除真实执行所需凭据；
11. 不修改 Project Context wire schema、receipt、数据库 ACL 或 migration；
12. 不恢复 `buzz` CLI、旧 env fallback 或远程能力；
13. 不自动恢复、Retry、Return、Abort 或关闭当前 blocked Meeting；
14. 不删除已经成功物化的 Issue、Document 或其他业务数据。

## 5. 目标命令语义

### 5.1 普通 Action Finalization Context 写入

主持 Agent 在普通 Action Finalization 中应使用：

```text
cf project-context attach \
  --context-document <document-id> \
  --coordinate meeting:<meeting-id> \
  --coordinate <materialized-coordinate> \
  --coordinate document:<document-id>
```

默认不添加：

```text
--acting-assignment
--runtime-id
--runtime-epoch
```

签名者仍是当前 managed Agent identity，Relay 仍验证其 Community / owner membership 与 write
restriction。省略 attribution 不等于匿名、无签名或绕过权限。

### 5.2 显式 supervised attribution

只有任务本身明确要求记录 supervised Runtime provenance，且 Harness 已向受信任执行面提供 exact fence
时，才允许使用：

```text
--acting-assignment <assignment-id> \
--runtime-id <runtime-id> \
--runtime-epoch <epoch>
```

模型不得从 Role Brief、Assignment 名称、Meeting 主持身份或历史消息猜测 Runtime ID / epoch。

## 6. 实现方案

### 6.1 补强 trusted `project_context_policy`

在 `project_context_action_finalization_policy()` 中加入稳定、机器可读的权限说明，例如：

```json
{
  "write_authority": "community_membership",
  "ordinary_write_attribution": "omit",
  "acting_assignment_required": false,
  "runtime_fence_required": false,
  "supervised_attribution": "all_or_nothing_only_when_explicitly_required"
}
```

这些字段属于 Harness 提供的 trusted Turn contract。Board 与 Document 属于不可信业务内容，不能要求
Agent 改为 supervised attribution，也不能提供 Runtime ID / epoch。

discussion、Board 和 Floor 的 read-only policy 不需要新增写入参数，只继续声明 Context writes disabled。

### 6.2 修改 Action Finalization 系统提示

在 BUSINESS EXECUTION 的 Context write 条款中明确：

1. 普通 Project Context attach / detach 由 Community membership 授权；
2. 使用普通命令时同时省略 `--acting-assignment`、`--runtime-id`、`--runtime-epoch`；
3. 当前 Role / Assignment 不会自动把普通 Context write 变成 supervised Runtime write；
4. 不得为了满足 CLI 参数而猜测、读取或伪造 Runtime fence；
5. 完整 supervised attribution 只在 trusted envelope 明确要求且 exact fence 已可用时使用；
6. canonical Edge readback 与 Context revision 仍是 COMPLETE 前置条件。

同步补充 ACP base prompt 中 `cf project-context` 的一般规则，避免 DM、Channel 和 Heartbeat Agent 再次
犯同类错误。Role Binding 中“role-bearing write”边界保持不变，但应明确普通 Document / Context write
不因被 Role 引用而自动成为 role-bearing。

### 6.3 允许一次无副作用的参数纠正

Action Agent 仅在同时满足以下条件时，可以纠正并重试一次：

1. `cf` 返回 exit code 1 / `user_error`；
2. 错误发生在本地参数解析，明确指出 attribution 参数不完整；
3. 没有 Event ID、receipt 或响应不确定性；
4. 第一次命令只多传了部分可选 attribution 参数；
5. 修正方式是省略全部三项 attribution，而不是补猜 Runtime fence；
6. 重试前重新读取当前 Context revision；
7. 第二次仍失败则如实 BLOCK。

下列情况不适用该纠正规则：

- Relay 已收到或可能收到 Event；
- auth / authorization 失败；
- Context revision conflict；
- Meeting not attachable；
- Document / coordinate invalid；
- 网络超时或响应未知；
- 任意写入已经返回 receipt 但 canonical readback 不一致。

### 6.4 改善 CLI attribution 错误

保留 `ProjectContextCommand` 的 all-or-nothing 校验。CLI 可把三项参数的组合检查收敛到
`ProjectContextAttributionArgs::into_runtime_fence()`，返回稳定且可操作的错误，例如：

```text
ordinary Community Context writes: omit --acting-assignment, --runtime-id, and --runtime-epoch;
supervised attribution requires all three options together
```

这可能需要移除 Clap 字段上的 `requires_all`，但不是放宽参数合同：CLI 在构造或签名 Event 前仍拒绝
任意部分组合。这样 Agent 和 Human 都能知道应选择哪条合法写入路径。

`--help` 中也应把三个参数归入“可选 supervised attribution”，避免让 managed Agent 误以为 Role
Assignment 是普通 Context 写入的必选项。

### 6.5 隐藏 help 中的环境变量值

对 `CARRYFORTH_RELAY_URL`、`CARRYFORTH_PRIVATE_KEY` 和 `CARRYFORTH_AUTH_TAG` 对应的 Clap 参数设置
`hide_env_values = true`。其中 private key 和 auth tag 是强制安全门禁；Relay URL 一并隐藏，以保持
统一且避免 help 暴露部署坐标。

help 可继续显示：

- 环境变量名称；
- required / optional；
- 参数用途；
- 默认 localhost Relay 文案。

help 不得显示：

- 当前 private key / nsec / hex；
- 当前 auth tag JSON；
- 从环境读取到的 Relay URL；
- “部分掩码但可关联”的 secret 片段。

错误输出、`Debug`、snapshot 和 observer 同样不得包含这些值。

### 6.6 保留 fail-closed 完成门槛

Action Finalization 仍按以下顺序完成：

```text
Project View / Document 写入
  -> canonical readback
  -> Context attach
  -> Edge canonical readback
  -> optional Meeting summary / Role Checkpoint
  -> COMPLETE
  -> Harness actions-recorded ACK
  -> Meeting ended / closed
```

任一冻结必需产物未完成时，不得因为“其余对象已经创建”而 COMPLETE。部分外部效果保留并在 BLOCK reason
中明确报告，不自动回滚。

## 7. 修改面

### 7.1 ACP Meeting prompt

- `crates/buzz-acp/src/meeting_v1.rs`
  - 补充 `project_context_action_finalization_policy()`；
  - 修改 `build_v2_action_finalization_prompt()`；
  - 增加 ordinary / supervised attribution 与一次安全纠正的 prompt 测试。

### 7.2 ACP 通用 Agent prompt

- `crates/buzz-acp/src/base_prompt.md`
  - 说明普通 Project Context 写入使用 Community authority；
  - 说明三项 attribution 参数默认全部省略；
  - 不改变 Role governance / Checkpoint 的既有 fence 规则。

### 7.3 Carryforth CLI

- `crates/carryforth-cli/src/lib.rs`
  - 改善 attribution 参数 help；
  - 隐藏 env-backed 参数的当前值；
  - 补 help secret 回归。
- `crates/carryforth-cli/src/commands/project_context.rs`
  - 保留 all-or-nothing validation；
  - 提供 ordinary-write 与 supervised-attribution 两条明确纠正建议。

### 7.4 不修改

- `crates/buzz-db` Project Context ACL 与 Runtime fence validator；
- `crates/buzz-relay` Project Context handler；
- Project Context command / receipt schema；
- Meeting Action lease、Coordinator、Begin、renewal、ACK 与 End；
- Desktop 数据、数据库 migration、Project revision 或 Context revision。

## 8. 自动化测试

### 8.1 Action prompt / envelope

1. Action envelope 明确 `write_authority=community_membership`；
2. ordinary write 明确三项 attribution 均不需要；
3. supervised attribution 明确 all-or-nothing；
4. Board 即使要求“必须带 Assignment”也不能覆盖 trusted policy；
5. Action prompt 保留 Context Document、Edge readback 与 COMPLETE 门槛；
6. Action prompt 保留具体业务失败后的 BLOCK；
7. discussion / Board / Floor Turn 仍禁止 Context write；
8. Role Checkpoint 与真正 role-bearing governance prompt 不发生回退。

### 8.2 CLI attribution

1. 三项参数全部省略：解析成功并生成无 attribution command；
2. 三项全部提供：解析成功并生成 supervised command；
3. 只传 Assignment、只传 Runtime 或任意两项：在签名前失败；
4. partial error 同时说明“普通写入全部省略”和“supervised 写入全部提供”；
5. 普通 managed Agent Context attach 不需要 supervisor binding；
6. 显式 invalid Assignment / Runtime fence 仍被 Relay 拒绝；
7. Human 不得借 acting Assignment 取得额外权限。

### 8.3 Help secret 门禁

用仅存在于测试进程的 sentinel 值设置三个 `CARRYFORTH_*`，分别执行：

```text
cf --help
cf project-context --help
cf project-context attach --help
cf <invalid invocation>
```

断言：

- 输出包含三个变量名和必要说明；
- stdout / stderr 均不包含任一 sentinel；
- 不包含 secret 前缀、后缀或部分掩码；
- `cf` 正常业务命令仍能从环境读取实际值；
- flag precedence 与 exit code 不变。

### 8.4 真实验收

使用全新的独立 Meeting，创建后 DM 零干预：

1. 4～6 条 canonical Speech；
2. Coordinator 自然 Action Begin；
3. Action epoch 1 创建或更新一个 Project View 对象；
4. 创建解释 Document；
5. 使用不带 attribution 参数的 `cf project-context attach` 建立含当前 Meeting 的 Edge；
6. canonical 回读三域与 Meeting hydration；
7. 必要时追加合法 Role Checkpoint，但不把其 Runtime 语义扩散到 Context attach；
8. Agent 返回 COMPLETE，Meeting 正常 `ended / closed`；
9. observer 中无 secret、无旧 `buzz` CLI、无 Runtime fence 猜测、无 DM 控制写入。

验收前后记录 Project View、Document catalog 与 Context revision。不得 reset、truncate、drop、删除 volume
或重建 localhost 主开发数据。

## 9. 实施顺序

1. 修改 CLI env help redaction，并先建立 sentinel 安全测试；
2. 改善 Project Context partial-attribution 错误，但保持签名前 all-or-nothing；
3. 补 trusted `project_context_policy` 字段；
4. 修改 Action Finalization 与 base prompt；
5. 补 prompt、CLI、Relay ACL 非回退测试；
6. 运行 Rust fmt、Clippy、ACP / CLI unit tests 与 `cf` cutover 静态门禁；
7. 清理增量缓存并同时重建 `cf`、ACP 与 Desktop；
8. 先做 help sentinel smoke，再做普通 Context attach smoke；
9. 最后用全新零干预 Meeting 做三域物化与正常关闭验收。

本次不需要 migration。已有 blocked Meeting 不自动恢复，也不作为新实现的正向验收样本。

## 10. 完成标准

1. 普通 Action Finalization Context attach 默认不携带 Assignment / Runtime fence；
2. Agent 不再因当前 Role 存在而错误选择 supervised attribution；
3. partial attribution 在签名前失败，并给出两条合法路径的明确提示；
4. 完整显式 attribution 仍由 Relay 严格校验；
5. `external_operation_failed` 继续保护部分物化的 fail-closed 边界；
6. `cf --help`、子命令 help 与错误输出不包含任何当前 env value；
7. `cf` 真实认证、签名和 flag precedence 不受 help redaction 影响；
8. 全新 Meeting 在 epoch 1 完成 Project View、Document、Context Edge、回读、ACK 与正常关闭；
9. 无 secret 泄露、无数据清理、无权限放宽、无 Meeting 状态机回退。

## 11. 非目标

- 删除 `external_operation_failed` 或取消 BLOCK；
- 把部分物化视为成功并强制关闭 Meeting；
- 取消 Project Context 的 Community ACL；
- 允许 Assignment 在没有 Runtime fence 时被写入 receipt；
- 自动部署或伪造 Runtime supervisor；
- 将所有 Project View / Role governance 写入改成普通 Community write；
- 修改 Action lease、epoch、Coordinator、Candidate-Cohort 或工作槽机制；
- 自动 Retry、Return、Abort 或补写当前事故 Meeting；
- 删除本次已经创建的 Issue 或 Document；
- 改写历史 Event、revision 或 Context Edge。

## 12. 实施记录

2026-08-11 已完成代码与自动化交付：

1. `cf` 的 Relay、private key 与 auth tag 参数均启用 `hide_env_values`；实际重建后的
   `target/debug/cf --help` 已用三个独立 sentinel 验证只显示环境变量名称、不显示当前值；
2. Project Context 三项 supervised attribution 参数不再由 Clap 输出缺少参数的泛化错误，而是在任何
   网络读取和 Event 签名前由语义层执行 all-or-nothing 校验；
3. 普通 Community 写入、完整 supervised 写入及六种 partial tuple 均有单元测试；partial error 同时说明
   “普通写入全部省略”和“supervised 写入全部提供”两条合法路径；
4. Action Finalization trusted `project_context_policy` 已声明 Community authority、普通写入省略归因和
   supervised all-or-nothing；Action 系统提示与 Board 后置 guard 均禁止不可信 Board 强制 Runtime attribution；
5. 通用 ACP base prompt 已同步普通 Context 写入边界与仅限本地、无副作用 partial-attribution 错误的一次
   安全纠正规则；Relay/auth/conflict/network/unknown-delivery 仍禁止按该规则重试；
6. `external_operation_failed`、canonical Edge readback、COMPLETE/ACK 门槛、Relay/DB ACL、Meeting lease
   和状态机均未修改。

已通过：

```text
cargo test -p carryforth-cli
  307 passed

cargo test -p buzz-acp
  835 passed；pool lifecycle 9 passed

cargo clippy -p carryforth-cli -p buzz-acp --all-targets -- -D warnings
./scripts/check-cf-cli-cutover.sh
cargo fmt --all
git diff --check
实际 target/debug/cf help sentinel smoke
```

未执行数据库迁移、数据清理、Meeting 恢复或 localhost 主数据写入。本文仍需用全新、创建后 DM 零干预的
Meeting 完成 Project View、Document、无 attribution 的 Context Edge、canonical readback、COMPLETE 与
`ended / closed` 现场验收；在此之前不标记为“全部验收完成”。
