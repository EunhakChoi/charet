//! Comments on screens and stories (`docs/domain-stories/03`).
//!
//! A comment points at an element of a capture. When the target is captured again, open comments
//! follow their element by score (see [`reanchor`]); when it is gone they stay where it was, marked
//! detached, with the snapshot they were made on.

use charet_model::{Actor, Anchor, Comment, CommentStatus, ReplyKind};
use serde::Deserialize;

use crate::policy::Command;
use crate::{Engine, EngineError, NewComment, NewReply};

/// Longest comment or reply.
const MAX_BODY: usize = 10_000;

/// Where a new comment points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum At {
    /// A point on the capture: the smallest element under it, or a small box around it.
    Point { x: i64, y: i64 },
    /// An element of the capture's element map, by selector.
    Selector(String),
}

/// Which comments to list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommentFilter {
    /// Comments on this branch's screens; `None` = on main.
    pub branch: Option<u32>,
    pub status: Option<CommentStatus>,
    pub kind: Option<String>,
    pub target: Option<String>,
}

/// One element of an element map, as far as anchoring cares.
#[derive(Debug, Clone, Deserialize)]
struct Element {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    #[serde(default)]
    chain: Vec<String>,
    #[serde(default)]
    sel: String,
    #[serde(default)]
    tag: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    src: Option<String>,
}

impl Element {
    fn anchor(&self) -> Anchor {
        Anchor {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
            chain: self.chain.clone(),
            sel: self.sel.clone(),
            tag: self.tag.clone(),
            text: self.text.clone(),
            src: self.src.clone(),
            detached: false,
        }
    }

    fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

fn elements(map: &serde_json::Value) -> Vec<Element> {
    serde_json::from_value(map.clone()).unwrap_or_default()
}

impl Engine {
    /// Adds a comment on the latest capture of (kind, target, theme).
    #[allow(clippy::too_many_arguments)]
    pub fn add_comment(
        &self,
        actor: &Actor,
        key: &str,
        kind: &str,
        target: &str,
        theme: &str,
        at: &At,
        body: &str,
    ) -> Result<Comment, EngineError> {
        self.comment_on(actor, key, None, kind, target, theme, at, body)
    }

    /// Adds a comment on a branch's screen: it points at a spot of the live preview (branches
    /// have no captures), and is listed with the branch, not on main.
    #[allow(clippy::too_many_arguments)]
    pub fn add_branch_comment(
        &self,
        actor: &Actor,
        key: &str,
        branch: u32,
        kind: &str,
        target: &str,
        theme: &str,
        at: &At,
        body: &str,
    ) -> Result<Comment, EngineError> {
        self.comment_on(actor, key, Some(branch), kind, target, theme, at, body)
    }

    #[allow(clippy::too_many_arguments)]
    fn comment_on(
        &self,
        actor: &Actor,
        key: &str,
        branch: Option<u32>,
        kind: &str,
        target: &str,
        theme: &str,
        at: &At,
        body: &str,
    ) -> Result<Comment, EngineError> {
        let project = self.visible(actor, key, Command::CommentAdd)?;
        let body = check_body(body)?;
        let branch_head = match branch {
            Some(number) => Some(
                self.ports
                    .records
                    .branch(key, number)?
                    .ok_or(EngineError::NotFound("branch"))?
                    .head,
            ),
            None => None,
        };
        if !matches!(kind, "screen" | "story") {
            return Err(EngineError::Invalid(format!(
                "kind must be screen or story, not {kind:?}"
            )));
        }
        if !crate::THEMES.contains(&theme) {
            return Err(EngineError::Invalid(format!("unknown theme {theme:?}")));
        }
        let capture = if branch.is_some() {
            None
        } else {
            self.ports
                .captures
                .list(key)?
                .into_iter()
                .find(|c| c.kind == kind && c.target == target && c.theme == theme)
        };
        let map = match &capture {
            Some(_) => self.ports.captures.elements(key, kind, target, theme)?,
            None => None,
        };
        let map = map.map_or_default(|m| elements(&m));
        let anchor = match at {
            At::Point { x, y } => element_at(&map, *x, *y).map_or_else(
                || Anchor {
                    x: x - 8,
                    y: y - 8,
                    w: 16,
                    h: 16,
                    ..Anchor::default()
                },
                Element::anchor,
            ),
            At::Selector(selector) => map
                .iter()
                .find(|e| &e.sel == selector)
                .map(Element::anchor)
                .ok_or_else(|| {
                    EngineError::Invalid(if capture.is_some() {
                        format!(
                            "no element {selector:?} in the capture of {target}; use a point x,y"
                        )
                    } else {
                        format!("{target} has no capture yet; capture it first or use a point x,y")
                    })
                })?,
        };
        let commit = match (&capture, branch_head) {
            (_, Some(head)) => head,
            (Some(capture), None) => capture.commit.clone(),
            (None, None) => self.resolve(key, "main")?,
        };
        let snapshot = match &capture {
            Some(_) => self.ports.captures.image(key, kind, target, theme)?,
            None => None,
        };
        let comment = NewComment {
            kind: kind.to_owned(),
            target: target.to_owned(),
            theme: theme.to_owned(),
            anchor,
            body,
            author: actor.person(),
            commit,
            branch,
        };
        let comment = self
            .ports
            .comments
            .add(key, &comment, snapshot.as_deref())?;
        self.publish(
            key,
            "comment.added",
            serde_json::json!({ "id": comment.id }),
        );
        self.comment_added(actor, &project, &comment);
        Ok(comment)
    }

