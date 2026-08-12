# Carryforth 开源发布面收敛计划

> 状态：实施中
> 日期：2026-08-11
> 范围：Carryforth 独立开源仓库、首个可用本地发行物及其公开构建与合规边界
> 关联：[Desktop 本地化方案](../local/desktop-localization-plan.md)、
> [`cf` CLI 去 Buzz 化实现计划](cli-cf-cutover-implementation-plan.md)、
> [Desktop 产品层去 Buzz 化实现计划](desktop-product-surface-de-buzz-implementation-plan.md)、
> [Mobile 客户端源码退役决策](mobile-client-source-retirement-decision.md)、
> [破坏性 migration 测试误删主数据库事故](../bug/destructive-migration-test-main-database-data-loss.md)

## 1. 结论

Carryforth 已经完成 Desktop local-only、`cf` CLI 和 Desktop 产品面的第一轮独立化，但当前仓库还不能直接被描述为
“外部用户下载后即可独立使用的 Carryforth 开源发行版”。

本阶段的目标不是一次性把所有 `buzz-*` 技术标识重命名，而是先建立一个**真实、独立、无内部依赖、可在全新机器
复现并启动**的公开发布面：

```text
Carryforth 源码仓库
        │
        ├── Carryforth Desktop
        ├── 本地 Relay 发行物
        ├── ACP / managed Agent sidecars
        ├── cf CLI
        └── 本地部署、升级、数据迁移与公开验证材料
```

首发必须满足：

1. 不依赖 Block/Buzz 帐号、Builderlab、内部制品库、内部 CI 或私有签名服务；
2. 默认不连接 Buzz 远程服务，不存在失败后回退到远程 Relay 的路径；
3. 全新机器按公开说明可以初始化本地身份、本地 Relay、Owner Community 并启动 Desktop；
4. 现有本地用户升级时不丢失身份、Community、消息、Agent、Project View、Document、Context 或 Meeting；
5. 公开仓库能够在无 Block 权限、无内部网络、无预置 secret 的公共 CI 中构建和验证；
6. 对上游 Apache-2.0 代码保留真实归属，同时不继续使用 Buzz/Block 商标作为当前产品身份，也不暗示 Block 为
   Carryforth 背书。

## 2. 首个公开发行范围

### 2.1 受支持产品面

首个 Carryforth 开源发行只承诺以下垂直切片：

| 组件 | 首发承诺 |
| --- | --- |
| Carryforth Desktop | local-only Human 客户端 |
| Local Relay | localhost Community、消息、Agent 与治理协议 |
| ACP / managed Agent | Desktop 托管 Agent 与 `cf` 调用链 |
| `cf` | Agent-first CLI 与本地 Relay 读写 |
| 本地依赖栈 | 版本固定、可重复启动、数据默认持久化 |
| Project 系统 | Project View、Document、Project Context、Meeting |

源码仓库可以继续包含 Web、benchmark 和历史兼容代码，但它们不自动成为首发的“受支持产品”。继承的 Mobile
客户端已依据 [Mobile 客户端源码退役决策](mobile-client-source-retirement-decision.md) 从活动树完整退役。继承的
Helm/Kubernetes 与 hosted Push Gateway executable 由后续的源码/本地开发面收口计划从活动树退役，不再以可执行
清单保存历史。README、Release Notes 和下载页面不得把未完成 clean-room 验收的组件描述为正式发行物。

当前 Relay 构建会内嵌 Web/Admin 资源，因此只把目录标记为 source-only 还不够。首发 Relay image 必须使用不打包、
不提供 Web/Admin 产品页面的构建/路由配置，只保留 Relay 必要的协议与健康检查 HTTP surface。若实现上无法拆分，
则所有仍可访问的 Web/Admin 页面必须先完成 Carryforth 品牌、链接、默认外连和资产合规验收，才能随 image 发布；
不能把已经随 image 交付的页面称为“未发行源码”。

### 2.2 最小平台基线

第一份 Release Candidate 至少覆盖：

- Linux x86_64 Carryforth Desktop；
- Linux amd64/arm64 Local Relay OCI image；
- Linux x86_64 `cf`；
- 一套从空数据目录启动的本地部署入口。

macOS、Windows 或其他商店发行只有在各自的签名、bundle identity、数据迁移和 clean-machine 门禁完成后才加入
正式矩阵。Android/iOS Mobile 源码已经退役；未来若恢复，必须先按独立产品阶段重新引入源码、构建与验收矩阵。
仍有源码但未进入矩阵的平台可以保留构建说明，但不得发布一个未经验证的“正式”安装包。

