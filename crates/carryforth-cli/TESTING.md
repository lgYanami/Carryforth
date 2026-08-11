# carryforth-cli Live Testing Guide

Manual testing runbook for verifying every CLI command against a local relay.
An agent or developer follows this step by step, running each command and
checking the output.

---

## 1. Prerequisites

Docker services running and healthy:

```bash
docker compose ps
# buzz-postgres   healthy
# buzz-redis      healthy
```

If not running: `just setup` from the repo root.

Tools: `jq`, `curl`, Rust toolchain.

---

## 2. Build the CLI

```bash
cargo build -p carryforth-cli
```

Use `cargo run -p carryforth-cli --` or the built binary at `target/debug/cf`.

---

## 3. Start the Relay

From the Carryforth repository root, in a separate terminal:

```bash
set -a && source .env && set +a
cargo run -p buzz-relay
```

Verify:

```bash
curl -s http://localhost:3000/_liveness
# "ok" or 200 status
```

The `.env` should have `BUZZ_REQUIRE_AUTH_TOKEN=false` for local dev.

---

## 4. Mint Test Credentials

### Option A: buzz-admin (full scopes including admin)

This mints a token with all CLI-relevant scopes (including `admin:channels`)
via direct DB access. Use this for testing admin operations (archive,
delete-channel, add/remove-channel-member).

```bash
DATABASE_URL="${DATABASE_URL:?set DATABASE_URL for the local Carryforth database}" \
cargo run -p buzz-admin -- mint-token \
  --name "cli-test" \
  --scopes "messages:read,messages:write,channels:read,channels:write,users:read,users:write,files:read,files:write,admin:channels"
```

This generates a keypair and prints:
- **Private key (nsec)** — save for `CARRYFORTH_PRIVATE_KEY` testing

Export:

```bash
export CARRYFORTH_RELAY_URL="http://localhost:3000"
export CARRYFORTH_PRIVATE_KEY="nsec1..."   # from the mint output
```

### Scope reference

| Scope | Self-mintable | Needed for |
|-------|:---:|------------|
| `messages:read` | ✅ | `messages get`, `messages thread`, `messages search`, `feed get` |
| `messages:write` | ✅ | `messages send`, `messages edit`, `messages delete`, `reactions`, `messages vote` |
| `channels:read` | ✅ | `channels list`, `channels get`, `channels members` |
| `channels:write` | ✅ | `channels create`, `channels update`, `channels join`, `channels leave`, `channels topic`, `channels purpose` |
| `users:read` | ✅ | `users get`, `users presence` |
| `users:write` | ✅ | `users set-profile`, `users set-presence` |
| `files:read` | ✅ | — |
| `files:write` | ✅ | — |
| `admin:channels` | ❌ | `channels archive`, `channels unarchive`, `channels delete`, `channels add-member`, `channels remove-member` |

---

## 5. Unit Tests

```bash
cargo test -p carryforth-cli
# Expected: see cargo test -p carryforth-cli for current count

cargo clippy -p carryforth-cli -- -D warnings
# Expected: zero warnings
```

---

## 6. Live Testing — Command by Command

Run each command, verify exit code 0 and check output. Most commands
return JSON (pipe through `jq .` to validate). Commands are ordered so
earlier ones create resources that later ones need.

### 6.1 Channels

