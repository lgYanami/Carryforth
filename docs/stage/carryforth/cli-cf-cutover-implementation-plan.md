# Carryforth `cf` CLI 去 Buzz 化实现计划

> 状态：代码交付完成；localhost 真实 Agent / Meeting 验收待执行
> 日期：2026-08-10
> 范围：Agent-first CLI 及其 Desktop / ACP / MCP 调用链
> 关联：[Desktop 本地化方案](../local/desktop-localization-plan.md)、
> [`cf` Managed Agent 执行宿主身份环境注入缺口修复设计](bug/cf-managed-agent-execution-host-auth-env-injection-fix-design.md)

## 1. 结论

本阶段从 Agent-first CLI 开始去 Buzz 化，并采用一次性切换：

| 层级 | 当前名称 | 目标名称 |
| --- | --- | --- |
| 产品 / 系统 | Buzz | Carryforth |
| 用户与 Agent 命令 | `buzz` | `cf` |
| Cargo package | `buzz-cli` | `carryforth-cli` |
| Rust library | `buzz_cli` | `carryforth_cli` |
| crate 目录 | `crates/buzz-cli` | `../../../crates/carryforth-cli` |
| Relay 地址变量 | `BUZZ_RELAY_URL` | `CARRYFORTH_RELAY_URL` |
| CLI 身份变量 | `BUZZ_PRIVATE_KEY` | `CARRYFORTH_PRIVATE_KEY` |
| CLI owner attestation | `BUZZ_AUTH_TAG` | `CARRYFORTH_AUTH_TAG` |

`cf` 是唯一正式命令。不得提供 `buzz` 或 `carryforth` 命令别名，也不读取旧
`BUZZ_*` CLI 环境变量作为 fallback。

这次交付必须是一条完整的垂直切片，而不是只修改 `../../../Cargo.toml` 中的二进制名称：

```text
Desktop sidecar / 本地安装
              │
              ▼
             cf
              ▲
              │
ACP system prompt ──► Agent ──► developer MCP private shim
              │
              └──── Carryforth CLI 环境与本地 Relay 坐标
```

完成后，无论 Human 在终端中使用，还是 Desktop 托管 Agent 通过 ACP/MCP 使用，看到和执行的
都只能是 `cf ...`。

## 2. 目标

1. 将 Agent-first CLI 的产品名、命令名、包名、帮助文本和文档统一为 Carryforth / `cf`。
2. 保证 Desktop 打包、开发启动、ACP 提示词、MCP multicall shim 和 Agent PATH 都能找到同一个
   `cf` 可执行文件。
3. 将 CLI 的公开身份环境变量统一为 `CARRYFORTH_*`，不保留旧变量兼容路径。
4. 保持 Channel、Agent、Project View、Document、Project Context 与 Meeting 的既有行为和数据不变。
5. 增加自动门禁，避免后续代码再次向 Human 或 Agent 输出 `buzz ...` 命令。

## 3. 本阶段的边界

### 3.1 纳入范围

- `crates/buzz-cli` 的目录、Cargo package、library 与 binary 重命名；
- Clap command name、help、错误、示例与生成的可执行命令；
- `buzz-dev-mcp` 中嵌入 CLI 的 multicall personality 与 private shim；
- ACP base prompt、Meeting / Project Space / Role Brief 等 Agent-facing 命令说明；
- Desktop Tauri sidecar、开发构建、`~/.local/bin` 便利链接和 managed Agent PATH；
- managed Agent 的 CLI 凭据注入与保留环境变量门禁；
- 直接调用 Agent-first CLI 的 Justfile、脚本、测试和验收 runbook；
- Cargo workspace、依赖引用、锁文件和发布产物清单。

### 3.2 明确不纳入范围

以下名称属于其他可执行组件、内部 crate、协议或持久化合同，本阶段不顺带重命名：

- `buzz-relay`、`buzz-acp`、`buzz-agent`、`buzz-dev-mcp`、`buzz-admin`；
- `buzz-sdk`、`buzz-core`、`buzz-project-*` 等 Rust 内部 crate；
- `buzz-project-view-v3`、`buzz-project-context-*`、`buzz-project-document-*` 等 capability；
- Nostr event kind、tag、`d` coordinate、数据库表、migration 与历史 event；
- `buzz://` deep link、Desktop bundle identifier、keyring service、数据目录和 Docker 资源名；
- Web、Mobile、Relay、ACP 等其他子系统的完整品牌替换。

这些内容会在后续 Carryforth 化阶段分别设计。当前保留它们不是为 Buzz 产品兼容，而是避免把
“改命令名称”错误扩大成 wire/storage migration。