### 2.3 明确非目标

本阶段不做：

- 不恢复远程 Community、Builderlab 登录或 Buzz 托管服务；
- 不提供本地失败后自动回退到公共 Relay 的路径；
- 不一次性重命名所有 `buzz-*` Rust crate、数据库表、Nostr kind、capability 或历史 event；
- 不改写旧 migration、历史事故文档或已有 canonical 数据；
- 不把 Web 和 benchmark 强行纳入首发支持承诺；
- 不在本计划中恢复已经退役的 Android/iOS Mobile 客户端；
- 不承诺多个 Harness 使用同一个 Agent 私钥时的 active-active 协调；
- 不把 Agent 模型供应商的帐号/API key 与 Carryforth 本地身份混为一体。

## 3. 当前发布阻断

### 3.1 仓库与项目身份仍属于旧产品面

当前根 README、AGENTS、贡献/发布说明、安全策略、治理文件、CODEOWNERS、issue 模板和部分 Cargo metadata
仍指向 Buzz、Block、内部配套仓库或旧 `block/sprout` 坐标。Desktop 已显示 Carryforth，但 Tauri bundle identifier 仍是
`xyz.block.buzz.app`，正式 release workflow 仍硬编码 `Buzz.app`、`block/buzz` 和 Block 私有签名动作。

如果直接发布，会同时产生三类问题：

- 外部贡献者无法判断 Carryforth 的维护者、治理入口和安全联系方式；
- 发布物仍携带 Buzz/Block 的商标与反向域名身份；
- fork 或新仓库无法在没有 Block secrets 的情况下运行 release workflow。

### 3.2 公开构建仍含内部依赖

两份 benchmark Python lockfile 大量固定到 Block 内部 Artifactory。公共网络环境无法按锁文件复现安装，同时会
暴露无意义的内部服务坐标。

初始审计时，release、Docker、Mobile 与签名流程也包含 canonical repo 判断、Block namespace、私有 action、私有
secret 和手工 Buildkite 步骤。这些可以作为上游历史留档，但不能继续作为 Carryforth 的权威公开发布链；其中
Mobile 源码和活动接线现已按独立退役决策删除。

另外，当前 release workflow 仍调用 `../../../desktop/scripts/build-release-config.mjs` 重新生成 release-only updater 配置，
并指向 `block/buzz` 的 `buzz-desktop-latest`。这会让已从日常 Desktop 产品面移除的旧 updater 在正式构建时重新出现；
必须删除这条 release-time 注入路径，而不只是检查当前开发配置。

### 3.3 旧 Push 外连必须保持退役

初始审计发现 Relay Push 与独立 Push Gateway 固定指向 `push.buzz.xyz`。即使不传消息正文，设备 endpoint、
installation grant、Relay pubkey 等元数据也不应在 Carryforth local-only 配置中发往旧供应方。

当前 Local Relay 已不创建 Push 外连。源码与本地开发面的后续收口还必须退役 Push Gateway executable、Docker
入口和部署 runbook，并且不暴露通过 environment、build flag 或 Desktop 设置重新启用旧 hosted Push 的路径。
未来若另立产品阶段重新设计 self-hosted Push，必须经过独立方案与隐私审查。

### 3.4 Desktop 安装包不能独立形成可用产品

Desktop 已固定连接 `ws://localhost:3000`，但当前 bundle 不包含或启动 Relay。用户仅下载 Desktop 安装包时，
实际上得不到可用后端；README 中“下载即可尝试”的表述因此不成立。

首发必须把产品单位定义为“本地 Carryforth 栈”，而不是单独的 GUI 文件。公开入口需要完成依赖检查、secret 生成、
持久化服务启动、Relay bootstrap、Owner 认领和 Desktop 启动，且重复执行保持幂等。

### 3.5 资产与供应链归属不完整

当前字体、声音、Provider logo 和部分安装资产缺少统一的 provenance/redistribution 清单。Apache-2.0 根许可证不能
自动证明第三方图形、声音、字体或商标也可被重新分发。

在首发前必须逐项确认来源与许可；无法确认的资产应替换为 Carryforth 自有资产、通用图标或纯文字，不以“已经在
上游仓库中”为再分发依据。

### 3.6 Relay image 仍会带入未收敛 Web/Admin 产品面

