//! In-memory ports for engine tests.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use charet_model::{
    Branch, BranchStatus, Comment, CommentStatus, Project, Reply, Role, User, Version,
};

use crate::{
    Accounts, Author, BranchEvent, Build, BuildState, BuiltFile, Capture, Captures, Change,
    ChangeKind, Comments, Engine, FileChange, Merged, NewBranch, NewComment, NewReply, NewUser,
    NewVersion, Ports, PreviewSettings, ProjectStore, Records, RenderJob, Renderer, SessionRow,
    Settings, Shot, StoreError, Workspace, WorkspaceError,
};

/// The fakes behind an engine, for tests that steer or inspect them.
#[derive(Clone)]
pub struct Fakes {
    pub workspace: Arc<MemoryWorkspace>,
    pub renderer: Arc<MemoryRenderer>,
    pub realtime: Arc<MemoryRealtime>,
    pub notices: Arc<MemoryNotices>,
    pub sender: Arc<MemorySender>,
    pub agents: Arc<MemoryAgents>,
    pub runner: Arc<FakeRunner>,
    pub net: Arc<FakeNet>,
}

pub fn engine() -> Engine {
    engine_with(&fakes())
}

pub fn engine_with_preview(settings: PreviewSettings) -> (Engine, Arc<MemoryWorkspace>) {
    let fakes = fakes();
    (engine_with(&fakes).with_preview(settings), fakes.workspace)
}

/// An engine with previews on (`render_origin` = `http://render.internal`).
pub fn engine_with_fakes() -> (Engine, Fakes) {
    let fakes = fakes();
    let engine = engine_with(&fakes).with_preview(PreviewSettings {
        origin: "http://preview.test".into(),
        render_origin: "http://render.internal".into(),
        secret: [1; 32],
        ttl_secs: 600,
    });
    (engine, fakes)
}

/// An engine with the web agent on: a fake runner, a Claude key from the environment and the
/// script agent.
pub fn engine_with_agents() -> (Engine, Fakes) {
    let fakes = fakes();
    let engine = engine_with(&fakes)
        .with_agents(
            fakes.runner.clone(),
            crate::AgentConfig {
                env_keys: HashMap::from([("claude".to_owned(), "sk-test-abcd".to_owned())]),
                script: true,
                max_running: 2,
                run_minutes: 30,
            },
        )
        .with_providers(fakes.net.clone());
    (engine, fakes)
}

pub fn file(path: &str, content: &str) -> FileChange {
    FileChange {
        path: path.into(),
        content: Some(content.as_bytes().to_vec()),
        executable: false,
    }
}

fn fakes() -> Fakes {
    Fakes {
        workspace: Arc::new(MemoryWorkspace::default()),
        renderer: Arc::new(MemoryRenderer::default()),
        realtime: Arc::new(MemoryRealtime::default()),
        notices: Arc::new(MemoryNotices::default()),
        sender: Arc::new(MemorySender::default()),
        agents: Arc::new(MemoryAgents::default()),
        runner: Arc::new(FakeRunner::default()),
        net: Arc::new(FakeNet::default()),
    }
}

fn engine_with(fakes: &Fakes) -> Engine {
    Engine::new(
        Ports {
            settings: Arc::new(MemorySettings::default()),
            records: Arc::new(MemoryRecords::default()),
            store: Arc::new(MemoryStore::default()),
            workspace: fakes.workspace.clone(),
            renderer: fakes.renderer.clone(),
            captures: Arc::new(MemoryCaptures::default()),
            comments: Arc::new(MemoryComments::default()),
            accounts: Arc::new(MemoryAccounts::default()),
            realtime: fakes.realtime.clone(),
            notices: fakes.notices.clone(),
            webhooks: fakes.sender.clone(),
            agents: fakes.agents.clone(),
        },
        true,
    )
    .with_secret([7; 32])
}

/// Records jobs. Screens render ready; with an image, the picture is `jpeg:<target>` and the
/// element map points at `src/<target>.tsx:3:5`.
#[derive(Default)]
pub struct MemoryRenderer(
    Mutex<Vec<RenderJob>>,
    Mutex<Option<serde_json::Value>>,
    Mutex<HashMap<String, Vec<String>>>,
);

impl MemoryRenderer {
    pub fn jobs(&self) -> Vec<RenderJob> {
        self.0.lock().unwrap().clone()
    }

    /// Problems every later render of `target` reports (none when empty).
    pub fn set_issues(&self, target: &str, issues: &[&str]) {
        self.2.lock().unwrap().insert(
            target.into(),
            issues.iter().map(|i| (*i).to_owned()).collect(),
        );
    }

    /// The element map of every later render.
    pub fn set_elements(&self, elements: serde_json::Value) {
        *self.1.lock().unwrap() = Some(elements);
    }
}

impl Renderer for MemoryRenderer {
    fn render(&self, job: &RenderJob) -> Result<Shot, WorkspaceError> {
        self.0.lock().unwrap().push(job.clone());
        let target = job
            .url
            .split(['?', '&'])
            .find_map(|p| {
                p.strip_prefix("screen=")
                    .or_else(|| p.strip_prefix("story="))
            })
            .unwrap_or_default()
            .to_owned();
        let issues = self
            .2
            .lock()
            .unwrap()
            .get(&target)
            .cloned()
            .unwrap_or_default();
        Ok(Shot {
            ready: Some("true".into()),
            issues,
            image: job.image.map(|_| format!("jpeg:{target}").into_bytes()),
            elements: job.map.then(|| {
                self.1.lock().unwrap().clone().unwrap_or_else(
                    || serde_json::json!([{ "tag": "main", "src": format!("src/{target}.tsx:3:5") }]),
                )
            }),
        })
    }
}

type CaptureKey = (String, String, String, String);
type StoredCapture = (Capture, Vec<u8>, serde_json::Value);

#[derive(Default)]
struct MemoryCaptures(Mutex<BTreeMap<CaptureKey, StoredCapture>>);

impl Captures for MemoryCaptures {
    fn remove_project(&self, project: &str) -> Result<(), StoreError> {
        self.0.lock().unwrap().retain(|key, _| key.0 != project);
        Ok(())
    }
    fn put(
        &self,
        project: &str,
        capture: &Capture,
        image: &[u8],
        elements: &serde_json::Value,
    ) -> Result<(), StoreError> {
        let key = (
            project.to_owned(),
            capture.kind.clone(),
            capture.target.clone(),
            capture.theme.clone(),
        );
        self.0
            .lock()
            .unwrap()
            .insert(key, (capture.clone(), image.to_vec(), elements.clone()));
        Ok(())
    }
    fn list(&self, project: &str) -> Result<Vec<Capture>, StoreError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.0 == project)
            .map(|(_, v)| v.0.clone())
            .collect())
    }
    fn image(
        &self,
        project: &str,
        kind: &str,
        target: &str,
        theme: &str,
    ) -> Result<Option<Vec<u8>>, StoreError> {
        let key = (project.into(), kind.into(), target.into(), theme.into());
        Ok(self.0.lock().unwrap().get(&key).map(|v| v.1.clone()))
    }
    fn elements(
        &self,
        project: &str,
        kind: &str,
        target: &str,
        theme: &str,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let key = (project.into(), kind.into(), target.into(), theme.into());
        Ok(self.0.lock().unwrap().get(&key).map(|v| v.2.clone()))
    }
}

