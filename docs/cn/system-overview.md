# Carryforth 系统概览

> 本文从产品运行角度介绍 Carryforth 的组件、数据流、身份、权限和网络边界。
> 协议与 crate 级细节见 [ARCHITECTURE.md](../../ARCHITECTURE.md)。

## 1. 总体结构

```text
┌──────────────────────────────────────────────────────────────────┐
│ Carryforth Desktop     Managed Agents / ACP       cf CLI         │
└───────────────┬──────────────────┬──────────────────┬────────────┘
                └──────────────────┼──────────────────┘
                                   │ signed Nostr events
                                   ▼
                         Local Carryforth Relay
                         (内部 crate：buzz-relay)
                                   │
                 ┌─────────────────┼─────────────────┐
                 ▼                 ▼                 ▼
             PostgreSQL           Redis           S3 / MinIO
```

当前支持面是本地单 Relay 架构。Desktop、Managed Agents 和 `cf` 不各自维护一份可竞争的项目事实；
它们通过 Relay 读取和提交 Community-scoped 状态。

这里的 Project / Community 与 Desktop 中名为 **Projects** 的功能不是同一个概念：

- Project / Community 是 Carryforth 的长期协作、身份和数据边界；
- Projects 是 Git 仓库、分支、提交、Issue 和 PR 的协作预览面。

一个 Carryforth Project 可以关联多个代码仓库；系统不把“项目”等同于“一个 Git 仓库”。

## 2. 主要组件

### 2.1 Carryforth Desktop

Desktop 是当前主要的人类界面，基于 Tauri 2 与 React 19。它提供 Channels、Messages、
Agent 管理、Project View、Documents、Project Context、Meetings、Git Projects 和媒体等界面。

Desktop 的部分高级功能仍受 Settings → Experiments 预览开关控制。
客户端开关只决定是否显示界面，不会绕过 Relay 的 capability、Community gate 或权限验证。

### 2.2 Local Relay

Relay 是规范状态和权限边界，负责：

- Nostr / NIP 事件接收、过滤、签名与查询；
- host 到 Community 的租户绑定；
- 成员、权限和 capability 检查；
- Project View、Documents、Context 与 Meeting 的验证和持久化；
- HTTP bridge、NIP metadata、媒体和 Git smart HTTP 等确有 HTTP 需求的表面；
- 后台 worker、审计和运行 readiness。

客户端提交的 event tag 不能决定 tenant。Community 必须由 Relay 根据受信任 host / workspace
坐标解析，防止客户端把数据写入另一个项目边界。

### 2.3 `cf` CLI

`cf` 是面向 Agent 的 Carryforth CLI。它对 Relay 请求签名，并为 Messages、Channels、Project View、
Documents、Project Context、Meetings、Git / PR、媒体、Role、Resource 和工作流提供闭合的读写合同。

Managed Agents 通过 `CARRYFORTH_RELAY_URL`、`CARRYFORTH_PRIVATE_KEY`
和 `CARRYFORTH_AUTH_TAG` 获得受控会话。私钥不得出现在日志、文档或命令示例中。

### 2.4 ACP 与 Managed Agents

`buzz-acp` 将 Relay 会话桥接为 ACP stdio JSON-RPC。Desktop 可以发现和管理 Built-in、Goose、
Claude Code、Codex 等 Runtime，并跟踪启动、停止、日志和 session 状态。

Runtime 是可替换执行实例，不是 Project 的状态所有者。Built-in Agent 也不是内置离线模型，
仍需要 Anthropic、OpenAI-compatible、Databricks 或其他受支持 Provider 的配置；
外部 Runtime 需要相应 CLI 或适配器。

### 2.5 数据与依赖

- PostgreSQL / pgvector：规范投影、项目领域状态和可选语义索引；
- Redis：运行协调、队列或缓存类依赖；
- MinIO：内容寻址媒体对象；
- Keycloak：开发身份基础设施；
- Prometheus：本地运行指标。

具体数据归属以 schema、migration 和对应领域代码为准，不应从这份概览推导表级合同。

## 3. 规范状态与派生状态

Carryforth 区分规范项目状态和派生读取结果：

- Project View 对象、Document Revision、Context Edge、Meeting 状态和签名消息是规范记录；
- Role Brief、客户端或模型生成的摘要、索引、查询结果和部分 UI cache 是从规范状态派生的读取面；
- 派生状态不能在冲突时覆盖 Relay 中的权威对象；
- 语义候选结果和 Agent 检索路径不作为新的虚拟 Event 写回规范历史。

这一区分避免某个 Agent 的总结、某次查询或某个客户端缓存静默成为项目的新事实源。

## 4. 身份与权限

### 4.1 稳定身份