Relay 的公开 OCI artifact 不只是一个 WebSocket binary；当前构建还会编译并服务 Web/Admin。相关页面仍可能包含
Buzz 品牌、旧 deep link、下载入口或远程产品假设。首发既然只承诺 Desktop，就必须在 artifact 层排除这些页面，
不能仅在 README 中把 `../../../web` 标为 source-only。

### 3.7 当前源码还不是 Release Candidate 来源

Release 必须来自 clean、受保护的 tag，而不是任意开发工作树。当前开发树中的 migration 0057 尚未进入跟踪的权威
提交，说明数据库已知 schema 与仓库文件链仍需先收敛；任何 RC 都必须包含从空库可重放的完整 migration 集，并与
选定上游/当前开发基线完成同步 review。

存在未跟踪文件、未提交生成物、缺失 migration、来源不明的二进制或仅存在于开发者 home 的运行依赖时，release job
必须直接拒绝构建，不能把本机“恰好能运行”当成可公开复现。

## 4. 不变量与法律边界

### 4.1 保留上游归属，不继承旧商标身份

- 根许可证继续保留 Apache License 2.0 与原始版权声明；
- 新增 `NOTICE` / `../../../UPSTREAM.md`，说明 Carryforth 基于 `block/buzz` 演进并列出适用的上游归属；
- Carryforth 新增代码的版权和维护信息使用当前 Carryforth 维护主体；
- README、网站、安装包和 Release Notes 不暗示 Block、Buzz 或 Square 为 Carryforth 背书；
- Buzz/Block 名称仅可出现在许可证、上游归属、历史文档和必要的技术兼容说明中；
- 未明确获得授权前，不把旧 Logo、应用名、bundle identifier 或发布 namespace 当成可继承商标资产。

### 4.2 数据连续性

开源化和品牌迁移不得成为清空本地数据的理由：

- 不执行 reset、truncate、drop、volume 删除或 Desktop sign-out；
- 不重建 localhost Community 来制造 clean state；
- 不改写历史 Nostr event、Project revision 或 Meeting record；
- 数据 migration 必须前向、幂等、可回读，并对已有真实数据做基线比对；
- migration 测试只能运行在显式创建的 scratch database，必须 fail closed 拒绝 localhost 主开发库；
- 构建缓存清理不得触碰 Docker volume、app-data、keyring、Agent state 或数据库目录。

### 4.3 默认不出网

“local-only”在本计划中表示：

- Desktop 与 Relay 不自动连接 Builderlab、Buzz Push、远程 Community、旧 updater 或旧 release endpoint；
- localhost 不可用时只报告本地错误，不尝试远程 fallback；
- 不通过隐藏 build flag、environment variable 或 deep link 重新启用旧远程产品；
- 用户明确配置的模型供应商、打开的外部链接或远程媒体属于显式用户行为，应在 UI 中独立说明，不得伪装成
  Carryforth 控制面流量。

本地 Nostr 私钥、NIP-42/NIP-98 签名和 Community membership 仍然保留。免除的是 Buzz 远程帐号认证，不是取消
本地身份与授权。

### 4.4 技术坐标按兼容价值处理

内部 crate、binary、环境变量和 wire/storage 名称不在本阶段做机械全局替换。允许暂时保留的 `buzz-*` 必须归入
明确类别：

| 类别 | 处理 |
| --- | --- |
| 当前用户可见产品名、链接、artifact | 改为 Carryforth |
| 当前公开 CLI | 只使用 `cf` / `CARRYFORTH_*` |
| 发布镜像、安装包、Release title | 使用 Carryforth namespace |
| Rust 内部 crate/binary | 可暂留，后续按调用图迁移 |
| Nostr kind、capability、数据库表 | 保持，除非有独立协议迁移设计 |
| 历史文档与上游归属 | 保留事实 |
| bundle/keyring/app-data 旧坐标 | 仅在完成无损迁移后退出 |

## 5. 目标公开发布面

### 5.1 源码仓库

公开仓库的首页应让陌生贡献者可以直接确认：

- 产品是什么、首发支持哪些功能和平台；
- 最小依赖与全新机器启动方式；
- 架构、数据边界、local-only 网络边界；
- Apache-2.0、上游归属与第三方 NOTICE；
- Carryforth 的贡献、治理、安全报告和行为准则入口；
- 已发布的 Desktop、Relay image、`cf` 与校验文件；
- 哪些目录是实验性、未发行或仅保留的上游代码。

仓库 metadata、package repository URL、issue template、CODEOWNERS、SECURITY 与 Release 链接全部使用 Carryforth
权威坐标。开发者本地是否保留名为 `upstream` 的 Git remote 不属于发布物，也不应出现在用户运行时。

