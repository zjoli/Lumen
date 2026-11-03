use crate::workspace::{WorkspaceError, WorkspaceResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Domain,
    Topic,
    Concept,
    Unit,
    Question,
    Resource,
}
impl ObjectKind {
    pub const ALL: [Self; 6] = [
        Self::Domain,
        Self::Topic,
        Self::Concept,
        Self::Unit,
        Self::Question,
        Self::Resource,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Domain => "domain",
            Self::Topic => "topic",
            Self::Concept => "concept",
            Self::Unit => "unit",
            Self::Question => "question",
            Self::Resource => "resource",
        }
    }
    pub fn directory(self) -> String {
        format!("{}s", self.name())
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct ObjectId(pub(crate) String);
impl ObjectId {
    pub fn new(kind: ObjectKind) -> Self {
        Self(format!("{}_{}", kind.name(), Uuid::now_v7()))
    }
    pub fn parse(s: impl Into<String>) -> WorkspaceResult<Self> {
        let s = s.into();
        let (kind, uuid) = s
            .split_once('_')
            .ok_or_else(|| WorkspaceError::InvalidFormat("invalid object ID".into()))?;
        if !ObjectKind::ALL.iter().any(|k| k.name() == kind)
            || Uuid::parse_str(uuid)
                .map(|v| v.get_version_num() != 7)
                .unwrap_or(true)
        {
            return Err(WorkspaceError::InvalidFormat(
                "expected typed UUIDv7".into(),
            ));
        }
        Ok(Self(s))
    }
    pub fn kind(&self) -> ObjectKind {
        ObjectKind::ALL
            .into_iter()
            .find(|k| self.0.starts_with(&format!("{}_", k.name())))
            .expect("validated ID")
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn path(&self) -> String {
        format!("{}/{}.md", self.kind().directory(), self.0)
    }
}
impl<'de> Deserialize<'de> for ObjectId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
impl std::fmt::Display for ObjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Revision(pub String);
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "revision", rename_all = "snake_case")]
pub enum ExpectedRevision {
    Absent,
    Match(Revision),
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReadView {
    Abstract,
    Overview,
    #[default]
    Full,
    Section(String),
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetRef {
    pub path: String,
    pub media_type: String,
    pub hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub uri: String,
    pub captured_at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub kind: StepKind,
    pub target: ObjectId,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Read,
    Answer,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Inquiry,
    Assessment,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectMetadata {
    pub schema_version: u32,
    pub id: ObjectId,
    #[serde(rename = "type")]
    pub kind: ObjectKind,
    pub title: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub member_of: Vec<ObjectId>,
    #[serde(default)]
    pub requires: Vec<ObjectId>,
    #[serde(default)]
    pub covers: Vec<ObjectId>,
    #[serde(default)]
    pub tests: Vec<ObjectId>,
    #[serde(default)]
    pub sources: Vec<ObjectId>,
    #[serde(default)]
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_kind: Option<QuestionKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
    #[serde(default)]
    pub attachments: Vec<AssetRef>,
    #[serde(default)]
    pub extensions: BTreeMap<String, Value>,
}
impl ObjectMetadata {
    pub fn new(kind: ObjectKind, title: impl Into<String>) -> Self {
        Self {
            schema_version: 1,
            id: ObjectId::new(kind),
            kind,
            title: title.into(),
            aliases: vec![],
            summary: None,
            member_of: vec![],
            requires: vec![],
            covers: vec![],
            tests: vec![],
            sources: vec![],
            steps: vec![],
            question_kind: None,
            origin: None,
            attachments: vec![],
            extensions: BTreeMap::new(),
        }
    }
    pub fn relations(&self) -> Vec<Relation> {
        let mut out = vec![];
        for (kind, ids) in [
            ("member_of", &self.member_of),
            ("requires", &self.requires),
            ("covers", &self.covers),
            ("tests", &self.tests),
            ("sources", &self.sources),
        ] {
            out.extend(ids.iter().map(|target| Relation {
                source: self.id.clone(),
                target: target.clone(),
                kind: kind.into(),
                typed: true,
            }));
        }
        out.extend(self.steps.iter().map(|s| {
            Relation {
                source: self.id.clone(),
                target: s.target.clone(),
                kind: match s.kind {
                    StepKind::Read => "read",
                    StepKind::Answer => "answer",
                }
                .into(),
                typed: true,
            }
        }));
        out
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeObject {
    pub metadata: ObjectMetadata,
    pub body: String,
}
pub type CreateObject = KnowledgeObject;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateObject {
    pub object: KnowledgeObject,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchObject {
    pub id: ObjectId,
    #[serde(default)]
    pub metadata_patch: serde_json::Map<String, Value>,
    pub body: Option<String>,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteObject {
    pub id: ObjectId,
    pub expected_revision: Revision,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ObjectChange {
    Create {
        object: KnowledgeObject,
    },
    Update {
        object: KnowledgeObject,
        expected_revision: Revision,
    },
    Delete {
        id: ObjectId,
        expected_revision: Revision,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ChangeSet {
    pub changes: Vec<ObjectChange>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeReceipt {
    pub changed: Vec<String>,
    pub generation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRef {
    pub repository_id: String,
    pub path: String,
    pub object_id: String,
    pub revision: Revision,
    pub line: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectView {
    pub metadata: ObjectMetadata,
    pub content: String,
    pub revision: Revision,
    pub source_ref: SourceRef,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadRequest {
    pub id: ObjectId,
    #[serde(default)]
    pub view: ReadView,
}
#[derive(Debug, Serialize)]
pub struct ReadOutcome {
    pub id: ObjectId,
    pub result: WorkspaceResult<ObjectView>,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ObjectFilter {
    #[serde(default)]
    pub kinds: Vec<ObjectKind>,
    pub scope: Option<ObjectId>,
    #[serde(default)]
    pub descendants: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Relation {
    pub source: ObjectId,
    pub target: ObjectId,
    pub kind: String,
    pub typed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RelationQuery {
    #[serde(default)]
    pub incoming: bool,
    pub kind: Option<String>,
    pub typed: Option<bool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageRequest {
    #[serde(default = "default_limit")]
    pub limit: usize,
    pub cursor: Option<String>,
}
fn default_limit() -> usize {
    20
}
impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: 20,
            cursor: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub generation: String,
}
pub fn paginate<T: Clone>(
    items: &[T],
    req: PageRequest,
    generation: &str,
) -> WorkspaceResult<Page<T>> {
    let start = match req.cursor {
        None => 0,
        Some(c) => {
            let (g, n) = c.split_once(':').ok_or(WorkspaceError::CursorExpired)?;
            if g != generation {
                return Err(WorkspaceError::CursorExpired);
            }
            n.parse::<usize>()
                .map_err(|_| WorkspaceError::CursorExpired)?
        }
    };
    if start > items.len() {
        return Err(WorkspaceError::CursorExpired);
    }
    let end = start
        .saturating_add(req.limit.clamp(1, 100))
        .min(items.len());
    Ok(Page {
        items: items[start..end].to_vec(),
        next_cursor: (end < items.len()).then(|| format!("{generation}:{end}")),
        generation: generation.into(),
    })
}
