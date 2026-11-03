//! What the engine needs from storage. Implemented by `charet-db` and `charet-store`.

use charet_model::{Branch, Person, Project, Version, VersionKind};

/// A storage failure, with enough text to log. Callers map it to a 500.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// Settings and other key-value state kept by the database.
pub trait Settings: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, StoreError>;
    fn set(&self, key: &str, value: &str) -> Result<(), StoreError>;
}

/// Projects and versions kept by the database.
pub trait Records: Send + Sync {
    /// Adds a project. Returns false when the key is taken.
    fn insert_project(&self, project: &Project) -> Result<bool, StoreError>;
    fn project(&self, key: &str) -> Result<Option<Project>, StoreError>;
    fn projects(&self) -> Result<Vec<Project>, StoreError>;
    /// Records the next version of `project` (numbers start at 1) and returns it.
    fn add_version(&self, project: &str, version: &NewVersion) -> Result<Version, StoreError>;
    /// Records the next branch of `project` (numbers start at 1) and returns it.
    fn insert_branch(&self, project: &str, branch: &NewBranch) -> Result<Branch, StoreError>;
    fn branch(&self, project: &str, number: u32) -> Result<Option<Branch>, StoreError>;
    /// Newest first.
    fn branches(&self, project: &str) -> Result<Vec<Branch>, StoreError>;
    /// Saves title, status, base, head, conflicts and merged version; stamps `updated_at`.
    fn update_branch(&self, project: &str, branch: &Branch) -> Result<(), StoreError>;
    fn add_branch_event(
        &self,
        project: &str,
        number: u32,
        event: &BranchEvent,
    ) -> Result<(), StoreError>;
    /// Oldest first.
    fn branch_events(&self, project: &str, number: u32) -> Result<Vec<BranchEvent>, StoreError>;
    /// Newest first.
    fn versions(&self, project: &str) -> Result<Vec<Version>, StoreError>;
    /// Marks version `number` as in the product repo at `commit`. False when there is no such
    /// version.
    fn mark_handoff(
        &self,
        project: &str,
        number: u32,
        commit: &str,
        actor: &str,
    ) -> Result<bool, StoreError>;
    /// Removes a project with its versions, captures and comments. False when there was none.
    fn delete_project(&self, key: &str) -> Result<bool, StoreError>;
    /// Renames a project and sets who can see it.
    fn update_project(
        &self,
        key: &str,
        title: &str,
        visibility: charet_model::Visibility,
    ) -> Result<(), StoreError>;
    /// What visitors may do on a project (`None`: the server's setting).
    fn set_project_visitors(
        &self,
        key: &str,
        visitors: Option<charet_model::VisitorAccess>,
    ) -> Result<(), StoreError>;
    /// Members (user ids) of a private project.
    fn members(&self, project: &str) -> Result<Vec<String>, StoreError>;
    fn is_member(&self, project: &str, user: &str) -> Result<bool, StoreError>;
    /// Adds (`true`) or removes a member.
    fn set_member(&self, project: &str, user: &str, member: bool) -> Result<(), StoreError>;
}

/// A version to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewVersion {
    pub commit: String,
    pub kind: VersionKind,
    pub actor: String,
    pub note: String,
    pub branch: Option<u32>,
    pub restored: Option<u32>,
    /// For an import: the product repo commit.
    pub source_commit: Option<String>,
    /// For an import joined with other work: the commit with exactly the repo's files.
    pub source_tree: Option<String>,
}

/// A branch to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBranch {
    pub title: String,
    pub author: Person,
    pub base: String,
    pub head: String,
}

/// One thing that happened on a branch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchEvent {
    pub actor: Person,
    /// `created`, `push`, `review`, `catch_up`, `conflict`, `merge`, `archive`, `reopen`, `duplicate`.
    pub kind: String,
    pub detail: String,
    pub at: String,
}

/// Who a commit is attributed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub email: String,
}

/// One file to write (`content: Some`) or delete (`content: None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub content: Option<Vec<u8>>,
    pub executable: bool,
}