### 5.2 发行物

首发至少发布：

1. `carryforth-desktop-<version>-<platform>`；
2. `carryforth-relay:<semver>` 多架构 OCI image；
3. `cf-<version>-<target>`；
4. 对应 SHA-256 checksums；
5. SBOM 与构建 provenance；
6. 版本固定的本地部署 manifest / bootstrap 工具；
7. Release Notes、升级说明和数据迁移说明。

artifact 可以包装仍未重命名的内部 binary，但 artifact 名、用户文档和容器 namespace 必须是 Carryforth。内部
binary 的后续重命名不能改变当前 wire/storage 合同。

### 5.3 本地部署入口

首发支持的部署入口必须做到：

- 检查 Docker/系统依赖并给出明确错误；
- 从公开 registry 拉取固定 semver 或 digest，不使用 `main` 漂移标签；
- 生成强随机本地 secrets，不要求用户手工替换多项 `CHANGE_ME`；
- 启动只需要的持久化服务和 Relay；非必要的 Keycloak、Prometheus、Push 不阻塞 Desktop；
- 幂等执行 migration 和 localhost Project capability bootstrap；
- 在首次身份建立后完成本地 Owner 认领；
- 启动、重建、停止脚本都不删除数据；停止只关闭进程/容器；
- 提供 `status`、日志位置、版本和健康检查；
- localhost 不可用时不自动切换远程地址。

实现时需在以下两种交付模型中固定一种，不能让 README 同时声称两者已经完成：

1. Desktop 安装器同时安装并管理本地 Relay 栈；或
2. 提供一个先运行的 Carryforth local bootstrap，Desktop 安装包明确声明依赖它。

首个 Release Candidate 可以采用第二种模型，但必须是一条公开、幂等、可验证的命令，而不是要求用户 checkout
仓库后手工拼接 `just`、SQL 和环境变量。

## 6. 分阶段实施

每阶段完成后 review 本文、不变量和实际 diff。任何涉及删除本地数据、改写历史 migration 或恢复远程 Buzz
服务的变更必须停止并单独设计。

### 阶段零：冻结首发合同与 clean-room 基线

1. 由 Human 维护者固定 canonical GitHub/GHCR/Release 坐标、版本号、支持组件与平台；
2. 由 Human 维护者确定 reverse-DNS identifier、安全报告/行为准则入口、维护者/CODEOWNERS、签名策略、签名密钥
   责任人与丢失/轮换流程；这些值不能由实现者猜测；
3. 冻结一个不可变的升级基线 commit/tag、数据库 schema/migration 号与 Desktop profile 坐标；
4. 初版至少支持 fresh install 和“当前 Carryforth 本地开发数据基线”升级。若要宣称可从公开 Buzz 版本升级，必须再
   明确选择一个 Buzz release/tag 并单独验收，不能泛称兼容所有 Buzz 版本；
5. 建立一台没有 Block VPN、凭据、内部 DNS 和旧 app-data 的 clean-room runner；
6. 记录当前公开构建失败点、默认外连和内部域名基线；
7. 为选定升级基线建立只读证据：pubkey、Community、消息、Agent、数据库 schema、三域 revision 与 Meeting 数量；
8. 把 Web/benchmark 标记为 supported、experimental 或 source-only，按独立决策退役 Mobile，退役 Push/Helm
   executable，并验证 Relay artifact 没有偷带 source-only 产品页面；
9. 与选定上游基线完成一次同步与许可证 review，提交完整 migration 链，确保 RC source 没有未跟踪文件。

阶段门禁：发布范围不再依赖团队成员的隐式理解，clean-room 构建失败能稳定复现。

### 阶段一：项目身份、归属与仓库治理

1. 更新 README、AGENTS、CONTRIBUTING、ARCHITECTURE、RELEASING、SECURITY、GOVERNANCE、CODEOWNERS 和
   issue/PR template；
2. 将 Cargo、Tauri、JS、Python 与容器 metadata 的 repository/homepage/description 指向 Carryforth；
3. 新增上游归属与第三方 NOTICE；
4. 选择并公布 Carryforth 的安全联系入口和维护者边界；
5. 建立产品商标 allowlist，旧 Buzz/Block 名只允许出现在归属、历史和技术兼容位置；
6. 确定 Carryforth 自有的 bundle/application reverse-DNS identifier。

阶段门禁：外部贡献者不需要访问 Block 组织资源即可理解、构建、报告问题和参与贡献。

