# Carryforth 中文文档

这里是 Carryforth 的中文阅读入口。根目录的 [README_CN.md](../../README_CN.md)
只负责介绍产品定位和最短上手路径；模型、运行方式、系统边界和当前成熟度分别记录在专题文档中。

## 建议阅读顺序

1. [核心模型](core-model.md)

   了解为什么连续性属于项目，以及 Project View、Role Continuity、Documents、
   Project Context、Meetings 与成员之间的关系。

2. [核心设计：Role Continuity](core-design/role-continuity.md)

   理解 Project 如何把长期 Role、Assignment 任期、Work Responsibility、Commitment、
   Checkpoint、Handoff 与派生 Role Brief 分开，使责任跨越 Agent 和 Runtime 持续存在。

3. [核心设计：先有坐标，后有上下文](core-design/coordinate-and-context.md)

   理解稳定坐标、无向 Edge / Hyperedge 与版本化 Document 为什么采用这种分工，
   以及 Agent 如何从当前工作坐标发现和维护相关上下文。

4. [核心设计：Agent 自主的上下文环境感知 Project Context 图检索](core-design/context-aware-semantic-graph-retrieval.md)

   理解 Agent 如何结合当前 Role 与相关工作环境，在统一 Project Context 图中渐进选择
   不同但相关、可追溯的上下文路径，而无需建立 Agent 私有上下文。

5. [核心设计：Meeting](core-design/meeting.md)

   理解 Human 与 Agent 如何从不同 Role、Work 和项目经历出发，聚合相关上下文、
   形成可行动的共同结论，并把结果显式写回 Project。

6. [系统概览](system-overview.md)

   了解 Desktop、Relay、Managed Agents、`cf` 和本地依赖如何协作，以及身份、权限、
   数据隔离和网络边界。

7. [本地源码开发](local-development.md)

   从一个未运行过 Carryforth 的环境开始，完成依赖检查、Provider 配置、构建、启动、
   重建和停止。

8. [当前状态](current-status.md)

   区分已经实现、需要显式启用、仍在资格化和尚未承诺的能力。

## 产品与治理文档

- [项目定位与目标](../project-positioning.md)
- [项目空间宪章](../project-space-constitution.md)
- [Project View 定义](../stage/project-view/project-view.md)
- [Role Continuity](../stage/role/role-continuity.md)
- [Project Document](../stage/document/document.md)
- [Project Context](../stage/project-context/project-context.md)

`docs/stage/` 中的内容还包括阶段设计、实施方案、缺陷修复和验收记录。
这些文档记录特定时间点的工程事实，不应把其中所有计划项都理解为当前已经开放的产品能力。

## 开发与运维参考

- [`cf` CLI 功能参考](cli-reference.md)
- [系统架构](../../ARCHITECTURE.md)
- [参与贡献](../../CONTRIBUTING.md)
- [测试指南](../../TESTING.md)
- [安全模型与漏洞报告](../../SECURITY.md)
- [项目治理](../../GOVERNANCE.md)
- [语义 pgvector 运维](../semantic-pgvector-operations.md)
- [上游来源与兼容说明](../../UPSTREAM.md)

## 文档边界

- 中文专题文档说明当前产品和源码运行方式，不代替协议、迁移或运维合同；
- 当专题说明与代码、migration、活动运维文档不一致时，应先核对当前实现并修正文档；
- `buzz-*`、`BUZZ_*` 等名称可能是兼容合同，不能仅为统一文案而机械改名；
- 任何密钥、真实私有地址、用户内容或内部基础设施信息都不得写入公开文档。