```bash
# channels create (stream)
cf channels create --name "test-stream" --type stream --visibility open \
  --description "CLI test channel" | jq .
# Save the channel ID:
CHANNEL_ID=$(cf channels create --name "test-cli" --type stream --visibility open | jq -r '.channel_id')
# Expected: {"event_id":"...","accepted":true,"message":"...","channel_id":"<uuid>"}

# channels create (forum) — needed for messages vote later
FORUM_ID=$(cf channels create --name "test-forum" --type forum --visibility open | jq -r '.channel_id')

# channels list
cf channels list | jq .
# Expected: [{"channel_id":"...","name":"...","description":"...","created_at":N}]
cf channels list --visibility open | jq .
cf channels list --member | jq .

# channels get
cf channels get --channel "$CHANNEL_ID" | jq .
# Expected: {"channel_id":"...","name":"...","description":"...","created_at":N,"pubkey":"..."} or null

# channels update
cf channels update --channel "$CHANNEL_ID" --name "test-cli-updated" \
  --description "Updated" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels topic
cf channels topic --channel "$CHANNEL_ID" --topic "Test topic" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels purpose
cf channels purpose --channel "$CHANNEL_ID" --purpose "Testing" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels join (may already be a member from create)
cf channels join --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels leave
# NOTE: Fails with 400 "cannot remove the last owner" if this identity is the
# sole owner (which it is after channels create). To test leave successfully,
# first add-member a second pubkey as owner. The relay enforces ≥1 owner.
cf channels leave --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."} (or 400 if last owner)

# Re-join so we can send messages
cf channels join --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels archive (requires admin:channels scope)
cf channels archive --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}

# channels unarchive
cf channels unarchive --channel "$CHANNEL_ID" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"..."}
```

### 6.2 Canvas

```bash
# canvas set
cf canvas set --channel "$CHANNEL_ID" --content "# Test Canvas" | jq .

# canvas set from stdin
echo "# Canvas from stdin" | cf canvas set --channel "$CHANNEL_ID" --content - | jq .

# canvas get
cf canvas get --channel "$CHANNEL_ID"
# Expected: raw markdown string, or: null
```

### 6.3 Messages

```bash
# messages send
MSG=$(cf messages send --channel "$CHANNEL_ID" --content "Hello from CLI test" | jq .)
echo "$MSG"
EVENT_ID=$(echo "$MSG" | jq -r '.event_id')

# messages send with reply + broadcast
REPLY=$(cf messages send --channel "$CHANNEL_ID" --content "Reply" \
  --reply-to "$EVENT_ID" --broadcast | jq .)
echo "$REPLY"
REPLY_ID=$(echo "$REPLY" | jq -r '.event_id')

# messages send with mentions — @name in content is auto-resolved, no flag needed
cf messages send --channel "$CHANNEL_ID" --content "Hey @someone" | jq .

# messages send with NIP-27 nostr:npub1… inline mention — auto-resolved to p-tag
cf messages send --channel "$CHANNEL_ID" \
  --content "Check with nostr:npub10elfcs4fr0l0r8af98jlmgdh9c8tcxjvz9qkw038js35mp4dma8qzvjptg on this" | jq .

# messages send from stdin — safe path for content with shell metacharacters
# (backticks, $vars, code blocks) that would otherwise be expanded by the shell.
echo 'Body with `backticks` and $vars stays literal.' \
  | cf messages send --channel "$CHANNEL_ID" --content - | jq .

# messages get
cf messages get --channel "$CHANNEL_ID" | jq .
cf messages get --channel "$CHANNEL_ID" --limit 5 | jq .

# messages thread
cf messages thread --channel "$CHANNEL_ID" --event "$EVENT_ID" | jq .

# messages search
cf messages search --query "Hello" | jq .
cf messages search --query "CLI test" --limit 5 | jq .

# messages edit
cf messages edit --event "$EVENT_ID" --content "Edited by CLI test" | jq .

# messages delete
cf messages delete --event "$REPLY_ID" | jq .
```

### 6.4 Diff Messages