原始 JSON 中如果包含上述协议 ID，必须保持原值；CLI 外层的人类可读说明则使用 Carryforth。

## 4. 不变量

### 4.1 行为不变量

- `cf` 的 subcommand、参数、JSON schema、exit code 和签名行为与当前 CLI 一致；
- 全局参数位置不变，例如 `cf --format compact project-view get`；
- Relay URL 的解析与本地 NIP-42/NIP-98 签名语义不变；
- Project View、Document、Context 和 Meeting 的 revision/CAS/receipt 语义不变；
- 只改入口名称，不借机修改权限、协议版本或业务状态机。

### 4.2 数据安全不变量

- 不新增数据库 migration；
- 不执行 reset、truncate、drop、volume 删除或 Desktop identity 清理；
- 不重写历史 Nostr event；
- 不修改本地 Community、消息、Agent、Project、Document、Context 或 Meeting 数据；
- 构建和验收不得运行会指向 localhost 主开发库的破坏性测试。

### 4.3 无兼容双轨

- 不构建 `buzz` 二进制；
- 不创建 `buzz -> cf` 或 `carryforth -> cf` alias；
- `cf` 不读取 `BUZZ_RELAY_URL`、`BUZZ_PRIVATE_KEY`、`BUZZ_AUTH_TAG`；
- prompt、帮助和当前文档不再建议执行 `buzz ...`；
- old CLI env keys 仍保留在 managed-Agent **拒绝名单**中，但仅作为已退役的敏感键，绝不作为输入。

## 5. 公开 CLI 合同

### 5.1 命令

```bash
cf --help
cf --format compact channels list
cf project-view get
cf documents list
cf project-context incident project:<uuid>
cf meetings list
```

Clap 的 program name、usage、about、long help、错误提示和 shell completion 都以 `cf` 为准，产品说明
使用 Carryforth，不出现 “Buzz CLI” 或 “Buzz relay”。

### 5.2 环境变量

CLI 只接受：

```text
CARRYFORTH_RELAY_URL
CARRYFORTH_PRIVATE_KEY
CARRYFORTH_AUTH_TAG
```

边界要求：

- `CARRYFORTH_PRIVATE_KEY` 仍是本地 Nostr identity，不表示取消签名认证；
- `CARRYFORTH_AUTH_TAG` 保持现有 NIP-OA 数据格式；
- command flags 仍高于环境变量；
- 缺失或错误变量的 stderr 必须直接指出新的变量名；
- 旧 `BUZZ_*` 变量即使存在，也不得影响 `cf`。

ACP、Desktop 等当前仍可能通过内部旧配置取得 Relay/identity。本阶段在**启动 CLI/MCP 的边界**将
权威运行态明确写入 `CARRYFORTH_*`，并从 Agent-visible CLI 环境中移除旧 CLI 变量；不得依靠环境继承
或双变量 fallback。其他子系统自身的配置变量留待其 Carryforth 化阶段处理。

## 6. 分阶段实现

每一阶段完成后先 review 设计不变量与实际 diff，再进入下一阶段。所有阶段完成前不得提交一个
“只有命令改名、Agent 实际不可用”的中间终态。

### 阶段一：Cargo 与可执行入口切换

1. 将 `crates/buzz-cli` 移动为 `../../../crates/carryforth-cli`；
2. package 改为 `carryforth-cli`，library 改为 `carryforth_cli`，binary 改为 `cf`；
3. 更新 root workspace、`../../../Cargo.lock`、`buzz-dev-mcp` 嵌入依赖和所有 `cargo -p` 门禁；
4. 修改 `main.rs`、doc example 与 Rust import；
5. 确认构建结果只有 `target/{debug,release}/cf`，不存在新生成的 `buzz`。

阶段验收：

```bash
cargo build -p carryforth-cli
target/debug/cf --help
test ! -e target/debug/buzz
```

### 阶段二：CLI 文案与公开配置切换

1. Clap program name 改为 `cf`，about/long_about 使用 Carryforth；
2. 三个公开环境变量切换为 `CARRYFORTH_*`；
3. 更新 auth、client、agent draft、memory 等错误文本；
4. 更新各 command 生成的后续操作、回读、修复和诊断命令；
5. 更新 CLI README、TESTING 与 doctest 示例；
6. 对 raw capability/error code 保持原始技术 ID，不做字符串替换。

阶段验收必须覆盖：

- flag 覆盖 env；
- 新 env 可用；
- 只设置旧 env 时按缺少凭据失败；
- help/stderr/actionable command 中没有 `buzz ...`；
- JSON 输出结构与改名前一致。