/// (project, comment, snapshot).
type StoredComment = (String, Comment, Option<Vec<u8>>);

#[derive(Default)]
struct MemoryComments(Mutex<Vec<StoredComment>>);

impl Comments for MemoryComments {
    fn remove_project(&self, project: &str) -> Result<(), StoreError> {
        self.0.lock().unwrap().retain(|(p, _, _)| p != project);
        Ok(())
    }
    fn add(
        &self,
        project: &str,
        new: &NewComment,
        snapshot: Option<&[u8]>,
    ) -> Result<Comment, StoreError> {
        let mut all = self.0.lock().unwrap();
        let comment = Comment {
            id: all.len() as u64 + 1,
            kind: new.kind.clone(),
            target: new.target.clone(),
            theme: new.theme.clone(),
            anchor: new.anchor.clone(),
            body: new.body.clone(),
            author: new.author.clone(),
            status: CommentStatus::Open,
            created_commit: new.commit.clone(),
            resolved_commit: None,
            rev: 1,
            created_at: "now".into(),
            updated_at: "now".into(),
            branch: new.branch,
            replies: Vec::new(),
        };
        all.push((
            project.into(),
            comment.clone(),
            snapshot.map(<[u8]>::to_vec),
        ));
        Ok(comment)
    }
    fn list(&self, project: &str) -> Result<Vec<Comment>, StoreError> {
        let all = self.0.lock().unwrap();
        Ok(all
            .iter()
            .filter(|c| c.0 == project)
            .map(|c| c.1.clone())
            .collect())
    }
    fn get(&self, project: &str, id: u64) -> Result<Option<Comment>, StoreError> {
        Ok(self.list(project)?.into_iter().find(|c| c.id == id))
    }
    fn reply(
        &self,
        project: &str,
        id: u64,
        rev: u32,
        reply: &NewReply,
    ) -> Result<Option<Comment>, StoreError> {
        let mut all = self.0.lock().unwrap();
        let Some((_, comment, _)) = all.iter_mut().find(|c| c.0 == project && c.1.id == id) else {
            return Ok(None);
        };
        if comment.rev != rev {
            return Ok(None);
        }
        comment.rev += 1;
        if let Some(status) = reply.status {
            comment.status = status;
            comment.resolved_commit.clone_from(&reply.resolved_commit);
        }
        comment.replies.push(Reply {
            id: comment.replies.len() as u64 + 1,
            author: reply.author.clone(),
            body: reply.body.clone(),
            kind: reply.kind,
            created_at: "now".into(),
        });
        Ok(Some(comment.clone()))
    }
    fn move_anchor(
        &self,
        project: &str,
        id: u64,
        anchor: &charet_model::Anchor,
    ) -> Result<(), StoreError> {
        let mut all = self.0.lock().unwrap();
        if let Some((_, comment, _)) = all.iter_mut().find(|c| c.0 == project && c.1.id == id) {
            comment.anchor = anchor.clone();
        }
        Ok(())
    }
    fn snapshot(&self, project: &str, id: u64) -> Result<Option<Vec<u8>>, StoreError> {
        let all = self.0.lock().unwrap();
        Ok(all
            .iter()
            .find(|c| c.0 == project && c.1.id == id)
            .and_then(|c| c.2.clone()))
    }
}

/// Builds stay queued until a test calls [`MemoryWorkspace::finish`].
#[derive(Default)]
pub struct MemoryWorkspace {
    builds: Mutex<HashMap<(String, String), Build>>,
    files: Mutex<HashMap<String, Vec<u8>>>,
    starts: Mutex<usize>,
}

impl MemoryWorkspace {
    pub fn starts(&self) -> usize {
        *self.starts.lock().unwrap()
    }

    /// Makes `path` part of every build.
    pub fn serve(&self, path: &str, content: &str) {
        self.files
            .lock()
            .unwrap()
            .insert(path.into(), content.as_bytes().to_vec());
    }

    pub fn finish(&self, state: BuildState) {
        for build in self.builds.lock().unwrap().values_mut() {
            build.state = state;
        }
    }
}

impl Workspace for MemoryWorkspace {
    fn build(&self, project: &str, commit: &str) -> Result<Option<Build>, WorkspaceError> {
        Ok(self
            .builds
            .lock()
            .unwrap()
            .get(&(project.into(), commit.into()))
            .cloned())
    }
    fn start_build(
        &self,
        project: &str,
        commit: &str,
        base: &str,
        _source: Vec<u8>,
        retry: bool,
    ) -> Result<Build, WorkspaceError> {
        let mut builds = self.builds.lock().unwrap();
        let key = (project.to_owned(), commit.to_owned());
        if let Some(build) = builds.get(&key)
            && build.base == base
            && !(retry && build.state == BuildState::Failed)
        {
            return Ok(build.clone());
        }
        *self.starts.lock().unwrap() += 1;
        let build = Build {
            base: base.to_owned(),
            state: BuildState::Queued,
            steps: Vec::new(),
            error: String::new(),
        };
        builds.insert(key, build.clone());
        Ok(build)
    }
    fn file(
        &self,
        _project: &str,
        _commit: &str,
        path: &str,
    ) -> Result<Option<BuiltFile>, WorkspaceError> {
        Ok(self.files.lock().unwrap().get(path).map(|body| BuiltFile {
            content_type: "application/octet-stream".into(),
            cache_control: "no-cache".into(),
            body: body.clone(),
        }))
    }
}

#[derive(Default)]
struct MemorySettings(Mutex<HashMap<String, String>>);

impl Settings for MemorySettings {
    fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
}

#[derive(Default)]
struct MemoryRecords {
    projects: Mutex<Vec<Project>>,
    versions: Mutex<Vec<(String, Version)>>,
    branches: Mutex<Vec<(String, Branch)>>,
    events: Mutex<Vec<(String, u32, BranchEvent)>>,
    members: Mutex<Vec<(String, String)>>,
}

