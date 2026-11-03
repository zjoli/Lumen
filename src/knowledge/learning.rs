use super::*;
use crate::workspace::{files, WorkspaceError as E, WorkspaceHandle, WorkspaceResult as R};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Study,
    Answer,
    Review,
    Assessment,
    Correction,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventMetadata {
    pub schema_version: u32,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub event_kind: EventKind,
    pub occurred_at: DateTime<Utc>,
    pub targets: Vec<ObjectId>,
    pub session_id: Option<String>,
    pub supersedes: Option<String>,
    #[serde(default)]
    pub data: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LearningEvent {
    pub metadata: EventMetadata,
    pub body: String,
}
impl LearningEvent {
    pub fn path(&self) -> R<String> {
        Ok(format!(
            "records/{:04}/{:02}/{}.md",
            self.metadata.occurred_at.year(),
            self.metadata.occurred_at.month(),
            self.metadata.id
        ))
    }
    pub fn validate(&self) -> R<()> {
        let m = &self.metadata;
        if m.schema_version != 1 {
            return Err(E::UnsupportedSchema(m.schema_version));
        }
        crate::workspace::validate_typed_id(&m.id, "event")?;
        if m.kind != "learning_event"
            || m.targets.is_empty()
            || (m.event_kind == EventKind::Correction) != m.supersedes.is_some()
        {
            return Err(E::InvalidFormat("invalid learning event".into()));
        }
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppendEvent {
    pub event_kind: EventKind,
    #[serde(default)]
    pub occurred_at: Option<DateTime<Utc>>,
    pub targets: Vec<ObjectId>,
    pub session_id: Option<String>,
    pub supersedes: Option<String>,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub body: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub target: Option<ObjectId>,
    pub event_kind: Option<EventKind>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LearningState {
    pub object_id: ObjectId,
    pub evidence_ids: Vec<String>,
    pub outcome: Option<String>,
    pub algorithm_version: u32,
}
#[derive(Clone)]
pub struct LearningService(pub(crate) WorkspaceHandle);
impl LearningService {
    pub async fn append(&self, req: AppendEvent, cancel: CancellationToken) -> R<LearningEvent> {
        self.0
            .call(cancel, move |s, c| {
                s.ensure_writable()?;
                let event = LearningEvent {
                    metadata: EventMetadata {
                        schema_version: 1,
                        id: format!("event_{}", uuid::Uuid::now_v7()),
                        kind: "learning_event".into(),
                        event_kind: req.event_kind,
                        occurred_at: req.occurred_at.unwrap_or_else(Utc::now),
                        targets: req.targets,
                        session_id: req.session_id,
                        supersedes: req.supersedes,
                        data: req.data,
                    },
                    body: req.body,
                };
                event.validate()?;
                let path = event.path()?;
                let bytes = format::encode(&event.metadata, &event.body)?;
                let mut tree = files::scan(&s.root)?;
                tree.insert(path.clone(), bytes.clone());
                Catalog::from_tree(&tree).require_valid()?;
                crate::workspace::check_cancel(c)?;
                files::apply(&s.root, BTreeMap::from([(path, Some(bytes))]))?;
                crate::retrieval::refresh(s, &tree, &CancellationToken::new())?;
                Ok(event)
            })
            .await
    }
    pub async fn events(
        &self,
        filter: EventFilter,
        page: PageRequest,
        cancel: CancellationToken,
    ) -> R<Page<LearningEvent>> {
        self.0
            .call(cancel, move |s, _| {
                let tree = files::scan(&s.root)?;
                let mut events = Catalog::from_tree(&tree)
                    .events
                    .into_iter()
                    .map(|(e, _, _)| e)
                    .filter(|e| {
                        filter
                            .target
                            .as_ref()
                            .map(|id| e.metadata.targets.contains(id))
                            .unwrap_or(true)
                            && filter
                                .event_kind
                                .as_ref()
                                .map(|k| *k == e.metadata.event_kind)
                                .unwrap_or(true)
                    })
                    .collect::<Vec<_>>();
                events.sort_by(|a, b| {
                    (&a.metadata.occurred_at, &a.metadata.id)
                        .cmp(&(&b.metadata.occurred_at, &b.metadata.id))
                });
                paginate(&events, page, &files::fingerprint(&tree))
            })
            .await
    }
    pub async fn state(
        &self,
        ids: Vec<ObjectId>,
        cancel: CancellationToken,
    ) -> R<Vec<LearningState>> {
        self.0
            .call(cancel, move |s, _| {
                let c = Catalog::from_tree(&files::scan(&s.root)?);
                Ok(derive_state(&c, ids))
            })
            .await
    }
}
pub(crate) fn derive_state(c: &Catalog, ids: Vec<ObjectId>) -> Vec<LearningState> {
    let superseded = c
        .events
        .iter()
        .filter_map(|(e, _, _)| e.metadata.supersedes.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let mut events = c
        .events
        .iter()
        .filter(|(e, _, _)| !superseded.contains(&e.metadata.id))
        .collect::<Vec<_>>();
    events.sort_by_key(|(e, _, _)| (e.metadata.occurred_at, e.metadata.id.clone()));
    ids.into_iter()
        .map(|id| {
            let relevant = events
                .iter()
                .filter(|(e, _, _)| e.metadata.targets.contains(&id))
                .collect::<Vec<_>>();
            LearningState {
                object_id: id,
                evidence_ids: relevant
                    .iter()
                    .map(|(e, _, _)| e.metadata.id.clone())
                    .collect(),
                outcome: relevant.iter().rev().find_map(|(e, _, _)| {
                    e.metadata
                        .data
                        .get("outcome")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }),
                algorithm_version: 1,
            }
        })
        .collect()
}