/// A changed path between two commits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// The project store: one git repository per project.
pub trait ProjectStore: Send + Sync {
    /// Creates the empty repository for `project`.
    fn create(&self, project: &str) -> Result<(), StoreError>;
    /// Deletes the project's history; nothing to delete is fine.
    fn remove(&self, project: &str) -> Result<(), StoreError>;
    /// The commit `main` points at, or `None` before the first commit.
    fn head(&self, project: &str) -> Result<Option<String>, StoreError> {
        self.line_head(project, "main")
    }
    /// The commit a line (`main` or `b/<number>`) points at, or `None` when there is none.
    fn line_head(&self, project: &str, line: &str) -> Result<Option<String>, StoreError>;
    /// Commits `changes` on top of `parent` (`None` = first commit) and moves `main` there, but only
    /// if `main` is still at `parent`. Returns the new commit, or `None` when `main` had moved.
    fn commit_main(
        &self,
        project: &str,
        parent: Option<&str>,
        changes: &[FileChange],
        author: &Author,
        message: &str,
    ) -> Result<Option<String>, StoreError> {
        self.commit_line(project, "main", parent, changes, author, message)
    }
    /// [`ProjectStore::commit_main`] for any line: `parent: None` creates the line from nothing.
    fn commit_line(
        &self,
        project: &str,
        line: &str,
        parent: Option<&str>,
        changes: &[FileChange],
        author: &Author,
        message: &str,
    ) -> Result<Option<String>, StoreError>;
    /// Commits `changes` on top of `parent` without moving any line (a commit only builds use).
    fn commit_detached(
        &self,
        project: &str,
        parent: &str,
        changes: &[FileChange],
        author: &Author,
        message: &str,
    ) -> Result<String, StoreError>;
    /// Points `line` at `new` if it is still at `old` (`None` = the line must not exist yet).
    /// False when it had moved.
    fn move_line(
        &self,
        project: &str,
        line: &str,
        new: &str,
        old: Option<&str>,
    ) -> Result<bool, StoreError>;
    /// A commit joining `theirs` into `ours` (parents: ours, theirs), or the files that clashed.
    /// Moves no line. `Clean(ours)` when `theirs` is already in `ours`.
    fn merge(
        &self,
        project: &str,
        ours: &str,
        theirs: &str,
        author: &Author,
        message: &str,
    ) -> Result<Merged, StoreError>;
    /// A commit on `onto` that undoes what `commit` changed against its first parent, or the files
    /// that clashed. Moves no line.
    fn revert(
        &self,
        project: &str,
        onto: &str,
        commit: &str,
        author: &Author,
        message: &str,
    ) -> Result<Merged, StoreError>;
    /// The newest commit both `a` and `b` come from.
    fn merge_base(&self, project: &str, a: &str, b: &str) -> Result<Option<String>, StoreError>;
    /// Marks `commit` as version `number` (tag `v/<number>`).
    fn tag_version(&self, project: &str, number: u32, commit: &str) -> Result<(), StoreError>;
    /// Resolves `main`, `v<number>` or a commit id to a commit id.
    fn resolve(&self, project: &str, rev: &str) -> Result<Option<String>, StoreError>;
    /// Every file path of `commit`, sorted.
    fn list(&self, project: &str, commit: &str) -> Result<Vec<String>, StoreError>;
    /// One file of `commit`, or `None` when it does not exist.
    fn read(&self, project: &str, commit: &str, path: &str) -> Result<Option<Vec<u8>>, StoreError>;
    /// The files of `commit` as a gzipped tar.
    fn archive(&self, project: &str, commit: &str) -> Result<Vec<u8>, StoreError>;
    /// A patch (`git apply` input, binary-safe) from `from` to `to`, limited to `paths` (all when
    /// empty).
    fn patch(
        &self,
        project: &str,
        from: &str,
        to: &str,
        paths: &[String],
    ) -> Result<Vec<u8>, StoreError>;
    /// Paths that differ from `from` to `to`.
    fn diff(&self, project: &str, from: &str, to: &str) -> Result<Vec<Change>, StoreError>;
}

/// What joining two lines of work gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    /// The new commit (or the unchanged one, when there was nothing to join).
    Clean(String),
    /// Files changed on both sides in ways that cannot be joined automatically.
    Conflict(Vec<String>),
}

/// Where a static build of one commit is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildState {
    Queued,
    Running,
    Ready,
    Failed,
}

/// One step of a build (extract, install, typecheck, lint, build).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStep {
    pub name: String,
    /// `ok`, `failed` or `skipped`.
    pub state: String,
    pub duration_ms: u64,
    pub detail: String,
}