impl Records for MemoryRecords {
    fn update_project(
        &self,
        key: &str,
        title: &str,
        visibility: charet_model::Visibility,
    ) -> Result<(), StoreError> {
        for p in self
            .projects
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|p| p.key == key)
        {
            p.title = title.into();
            p.visibility = visibility;
        }
        Ok(())
    }
    fn set_project_visitors(
        &self,
        key: &str,
        visitors: Option<charet_model::VisitorAccess>,
    ) -> Result<(), StoreError> {
        for p in self
            .projects
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|p| p.key == key)
        {
            p.visitors = visitors;
        }
        Ok(())
    }
    fn members(&self, project: &str) -> Result<Vec<String>, StoreError> {
        Ok(self
            .members
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == project)
            .map(|(_, u)| u.clone())
            .collect())
    }
    fn is_member(&self, project: &str, user: &str) -> Result<bool, StoreError> {
        Ok(self
            .members
            .lock()
            .unwrap()
            .iter()
            .any(|(p, u)| p == project && u == user))
    }
    fn set_member(&self, project: &str, user: &str, member: bool) -> Result<(), StoreError> {
        let mut members = self.members.lock().unwrap();
        members.retain(|(p, u)| !(p == project && u == user));
        if member {
            members.push((project.into(), user.into()));
        }
        Ok(())
    }
    fn delete_project(&self, key: &str) -> Result<bool, StoreError> {
        let mut projects = self.projects.lock().unwrap();
        let before = projects.len();
        projects.retain(|p| p.key != key);
        self.versions.lock().unwrap().retain(|(p, _)| p != key);
        Ok(projects.len() < before)
    }
    fn insert_project(&self, project: &Project) -> Result<bool, StoreError> {
        let mut projects = self.projects.lock().unwrap();
        if projects.iter().any(|p| p.key == project.key) {
            return Ok(false);
        }
        projects.push(project.clone());
        Ok(true)
    }
    fn project(&self, key: &str) -> Result<Option<Project>, StoreError> {
        Ok(self
            .projects
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.key == key)
            .cloned())
    }
    fn projects(&self) -> Result<Vec<Project>, StoreError> {
        Ok(self.projects.lock().unwrap().clone())
    }
    fn add_version(&self, project: &str, new: &NewVersion) -> Result<Version, StoreError> {
        let mut versions = self.versions.lock().unwrap();
        let number =
            u32::try_from(versions.iter().filter(|(p, _)| p == project).count() + 1).unwrap();
        let version = Version {
            number,
            commit: new.commit.clone(),
            kind: new.kind,
            actor: new.actor.clone(),
            note: new.note.clone(),
            branch: new.branch,
            restored: new.restored,
            source_commit: new.source_commit.clone(),
            source_tree: new.source_tree.clone(),
            handoff: None,
            created_at: String::new(),
        };
        versions.push((project.into(), version.clone()));
        Ok(version)
    }
    fn insert_branch(&self, project: &str, new: &NewBranch) -> Result<Branch, StoreError> {
        let mut branches = self.branches.lock().unwrap();
        let number =
            u32::try_from(branches.iter().filter(|(p, _)| p == project).count() + 1).unwrap();
        let branch = Branch {
            number,
            title: new.title.clone(),
            author: new.author.clone(),
            status: BranchStatus::Open,
            base: new.base.clone(),
            head: new.head.clone(),
            conflicts: Vec::new(),
            merged_version: None,
            approved: None,
            check: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        branches.push((project.into(), branch.clone()));
        Ok(branch)
    }
    fn branch(&self, project: &str, number: u32) -> Result<Option<Branch>, StoreError> {
        Ok(self
            .branches
            .lock()
            .unwrap()
            .iter()
            .find(|(p, b)| p == project && b.number == number)
            .map(|(_, b)| b.clone()))
    }
    fn branches(&self, project: &str) -> Result<Vec<Branch>, StoreError> {
        let mut found: Vec<_> = self
            .branches
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == project)
            .map(|(_, b)| b.clone())
            .collect();
        found.reverse();
        Ok(found)
    }
    fn update_branch(&self, project: &str, branch: &Branch) -> Result<(), StoreError> {
        for (_, b) in self
            .branches
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|(p, b)| p == project && b.number == branch.number)
        {
            *b = branch.clone();
        }
        Ok(())
    }
    fn add_branch_event(
        &self,
        project: &str,
        number: u32,
        event: &BranchEvent,
    ) -> Result<(), StoreError> {
        self.events
            .lock()
            .unwrap()
            .push((project.into(), number, event.clone()));
        Ok(())
    }
    fn branch_events(&self, project: &str, number: u32) -> Result<Vec<BranchEvent>, StoreError> {
        Ok(self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, n, _)| p == project && *n == number)
            .map(|(_, _, e)| e.clone())
            .collect())
    }
    fn mark_handoff(
        &self,
        project: &str,
        number: u32,
        commit: &str,
        actor: &str,
    ) -> Result<bool, StoreError> {
        let mut versions = self.versions.lock().unwrap();
        let found = versions
            .iter_mut()
            .find(|(p, v)| p == project && v.number == number);
        Ok(found
            .map(|(_, v)| {
                v.handoff = Some(charet_model::Handoff {
                    commit: commit.into(),
                    actor: actor.into(),
                    at: "now".into(),
                });
            })
            .is_some())
    }
    fn versions(&self, project: &str) -> Result<Vec<Version>, StoreError> {
        let mut found: Vec<_> = self
            .versions
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == project)
            .map(|(_, v)| v.clone())
            .collect();
        found.reverse();
        Ok(found)
    }
}

type Files = BTreeMap<String, Vec<u8>>;

#[derive(Default)]
struct Repo {
    /// Files and parents of each commit.
    commits: HashMap<String, (Files, Vec<String>)>,
    lines: HashMap<String, String>,
    tags: HashMap<u32, String>,
}

impl Repo {
    fn files(&self, commit: &str) -> &Files {
        &self.commits[commit].0
    }

    fn add(&mut self, files: Files, parents: Vec<String>) -> String {
        let id = format!("c{}", self.commits.len() + 1);
        self.commits.insert(id.clone(), (files, parents));
        id
    }

    /// `commit` and everything it comes from, nearest first.
    fn ancestry(&self, commit: &str) -> Vec<String> {
        let mut seen = Vec::new();
        let mut queue = std::collections::VecDeque::from([commit.to_owned()]);
        while let Some(next) = queue.pop_front() {
            if seen.contains(&next) {
                continue;
            }
            queue.extend(self.commits[&next].1.iter().cloned());
            seen.push(next);
        }
        seen
    }

    /// The common ancestor that no other common ancestor comes after.
    fn base(&self, a: &str, b: &str) -> Option<String> {
        let theirs = self.ancestry(b);
        let common: Vec<String> = self
            .ancestry(a)
            .into_iter()
            .filter(|c| theirs.contains(c))
            .collect();
        common
            .iter()
            .find(|c| {
                !common
                    .iter()
                    .any(|d| d != *c && self.ancestry(d).contains(c))
            })
            .cloned()
    }

