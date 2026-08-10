pub mod agent_management;
mod client;
mod commands;
mod error;
mod validate;

use clap::{Parser, Subcommand};
use client::CarryforthClient;
use error::CliError;
use nostr::Keys;
use uuid::Uuid;

/// Run the Carryforth CLI from raw arguments (including `argv[0]`).
///
/// Returns a process exit code (0 = success).
///
/// # Example
///
/// ```ignore
/// let code = carryforth_cli::run_from_args(std::env::args()).await;
/// std::process::exit(code);
/// ```
pub async fn run_from_args<I, S>(args: I) -> i32
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    // Install ring as the process-level rustls CryptoProvider. Required because the
    // release workflow builds all binaries in one cargo invocation, which unifies
    // features across the workspace and enables *both* ring (from buzz-acp/buzz-dev-mcp)
    // and aws-lc-rs (from reqwest's rustls feature via hyper-rustls). With both on,
    // rustls cannot auto-select a provider, and any code that reaches
    // ClientConfig::builder() — specifically the WSS path in publish_ephemeral_event
    // used by `agents draft-create`, `agents draft-update`, and `users set-presence`
    // — panics at rustls crypto/mod.rs. The `let _ =` swallow is intentional: when
    // buzz-dev-mcp delegates to run_from_args, it has already installed ring; the
    // double-install returns Err and is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            if e.use_stderr() {
                error::print_error(&CliError::Usage(e.to_string()));
                return 1;
            } else {
                // --help and --version: print normally (intentional human output)
                let _ = e.print();
                return 0;
            }
        }
    };
    match run(cli).await {
        Ok(()) => 0,
        Err(e) => {
            error::print_error(&e);
            error::exit_code(&e)
        }
    }
}

#[derive(Parser)]
#[command(
    name = "cf",
    about = "Carryforth CLI — interact with a Carryforth relay",
    long_about = "\
Carryforth CLI — interact with a Carryforth relay

Configuration (flags override env vars):
  CARRYFORTH_RELAY_URL     Relay base URL        [default: http://localhost:3000]
  CARRYFORTH_PRIVATE_KEY   Nostr private key (hex or nsec)  [required]
  CARRYFORTH_AUTH_TAG      NIP-OA auth tag JSON  [optional]

The 'pack' subcommand runs locally and does not require a relay connection.

Exit codes: 0=ok  1=bad input  2=relay/network error  3=auth error  4=other  5=write conflict
Errors are JSON on stderr: {\"error\": \"<category>\", \"message\": \"<detail>\"}"
)]
struct Cli {
    /// Relay URL (http:// or https://). Overrides CARRYFORTH_RELAY_URL env var.
    #[arg(
        long,
        env = "CARRYFORTH_RELAY_URL",
        hide_env_values = true,
        default_value = "http://localhost:3000"
    )]
    relay: String,

    /// Nostr private key (hex or nsec). This is the CLI's identity.
    #[arg(long, env = "CARRYFORTH_PRIVATE_KEY", hide_env_values = true)]
    private_key: Option<String>,

    /// NIP-OA auth tag JSON (owner attestation). Injected into every signed event.
    #[arg(long, env = "CARRYFORTH_AUTH_TAG", hide_env_values = true)]
    auth_tag: Option<String>,

    /// Output format: 'json' (default, full fields) or 'compact' (reduced fields).
    #[arg(long, value_enum, default_value = "json")]
    format: OutputFormat,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Clone, clap::ValueEnum)]
pub enum ChannelType {
    #[value(name = "stream")]
    Stream,
    #[value(name = "forum")]
    Forum,
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stream => write!(f, "stream"),
            Self::Forum => write!(f, "forum"),
        }
    }
}

#[derive(Clone, clap::ValueEnum)]
pub enum ChannelVisibility {
    #[value(name = "open")]
    Open,
    #[value(name = "private")]
    Private,
}

impl std::fmt::Display for ChannelVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Private => write!(f, "private"),
        }
    }
}

#[derive(Clone, clap::ValueEnum)]
pub enum PresenceStatus {
    #[value(name = "online")]
    Online,
    #[value(name = "away")]
    Away,
    #[value(name = "offline")]
    Offline,
}

#[derive(Clone, clap::ValueEnum)]
pub enum EmojiScope {
    #[value(name = "own")]
    Own,
    #[value(name = "workspace")]
    Workspace,
}

impl std::fmt::Display for PresenceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Away => write!(f, "away"),
            Self::Offline => write!(f, "offline"),
        }
    }
}

/// Output format for read commands.
#[derive(Clone, clap::ValueEnum, Default)]
pub enum OutputFormat {
    /// Full normalized JSON (default)
    #[default]
    #[value(name = "json")]
    Json,
    /// Reduced fields for agent scanning
    #[value(name = "compact")]
    Compact,
}

/// Meeting floor-allocation protocol selected at creation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, Default)]
pub enum MeetingPolicy {
    /// Meeting V0 uniform claim arbitration.
    #[value(name = "uniform-v0")]
    UniformV0,
    /// Meeting V1 moderator-controlled baton passing.
    #[value(name = "moderated-baton-v1")]
    ModeratedBatonV1,
    /// Meeting V2 moderator-maintained shared board.
    #[value(name = "moderated-board-v1")]
    ModeratedBoardV2,
    /// Meeting V2 with direct moderator action finalization before normal close.
    #[default]
    #[value(name = "moderated-board-actions-v3")]
    ModeratedBoardActionsV3,
}

/// Meeting V1 moderator intent-rejection reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MeetingIntentRejectionReason {
    /// The proposed contribution is outside the meeting topic.
    #[value(name = "off_topic")]
    OffTopic,
    /// The proposed contribution duplicates another one.
    Duplicate,
    /// A newer contribution replaces this one.
    Superseded,
    /// The contribution cannot be supported in this meeting.
    Unsupported,
    /// The contribution does not fit the current agenda.
    #[value(name = "agenda_mismatch")]
    AgendaMismatch,
}

/// Meeting V1 moderator handoff-dismissal reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MeetingHandoffDismissReason {
    /// A newer question or request replaces this one.
    Superseded,
    /// The question was answered through another path.
    #[value(name = "answered_elsewhere")]
    AnsweredElsewhere,
    /// The question is outside the meeting scope.
    #[value(name = "out_of_scope")]
    OutOfScope,
    /// The answer is no longer needed.
    #[value(name = "no_longer_needed")]
    NoLongerNeeded,
}

/// Meeting V1 moderator DecisionAttempt terminal class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MeetingDecisionAttemptFinishOutcome {
    /// Model execution completed and needs no primary action.
    Completed,
    /// Shared protocol state made the model result irrelevant.
    Discarded,
}

/// Observable stage reported by a Meeting V1 Grant holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MeetingGrantProgressStage {
    /// Synchronizing context for the turn.
    #[value(name = "context_sync")]
    ContextSync,
    /// Calling an allowed tool.
    #[value(name = "tool_use")]
    ToolUse,
    /// Generating a response.
    Generating,
    /// A Human is composing input.
    Composing,
    /// Submitting the signed speech event.
    Submitting,
}

/// Meeting V1 Grant Yield reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MeetingGrantYieldReason {
    /// The turn is no longer needed.
    #[value(name = "no_longer_needed")]
    NoLongerNeeded,
    /// The holder cannot provide a useful answer.
    #[value(name = "unable_to_answer")]
    UnableToAnswer,
    /// Required context is unavailable.
    #[value(name = "insufficient_context")]
    InsufficientContext,
    /// An allowed tool failed.
    #[value(name = "tool_failure")]
    ToolFailure,
    /// The holder explicitly cancelled the turn.
    Cancelled,
}

/// Meeting V1 directed-handoff type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum MeetingHandoffType {
    /// Ask the target a question.
    Question,
    /// Ask the target to provide information.
    #[value(name = "information_request")]
    InformationRequest,
    /// Ask the target to clarify a point.
    Clarification,
    /// Ask the target to review something.
    Review,
    /// Explicitly request a response from the target.
    #[value(name = "response_requested")]
    ResponseRequested,
}

#[derive(Subcommand)]
enum Cmd {
    /// Draft owner-reviewed agent creation and updates
    #[command(subcommand)]
    Agents(AgentsCmd),
    /// Send, read, search, and manage messages
    #[command(subcommand)]
    Messages(MessagesCmd),
    /// Create, configure, and manage channels
    #[command(subcommand)]
    Channels(ChannelsCmd),
    /// Create, inspect, and end versioned shared Meeting rooms
    #[command(subcommand)]
    Meetings(MeetingsCmd),
    /// Get and set channel canvas documents
    #[command(subcommand)]
    Canvas(CanvasCmd),
    /// Add, remove, and list emoji reactions
    #[command(subcommand)]
    Reactions(ReactionsCmd),
    /// Manage your custom emoji set (workspace palette is the union of all members' sets)
    #[command(subcommand)]
    Emoji(EmojiCmd),
    /// List, open, and manage direct messages
    #[command(subcommand)]
    Dms(DmsCmd),
    /// Look up users and manage profiles and presence
    #[command(subcommand)]
    Users(UsersCmd),
    /// Create, trigger, and manage workflows
    #[command(subcommand)]
    Workflows(WorkflowsCmd),
    /// Read the activity feed
    #[command(subcommand)]
    Feed(FeedCmd),
    /// Publish notes and manage the social graph (NIP-01/02)
    #[command(subcommand)]
    Social(SocialCmd),
    /// Publish and edit long-form NIP-23 notes — team knowledge base
    #[command(subcommand)]
    Notes(NotesCmd),
    /// Announce and discover git repositories (NIP-34)
    #[command(subcommand)]
    Repos(ReposCmd),
    /// Send, get, list, and set status on git patches (NIP-34)
    #[command(subcommand)]
    Patches(PatchesCmd),
    /// Create, get, list, and set status on git issues (NIP-34)
    #[command(subcommand)]
    Issues(IssuesCmd),
    /// Open, update, list, and set status on git pull requests (NIP-34)
    #[command(subcommand)]
    Pr(PrCmd),
    /// Upload and download relay Blossom media
    #[command(subcommand)]
    Media(MediaCmd),
    /// Upload files to the relay's Blossom store
    #[command(subcommand)]
    Upload(UploadCmd),
    /// Agent engram management — persistent memory per NIP-AE
    #[command(subcommand)]
    Mem(MemCmd),
    /// Read and mutate the Community's canonical Project View
    #[command(subcommand, name = "project-view")]
    ProjectView(ProjectViewCmd),
    /// Read and maintain independent versioned Project Documents
    #[command(subcommand)]
    Documents(DocumentsCmd),
    /// Discover and maintain Project Context hyperedges
    #[command(subcommand, name = "project-context")]
    ProjectContext(ProjectContextCmd),
    /// Resolve Project View Resources and their mandatory Guides
    #[command(subcommand)]
    Resources(ResourcesCmd),
    /// Read and govern Project View v3 Roles and Assignments
    #[command(subcommand)]
    Roles(RolesCmd),
    /// Submit trusted managed-runtime evidence and read availability
    #[command(subcommand)]
    Runtime(RuntimeCmd),
    /// Persona pack operations (local, no relay connection needed)
    #[command(subcommand)]
    Pack(PackCmd),
    /// Community moderation — reports queue, bans, timeouts, audit trail
    #[command(subcommand)]
    Moderation(ModerationCmd),
}

/// Project View object type accepted by CLI commands.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProjectViewObjectTypeArg {
    /// The unique project profile.
    #[value(name = "project_profile")]
    ProjectProfile,
    /// A desired project outcome.
    Goal,
    /// A stable semantic responsibility.
    Role,
    /// A body of planning logic.
    Plan,
    /// A segment within a plan.
    Stage,
    /// Something the project intends to satisfy.
    Requirement,
    /// A discovered problem or gap.
    Issue,
    /// A unit of execution.
    Work,
    /// A stable project resource.
    Resource,
}

impl From<ProjectViewObjectTypeArg> for buzz_project_view::ProjectViewObjectType {
    fn from(value: ProjectViewObjectTypeArg) -> Self {
        match value {
            ProjectViewObjectTypeArg::ProjectProfile => Self::ProjectProfile,
            ProjectViewObjectTypeArg::Goal => Self::Goal,
            ProjectViewObjectTypeArg::Role => Self::Role,
            ProjectViewObjectTypeArg::Plan => Self::Plan,
            ProjectViewObjectTypeArg::Stage => Self::Stage,
            ProjectViewObjectTypeArg::Requirement => Self::Requirement,
            ProjectViewObjectTypeArg::Issue => Self::Issue,
            ProjectViewObjectTypeArg::Work => Self::Work,
            ProjectViewObjectTypeArg::Resource => Self::Resource,
        }
    }
}

/// Governance level assigned when creating a Project Role.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProjectRoleLevelArg {
    /// A Leader Role whose active Assignment projects Community admin.
    Admin,
    /// An ordinary Role whose active Assignment projects Community member.
    Member,
}

impl From<ProjectRoleLevelArg> for buzz_project_view::v2::RoleLevel {
    fn from(value: ProjectRoleLevelArg) -> Self {
        match value {
            ProjectRoleLevelArg::Admin => Self::Admin,
            ProjectRoleLevelArg::Member => Self::Member,
        }
    }
}