```bash
# messages send-diff from stdin
echo '--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,3 @@
-fn old() {}
+fn new() {}' | cf messages send-diff \
  --channel "$CHANNEL_ID" \
  --diff - \
  --repo "https://github.com/example/repo" \
  --commit "abcdef1234567890abcdef1234567890abcdef12" | jq .

# messages send-diff with metadata
echo "diff content" | cf messages send-diff \
  --channel "$CHANNEL_ID" \
  --diff - \
  --repo "https://github.com/example/repo" \
  --commit "abcdef1234567890abcdef1234567890abcdef12" \
  --file "src/main.rs" \
  --lang "rust" \
  --description "Refactored main" | jq .

# messages send-diff with branch + PR metadata
echo "diff content" | cf messages send-diff \
  --channel "$CHANNEL_ID" \
  --diff - \
  --repo "https://github.com/example/repo" \
  --commit "abcdef1234567890abcdef1234567890abcdef12" \
  --parent-commit "1234567890abcdef1234567890abcdef12345678" \
  --source-branch "feature/cli" \
  --target-branch "main" \
  --pr 42 | jq .
```

### 6.5 Reactions

```bash
# Send a message to react to
REACT_MSG=$(cf messages send --channel "$CHANNEL_ID" --content "React to this")
REACT_ID=$(echo "$REACT_MSG" | jq -r '.event_id')

# reactions add
cf reactions add --event "$REACT_ID" --emoji "👍" | jq .

# reactions get
cf reactions get --event "$REACT_ID" | jq .
# Expected: {"reactions":[{"emoji":"...","count":N,"pubkeys":["..."]}]}

# reactions remove
cf reactions remove --event "$REACT_ID" --emoji "👍" | jq .
```

### 6.6 DMs

```bash
# dms list
cf dms list | jq .
# Expected: [{"dm_id":"...","participants":["..."],"created_at":N}]

# dms open (needs a real pubkey — use your own or a test one)
# Get your own pubkey first:
MY_PUBKEY=$(cf users get | jq -r '.[0].pubkey // empty')
echo "My pubkey: $MY_PUBKEY"

# dms open with a synthetic pubkey (relay will create the user)
DM_RESULT=$(cf dms open --pubkey "0000000000000000000000000000000000000000000000000000000000000001")
echo "$DM_RESULT" | jq .
# Expected: {"event_id":"...","accepted":true,"message":"...","dm_id":"<uuid>"}
DM_ID=$(echo "$DM_RESULT" | jq -r '.dm_id')

# dms add-member (requires messages:write scope — NOT admin:channels)
cf dms add-member --channel "$DM_ID" \
  --pubkey "0000000000000000000000000000000000000000000000000000000000000002" | jq .
```

### 6.7 Users & Presence

```bash
# users get — own profile (0 pubkeys)
cf users get | jq .
# Expected: [{...profile...}] — always returns an array, even for single results

# users get — single pubkey
cf users get --pubkey "$MY_PUBKEY" | jq .

# users get — batch (2+ pubkeys)
cf users get --pubkey "$MY_PUBKEY" --pubkey "$MY_PUBKEY" | jq .

# users set-profile
cf users set-profile --name "CLI Test Agent" --about "Testing carryforth-cli" | jq .

# users presence
cf users presence --pubkeys "$MY_PUBKEY" | jq .

# users set-presence
cf users set-presence --status online | jq .
cf users set-presence --status away | jq .
cf users set-presence --status offline | jq .
# Note: set-presence may fail — kind:20001 is ephemeral and rejected by the HTTP bridge
```

### 6.8 Channel Members (add/remove require admin:channels)

```bash
# channels add-member
cf channels add-member --channel "$CHANNEL_ID" \
  --pubkey "0000000000000000000000000000000000000000000000000000000000000001" \
  --role member | jq .

# channels members
cf channels members --channel "$CHANNEL_ID" | jq .
# Expected: [{"pubkey":"...","role":"..."}]

# channels remove-member
cf channels remove-member --channel "$CHANNEL_ID" \
  --pubkey "0000000000000000000000000000000000000000000000000000000000000001" | jq .
```

### 6.9 Workflows