    /// File-level three-way join of `ours` and `theirs` over `base`.
    fn join(base: &Files, ours: &Files, theirs: &Files) -> Result<Files, Vec<String>> {
        let mut paths: Vec<&String> = base
            .keys()
            .chain(ours.keys())
            .chain(theirs.keys())
            .collect();
        paths.sort();
        paths.dedup();
        let (mut out, mut clashes) = (Files::new(), Vec::new());
        for path in paths {
            let (b, o, t) = (base.get(path), ours.get(path), theirs.get(path));
            let pick = if o == t || t == b {
                o
            } else if o == b {
                t
            } else {
                clashes.push(path.clone());
                continue;
            };
            if let Some(content) = pick {
                out.insert(path.clone(), content.clone());
            }
        }
        if clashes.is_empty() {
            Ok(out)
        } else {
            Err(clashes)
        }
    }
}

#[derive(Default)]
struct MemoryStore(Mutex<HashMap<String, Repo>>);

impl ProjectStore for MemoryStore {
    fn remove(&self, project: &str) -> Result<(), StoreError> {
        self.0.lock().unwrap().remove(project);
        Ok(())
    }
    fn create(&self, project: &str) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .insert(project.into(), Repo::default());
        Ok(())
    }
    fn line_head(&self, project: &str, line: &str) -> Result<Option<String>, StoreError> {
        Ok(self.0.lock().unwrap()[project].lines.get(line).cloned())
    }
    fn commit_line(
        &self,
        project: &str,
        line: &str,
        parent: Option<&str>,
        changes: &[FileChange],
        _author: &Author,
        _message: &str,
    ) -> Result<Option<String>, StoreError> {
        let mut repos = self.0.lock().unwrap();
        let repo = repos.get_mut(project).unwrap();
        if repo.lines.get(line).map(String::as_str) != parent {
            return Ok(None);
        }
        let mut files = parent.map(|p| repo.files(p).clone()).unwrap_or_default();
        for change in changes {
            match &change.content {
                Some(content) => files.insert(change.path.clone(), content.clone()),
                None => files.remove(&change.path),
            };
        }
        let id = repo.add(files, parent.into_iter().map(str::to_owned).collect());
        repo.lines.insert(line.into(), id.clone());
        Ok(Some(id))
    }
    fn commit_detached(
        &self,
        project: &str,
        parent: &str,
        changes: &[FileChange],
        _author: &Author,
        _message: &str,
    ) -> Result<String, StoreError> {
        let mut repos = self.0.lock().unwrap();
        let repo = repos.get_mut(project).unwrap();
        let mut files = repo.files(parent).clone();
        for change in changes {
            match &change.content {
                Some(content) => files.insert(change.path.clone(), content.clone()),
                None => files.remove(&change.path),
            };
        }
        Ok(repo.add(files, vec![parent.to_owned()]))
    }
    fn move_line(
        &self,
        project: &str,
        line: &str,
        new: &str,
        old: Option<&str>,
    ) -> Result<bool, StoreError> {
        let mut repos = self.0.lock().unwrap();
        let repo = repos.get_mut(project).unwrap();
        if repo.lines.get(line).map(String::as_str) != old {
            return Ok(false);
        }
        repo.lines.insert(line.into(), new.into());
        Ok(true)
    }
    fn merge(
        &self,
        project: &str,
        ours: &str,
        theirs: &str,
        _author: &Author,
        _message: &str,
    ) -> Result<Merged, StoreError> {
        let mut repos = self.0.lock().unwrap();
        let repo = repos.get_mut(project).unwrap();
        if repo.ancestry(ours).iter().any(|c| c == theirs) {
            return Ok(Merged::Clean(ours.into()));
        }
        let base = repo
            .base(ours, theirs)
            .map(|b| repo.files(&b).clone())
            .unwrap_or_default();
        match Repo::join(&base, repo.files(ours), repo.files(theirs)) {
            Ok(files) => Ok(Merged::Clean(
                repo.add(files, vec![ours.into(), theirs.into()]),
            )),
            Err(clashes) => Ok(Merged::Conflict(clashes)),
        }
    }
    fn revert(
        &self,
        project: &str,
        onto: &str,
        commit: &str,
        _author: &Author,
        _message: &str,
    ) -> Result<Merged, StoreError> {
        let mut repos = self.0.lock().unwrap();
        let repo = repos.get_mut(project).unwrap();
        let parent = repo.commits[commit].1.first().cloned();
        let before = parent.map(|p| repo.files(&p).clone()).unwrap_or_default();
        match Repo::join(repo.files(commit), repo.files(onto), &before) {
            Ok(files) => Ok(Merged::Clean(repo.add(files, vec![onto.into()]))),
            Err(clashes) => Ok(Merged::Conflict(clashes)),
        }
    }
    fn merge_base(&self, project: &str, a: &str, b: &str) -> Result<Option<String>, StoreError> {
        Ok(self.0.lock().unwrap()[project].base(a, b))
    }
    fn tag_version(&self, project: &str, number: u32, commit: &str) -> Result<(), StoreError> {
        self.0
            .lock()
            .unwrap()
            .get_mut(project)
            .unwrap()
            .tags
            .insert(number, commit.into());
        Ok(())
    }
    fn resolve(&self, project: &str, rev: &str) -> Result<Option<String>, StoreError> {
        let repos = self.0.lock().unwrap();
        let repo = &repos[project];
        if rev == "main" {
            return Ok(repo.lines.get("main").cloned());
        }
        if let Some(number) = rev.strip_prefix('b').filter(|n| n.parse::<u32>().is_ok()) {
            return Ok(repo.lines.get(&format!("b/{number}")).cloned());
        }
        if let Some(number) = rev.strip_prefix('v').and_then(|n| n.parse().ok()) {
            return Ok(repo.tags.get(&number).cloned());
        }
        Ok(repo.commits.contains_key(rev).then(|| rev.to_owned()))
    }
    fn list(&self, project: &str, commit: &str) -> Result<Vec<String>, StoreError> {
        let repos = self.0.lock().unwrap();
        Ok(repos[project].files(commit).keys().cloned().collect())
    }
    fn read(&self, project: &str, commit: &str, path: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let repos = self.0.lock().unwrap();
        Ok(repos[project].files(commit).get(path).cloned())
    }
    fn archive(&self, _project: &str, commit: &str) -> Result<Vec<u8>, StoreError> {
        Ok(commit.as_bytes().to_vec())
    }
    /// One `<kind> <path>` line per change (enough for tests).
    fn patch(
        &self,
        project: &str,
        from: &str,
        to: &str,
        paths: &[String],
    ) -> Result<Vec<u8>, StoreError> {
        let mut out = String::new();
        for change in self.diff(project, from, to)? {
            if paths.is_empty() || paths.contains(&change.path) {
                let _ = writeln!(out, "{:?} {}", change.kind, change.path);
            }
        }
        Ok(out.into_bytes())
    }
    fn diff(&self, project: &str, from: &str, to: &str) -> Result<Vec<Change>, StoreError> {
        let repos = self.0.lock().unwrap();
        let (a, b) = (repos[project].files(from), repos[project].files(to));
        let mut changes = Vec::new();
        for (path, content) in b {
            match a.get(path) {
                None => changes.push(Change {
                    path: path.clone(),
                    kind: ChangeKind::Added,
                }),
                Some(old) if old != content => {
                    changes.push(Change {
                        path: path.clone(),
                        kind: ChangeKind::Modified,
                    });
                }
                Some(_) => {}
            }
        }
        for path in a.keys().filter(|p| !b.contains_key(*p)) {
            changes.push(Change {
                path: path.clone(),
                kind: ChangeKind::Deleted,
            });
        }
        changes.sort_by(|x, y| x.path.cmp(&y.path));
        Ok(changes)
    }
}