### 阶段三：ACP 与 MCP 的 Agent 调用链切换

1. `buzz-dev-mcp` 的 multicall personality 从 `buzz` 改为 `cf`；
2. private shim 只创建 `cf`，内部 `buzz_path()` 改成中性或 `cf_path()`；
3. read-only Meeting 等固定路径调用切换为 private `cf` path；
4. MCP bootstrap 与 ACP base prompt 统一说明 “The `cf` CLI is your primary interface”；
5. Project View、Document、Context、Role 与 Meeting prompt 中的命令全部切换为 `cf`；
6. ACP 按权威运行配置为 MCP 注入 `CARRYFORTH_*`；
7. Agent subprocess 不得依靠旧 CLI env，旧键继续被过滤/拒绝；
8. `buzz-agent` 的安全 passthrough 列表同步允许新键并拒绝用户覆盖。

这一阶段的关键回归不是字符串扫描，而是真实 Agent 能执行：

```text
Agent receives prompt -> runs `cf ...` -> private shim resolves the embedded CLI
-> request is signed -> localhost Relay accepts -> canonical readback succeeds
```

### 阶段四：Desktop sidecar 与本地安装切换

1. Tauri `externalBin` 从 `binaries/buzz` 改为 `binaries/cf`；
2. Justfile 与 sidecar bundling 复制/校验 `cf-<target-triple>`；
3. managed Agent PATH 注释、发现逻辑和测试切换为 `cf`；
4. 生产便利链接使用 `~/.local/bin/cf`；
5. 开发便利链接可使用 `~/.local/bin/cf-dev`，但 Agent private PATH 中仍提供标准命令 `cf`；
6. Desktop 启动时只清理**确认指向旧 Carryforth/Buzz app bundle CLI 的** `buzz` / `buzz-dev`
   symlink；不得删除普通文件、目录或指向第三方程序的 symlink；
7. 若 `~/.local/bin/cf` 已是普通文件（例如其他软件安装的同名 CLI），不得覆盖。Desktop 应报告
   human terminal shortcut 未安装；App bundle 与 Agent private shim 仍使用自身的精确 `cf`。

本阶段承认 `cf` 在通用开发环境中可能与 Cloud Foundry CLI 重名。解决方式是精确 sidecar path、
private shim 和不覆盖用户文件，而不是增加 `carryforth` alias。

### 阶段五：脚本、runbook 与验收入口切换

按用途区分替换，禁止全仓库机械替换 `buzz`：

- 构建 Agent-first CLI：`cargo ... -p carryforth-cli`；
- 调用 Agent-first CLI：`target/release/cf` 或 `cf`；
- CLI 调用所需 env：`CARRYFORTH_*`；
- 调用 Relay、ACP、Admin 或引用 capability 时保留其当前技术名称。

需要覆盖的主要入口：

- Justfile 的 build、sidecar、Project View/Document/Context/Meeting gate；
- Meeting 与 Project 三域 live acceptance；
- `buzz-test-client` 中启动 Agent-first CLI 的测试；
- Desktop live E2E 和 sidecar tests；
- `../../../scripts/build-sprig.sh` 中暴露给 Agent 的 CLI 名称；
- contributor 文档中当前有效的 CLI 使用说明。

历史设计/事故文档不做全量改写。若其中的命令仍会被当作当前操作指南，则在文档头部添加
“CLI 已切换为 `cf`”说明；纯历史证据保留原文。

### 阶段六：防回退门禁

新增一个有明确 allowlist 的静态门禁，检查当前用户/Agent-facing surface：

- `carryforth-cli` 不声明 `buzz` binary；
- Desktop 不打包 `binaries/buzz`；
- MCP shim 不创建 `buzz` personality；
- ACP 当前 prompt 不包含 shell command `buzz ...`；
- CLI help、README、TESTING 不包含旧 CLI env；
- 生成的 actionable command 不包含 `buzz ...`。

门禁不能对全仓库执行简单的 `rg buzz` 后要求零命中，因为协议 ID、内部 crate 和历史材料仍合法存在。
allowlist 必须按“技术标识为何保留”分类，而不是逐个随意忽略。

## 7. 关键代码修改面