```bash
# workflows create
# NOTE: trigger uses `on:` tag (serde internally tagged enum).
# Valid triggers: message_posted, reaction_added, diff_posted, schedule, webhook
# Steps use `action:` tag: send_message, send_dm, set_channel_topic, add_reaction, etc.
WF=$(cf workflows create --channel "$CHANNEL_ID" \
  --yaml 'name: test-wf
trigger:
  on: webhook
steps:
  - id: step1
    action: send_message
    text: "Hello from workflow"' | jq .)
echo "$WF"
WF_ID=$(echo "$WF" | jq -r '.workflow_id')

# workflows list
cf workflows list --channel "$CHANNEL_ID" | jq .

# workflows get
cf workflows get --workflow "$WF_ID" | jq .
# Expected: {"workflow_id":"...","content":"<yaml>","created_at":N,"pubkey":"..."} or null

# workflows update (requires --channel)
cf workflows update --channel "$CHANNEL_ID" --workflow "$WF_ID" \
  --yaml 'name: test-wf-updated
trigger:
  on: webhook
steps:
  - id: step1
    action: send_message
    text: "Updated"' | jq .

# workflows trigger
# NOTE: May return 400 "workflow not found" — the relay indexes workflow
# definitions into a DB table asynchronously. If the definition event hasn't
# been indexed yet, the trigger handler won't find it.
cf workflows trigger --workflow "$WF_ID" | jq .

# workflows runs
cf workflows runs --workflow "$WF_ID" | jq .
# Expected: [] — relay stores runs in DB, not as Nostr events; empty is normal

# workflows approve — requires a workflow run waiting for approval
# This is hard to test ad-hoc without a workflow that has an approval gate.
# Test the validation instead:
cf workflows approve --token "00000000-0000-0000-0000-000000000000" 2>&1 || true
# Should fail with relay error (token not found), not a validation error
# To test the deny path: cf workflows approve --token <UUID> --approved false

# workflows delete
cf workflows delete --workflow "$WF_ID" | jq .
```

### 6.10 Feed

```bash
cf feed get | jq .
cf feed get --limit 5 | jq .
# Expected: [{id,pubkey,kind,content,created_at,tags}] — sig-stripped, sorted newest-first
```

### 6.11 Forum & Voting

```bash
# Send a forum post (kind 45001) to the forum channel
FORUM_POST=$(cf messages send --channel "$FORUM_ID" \
  --content "Forum post for vote testing" --kind 45001 | jq .)
echo "$FORUM_POST"
FORUM_EVENT_ID=$(echo "$FORUM_POST" | jq -r '.event_id')

# messages vote (up)
cf messages vote --event "$FORUM_EVENT_ID" --direction up | jq .

# messages vote (down)
cf messages vote --event "$FORUM_EVENT_ID" --direction down | jq .
```

### 6.12 Notes (NIP-23 long-form, kind:30023)

Editable team-knowledge notes keyed by `(kind:30023, you, d=slug)`. `set` is an
idempotent upsert; `rm` is a NIP-09 a-tag deletion. Output is plain text (refs),
not JSON — except `get`/`ls`, which emit JSON.

```bash
# set (first publish — --title required, body from stdin)
cat <<'EOF' | cf notes set --name dco-check --title "DCO Check" \
  --summary "How we verify DCO" --tag dco --tag ci --content -
Run `git log --format='%(trailers:key=Signed-off-by)'` ...
EOF
# → prints event_id / naddr / coordinate / slug / title

# set (edit — omit --title to carry it forward; published_at preserved)
echo "Updated body." | cf notes set --name dco-check --content -

# get by name (own author resolves directly; cross-author #d query otherwise)
cf notes get --name dco-check | jq .
cf notes get --name dco-check --content-only

# get by naddr (exact coordinate; paste the naddr from a set/get above)
cf notes get --naddr "$NADDR" | jq .

# ls (own by default; --author all across the team; --tag filters)
cf notes ls | jq .
cf notes ls --tag dco | jq .
cf notes ls --author all --limit 10 | jq .

# rm (NIP-09 a-tag deletion; subsequent get must 404)
cf notes rm --name dco-check
# → prints deleted <coordinate> / deletion <event-id>
cf notes get --name dco-check   # exits non-zero: not found

# rm of a slug you never published → NotFound, no kind:5 emitted
cf notes rm --name does-not-exist   # exits non-zero
```