/// A public engine (no local door).
pub fn public_engine() -> Engine {
    let mut engine = engine_with(&fakes());
    engine.local_mode = false;
    engine
}

/// Members in memory: (token, user), sessions by hash: (user, expires, replaced).
#[derive(Default)]
pub struct MemoryAccounts {
    users: Mutex<Vec<(String, User)>>,
    sessions: Mutex<HashMap<String, (String, u64, bool)>>,
}

impl Accounts for MemoryAccounts {
    fn user(&self, id: &str) -> Result<Option<User>, StoreError> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .iter()
            .find(|(_, u)| u.id == id)
            .map(|(_, u)| u.clone()))
    }
    fn users(&self) -> Result<Vec<User>, StoreError> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .iter()
            .map(|(_, u)| u.clone())
            .collect())
    }
    fn insert_user(&self, new: &NewUser) -> Result<User, StoreError> {
        let user = User {
            id: new.id.clone(),
            name: new.name.clone(),
            role: new.role,
            created_at: "2026-01-01T00:00:00Z".into(),
            last_used_at: None,
        };
        self.users
            .lock()
            .unwrap()
            .push((new.token.clone(), user.clone()));
        Ok(user)
    }
    fn update_user(&self, id: &str, name: &str, role: Role) -> Result<bool, StoreError> {
        let mut users = self.users.lock().unwrap();
        let Some((_, user)) = users.iter_mut().find(|(_, u)| u.id == id) else {
            return Ok(false);
        };
        user.name = name.to_owned();
        user.role = role;
        Ok(true)
    }
    fn delete_user(&self, id: &str) -> Result<bool, StoreError> {
        let mut users = self.users.lock().unwrap();
        let before = users.len();
        users.retain(|(_, u)| u.id != id);
        self.sessions.lock().unwrap().retain(|_, (u, ..)| u != id);
        Ok(users.len() != before)
    }
    fn user_token(&self, id: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .users
            .lock()
            .unwrap()
            .iter()
            .find(|(_, u)| u.id == id)
            .map(|(t, _)| t.clone()))
    }
    fn set_user_token(&self, id: &str, token: &str) -> Result<bool, StoreError> {
        let mut users = self.users.lock().unwrap();
        let Some((old, _)) = users.iter_mut().find(|(_, u)| u.id == id) else {
            return Ok(false);
        };
        token.clone_into(old);
        Ok(true)
    }
    fn user_by_token(&self, token: &str) -> Result<Option<User>, StoreError> {
        let mut users = self.users.lock().unwrap();
        Ok(users.iter_mut().find(|(t, _)| t == token).map(|(_, u)| {
            u.last_used_at = Some("now".into());
            u.clone()
        }))
    }
    fn insert_session(
        &self,
        token_hash: &str,
        user: &str,
        expires_at: u64,
    ) -> Result<(), StoreError> {
        self.sessions
            .lock()
            .unwrap()
            .insert(token_hash.into(), (user.into(), expires_at, false));
        Ok(())
    }
    fn session(&self, token_hash: &str, now: u64) -> Result<Option<SessionRow>, StoreError> {
        let Some((user, expires, replaced)) =
            self.sessions.lock().unwrap().get(token_hash).cloned()
        else {
            return Ok(None);
        };
        if expires <= now {
            return Ok(None);
        }
        Ok(self.user(&user)?.map(|user| SessionRow { user, replaced }))
    }
    fn replace_sessions(&self, user: &str, except_hash: &str) -> Result<(), StoreError> {
        for (hash, (u, _, replaced)) in self.sessions.lock().unwrap().iter_mut() {
            if u == user && hash != except_hash {
                *replaced = true;
            }
        }
        Ok(())
    }
    fn delete_session(&self, token_hash: &str) -> Result<(), StoreError> {
        self.sessions.lock().unwrap().remove(token_hash);
        Ok(())
    }
    fn delete_sessions(&self, user: &str) -> Result<(), StoreError> {
        self.sessions.lock().unwrap().retain(|_, (u, ..)| u != user);
        Ok(())
    }
}

/// Records what commands publish; rooms are not simulated.
#[derive(Default)]
pub struct MemoryRealtime {
    pub events: Mutex<Vec<(String, String)>>,
    kicked: Mutex<Vec<crate::Kick>>,
}

impl MemoryRealtime {
    pub fn kicks(&self) -> Vec<crate::Kick> {
        self.kicked.lock().unwrap().clone()
    }
}

impl crate::Realtime for MemoryRealtime {
    fn join(&self, _: &str, _: crate::Peer, _: Option<u64>) -> crate::Joined {
        let (_, receiver) = tokio::sync::mpsc::unbounded_channel();
        crate::Joined {
            client: 1,
            hello: "{}".into(),
            receiver,
        }
    }
    fn presence(&self, _: &str, _: u64, _: serde_json::Value) {}
    fn leave(&self, _: &str, _: u64) {}
    fn publish(&self, project: &str, kind: &str, _: serde_json::Value) {
        self.events
            .lock()
            .unwrap()
            .push((project.into(), kind.into()));
    }
    fn publish_admins(&self, kind: &str, _: serde_json::Value) {
        self.events.lock().unwrap().push(("*".into(), kind.into()));
    }
    fn notify(&self, people: &[String]) {
        self.events
            .lock()
            .unwrap()
            .push(("*".into(), format!("notify {}", people.join(","))));
    }
    fn kick(&self, who: crate::Kick) {
        self.kicked.lock().unwrap().push(who.clone());
        self.events
            .lock()
            .unwrap()
            .push(("*".into(), format!("kick {who:?}")));
    }
    fn working(&self, project: &str, person: &charet_model::Person, on: bool) {
        self.events
            .lock()
            .unwrap()
            .push((project.into(), format!("working {} {on}", person.id)));
    }
}

/// Notifications and webhooks in memory.
#[derive(Default)]
pub struct MemoryNotices {
    notifications: Mutex<Vec<(String, charet_model::Notification)>>,
    hooks: Mutex<Vec<(charet_model::Webhook, String)>>,
}