/// A static build of one commit, as `charet-workspace` reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Build {
    /// The base path it was built for; a build for another one is made again.
    #[serde(skip)]
    pub base: String,
    pub state: BuildState,
    pub steps: Vec<BuildStep>,
    pub error: String,
}

/// A built file, ready to send to the preview origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltFile {
    pub content_type: String,
    pub cache_control: String,
    pub body: Vec<u8>,
}

/// Why `charet-workspace` could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceError {
    /// Not running or not reachable.
    Unavailable(String),
    /// Answered with something unexpected.
    Failed(String),
}

/// `charet-workspace`: installs, checks and builds commits (`apps/charet-workspace`).
pub trait Workspace: Send + Sync {
    /// The build of `commit`, if one was started.
    fn build(&self, project: &str, commit: &str) -> Result<Option<Build>, WorkspaceError>;
    /// Starts building `commit` from `source` (gzipped tar of its files), served under `base`.
    /// Returns the current build unchanged when one exists, unless it failed and `retry` is set.
    fn start_build(
        &self,
        project: &str,
        commit: &str,
        base: &str,
        source: Vec<u8>,
        retry: bool,
    ) -> Result<Build, WorkspaceError>;
    /// A file of a ready build; `None` when there is no such file.
    fn file(
        &self,
        project: &str,
        commit: &str,
        path: &str,
    ) -> Result<Option<BuiltFile>, WorkspaceError>;
    /// Whether the service answers.
    fn health(&self) -> Result<(), WorkspaceError> {
        Ok(())
    }
}

/// Picture format of a render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    Jpeg,
    Png,
}

/// One page for `charet-render` to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderJob {
    pub url: String,
    pub width: u32,
    pub height: u32,
    /// `None` = check only.
    pub image: Option<ImageKind>,
    /// Collect the element map.
    pub map: bool,
}

/// What `charet-render` saw.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shot {
    /// `data-charet-ready` (`true` / `error`), `None` when the page never got ready.
    pub ready: Option<String>,
    pub issues: Vec<String>,
    pub image: Option<Vec<u8>>,
    /// Element map: `[{x, y, w, h, chain, sel, tag, text, src?, slot?}]`.
    pub elements: Option<serde_json::Value>,
}

/// `charet-render`: Chromium screenshots and checks (`apps/charet-render`).
pub trait Renderer: Send + Sync {
    fn render(&self, job: &RenderJob) -> Result<Shot, WorkspaceError>;
    /// Whether the service answers.
    fn health(&self) -> Result<(), WorkspaceError> {
        Ok(())
    }
}

/// A stored capture of one screen or story in one theme (the latest one).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    /// `screen` or `story`.
    pub kind: String,
    pub target: String,
    /// `light` or `dark`.
    pub theme: String,
    pub commit: String,
    pub width: u32,
    pub height: u32,
    pub issues: Vec<String>,
    /// Source files the rendered elements came from (`data-src`, relative to the app).
    pub sources: Vec<String>,
    pub captured_at: String,
}

/// Where captures live: metadata plus the image and element map files.
pub trait Captures: Send + Sync {
    /// Stores `capture`, replacing the previous one for the same (kind, target, theme).
    fn put(
        &self,
        project: &str,
        capture: &Capture,
        image: &[u8],
        elements: &serde_json::Value,
    ) -> Result<(), StoreError>;
    fn list(&self, project: &str) -> Result<Vec<Capture>, StoreError>;
    fn image(
        &self,
        project: &str,
        kind: &str,
        target: &str,
        theme: &str,
    ) -> Result<Option<Vec<u8>>, StoreError>;
    fn elements(
        &self,
        project: &str,
        kind: &str,
        target: &str,
        theme: &str,
    ) -> Result<Option<serde_json::Value>, StoreError>;
    /// Deletes every capture file of `project` (its rows go with the project).
    fn remove_project(&self, project: &str) -> Result<(), StoreError>;
}

/// A comment to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewComment {
    pub kind: String,
    pub target: String,
    pub theme: String,
    pub anchor: charet_model::Anchor,
    pub body: String,
    pub author: charet_model::Person,
    pub commit: String,
    /// The branch whose screen it is on; `None` = main.
    pub branch: Option<u32>,
}