| 修改面 | 主要位置 | 目标 |
| --- | --- | --- |
| CLI crate | `crates/buzz-cli` | 路径/package/lib/bin 全部切换 |
| CLI 文案 | `src/lib.rs`、`src/client.rs`、`src/commands/*` | `cf`、Carryforth、`CARRYFORTH_*` |
| workspace | `../../../Cargo.toml`、`../../../Cargo.lock`、Justfile | 新 package 与 binary |
| embedded CLI | `../../../crates/buzz-dev-mcp` | multicall personality 与 private path 改为 `cf` |
| Agent contract | `crates/buzz-acp/src/*prompt*`、Project/Role/Meeting context | 只教 Agent 使用 `cf` |
| Agent env | ACP MCP server env、Desktop managed-agent env、`buzz-agent` passthrough | 注入新键、拒绝旧键 |
| Desktop bundle | `../../../desktop/src-tauri/tauri.conf.json`、sidecar scripts、nest symlink | bundle/install `cf` |
| Tests/runbooks | CLI tests、Desktop/Tauri tests、Project/Meeting scripts | 使用新入口并防回退 |

## 8. 测试与验收

### 8.1 CLI 单元与契约测试

- `cf --help`、`cf --version`、usage/error program name；
- 三个 `CARRYFORTH_*` 环境变量和 flag precedence；
- 旧 env 不生效；
- exit code 0/1/2/3/4/5 不变；
- read/write JSON schema 与当前 golden fixture 一致；
- generated follow-up command 以 `cf` 开头；
- raw `buzz-project-*` capability/error code 不被改写。

### 8.2 MCP / ACP 集成测试

- multicall argv0=`cf` 能进入 embedded CLI；
- shim 目录存在 `cf` 且不存在 `buzz`；
- private fixed-path reads 使用正确可执行文件；
- Agent prompt 只声明 `cf`；
- 新 CLI env 注入正确，secret 不出现在日志和模型上下文；
- 旧键无法由 persona/global env 绕过 reserved-key 门禁；
- Codex/Claude ACP 至少各完成一次只读和一次有 revision fence 的写入。

### 8.3 Desktop 打包与安装测试

- dev/release external binary 名称正确；
- macOS/Linux/Windows sidecar triple 解析正确；
- Agent PATH 优先命中 app-owned `cf`；
- prod `cf` 与 dev `cf-dev` 不互相覆盖；
- 已存在第三方 `cf` 普通文件时不覆盖，Agent private shim 仍可用；
- owned legacy symlink 安全清除，任意用户文件保持不变。

### 8.4 本地全链路验收

在不清理现有数据的 localhost 环境验证：

1. Desktop 重启并恢复原身份、Community 与 managed Agents；
2. Human 使用 `cf` 读取 Channel、Project View、Document、Context 和 Meeting；
3. Agent 在 DM 中自行调用 `cf` 回复；
4. 新建一场正常 Meeting，主持 Agent 使用 `cf` 完成 Action Finalization；
5. 对 Project View、Document、Context 做写入和 canonical readback；
6. Meeting 正常 `completed_closed`；
7. 重启后新增数据仍存在；
8. 日志中没有 `command not found: buzz`、旧 env 缺失或 sidecar lookup 错误。

### 8.5 数据安全门禁

验收前后记录并比对：

- Community ID 与 Owner pubkey；
- Channel/message 基线数量；
- Project revision；
- Document catalog revision；
- Project Context revision；
- terminal/active Meeting 数量；
- Desktop identity pubkey 与 managed Agent IDs。

不得以清库或重建数据作为通过条件。

## 9. 实施顺序与提交边界

建议分为四个可 review 的提交，但在同一分支连续完成：

1. `refactor(cli): rename agent cli to cf`
   - crate/package/bin/help/env contract；
2. `refactor(agent): route managed agents through cf`
   - ACP prompt、MCP shim、env 注入与安全门禁；
3. `refactor(desktop): bundle and install cf sidecar`
   - Desktop、Justfile、sidecar、symlink；
4. `test(cli): migrate cf runbooks and prevent buzz command regressions`
   - scripts、tests、docs、静态门禁和验收记录。

阶段 review 重点：

- 是否误改 wire/storage identifier；
- 是否留下任一 Agent-facing `buzz ...`；
- 是否出现新旧命令或新旧 env 双轨；
- 是否破坏 Desktop sidecar、MCP multicall 或 secret 过滤；
- 是否触发任何数据迁移或清理。

## 10. 完成标准

- `cf` 是源码、构建产物、Desktop bundle、Human terminal 和 Agent prompt 中唯一的 Agent-first CLI 名称；
- `buzz` 与 `carryforth` 命令均不存在，也没有 alias/fallback；
- `carryforth-cli` package/library/path 已完成重命名；
- CLI 只接受 `CARRYFORTH_*` 公开变量；
- Desktop/ACP/MCP 能稳定向 Agent 提供 `cf` 和必要凭据；
- Channel、Agent、Project View、Document、Project Context、Meeting 回归通过；
- 现有本地数据、身份、权限和 revision 连续；
- 自动门禁能阻止旧命令重新进入当前提示词、文档和 bundle；
- 技术性 `buzz-*` capability 与持久化标识保持原值，并有清晰 allowlist 说明。

