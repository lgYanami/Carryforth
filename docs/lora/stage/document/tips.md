# Project Document 开发提示

## 当前仍是未发布的开发阶段

Buzz目前没有已部署的真实环境，没有已发布版本，也没有需要保留的真实Community数据或外部客户端。
因此，Project Document / Project View后续开发默认面向当前最终设计，不把开发过程中的v1、v2、v3
当作已经对外发布的兼容性承诺。

具体执行原则：

- 可以直接调整或替换尚未发布的schema、wire contract、CLI和客户端实现，以得到更简单、正确的最终
  结构；不必仅为假设中的旧部署增加dual read/write、迁移桥接或长期兼容层。
- 未发布不等于不需要真实运行验收。本地启动的真实Relay、PostgreSQL、Redis、Desktop、CLI与ACP
  进程就是有效的真实运行环境；完整签名、权限、maintenance、进程回收、cutover和恢复流程不能只由
  unit test或mock代替。
- 如果当前最终实现仍保留v2 → v3 migration/cutover路径，就必须在本地真实栈上执行并验收这条路径；
  如果确认发布前不需要该路径，则应明确简化或删除，而不是保留一条从未真实运行的复杂兼容路径。
- 不要求生产cohort、真实用户数据、已发布旧客户端或外部部署权限。需要的是可重复的本地真实canary：
  一个有v2状态的本地Community执行cutover，以及一个空Community直接初始化v3。
- 规划文档中的生产rollout、真实数据备份与历史客户端兼容可作为未来发布前的安全参考，不应驱动
  当前不必要的兼容复杂度；但规划中仍属于当前实现的运行路径必须完成本地端到端验收。
- 这不降低当前版本的正确性、安全性、权限边界、事务原子性和测试要求；只是取消对不存在的已发布
  版本与真实数据进行兼容的假设。
- 一旦项目首次发布、部署持久环境、产生不可丢弃的数据或出现外部消费者，必须重新明确版本兼容、
  数据迁移、回滚和canary策略；不能继续沿用本提示中的“无兼容负担”前提。

当本提示与阶段规划冲突时，应区分两类事项：已发布版本兼容和生产rollout属于未来发布准备；当前代码
实际保留的初始化、迁移、maintenance与恢复路径属于当前本地真实运行验收，仍是阶段完成条件。

## 阶段 7 使用单机预发布基线

当前只有一台开发机，因此阶段 7不把生产规模或不存在的真实用户观察作为交付门槛：

- 必做容量数据集为至少100,000条小正文revision，并同时包含hot Document与宽catalog；
- 1,000,000条revision是non-blocking extended soak，只在磁盘preflight通过时运行；
- 不生成100万条上限正文。49,152字节正文只用小规模case验证；
- 生产dashboard替换为可归档的本地JSON / Markdown报告；多节点、HA与分布式压测延期到出现真实
  部署拓扑后；
- 没有真实用户时不要求生产错误率窗口或Adapter usage evidence；阶段 5 / 6 canary在最终代码上各
  重跑一次即可；
- signer rotation、backup / restore、projection parity、权限、安全和Secret incident路径仍必须使用
  独立scratch环境真实运行，不能只做mock；
- 阶段 7完成不授权发布、默认启用v3或broad rollout。首次部署前根据当时磁盘、数据量、拓扑和
  运维责任另立deployment / rollout gate。

容量工具必须先用小样本估算数据库、索引与WAL空间，设置剩余空间熔断，并在退出时精确清理测试
数据库和临时文件。不得为了达到名义行数耗尽唯一开发机。

## 动态管理本地构建产物

本项目的Rust workspace与Desktop Tauri会分别在`target/`和`desktop/src-tauri/target/`产生大量增量编译
目录。连续阶段开发若不清理，磁盘占用会持续增长。

执行原则：

- Agent运行Cargo build / test / clippy时默认设置`CARGO_INCREMENTAL=0`；
- 每个独立测试批次结束后（成功或失败）删除两个target tree下所有名为`incremental`的目录；
- 长时间canary必须在`trap ... EXIT`中清理，不能只在成功路径清理；
- 删除范围必须先固定为仓库内的`target`与`desktop/src-tauri/target`，不得对workspace root、`$HOME`或未解析
  环境变量执行递归删除；
- 仍需保留当前canary复用的Relay / CLI / Admin / ACP二进制和普通依赖产物；只有明确不再复用时才考虑更大范围
  的`cargo clean`。

当前Stage 5与Stage 6真实canary已内置上述策略。手工批次可使用同等的退出清理：

```bash
export CARGO_INCREMENTAL=0
# cargo test / clippy / build ...
find target desktop/src-tauri/target -type d -name incremental -prune -exec rm -rf -- {} +
```