    pub fn comments(
        &self,
        actor: &Actor,
        key: &str,
        filter: &CommentFilter,
    ) -> Result<Vec<Comment>, EngineError> {
        self.visible(actor, key, Command::ProjectView)?;
        Ok(self
            .ports
            .comments
            .list(key)?
            .into_iter()
            .filter(|c| c.branch == filter.branch)
            .filter(|c| filter.status.is_none_or(|s| c.status == s))
            .filter(|c| filter.kind.as_ref().is_none_or(|k| &c.kind == k))
            .filter(|c| filter.target.as_ref().is_none_or(|t| &c.target == t))
            .collect())
    }

    pub fn comment(&self, actor: &Actor, key: &str, id: u64) -> Result<Comment, EngineError> {
        self.visible(actor, key, Command::ProjectView)?;
        self.ports
            .comments
            .get(key, id)?
            .ok_or(EngineError::NotFound("comment"))
    }

    /// The capture snapshot the comment was made on (JPEG).
    pub fn comment_snapshot(
        &self,
        actor: &Actor,
        key: &str,
        id: u64,
    ) -> Result<Vec<u8>, EngineError> {
        self.comment(actor, key, id)?;
        self.ports
            .comments
            .snapshot(key, id)?
            .ok_or(EngineError::NotFound("snapshot"))
    }

    /// Replies, resolves or reopens. `rev` is the comment revision the caller saw; when someone
    /// wrote in between, nothing changes and the caller gets [`EngineError::Changed`].
    pub fn answer_comment(
        &self,
        actor: &Actor,
        key: &str,
        id: u64,
        rev: u32,
        kind: ReplyKind,
        body: &str,
    ) -> Result<Comment, EngineError> {
        let project = self.visible(actor, key, Command::CommentAdd)?;
        let current = self
            .ports
            .comments
            .get(key, id)?
            .ok_or(EngineError::NotFound("comment"))?;
        // A resolve or reopen note is optional; a reply needs text.
        let body = if kind != ReplyKind::Reply && body.trim().is_empty() {
            String::new()
        } else {
            check_body(body)?
        };
        let (status, resolved_commit) = match kind {
            ReplyKind::Reply => (None, None),
            ReplyKind::Resolve => {
                if current.status == CommentStatus::Resolved {
                    return Err(EngineError::Invalid(format!("c{id} is already resolved")));
                }
                (
                    Some(CommentStatus::Resolved),
                    Some(self.resolve(key, "main")?),
                )
            }
            ReplyKind::Reopen => {
                if current.status == CommentStatus::Open {
                    return Err(EngineError::Invalid(format!("c{id} is already open")));
                }
                (Some(CommentStatus::Open), None)
            }
        };
        let reply = NewReply {
            author: actor.person(),
            body: body.clone(),
            kind,
            status,
            resolved_commit,
        };
        let comment = self
            .ports
            .comments
            .reply(key, id, rev, &reply)?
            .ok_or(EngineError::Changed(current.rev))?;
        self.publish(key, "comment.updated", serde_json::json!({ "id": id }));
        self.comment_answered(actor, &project, &comment, kind, &body);
        Ok(comment)
    }