/// A thread entry to store, with the status it sets (resolve, reopen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReply {
    pub author: charet_model::Person,
    pub body: String,
    pub kind: charet_model::ReplyKind,
    pub status: Option<charet_model::CommentStatus>,
    /// Set with a resolve.
    pub resolved_commit: Option<String>,
}

/// Comments, their threads and the capture snapshot each was made on.
pub trait Comments: Send + Sync {
    fn add(
        &self,
        project: &str,
        comment: &NewComment,
        snapshot: Option<&[u8]>,
    ) -> Result<charet_model::Comment, StoreError>;
    /// Every comment of `project` with its replies, oldest first.
    fn list(&self, project: &str) -> Result<Vec<charet_model::Comment>, StoreError>;
    fn get(&self, project: &str, id: u64) -> Result<Option<charet_model::Comment>, StoreError>;
    /// Adds `reply` when the comment is still at `rev`; returns `None` when it moved on.
    fn reply(
        &self,
        project: &str,
        id: u64,
        rev: u32,
        reply: &NewReply,
    ) -> Result<Option<charet_model::Comment>, StoreError>;
    /// Moves the anchor after a new capture (does not count as a change by a person).
    fn move_anchor(
        &self,
        project: &str,
        id: u64,
        anchor: &charet_model::Anchor,
    ) -> Result<(), StoreError>;
    fn snapshot(&self, project: &str, id: u64) -> Result<Option<Vec<u8>>, StoreError>;
    /// Deletes every snapshot file of `project` (its rows go with the project).
    fn remove_project(&self, project: &str) -> Result<(), StoreError>;
}

/// A member to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub id: String,
    pub name: String,
    pub role: charet_model::Role,
    pub token: String,
}

/// A browser session as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub user: charet_model::User,
    /// The same token came in from another browser since: this one is over.
    pub replaced: bool,
}

/// Members, their tokens and browser sessions, kept by the database. Session tokens arrive
/// hashed; member tokens are kept as they are so admins can hand them over again.
pub trait Accounts: Send + Sync {
    fn user(&self, id: &str) -> Result<Option<charet_model::User>, StoreError>;
    /// Every member, oldest first.
    fn users(&self) -> Result<Vec<charet_model::User>, StoreError>;
    fn insert_user(&self, user: &NewUser) -> Result<charet_model::User, StoreError>;
    /// False when there is no such member.
    fn update_user(
        &self,
        id: &str,
        name: &str,
        role: charet_model::Role,
    ) -> Result<bool, StoreError>;
    /// Removes the member with their sessions. False when there was none.
    fn delete_user(&self, id: &str) -> Result<bool, StoreError>;
    fn user_token(&self, id: &str) -> Result<Option<String>, StoreError>;
    /// Replaces the member's token. False when there is no such member.
    fn set_user_token(&self, id: &str, token: &str) -> Result<bool, StoreError>;
    /// The member a token belongs to; records the use.
    fn user_by_token(&self, token: &str) -> Result<Option<charet_model::User>, StoreError>;
    fn insert_session(
        &self,
        token_hash: &str,
        user: &str,
        expires_at: u64,
    ) -> Result<(), StoreError>;
    /// A session that has not expired at `now`.
    fn session(&self, token_hash: &str, now: u64) -> Result<Option<SessionRow>, StoreError>;
    /// Marks every other live session of `user` replaced.
    fn replace_sessions(&self, user: &str, except_hash: &str) -> Result<(), StoreError>;
    fn delete_session(&self, token_hash: &str) -> Result<(), StoreError>;
    /// Ends every session of `user` (their token changed).
    fn delete_sessions(&self, user: &str) -> Result<(), StoreError>;
}

/// Someone in a project room, as the others see them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    /// This connection (one person can have several tabs).
    pub id: String,
    /// `local`, a user id or `visitor:<id>` (same as comment authors).
    pub person: String,
    pub name: String,
    /// `local`, `user` or `visitor`.
    pub kind: String,
    pub role: charet_model::Role,
}

/// What a connection receives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// A JSON message.
    Text(String),
    /// Close the connection (access was taken away).
    Close,
}

/// A connection that joined a room.
#[derive(Debug)]
pub struct Joined {
    pub client: u64,
    /// The first message: who is here, the latest sequence number and missed events.
    pub hello: String,
    pub receiver: tokio::sync::mpsc::UnboundedReceiver<Outgoing>,
}