### 阶段二：默认外连与内部依赖清零

1. 在 Carryforth 首发构建中直接关闭 Relay Push，不暴露启用开关，并从活动源码/部署面退役 Push Gateway；
2. 删除当前产品面的 Buzz/Builderlab/updater/远程 Community URL 与 fallback；
3. 从公开 lockfile、workflow、脚本和示例中移除 Block Artifactory、内部 ECR、Blox、sqprod 等坐标；
4. 在纯公网源重新生成需要随首发保留的 lockfile；不属于首发的 benchmark 可移出默认构建/发行面；
5. 将 GHCR、GitHub repository 和 release URL 参数化为 Carryforth 权威 namespace；
6. 增加当前工作树和完整 Git history 的 secret 扫描；真实凭据一旦发现，必须先撤销/轮换再清理历史；
7. 审计 Git history、LFS、子模块、fixture 与文档中的个人信息、内部架构材料和不可公开资产；
8. 增加 outbound inventory，区分控制面自动外连与用户显式模型/链接/媒体流量。

阶段门禁：断开外网或封锁旧 Buzz/Block 域名时，本地核心流程仍能运行；CI 扫描不再发现内部制品 URL。

### 阶段三：bundle identity 与本地数据迁移

当前 `xyz.block.buzz.app` 不能作为 Carryforth 独立发行的长期身份。切换到自有 identifier 前必须先实现平台级无损迁移，
而不是直接改 `tauri.conf.json`。

迁移顺序：

1. 启动前识别 legacy bundle/app-data/keyring/storage 坐标；
2. 若新坐标为空，读取旧身份和本地状态；若新旧两侧均有数据则停止并提示冲突，不自动覆盖；
3. 将数据复制/导入到新坐标，保留原目录作为可恢复副本；
4. 重新打开新坐标并回读 pubkey、Community ID、消息/Agent 和三域 revision；
5. 只有全部回读一致，才把新坐标标记为 committed；
6. 首个迁移版本不自动删除旧 keyring entry 或 app-data；后续清理必须是独立、可见、可恢复的用户操作；
7. fresh install 只创建新 Carryforth 坐标，不产生 legacy Buzz 目录；
8. dev 与 production profile 分开迁移，不相互读取。

bundle identity 迁移只约束**进入当前支持矩阵的平台**。首发最低矩阵要求 Linux 完成上述迁移；macOS、Windows
在加入正式发行前分别完成其数据目录、WebView storage、keyring service、Nest/Agent state 和 deep-link 验收。
尚未发布的平台不阻塞 Linux RC，但也不得仅靠修改 identifier 发布。

此前 Desktop 产品面方案保留 `xyz.block.buzz.app` 是当时避免数据丢失的正确边界；本文不会改写那次交付事实。
只有本阶段的迁移实现与回读门禁完成后，才退出该兼容技术坐标。

阶段门禁：已有本地数据更新前后完全一致，fresh install 不生成旧产品身份，冲突场景 fail closed。

### 阶段四：本地栈 bootstrap 与升级合同

1. 选择并实现唯一公开 bootstrap 入口；
2. 固定 Relay/DB/Redis/对象存储等版本或 digest；
3. 移除 Keycloak/Prometheus 等非核心服务对 Desktop readiness 的硬阻塞；
4. 实现幂等初始化、Owner 认领、Project capability bootstrap、健康检查和日志诊断；
5. 保留 `start`、`rebuild-start`、`stop` 的无数据删除语义；
6. 建立从阶段零冻结的 immutable baseline 升级的 migration/readback、备份与恢复门禁；
7. 文档明确本地身份签名与远程帐号登录的区别。

阶段门禁：空机器和已有数据机器都能按同一公开流程启动，停止/重建后数据不变。

### 阶段五：公共 CI 与发行链

1. 新建不依赖 canonical `block/buzz` 判断的公共 CI；
2. 移除 `Buzz.app`、Block 私有 codesign action、内部 bucket/role 与私有 Buildkite 依赖；
3. 统一 Carryforth artifact/container 名，版本由单一 release manifest 派生；
4. 公共 runner 至少构建 unsigned/community artifacts；平台签名使用 Carryforth 自有 secrets，缺少签名时不伪装成
   官方 signed build；