    /// After a new capture: moves open comments on it to where their element is now.
    pub(crate) fn reanchor_comments(
        &self,
        key: &str,
        kind: &str,
        target: &str,
        theme: &str,
        map: &serde_json::Value,
    ) -> Result<(), EngineError> {
        let map = elements(map);
        for comment in self.ports.comments.list(key)? {
            if comment.status != CommentStatus::Open
                || comment.branch.is_some()
                || comment.kind != kind
                || comment.target != target
                || comment.theme != theme
            {
                continue;
            }
            let anchor = reanchor(&comment.anchor, &map);
            if anchor != comment.anchor {
                self.ports.comments.move_anchor(key, comment.id, &anchor)?;
            }
        }
        Ok(())
    }
}

fn check_body(body: &str) -> Result<String, EngineError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(EngineError::Invalid("the message is empty".into()));
    }
    if body.chars().count() > MAX_BODY {
        return Err(EngineError::Invalid(format!(
            "the message is longer than {MAX_BODY} characters"
        )));
    }
    Ok(body.to_owned())
}

/// The smallest element containing the point (the first one on ties).
fn element_at(map: &[Element], x: i64, y: i64) -> Option<&Element> {
    let mut best: Option<&Element> = None;
    for element in map.iter().filter(|e| e.contains(x, y)) {
        if best.is_none_or(|b| element.w * element.h < b.w * b.h) {
            best = Some(element);
        }
    }
    best
}

fn chain_tail(chain: &[String]) -> &[String] {
    &chain[chain.len().saturating_sub(3)..]
}