Human 与 Agent 使用 Nostr 密钥对作为稳定身份，Relay 操作保持签名。
Agent Runtime、模型、Provider、Persona 和进程 ID 都不是成员身份本身。

Desktop 优先把私钥保存在操作系统 keyring；缺少可用 keyring 时，受支持路径可以回退到
权限受限的本地文件。Provider API Key 则保存在被 Git 忽略的私有 `.env`，
与 Nostr 成员身份和 Relay 签名密钥是不同的凭据边界。

### 4.2 Community 边界

当前一个 Community 对应一个 Project 的租户边界。Project View、Documents、Context、Meetings、
Messages 和成员状态都必须属于同一 Community；跨 Project 引用和写入应失败关闭。

### 4.3 授权层次

授权不是一个前端布尔开关，而是多层共同成立：

- host / workspace 绑定；
- Community membership 与 ban 状态；
- owner、admin、member 基础等级；
- 领域 capability 与 durable gate；
- 当前对象 Revision、generation 或生命周期；
- 调用者签名和请求正文绑定；
- Provider 出境和结果释放前的再次验证。

因此，某个界面可见或某个进程已启动，不代表当前成员自动获得写入或外部查询权限。

## 5. 本地优先的含义

Desktop 默认连接：

```text
ws://localhost:3000
```

本地 Relay 不可用时，Desktop 报告本地连接错误，不会回退到旧的 Carryforth / Buzz hosted account、
community、updater 或 push 服务。

“本地优先”不代表：

- 所有网络请求都被禁止；
- 修改仓库中的 loopback 默认绑定不需要单独安全设计；
- 系统不需要身份和授权；
- Provider、远程媒体或 Git remote 不会访问网络；
- 开发 Compose 已达到生产加固标准。

源码开发环境会向宿主机 loopback 发布多个服务端口，并使用开发配置和开发凭据。
未经单独安全设计，不要扩大绑定范围并暴露到不受信任网络。

## 6. 语义 Provider 边界

Project Context 语义发现使用用户配置的外部 Provider 生成向量。语义索引覆盖 eligible current
Coordinates 与 relation Documents。Managed Agent 可以用它发现起始 Coordinate、按 relation Documents
排列 incident Edges，以及在一条已选 Edge 内排列成员；随后由 Agent 结合当前 Role 和相关工作环境，
而不是由 Provider 自动生成渐进阅读路径。

允许进入 Provider 请求的索引信息受到合同限制，每次查询还要经过 Community gate、成员授权、对象
currentness、Provider admission 和结果签名等检查。

启用语义检索时，operator 必须明确确认自然语言 query 与索引 overview 文本（来源类型、当前可见
标题/名称和可选摘要）会离开本地控制面并发送给所配置的 Provider；当前 foundation 不发送 Document
正文或 chunk。Provider API Key 只应存在于本地私有环境或受控 secret 注入中，不应写入 Project
Document、日志或事件。

语义检索是可选能力。关闭 Worker 和语义 HTTP 进程，或关闭 Community query gate，不会删除
Project View、Documents、Context Edge 或其他规范项目数据。保留的有界完整路径型
`semantic-query` 使用同一派生基础设施，但只是 Agent 自主渐进检索的补充能力。

## 7. 媒体与外部内容

Relay 提供 Blossom 风格的内容寻址上传，并使用 MinIO 保存媒体。
Desktop 支持图片、视频和普通文件附件；本地图片处理和视频转码可能依赖系统 `ffmpeg`，
而根启动脚本不会安装或检查 `ffmpeg`。

外部链接、Git remote、媒体 URL 和 Resource Guide 都可能指向本机之外。
“对象记录在本地 Project”不等于它指向的外部内容已经离线复制、长期可用或受 Carryforth 授权控制。

## 8. 审计与安全边界

审计记录和哈希链用于检测不一致与篡改，但不能被描述成：

- 已经获得合规认证；
- 在拥有底层数据库写权限的攻击者面前绝对不可重算；
- 所有外部系统行为都能由 Carryforth 审计；
- 对用户数据提供了端到端加密保证。

安全模型和已知边界见 [SECURITY.md](../../SECURITY.md)。安全漏洞必须按其中方式私下报告。

## 9. 兼容标识

代码中仍保留大量 `buzz-*` crate / binary、`BUZZ_*` 环境变量、数据库标识、Nostr kind、
bundle / keyring 与 app-data 坐标。这些名称可能是 wire、存储或既有数据连续性的兼容合同。

它们不是当前产品身份，也不能为了文案统一而机械重命名。任何变更都需要独立的数据与协议迁移设计。
来源和归属说明见 [UPSTREAM.md](../../UPSTREAM.md)。