5. 发布 Desktop、Relay image 和 `cf`，生成 checksum、SBOM、provenance 和 release manifest；
6. Release workflow 从 Tauri/Cargo 配置读取产品名，不再硬编码 `.app` 名；
7. fork 的 PR 与 tag 构建必须能运行只读验证，不尝试推送到 Block namespace；
8. 删除 `build-release-config.mjs` 与 release environment 对旧 updater 的再注入，删除不再属于当前产品的
   Mobile/private release 接线；Push Gateway 发布 job 与 executable 直接退役；
9. 对最终安装包解包扫描并做运行时网络审计，证明 release-only 配置没有重新带入旧 updater、Buzz URL 或
   `Buzz.app` 产物名。

阶段门禁：在 Carryforth fork/namespace 中，不提供任何 Block secret 也能完成公开 unsigned 构建；官方发布只增加
签名，不改变二进制功能。

### 阶段六：资产、许可证与供应链收口

1. 建立每个字体、声音、图标、Logo、安装背景和示例数据的 provenance 表；
2. 补齐可再分发许可证，将其汇总到 Third-Party Notices；
3. 无法确认授权的 Provider logo 使用通用图标或文字替代；
4. 对 Cargo、pnpm 和 Python 依赖运行许可证与漏洞检查；
5. 生成 CycloneDX 或 SPDX SBOM；
6. 固定 Rust、Node、pnpm 等公开工具链版本，并同步 README/CONTRIBUTING；
7. 对 vendored binary、Hermit package 和 sidecar 建立来源、版本、校验和与更新流程。

阶段门禁：每个随安装包/容器分发的文件都能说明来源与授权，依赖清单可由 release artifact 回溯。

### 阶段七：Release Candidate 与公开验收

1. 在 clean-room 环境从公开仓库完成构建；
2. 用公开 artifact 从空数据启动 Local Relay 与 Desktop；
3. 创建本地身份与 Owner Community；
4. 验收 Channel、消息、managed Agent、Project View、Document、Project Context 和 4–6 轮 Meeting；
5. 关闭、重启、升级并回读全部数据；
6. 断开旧 Buzz/Block 域名并复跑核心流程；
7. 用已有数据副本执行 bundle/app-data migration；
8. 核对 artifact 名、图标、深链、日志、帮助和网络连接均属于 Carryforth；
9. 发布 Release Candidate，保留可复现的验收记录、SBOM、checksums 与已知限制。

RC 必须从 clean tagged commit 构建：Git index/worktree 干净、无未跟踪输入、submodule/LFS revision 固定、完整
migration 可从空库重放、构建记录指向精确 commit。开发者本机已有数据库或 `../../../target` 产物不得成为隐式输入。

阶段门禁：所有首发支持平台通过后才标记稳定版本；单个平台失败不会由其他平台结果替代。

## 7. 自动门禁

### 7.1 品牌与内部域名门禁

建立按语义分类的 allowlist，禁止当前产品/发布面重新出现：

- `block/buzz`、`block/sprout` 发布与下载 URL；
- Builderlab、Buzz hosted Community 与 Buzz Push endpoint；
- Block Artifactory、内部 ECR、sqprod、Blox 和内部 CI 坐标；
- `Buzz.app`、Buzz 图标/wordmark 和旧 deep-link scheme；
- 当前用户/Agent 帮助中的 `buzz` CLI。

许可证、上游归属、历史文档、内部 crate 和协议 ID 可以保留，但必须由有理由的类别 allowlist 覆盖，不能逐次随意
加例外。

### 7.2 Secret 与隐私门禁

- 每个 PR 扫描新增内容；
- release 前扫描完整 Git history；
- 对测试 key、fixture 和 placeholder 使用明确格式，避免与真实 secret 混淆；
- CLI/help 不回显 secret 当前值，只显示变量名或脱敏状态；
- 构建日志、SBOM、provenance 和 crash report 不包含私钥、token、认证 tag 或内部 URL；
- 默认网络审计记录目的域、触发功能和是否用户显式操作，但不记录 secret 或消息正文。

### 7.3 数据安全门禁

- migration/integration test 必须验证 scratch database 名称或唯一 marker；
- 遇到 localhost 主开发数据库、宽泛路径或未知 volume 时直接拒绝破坏性动作；
- Release/CI 脚本不得执行 `docker compose down -v`、全局 reset 或 app-data 清理；
- bundle migration 在提交前后做身份与业务 revision 对照；
- clean build/cache cleanup 只触及明确的构建缓存目录。

### 7.4 可复现构建门禁