/// The same element in a new element map, by score: same source location +4, last three
/// components of the chain +3, text +3, selector +2, tag +1. Below 4 is not a match; equal scores
/// go to the closest box. With no match the anchor stays and is marked detached.
fn reanchor(anchor: &Anchor, map: &[Element]) -> Anchor {
    let mut best: Option<(&Element, u32, f64)> = None;
    for element in map {
        let mut score = 0;
        if anchor.src.is_some() && element.src == anchor.src {
            score += 4;
        }
        if !anchor.chain.is_empty() && chain_tail(&element.chain) == chain_tail(&anchor.chain) {
            score += 3;
        }
        if !anchor.text.is_empty() && element.text == anchor.text {
            score += 3;
        }
        if !anchor.sel.is_empty() && element.sel == anchor.sel {
            score += 2;
        }
        if !anchor.tag.is_empty() && element.tag == anchor.tag {
            score += 1;
        }
        if score < 4 {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let distance = ((element.x - anchor.x) as f64).hypot((element.y - anchor.y) as f64);
        if best.is_none_or(|(_, s, d)| score > s || (score == s && distance < d)) {
            best = Some((element, score, distance));
        }
    }
    match best {
        Some((element, _, _)) => element.anchor(),
        None => Anchor {
            detached: true,
            ..anchor.clone()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BuildState;
    use crate::testing::{engine_with_fakes, file};
    use charet_model::Actor;
    use serde_json::json;

    #[allow(clippy::too_many_arguments)]
    fn el(
        x: i64,
        y: i64,
        w: i64,
        h: i64,
        chain: &[&str],
        sel: &str,
        tag: &str,
        text: &str,
        src: Option<&str>,
    ) -> Element {
        Element {
            x,
            y,
            w,
            h,
            chain: chain.iter().map(|c| (*c).to_owned()).collect(),
            sel: sel.into(),
            tag: tag.into(),
            text: text.into(),
            src: src.map(str::to_owned),
        }
    }

    #[test]
    fn point_picks_the_smallest_element() {
        let map = [
            el(0, 0, 100, 100, &["Page"], "body > main", "main", "", None),
            el(
                10,
                10,
                20,
                10,
                &["Page", "Button"],
                "body > main > button",
                "button",
                "Save",
                None,
            ),
        ];
        assert_eq!(element_at(&map, 15, 15).unwrap().tag, "button");
        assert_eq!(element_at(&map, 80, 80).unwrap().tag, "main");
        assert!(element_at(&map, 200, 200).is_none());
    }

    /// Cases from the old canvas rules, plus the source location.
    #[test]
    fn reanchor_scores() {
        let save = el(
            10,
            10,
            20,
            10,
            &["App", "Form", "Button"],
            "s1",
            "button",
            "Save",
            Some("src/form.tsx:12:5"),
        );
        let anchor = save.anchor();

        // Moved and restyled, same source and text: follows.
        let moved = el(
            300,
            40,
            30,
            12,
            &["App", "Form", "Button"],
            "s9",
            "button",
            "Save",
            Some("src/form.tsx:12:5"),
        );
        assert_eq!(
            reanchor(&anchor, std::slice::from_ref(&moved)),
            moved.anchor()
        );

        // Line numbers shifted, but chain + text + tag still match (3 + 3 + 1).
        let shifted = el(
            10,
            30,
            20,
            10,
            &["App", "Form", "Button"],
            "s1",
            "button",
            "Save",
            Some("src/form.tsx:14:5"),
        );
        assert_eq!(reanchor(&anchor, std::slice::from_ref(&shifted)).y, 30);

        // Two candidates with equal score: the closer one wins.
        let near = el(
            12,
            12,
            20,
            10,
            &["X", "Form", "Button"],
            "a",
            "button",
            "Save",
            None,
        );
        let far = el(
            400,
            400,
            20,
            10,
            &["X", "Form", "Button"],
            "b",
            "button",
            "Save",
            None,
        );
        assert_eq!(reanchor(&anchor, &[far, near.clone()]), near.anchor());

        // Only the tag and selector match (2 + 1 < 4): lost, stays put, detached.
        let other = el(10, 10, 20, 10, &["Other"], "s1", "button", "Cancel", None);
        let lost = reanchor(&anchor, &[other]);
        assert!(lost.detached);
        assert_eq!((lost.x, lost.y, lost.text.as_str()), (10, 10, "Save"));
    }

    const MANIFEST: &str =
        r#"{"manifestVersion":1,"screens":[{"id":"list","size":{"w":800,"h":600}}]}"#;

    /// A project with one capture of `list` and a comment on its Add button.
    fn commented() -> (Engine, crate::testing::Fakes, Comment) {
        let (engine, fakes) = engine_with_fakes();
        let import = crate::Import {
            files: vec![file("src/list.tsx", "v1")],
            source: "test".into(),
            commit: None,
        };
        engine
            .create_project(&Actor::Local, "shop", "Shop", Some(import))
            .unwrap();
        engine
            .preview(&Actor::Local, "shop", "main", false)
            .unwrap();
        fakes.workspace.finish(BuildState::Ready);
        fakes.workspace.serve(".charet/manifest.json", MANIFEST);
        fakes.renderer.set_elements(json!([
            { "x": 0, "y": 0, "w": 800, "h": 600, "chain": ["List"], "sel": "body > main", "tag": "main", "text": "", "src": "src/list.tsx:3:5" },
            { "x": 10, "y": 10, "w": 60, "h": 20, "chain": ["List", "Button"], "sel": "body > main > button", "tag": "button", "text": "Add", "src": "src/list.tsx:7:9" }
        ]));
        engine
            .capture(&Actor::Local, "shop", "main", &[], &[], false)
            .unwrap();

        let comment = engine
            .add_comment(
                &Actor::Local,
                "shop",
                "screen",
                "list",
                "light",
                &At::Point { x: 20, y: 15 },
                " Make it primary ",
            )
            .unwrap();
        assert_eq!(comment.body, "Make it primary");
        assert_eq!(comment.anchor.text, "Add");
        assert_eq!(comment.anchor.src.as_deref(), Some("src/list.tsx:7:9"));
        assert_eq!(comment.status, CommentStatus::Open);
        assert_eq!(
            engine
                .comment_snapshot(&Actor::Local, "shop", comment.id)
                .unwrap(),
            b"jpeg:list"
        );

        // No element at the point: a small box around it.
        let loose = engine
            .add_comment(
                &Actor::Local,
                "shop",
                "screen",
                "list",
                "light",
                &At::Point { x: 900, y: 900 },
                "here",
            )
            .unwrap();
        assert_eq!(
            (loose.anchor.x, loose.anchor.w, loose.anchor.tag.as_str()),
            (892, 16, "")
        );
        let missing = engine.add_comment(
            &Actor::Local,
            "shop",
            "screen",
            "list",
            "light",
            &At::Selector("nav".into()),
            "x",
        );
        assert!(matches!(missing, Err(EngineError::Invalid(_))));

        (engine, fakes, comment)
    }

    #[test]
    fn comment_thread() {
        let (engine, _, comment) = commented();
        // Thread: reply, stale write refused, resolve, reopen.
        let replied = engine
            .answer_comment(
                &Actor::Local,
                "shop",
                comment.id,
                comment.rev,
                ReplyKind::Reply,
                "Which shade?",
            )
            .unwrap();
        assert_eq!(replied.rev, comment.rev + 1);
        assert_eq!(replied.replies.len(), 1);
        let stale = engine.answer_comment(
            &Actor::Local,
            "shop",
            comment.id,
            comment.rev,
            ReplyKind::Resolve,
            "",
        );
        assert_eq!(stale, Err(EngineError::Changed(replied.rev)));
        let resolved = engine
            .answer_comment(
                &Actor::Local,
                "shop",
                comment.id,
                replied.rev,
                ReplyKind::Resolve,
                "",
            )
            .unwrap();
        assert_eq!(resolved.status, CommentStatus::Resolved);
        assert!(resolved.resolved_commit.is_some());
        let open = CommentFilter {
            status: Some(CommentStatus::Open),
            ..CommentFilter::default()
        };
        assert_eq!(
            engine.comments(&Actor::Local, "shop", &open).unwrap().len(),
            1
        );
        let reopened = engine
            .answer_comment(
                &Actor::Local,
                "shop",
                comment.id,
                resolved.rev,
                ReplyKind::Reopen,
                "not yet",
            )
            .unwrap();
        assert_eq!(reopened.status, CommentStatus::Open);
        assert_eq!(reopened.replies.last().unwrap().body, "not yet");
    }

    #[test]
    fn comments_follow_their_element() {
        let (engine, fakes, comment) = commented();
        // A new capture where the button moved: the comment follows. Then it disappears: detached.
        let base = engine.ports.store.head("shop").unwrap().unwrap();
        engine
            .push(
                &Actor::Local,
                "shop",
                &base,
                "move",
                &[file("src/list.tsx", "v2")],
            )
            .unwrap();
        engine
            .preview(&Actor::Local, "shop", "main", false)
            .unwrap();
        fakes.workspace.finish(BuildState::Ready);
        fakes.renderer.set_elements(json!([
            { "x": 500, "y": 300, "w": 60, "h": 20, "chain": ["List", "Button"], "sel": "body > main > div > button", "tag": "button", "text": "Add", "src": "src/list.tsx:9:9" }
        ]));
        engine
            .capture(&Actor::Local, "shop", "main", &[], &[], false)
            .unwrap();
        let moved = engine.comment(&Actor::Local, "shop", comment.id).unwrap();
        assert_eq!(
            (moved.anchor.x, moved.anchor.y, moved.anchor.detached),
            (500, 300, false)
        );
        assert_eq!(
            moved.rev, comment.rev,
            "following is not a change by a person"
        );

        let base = engine.ports.store.head("shop").unwrap().unwrap();
        engine
            .push(
                &Actor::Local,
                "shop",
                &base,
                "remove",
                &[file("src/list.tsx", "v3")],
            )
            .unwrap();
        engine
            .preview(&Actor::Local, "shop", "main", false)
            .unwrap();
        fakes.workspace.finish(BuildState::Ready);
        fakes.renderer.set_elements(json!([]));
        engine
            .capture(&Actor::Local, "shop", "main", &[], &[], false)
            .unwrap();
        let gone = engine.comment(&Actor::Local, "shop", comment.id).unwrap();
        assert!(gone.anchor.detached);
        assert_eq!((gone.anchor.x, gone.anchor.y), (500, 300));
    }

    #[test]
    fn visitors_comment_where_the_project_lets_them() {
        let (engine, _, comment) = commented();
        let visitor = engine
            .visitor(&crate::members::new_visitor_id())
            .unwrap()
            .unwrap();
        let add = |engine: &Engine| {
            engine.add_comment(
                &visitor,
                "shop",
                "screen",
                "list",
                "light",
                &At::Point { x: 20, y: 15 },
                "From outside",
            )
        };
        // Looking only (the default).
        assert!(matches!(add(&engine), Err(EngineError::Forbidden)));
        engine
            .set_project_visitors(
                &Actor::Local,
                "shop",
                Some(charet_model::VisitorAccess::Comment),
            )
            .unwrap();
        let mine = add(&engine).unwrap();
        assert!(mine.author.id.starts_with("visitor:"));
        assert_eq!(mine.author.name, visitor.name());
        let answered = engine
            .answer_comment(
                &visitor,
                "shop",
                comment.id,
                comment.rev,
                ReplyKind::Reply,
                "Agreed",
            )
            .unwrap();
        assert_eq!(answered.replies.len(), 1);
    }
}