/// Whose connections to close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kick {
    /// Every connection of this person (`local`, user id or `visitor:<id>`).
    Person(String),
    /// Every visitor, wherever they are (visitors were turned off).
    Visitors,
}

/// The realtime hub (`charet-collab`): rooms per project, presence relayed as is, events with a
/// sequence number so a reconnecting client can catch up.
pub trait Realtime: Send + Sync {
    fn join(&self, project: &str, peer: Peer, since: Option<u64>) -> Joined;
    /// Relays a presence message (cursor, selection, viewport, chat …) to the others in the room.
    fn presence(&self, project: &str, client: u64, data: serde_json::Value);
    fn leave(&self, project: &str, client: u64);
    /// An event everyone in the project room gets.
    fn publish(&self, project: &str, kind: &str, data: serde_json::Value);
    /// An event only admins get, wherever they are.
    fn publish_admins(&self, kind: &str, data: serde_json::Value);
    /// Tells these people (user ids) their inbox changed, wherever they are connected.
    fn notify(&self, people: &[String]);
    fn kick(&self, who: Kick);
    /// Marks someone working on the project from their own checkout (until they push).
    fn working(&self, project: &str, person: &charet_model::Person, on: bool);
}

/// A notification to store for one member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotification {
    pub user: String,
    pub kind: String,
    pub project: Option<String>,
    pub actor: String,
    pub text: String,
    pub link: String,
    pub data: serde_json::Value,
}

/// A webhook to store. `salt` with the server key gives the signing secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewWebhook {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub project: Option<String>,
    pub salt: String,
    pub secret_last4: String,
    pub created_by: String,
}

/// Inbox notifications and webhooks kept by the database.
pub trait Notices: Send + Sync {
    fn insert_notifications(&self, notifications: &[NewNotification]) -> Result<(), StoreError>;
    /// Newest first, at most `limit`.
    fn notifications(
        &self,
        user: &str,
        unread_only: bool,
        limit: u32,
    ) -> Result<Vec<charet_model::Notification>, StoreError>;
    fn unread_count(&self, user: &str) -> Result<u32, StoreError>;
    /// Marks these (or, with `None`, all) of the user's notifications read.
    fn mark_read(&self, user: &str, ids: Option<&[u64]>) -> Result<(), StoreError>;
    /// Every webhook with its salt.
    fn webhooks(&self) -> Result<Vec<(charet_model::Webhook, String)>, StoreError>;
    fn insert_webhook(&self, hook: &NewWebhook) -> Result<charet_model::Webhook, StoreError>;
    fn delete_webhook(&self, id: &str) -> Result<bool, StoreError>;
    fn set_webhook_salt(&self, id: &str, salt: &str, last4: &str) -> Result<bool, StoreError>;
    /// The outcome of the latest delivery: an HTTP status, or an error when none came back.
    fn record_delivery(
        &self,
        id: &str,
        status: Option<u16>,
        error: Option<&str>,
    ) -> Result<(), StoreError>;
}

/// Sends one webhook POST and returns the HTTP status (`charet-server` over HTTPS).
pub trait WebhookSender: Send + Sync {
    fn post(&self, url: &str, headers: &[(&str, String)], body: &[u8]) -> Result<u16, String>;
}

/// Runs the tunnels that bring people to the remote door (`charet-tunnel`): Tailscale serve and
/// Funnel. Calls may take a few seconds.
pub trait Tunnels: Send + Sync {
    /// The ways this machine can share right now, for the sharing page.
    fn modes(&self) -> Vec<TunnelMode>;
    /// Opens `mode` to the remote door; returns the address people open.
    fn open(&self, mode: crate::ShareMode, options: &TunnelOptions) -> Result<String, String>;
    fn close(&self, mode: crate::ShareMode) -> Result<(), String>;
}

/// What a tunnel needs from the person (ngrok: their authtoken, and a domain of theirs).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TunnelOptions {
    pub authtoken: Option<String>,
    pub domain: Option<String>,
}

impl std::fmt::Debug for TunnelOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelOptions")
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

/// One way to share, as this machine sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelMode {
    pub mode: crate::ShareMode,
    pub available: bool,
    /// The address it would give, or why it cannot be used.
    pub detail: String,
}

