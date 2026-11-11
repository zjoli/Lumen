use super::{format, learning::*, types::*};
use crate::workspace::{
    files::FileTree, Diagnostic, RepositoryConfig, ValidationReport, WorkspaceError as E,
    WorkspaceResult as R,
};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Default)]
pub(crate) struct Catalog {
    pub objects: BTreeMap<ObjectId, (KnowledgeObject, Revision, String)>,
    pub events: Vec<(LearningEvent, Revision, String)>,
    pub diagnostics: Vec<Diagnostic>,
    body_lines: BTreeMap<ObjectId, usize>,
}
impl Catalog {
    pub fn from_tree(tree: &FileTree) -> Self {
        let mut c = Self::default();
        let mut duplicates = BTreeSet::new();
        for (path, bytes) in tree {
            let result = (|| -> R<()> {
                if path == "repository.yaml" {
                    let r: RepositoryConfig = serde_yaml_ng::from_slice(bytes)?;
                    if r.schema_version != 1 {
                        return Err(E::UnsupportedSchema(r.schema_version));
                    }
                    let u = uuid::Uuid::parse_str(&r.repository_id)
                        .map_err(|_| E::InvalidFormat("repository_id must be UUIDv7".into()))?;
                    if u.get_version_num() != 7 {
                        return Err(E::InvalidFormat("repository_id must be UUIDv7".into()));
                    }
                } else if ObjectKind::ALL
                    .iter()
                    .any(|k| path.starts_with(&format!("{}/", k.directory())))
                {
                    let text = std::str::from_utf8(bytes)
                        .map_err(|_| E::InvalidFormat("non UTF-8 markdown".into()))?;
                    let object = format::parse_object(text)?;
                    let id = object.metadata.id.clone();
                    if c.objects.contains_key(&id) {
                        duplicates.insert(id.clone());
                        return Err(E::DuplicateId(id.to_string()));
                    }
                    if path != &id.path() {
                        return Err(E::InvalidPath(format!("expected {}", id.path())));
                    }
                    c.body_lines.insert(
                        id.clone(),
                        text[..text.len() - object.body.len()]
                            .bytes()
                            .filter(|b| *b == b'\n')
                            .count()
                            + 1,
                    );
                    c.objects
                        .insert(id, (object, format::revision(bytes), path.clone()));
                } else if path.starts_with("records/") {
                    let text = std::str::from_utf8(bytes)
                        .map_err(|_| E::InvalidFormat("invalid event UTF-8".into()))?;
                    let (metadata, body) = format::decode::<EventMetadata>(text)?;
                    let event = LearningEvent { metadata, body };
                    event.validate()?;
                    if event.path()? != *path {
                        return Err(E::InvalidPath("event path does not match ID/date".into()));
                    }
                    c.events
                        .push((event, format::revision(bytes), path.clone()));
                } else if path.starts_with("sessions/") {
                    crate::workspace::validate_segment(path, bytes)?;
                } else if path.starts_with("assets/") {
                    if bytes.len() > 10 * 1024 * 1024 {
                        return Err(E::AssetTooLarge);
                    }
                    let stem = path
                        .strip_prefix("assets/")
                        .unwrap()
                        .split('.')
                        .next()
                        .unwrap_or("");
                    if stem != blake3::hash(bytes).to_hex().as_str() {
                        return Err(E::InvalidFormat("asset hash mismatch".into()));
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                c.diagnostics.push(Diagnostic {
                    path: path.clone(),
                    message: e.to_string(),
                })
            }
        }
        if !tree.contains_key("repository.yaml") {
            c.diagnostics.push(Diagnostic {
                path: "repository.yaml".into(),
                message: "missing repository metadata".into(),
            })
        }
        for id in duplicates {
            c.objects.remove(&id);
        }
        let mut invalid = BTreeSet::new();
        for (id, (object, _, path)) in &c.objects {
            let meta = &object.metadata;
            let result = (|| -> R<()> {
                crate::study::validate_goal(meta, &c)?;
                if meta.question_kind.is_some() && meta.kind != ObjectKind::Question
                    || meta.origin.is_some() && meta.kind != ObjectKind::Resource
                {
                    return Err(E::InvalidFormat(
                        "type-specific field used on wrong object".into(),
                    ));
                }
                for r in meta.relations() {
                    let target = c
                        .objects
                        .get(&r.target)
                        .ok_or_else(|| E::InvalidRelation(format!("missing {}", r.target)))?
                        .0
                        .metadata
                        .kind;
                    let valid = match r.kind.as_str() {
                        "member_of" => match meta.kind {
                            ObjectKind::Domain => false,
                            ObjectKind::Topic => {
                                matches!(target, ObjectKind::Domain | ObjectKind::Topic)
                            }
                            _ => target == ObjectKind::Topic,
                        },
                        "requires" => match meta.kind {
                            ObjectKind::Concept => target == ObjectKind::Concept,
                            ObjectKind::Unit => {
                                matches!(target, ObjectKind::Concept | ObjectKind::Unit)
                            }
                            _ => false,
                        },
                        "covers" => meta.kind == ObjectKind::Unit && target == ObjectKind::Concept,
                        "tests" => {
                            meta.kind == ObjectKind::Question && target == ObjectKind::Concept
                        }
                        "sources" => {
                            !matches!(meta.kind, ObjectKind::Domain | ObjectKind::Resource)
                                && target == ObjectKind::Resource
                        }
                        "read" => meta.kind == ObjectKind::Unit && target == ObjectKind::Resource,
                        "answer" => meta.kind == ObjectKind::Unit && target == ObjectKind::Question,
                        _ => false,
                    };
                    if !valid {
                        return Err(E::InvalidRelation(format!(
                            "{} -> {} ({})",
                            id, r.target, r.kind
                        )));
                    }
                }
                for asset in &meta.attachments {
                    let bytes = tree
                        .get(&asset.path)
                        .ok_or_else(|| E::NotFound(asset.path.clone()))?;
                    if !asset.path.starts_with("assets/")
                        || blake3::hash(bytes).to_hex().as_str() != asset.hash
                    {
                        return Err(E::InvalidFormat("invalid attachment reference".into()));
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                invalid.insert(id.clone());
                c.diagnostics.push(Diagnostic {
                    path: path.clone(),
                    message: e.to_string(),
                })
            }
        }
        for kind in ["member_of", "requires"] {
            for id in c.objects.keys() {
                if c.cyclic(id, id, kind, &mut BTreeSet::new()) {
                    invalid.insert(id.clone());
                    c.diagnostics.push(Diagnostic {
                        path: id.path(),
                        message: format!("cycle in {kind}"),
                    });
                }
            }
        }
        let mut event_ids = BTreeSet::new();
        let all_events = c
            .events
            .iter()
            .map(|(e, _, _)| e.metadata.id.clone())
            .collect::<BTreeSet<_>>();
        for (event, _, path) in &c.events {
            let mut add = |message: String| {
                c.diagnostics.push(Diagnostic {
                    path: path.clone(),
                    message,
                })
            };
            if !event_ids.insert(event.metadata.id.clone()) {
                add("duplicate event ID".into())
            }
            for id in &event.metadata.targets {
                if !c.objects.contains_key(id) {
                    add(format!("event target missing: {id}"))
                }
            }
            if let Some(old) = &event.metadata.supersedes {
                if !all_events.contains(old) || old == &event.metadata.id {
                    add("invalid correction target".into())
                }
            }
        }
        let replacements = c
            .events
            .iter()
            .filter_map(|(e, _, _)| {
                e.metadata
                    .supersedes
                    .as_ref()
                    .map(|old| (e.metadata.id.clone(), old.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        for (event, _, path) in &c.events {
            let mut seen = BTreeSet::new();
            let mut current = event.metadata.id.as_str();
            while let Some(next) = replacements.get(current) {
                if !seen.insert(current.to_string()) {
                    c.diagnostics.push(Diagnostic {
                        path: path.clone(),
                        message: "cycle in event corrections".into(),
                    });
                    break;
                }
                current = next;
            }
        }
        loop {
            let before = invalid.len();
            for (id, (object, _, path)) in &c.objects {
                if !invalid.contains(id)
                    && object
                        .metadata
                        .relations()
                        .iter()
                        .any(|r| invalid.contains(&r.target))
                {
                    invalid.insert(id.clone());
                    c.diagnostics.push(Diagnostic {
                        path: path.clone(),
                        message: "relation target is invalid".into(),
                    });
                }
            }
            if invalid.len() == before {
                break;
            }
        }
        for id in invalid {
            c.objects.remove(&id);
        }
        for (event, _, path) in &c.events {
            if event
                .metadata
                .targets
                .iter()
                .any(|id| !c.objects.contains_key(id))
            {
                c.diagnostics.push(Diagnostic {
                    path: path.clone(),
                    message: "event refers to invalid knowledge".into(),
                });
            }
        }
        let invalid_events = c
            .diagnostics
            .iter()
            .filter(|d| d.path.starts_with("records/"))
            .map(|d| d.path.clone())
            .collect::<BTreeSet<_>>();
        c.events
            .retain(|(_, _, path)| !invalid_events.contains(path));
        c
    }
    fn cyclic(
        &self,
        start: &ObjectId,
        node: &ObjectId,
        kind: &str,
        seen: &mut BTreeSet<ObjectId>,
    ) -> bool {
        if !seen.insert(node.clone()) {
            return false;
        }
        self.objects
            .get(node)
            .map(|(o, _, _)| {
                o.metadata
                    .relations()
                    .into_iter()
                    .filter(|r| r.kind == kind)
                    .any(|r| r.target == *start || self.cyclic(start, &r.target, kind, seen))
            })
            .unwrap_or(false)
    }
    pub fn report(&self) -> ValidationReport {
        ValidationReport {
            valid: self.diagnostics.is_empty(),
            diagnostics: self.diagnostics.clone(),
        }
    }
    pub fn require_valid(&self) -> R<()> {
        if let Some(d) = self.diagnostics.first() {
            Err(E::InvalidFormat(format!("{}: {}", d.path, d.message)))
        } else {
            Ok(())
        }
    }
    pub fn scope(&self, id: &ObjectId, descendants: bool) -> BTreeSet<ObjectId> {
        let mut parents = BTreeSet::from([id.clone()]);
        if descendants {
            loop {
                let old = parents.len();
                for (k, (o, _, _)) in &self.objects {
                    if o.metadata.kind == ObjectKind::Topic
                        && o.metadata.member_of.iter().any(|p| parents.contains(p))
                    {
                        parents.insert(k.clone());
                    }
                }
                if parents.len() == old {
                    break;
                }
            }
        }
        let mut result = parents.clone();
        for (k, (o, _, _)) in &self.objects {
            if o.metadata.member_of.iter().any(|p| parents.contains(p)) {
                result.insert(k.clone());
            }
        }
        result
    }
    pub fn view(&self, id: &ObjectId, view: ReadView, repository_id: &str) -> R<ObjectView> {
        let (o, revision, path) = self
            .objects
            .get(id)
            .ok_or_else(|| E::NotFound(id.to_string()))?;
        let base_line = *self.body_lines.get(id).unwrap_or(&1);
        let (content, line) = match view {
            ReadView::Full => (o.body.clone(), base_line),
            ReadView::Abstract => (
                o.metadata.summary.clone().unwrap_or_else(|| {
                    o.body
                        .lines()
                        .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                        .unwrap_or(&o.metadata.title)
                        .chars()
                        .take(256)
                        .collect()
                }),
                1,
            ),
            ReadView::Overview => {
                let mut text = format!(
                    "# {}\n\n{}\n",
                    o.metadata.title,
                    o.metadata.summary.as_deref().unwrap_or("")
                );
                for member in self.scope(id, false) {
                    if member != *id {
                        if let Some((m, _, _)) = self.objects.get(&member) {
                            text.push_str(&format!("- [{}](lumen:{})\n", m.metadata.title, member))
                        }
                    }
                }
                for r in o.metadata.relations() {
                    text.push_str(&format!("- {}: {}\n", r.kind, r.target))
                }
                (text.chars().take(4000).collect(), 1)
            }
            ReadView::Section(heading) => {
                let lines = o.body.lines().collect::<Vec<_>>();
                let start = lines
                    .iter()
                    .position(|l| l.trim_start_matches('#').trim() == heading)
                    .ok_or_else(|| E::NotFound(heading.clone()))?;
                let level = lines[start].chars().take_while(|c| *c == '#').count();
                let end = (start + 1..lines.len())
                    .find(|i| {
                        let n = lines[*i].chars().take_while(|c| *c == '#').count();
                        n > 0 && n <= level
                    })
                    .unwrap_or(lines.len());
                (lines[start..end].join("\n"), base_line + start)
            }
        };
        Ok(ObjectView {
            metadata: o.metadata.clone(),
            content,
            revision: revision.clone(),
            source_ref: SourceRef {
                repository_id: repository_id.into(),
                path: path.clone(),
                object_id: id.to_string(),
                revision: revision.clone(),
                line,
            },
        })
    }
}