### 6.13 Project Documents

Project Document 必须先由 operator 在已初始化、strict-ready 的 Project View v3 Community
上 bootstrap、verify 并 enable；Relay 需要稳定 signer。普通 CLI 不再把 schema v2 当作
可运行的 Document 治理前置条件。不要把 Secret、token 或 private key 写入测试正文。

```bash
export COMMUNITY_HOST="localhost:3000"
export RELAY_PUBKEY="$(curl -fsS "http://${COMMUNITY_HOST}/info" | jq -er '.self')"

DATABASE_URL="${DATABASE_URL:?}" buzz-admin project-document bootstrap \
  --community "$COMMUNITY_HOST" --expected-pubkey "$RELAY_PUBKEY"
DATABASE_URL="${DATABASE_URL:?}" buzz-admin project-document verify \
  --community "$COMMUNITY_HOST" --expected-pubkey "$RELAY_PUBKEY"
DATABASE_URL="${DATABASE_URL:?}" buzz-admin project-document enable \
  --community "$COMMUNITY_HOST" --expected-pubkey "$RELAY_PUBKEY"

# create performs receipt validation plus exact immutable-revision read-back
CREATE=$(cf documents create --title "CLI runbook" \
  --summary "safe test fixture" --content "# Runbook" | jq .)
DOCUMENT_ID=$(jq -er '.document_id' <<<"$CREATE")
jq -e '.accepted and .document_revision == 1' <<<"$CREATE"

# list/history are metadata-only; body is fetched explicitly
cf documents list | jq .
cf documents get "$DOCUMENT_ID" | jq .
cf documents get "$DOCUMENT_ID" --content-only
cf --format compact documents history "$DOCUMENT_ID" | jq .

cf documents update "$DOCUMENT_ID" --expected-revision 1 \
  --title "CLI runbook v2" --clear-summary --content $'# Runbook\n\nRevision 2' | jq .

# Patch must match revision 2 at its declared line positions: no fuzz, offset,
# automatic rebase, or silent temporary output.
cf documents patch "$DOCUMENT_ID" --expected-revision 2 \
  --patch-file ./document.patch --output ./merged.md | jq .

# A stale expected revision is conflict exit 5 and does not discard local input.
cf documents update "$DOCUMENT_ID" --expected-revision 2 \
  --title stale --clear-summary --content "must not commit"; test "$?" -eq 5

cf documents get "$DOCUMENT_ID" --revision 1 | jq .
cf documents delete "$DOCUMENT_ID" --expected-revision 3 | jq .
cf documents history "$DOCUMENT_ID" | jq .
cf documents get "$DOCUMENT_ID" --revision 1 | jq .
```

`--format compact` 是 global flag，必须放在 `documents` 之前。普通 Document CRUD 使用
Community member identity，不把 Role Assignment 或 Runtime fence 当作 Document ACL；调用者也
不得手工伪造这些归因字段。运输结果不明确时，CLI 只在 exact revision 的 `source_event_id`
证明同一 signed command 已提交后报告成功；否则返回 exit 2
`delivery_unknown`，调用者不得自动重签重发。

### 6.14 Project Context semantic query

The command is available only when NIP-11 advertises
`buzz-project-context-semantic-query-http` and a canonical Relay `self` key.
It resolves the current Project identity from the verified Project View v3
snapshot before sending the query.

```bash
cf project-context semantic-query \
  --problem "why did this release incident recur?" | jq .

cf --format compact project-context semantic-query \
  --problem "which work explains this requirement?" \
  --initial-coordinate "requirement:${REQUIREMENT_ID}" \
  --context-coordinate "work:${WORK_ID}" \
  --lifecycle non-terminal \
  --max-semantic-roots 4 \
  --max-hops-per-path 2 | jq -c .
```