impl crate::Notices for MemoryNotices {
    fn insert_notifications(&self, rows: &[crate::NewNotification]) -> Result<(), StoreError> {
        let mut all = self.notifications.lock().unwrap();
        for row in rows {
            let id = all.len() as u64 + 1;
            all.push((
                row.user.clone(),
                charet_model::Notification {
                    id,
                    kind: row.kind.clone(),
                    project: row.project.clone(),
                    actor: row.actor.clone(),
                    text: row.text.clone(),
                    link: row.link.clone(),
                    data: row.data.clone(),
                    created_at: String::new(),
                    read_at: None,
                },
            ));
        }
        Ok(())
    }
    fn notifications(
        &self,
        user: &str,
        unread_only: bool,
        limit: u32,
    ) -> Result<Vec<charet_model::Notification>, StoreError> {
        Ok(self
            .notifications
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|(u, n)| u == user && (!unread_only || n.read_at.is_none()))
            .take(limit as usize)
            .map(|(_, n)| n.clone())
            .collect())
    }
    fn unread_count(&self, user: &str) -> Result<u32, StoreError> {
        Ok(u32::try_from(self.notifications(user, true, u32::MAX)?.len()).unwrap_or(u32::MAX))
    }
    fn mark_read(&self, user: &str, ids: Option<&[u64]>) -> Result<(), StoreError> {
        for (u, n) in self.notifications.lock().unwrap().iter_mut() {
            if u == user && ids.is_none_or(|ids| ids.contains(&n.id)) {
                n.read_at = Some("now".into());
            }
        }
        Ok(())
    }
    fn webhooks(&self) -> Result<Vec<(charet_model::Webhook, String)>, StoreError> {
        Ok(self.hooks.lock().unwrap().clone())
    }
    fn insert_webhook(
        &self,
        hook: &crate::NewWebhook,
    ) -> Result<charet_model::Webhook, StoreError> {
        let webhook = charet_model::Webhook {
            id: hook.id.clone(),
            url: hook.url.clone(),
            events: hook.events.clone(),
            project: hook.project.clone(),
            secret_last4: hook.secret_last4.clone(),
            created_by: hook.created_by.clone(),
            created_at: String::new(),
            last_status: None,
            last_error: None,
            last_delivery_at: None,
        };
        self.hooks
            .lock()
            .unwrap()
            .push((webhook.clone(), hook.salt.clone()));
        Ok(webhook)
    }
    fn delete_webhook(&self, id: &str) -> Result<bool, StoreError> {
        let mut hooks = self.hooks.lock().unwrap();
        let before = hooks.len();
        hooks.retain(|(h, _)| h.id != id);
        Ok(hooks.len() != before)
    }
    fn set_webhook_salt(&self, id: &str, salt: &str, last4: &str) -> Result<bool, StoreError> {
        let mut hooks = self.hooks.lock().unwrap();
        let Some((hook, old)) = hooks.iter_mut().find(|(h, _)| h.id == id) else {
            return Ok(false);
        };
        salt.clone_into(old);
        last4.clone_into(&mut hook.secret_last4);
        Ok(true)
    }
    fn record_delivery(
        &self,
        id: &str,
        status: Option<u16>,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        if let Some((hook, _)) = self
            .hooks
            .lock()
            .unwrap()
            .iter_mut()
            .find(|(h, _)| h.id == id)
        {
            hook.last_status = status;
            hook.last_error = error.map(str::to_owned);
            hook.last_delivery_at = Some("now".into());
        }
        Ok(())
    }
}

/// One recorded POST: url, headers, body.
pub type Post = (String, Vec<(String, String)>, String);

/// Records webhook POSTs and answers 200.
#[derive(Default)]
pub struct MemorySender(pub Mutex<Vec<Post>>);

impl MemorySender {
    /// Waits up to two seconds for `n` posts (deliveries run on their own threads).
    pub fn wait_for(&self, n: usize) -> Vec<Post> {
        for _ in 0..200 {
            let posts = self.0.lock().unwrap().clone();
            if posts.len() >= n {
                return posts;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        self.0.lock().unwrap().clone()
    }
}

impl crate::WebhookSender for MemorySender {
    fn post(&self, url: &str, headers: &[(&str, String)], body: &[u8]) -> Result<u16, String> {
        self.0.lock().unwrap().push((
            url.to_owned(),
            headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
            String::from_utf8_lossy(body).into_owned(),
        ));
        Ok(200)
    }
}

impl Engine {
    /// Moves `main` from `base` with `changes` as a new version, as a merged branch would. Only
    /// tests skip the branch; people and agents go through one.
    pub(crate) fn push(
        &self,
        actor: &charet_model::Actor,
        key: &str,
        base: &str,
        message: &str,
        changes: &[FileChange],
    ) -> Result<charet_model::Version, crate::EngineError> {
        let commit = self
            .ports
            .store
            .commit_main(
                key,
                Some(base),
                changes,
                &crate::projects::author(actor),
                message,
            )?
            .ok_or(crate::EngineError::Conflict {
                head: String::new(),
            })?;
        let version = self.record_version(
            key,
            actor,
            &crate::NewVersion {
                commit,
                kind: charet_model::VersionKind::Push,
                actor: actor.name().to_owned(),
                note: message.to_owned(),
                branch: None,
                restored: None,
                source_commit: None,
                source_tree: None,
            },
        )?;
        self.ports.realtime.working(key, &actor.person(), false);
        Ok(version)
    }
}

/// Agent runs in memory.
#[derive(Default)]
pub struct MemoryAgents {
    runs: Mutex<Vec<(charet_model::AgentRun, Option<String>)>>,
    messages: Mutex<Vec<charet_model::AgentMessage>>,
    attachments: Mutex<HashMap<u64, Vec<charet_model::Attachment>>>,
}

impl crate::AgentRecords for MemoryAgents {
    fn insert_run(&self, new: &crate::NewRun) -> Result<charet_model::AgentRun, StoreError> {
        let mut runs = self.runs.lock().unwrap();
        let run = charet_model::AgentRun {
            id: runs.len() as u64 + 1,
            project: new.project.clone(),
            branch: new.branch,
            actor: new.actor.clone(),
            agent: new.agent,
            model: new.model.clone(),
            prompt: new.prompt.clone(),
            context: new.context.clone(),
            attachments: u32::try_from(new.attachments.len()).unwrap_or(u32::MAX),
            state: charet_model::RunState::Queued,
            error: None,
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            started_at: None,
            ended_at: None,
            runner_seq: 0,
        };
        runs.push((run.clone(), None));
        drop(runs);
        self.attachments
            .lock()
            .unwrap()
            .insert(run.id, new.attachments.clone());
        self.add_message(run.id, charet_model::MessageKind::User, &new.prompt, None)?;
        Ok(run)
    }

    fn run(&self, id: u64) -> Result<Option<charet_model::AgentRun>, StoreError> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .find(|(r, _)| r.id == id)
            .map(|(r, _)| r.clone()))
    }