## 11. 后续 Carryforth 化

本阶段完成后，再分别规划：

1. Relay / ACP / Agent / MCP / Admin 二进制与 crate 命名；
2. Desktop app name、bundle identifier、deep link、数据目录与 keyring；
3. Docker、部署、metrics、日志 target 和环境变量；
4. 在有明确数据迁移策略后，评估 capability、tag 与历史 coordinate 是否需要改名；
5. README、CONTRIBUTING、AGENTS、release 与仓库路径的全局品牌收口。

后续阶段不得反向恢复 `buzz` CLI，也不得为了减少改动重新引入旧命令兼容层。

## 12. 实施记录（2026-08-10）

本方案已按一次性切换完成代码交付：

1. `crates/buzz-cli` 已迁移为 `../../../crates/carryforth-cli`，Cargo package、Rust library 与 binary
   分别为 `carryforth-cli`、`carryforth_cli` 与 `cf`；
2. CLI 公开身份、Relay 坐标与超时变量已统一为 `CARRYFORTH_*`，不存在旧 `BUZZ_*` CLI
   输入 fallback；
3. ACP 以单一 typed builder 将其内部权威 Relay、Agent identity 与已验证 owner attestation 映射为
   `CARRYFORTH_*`，并同时注入模型 Agent / Code Mode 执行宿主与 developer MCP；子进程会移除父
   环境和 persona 中的退役 `BUZZ_*` CLI 变量，Agent prompt、Meeting、Project Space、Role Brief
   与 developer MCP 只向 Agent 暴露 `cf`；
4. Desktop external binary、dev/release bundling、Windows placeholders、managed Agent skill、
   Sprig personality、CI cache/canary/release artifact 均已切换为 `cf`；
5. `~/.local/bin/cf` / `cf-dev` 仅在路径空闲或已确认归 Carryforth 所有时创建；Desktop
   reset 不删除该通用命令名，避免破坏第三方 Cloud Foundry CLI；
6. 旧 app-owned `buzz` CLI/skill 链接仅在目标与版本标记能证明所有权时清理，普通文件、
   用户目录和任意 symlink 均保留；
7. 新增 `../../../scripts/check-cf-cli-cutover.sh` 并接入 `just check`，覆盖 Cargo 路径、构建产物、
   Desktop sidecar、MCP personality、公开 CLI env 与 Agent-facing actionable command。

已通过的门禁：

- `cargo check --workspace --all-targets`；
- `cargo clippy --workspace --all-targets -- -D warnings`；
- Desktop Tauri `cargo clippy --all-targets -- -D warnings`；
- `cargo test -p carryforth-cli --lib`（304 项）；
- `cargo test -p buzz-acp --lib`（835 项）；
- `cargo test -p buzz-agent -p buzz-dev-mcp --lib`（267 + 89 项）；
- Desktop frontend tests（3643 项）、lint 与 typecheck；
- Desktop reset、managed Agent nest 与 env 门禁测试（13 + 43 + 43 项）；
- `cargo fmt`、Desktop Tauri fmt、所有修改 shell 的 `bash -n`、`git diff --check`；
- `../../../scripts/check-cf-cli-cutover.sh` 与 `../../../scripts/test-project-view-release-contract.sh`；
- `cargo build -p carryforth-cli`、`target/debug/cf --help`，以及“只设置旧 CLI env 仍按缺少
  `CARRYFORTH_PRIVATE_KEY` 失败”的无 fallback 验证。

尚未计为通过的外部/非本阶段门禁：

- Harbor benchmark Python 测试环境需要从 Block 内部制品源下载依赖，当前 TLS 连接失败，
  因而未执行到测试体；
- 全 workspace tests 中既有 Relay `mesh_demo` echo 用例稳定返回 504（期望 200）；本次 diff
  未修改该模块，CLI/ACP/Desktop 相关测试均通过；
- localhost 真实 Agent 调用、Meeting Action Finalization 与 Desktop 打包运行验收留给后续现场
  验收；执行时必须保留现有数据库和 Desktop 身份。此前发现的模型执行宿主缺少
  `CARRYFORTH_*` 问题已完成代码与自动化修复，但仍需在重建后的真实进程中验收。

本次实现未新增 migration，未执行 reset、truncate、drop、volume 删除或 Desktop 数据清理。
