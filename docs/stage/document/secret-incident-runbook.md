# Project Document Secret incident runbook

本 runbook 适用于疑似 Secret、credential、private key、token 或必须限制传播的数据被写入
Project Document 的情况。Document 的 tombstone 和历史 revision 都不是擦除机制；处置重点是
立即停止普通协议访问、在事实源轮换或撤销 Secret，并只使用坐标完成调查。

## 触发与权限

任何成员发现疑似泄露都应立即通知 Community operator / incident owner，不要在消息、工单、
日志或通知中复制正文。处置者需要数据库连接、稳定 Relay signer 的受限 key file，以及外部
Secret Manager / credential issuer 的轮换权限。Relay key file 必须是普通文件，Unix 权限不得
向 group / world 开放。

允许在处置记录中出现的 Buzz 坐标只有：

- Community host / ID；
- Document ID 与 revision；
- command、head、revision、meta event ID；
- audit sequence / incident ticket ID；
- 时间、处置人和不含正文的状态结论。

禁止记录 title、summary、Markdown、locator、疑似 Secret 的片段、hash 或“打码后”的值。

## 立即处置

1. 立即关闭该 Community 的能力：

   ```bash
   DATABASE_URL="${DATABASE_URL:?}" \
     buzz-admin project-document disable --community "${COMMUNITY_HOST:?}"
   ```

2. 用 `buzz-admin project-document status --community "$COMMUNITY_HOST"` 确认 `enabled=false`；
   用 NIP-11 确认 `buzz-project-document-v1` 已消失；确认普通成员的 list / get / history、REQ、
   COUNT、`/query`、`/count` 与 live fan-out 全部 fail closed。
3. 立即在外部事实源轮换或撤销 credential。不要等待 Buzz 内的 delete、修订或调查完成。
4. 记录上面的坐标和 audit coordinate，不复制内容。若需要 forensic access，使用组织另行
   审计、最小授权的 operator procedure；普通 Document reader 没有绕过开关的后门。

## 暴露面评估

Incident owner 至少评估以下位置，并按组织流程通知 / 升级：

- Agent workspace、tool output、prompt transcript 和本地文件；
- CLI stdout、shell history、重定向文件和 CI artifact；
- Desktop / future client cache、剪贴板与导出；
- Relay / proxy logs、metrics、traces、audit detail 和 alert payload；
- PostgreSQL、备份、快照、只读副本和已下载副本；
- 任何取得过该 credential 的外部系统。

正常遥测只允许 body byte count、operation、revision、event / actor coordinate 和低基数结果，
不得记录 Document 内容。若在日志中发现正文，保持 capability disabled，并把日志系统本身纳入
事件范围。

## 删除、恢复与重新启用

- 不把 `documents delete` 或数据库 hard delete 当作擦除：delete 只追加 bodyless tombstone，
  旧 revision 仍是不可变历史；生产表的 hard-delete / rewrite guard 不得绕过。
- 若泄露的是可失效 credential，只有在外部轮换 / 撤销完成、暴露面评估完成、通知要求满足，
  且 incident owner 明确确认残留历史可以重新向成员开放后，才可进入 reviewed re-enable。
- 若数据本身必须擦除而不能通过失效消除风险，保持 disabled，等待独立 scrub / recovery 设计；
  v1 不提供 scrub。
- 重新启用前运行：

  ```bash
  DATABASE_URL="${DATABASE_URL:?}" \
    buzz-admin project-document verify \
      --community "${COMMUNITY_HOST:?}" \
      --expected-pubkey "${EXPECTED_RELAY_PUBKEY:?}"

  DATABASE_URL="${DATABASE_URL:?}" \
    buzz-admin project-document enable \
      --community "${COMMUNITY_HOST:?}" \
      --relay-key-file "${RELAY_KEY_FILE:?}" \
      --expected-pubkey "${EXPECTED_RELAY_PUBKEY:?}"
  ```

  `verify` 必须确认 schema、ready 的 Project View v3、bootstrap、stable signer 和全部 canonical /
  projection pointer parity。重新启用后只用坐标验证 current / pinned revision；不要把正文复制到
  incident 记录。

## 演练

隔离演练入口是：

```bash
just project-document-test-e2e
```

脚本只创建文字明确标注为 synthetic、且不包含真实 credential 的 fixture，随后执行
`disable → protocol fail closed → coordinate-only assess → simulated external rotation → verify →
reviewed enable`，检查 fixture 正文没有进入 Relay log，最后再次 disable 并证明 canonical history
行数不变。bootstrap、enable、disable 都必须形成 `project_document_control` audit 记录。

演练失败时不得在真实 Community enable；保留隔离测试的坐标与无正文日志，修复后从头重跑。