- 公共 DNS/registry 环境能解析全部依赖；
- lockfile 不含内部 source/index；
- build 不读取开发者 home 中的旧 binary、keyring 或配置；
- build 只接受 clean tagged commit，拒绝未跟踪文件、缺失 migration、漂移 submodule/LFS 或未声明生成输入；
- release artifact 的版本、Git commit、toolchain、SBOM 与 checksum 可相互对应；
- 同一 commit 的 clean build 产物差异若无法完全消除，至少记录非确定来源并阻止签错版本。

## 8. 验收矩阵

| 场景 | 必须结果 |
| --- | --- |
| 无 Block/Buzz 权限的公共 CI | 构建、测试和 unsigned artifact 成功 |
| dirty/untracked RC source | 发布任务 fail closed，不消费本机文件 |
| 空数据库 migration replay | 从仓库完整迁移到目标 schema，随后可启动 Relay |
| 封锁旧 Buzz/Block 域名 | localhost 核心功能不受影响 |
| 只安装 Desktop、未装本地栈 | 明确提示本地依赖，不远程 fallback |
| clean bootstrap | 自动生成 secrets、启动 Relay、初始化 Owner、启动 Desktop |
| 重复 bootstrap | 幂等，不创建第二个 Community，不改变身份 |
| stop / rebuild-start | 容器/进程重启，业务数据不删除 |
| 旧 bundle 升级 | pubkey、Community、消息、Agent、三域 revision、Meeting 全部一致 |
| 新旧坐标同时有数据 | fail closed，要求 Human 选择，不覆盖任一侧 |
| Carryforth 首发 Relay | 零 Push 外连，且没有旧服务启用开关 |
| localhost 不可用 | 只显示本地错误，不尝试远程 Community |
| `cf` smoke | help 不泄密，核心 read/write/exit code 正常 |
| Meeting smoke | 4–6 轮自然流程、Action 物化、Context Edge 与 closed 正常 |
| Release artifact | Carryforth 命名、checksum/SBOM/provenance 完整 |
| 最终安装包与运行时网络 | 不含旧 updater、`Buzz.app` 或 Buzz release 请求 |
| Relay OCI 内容 | 不暴露未收敛的 Web/Admin 页面 |
| source/asset audit | 无内部 lock URL，每个打包资产有许可来源 |
| 历史数据读取 | 保留旧协议 ID 和历史 Meeting，不因品牌迁移回退 |

## 9. 风险与取舍

### 9.1 首发范围较小

不同时发布 Web、Push 和所有平台，并从活动树退役 Mobile，会减少“全平台”宣传面，但能保证公开承诺与实际可用性
一致。后续组件应复用本计划门禁逐个加入，而不是降低首发门槛。

### 9.2 bundle identity 迁移复杂

继续使用 `xyz.block.buzz.app` 会保留旧产品身份；直接改 identifier 又会表现为身份和数据丢失。因此必须把迁移作为
独立阶段，并允许 Linux 先发布、其他平台在迁移验证后加入。

### 9.3 内部技术名仍可见

诊断日志、crate 和 capability 中仍可能出现 `buzz-*`。这不等同于产品面未切换，但需要明确 allowlist。若后续要
迁移 runtime 名称，应按 binary/env、observability、wire/storage 三层分别设计，不能机械替换。

### 9.4 local-only 降低自动恢复能力

没有远程 fallback 后，本地 Relay 故障必须由本地健康检查、日志和恢复工具解决。这是本地产品的明确选择，不应通过
重新加入隐藏远程服务来规避。

### 9.5 第三方模型并非完全离线

Codex、Claude 等 managed Agent 仍可能需要用户自行配置并访问其供应商。发布说明必须把这类用户选择的出网与
Carryforth 控制面默认不出网区分开；若未来承诺 air-gap，需要另行提供本地模型与素材代理方案。

## 10. 完成标准

只有同时满足以下条件，才能把 Carryforth 标记为独立开源初版：

1. 首发范围、支持平台和已知限制已公开且与实际 artifact 一致；
2. 当前产品、仓库治理、release metadata 与公开 URL 均属于 Carryforth；
3. Apache-2.0、上游归属和第三方 NOTICE 完整；
4. 默认没有 Buzz/Builderlab/Block 控制面外连或 fallback；
5. 公开 lockfile、构建和测试不依赖内部网络、内部 action 或 Block secret；
6. 全新机器能通过公开入口启动完整本地 Carryforth 栈；
7. 已有本地用户升级后身份与全部业务数据保持一致；
8. 公共 CI 能生成 Carryforth Desktop、Relay image、`cf`、checksum、SBOM 与 provenance；
9. 打包资产与依赖的来源、许可和安全扫描通过；
10. clean-room 端到端验收覆盖消息、Agent、Project 三域与 Meeting，并保留脱敏证据；
11. release、rebuild、migration 和测试脚本均有数据安全门禁；
12. 未通过的 Web 或平台构建没有被宣传为受支持发行物，Mobile 与 Push/Helm executable 不存在于当前活动面。