The unversioned output wrapper is `{result,read_commands}`. `result` is the
unchanged SDK-verified, Relay-signed closed result DTO. `read_commands` is an
unsigned deterministic convenience projection derived from returned canonical
identities; it is sorted and deduplicated and must not be treated as signed
result content. `--format compact` changes JSON whitespace only.

A semantic query can incur Provider cost. The CLI serializes and NIP-98-signs
the strict single-filter body once, sends exactly one HTTP attempt with a
45-second total timeout, and never enters the ordinary `/query` retry loop.
Timeout, body loss, 429, 502, 503, or 504 therefore returns an error without an
automatic replay. A user-initiated rerun creates a new request UUID and auth
Event.

---

## 7. Error Path Testing

Verify the CLI produces correct JSON on stderr and correct exit codes.

```bash
# Exit 1: Invalid UUID
cf channels get --channel "not-a-uuid" 2>&1; echo "exit: $?"
# stderr: {"error":"user_error","message":"invalid UUID: not-a-uuid"}
# exit: 1

# Exit 1: Invalid hex64
cf messages delete --event "not-hex" 2>&1; echo "exit: $?"
# stderr: {"error":"user_error","message":"must be a 64-character hex string: not-hex"}
# exit: 1

# Exit 1: Invalid --type value (clap validates the enum — multi-line error)
cf channels create --name x --type invalid --visibility open 2>&1; echo "exit: $?"
# stderr: {"error":"user_error","message":"error: invalid value 'invalid' for '--type <CHANNEL_TYPE>'\n  [possible values: stream, forum]\n..."}
# exit: 1

# Exit 1: Invalid --direction value
cf messages vote --event "$(printf '0%.0s' {1..64})" \
  --direction sideways 2>&1; echo "exit: $?"
# exit: 1

# Exit 1: Empty body guard
cf users set-profile 2>&1; echo "exit: $?"
# exit: 1 (at least one field required)

# Exit 3: No auth configured
env -u CARRYFORTH_PRIVATE_KEY \
  cargo run -p carryforth-cli -- channels list 2>&1; echo "exit: $?"
# stderr: {"error":"auth_error","message":"auth error: CARRYFORTH_PRIVATE_KEY is required (use --private-key or set env var)"}
# exit: 3

# Not-found returns null, not an error (exit 0)
cf channels get --channel "00000000-0000-0000-0000-000000000000"
# stdout: null
# exit: 0
```

---

## 8. Auth Testing

Test authentication.

```bash
# Private key (CARRYFORTH_PRIVATE_KEY)
CARRYFORTH_PRIVATE_KEY="nsec1..." cf channels list | jq .
# Should succeed

# No auth → exit 3
env -u CARRYFORTH_PRIVATE_KEY \
  cargo run -p carryforth-cli -- channels list 2>&1; echo "exit: $?"
# stderr: {"error":"auth_error","message":"auth error: CARRYFORTH_PRIVATE_KEY is required (use --private-key or set env var)"}
# exit: 3
```

---

## 9. Cleanup

```bash
# Delete test channels
cf channels delete --channel "$CHANNEL_ID" | jq .
cf channels delete --channel "$FORUM_ID" | jq .
```

---

## 10. Checklist