    fn runs(
        &self,
        project: &str,
        branch: Option<u32>,
    ) -> Result<Vec<charet_model::AgentRun>, StoreError> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, _)| r.project == project && branch.is_none_or(|b| b == r.branch))
            .map(|(r, _)| r.clone())
            .collect())
    }

    fn open_runs(&self) -> Result<Vec<charet_model::AgentRun>, StoreError> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, _)| !r.state.finished())
            .map(|(r, _)| r.clone())
            .collect())
    }

    fn all_runs(&self) -> Result<Vec<charet_model::AgentRun>, StoreError> {
        let mut runs: Vec<_> = self
            .runs
            .lock()
            .unwrap()
            .iter()
            .map(|(r, _)| r.clone())
            .collect();
        runs.reverse();
        Ok(runs)
    }

    fn update_run(&self, id: u64, update: &crate::RunUpdate) -> Result<(), StoreError> {
        let mut runs = self.runs.lock().unwrap();
        let Some((run, token)) = runs.iter_mut().find(|(r, _)| r.id == id) else {
            return Err(StoreError("no such run".into()));
        };
        if let Some(state) = update.state {
            run.state = state;
        }
        if update.error.is_some() {
            run.error.clone_from(&update.error);
        }
        if let Some(n) = update.tokens_in {
            run.tokens_in = n;
        }
        if let Some(n) = update.tokens_out {
            run.tokens_out = n;
        }
        if update.cost_usd.is_some() {
            run.cost_usd = update.cost_usd;
        }
        if let Some(hash) = &update.token_hash {
            token.clone_from(hash);
        }
        if let Some(seq) = update.runner_seq {
            run.runner_seq = seq;
        }
        if update.started {
            run.started_at = Some("2026-01-01T00:00:01Z".into());
        }
        if update.ended {
            run.ended_at = Some("2026-01-01T00:00:02Z".into());
        }
        Ok(())
    }

    fn run_by_token(&self, hash: &str) -> Result<Option<charet_model::AgentRun>, StoreError> {
        Ok(self
            .runs
            .lock()
            .unwrap()
            .iter()
            .find(|(r, t)| t.as_deref() == Some(hash) && !r.state.finished())
            .map(|(r, _)| r.clone()))
    }

    fn add_message(
        &self,
        run: u64,
        kind: charet_model::MessageKind,
        text: &str,
        tool: Option<&str>,
    ) -> Result<charet_model::AgentMessage, StoreError> {
        let mut messages = self.messages.lock().unwrap();
        let seq = u32::try_from(messages.iter().filter(|m| m.run == run).count()).unwrap() + 1;
        let message = charet_model::AgentMessage {
            run,
            seq,
            kind,
            text: text.into(),
            tool: tool.map(Into::into),
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        messages.push(message.clone());
        Ok(message)
    }

    fn messages(
        &self,
        run: u64,
        after: u32,
    ) -> Result<Vec<charet_model::AgentMessage>, StoreError> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.run == run && m.seq > after)
            .cloned()
            .collect())
    }

    fn attachments(&self, run: u64) -> Result<Vec<charet_model::Attachment>, StoreError> {
        Ok(self
            .attachments
            .lock()
            .unwrap()
            .get(&run)
            .cloned()
            .unwrap_or_default())
    }
}

/// How a fake run ends: state, error, cost.
type Ending = (charet_model::RunState, Option<String>, Option<f64>);

/// A runner the test drives: it records starts and cancels and hands out queued progress.
#[derive(Default)]
pub struct FakeRunner {
    pub started: Mutex<Vec<crate::RunnerStart>>,
    pub canceled: Mutex<Vec<u64>>,
    events: Mutex<HashMap<u64, Vec<crate::RunnerEvent>>>,
    finished: Mutex<HashMap<u64, Ending>>,
    pub down: Mutex<bool>,
}

impl FakeRunner {
    pub fn say(&self, id: u64, kind: charet_model::MessageKind, text: &str) {
        self.events
            .lock()
            .unwrap()
            .entry(id)
            .or_default()
            .push(crate::RunnerEvent {
                kind,
                text: text.into(),
                tool: None,
            });
    }

    pub fn finish(
        &self,
        id: u64,
        state: charet_model::RunState,
        error: Option<&str>,
        cost: Option<f64>,
    ) {
        self.finished
            .lock()
            .unwrap()
            .insert(id, (state, error.map(Into::into), cost));
    }

    pub fn last_start(&self) -> crate::RunnerStart {
        self.started
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a run started")
    }
}

impl crate::AgentRunner for FakeRunner {
    fn start(&self, run: &crate::RunnerStart) -> Result<(), WorkspaceError> {
        if *self.down.lock().unwrap() {
            return Err(WorkspaceError::Unavailable("down".into()));
        }
        self.started.lock().unwrap().push(run.clone());
        Ok(())
    }

    fn poll(&self, id: u64, after: u32) -> Result<Option<crate::RunnerProgress>, WorkspaceError> {
        if !self.started.lock().unwrap().iter().any(|s| s.id == id) {
            return Ok(None);
        }
        let events = self
            .events
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let finished = self.finished.lock().unwrap().get(&id).cloned();
        let next = u32::try_from(events.len()).unwrap();
        Ok(Some(crate::RunnerProgress {
            events: events.into_iter().skip(after as usize).collect(),
            next,
            finished: finished.as_ref().map(|f| f.0),
            error: finished.as_ref().and_then(|f| f.1.clone()),
            tokens_in: 100,
            tokens_out: 20,
            cost_usd: finished.and_then(|f| f.2),
        }))
    }

    fn cancel(&self, id: u64) -> Result<(), WorkspaceError> {
        self.canceled.lock().unwrap().push(id);
        Ok(())
    }

    fn health(&self) -> Result<(), WorkspaceError> {
        if *self.down.lock().unwrap() {
            return Err(WorkspaceError::Unavailable("down".into()));
        }
        Ok(())
    }
}

/// The providers, steered by tests: which keys they refuse, whether a person finished the device
/// sign-in, what a refresh gives.
#[derive(Default)]
pub struct FakeNet {
    refused_keys: Mutex<Vec<String>>,
    device: Mutex<DevicePlan>,
    issued: Mutex<String>,
    refresh_with: Mutex<Option<String>>,
    refresh_refused: Mutex<Option<String>>,
    /// The next refresh answers without a new refresh token.
    keep_refresh_token: Mutex<bool>,
    /// The refresh tokens refreshes were asked with.
    refreshed_with: Mutex<Vec<String>>,
    /// Runs while an exchange is in flight (an admin acting meanwhile).
    during_exchange: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// Starting a device sign-in is refused with this message.
    device_start_refused: Mutex<Option<String>>,
    polls: Mutex<u32>,
    device_starts: Mutex<u32>,
    refreshes: Mutex<u32>,
    exchanges: Mutex<Vec<(String, String)>>,
}

