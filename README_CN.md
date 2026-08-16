<h1 align="center">Carryforth</h1>

<p align="center">
  <strong>连续性属于项目，而不是某个 Agent。</strong>
</p>

<p align="center">
  一个本地优先、以项目为持续主体的人类—Agent 协作空间。
</p>

<p align="center">
  <a href="README.md">English</a> ·
  <a href="docs/cn/README.md">中文文档</a> ·
  <a href="docs/project-positioning.md">项目定位</a> ·
  <a href="ARCHITECTURE.md">架构</a> ·
  <a href="CONTRIBUTING.md">参与贡献</a> ·
  <a href="SECURITY.md">安全</a> ·
  <a href="LICENSE">Apache 2.0</a>
</p>

> [!IMPORTANT]
> Carryforth 仍处于**开发阶段**。当前仓库仅供从源码进行本地构建、功能评估和参考学习；
> 尚未发布稳定安装包，也不承诺生产部署、正式支持或既有数据的稳定升级路径。

## 来源与致谢

Carryforth 是基于 Block, Inc. 以 Apache License 2.0 发布的
[`block/buzz`](https://github.com/block/buzz) 源码开发和演进的独立项目，**不是从零重写**。
Buzz 提供的 Desktop、本地 Nostr Relay、Agent 运行与协作基础，为 Carryforth 提供了坚实的工程起点。

我们感谢 Block, Inc. 和所有 Buzz contributors 对开源社区的贡献，也推荐对本地优先协作、
Nostr 与 Agent 工作空间感兴趣的读者了解原始 [Buzz 项目](https://github.com/block/buzz)。

Carryforth 在这一基础上继续探索“以项目而不是 Agent 作为持续主体”的方向，加入并重构了
Project View、Role Continuity、Project Documents、Project Context、结构化 Meetings、
本地单 Relay 运行边界和面向 Agent 的 `cf` CLI 等能力。

Carryforth 由独立维护者维护，与 Block, Inc. 无隶属、赞助或背书关系。
公开源码基线是基于
[`block/buzz@ab3af828`](https://github.com/block/buzz/commit/ab3af828714ab699dfc87644d234014987a4fe6b)
审核后导入的压缩快照；Carryforth 仓库不复制 Buzz 的提交祖先链。仓库保留适用的
上游许可证和版权通知，并以 Carryforth 自有的 [NOTICE](NOTICE) 记录这一归因。
详见 [LICENSE](LICENSE) 与 [UPSTREAM.md](UPSTREAM.md)。代码中的 `buzz-*` 名称、
`BUZZ_*` 环境变量以及部分数据库、协议和 bundle 坐标是既有 wire / storage /
数据连续性的兼容合同，不代表当前产品身份。

## Carryforth 是什么

今天的 Agent 很擅长完成一项任务，却很难天然地延续一个长期项目。
上下文通常留在某次对话、某个 Leader、某个 Agent 的记忆里；当会话结束、模型更换、
团队解散或成员离开时，项目往往又要从头解释自己。

Carryforth 把这个关系反过来：项目长期存在，人类与 Agent 以成员、角色和责任加入；
成员可以进入、离开、恢复或被替换，但项目的认知、工作状态、文档、上下文、已记录选择和承诺继存。

这里的基本单位不是一次对话、一个代码仓库或一支临时 Agent Team，而是**项目**。
Agent 是项目中具有独立生命周期的成员；即使是 Leader，也不拥有项目的全部上下文，
更不是项目得以延续的必要条件。

Carryforth 不是一个“记住所有聊天”的超级 Agent。它提供一个共同的项目空间：
人类和 Agent 在同一套身份、权限和项目状态上协作，并把真正会影响未来工作的内容持续写回项目。

## 界面预览

### Project View

![Carryforth Project View 项目概览](docs/image/project-view-overview.png)

Project View 汇总项目方向、计划与阶段、角色、待关注事项和资源，让人类与 Agent
从同一份经过验证的项目状态继续工作。

### Project Context

![Carryforth Project Context 关系图](docs/image/project-context.png)

Project Context 将项目对象、Documents 和 Meetings 之间显式保存的关系组织成可浏览的上下文图；
图中的布局只用于浏览，不表示排名或因果。

### Meetings

![Carryforth 结构化 Agent Meeting](docs/image/meeting.png)

这张本地开发界面截图展示的是 Meeting 的 action-recording 恢复保护状态：当 action
materialization 等待恢复时，共享 Board 和已有结果记录仍保持可见。它是恢复状态示例，
不是理想化的已完成 Meeting。

## 项目如何保持连续

```text
Project / Community
│
├── Project View
│   ├── Project Profile
│   ├── Goal
│   ├── Role
│   ├── Plan
│   ├── Stage
│   ├── Requirement
│   ├── Issue
│   ├── Work
│   └── Resource
│
├── Project Documents
├── Project Context
├── Meetings
└── Human / Agent Members
```

Project View 保存项目当前的一阶状态；Documents 保存可演进的项目内容；Project Context
解释对象之间“为什么相关”；Meetings 承载正式协作；Role、Assignment、Checkpoint 和 Handoff
让责任在 Agent Runtime 更换后仍可继续。

每个模型的身份、关系和边界见[核心模型](docs/cn/core-model.md)。

## Role Continuity

Role 是 Project 长期持有的稳定责任，Assignment 是 Human / Agent Member 承担它的一段任期。
Work Responsibility 跨任期持续，Commitment 归因到具体 Assignment 与 Member；持续追加的 Checkpoint、可选
Handoff 和派生 Role Brief 让继任者无需等待前任上线或提交退出总结，也能从 Project 状态接续工作。

详见[核心设计：Role Continuity](docs/cn/core-design/role-continuity.md)。

## 实验性的上下文环境感知 Agent 图检索

Agent 可以结合当前 Role，以及与问题有关的 Work、Issue、Meeting 目的等已验证任务事实，渐进检索
Project Context。通常直接从当前工作已经明确的 Coordinate 开始；没有可靠起点时，才用全图语义
检索提出候选并由 Agent 自己观察筛选。随后 Agent 按 `Coordinate → Edge → Coordinate` 逐跳前进：
语义排名缩小每一步的候选范围，canonical 轻量观察帮助排除误匹配，relation Document 保留为什么
能够经过这条关系的依据，完整正文只在实际任务需要时按需读取。

因此，同一个问题可以让处于不同上下文环境的 Agent 取得不同但相关、可追溯的上下文路径。
所有 Agent 仍读取 Project 共同持有的一张 Context Graph，不为 Role 或 Agent 建立私有图；语义排名
不会替 Agent 自动选择路径，遍历只沿真实无向 Hyperedge 进行，也不会创建或改写项目关系。

其中的语义能力并非完全本地：索引可能将来源类型、当前可见标题/名称和可选摘要发送给用户配置的
Provider；当前 foundation 不发送 Document 正文或 chunk。自然语言起点与一跳检索会把 query 文本
发送给同一 Provider。源码启动可以准备语义进程和 Provider 配置，但 operator 仍须单独开启 Community
durable gate，并明确确认这项 Provider 数据出境。

详见[核心设计：Agent 自主的上下文环境感知 Project Context 图检索](docs/cn/core-design/context-aware-semantic-graph-retrieval.md)。

## 当前能力

当前仓库已经把以下能力接入同一个本地项目边界：

- Carryforth Desktop：项目导航、Project View、Documents、Project Context 和 Meetings；
- 本地 Relay：Community 权限、签名事件、规范状态、查询和审计边界；
- ACP 托管 Agent：以项目成员身份运行，并接收受控的 Carryforth 环境；
- [`cf` CLI](docs/cn/cli-reference.md)：面向 Agent 的消息、项目对象、文档、上下文、会议和媒体操作；
- Channels 与 Messages：基于签名 Nostr 事件的日常协作；
- Git 项目协作与内容寻址媒体的预览能力；
- 可选、受门控的语义候选发现与 Agent 渐进式 Project Context 图检索。

Relay 是当前的权威状态边界。系统会校验和保存结构，但不会自动理解整个项目，
也不会把每段聊天、草稿或模型推理自动提升为项目事实。

**已实现不等于新环境中默认开放。** Project View、Documents、Project Context、Meetings、
Git Projects 和语义检索面仍有预览开关、Relay readiness、Community durable gate 或 Owner
签名初始化等要求。具体能力与启用边界见[当前状态](docs/cn/current-status.md)。

## 从源码启动

当前支持的是源码开发与本地评估流程。准备好 Docker 24+（含 Compose v2）、Python 3、`curl`
和 Tauri 所需的系统依赖后运行：

```bash
git clone https://github.com/lgYanami/Carryforth.git
cd Carryforth
./start.sh
```

脚本只检查外部系统依赖，不会安装 Docker、Python、`curl` 或系统软件包。首次运行会创建私有的本地
`.env`。源码启动默认开启语义 Worker 和 Query HTTP 进程开关，因此会在缺少时询问
Provider API Key、HTTPS Base URL 和 Request Model，三者都没有默认值。也可以在启动前显式
关闭这两个默认开启的进程开关；Agent 起点发现与一跳语义 process master 仍默认关闭。进程启动不会
开启任一 Community 语义 gate，也不代替 Provider 数据出境确认。启动过程会保留已有 Docker volume
和项目数据。

> [!WARNING]
> 这是仅用于可信本机的开发栈。仓库中的 `.env.example` 会把 Relay 绑定到 loopback，
> Relay 原始二进制的默认绑定也是 loopback，仓库中的 Compose 文件也只把依赖端口发布到
> loopback；但所有本地服务仍使用开发凭据。只能在可信主机上运行；
> 未经过独立的安全设计，不要主动把这些端口暴露到局域网或 Internet。

完整流程、关闭语义配置的方法，以及重建和停止命令见[本地源码开发](docs/cn/local-development.md)。

## 继续阅读

- [中文文档导航](docs/cn/README.md)
- [`cf` CLI 功能参考](docs/cn/cli-reference.md)：全部当前命令域与可执行子命令，
  以及身份、输出、冲突和能力边界
- [核心模型](docs/cn/core-model.md)：Project View、Role Continuity、Documents、Context、Meetings 与成员
- [核心设计：Role Continuity](docs/cn/core-design/role-continuity.md)：
  责任、任期、Work 承诺和外化局势如何跨 Agent 与 Runtime 持续存在
- [核心设计：先有坐标，后有上下文](docs/cn/core-design/coordinate-and-context.md)：
  坐标上下文、关联上下文与 Agent 的渐进式发现
- [核心设计：Agent 自主的上下文环境感知 Project Context 图检索](docs/cn/core-design/context-aware-semantic-graph-retrieval.md)：
  Agent 如何结合 Role 与工作环境，从一张共同图中选择不同但相关的路径
- [核心设计：Meeting](docs/cn/core-design/meeting.md)：
  Human 与 Agent 如何聚合分布式上下文、形成共同结论并显式产出
- [系统概览](docs/cn/system-overview.md)：组件、数据流、身份、权限、安全与本地优先边界
- [本地源码开发](docs/cn/local-development.md)：依赖、配置、构建、启动、停止与数据保护
- [当前状态](docs/cn/current-status.md)：预览能力、启用条件、本地范围与延后的制品边界
- [项目定位](docs/project-positioning.md)与[项目空间宪章](docs/project-space-constitution.md)

## 当前阶段

Carryforth 是一个活跃开发中的源码项目。当前公开范围仅包括
**从源码进行本地构建、功能评估和参考学习**，不包含二进制、安装包、容器或其他已打包
发行物，也不承诺生产部署、正式支持或既有数据的稳定升级路径。

当前本地评估范围聚焦 Linux Desktop、本地单 Relay、ACP 托管 Agent、`cf` CLI，
以及 Channels、Messages、Project View、Documents、Project Context 和 Meetings。
Web 当前也只是源码面；macOS、Windows、自动更新、生产级多实例部署和长期升级不属于已承诺能力。

## 贡献与许可证

提交代码前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)。安全漏洞请按
[SECURITY.md](SECURITY.md) 私下报告，不要发布到公开 Issue。

Carryforth 源码以 [Apache License 2.0](LICENSE) 分发，保留适用的上游版权通知，并在
[NOTICE](NOTICE) 中独立记录归因。第三方依赖和素材可能适用各自许可证，当前源码审计以及
延后的未来制品边界见
[release/THIRD_PARTY_ASSETS.md](release/THIRD_PARTY_ASSETS.md)。