| # | Command | Tested | Notes |
|---|---------|:------:|-------|
| 1 | `messages send` | ☐ | Basic, reply, broadcast, mentions, stdin |
| 2 | `messages send-diff` | ☐ | Stdin, metadata, branch/PR |
| 3 | `messages edit` | ☐ | |
| 4 | `messages delete` | ☐ | |
| 5 | `messages get` | ☐ | With limit |
| 6 | `messages thread` | ☐ | |
| 7 | `messages search` | ☐ | With limit |
| 8 | `messages vote` | ☐ | Up and down |
| 9 | `channels list` | ☐ | With visibility, member |
| 10 | `channels get` | ☐ | |
| 11 | `channels create` | ☐ | Stream and forum |
| 12 | `channels update` | ☐ | |
| 13 | `channels topic` | ☐ | |
| 14 | `channels purpose` | ☐ | |
| 15 | `channels join` | ☐ | |
| 16 | `channels leave` | ☐ | |
| 17 | `channels archive` | ☐ | Needs admin:channels |
| 18 | `channels unarchive` | ☐ | Needs admin:channels |
| 19 | `channels delete` | ☐ | Needs admin:channels |
| 20 | `channels members` | ☐ | |
| 21 | `channels add-member` | ☐ | Needs admin:channels |
| 22 | `channels remove-member` | ☐ | Needs admin:channels |
| 23 | `canvas get` | ☐ | |
| 24 | `canvas set` | ☐ | Direct and stdin |
| 25 | `reactions add` | ☐ | |
| 26 | `reactions remove` | ☐ | |
| 27 | `reactions get` | ☐ | |
| 28 | `dms list` | ☐ | |
| 29 | `dms open` | ☐ | |
| 30 | `dms add-member` | ☐ | Needs messages:write |
| 31 | `users get` | ☐ | Self, single, batch |
| 32 | `users set-profile` | ☐ | |
| 33 | `users presence` | ☐ | |
| 34 | `users set-presence` | ☐ | online, away, offline |
| 35 | `workflows list` | ☐ | |
| 36 | `workflows create` | ☐ | |
| 37 | `workflows update` | ☐ | |
| 38 | `workflows delete` | ☐ | |
| 39 | `workflows trigger` | ☐ | |
| 40 | `workflows runs` | ☐ | |
| 41 | `workflows get` | ☐ | |
| 42 | `workflows approve` | ☐ | Validation only (needs approval gate); bare = approve, `--approved false` = deny |
| 43 | `feed get` | ☐ | |
| 44 | `social publish` | ☐ | |
| 45 | `social set-contacts` | ☐ | |
| 46 | `social event` | ☐ | |
| 47 | `social notes` | ☐ | |
| 48 | `social contacts` | ☐ | |
| 49 | `repos create` | ☐ | |
| 50 | `repos get` | ☐ | |
| 51 | `repos list` | ☐ | |
| 52 | `repos protect list` | ☐ | Empty/populated rules; unknown rules visible; malformed rule reported in validation_error |
| 53 | `repos protect set` | ☐ | Create and replace complete exact-ref rule; verify metadata is preserved |
| 54 | `repos protect remove` | ☐ | Remove exact ref; missing rule → NotFound |
| 55 | `upload file` | ☐ | |
| 56 | `pack validate` | ☐ | Local, no relay |
| 57 | `pack inspect` | ☐ | Local, no relay |
| 58 | `notes set` | ☐ | First publish, edit/carry, --clear-tags, ambiguity, empty-stdin guard |
| 59 | `notes get` | ☐ | By name, by naddr, --content-only, cross-author, ambiguous → exit 1 |
| 60 | `notes ls` | ☐ | Own, --author all, --tag, --limit |
| 61 | `notes rm` | ☐ | Delete→get 404, double-delete idempotent, missing slug → NotFound |
| 62 | `documents list` | ☐ | Metadata only; bounded catalog snapshot |
| 63 | `documents get` | ☐ | Current, pinned, and content-only |
| 64 | `documents history` | ☐ | Metadata only; complete immutable history |
| 65 | `documents create` | ☐ | Receipt + exact read-back |
| 66 | `documents update` | ☐ | Complete snapshot; stale revision exits 5 |
| 67 | `documents patch` | ☐ | Exact position, zero fuzz/offset |
| 68 | `documents delete` | ☐ | Tombstone plus pinned historical read |