/// Commands for the Community-global Project View.
#[derive(Subcommand)]
pub enum ProjectViewCmd {
    /// Read and assemble one consistent logical Project View snapshot
    Get,
    /// Read one active object or tombstone by stable coordinate
    GetObject {
        /// Canonical Project View object type.
        #[arg(value_enum)]
        object_type: ProjectViewObjectTypeArg,
        /// Stable object UUID.
        id: Uuid,
    },
    /// Initialize one prepared empty schema-v3 Community from a closed command
    InitV3 {
        /// JSON file containing the complete ProjectViewInitializeV3 command.
        #[arg(long)]
        command: String,
    },
    /// Explicit Human-only v2-to-v3 migration review (does not mutate Relay state)
    V3 {
        #[command(subcommand)]
        command: ProjectViewV3ClientCmd,
    },
    /// Discover and edit schema-v3 Context Reference coordinates
    Context {
        #[command(subcommand)]
        command: ProjectViewContextCmd,
    },
    /// Create one typed object with an optional caller-selected UUID v4
    Create {
        /// Object type to create (the project profile is not creatable here).
        #[arg(value_enum)]
        object_type: ProjectViewObjectTypeArg,
        /// Project revision on which this intent was based.
        #[arg(long)]
        expected_project_revision: u64,
        /// Explicit UUID v4; omitted to generate a fresh ID.
        #[arg(long)]
        id: Option<Uuid>,
        /// Typed body/relations JSON, or `-`; `summary` is optional retrieval metadata.
        #[arg(long)]
        data: String,
        /// Initial Role level. Valid only when object_type is `role`.
        #[arg(long, value_enum)]
        role_level: Option<ProjectRoleLevelArg>,
    },
    /// Apply one closed, typed patch to an active object
    Update {
        /// Canonical Project View object type.
        #[arg(value_enum)]
        object_type: ProjectViewObjectTypeArg,
        /// Stable object UUID.
        id: Uuid,
        /// Project revision on which this intent was based.
        #[arg(long)]
        expected_project_revision: u64,
        /// Typed patch, or `-`; omit `summary` to keep, use text to set, or null to clear.
        #[arg(long)]
        patch: String,
    },
    /// Tombstone one active object
    Delete {
        /// Canonical Project View object type.
        #[arg(value_enum)]
        object_type: ProjectViewObjectTypeArg,
        /// Stable object UUID.
        id: Uuid,
        /// Project revision on which this intent was based.
        #[arg(long)]
        expected_project_revision: u64,
    },
}

/// Context Reference operations for one active Project View object.
#[derive(Subcommand)]
pub enum ProjectViewContextCmd {
    /// List the object's canonical Context Reference set
    List {
        /// Stable source object UUID.
        object_id: Uuid,
    },
    /// Add one Resource, live Document, or pinned Document coordinate
    Add {
        /// Stable source object UUID.
        object_id: Uuid,
        /// Stable target Resource UUID.
        #[arg(
            long,
            conflicts_with = "document",
            required_unless_present = "document"
        )]
        resource: Option<Uuid>,
        /// Stable target Document UUID.
        #[arg(
            long,
            conflicts_with = "resource",
            required_unless_present = "resource"
        )]
        document: Option<Uuid>,
        /// Exact pinned Document revision; omission means a live reference.
        #[arg(long, requires = "document")]
        revision: Option<u64>,
    },
    /// Remove one exact Resource, live Document, or pinned Document coordinate
    Remove {
        /// Stable source object UUID.
        object_id: Uuid,
        /// Stable target Resource UUID.
        #[arg(
            long,
            conflicts_with = "document",
            required_unless_present = "document"
        )]
        resource: Option<Uuid>,
        /// Stable target Document UUID.
        #[arg(
            long,
            conflicts_with = "resource",
            required_unless_present = "resource"
        )]
        document: Option<Uuid>,
        /// Exact pinned Document revision; omission means a live reference.
        #[arg(long, requires = "document")]
        revision: Option<u64>,
    },
}

/// Explicit local migration-review workflows.
#[derive(Subcommand)]
pub enum ProjectViewV3ClientCmd {
    /// Review legacy Resource mappings for an operator v2-to-v3 cutover.
    Resources {
        #[command(subcommand)]
        command: ProjectViewV3ResourcesClientCmd,
    },
}

/// Human Resource review commands.
#[derive(Subcommand)]
pub enum ProjectViewV3ResourcesClientCmd {
    /// Verify frozen v2 migration inputs and create detached Human approvals
    Approve {
        /// Operator-exported draft JSON completed by the Human reviewer.
        #[arg(long)]
        manifest: String,
        /// Destination for the closed reviewed manifest JSON.
        #[arg(long)]
        out: String,
    },
}

/// Commands for the Community-global Project Document catalog.
#[derive(Subcommand)]
pub enum DocumentsCmd {
    /// List active Document metadata without fetching Markdown bodies
    List,
    /// Read the current or one pinned immutable Document revision
    Get {
        /// Stable Document UUID.
        document_id: Uuid,
        /// Exact immutable revision; omit for current.
        #[arg(long)]
        revision: Option<u64>,
        /// Print only raw Markdown to stdout.
        #[arg(long)]
        content_only: bool,
    },
    /// List immutable revision metadata without printing Markdown bodies
    History {
        /// Stable Document UUID.
        document_id: Uuid,
    },
    /// Create a complete revision-one Document snapshot
    Create {
        /// Canonical non-empty title.
        #[arg(long)]
        title: String,
        /// Optional non-empty summary.
        #[arg(long)]
        summary: Option<String>,
        /// Literal Markdown, or `-` for bounded stdin.
        #[arg(long, conflicts_with = "content_file")]
        content: Option<String>,
        /// Markdown file path, or `-` for bounded stdin.
        #[arg(long, conflicts_with = "content")]
        content_file: Option<String>,
        /// Client-selected UUID v4; generated when omitted.
        #[arg(long)]
        document_id: Option<Uuid>,
    },
    /// Replace the complete active Document snapshot
    Update {
        /// Stable Document UUID.
        document_id: Uuid,
        /// Exact current revision observed by the caller.
        #[arg(long)]
        expected_revision: u64,
        /// Complete next title.
        #[arg(long)]
        title: String,
        /// Complete next non-empty summary.
        #[arg(long, conflicts_with = "clear_summary")]
        summary: Option<String>,
        /// Explicitly omit the summary in the next snapshot.
        #[arg(long, conflicts_with = "summary")]
        clear_summary: bool,
        /// Literal Markdown, or `-` for bounded stdin.
        #[arg(long, conflicts_with = "content_file")]
        content: Option<String>,
        /// Markdown file path, or `-` for bounded stdin.
        #[arg(long, conflicts_with = "content")]
        content_file: Option<String>,
    },
    /// Apply one exact-position unified diff and submit a full update
    Patch {
        /// Stable Document UUID.
        document_id: Uuid,
        /// Exact base revision to fetch and patch.
        #[arg(long)]
        expected_revision: u64,
        /// Unified diff file, or `-` for bounded stdin.
        #[arg(long)]
        patch_file: String,
        /// Optional file in which to save the exact merged Markdown.
        #[arg(long)]
        output: Option<String>,
        /// Replace the title; omitted preserves the base title.
        #[arg(long)]
        title: Option<String>,
        /// Replace the summary; omitted preserves the base summary.
        #[arg(long, conflicts_with = "clear_summary")]
        summary: Option<String>,
        /// Explicitly clear the base summary.
        #[arg(long, conflicts_with = "summary")]
        clear_summary: bool,
    },
    /// Append a bodyless tombstone revision
    Delete {
        /// Stable Document UUID.
        document_id: Uuid,
        /// Exact current revision observed by the caller.
        #[arg(long)]
        expected_revision: u64,
    },
}

/// Commands for Project Context Edge discovery and maintenance.
#[derive(Subcommand)]
pub enum ProjectContextCmd {
    /// Find the unique Edge with exactly this unordered coordinate set
    Exact {
        /// Typed coordinate token; repeat for every endpoint.
        #[arg(long = "coordinate", required = true)]
        coordinates: Vec<String>,
    },
    /// Find every Edge incident to one coordinate
    Incident {
        /// Typed coordinate token.
        coordinate: String,
    },
    /// Find every Edge containing all supplied coordinates; none means all Edges
    #[command(name = "contains-all")]
    ContainsAll {
        /// Typed coordinate token; repeat to form the required subset.
        #[arg(long = "coordinate")]
        coordinates: Vec<String>,
    },
    /// Attach one existing Project Document to an exact coordinate set
    ///
    /// Meeting coordinates are accepted when Relay-verified terminal or in a
    /// frozen Action Finalization window; other active Meetings are rejected.
    Attach {
        /// Existing active Project Document carrying the explanatory context.
        #[arg(long = "context-document")]
        context_document_id: Uuid,
        /// Typed coordinate token; repeat for every endpoint.
        #[arg(long = "coordinate", required = true)]
        coordinates: Vec<String>,
        #[command(flatten)]
        attribution: ProjectContextAttributionArgs,
    },
    /// Detach one Project Document from its exact coordinate set
    Detach {
        /// Currently bound Project Document.
        #[arg(long = "context-document")]
        context_document_id: Uuid,
        /// Typed coordinate token; repeat for every endpoint.
        #[arg(long = "coordinate", required = true)]
        coordinates: Vec<String>,
        #[command(flatten)]
        attribution: ProjectContextAttributionArgs,
    },
}

/// Optional explicit managed-Agent attribution for a Context write.
#[derive(clap::Args)]
pub struct ProjectContextAttributionArgs {
    /// Optional supervised attribution; omit for an ordinary Community Context write.
    #[arg(long = "acting-assignment")]
    acting_assignment_id: Option<Uuid>,
    /// Supervised Runtime UUID; requires Assignment and Runtime epoch when used.
    #[arg(long = "runtime-id")]
    runtime_id: Option<Uuid>,
    /// Supervised Runtime epoch; requires Assignment and Runtime UUID when used.
    #[arg(long = "runtime-epoch")]
    runtime_epoch: Option<u64>,
}

/// Commands for locator-free schema-v3 Resources.
#[derive(Subcommand)]
pub enum ResourcesCmd {
    /// Resolve one current Resource and read its mandatory Guide Document
    Guide {
        /// Stable Resource UUID.
        resource_id: Uuid,
        /// Exact Guide Document revision; omit for current.
        #[arg(long)]
        revision: Option<u64>,
        /// Print only raw Guide Markdown to stdout.
        #[arg(long)]
        content_only: bool,
    },
}

/// Trusted runtime supervisor operations.
#[derive(Subcommand)]
pub enum RuntimeCmd {
    /// Submit one immutable, Assignment-scoped supervisor observation
    Evidence {
        /// Closed evidence transition.
        #[arg(value_enum)]
        evidence: RuntimeEvidenceArg,
        /// Exact managed-Agent Assignment UUID.
        #[arg(long)]
        assignment: Uuid,
        /// Stable logical runtime UUID.
        #[arg(long)]
        runtime: Uuid,
        /// Current server-allocated epoch; omit only for `start`.
        #[arg(long)]
        epoch: Option<u64>,
        /// Stable retry key; generated when omitted.
        #[arg(long)]
        idempotency_key: Option<Uuid>,
        /// Bounded diagnostic summary for abnormal/failed recovery.
        #[arg(long)]
        summary: Option<String>,
        /// Operating-system exit status for `abnormal_exit`.
        #[arg(long)]
        exit_code: Option<i32>,
    },
    /// Read one Assignment's current runtime availability
    Status {
        /// Assignment UUID.
        #[arg(long)]
        assignment: Uuid,
    },
}

/// Closed trusted runtime evidence type.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum RuntimeEvidenceArg {
    /// Open a newly fenced runtime epoch.
    Start,
    /// Renew the lease for an available epoch.
    #[value(name = "lease_renewed")]
    LeaseRenewed,
    /// Retire a deliberately stopped available runtime without ending its Assignment.
    #[value(name = "graceful_stop")]
    GracefulStop,
    /// Begin recovery after a trusted abnormal process exit.
    #[value(name = "abnormal_exit")]
    AbnormalExit,
    /// Allocate the next fenced epoch for one bounded recovery attempt.
    #[value(name = "recovery_attempt")]
    RecoveryAttempt,
    /// Mark the currently fenced replacement attempt as healthy.
    #[value(name = "recovery_succeeded")]
    RecoverySucceeded,
    /// Record a failed recovery result.
    #[value(name = "recovery_failed")]
    RecoveryFailed,
    /// Prove supervisor health without consuming a retry.
    #[value(name = "supervisor_heartbeat")]
    SupervisorHeartbeat,
}

