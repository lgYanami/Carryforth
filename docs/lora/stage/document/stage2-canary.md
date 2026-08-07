# Project Document 阶段 2 canary

执行日期：2026-07-31

最近更新：2026-08-07

范围：隔离的 schema-v3 greenfield Project View Community；不连接生产系统，不使用真实
Secret，也不经过legacy v1/v2普通运行面。

## 自动化 canary

`scripts/test-project-document-e2e.sh` 是可重复执行的首个 canary。它使用独立 PostgreSQL
database、真实 Relay、Redis、`buzz-admin`、`buzz` CLI 和两个独立 test-client 进程，执行：

1. 在名称受限的独立scratch database中显式创建schema-v3、disabled、uninitialized Community，
   保持Document flag off，验证NIP-11、write、REQ、COUNT、HTTP query/count、wildcard/by-ID和
   behind-Relay projection全部fail closed；
2. 使用与Relay projection signer分离的direct Human owner执行
   `prepare-v3 → owner-signed init-v3 → checked enable`；分别验证bootstrap-only discovery、
   initialized-disabled无普通Project View广告，以及enabled后只广告`buzz-project-view-v3`；
3. 在Relay停止时执行Document bootstrap / verify / enable，并验证stable signer与empty catalog；
4. 由 Human 创建普通 Document，通过另一授权连接观察 command + revision + head + meta live
   bundle；验证非成员 HTTP / WS 无法读取；
5. 在订阅存续期间撤销 reader membership，再由另一 Human 更新，验证 final local/Redis fan-out
   不向已撤权连接发送事件；
6. 用真实 CLI 完成 create、metadata-only list、current get、完整 update、zero-fuzz exact patch、
   metadata-only history、pinned get、stale conflict exit 5、delete 和 tombstone 后 pinned read；
7. 运行 synthetic Secret drill：只保留 Document / event 坐标，disable 后验证广告与普通读取
   消失、Relay log 无 fixture 正文，模拟外部轮换与影响评估，经 verify 后 reviewed re-enable；
8. 最后再次 disable，比较前后 revision row count，并检查控制面 audit chain。

脚本不会接收或复用现有database名称；主库和任何不匹配`buzz_pd_e2e_<pid>_<random>`（恢复副本
额外带`_restore`）的database都不在清理范围。schema-v3初始化失败时canary直接失败，不会恢复
旧CLI、旧capability或dual-write作为兼容回退。

managed Agent 的 active Assignment / Runtime fence 使用同一生产 DB coordinator，而不是 canary
专用旁路：managed owner membership、active Assignment 和 supervised runtime epoch 都在 receipt
lookup 前与 commit 前重新验证；stale epoch 的 focused DB test 必须与本 canary 一起通过。ACP base
prompt 只公布 metadata-first 的 `documents list/get/history`，不会自动遍历正文，也没有 Resource
Guide / Context 的提前声明。

## 通过条件与证据

- `just project-document-test-unit`：pure / SDK / Relay / CLI / ACP / admin focused tests；
- `just project-document-test-db`：atomicity、receipt replay、runtime fence、same-Document race、
  immutable history 与 signer parity；
- `just project-document-test-e2e`：上述真实disabled/greenfield-v3/enabled/incident canary；
- `just test-migrations`：migration / desired schema drift；
- `just ci` 与 `just test`：仓库级质量门。

执行输出不得保存 Document 正文；提交记录只写通过 / 失败和测试命令。任何一步失败都视为
canary 未通过，Community 必须保持 disabled。