/// A run as it is first recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRun {
    pub project: String,
    pub branch: u32,
    pub actor: Person,
    pub agent: charet_model::AgentKind,
    pub model: String,
    pub prompt: String,
    pub context: serde_json::Value,
    pub attachments: Vec<charet_model::Attachment>,
}

/// How a run ended, or where it is.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunUpdate {
    pub state: Option<charet_model::RunState>,
    pub error: Option<String>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub cost_usd: Option<f64>,
    /// Sets (`Some(Some(hash))`) or clears (`Some(None)`) the container's token.
    pub token_hash: Option<Option<String>>,
    pub runner_seq: Option<u32>,
    pub started: bool,
    pub ended: bool,
}

/// Web agent runs and their conversations, kept by the database.
pub trait AgentRecords: Send + Sync {
    /// Records a queued run with the person's prompt as its first message.
    fn insert_run(&self, run: &NewRun) -> Result<charet_model::AgentRun, StoreError>;
    fn run(&self, id: u64) -> Result<Option<charet_model::AgentRun>, StoreError>;
    /// A project's runs, oldest first; only one branch's when given.
    fn runs(
        &self,
        project: &str,
        branch: Option<u32>,
    ) -> Result<Vec<charet_model::AgentRun>, StoreError>;
    /// Runs not finished yet, on every project, oldest first.
    fn open_runs(&self) -> Result<Vec<charet_model::AgentRun>, StoreError>;
    /// Every run, newest first, for the cost table.
    fn all_runs(&self) -> Result<Vec<charet_model::AgentRun>, StoreError>;
    fn update_run(&self, id: u64, update: &RunUpdate) -> Result<(), StoreError>;
    /// The unfinished run holding this token hash.
    fn run_by_token(&self, hash: &str) -> Result<Option<charet_model::AgentRun>, StoreError>;
    /// Appends a message (its `seq` is the next one).
    fn add_message(
        &self,
        run: u64,
        kind: charet_model::MessageKind,
        text: &str,
        tool: Option<&str>,
    ) -> Result<charet_model::AgentMessage, StoreError>;
    /// A run's messages after `after` (0: all).
    fn messages(&self, run: u64, after: u32)
    -> Result<Vec<charet_model::AgentMessage>, StoreError>;
    fn attachments(&self, run: u64) -> Result<Vec<charet_model::Attachment>, StoreError>;
}

/// What the runner needs to start a run.
#[derive(Clone, PartialEq)]
pub struct RunnerStart {
    pub id: u64,
    pub agent: charet_model::AgentKind,
    pub model: String,
    /// The vendor key; empty for the script agent and when `oauth` is set.
    pub api_key: String,
    /// A subscription login to run on instead of a key (its access token, renewed before the run).
    pub oauth: Option<crate::RunLogin>,
    pub project: String,
    pub branch: u32,
    /// The token the container uses at the server's agent door.
    pub token: String,
    pub prompt: String,
    pub context: serde_json::Value,
    pub images: Vec<charet_model::Attachment>,
    /// Earlier turns on this branch, oldest first: (`user` | `assistant`, text).
    pub history: Vec<(String, String)>,
}

impl std::fmt::Debug for RunnerStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerStart")
            .field("id", &self.id)
            .field("agent", &self.agent)
            .finish_non_exhaustive()
    }
}

/// A line the runner saw from the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerEvent {
    pub kind: charet_model::MessageKind,
    pub text: String,
    pub tool: Option<String>,
}

/// What a run has done so far.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunnerProgress {
    /// Events after the `after` asked for, and the count to ask after next time.
    pub events: Vec<RunnerEvent>,
    pub next: u32,
    /// `None` while it runs; then done, failed or canceled.
    pub finished: Option<charet_model::RunState>,
    pub error: Option<String>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: Option<f64>,
}

/// `charet-agent`: runs coding agents in containers (`apps/charet-agent`).
pub trait AgentRunner: Send + Sync {
    fn start(&self, run: &RunnerStart) -> Result<(), WorkspaceError>;
    /// Progress since `after` events; `None` when the runner does not know the run (restarted).
    fn poll(&self, id: u64, after: u32) -> Result<Option<RunnerProgress>, WorkspaceError>;
    fn cancel(&self, id: u64) -> Result<(), WorkspaceError>;
    fn health(&self) -> Result<(), WorkspaceError> {
        Ok(())
    }
}