/// Project View v3 Role continuity commands.
#[derive(Subcommand)]
pub enum RolesCmd {
    /// List canonical Roles with their current assignee or vacancy
    List,
    /// Render the verified current Role Brief for one Member
    Brief {
        /// Member public key (hex or npub); defaults to the CLI signer.
        #[arg(long)]
        member: Option<String>,
        /// Render the shared prompt/human Markdown form instead of JSON.
        #[arg(long)]
        markdown: bool,
    },
    /// Read one canonical Role and its current Assignment
    Get {
        /// Stable Role UUID.
        role: Uuid,
    },
    /// Read one Member's current Role Assignment
    Current {
        /// Member public key (hex or npub); defaults to the CLI signer.
        #[arg(long)]
        member: Option<String>,
    },
    /// List Role Assignment Proposals
    Proposals {
        /// Limit to one effective status.
        #[arg(long, value_enum)]
        status: Option<RoleProposalStatusArg>,
        /// Maximum history entries to scan and return.
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Opaque `next_before` cursor from the preceding page.
        #[arg(long)]
        before: Option<String>,
    },
    /// Request a Role as the current signer
    Request {
        /// Desired Role UUID.
        #[arg(long)]
        role: Uuid,
        /// Project revision on which the request is based.
        #[arg(long)]
        expected_project_revision: u64,
        /// Proposal lifetime in hours.
        #[arg(long, default_value_t = 168, value_parser = clap::value_parser!(u16).range(1..=720))]
        expires_in_hours: u16,
        /// Optional request context.
        #[arg(long)]
        reason: Option<String>,
        /// Current active Assignment fence when already assigned.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Offer a Role to a candidate
    Offer {
        /// Offered Role UUID.
        #[arg(long)]
        role: Uuid,
        /// Candidate public key (hex or npub).
        #[arg(long)]
        member: String,
        /// Project revision on which the complete move is based.
        #[arg(long)]
        expected_project_revision: u64,
        /// Proposal lifetime in hours.
        #[arg(long, default_value_t = 168, value_parser = clap::value_parser!(u16).range(1..=720))]
        expires_in_hours: u16,
        /// Optional offer context.
        #[arg(long)]
        reason: Option<String>,
        /// Active Leader Assignment fence when authorizing as a Leader.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Confirm, reject, withdraw, authorize, or expire a Proposal
    Proposal {
        #[command(subcommand)]
        command: RoleProposalCmd,
    },
    /// Read or act on one Assignment
    Assignment {
        #[command(subcommand)]
        command: RoleAssignmentCmd,
    },
    /// Assign, accept, release, or recommit Role-owned Work
    Work {
        #[command(subcommand)]
        command: RoleWorkCmd,
    },
    /// Append or page through structured Role Checkpoints
    Checkpoint {
        #[command(subcommand)]
        command: RoleCheckpointCmd,
    },
    /// Append or page through Role Handoff history
    Handoff {
        #[command(subcommand)]
        command: RoleHandoffCmd,
    },
}

/// Effective Proposal status accepted by `cf roles proposals`.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum RoleProposalStatusArg {
    /// Awaiting one or both confirmations.
    Open,
    /// Assignment was activated.
    Consumed,
    /// Explicitly rejected.
    Rejected,
    /// Withdrawn by its creator.
    Withdrawn,
    /// Canonical deadline has passed.
    Expired,
}

/// Commands targeting one Proposal.
#[derive(Subcommand)]
pub enum RoleProposalCmd {
    /// Accept an offer as its candidate
    Accept {
        /// Proposal UUID.
        proposal: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Optional current Assignment fence.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Reject an open Proposal
    Reject {
        /// Proposal UUID.
        proposal: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Optional explanation.
        #[arg(long)]
        reason: Option<String>,
        /// Optional current Assignment fence.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Withdraw a Proposal created by the signer
    Withdraw {
        /// Proposal UUID.
        proposal: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Optional explanation.
        #[arg(long)]
        reason: Option<String>,
        /// Optional current Assignment fence.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Authorize a candidate request as owner or Leader
    Authorize {
        /// Proposal UUID.
        proposal: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Active Leader Assignment fence when not Community owner.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Materialize an already effective Proposal expiration
    Expire {
        /// Proposal UUID.
        proposal: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Optional current Assignment fence.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
}

/// Commands targeting Assignments.
#[derive(Subcommand)]
pub enum RoleAssignmentCmd {
    /// List Assignment history, optionally narrowed by Role or Member
    List {
        /// Role UUID.
        #[arg(long)]
        role: Option<Uuid>,
        /// Member public key (hex or npub).
        #[arg(long)]
        member: Option<String>,
        /// Include ended tenure history.
        #[arg(long)]
        include_ended: bool,
        /// Maximum history entries to return.
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Opaque `next_before` cursor from the preceding page.
        #[arg(long)]
        before: Option<String>,
    },
    /// Read one Assignment by UUID
    Get {
        /// Assignment UUID.
        assignment: Uuid,
    },
    /// End another Member's active Assignment
    End {
        /// Target Assignment UUID.
        assignment: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Optional governance explanation.
        #[arg(long)]
        reason: Option<String>,
        /// Active Leader Assignment fence when not Community owner.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Ask governance to arrange a replacement without self-ending
    RequestReplacement {
        /// Caller's active Assignment UUID.
        assignment: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Optional context.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Report inability to continue without self-ending
    ReportUnableToContinue {
        /// Caller's active Assignment UUID.
        assignment: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Optional context.
        #[arg(long)]
        reason: Option<String>,
    },
}

/// Commands targeting Work responsibility and Commitment.
#[derive(Subcommand)]
pub enum RoleWorkCmd {
    /// Assign one Work to a stable Role
    Assign {
        /// Work UUID.
        #[arg(long)]
        work: Uuid,
        /// Responsible Role UUID.
        #[arg(long)]
        role: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Active Leader Assignment fence when not Community owner.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Clear the responsible Role from one uncommitted Work
    Unassign {
        /// Work UUID.
        #[arg(long)]
        work: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Active Leader Assignment fence when not Community owner.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Accept Work owned by the caller's current Role
    Accept {
        /// Work UUID.
        #[arg(long)]
        work: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Current Assignment fence; managed runtimes resolve it automatically.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Release the caller's active Commitment without changing Work status
    Release {
        /// Commitment UUID.
        #[arg(long)]
        commitment: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Optional release context.
        #[arg(long)]
        reason: Option<String>,
        /// Current Assignment fence; managed runtimes resolve it automatically.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Atomically replace the caller's active Commitment to the same Work
    Recommit {
        /// Work UUID.
        #[arg(long)]
        work: Uuid,
        /// Active Commitment observed by the caller.
        #[arg(long)]
        commitment: Uuid,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Current Assignment fence; managed runtimes resolve it automatically.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
}

/// Commands targeting append-only Role Checkpoints.
#[derive(Subcommand)]
pub enum RoleCheckpointCmd {
    /// Append a structured Checkpoint through the current Assignment
    Append {
        /// JSON file containing `RoleCheckpointContent`, or `-` for stdin.
        #[arg(long)]
        input: String,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Reviewed Project revision; defaults to the expected revision.
        #[arg(long)]
        based_on_project_revision: Option<u64>,
        /// Earlier Checkpoint from this Assignment being corrected.
        #[arg(long)]
        supersedes: Option<Uuid>,
        /// Current Assignment fence; managed runtimes resolve it automatically.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Page through Checkpoint history, newest first
    List {
        /// Limit to one Role.
        #[arg(long)]
        role: Option<Uuid>,
        /// Limit to one Assignment.
        #[arg(long)]
        assignment: Option<Uuid>,
        /// Maximum history entries to return.
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Opaque `next_before` cursor from the preceding page.
        #[arg(long)]
        before: Option<String>,
    },
}

/// Member-authored Handoff causes.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum RoleHandoffCauseArg {
    /// A planned transition or context transfer.
    Planned,
    /// Other explicitly described transition context.
    Other,
}

/// Commands targeting append-only Role Handoffs.
#[derive(Subcommand)]
pub enum RoleHandoffCmd {
    /// Append a Handoff note without ending the Assignment
    Append {
        /// JSON file containing `RoleHandoffContent`, or `-` for stdin.
        #[arg(long)]
        input: String,
        /// Current project revision.
        #[arg(long)]
        expected_project_revision: u64,
        /// Known successor Assignment in the same Role.
        #[arg(long)]
        to_assignment: Option<Uuid>,
        /// Checkpoint explicitly carried into this Handoff.
        #[arg(long)]
        checkpoint: Option<Uuid>,
        /// Member-authored transition cause.
        #[arg(long, value_enum, default_value = "planned")]
        cause: RoleHandoffCauseArg,
        /// Current Assignment fence; managed runtimes resolve it automatically.
        #[arg(long)]
        acting_assignment: Option<Uuid>,
    },
    /// Page through Handoff history, newest first
    List {
        /// Limit to one Role.
        #[arg(long)]
        role: Option<Uuid>,
        /// Limit to one source Assignment.
        #[arg(long)]
        assignment: Option<Uuid>,
        /// Maximum history entries to return.
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Opaque `next_before` cursor from the preceding page.
        #[arg(long)]
        before: Option<String>,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum RespondToArg {
    #[value(name = "owner-only")]
    OwnerOnly,
    #[value(name = "anyone")]
    Anyone,
}

impl RespondToArg {
    fn to_wire(self) -> String {
        match self {
            Self::OwnerOnly => "owner-only",
            Self::Anyone => "anyone",
        }
        .to_string()
    }
}

#[derive(Subcommand)]
pub enum AgentsCmd {
    /// Open a prefilled create-agent form in the owner's Carryforth Desktop
    DraftCreate {
        /// Current channel UUID; the new agent is added here after save
        #[arg(long)]
        channel: String,
        /// Proposed agent name
        #[arg(long)]
        display_name: String,
        /// Proposed instructions; use '-' to read from stdin
        #[arg(long)]
        system_prompt: String,
    },
    /// Open a prefilled edit-agent form in the owner's Carryforth Desktop
    DraftUpdate {
        /// Current channel UUID
        #[arg(long)]
        channel: String,
        /// Current name of the personal agent to update
        #[arg(long)]
        agent_name: String,
        #[arg(long)]
        display_name: Option<String>,
        /// Replacement instructions; use '-' to read from stdin
        #[arg(long)]
        system_prompt: Option<String>,
        #[arg(long)]
        runtime: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_enum)]
        respond_to: Option<RespondToArg>,
    },
    /// Submit a NIP-IA archive request for an identity (kind 9035)
    #[command(
        after_help = "The relay chooses the consent path (self / admin / owner) from the \
submitted request; this command does not retry with a different shape.\n\n\
Suggested --reason codes (unknown values are allowed): rotated, retired, \
bot-rebuilt, left-organization, spam\n\n\
Archiving a third-party identity is a human owner/admin action: an agent \
running under CARRYFORTH_AUTH_TAG signs as itself, so it can only ever satisfy \
the self path (target == signer) — not the owner-of-agent path for another \
identity.\n\n\
Examples:\n  \
cf agents archive <PUBKEY> --reason retired\n  \
cf agents archive <PUBKEY> --reason bot-rebuilt --replaced-by <NEW_PUBKEY>"
    )]
    Archive {
        /// Target identity pubkey (hex)
        target_pubkey: String,
        /// Machine-readable reason code, max 64 UTF-8 bytes
        #[arg(long)]
        reason: Option<String>,
        /// Rotation pointer pubkey (hex); must differ from the target
        #[arg(long)]
        replaced_by: Option<String>,
        /// Optional human-readable note (not parsed for authorization)
        #[arg(long, default_value = "")]
        content: String,
    },
    /// Submit a NIP-IA unarchive request for an identity (kind 9036)
    #[command(after_help = "Examples:\n  \
cf agents unarchive <PUBKEY> --reason returned")]
    Unarchive {
        /// Target identity pubkey (hex)
        target_pubkey: String,
        /// Machine-readable reason code, max 64 UTF-8 bytes
        #[arg(long)]
        reason: Option<String>,
        /// Optional human-readable note (not parsed for authorization)
        #[arg(long, default_value = "")]
        content: String,
    },
    /// Read the relay's current NIP-IA archive snapshot (kind 13535)
    #[command(
        after_help = "Verifies the snapshot's NIP-11 `self` authorship, event id, signature, \
and NIP-70 `-` protection tag before trusting it. Any trust failure is a \
nonzero-exit error, never a false-empty success — this command's whole \
purpose is verification.\n\n\
Examples:\n  \
cf agents archived"
    )]
    Archived,
}

#[derive(Subcommand)]
pub enum MessagesCmd {
    /// Send a message to a channel
    #[command(
        after_help = "Examples:\n  cf messages send --channel <UUID> --content \"hello\"\n  cf messages send --channel <UUID> --content \"@alice check this\"\n  echo \"hello from stdin\" | cf messages send --channel <UUID> --content -"
    )]
    Send {
        /// Channel UUID (from 'cf channels list')
        #[arg(long)]
        channel: String,
        /// Message text — supports @mentions and markdown. Use '-' to read from stdin.
        #[arg(long)]
        content: String,
        /// Nostr event kind (default: channel default)
        #[arg(long)]
        kind: Option<u16>,
        /// Event ID to reply to (creates a thread)
        #[arg(long)]
        reply_to: Option<String>,
        /// Also publish to the Nostr network
        #[arg(long, default_value_t = false)]
        broadcast: bool,
        /// Attach file(s) — uploads and includes as imeta tags
        #[arg(long = "file")]
        files: Vec<String>,
    },
    /// Send a code diff / patch to a channel
    SendDiff {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Diff/patch content (use '-' to read from stdin)
        #[arg(long)]
        diff: String,
        /// Repository URL (e.g. https://github.com/org/repo)
        #[arg(long)]
        repo: String,
        /// Commit SHA
        #[arg(long)]
        commit: String,
        /// Single file path within the repo
        #[arg(long)]
        file: Option<String>,
        /// Parent commit SHA for three-way diff context
        #[arg(long)]
        parent_commit: Option<String>,
        /// Source branch name
        #[arg(long)]
        source_branch: Option<String>,
        /// Target branch name
        #[arg(long)]
        target_branch: Option<String>,
        /// Pull request number
        #[arg(long)]
        pr: Option<u32>,
        /// Language hint (auto-detected from file extension if omitted)
        #[arg(long)]
        lang: Option<String>,
        /// Human-readable description of the change
        #[arg(long)]
        description: Option<String>,
        /// Event ID to reply to (creates a thread)
        #[arg(long)]
        reply_to: Option<String>,
    },
    /// Edit a previously sent message
    Edit {
        /// Event ID of the message to edit (64-char hex)
        #[arg(long)]
        event: String,
        /// New message content
        #[arg(long)]
        content: String,
    },
    /// Delete a message by event ID
    Delete {
        /// Event ID to delete (64-char hex)
        #[arg(long)]
        event: String,
        /// Optional moderation audit action UUID for the public tombstone
        #[arg(long)]
        action_id: Option<Uuid>,
        /// Optional machine-readable public reason code for the tombstone
        #[arg(long)]
        reason_code: Option<String>,
        /// Optional human-readable public reason for the tombstone
        #[arg(long)]
        public_reason: Option<String>,
    },
    /// Retrieve messages from a channel
    #[command(
        after_help = "Examples:\n  cf messages get --channel <UUID>\n  cf messages get --channel <UUID> --limit 50 --kinds 1,1984"
    )]
    Get {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
        /// Unix timestamp — return messages before this time
        #[arg(long)]
        before: Option<i64>,
        /// Unix timestamp — return messages after this time
        #[arg(long)]
        since: Option<i64>,
        /// Comma-separated event kinds to filter (e.g. 1,1984)
        #[arg(long)]
        kinds: Option<String>,
    },
    /// Get a message thread (replies to a root message)
    Thread {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Root message event ID (64-char hex)
        #[arg(long)]
        event: String,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
        /// Maximum reply nesting depth to include
        #[arg(long)]
        depth_limit: Option<u32>,
    },
    /// Full-text search across messages
    #[command(
        after_help = "Examples:\n  cf messages search --query checkout\n  cf messages search --author npub1... --since 1783497600\n  cf messages search --author Aaron --query checkout --limit 20"
    )]
    Search {
        /// Search query string (optional when --author is given)
        #[arg(long)]
        query: Option<String>,
        /// Filter by author: 64-char hex pubkey, npub, or display name
        #[arg(long)]
        author: Option<String>,
        /// Unix timestamp — return messages after this time
        #[arg(long)]
        since: Option<i64>,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Upvote or downvote a forum post
    Vote {
        /// Event ID of the post to vote on (64-char hex)
        #[arg(long)]
        event: String,
        /// Vote direction: "up" or "down"
        #[arg(long)]
        direction: String,
    },
}

#[derive(Subcommand)]
pub enum ChannelsCmd {
    /// List channels visible to the current identity
    #[command(after_help = "Examples:\n  cf channels list\n  cf channels list --visibility open")]
    List {
        /// Filter by visibility
        #[arg(long, value_enum)]
        visibility: Option<ChannelVisibility>,
        /// Only show channels where the current identity is a member
        #[arg(long, default_value_t = false)]
        member: bool,
        /// Maximum number of channels to return [default: 500]
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get details for a single channel
    Get {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Search channels by human-readable name
    #[command(
        after_help = "Examples:\n  cf channels search --query composer\n  cf channels search --query buzz-chat-composer --exact\n  cf channels search --query design --include-archived"
    )]
    Search {
        /// Search query (case-insensitive substring of channel name)
        #[arg(long)]
        query: String,
        /// Require an exact case-insensitive match instead of substring
        #[arg(long, default_value_t = false)]
        exact: bool,
        /// Include archived channels in results
        #[arg(long, default_value_t = false)]
        include_archived: bool,
        /// Maximum number of channel-metadata events to fetch from the relay
        #[arg(long, default_value_t = 1000)]
        limit: u32,
    },
    /// Create a new channel
    #[command(
        after_help = "Examples:\n  cf channels create --name general --type stream --visibility open\n  cf channels create --name design --type forum --visibility open --description \"Design discussions\"\n  cf channels create --name standup --type stream --visibility open --ttl 3600  # ephemeral, archived after 1h idle\n  cf channels create --name project-x --template \"Carryforth Team\"  # type/visibility/canvas/roster from the template; explicit flags override"
    )]
    Create {
        /// Channel name
        #[arg(long)]
        name: String,
        /// Channel type. Required unless --template supplies one.
        #[arg(long = "type", value_enum, required_unless_present = "template")]
        channel_type: Option<ChannelType>,
        /// Channel visibility. Required unless --template supplies one.
        #[arg(long, value_enum, required_unless_present = "template")]
        visibility: Option<ChannelVisibility>,
        /// Channel description
        #[arg(long)]
        description: Option<String>,
        /// Make the channel ephemeral: lifetime in seconds. The relay archives
        /// it once this many seconds pass without a new message.
        #[arg(long, value_name = "SECONDS")]
        ttl: Option<i64>,
        /// Apply a desktop-local channel template by name (case-insensitive):
        /// supplies default type/visibility/description/canvas, and resolves
        /// its agent roster against the relay to add as members.
        #[arg(long)]
        template: Option<String>,
        /// Override the channel-templates.json path (default: the desktop
        /// app's prod app-data dir). Mainly for the dev store or testing.
        #[arg(long, value_name = "PATH")]
        templates_file: Option<String>,
    },
    /// Update channel name, description, or ephemeral TTL
    Update {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// New channel name
        #[arg(long)]
        name: Option<String>,
        /// New channel description
        #[arg(long)]
        description: Option<String>,
        /// Make the channel ephemeral (or change its lifetime): seconds until
        /// the relay archives it after the last message. Conflicts with --no-ttl.
        #[arg(long, value_name = "SECONDS", conflicts_with = "no_ttl")]
        ttl: Option<i64>,
        /// Clear an existing TTL, making the channel permanent.
        #[arg(long)]
        no_ttl: bool,
    },
    /// Set the channel topic
    Topic {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// New topic text
        #[arg(long)]
        topic: String,
    },
    /// Set the channel purpose
    Purpose {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// New purpose text
        #[arg(long)]
        purpose: String,
    },
    /// Join a channel
    Join {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Leave a channel
    Leave {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Archive a channel
    Archive {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Unarchive a channel
    Unarchive {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Delete a channel permanently
    Delete {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// List members of a channel
    Members {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Add a member to a channel
    #[command(name = "add-member")]
    AddMember {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Member pubkey (64-char hex)
        #[arg(long)]
        pubkey: String,
        /// Member role (owner, admin, member, guest, bot)
        #[arg(long)]
        role: Option<String>,
    },
    /// Remove a member from a channel
    #[command(name = "remove-member")]
    RemoveMember {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Member pubkey (64-char hex)
        #[arg(long)]
        pubkey: String,
    },
    /// Set your channel addition policy
    #[command(name = "set-add-policy")]
    SetAddPolicy {
        /// Policy: anyone | owner_only | nobody
        #[arg(long)]
        policy: String,
    },
}

#[derive(Subcommand)]
pub enum MeetingsCmd {
    /// Create a private meeting with a frozen initial roster
    #[command(
        after_help = "Example:\n  cf meetings create --title \"Design review\" --board - --participant <PUBKEY>"
    )]
    Create {
        /// Meeting title
        #[arg(long)]
        title: String,
        /// Optional meeting description
        #[arg(long)]
        description: Option<String>,
        /// Optional source channel UUID; every participant must already be able to read it
        #[arg(long)]
        source: Option<String>,
        /// Internal legacy protocol override
        #[arg(
            long,
            value_enum,
            default_value = "moderated-board-actions-v3",
            hide = true
        )]
        policy: MeetingPolicy,
        /// Internal legacy moderator override
        #[arg(long, hide = true)]
        moderator: Option<String>,
        /// Initial Meeting Markdown board; use '-' to read from stdin
        #[arg(long)]
        board: Option<String>,
        /// Other participant pubkeys (repeat once per participant; the creator is implicit)
        #[arg(long = "participant", required = true)]
        participants: Vec<String>,
    },
    /// List meetings visible to the current identity
    List {
        /// Include meetings that have ended
        #[arg(long, default_value_t = false)]
        include_ended: bool,
        /// Maximum number of meeting metadata events to scan
        #[arg(long, default_value_t = 500)]
        limit: u32,
    },
    /// Show one meeting's identity and lifecycle
    Show {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Update the Meeting-owned retrieval summary in Action Finalization
    Update {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Complete summary text, or `-` for stdin
        #[arg(
            long,
            conflicts_with = "clear_summary",
            required_unless_present = "clear_summary"
        )]
        summary: Option<String>,
        /// Explicitly clear the current summary
        #[arg(long, conflicts_with = "summary", required_unless_present = "summary")]
        clear_summary: bool,
    },
    /// Read or maintain the current Meeting V2 board
    Board {
        #[command(subcommand)]
        command: MeetingBoardCmd,
    },
    /// Inspect Meeting V2 action-finalization state
    Actions {
        #[command(subcommand)]
        command: MeetingActionsCmd,
    },
    /// List the meeting's complete participant roster
    Participants {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Read the canonical meeting speech history
    History {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Maximum number of speech events to return
        #[arg(long, default_value_t = 500)]
        limit: u32,
    },
    /// Send one message using the current identity's active Grant
    Say {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Message text; use '-' to read from stdin
        #[arg(long)]
        content: String,
        /// Mentioned participant pubkey (repeatable)
        #[arg(long = "mention")]
        mentions: Vec<String>,
        /// Participant who should receive a directed handoff
        #[arg(long)]
        handoff_to: Option<String>,
        /// Directed handoff semantic type
        #[arg(long, requires = "handoff_to")]
        handoff_type: Option<MeetingHandoffType>,
        /// Required directed handoff explanation
        #[arg(long, requires = "handoff_to")]
        handoff_reason: Option<String>,
    },
    /// Read and manage moderated Meeting speech intents
    Intents {
        #[command(subcommand)]
        command: MeetingIntentsCmd,
    },
    /// Submit moderated Meeting decisions
    Moderator {
        #[command(subcommand)]
        command: MeetingModeratorCmd,
    },
    /// Respond to the current moderated Meeting Offer
    Offer {
        #[command(subcommand)]
        command: MeetingOfferCmd,
    },
    /// Maintain or yield the current moderated Meeting Grant
    Grant {
        #[command(subcommand)]
        command: MeetingGrantCmd,
    },
    /// Inspect or claim the relay-authoritative speech floor
    Floor {
        #[command(subcommand)]
        command: MeetingFloorCmd,
    },
    /// End a meeting and make its room read-only
    End {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Normally close a Meeting V2 after its final explicit Board result
    Close {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Abnormally terminate a Meeting V2 without declaring its goal reached
    Abort {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Stable machine-readable abort reason
        #[arg(long)]
        reason_code: String,
        /// Optional short explanation
        #[arg(long)]
        reason: Option<String>,
    },
}

/// Meeting V2 action-finalization operations.
#[derive(Subcommand)]
pub enum MeetingActionsCmd {
    /// Read the Relay-authoritative action run and close-gate progress
    Status {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Enter action finalization from the completed final Board
    Begin {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Durably block the current action run with a closed reason code
    Block {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Stable direct-action failure category
        #[arg(long)]
        reason_code: String,
        /// Optional bounded diagnostic
        #[arg(long)]
        reason: Option<String>,
    },
    /// Open a fresh execution window for a blocked action run
    Retry {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Confirm action outputs are recorded and close the Meeting
    #[command(name = "confirm-recorded")]
    ConfirmRecorded {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Return to Board while preserving any external effects already produced
    #[command(name = "return-to-board")]
    ReturnToBoard {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
}

/// Meeting V2 current-board operations.
#[derive(Subcommand)]
pub enum MeetingBoardCmd {
    /// Get the complete current board document
    Get {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Replace the complete current board and open the Floor window
    Update {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Path to the complete Markdown board; use '-' to read from stdin
        #[arg(long)]
        board: String,
        /// Explicit Control Token epoch; defaults to current State
        #[arg(long)]
        control_epoch: Option<u64>,
        /// Explicit Board window fence; defaults to current State
        #[arg(long)]
        board_window: Option<u64>,
    },
    /// Confirm the current board is unchanged and open the Floor window
    Unchanged {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Explicit Control Token epoch; defaults to current State
        #[arg(long)]
        control_epoch: Option<u64>,
        /// Explicit Board window fence; defaults to current State
        #[arg(long)]
        board_window: Option<u64>,
    },
}

/// Moderated Meeting speech-intent operations.
#[derive(Subcommand)]
pub enum MeetingIntentsCmd {
    /// List the Relay-authoritative pending intent pool
    List {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Submit one pending speech intent
    Submit {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// One-sentence summary; use '-' to read from stdin
        #[arg(long)]
        summary: String,
        /// Optional participant whom the contribution addresses
        #[arg(long)]
        addressed_to: Option<String>,
    },
    /// Compare-and-swap refresh an existing pending intent
    Refresh {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Stable ID of the original Intent Submit event
        #[arg(long)]
        intent: String,
        /// Replacement summary; use '-' to read from stdin
        #[arg(long)]
        summary: String,
        /// Optional replacement addressed participant
        #[arg(long)]
        addressed_to: Option<String>,
    },
    /// Compare-and-swap withdraw an existing pending intent
    Withdraw {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Stable ID of the original Intent Submit event
        #[arg(long)]
        intent: String,
    },
}

/// Moderated Meeting moderator-control operations.
#[derive(Subcommand)]
pub enum MeetingModeratorCmd {
    /// Select exactly one pending intent or open handoff
    Select {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Pending Intent ID
        #[arg(long)]
        intent: Option<String>,
        /// Open Handoff ID
        #[arg(long)]
        handoff: Option<String>,
        /// Optional short selection explanation
        #[arg(long)]
        reason: Option<String>,
        /// Deferral in INTENT_ID:REASON form; repeatable
        #[arg(long = "defer")]
        deferrals: Vec<String>,
        /// Registered DecisionAttempt ID; required for an Agent moderator
        #[arg(long)]
        attempt: Option<String>,
    },
    /// Reject one pending intent
    Reject {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Stable Intent ID
        #[arg(long)]
        intent: String,
        /// Machine-readable rejection reason
        #[arg(long)]
        reason_code: MeetingIntentRejectionReason,
        /// Required human-readable explanation
        #[arg(long)]
        reason: String,
        /// Registered DecisionAttempt ID; required for an Agent moderator
        #[arg(long)]
        attempt: Option<String>,
    },
    /// Close one unresolved directed handoff
    #[command(name = "dismiss-handoff")]
    DismissHandoff {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Stable Handoff ID
        #[arg(long)]
        handoff: String,
        /// Machine-readable dismissal reason
        #[arg(long)]
        reason_code: MeetingHandoffDismissReason,
        /// Required human-readable explanation
        #[arg(long)]
        reason: String,
        /// Registered DecisionAttempt ID; required for an Agent moderator
        #[arg(long)]
        attempt: Option<String>,
    },
    /// Register a Relay-authoritative Candidate Cohort before model dispatch
    #[command(name = "attempt-start")]
    AttemptStart {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Abandoned attempt replaced without refreshing its deadline
        #[arg(long)]
        replacement: Option<String>,
    },
    /// Terminalize a registered DecisionAttempt without a primary action
    #[command(name = "attempt-finish")]
    AttemptFinish {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Registered DecisionAttempt ID
        #[arg(long)]
        attempt: String,
        /// Completed or discarded terminal class
        #[arg(long)]
        outcome: MeetingDecisionAttemptFinishOutcome,
        /// Closed machine-readable terminal reason
        #[arg(long)]
        reason_code: String,
    },
    /// Consume one Relay-issued selected-source retry ticket
    Retry {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Failed DecisionAttempt ID
        #[arg(long)]
        attempt: String,
        /// One-use retry ticket ID
        #[arg(long)]
        ticket: String,
        /// Failed signed moderator action event ID
        #[arg(long)]
        failed_action: String,
        /// Failed attempt number
        #[arg(long)]
        attempt_number: u64,
    },
    /// Close an empty current Candidate Cohort
    #[command(name = "complete-cohort")]
    CompleteCohort {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Registered DecisionAttempt ID
        #[arg(long)]
        attempt: String,
    },
    /// Mark a running DecisionAttempt abandoned after Runtime loss
    #[command(name = "attempt-abandon")]
    AttemptAbandon {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Registered DecisionAttempt ID
        #[arg(long)]
        attempt: String,
    },
    /// Withdraw the Agent moderator's own Intent through its DecisionAttempt
    #[command(name = "withdraw-self")]
    WithdrawSelf {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Registered DecisionAttempt ID
        #[arg(long)]
        attempt: String,
        /// Stable moderator self-Intent ID
        #[arg(long)]
        intent: String,
    },
    /// Recall control after the current allocation chain
    Recall {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Optional human-readable explanation
        #[arg(long)]
        reason: Option<String>,
    },
}

/// Moderated Meeting Offer response operations.
#[derive(Subcommand)]
pub enum MeetingOfferCmd {
    /// Acknowledge the current Offer
    Ack {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Explicit Offer ID; defaults to the active Offer in State
        #[arg(long)]
        offer: Option<String>,
    },
    /// Decline the current Offer
    Decline {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Explicit Offer ID; defaults to the active Offer in State
        #[arg(long)]
        offer: Option<String>,
        /// Optional short explanation
        #[arg(long)]
        reason: Option<String>,
    },
}

/// Moderated Meeting active Grant operations.
#[derive(Subcommand)]
pub enum MeetingGrantCmd {
    /// Extend the active Grant's soft lease
    Progress {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Observable local execution stage
        #[arg(long)]
        stage: MeetingGrantProgressStage,
        /// Explicit Grant ID; defaults to the active Grant in State
        #[arg(long)]
        grant: Option<String>,
    },
    /// Immediately yield the active Grant
    Yield {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Explicit Grant ID; defaults to the active Grant in State
        #[arg(long)]
        grant: Option<String>,
        /// Optional machine-readable reason
        #[arg(long)]
        reason_code: Option<MeetingGrantYieldReason>,
        /// Optional short explanation
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum MeetingFloorCmd {
    /// Show the highest-revision floor state
    Status {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Read Claim and Round State control history
    History {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Maximum number of control events to return
        #[arg(long, default_value_t = 500)]
        limit: u32,
    },
    /// Request the next available V1 floor slot as a Human participant
    Request {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
    /// Withdraw the current identity's queued/offered V1 Human request
    Withdraw {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Explicit Request ID; defaults to the active request in State
        #[arg(long)]
        request: Option<String>,
    },
    /// Submit one Claim for the current open/claiming round
    Claim {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Wait until this round is granted or advances
        #[arg(long, default_value_t = false)]
        wait: bool,
        /// Maximum seconds to wait for the round result
        #[arg(long, default_value_t = 20)]
        timeout: u64,
    },
    /// Declare that this Agent will resolve one intent basis for the round
    Ready {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Opaque intent basis id, such as speech:<event-id>
        #[arg(long)]
        basis: String,
    },
    /// Complete a previously Ready intent without claiming
    Pass {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
        /// Opaque intent basis id used by the matching Ready
        #[arg(long)]
        basis: String,
    },
    /// Yield the current identity's active Grant and immediately open a new round
    Yield {
        /// Meeting UUID
        #[arg(long)]
        meeting: String,
    },
}

#[derive(Subcommand)]
pub enum CanvasCmd {
    /// Get the canvas document for a channel
    Get {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Set (replace) the canvas document for a channel
    Set {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Canvas content (markdown; use '-' to read from stdin)
        #[arg(long)]
        content: String,
    },
}

#[derive(Subcommand)]
pub enum ReactionsCmd {
    /// Add an emoji reaction to a message
    Add {
        /// Event ID (64-char hex)
        #[arg(long)]
        event: String,
        /// Emoji character (e.g. '👍') or custom emoji shortcode
        #[arg(long)]
        emoji: String,
        /// Image URL for a custom emoji reaction; when set, content becomes `:shortcode:`
        #[arg(long = "emoji-url")]
        emoji_url: Option<String>,
    },
    /// Remove an emoji reaction from a message
    Remove {
        /// Event ID (64-char hex)
        #[arg(long)]
        event: String,
        /// Emoji character to remove
        #[arg(long)]
        emoji: String,
    },
    /// List reactions on a message
    Get {
        /// Event ID (64-char hex)
        #[arg(long)]
        event: String,
    },
}

#[derive(Subcommand)]
pub enum EmojiCmd {
    /// List the workspace custom emoji palette (union of every member's set)
    List,
    /// Add or update a custom emoji in your own set
    Set {
        /// Emoji shortcode, without surrounding colons
        #[arg(long)]
        shortcode: String,
        /// Image URL for the emoji
        #[arg(long)]
        url: String,
    },
    /// Remove a custom emoji from your own set
    Rm {
        /// Emoji shortcode, without surrounding colons
        #[arg(long)]
        shortcode: String,
    },
    /// Export custom emojis to stdout or a file
    Export {
        /// Write JSON to this file path instead of stdout
        #[arg(long)]
        file: Option<String>,
        /// Export your own set (default) or the full workspace palette
        #[arg(long, value_enum, default_value = "own")]
        scope: EmojiScope,
    },
    /// Import custom emojis from stdin or a file into your own set
    Import {
        /// Read JSON from this file path instead of stdin
        #[arg(long)]
        file: Option<String>,
        /// Replace your entire set instead of merging
        #[arg(long, default_value_t = false)]
        replace: bool,
        /// Print what would be published without writing
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum DmsCmd {
    /// List direct message conversations
    List {
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Open a new direct message with one or more users
    Open {
        /// User pubkey(s) to DM (64-char hex, 1-8)
        #[arg(long = "pubkey")]
        pubkeys: Vec<String>,
    },
    /// Add a member to an existing DM conversation
    AddMember {
        /// DM conversation UUID
        #[arg(long)]
        channel: String,
        /// User pubkey to add (64-char hex)
        #[arg(long)]
        pubkey: String,
    },
    /// Hide a DM conversation from your DM list
    Hide {
        /// DM conversation UUID
        #[arg(long)]
        channel: String,
    },
}

#[derive(Subcommand)]
pub enum UsersCmd {
    /// Look up user profiles by pubkey or name
    Get {
        /// User pubkey(s) to look up (64-char hex). Omit for your own profile
        #[arg(long = "pubkey")]
        pubkeys: Vec<String>,
        /// Search by display name (case-insensitive substring match)
        #[arg(long = "name")]
        name: Option<String>,
    },
    /// Update the current identity's profile
    #[command(name = "set-profile")]
    SetProfile {
        /// Display name
        #[arg(long)]
        name: Option<String>,
        /// Avatar URL
        #[arg(long)]
        avatar: Option<String>,
        /// Bio / about text
        #[arg(long)]
        about: Option<String>,
        /// NIP-05 identifier (e.g. user@example.com)
        #[arg(long)]
        nip05: Option<String>,
    },
    /// Get presence status for users
    Presence {
        /// Comma-separated pubkeys (64-char hex)
        #[arg(long)]
        pubkeys: String,
    },
    /// Set your presence status (online/away/offline)
    #[command(name = "set-presence")]
    SetPresence {
        /// Presence status
        #[arg(long, value_enum)]
        status: PresenceStatus,
    },
}

#[derive(Subcommand)]
pub enum WorkflowsCmd {
    /// List workflows in a channel
    List {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Get details for a single workflow
    Get {
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
    },
    /// Create a workflow from a YAML definition
    Create {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Workflow YAML definition
        #[arg(long)]
        yaml: String,
    },
    /// Update a workflow's YAML definition
    Update {
        /// Channel UUID the workflow belongs to
        #[arg(long)]
        channel: String,
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
        /// Updated workflow YAML definition
        #[arg(long)]
        yaml: String,
    },
    /// Delete a workflow
    Delete {
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
    },
    /// Trigger a workflow run
    #[command(
        after_help = "Examples:\n  cf workflows trigger --workflow <UUID>\n  cf workflows trigger --workflow <UUID> --inputs '{\"key\":\"value\"}'"
    )]
    Trigger {
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
        /// JSON object of input variables passed to the workflow as event content
        #[arg(long)]
        inputs: Option<String>,
    },
    /// List runs for a workflow
    Runs {
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Approve or deny a workflow step
    #[command(
        after_help = "Examples:\n  cf workflows approve --token <UUID>\n  cf workflows approve --token <UUID> --approved false --note \"needs revision\""
    )]
    Approve {
        /// The approval token UUID (from the approval request)
        #[arg(long)]
        token: String,
        /// Approve (true) or deny (false) the step
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        approved: bool,
        /// Optional note to include with the approval/denial
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum FeedCmd {
    /// Get recent activity feed entries
    Get {
        /// Unix timestamp — return entries after this time
        #[arg(long)]
        since: Option<i64>,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
        /// Comma-separated feed types to include: mentions, needs_action, activity, agent_activity
        #[arg(long)]
        types: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum SocialCmd {
    /// Publish a text note (NIP-01 kind:1)
    #[command(name = "publish")]
    PublishNote {
        /// Text content of the note.
        #[arg(long)]
        content: String,
        /// 64-char hex event ID to reply to.
        #[arg(long)]
        reply_to: Option<String>,
    },
    /// Set your contact list (NIP-02 kind:3)
    #[command(name = "set-contacts")]
    SetContactList {
        /// JSON array of contacts: [{"pubkey":"hex","relay_url":"...","petname":"..."}]
        #[arg(long)]
        contacts: String,
    },
    /// Get a single event by ID
    #[command(name = "event")]
    GetEvent {
        /// 64-char hex event ID.
        #[arg(long)]
        event: String,
    },
    /// Get recent notes published by a user
    #[command(name = "notes")]
    GetUserNotes {
        /// 64-char hex pubkey of the author.
        #[arg(long)]
        pubkey: String,
        /// Maximum number of notes to return (default 50, max 100).
        #[arg(long)]
        limit: Option<u32>,
        /// Unix timestamp cursor — return notes created before this time.
        #[arg(long)]
        before: Option<i64>,
        /// Event ID cursor — return notes created before this event (composite pagination with --before).
        #[arg(long)]
        before_id: Option<String>,
    },
    /// Get a user's contact list
    #[command(name = "contacts")]
    GetContactList {
        /// 64-char hex pubkey.
        #[arg(long)]
        pubkey: String,
    },
    /// Publish a NIP-51/NIP-65 social list or set.
    #[command(name = "set-list")]
    SetList {
        /// Supported kind: 10000, 10001, 10002, 10003, 30000, or 30003.
        #[arg(long)]
        kind: u16,
        /// JSON array of Nostr tags, e.g. [["p","<hex>"],["d","friends"]].
        #[arg(long)]
        tags: String,
        /// Event content.
        #[arg(long, default_value = "")]
        content: String,
    },
    /// Get NIP-51/NIP-65 social lists or sets by author and kind.
    #[command(name = "list")]
    GetList {
        /// 64-char hex pubkey of the author.
        #[arg(long)]
        pubkey: String,
        /// Supported kind: 10000, 10001, 10002, 10003, 30000, or 30003.
        #[arg(long)]
        kind: u32,
        /// Optional d-tag for parameterized replaceable sets.
        #[arg(long)]
        d_tag: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum NotesCmd {
    /// Create or update a note. Idempotent upsert keyed by `(me, --name)`.
    ///
    /// `published_at` is preserved on edits (only set on first create).
    /// `--title` is required on first create; on subsequent edits the existing
    /// title is carried forward when `--title` is omitted, and `--title ""`
    /// explicitly clears it.
    #[command(
        after_help = "Examples:\n  echo '# Hello' | cf notes set --name hello --title 'Hello' --content -\n  cf notes set --name hello --tag onboarding --content - < draft.md"
    )]
    Set {
        /// Slug — becomes the `d` tag. `[a-z0-9._-]{1,80}`.
        #[arg(long)]
        name: String,
        /// Note title (NIP-23 `title` tag). Required on first create; omit to carry; `""` to clear.
        #[arg(long)]
        title: Option<String>,
        /// Short summary (NIP-23 `summary` tag). Omit to carry; `""` to clear.
        #[arg(long)]
        summary: Option<String>,
        /// Topic tag (NIP-23 `t` tag). May be repeated. Replaces (not merges) existing tags on edit; omit to carry forward.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Clear all `t` tags on update. Mutually exclusive with `--tag`.
        /// Without this and without `--tag`, existing tags are carried forward.
        #[arg(long, default_value_t = false)]
        clear_tags: bool,
        /// Markdown body. Use `-` to read from stdin.
        #[arg(long)]
        content: String,
        /// Allow committing an empty body (refused by default to catch upstream pipeline failures).
        #[arg(long, default_value_t = false)]
        allow_empty: bool,
    },
    /// Read a note by `--naddr` (exact) or `--name <slug>` (cross-author lookup).
    Get {
        /// NIP-19 `naddr1…` or `30023:<pubkey>:<slug>` coordinate. Mutually exclusive with `--name`.
        #[arg(long)]
        naddr: Option<String>,
        /// Slug to look up across authors. Mutually exclusive with `--naddr`.
        #[arg(long)]
        name: Option<String>,
        /// Disambiguate `--name` to a specific author (hex pubkey, display name, or `me`).
        #[arg(long)]
        author: Option<String>,
        /// On an ambiguous `--name` (multiple authors), pick the most recently updated note
        /// instead of erroring. Mutually exclusive with `--author` and `--naddr`.
        #[arg(long, default_value_t = false)]
        latest: bool,
        /// Print only the markdown body, not the full event JSON.
        #[arg(long, default_value_t = false)]
        content_only: bool,
    },
    /// List notes. Defaults to your own.
    Ls {
        /// Hex pubkey, display name, `me`, or `all`.
        #[arg(long, default_value = "me")]
        author: Option<String>,
        /// Filter by NIP-23 `t` tag.
        #[arg(long)]
        tag: Option<String>,
        /// Max results (default 50, hard cap 200).
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Delete one of your own notes via NIP-09 (kind:5).
    ///
    /// Emits an a-tag-only deletion targeting the addressable coordinate
    /// `30023:<pubkey>:<slug>` (no `e` tag — an `e` tag would route around the
    /// relay's coordinate soft-delete and leave the note alive). Read-before-
    /// write gives a clean NotFound when there's nothing to delete.
    Rm {
        /// Slug of the note to delete. Only your own notes can be removed.
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum ReposCmd {
    /// Announce a git repository (NIP-34)
    Create {
        /// Repository identifier: [a-zA-Z0-9._-]{1,64}
        #[arg(long)]
        id: String,
        /// Human-readable display name
        #[arg(long)]
        name: Option<String>,
        /// Repository description
        #[arg(long)]
        description: Option<String>,
        /// Clone URL(s) — can be specified multiple times
        #[arg(long = "clone")]
        clone_urls: Vec<String>,
        /// Web browsing URL
        #[arg(long)]
        web: Option<String>,
        /// Preferred Nostr relay(s) for repo discovery — can be specified multiple times
        #[arg(long = "nostr-relay")]
        relays: Vec<String>,
    },
    /// Get a repository announcement
    Get {
        /// Repository identifier (d-tag)
        #[arg(long)]
        id: String,
        /// Owner pubkey (64-char hex). Omit to match any owner.
        #[arg(long)]
        owner: Option<String>,
    },
    /// List repository announcements
    List {
        /// Owner pubkey (64-char hex). Omit for your repos.
        #[arg(long)]
        owner: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Manage branch and tag protection rules on one of your repositories.
    #[command(subcommand)]
    Protect(ReposProtectCmd),
}

/// Commands for inspecting and changing repository protection rules.
#[derive(Subcommand)]
pub enum ReposProtectCmd {
    /// List the repository's protection rules.
    List {
        /// Repository identifier (d-tag).
        #[arg(long)]
        id: String,
    },
    /// Create or replace the rule for an exact ref pattern.
    Set {
        /// Repository identifier (d-tag).
        #[arg(long)]
        id: String,
        /// Full ref pattern, such as refs/heads/main or refs/heads/*.
        #[arg(long = "ref")]
        ref_pattern: String,
        /// Minimum role allowed to push.
        #[arg(long)]
        push: Option<RepoPushRole>,
        /// Reject non-fast-forward updates.
        #[arg(long, default_value_t = false)]
        no_force_push: bool,
        /// Reject deletion of matching refs.
        #[arg(long, default_value_t = false)]
        no_delete: bool,
        /// Require the NIP-34 patch workflow instead of direct pushes.
        #[arg(long, default_value_t = false)]
        require_patch: bool,
    },
    /// Remove every protection rule for an exact ref pattern.
    Remove {
        /// Repository identifier (d-tag).
        #[arg(long)]
        id: String,
        /// Full ref pattern to remove.
        #[arg(long = "ref")]
        ref_pattern: String,
    },
}

/// Minimum channel role accepted by a repository push rule.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum RepoPushRole {
    /// Repository owner only.
    Owner,
    /// Repository owner or channel admin.
    Admin,
    /// Any channel member.
    Member,
}

#[derive(Subcommand)]
pub enum PatchesCmd {
    /// Send a git patch (NIP-34 kind:1617)
    #[command(
        after_help = "Examples:\n  git format-patch -1 HEAD --stdout | cf patches send --repo-owner <hex> --repo-id myrepo --patch-file - --root\n  cf patches send --repo-owner <hex> --repo-id myrepo --patch-file 0001-fix.patch --reply-to <prev-patch-id>"
    )]
    Send {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Path to a `git format-patch` file, or '-' to read from stdin
        #[arg(long)]
        patch_file: String,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Additional recipient pubkey(s) — can be specified multiple times
        #[arg(long = "to")]
        to: Vec<String>,
        /// Previous patch event id (series) or original root (revision)
        #[arg(long)]
        reply_to: Option<String>,
        /// Mark as the first patch of a new series
        #[arg(long, default_value_t = false)]
        root: bool,
        /// Mark as the first patch of a new revision of an existing series
        #[arg(long, default_value_t = false)]
        root_revision: bool,
        /// Commit ID this patch produces when applied
        #[arg(long)]
        commit: Option<String>,
        /// Parent commit ID
        #[arg(long)]
        parent_commit: Option<String>,
        /// PGP signature of the commit
        #[arg(long)]
        commit_pgp_sig: Option<String>,
        /// Committer identity: 'name|email|timestamp|tz-offset-minutes'
        #[arg(long)]
        committer: Option<String>,
    },
    /// Get a patch by event id
    Get {
        /// Patch event id (64-char hex)
        #[arg(long)]
        event: String,
    },
    /// List patches for a repo
    List {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Filter by patch author pubkey
        #[arg(long)]
        author: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Set status on a patch (open/merged/closed/draft — NIP-34 kind:1630-1633)
    Status {
        /// Root patch event id (first patch of the series/revision)
        #[arg(long)]
        root: String,
        /// New status
        #[arg(long, value_parser = ["open", "merged", "closed", "draft"])]
        status: String,
        /// Markdown context for the status change ('-' to read from stdin)
        #[arg(long)]
        content: Option<String>,
        /// Repo owner pubkey — requires --repo-id
        #[arg(long, requires = "repo_id")]
        repo_owner: Option<String>,
        /// Repo identifier (d-tag) — requires --repo-owner
        #[arg(long, requires = "repo_owner")]
        repo_id: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Root id of the revision that was accepted (status=merged only)
        #[arg(long)]
        revision: Option<String>,
        /// Additional recipient pubkey(s) for the status event (besides the
        /// repo owner, which is tagged automatically when --repo-owner is
        /// given) — e.g. root/revision author. Can be specified multiple times.
        #[arg(long = "to")]
        to: Vec<String>,
        /// Applied patch event id — can be specified multiple times (status=merged only).
        /// Accepts `<id>`, `<id>:<relay-url>`, or `<id>:<relay-url>:<pubkey>`.
        #[arg(long = "q")]
        q: Vec<String>,
        /// Merge commit id (status=merged only)
        #[arg(long)]
        merge_commit: Option<String>,
        /// Commit id applied to the target branch — can be specified multiple times (status=merged only)
        #[arg(long = "applied-as-commit")]
        applied_as_commit: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum PrCmd {
    /// Open a git pull request (NIP-34 kind:1618)
    #[command(
        after_help = "Examples:\n  cf pr open --repo-owner <hex> --repo-id myrepo --subject 'Fix bug' --body-file - --commit $(git rev-parse HEAD) --clone https://relay/git/owner/myrepo --branch-name fix-bug\n  cf pr update --repo-owner <hex> --repo-id myrepo --pr <event> --pr-author <hex> --commit $(git rev-parse HEAD) --clone https://relay/git/owner/myrepo"
    )]
    Open {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Pull request subject/header
        #[arg(long, alias = "title")]
        subject: String,
        /// Pull request body markdown. Use '-' to read from stdin.
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Path to pull request body markdown, or '-' to read from stdin.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<String>,
        /// Tip commit of the PR branch
        #[arg(long)]
        commit: String,
        /// Clone URL where the tip commit can be fetched — can be specified multiple times
        #[arg(long = "clone", required = true)]
        clone: Vec<String>,
        /// Recommended branch name
        #[arg(long)]
        branch_name: Option<String>,
        /// Most recent common ancestor with the target branch
        #[arg(long)]
        merge_base: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Label — can be specified multiple times
        #[arg(long = "label")]
        label: Vec<String>,
        /// Additional recipient pubkey(s) — can be specified multiple times
        #[arg(long = "to")]
        to: Vec<String>,
        /// Channel where this pull request originated (NIP-29 h-tag)
        #[arg(long)]
        channel: Option<String>,
        /// Root patch event id this PR revises
        #[arg(long)]
        revision_of: Option<String>,
    },
    /// Update a git pull request tip (NIP-34 kind:1619)
    Update {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Pull request event id being updated
        #[arg(long)]
        pr: String,
        /// Pull request author's pubkey
        #[arg(long)]
        pr_author: String,
        /// Updated tip commit of the PR branch
        #[arg(long)]
        commit: String,
        /// Clone URL where the updated tip commit can be fetched — can be specified multiple times
        #[arg(long = "clone", required = true)]
        clone: Vec<String>,
        /// Markdown context for the update. Use '-' to read from stdin.
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Path to markdown context for the update, or '-' to read from stdin.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<String>,
        /// Most recent common ancestor with the target branch
        #[arg(long)]
        merge_base: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Additional recipient pubkey(s) — can be specified multiple times
        #[arg(long = "to")]
        to: Vec<String>,
    },
    /// Get a PR by event id
    Get {
        /// PR event id (64-char hex)
        #[arg(long)]
        event: String,
    },
    /// List PRs for a repo
    List {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Filter by PR author pubkey
        #[arg(long)]
        author: Option<String>,
        /// Filter by label
        #[arg(long)]
        label: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Set status on a PR (open/merged/closed/draft — NIP-34 kind:1630-1633)
    Status {
        /// Pull request event id
        #[arg(long)]
        pr: String,
        /// New status
        #[arg(long, value_parser = ["open", "merged", "closed", "draft"])]
        status: String,
        /// Markdown context for the status change. Use '-' to read from stdin.
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Path to markdown context for the status change, or '-' to read from stdin.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<String>,
        /// Repo owner pubkey — requires --repo-id
        #[arg(long, requires = "repo_id")]
        repo_owner: Option<String>,
        /// Repo identifier (d-tag) — requires --repo-owner
        #[arg(long, requires = "repo_owner")]
        repo_id: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Additional recipient pubkey(s) for the status event (besides the
        /// repo owner, which is tagged automatically when --repo-owner is
        /// given) — e.g. PR author/reviewers. Can be specified multiple times.
        #[arg(long = "to")]
        to: Vec<String>,
        /// Merge commit id (status=merged only)
        #[arg(long)]
        merge_commit: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum IssuesCmd {
    /// Create a git issue (NIP-34 kind:1621)
    Create {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Issue title
        #[arg(long, alias = "subject")]
        title: String,
        /// Issue body, markdown. Use '-' to read from stdin.
        #[arg(long)]
        content: String,
        /// Label — can be specified multiple times
        #[arg(long = "label")]
        label: Vec<String>,
        /// Additional recipient pubkey(s) — can be specified multiple times
        #[arg(long = "to")]
        to: Vec<String>,
    },
    /// Get an issue by event id
    Get {
        /// Issue event id (64-char hex)
        #[arg(long)]
        event: String,
    },
    /// List issues for a repo
    List {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Filter by issue author pubkey
        #[arg(long)]
        author: Option<String>,
        /// Filter by label
        #[arg(long)]
        label: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Set status on an issue (open/resolved/closed/draft — NIP-34 kind:1630-1633)
    Status {
        /// Issue event id
        #[arg(long)]
        issue: String,
        /// New status
        #[arg(long, value_parser = ["open", "resolved", "closed", "draft"])]
        status: String,
        /// Markdown context for the status change ('-' to read from stdin)
        #[arg(long)]
        content: Option<String>,
        /// Repo owner pubkey — requires --repo-id
        #[arg(long, requires = "repo_id")]
        repo_owner: Option<String>,
        /// Repo identifier (d-tag) — requires --repo-owner
        #[arg(long, requires = "repo_owner")]
        repo_id: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Additional recipient pubkey(s) for the status event (besides the
        /// repo owner, which is tagged automatically when --repo-owner is
        /// given) — e.g. the issue author. Can be specified multiple times.
        #[arg(long = "to")]
        to: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum UploadCmd {
    /// Upload a file to the relay's Blossom store
    File {
        /// Path to the file to upload
        #[arg(long)]
        file: String,
    },
}

#[derive(Subcommand)]
pub enum MediaCmd {
    /// Download relay media with Blossom get auth
    Get {
        /// Relay media URL or sha256[.ext] path segment
        input: String,
        /// Output path. Omit or use '-' to write raw bytes to stdout.
        #[arg(short, long)]
        output: Option<String>,
    },
}

/// Subcommands for `cf mem`.
#[derive(Subcommand)]
pub enum MemCmd {
    /// List non-tombstoned memory entries
    Ls {
        /// Owner pubkey (hex). Overrides CARRYFORTH_AUTH_TAG.
        #[arg(long)]
        owner: Option<String>,
        /// Agent pubkey (hex) to read as this key's owner.
        #[arg(long)]
        agent: Option<String>,
        /// Emit JSON instead of tab-delimited lines.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Print the value of a slug to stdout (no trailing newline)
    Get {
        slug: String,
        #[arg(long)]
        owner: Option<String>,
        /// Agent pubkey (hex) to read as this key's owner.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Print sha256(value) in hex (use as `--base-hash` for `mem patch`).
    Hash {
        slug: String,
        #[arg(long)]
        owner: Option<String>,
        /// Agent pubkey (hex) to read as this key's owner.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Set a slug's value. Pass `-` to read the value from stdin.
    Set {
        slug: String,
        value: String,
        #[arg(long)]
        owner: Option<String>,
        /// Allow committing an empty value. Without this, a zero-byte stdin
        /// read is rejected to prevent silent data loss from upstream
        /// pipeline failures.
        #[arg(long, default_value_t = false)]
        allow_empty: bool,
    },
    /// Apply a unified diff to a slug's current value (safer than set).
    ///
    /// Reads the diff from stdin or `--patch-file`. Refuses to apply if the
    /// slug has changed since `--base-hash` was captured, and refuses
    /// hunks whose context doesn't match the current value verbatim.
    Patch {
        slug: String,
        /// Read the patch from a file instead of stdin.
        #[arg(long)]
        patch_file: Option<String>,
        /// sha256 hex digest (lowercase) of the value the patch was generated
        /// against. Hashes the exact UTF-8 bytes returned by `cf mem get`,
        /// not normalized lines. Run `cf mem hash <slug>` to capture this
        /// before editing.
        #[arg(long)]
        base_hash: Option<String>,
        /// Skip the base-hash check. Unsafe if concurrent edits are possible —
        /// the patch will be applied against whatever the current value is,
        /// even if another agent rewrote it after the patch was generated.
        #[arg(long, default_value_t = false)]
        no_base_hash: bool,
        /// Echo the input patch + resulting sha256 and exit without writing.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Allow committing an empty result.
        #[arg(long, default_value_t = false)]
        allow_empty: bool,
        #[arg(long)]
        owner: Option<String>,
    },
    /// Publish a tombstone for a slug (cannot be used on `core`).
    Rm {
        slug: String,
        #[arg(long)]
        owner: Option<String>,
    },
}

/// Subcommands for `cf pack`.
#[derive(Subcommand)]
pub enum PackCmd {
    /// Validate a persona pack directory
    Validate {
        /// Path to the pack directory
        path: String,
    },
    /// Inspect a persona pack — show metadata and effective config
    Inspect {
        /// Path to the pack directory
        path: String,
    },
}

/// Community moderation commands.
///
/// The community (tenant) is selected by the relay host in `--relay` /
/// `CARRYFORTH_RELAY_URL` — moderation commands are community-global and carry no
/// channel scope. The signing key must be a community owner/admin; the relay
/// authorizes every command.
#[derive(Subcommand)]
pub enum ModerationCmd {
    /// List reports in the moderation queue (newest first)
    #[command(
        after_help = "Examples:\n  cf moderation reports\n  cf moderation reports --status open --limit 20"
    )]
    Reports {
        /// Filter by status: open | resolved | dismissed | escalated (default: all)
        #[arg(long)]
        status: Option<String>,
        /// Maximum number of reports to return
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Resolve or dismiss a report (kind 9044)
    #[command(
        after_help = "Examples:\n  cf moderation resolve --report <REPORT_EVENT_ID> --status dismissed --action dismiss\n  cf moderation resolve --report <REPORT_EVENT_ID> --status resolved --action ban --reason \"rule 3\""
    )]
    Resolve {
        /// Hex event id of the kind:1984 report being resolved
        #[arg(long)]
        report: String,
        /// Resolution status: resolved | dismissed
        #[arg(long)]
        status: String,
        /// Action taken: delete | kick | ban | timeout | dismiss | escalate
        #[arg(long)]
        action: String,
        /// Optional reason — relayed to the reporter, so keep it tombstone-safe
        #[arg(long)]
        reason: Option<String>,
    },
    /// Ban a member from the community (kind 9040)
    #[command(
        after_help = "Examples:\n  cf moderation ban --pubkey <HEX>\n  cf moderation ban --pubkey <HEX> --expires-in 604800 --reason \"repeated spam\""
    )]
    Ban {
        /// Target member pubkey (hex)
        #[arg(long)]
        pubkey: String,
        /// Ban duration in seconds from now (omit for a permanent ban)
        #[arg(long, conflicts_with = "expires_at")]
        expires_in: Option<u64>,
        /// Absolute ban expiry as a unix timestamp (seconds)
        #[arg(long)]
        expires_at: Option<u64>,
        /// Optional private ban reason (audit only)
        #[arg(long)]
        reason: Option<String>,
    },
    /// Lift a member's ban (kind 9041)
    Unban {
        /// Target member pubkey (hex)
        #[arg(long)]
        pubkey: String,
    },
    /// Time out a member — a write-block, not a disconnect (kind 9042)
    #[command(
        after_help = "Examples:\n  cf moderation timeout --pubkey <HEX> --expires-in 3600\n  cf moderation timeout --pubkey <HEX> --expires-at 1783500000 --reason \"cool off\""
    )]
    Timeout {
        /// Target member pubkey (hex)
        #[arg(long)]
        pubkey: String,
        /// Timeout duration in seconds from now
        #[arg(long, conflicts_with = "expires_at")]
        expires_in: Option<u64>,
        /// Absolute timeout expiry as a unix timestamp (seconds)
        #[arg(long)]
        expires_at: Option<u64>,
        /// Optional private timeout reason (audit only)
        #[arg(long)]
        reason: Option<String>,
    },
    /// Clear a member's timeout early (kind 9043)
    Untimeout {
        /// Target member pubkey (hex)
        #[arg(long)]
        pubkey: String,
    },
    /// List currently-restricted members (active ban or timeout)
    Restricted,
    /// Read the moderation audit trail (newest first)
    Audit {
        /// Maximum number of audit rows to return
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
}

async fn run(cli: Cli) -> Result<(), CliError> {
    let relay_url = client::normalize_relay_url(&cli.relay);

    // Pack commands are local-only — no relay connection needed.
    if let Cmd::Pack(ref sub) = cli.command {
        return match sub {
            PackCmd::Validate { path } => commands::pack::cmd_validate(path),
            PackCmd::Inspect { path } => commands::pack::cmd_inspect(path),
        };
    }

    // Auth: private key is required for all relay operations.
    // The keypair IS the identity — no tokens, no other auth.
    let private_key_str = cli.private_key.ok_or_else(|| {
        CliError::Auth(
            "CARRYFORTH_PRIVATE_KEY is required (use --private-key or set env var)".into(),
        )
    })?;
    let keys = Keys::parse(&private_key_str)
        .map_err(|e| CliError::Key(format!("invalid CARRYFORTH_PRIVATE_KEY: {e}")))?;

    // NIP-OA: parse and verify the auth tag if provided.
    let (auth_tag, auth_tag_json) = match cli.auth_tag {
        Some(ref json) if !json.is_empty() => {
            let tag = buzz_sdk::nip_oa::parse_auth_tag(json)
                .map_err(|e| CliError::Auth(format!("CARRYFORTH_AUTH_TAG is malformed: {e}")))?;
            buzz_sdk::nip_oa::verify_auth_tag(json, &keys.public_key()).map_err(|e| {
                CliError::Auth(format!(
                    "CARRYFORTH_AUTH_TAG verification failed for pubkey {}: {e}",
                    keys.public_key().to_hex()
                ))
            })?;
            (Some(tag), Some(json.clone()))
        }
        _ => (None, None),
    };

    let client = CarryforthClient::new(relay_url, keys, auth_tag, auth_tag_json)?;

    match cli.command {
        Cmd::Agents(sub) => commands::agents::dispatch(sub, &client).await,
        Cmd::Messages(sub) => commands::messages::dispatch(sub, &client, &cli.format).await,
        Cmd::Channels(sub) => commands::channels::dispatch(sub, &client, &cli.format).await,
        Cmd::Meetings(sub) => commands::meetings::dispatch(sub, &client, &cli.format).await,
        Cmd::Canvas(sub) => commands::channels::dispatch_canvas(sub, &client).await,
        Cmd::Reactions(sub) => commands::reactions::dispatch(sub, &client).await,
        Cmd::Emoji(sub) => commands::emoji::dispatch(sub, &client).await,
        Cmd::Dms(sub) => commands::dms::dispatch(sub, &client).await,
        Cmd::Users(sub) => commands::users::dispatch(sub, &client, &cli.format).await,
        Cmd::Workflows(sub) => commands::workflows::dispatch(sub, &client).await,
        Cmd::Feed(sub) => commands::feed::dispatch(sub, &client, &cli.format).await,
        Cmd::Social(sub) => commands::social::dispatch(sub, &client).await,
        Cmd::Notes(sub) => commands::notes::dispatch(sub, &client).await,
        Cmd::Repos(sub) => commands::repos::dispatch(sub, &client).await,
        Cmd::Patches(sub) => commands::patches::dispatch(sub, &client).await,
        Cmd::Issues(sub) => commands::issues::dispatch(sub, &client).await,
        Cmd::Pr(sub) => commands::pr::dispatch(sub, &client).await,
        Cmd::Media(sub) => commands::upload::dispatch_media(sub, &client).await,
        Cmd::Upload(sub) => commands::upload::dispatch(sub, &client).await,
        Cmd::Mem(sub) => commands::mem::dispatch(sub, &client).await,
        Cmd::ProjectView(sub) => commands::project_view::dispatch(sub, &client, &cli.format).await,
        Cmd::Documents(sub) => commands::documents::dispatch(sub, &client, &cli.format).await,
        Cmd::ProjectContext(sub) => {
            commands::project_context::dispatch(sub, &client, &cli.format).await
        }
        Cmd::Resources(sub) => commands::resources::dispatch(sub, &client, &cli.format).await,
        Cmd::Roles(sub) => commands::roles::dispatch(sub, &client, &cli.format).await,
        Cmd::Runtime(sub) => commands::runtime::dispatch(sub, &client).await,
        Cmd::Moderation(sub) => commands::moderation::dispatch(sub, &client, &cli.format).await,
        Cmd::Pack(_) => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};
    use std::ffi::OsString;
    use std::sync::Mutex;

    static CLI_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore(Vec<(&'static str, Option<OsString>)>);

    impl EnvRestore {
        fn capture(names: &[&'static str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    /// Smoke test: CLI definition is valid and parseable.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn project_context_query_and_write_surface_is_parseable() {
        let requirement_a = "10000000-0000-4000-8000-000000000001";
        let requirement_b = "10000000-0000-4000-8000-000000000002";
        let document = "20000000-0000-4000-8000-000000000001";
        let assignment = "30000000-0000-4000-8000-000000000001";
        let runtime = "40000000-0000-4000-8000-000000000001";

        for args in [
            vec![
                "cf",
                "project-context",
                "exact",
                "--coordinate",
                requirement_a,
                "--coordinate",
                requirement_b,
            ],
            vec!["cf", "project-context", "incident", requirement_a],
            vec!["cf", "project-context", "contains-all"],
            vec![
                "cf",
                "project-context",
                "contains-all",
                "--coordinate",
                requirement_a,
            ],
            vec![
                "cf",
                "project-context",
                "attach",
                "--context-document",
                document,
                "--coordinate",
                requirement_a,
                "--coordinate",
                requirement_b,
                "--acting-assignment",
                assignment,
                "--runtime-id",
                runtime,
                "--runtime-epoch",
                "7",
            ],
            vec![
                "cf",
                "project-context",
                "attach",
                "--context-document",
                document,
                "--coordinate",
                requirement_a,
                "--coordinate",
                requirement_b,
            ],
            vec![
                "cf",
                "project-context",
                "detach",
                "--context-document",
                document,
                "--coordinate",
                requirement_a,
                "--coordinate",
                requirement_b,
            ],
        ] {
            Cli::try_parse_from(args).expect("parse Project Context command");
        }
    }

    #[test]
    fn project_context_partial_attribution_reaches_actionable_semantic_validation() {
        let cli = Cli::try_parse_from([
            "cf",
            "project-context",
            "attach",
            "--context-document",
            "20000000-0000-4000-8000-000000000001",
            "--coordinate",
            "requirement:10000000-0000-4000-8000-000000000001",
            "--coordinate",
            "requirement:10000000-0000-4000-8000-000000000002",
            "--acting-assignment",
            "30000000-0000-4000-8000-000000000001",
        ])
        .expect("Clap must preserve a partial tuple for the actionable semantic validator");
        let Cmd::ProjectContext(ProjectContextCmd::Attach { attribution, .. }) = cli.command else {
            panic!("expected Project Context Attach");
        };
        assert!(attribution.acting_assignment_id.is_some());
        assert!(attribution.runtime_id.is_none());
        assert!(attribution.runtime_epoch.is_none());
    }

    #[test]
    fn cli_help_never_renders_current_auth_environment_values() {
        let _lock = CLI_ENV_LOCK.lock().expect("CLI env test lock");
        const RELAY_ENV: &str = "CARRYFORTH_RELAY_URL";
        const PRIVATE_KEY_ENV: &str = "CARRYFORTH_PRIVATE_KEY";
        const AUTH_TAG_ENV: &str = "CARRYFORTH_AUTH_TAG";
        const RELAY_SENTINEL: &str = "https://cf-help-relay-secret.invalid/unique";
        const PRIVATE_KEY_SENTINEL: &str = "cf_help_private_secret_8f305ada";
        const AUTH_TAG_SENTINEL: &str = "cf_help_auth_tag_secret_73b7769b";
        let _restore = EnvRestore::capture(&[RELAY_ENV, PRIVATE_KEY_ENV, AUTH_TAG_ENV]);
        std::env::set_var(RELAY_ENV, RELAY_SENTINEL);
        std::env::set_var(PRIVATE_KEY_ENV, PRIVATE_KEY_SENTINEL);
        std::env::set_var(
            AUTH_TAG_ENV,
            format!(r#"{{"secret":"{AUTH_TAG_SENTINEL}"}}"#),
        );

        let mut root = Cli::command();
        let root_help = root.render_long_help().to_string();

        let mut project_context_command = Cli::command();
        let project_context_help = project_context_command
            .find_subcommand_mut("project-context")
            .expect("Project Context subcommand")
            .render_long_help()
            .to_string();

        let mut attach_command = Cli::command();
        let attach_help = attach_command
            .find_subcommand_mut("project-context")
            .expect("Project Context subcommand")
            .find_subcommand_mut("attach")
            .expect("Project Context Attach subcommand")
            .render_long_help()
            .to_string();

        let invalid_invocation = match Cli::try_parse_from(["cf", "project-context", "attach"]) {
            Ok(_) => panic!("incomplete invocation must fail"),
            Err(error) => error.to_string(),
        };
        let rendered = [
            root_help.as_str(),
            project_context_help.as_str(),
            attach_help.as_str(),
            invalid_invocation.as_str(),
        ]
        .join("\n");

        for env_name in [RELAY_ENV, PRIVATE_KEY_ENV, AUTH_TAG_ENV] {
            assert!(root_help.contains(env_name), "root help omitted {env_name}");
        }
        for secret in [RELAY_SENTINEL, PRIVATE_KEY_SENTINEL, AUTH_TAG_SENTINEL] {
            assert!(
                !rendered.contains(secret),
                "CLI help or usage output exposed sentinel {secret}"
            );
        }
        assert!(attach_help.contains("ordinary Community Context write"));
        assert!(attach_help.contains("requires Assignment and Runtime epoch"));
    }

    #[test]
    fn project_view_role_create_parses_explicit_admin_level() {
        let cli = Cli::try_parse_from([
            "cf",
            "project-view",
            "create",
            "role",
            "--expected-project-revision",
            "7",
            "--data",
            "role.json",
            "--role-level",
            "admin",
        ])
        .expect("parse governed Role create");

        let Cmd::ProjectView(ProjectViewCmd::Create { role_level, .. }) = cli.command else {
            panic!("expected Project View Role create");
        };
        assert!(matches!(role_level, Some(ProjectRoleLevelArg::Admin)));
    }

    #[test]
    fn meeting_create_defaults_to_current_complete_policy() {
        let cli = Cli::try_parse_from([
            "cf",
            "meetings",
            "create",
            "--title",
            "Review",
            "--board",
            "# Goal",
            "--participant",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .expect("parse default Meeting Create");
        let Cmd::Meetings(MeetingsCmd::Create {
            policy, moderator, ..
        }) = cli.command
        else {
            panic!("expected Meeting Create");
        };
        assert_eq!(policy, MeetingPolicy::ModeratedBoardActionsV3);
        assert!(moderator.is_none());
    }

    #[test]
    fn meeting_create_help_exposes_only_the_current_surface() {
        let mut command = Cli::command();
        let create = command
            .find_subcommand_mut("meetings")
            .and_then(|meetings| meetings.find_subcommand_mut("create"))
            .expect("Meeting Create command");
        let help = create.render_long_help().to_string();

        assert!(help.contains(
            "cf meetings create --title \"Design review\" --board - --participant <PUBKEY>"
        ));
        assert!(help.contains("--board"));
        for legacy_surface in [
            "--policy",
            "--moderator",
            "uniform-v0",
            "moderated-baton-v1",
            "moderated-board-v1",
            "moderated-board-actions-v3",
        ] {
            assert!(
                !help.contains(legacy_surface),
                "Meeting Create help leaked internal protocol surface: {legacy_surface}"
            );
        }
    }

    #[test]
    fn meeting_create_parses_explicit_v1_policy_and_moderator() {
        let moderator = "bb".repeat(32);
        let cli = Cli::try_parse_from([
            "cf",
            "meetings",
            "create",
            "--policy",
            "moderated-baton-v1",
            "--moderator",
            &moderator,
            "--title",
            "Review",
            "--participant",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .expect("parse Meeting V1 Create");
        let Cmd::Meetings(MeetingsCmd::Create {
            policy,
            moderator: parsed_moderator,
            ..
        }) = cli.command
        else {
            panic!("expected Meeting Create");
        };
        assert_eq!(policy, MeetingPolicy::ModeratedBatonV1);
        assert_eq!(parsed_moderator.as_deref(), Some(moderator.as_str()));
    }

    #[test]
    fn meeting_create_and_board_get_parse_v2_surface() {
        let participant = "aa".repeat(32);
        let cli = Cli::try_parse_from([
            "cf",
            "meetings",
            "create",
            "--policy",
            "moderated-board-v1",
            "--title",
            "Review",
            "--board",
            "# Goal",
            "--participant",
            &participant,
        ])
        .expect("parse Meeting V2 Create");
        let Cmd::Meetings(MeetingsCmd::Create {
            policy,
            moderator,
            board,
            ..
        }) = cli.command
        else {
            panic!("expected Meeting Create");
        };
        assert_eq!(policy, MeetingPolicy::ModeratedBoardV2);
        assert!(moderator.is_none());
        assert_eq!(board.as_deref(), Some("# Goal"));

        Cli::try_parse_from([
            "cf",
            "meetings",
            "board",
            "get",
            "--meeting",
            "00000000-0000-4000-8000-000000000001",
        ])
        .expect("parse Meeting V2 board get");

        Cli::try_parse_from([
            "cf",
            "meetings",
            "board",
            "update",
            "--meeting",
            "00000000-0000-4000-8000-000000000001",
            "--board",
            "# Updated",
            "--control-epoch",
            "2",
            "--board-window",
            "3",
        ])
        .expect("parse Meeting V2 board update");
        Cli::try_parse_from([
            "cf",
            "meetings",
            "board",
            "unchanged",
            "--meeting",
            "00000000-0000-4000-8000-000000000001",
        ])
        .expect("parse Meeting V2 board unchanged with inferred fences");
        Cli::try_parse_from([
            "cf",
            "meetings",
            "close",
            "--meeting",
            "00000000-0000-4000-8000-000000000001",
        ])
        .expect("parse Meeting V2 close");
        Cli::try_parse_from([
            "cf",
            "meetings",
            "abort",
            "--meeting",
            "00000000-0000-4000-8000-000000000001",
            "--reason-code",
            "goal_unreachable",
            "--reason",
            "Evidence unavailable",
        ])
        .expect("parse Meeting V2 abort");
    }

    #[test]
    fn meeting_actions_policy_and_status_parse_for_staged_delivery() {
        let participant = "aa".repeat(32);
        let cli = Cli::try_parse_from([
            "cf",
            "meetings",
            "create",
            "--policy",
            "moderated-board-actions-v3",
            "--title",
            "Action review",
            "--board",
            "# Goal",
            "--participant",
            &participant,
        ])
        .expect("parse action-capable Meeting V2 Create");
        let Cmd::Meetings(MeetingsCmd::Create { policy, .. }) = cli.command else {
            panic!("expected Meeting Create");
        };
        assert_eq!(policy, MeetingPolicy::ModeratedBoardActionsV3);

        Cli::try_parse_from([
            "cf",
            "meetings",
            "actions",
            "status",
            "--meeting",
            "00000000-0000-4000-8000-000000000001",
        ])
        .expect("parse Meeting V2 action status");
    }

    #[test]
    fn meeting_summary_update_requires_exactly_one_mutation() {
        let meeting = "00000000-0000-4000-8000-000000000001";
        for args in [
            vec![
                "cf",
                "meetings",
                "update",
                "--meeting",
                meeting,
                "--summary",
                "Decision and verified outputs.",
            ],
            vec![
                "cf",
                "meetings",
                "update",
                "--meeting",
                meeting,
                "--clear-summary",
            ],
        ] {
            Cli::try_parse_from(args).expect("parse Meeting summary mutation");
        }

        for args in [
            vec!["cf", "meetings", "update", "--meeting", meeting],
            vec![
                "cf",
                "meetings",
                "update",
                "--meeting",
                meeting,
                "--summary",
                "Decision",
                "--clear-summary",
            ],
        ] {
            assert!(
                Cli::try_parse_from(args).is_err(),
                "ambiguous Meeting summary mutation must fail"
            );
        }
    }

    #[test]
    fn meeting_v1_baton_command_surface_is_parseable() {
        let id = "aa".repeat(32);
        let participant = "bb".repeat(32);
        let cases = vec![
            vec![
                "cf",
                "meetings",
                "intents",
                "list",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
            ],
            vec![
                "cf",
                "meetings",
                "intents",
                "submit",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--summary",
                "A risk",
            ],
            vec![
                "cf",
                "meetings",
                "intents",
                "refresh",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--intent",
                &id,
                "--summary",
                "A newer risk",
            ],
            vec![
                "cf",
                "meetings",
                "intents",
                "withdraw",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--intent",
                &id,
            ],
            vec![
                "cf",
                "meetings",
                "moderator",
                "select",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--intent",
                &id,
            ],
            vec![
                "cf",
                "meetings",
                "moderator",
                "reject",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--intent",
                &id,
                "--reason-code",
                "off_topic",
                "--reason",
                "Not on agenda",
            ],
            vec![
                "cf",
                "meetings",
                "moderator",
                "dismiss-handoff",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--handoff",
                &id,
                "--reason-code",
                "answered_elsewhere",
                "--reason",
                "Already answered",
            ],
            vec![
                "cf",
                "meetings",
                "moderator",
                "recall",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
            ],
            vec![
                "cf",
                "meetings",
                "floor",
                "request",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
            ],
            vec![
                "cf",
                "meetings",
                "floor",
                "withdraw",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--request",
                &id,
            ],
            vec![
                "cf",
                "meetings",
                "offer",
                "ack",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--offer",
                &id,
            ],
            vec![
                "cf",
                "meetings",
                "offer",
                "decline",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--offer",
                &id,
                "--reason",
                "Unavailable",
            ],
            vec![
                "cf",
                "meetings",
                "grant",
                "progress",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--grant",
                &id,
                "--stage",
                "tool_use",
            ],
            vec![
                "cf",
                "meetings",
                "grant",
                "yield",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--grant",
                &id,
                "--reason-code",
                "tool_failure",
            ],
            vec![
                "cf",
                "meetings",
                "say",
                "--meeting",
                "00000000-0000-4000-8000-000000000001",
                "--content",
                "Answer",
                "--mention",
                &participant,
                "--handoff-to",
                &participant,
                "--handoff-type",
                "question",
                "--handoff-reason",
                "Please answer",
            ],
        ];
        for args in cases {
            Cli::try_parse_from(args).expect("parse Meeting V1 Baton command");
        }
    }

    #[test]
    fn command_inventory_is_stable() {
        let expected_groups: Vec<&str> = vec![
            "agents",
            "canvas",
            "channels",
            "dms",
            "documents",
            "emoji",
            "feed",
            "issues",
            "media",
            "meetings",
            "mem",
            "messages",
            "moderation",
            "notes",
            "pack",
            "patches",
            "pr",
            "project-context",
            "project-view",
            "reactions",
            "repos",
            "resources",
            "roles",
            "runtime",
            "social",
            "upload",
            "users",
            "workflows",
        ];

        let cmd = Cli::command();
        let mut actual: Vec<String> = cmd
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .filter(|n| n != "help")
            .collect();
        actual.sort();

        assert_eq!(
            actual.len(),
            expected_groups.len(),
            "Expected {} groups, got {}. Actual: {:?}",
            expected_groups.len(),
            actual.len(),
            actual
        );
        assert_eq!(
            actual, expected_groups,
            "Command group inventory drift detected"
        );
    }

    #[test]
    fn subcommand_names_are_stable() {
        fn names(cmd: &clap::Command, group: &str) -> Vec<String> {
            let group_cmd = cmd
                .get_subcommands()
                .find(|s| s.get_name() == group)
                .unwrap_or_else(|| panic!("group '{}' not found", group));
            let mut names: Vec<String> = group_cmd
                .get_subcommands()
                .map(|s| s.get_name().to_string())
                .filter(|n| n != "help")
                .collect();
            names.sort();
            names
        }

        let cmd = Cli::command();
        assert_eq!(
            names(&cmd, "agents"),
            vec![
                "archive",
                "archived",
                "draft-create",
                "draft-update",
                "unarchive"
            ]
        );
        assert_eq!(
            names(&cmd, "messages"),
            vec![
                "delete",
                "edit",
                "get",
                "search",
                "send",
                "send-diff",
                "thread",
                "vote"
            ]
        );
        assert_eq!(
            names(&cmd, "project-view"),
            vec![
                "context",
                "create",
                "delete",
                "get",
                "get-object",
                "init-v3",
                "update",
                "v3"
            ]
        );
        assert_eq!(names(&cmd, "resources"), vec!["guide"]);
        assert_eq!(
            names(&cmd, "project-context"),
            vec!["attach", "contains-all", "detach", "exact", "incident"]
        );
        let project_view = cmd
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "project-view")
            .expect("project-view command");
        let v3 = project_view
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "v3")
            .expect("project-view v3 command");
        let resources = v3
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "resources")
            .expect("project-view v3 resources command");
        let mut resource_review_names = resources
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_owned())
            .filter(|name| name != "help")
            .collect::<Vec<_>>();
        resource_review_names.sort();
        assert_eq!(resource_review_names, vec!["approve"]);
        assert_eq!(
            names(&cmd, "roles"),
            vec![
                "assignment",
                "brief",
                "checkpoint",
                "current",
                "get",
                "handoff",
                "list",
                "offer",
                "proposal",
                "proposals",
                "request",
                "work"
            ]
        );
        let roles = cmd
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "roles")
            .expect("roles command");
        let work = roles
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "work")
            .expect("roles work command");
        let mut work_names: Vec<String> = work
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_string())
            .filter(|name| name != "help")
            .collect();
        work_names.sort();
        assert_eq!(
            work_names,
            vec!["accept", "assign", "recommit", "release", "unassign"]
        );
        for history_command in ["checkpoint", "handoff"] {
            let history = roles
                .get_subcommands()
                .find(|subcommand| subcommand.get_name() == history_command)
                .unwrap_or_else(|| panic!("roles {history_command} command"));
            let mut history_names = history
                .get_subcommands()
                .map(|subcommand| subcommand.get_name().to_owned())
                .filter(|name| name != "help")
                .collect::<Vec<_>>();
            history_names.sort();
            assert_eq!(history_names, vec!["append", "list"]);
        }
        assert_eq!(
            names(&cmd, "channels"),
            vec![
                "add-member",
                "archive",
                "create",
                "delete",
                "get",
                "join",
                "leave",
                "list",
                "members",
                "purpose",
                "remove-member",
                "search",
                "set-add-policy",
                "topic",
                "unarchive",
                "update"
            ]
        );
        assert_eq!(names(&cmd, "canvas"), vec!["get", "set"]);
        assert_eq!(
            names(&cmd, "meetings"),
            vec![
                "abort",
                "actions",
                "board",
                "close",
                "create",
                "end",
                "floor",
                "grant",
                "history",
                "intents",
                "list",
                "moderator",
                "offer",
                "participants",
                "say",
                "show",
                "update"
            ]
        );
        assert_eq!(names(&cmd, "reactions"), vec!["add", "get", "remove"]);
        assert_eq!(
            names(&cmd, "emoji"),
            vec!["export", "import", "list", "rm", "set"]
        );
        assert_eq!(
            names(&cmd, "dms"),
            vec!["add-member", "hide", "list", "open"]
        );
        assert_eq!(
            names(&cmd, "users"),
            vec!["get", "presence", "set-presence", "set-profile"]
        );
        assert_eq!(
            names(&cmd, "workflows"),
            vec!["approve", "create", "delete", "get", "list", "runs", "trigger", "update"]
        );
        assert_eq!(names(&cmd, "feed"), vec!["get"]);
        assert_eq!(
            names(&cmd, "social"),
            vec![
                "contacts",
                "event",
                "list",
                "notes",
                "publish",
                "set-contacts",
                "set-list"
            ]
        );
        assert_eq!(
            names(&cmd, "repos"),
            vec!["create", "get", "list", "protect"]
        );
        let repos = cmd
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "repos")
            .expect("repos command");
        let protect = repos
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "protect")
            .expect("repos protect command");
        let mut protect_names: Vec<String> = protect
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_string())
            .filter(|name| name != "help")
            .collect();
        protect_names.sort();
        assert_eq!(protect_names, vec!["list", "remove", "set"]);
        assert_eq!(
            names(&cmd, "pr"),
            vec!["get", "list", "open", "status", "update"]
        );
        assert_eq!(
            names(&cmd, "patches"),
            vec!["get", "list", "send", "status"]
        );
        assert_eq!(
            names(&cmd, "issues"),
            vec!["create", "get", "list", "status"]
        );
        assert_eq!(names(&cmd, "media"), vec!["get"]);
        assert_eq!(names(&cmd, "upload"), vec!["file"]);
        assert_eq!(names(&cmd, "pack"), vec!["inspect", "validate"]);
        assert_eq!(
            names(&cmd, "moderation"),
            vec![
                "audit",
                "ban",
                "reports",
                "resolve",
                "restricted",
                "timeout",
                "unban",
                "untimeout"
            ]
        );
    }

    #[test]
    fn subcommand_counts_are_stable() {
        let expected: Vec<(&str, usize)> = vec![
            ("agents", 5),
            ("canvas", 2),
            ("channels", 16),
            ("dms", 4),
            ("emoji", 5),
            ("feed", 1),
            ("issues", 4),
            ("media", 1),
            ("meetings", 17),
            ("messages", 8),
            ("pack", 2),
            ("patches", 4),
            ("pr", 5),
            ("project-view", 8),
            ("reactions", 3),
            ("repos", 4),
            ("resources", 1),
            ("roles", 12),
            ("social", 7),
            ("upload", 1),
            ("users", 4),
            ("workflows", 8),
        ];

        let cmd = Cli::command();
        for (group_name, expected_count) in &expected {
            let group = cmd
                .get_subcommands()
                .find(|s| s.get_name() == *group_name)
                .unwrap_or_else(|| panic!("group '{}' not found", group_name));
            let actual_count = group
                .get_subcommands()
                .filter(|s| s.get_name() != "help")
                .count();
            assert_eq!(
                actual_count, *expected_count,
                "Group '{}': expected {} subcommands, got {}",
                group_name, expected_count, actual_count
            );
        }
    }
}