完成这些条件后，后续再分别规划 runtime binary/env 去 Buzz 化、内部 crate 命名整理、Web 产品面、未来新建的
Mobile 产品面以及可选 self-hosted Push；这些后续工作不得反向改变本阶段建立的 local-only、无损升级和公开可复现
边界。

## 11. 本轮实施记录（2026-08-11）

本轮已经完成的是**公开发布面的基础收敛与 fail-closed 门禁**，不是首个稳定版本已经可发布：

| 阶段 | 当前状态 | 已交付 / 剩余边界 |
| --- | --- | --- |
| 阶段零 | 部分完成，RC 阻断 | 首发范围固定为 Linux Desktop、Local Relay、ACP 与 `cf`；clean tag 与完整 migration 门禁已建立。签名主体、正式安全/行为准则私密入口、发布治理责任和 immutable upgrade baseline 仍需 Human 固定，并提交 tag-bound evidence。 |
| 阶段一 | 代码完成，Human 决策待办 | README、治理、安全策略、贡献说明、package metadata、`NOTICE` 与 `../../../UPSTREAM.md` 已切到 Carryforth。reverse-DNS identifier、签名与长期联系人尚未选择，因此 bundle identity 未改。 |
| 阶段二 | 首发运行面完成，仓库历史审计待办 | 公开 lockfile 已改为公网源；Relay 首发 binary 不再启动或公告旧 Push，OCI image 不再打包 Web/Admin；公开 release/current test 示例中的内部服务坐标已清理。完整 Git history 的 secret/隐私审计仍未完成。 |
| 阶段三 | 未开始 | 仍保留旧 bundle、keyring 与 app-data 坐标以保护现有数据。没有无损迁移与回读证据前不得直接改 identifier。 |
| 阶段四 | 入口已建立，RC 阻断 | `../../../deploy/local` 提供持久化 local-only 栈、随机 secret 与非删除 lifecycle；但 Owner 签名后的 Project View v3/Document/Context Edge/Meeting capability bootstrap 与 canonical readback、从 Human 冻结基线执行的既有数据 migration/readback 仍未完成。当前入口不能宣称为稳定升级路径。 |
| 阶段五 | 公共 unsigned lane 已建立，发布被门禁阻断 | 公共 workflow 已移除旧 updater、Block 私有签名/移动端/Push 发布链，可构建 Linux Desktop、`cf`、Local Stack 与 Relay OCI，并生成 checksum/provenance。正式 release 在资产、SBOM、container provenance 等门禁未通过时会 fail closed；锁定的 RustSec 审计及其归档证据尚未接入 CI，不能宣称依赖安全门禁已完成。 |
| 阶段六 | inventory 完成，8 项 blocker | 已建立逐文件资产 provenance/hash inventory，并把原先只存在于文字合同中的四项义务纳入机器门禁。当前为 1 项 Desktop 字体 blocker 与 7 项 release obligation：除依赖/SBOM、Relay container provenance、bundle identity 外，还明确阻断 Owner-signed capability bootstrap、既有数据 migration/readback、clean-room E2E 与 Human 私密报告/发行治理决策。 |
| 阶段七 | 未开始，RC 阻断 | 尚未执行仅使用公开发行物的 clean-room fresh install、已有数据升级、断旧域名网络审计和完整 Meeting 端到端 RC 验收，也没有对应的 tag-bound evidence。 |

当前开发门禁：

```bash
./scripts/check-open-source-release-surface.sh
./scripts/test-carryforth-local-deployment.sh
./scripts/check-release-asset-inventory.sh
```

前两个命令验证当前源码与 bootstrap 合同；第三个命令验证资产清单完整性并列出未解决项。正式 tagged release 会运行
严格模式并在任何资产/供应链/运行验收/治理 blocker 存在时拒绝发布。release obligation 不能只修改
`release_status`：`cleared` 必须指向 `release/evidence/<tag>/<obligation-id>.json`，并由清单绑定 evidence
schema、release tag 与 SHA-256；严格模式还会核验 tag、记录的 source commit 与 release `HEAD` 一致。不得通过删除
门禁、复用其他 tag 的报告或把 blocker 改成警告来制造“可发布”结果。