#[derive(Default)]
enum DevicePlan {
    #[default]
    Waiting,
    Finished,
    Failed(String),
}

impl FakeNet {
    /// Keys that start with `prefix` are refused.
    pub fn refuse_keys(&self, prefix: &str) {
        self.refused_keys.lock().unwrap().push(prefix.into());
    }

    /// The person finished the device sign-in; the provider issues `access_token`.
    pub fn finish_device(&self, access_token: &str) {
        *self.device.lock().unwrap() = DevicePlan::Finished;
        *self.issued.lock().unwrap() = access_token.into();
    }

    pub fn fail_device(&self, message: &str) {
        *self.device.lock().unwrap() = DevicePlan::Failed(message.into());
    }

    /// The access token the next refresh issues (default: the one issued at sign-in).
    pub fn next_refresh(&self, access_token: &str) {
        *self.refresh_with.lock().unwrap() = Some(access_token.into());
    }

    /// Starting a `ChatGPT` sign-in fails (the provider cannot be reached).
    pub fn refuse_device_start(&self, message: &str) {
        *self.device_start_refused.lock().unwrap() = Some(message.into());
    }

    /// The next refresh keeps the refresh token it was given (the answer has no new one).
    pub fn keep_refresh_token(&self) {
        *self.keep_refresh_token.lock().unwrap() = true;
    }

    /// What refreshes were asked with, oldest first.
    pub fn refreshed_with(&self) -> Vec<String> {
        self.refreshed_with.lock().unwrap().clone()
    }

    /// `act` runs while the next exchange is in flight, as if an admin did it just then.
    pub fn during_exchange(&self, act: impl Fn() + Send + 'static) {
        *self.during_exchange.lock().unwrap() = Some(Box::new(act));
    }

    fn exchange_in_flight(&self) {
        if let Some(act) = self.during_exchange.lock().unwrap().take() {
            act();
        }
    }

    pub fn refuse_refresh(&self, message: &str) {
        *self.refresh_refused.lock().unwrap() = Some(message.into());
    }

    /// How many times the provider was asked whether the device sign-in was done.
    pub fn polls(&self) -> u32 {
        *self.polls.lock().unwrap()
    }

    pub fn refreshes(&self) -> u32 {
        *self.refreshes.lock().unwrap()
    }

    /// (code, verifier) of each Claude code exchange.
    pub fn anthropic_exchanges(&self) -> Vec<(String, String)> {
        self.exchanges.lock().unwrap().clone()
    }

    fn tokens(access: String, refresh: &str, id_token: bool) -> crate::Tokens {
        crate::Tokens {
            id_token: id_token.then(|| access.clone()),
            access,
            refresh: refresh.into(),
            expires_in: 3600,
        }
    }
}

impl crate::ProviderNet for FakeNet {
    fn check_key(&self, _provider: crate::ProviderId, key: &str) -> crate::KeyCheck {
        if self
            .refused_keys
            .lock()
            .unwrap()
            .iter()
            .any(|prefix| key.starts_with(prefix.as_str()))
        {
            return crate::KeyCheck {
                ok: false,
                reason: Some(crate::KeyProblem::Invalid),
                message: "The provider refused this key.".into(),
                models: None,
            };
        }
        crate::KeyCheck {
            ok: true,
            reason: None,
            message: "The key works.".into(),
            models: Some(12),
        }
    }

    fn openai_device_start(&self) -> Result<crate::DeviceAuth, crate::ProviderError> {
        if let Some(message) = self.device_start_refused.lock().unwrap().take() {
            return Err(crate::ProviderError::Unavailable(message));
        }
        // Every sign-in gets its own id, as the provider's do.
        let mut starts = self.device_starts.lock().unwrap();
        *starts += 1;
        Ok(crate::DeviceAuth {
            device_auth_id: format!("dev-{starts}"),
            user_code: "ABCD-1234".into(),
            interval_secs: 30,
        })
    }

    fn openai_device_poll(
        &self,
        _device: &crate::DeviceAuth,
    ) -> Result<crate::DevicePoll, crate::ProviderError> {
        *self.polls.lock().unwrap() += 1;
        Ok(match &*self.device.lock().unwrap() {
            DevicePlan::Waiting => crate::DevicePoll::Pending,
            DevicePlan::Finished => crate::DevicePoll::Done {
                authorization_code: "auth-code".into(),
                code_verifier: "device-verifier".into(),
            },
            DevicePlan::Failed(message) => crate::DevicePoll::Failed(message.clone()),
        })
    }

    fn openai_exchange(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<crate::Tokens, crate::ProviderError> {
        if (code, verifier, redirect_uri)
            != (
                "auth-code",
                "device-verifier",
                crate::OPENAI_DEVICE_REDIRECT,
            )
        {
            return Err(crate::ProviderError::Rejected("wrong exchange".into()));
        }
        self.exchange_in_flight();
        Ok(Self::tokens(
            self.issued.lock().unwrap().clone(),
            "refresh-1",
            true,
        ))
    }

    fn openai_refresh(&self, refresh_token: &str) -> Result<crate::Tokens, crate::ProviderError> {
        *self.refreshes.lock().unwrap() += 1;
        self.refreshed_with
            .lock()
            .unwrap()
            .push(refresh_token.into());
        if let Some(message) = self.refresh_refused.lock().unwrap().clone() {
            return Err(crate::ProviderError::Rejected(message));
        }
        let access = self
            .refresh_with
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| self.issued.lock().unwrap().clone());
        let keep = std::mem::take(&mut *self.keep_refresh_token.lock().unwrap());
        Ok(Self::tokens(
            access,
            if keep { "" } else { "refresh-2" },
            true,
        ))
    }

    fn anthropic_exchange(
        &self,
        code: &str,
        _state: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<crate::Tokens, crate::ProviderError> {
        if redirect_uri != crate::ANTHROPIC_REDIRECT {
            return Err(crate::ProviderError::Rejected("wrong redirect".into()));
        }
        self.exchanges
            .lock()
            .unwrap()
            .push((code.into(), verifier.into()));
        self.exchange_in_flight();
        Ok(Self::tokens("sk-ant-oat-1".into(), "refresh-1", false))
    }

    fn anthropic_refresh(
        &self,
        _refresh_token: &str,
    ) -> Result<crate::Tokens, crate::ProviderError> {
        *self.refreshes.lock().unwrap() += 1;
        if let Some(message) = self.refresh_refused.lock().unwrap().clone() {
            return Err(crate::ProviderError::Rejected(message));
        }
        Ok(Self::tokens("sk-ant-oat-2".into(), "refresh-2", false))
    }
}
