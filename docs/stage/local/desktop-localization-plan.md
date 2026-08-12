# Desktop 本地化方案

> 状态：代码交付完成，现场验收待执行
> 日期：2026-08-09
> 范围：Desktop 与本地 Relay 启动协作；不包含 Web、Mobile

## 1. 目标

让 Carryforth Desktop 默认作为本地应用运行：

- 只连接 `ws://localhost:3000`；
- 不要求 Builderlab/Buzz 远程账号注册或登录；
- 首次启动直接建立并进入本地社区；
- 远程社区相关代码暂不删除，但不挂载、不调用、不联网，也不提供重新启用入口；
- 清理旧 Buzz 远程社区配置与客户端缓存，不为其保留兼容路径；
- 在全新环境中，让首个本地 Desktop 身份安全地成为本地社区 Owner。

## 2. “无需认证”的边界

本地化去除的是**远程账号认证**，不是取消身份与签名：

- 不触发 Builderlab 登录、远程托管社区认证及远程身份绑定；
- Desktop 继续在本机生成并持久化 Nostr 身份；
- Desktop 与本地 Relay 之间继续使用透明的 NIP-42/NIP-98 签名；
- Agent 所需的 Codex/Claude 等模型供应商认证属于独立能力，不在本阶段处理。

## 3. Desktop Local-only 模式

Desktop 默认运行在 `local-only` 模式：

1. 实际 Relay 地址固定为 `ws://localhost:3000`；
2. 自动创建或复用 `Local Dev` 社区并将其设为当前社区；
3. 不显示首次远程社区选择、Hosted Communities、远程加入或创建入口；
4. 不响应远程社区 connect/join/add deep link；
5. Native 端拒绝非本地 workspace，作为前端门禁的兜底；
6. 远程能力直接关闭，不提供构建开关、部署开关、设置项或运行时启用途径；
7. 本地 Relay 不可用时只报告本地连接错误，不允许降级或回退到远程 Relay。

## 4. 远程社区数据清理

Carryforth 不继承 Buzz 远程社区的数据与兼容责任：

- 启动本地化版本时，移除所有非 `ws://localhost:3000` 的社区配置；
- 当前 active community 若指向远程社区，直接切换到本地 `Local Dev`；
- 清理远程社区对应的图标、未读、消息快照、导航状态与 Agent runtime pair 等本地客户端缓存；
- 不连接远程 Relay，不执行远程注销，也不尝试迁移或下载远程社区内容；
- 保留本地身份、本地社区、本地 Relay 数据库、项目数据和本地 Agent 定义；
- 本地社区初始化保持幂等，重复启动不得重复创建或破坏本地数据。

这是一次明确的单向切断：被删除的只是 Desktop 上与 Buzz 远程社区相关的配置和缓存，不是本地业务数据。

## 5. 本地 Owner 首次认领

采用“空社区首次认领”方案：

- 仅本地部署模式允许认领；
- 仅当社区尚无 Owner、尚未完成初始化时允许执行；
- 首个通过本地签名认证的 Desktop Human 身份成为 Owner；
- 认领与 Owner 写入必须原子完成，只能成功一次；
- Owner 已存在后永久关闭自动提升路径；
- 后续身份不会因为连接 localhost 而自动获得 Owner；
- 远程部署不得暴露或复用该认领能力。

该机制只负责首次权限引导，不替代正常的 Community owner/admin/member 权限模型。

## 6. 启动与依赖

本地 Desktop 的必要启动链路应收敛为：

1. 启动本地数据服务与 Relay；
2. 启动 Desktop，加载或生成本地身份；
3. 幂等建立本地社区并完成首次 Owner 认领；
4. 进入 Desktop，恢复本地 Agent 与项目能力。

Builderlab、远程 Relay 和远程账号服务不得成为本地 Desktop 的启动门禁。本地数据库、Docker volume、身份目录和业务数据不得因本地化而被重置；旧 Buzz 远程社区的客户端配置和缓存按第 4 节清理。

## 7. 分阶段交付

### 阶段一：Desktop 连接策略

- 建立默认 `local-only` 模式；
- 自动选择本地社区；
- 停止所有远程社区后台访问；
- 清理既有远程社区配置和缓存；
- 保留远程实现代码，但删除所有用户可达和部署可达的启用路径。

### 阶段二：首次 Owner 认领

- 增加仅本地、仅空社区可用的一次性认领；
- Desktop 首次启动完成认领并回读 Owner 状态；
- 保证并发请求下只有一个身份成功。

### 阶段三：远程入口收敛

- 隐藏远程社区与 Builderlab UI；
- Native 远程命令 fail-closed；
- 禁止远程社区 deep link、探测和自动回退。

### 阶段四：验收

- 全新环境从零启动；
- 已有环境保留全部本地数据，并清理旧远程社区客户端状态；
- 重启后身份、Owner、消息和项目数据保持；
- 验收 Channel、Agent、Project View、Document、Project Context 与 Meeting；
- 通过网络审计确认 Desktop 未连接 Builderlab 或远程社区。

## 8. 完成标准

- 全新机器无需远程账号即可进入 `Local Dev`；
- 首个 Desktop 身份是本地社区唯一的初始 Owner；
- 本地全功能可用，重启后权限与数据连续；
- 旧远程社区配置和缓存已清理，且不存在主动远程连接；
- 本地 Relay 不可用时明确报错，不自动连接任何远程 Relay；
- 不存在能够重新启用远程社区能力的构建、部署或运行时开关；
- 没有删除、重建或清空现有数据库与持久化目录；
- Web、Mobile 和既有远程社区实现代码不因本次本地化被删除。

## 9. 非目标

- 本阶段不做产品重命名和完整品牌替换；
- 不修改 Web 或 Mobile；
- 不取消 Nostr 身份、事件签名或 Community 权限模型；
- 不处理模型供应商账号、更新服务、链接预览等严格 air-gap 议题；
- 不迁移、同步或恢复旧 Buzz 远程社区内容。

## 10. 实施记录

本次代码交付已完成以下关键点：

- Desktop 的 Native 与前端连接坐标固定为 `ws://localhost:3000`，不存在远程启用开关；
- 启动时幂等创建或复用 `Local Dev`，删除旧远程 Community 配置及其客户端缓存；
- Hosted Communities、远程添加/切换、远程 Community deep link 与 Builderlab 网络入口均已关闭；
- workspace、Agent runtime、延迟 Profile 写入和 NIP-42 签名入口在 Native 层拒绝非 canonical localhost；
- 最终 Desktop 身份通过 loopback + NIP-98 入口原子认领 greenfield Community Owner，已有 Owner 永不被替换；
- 未引入数据库重建、volume 清理、身份重置或本地业务数据迁移。

已完成静态类型检查、Desktop 构建、相关前后端单元测试与 Rust clippy。全新环境启动、已有数据重启以及出站网络审计仍属于现场验收项，完成前不宣称整体本地化验收通过。
