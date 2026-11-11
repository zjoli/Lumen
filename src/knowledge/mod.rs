mod catalog;
pub mod format;
mod learning;
mod tools;
mod types;
use crate::workspace::{
    check_cancel, files, WorkspaceError as E, WorkspaceHandle, WorkspaceResult as R,
};
pub(crate) use catalog::Catalog;
pub(crate) use learning::derive_state as learning_states;
pub use learning::*;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;
pub use tools::*;
pub use types::*;

#[derive(Clone)]
pub struct KnowledgeService(pub(crate) WorkspaceHandle);
impl KnowledgeService {
    /// Omitted metadata stays unchanged; null clears an optional field. Body "" clears text.
    pub async fn patch(&self, req: PatchObject, cancel: CancellationToken) -> R<ObjectView> {
        self.0
            .call(cancel, move |s, c| {
                s.ensure_writable()?;
                if req
                    .metadata_patch
                    .keys()
                    .any(|k| matches!(k.as_str(), "id" | "type" | "schema_version"))
                {
                    return Err(E::InvalidFormat("identity fields cannot be patched".into()));
                }
                let mut tree = files::scan(&s.root)?;
                let catalog = Catalog::from_tree(&tree);
                let (old, revision, path) = catalog
                    .objects
                    .get(&req.id)
                    .ok_or_else(|| E::NotFound(req.id.to_string()))?;
                if *revision != req.expected_revision {
                    return Err(E::RevisionConflict(req.id.to_string()));
                }
                let mut metadata = serde_json::to_value(&old.metadata)?;
                merge_patch(&mut metadata, serde_json::Value::Object(req.metadata_patch));
                let metadata: ObjectMetadata = serde_json::from_value(metadata)?;
                let body = req.body.unwrap_or_else(|| old.body.clone());
                let bytes = format::encode(&metadata, &body)?;
                let path = path.clone();
                tree.insert(path.clone(), bytes.clone());
                let updated = Catalog::from_tree(&tree);
                updated.require_valid()?;
                check_cancel(c)?;
                let expected = BTreeMap::from([(path.clone(), Some(req.expected_revision.0))]);
                files::apply_checked(&s.root, BTreeMap::from([(path, Some(bytes))]), expected)?;
                crate::retrieval::refresh(s, &tree, &CancellationToken::new())?;
                updated.view(&req.id, ReadView::Full, &s.repository.repository_id)
            })
            .await
    }
    pub async fn list(
        &self,
        filter: ObjectFilter,
        page: PageRequest,
        cancel: CancellationToken,
    ) -> R<Page<ObjectMetadata>> {
        self.0
            .call(cancel, move |s, _| {
                s.ensure_writable()?;
                let tree = files::scan(&s.root)?;
                let c = Catalog::from_tree(&tree);
                let scope = filter
                    .scope
                    .as_ref()
                    .map(|id| c.scope(id, filter.descendants));
                let items = c
                    .objects
                    .values()
                    .filter(|(o, _, _)| {
                        (filter.kinds.is_empty() || filter.kinds.contains(&o.metadata.kind))
                            && scope
                                .as_ref()
                                .map(|ids| ids.contains(&o.metadata.id))
                                .unwrap_or(true)
                    })
                    .map(|(o, _, _)| o.metadata.clone())
                    .collect::<Vec<_>>();
                paginate(&items, page, &files::fingerprint(&tree))
            })
            .await
    }
    pub async fn resolve(
        &self,
        query: String,
        cancel: CancellationToken,
    ) -> R<Vec<ObjectMetadata>> {
        self.0
            .call(cancel, move |s, _| {
                let c = Catalog::from_tree(&files::scan(&s.root)?);
                let q = query.to_lowercase();
                Ok(c.objects
                    .values()
                    .filter(|(o, _, _)| {
                        o.metadata.id.as_str() == query
                            || o.metadata.title.to_lowercase() == q
                            || o.metadata.aliases.iter().any(|a| a.to_lowercase() == q)
                    })
                    .map(|(o, _, _)| o.metadata.clone())
                    .collect())
            })
            .await
    }
    pub async fn read(
        &self,
        id: ObjectId,
        view: ReadView,
        cancel: CancellationToken,
    ) -> R<ObjectView> {
        let generator = summary_generator(&self.0);
        self.0
            .call(cancel, move |s, _| {
                s.ensure_writable()?;
                let catalog = Catalog::from_tree(&files::scan(&s.root)?);
                read_view(s, &catalog, &id, view, &generator)
            })
            .await
    }
    pub async fn read_many(
        &self,
        requests: Vec<ReadRequest>,
        cancel: CancellationToken,
    ) -> R<Vec<ReadOutcome>> {
        let generator = summary_generator(&self.0);
        self.0
            .call(cancel, move |s, _| {
                s.ensure_writable()?;
                let c = Catalog::from_tree(&files::scan(&s.root)?);
                Ok(requests
                    .into_iter()
                    .map(|r| ReadOutcome {
                        id: r.id.clone(),
                        result: read_view(s, &c, &r.id, r.view, &generator),
                    })
                    .collect())
            })
            .await
    }
    pub async fn create(
        &self,
        mut object: CreateObject,
        cancel: CancellationToken,
    ) -> R<ObjectView> {
        object.metadata.id = ObjectId::new(object.metadata.kind);
        let id = object.metadata.id.clone();
        self.apply(
            ChangeSet {
                changes: vec![ObjectChange::Create { object }],
            },
            cancel.clone(),
        )
        .await?;
        self.read(id, ReadView::Full, cancel).await
    }
    pub async fn update(&self, req: UpdateObject, cancel: CancellationToken) -> R<ObjectView> {
        let id = req.object.metadata.id.clone();
        self.apply(
            ChangeSet {
                changes: vec![ObjectChange::Update {
                    object: req.object,
                    expected_revision: req.expected_revision,
                }],
            },
            cancel.clone(),
        )
        .await?;
        self.read(id, ReadView::Full, cancel).await
    }
    pub async fn delete(&self, req: DeleteObject, cancel: CancellationToken) -> R<ChangeReceipt> {
        self.apply(
            ChangeSet {
                changes: vec![ObjectChange::Delete {
                    id: req.id,
                    expected_revision: req.expected_revision,
                }],
            },
            cancel,
        )
        .await
    }
    pub async fn apply(&self, set: ChangeSet, cancel: CancellationToken) -> R<ChangeReceipt> {
        self.0
            .call(cancel, move |s, cancel| {
                s.ensure_writable()?;
                let mut tree = files::scan(&s.root)?;
                let mut changes = BTreeMap::new();
                let mut expected_revisions = BTreeMap::new();
                for change in set.changes {
                    check_cancel(cancel)?;
                    let (id, expected, bytes) = match change {
                        ObjectChange::Create { object } => {
                            let id = object.metadata.id.clone();
                            (
                                id,
                                ExpectedRevision::Absent,
                                Some(format::encode(&object.metadata, &object.body)?),
                            )
                        }
                        ObjectChange::Update {
                            object,
                            expected_revision,
                        } => {
                            let id = object.metadata.id.clone();
                            (
                                id,
                                ExpectedRevision::Match(expected_revision),
                                Some(format::encode(&object.metadata, &object.body)?),
                            )
                        }
                        ObjectChange::Delete {
                            id,
                            expected_revision,
                        } => (id, ExpectedRevision::Match(expected_revision), None),
                    };
                    let path = id.path();
                    if changes.contains_key(&path) {
                        return Err(E::InvalidFormat("duplicate object in ChangeSet".into()));
                    }
                    check_revision(tree.get(&path), &expected, &path)?;
                    expected_revisions
                        .insert(path.clone(), tree.get(&path).map(|b| format::revision(b).0));
                    if let Some(b) = &bytes {
                        format::parse_object(
                            std::str::from_utf8(b).map_err(|_| E::InvalidFormat("UTF-8".into()))?,
                        )?;
                        tree.insert(path.clone(), b.clone());
                    } else {
                        tree.remove(&path);
                    }
                    changes.insert(path, bytes);
                }
                Catalog::from_tree(&tree).require_valid()?;
                check_cancel(cancel)?;
                let changed = files::apply_checked(&s.root, changes, expected_revisions)?;
                crate::retrieval::refresh(s, &tree, &CancellationToken::new())?;
                s.emit(
                    crate::workspace::WorkspaceEventKind::FilesChanged,
                    Some(files::fingerprint(&tree)),
                );
                Ok(ChangeReceipt {
                    changed,
                    generation: files::fingerprint(&tree),
                })
            })
            .await
    }
    pub async fn relations(
        &self,
        id: ObjectId,
        query: RelationQuery,
        page: PageRequest,
        cancel: CancellationToken,
    ) -> R<Page<Relation>> {
        self.0
            .call(cancel, move |s, _| {
                let tree = files::scan(&s.root)?;
                let c = Catalog::from_tree(&tree);
                let mut all = vec![];
                for (o, _, _) in c.objects.values() {
                    all.extend(o.metadata.relations());
                    for e in pulldown_cmark::Parser::new(&o.body) {
                        if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link {
                            dest_url,
                            ..
                        }) = e
                        {
                            if let Some(id) = dest_url.strip_prefix("lumen:") {
                                if let Ok(target) = ObjectId::parse(id) {
                                    all.push(Relation {
                                        source: o.metadata.id.clone(),
                                        target,
                                        kind: "link".into(),
                                        typed: false,
                                    });
                                }
                            }
                        }
                    }
                }
                let items = all
                    .into_iter()
                    .filter(|r| {
                        (if query.incoming {
                            r.target == id
                        } else {
                            r.source == id
                        }) && query.kind.as_ref().map(|k| *k == r.kind).unwrap_or(true)
                            && query.typed.map(|t| t == r.typed).unwrap_or(true)
                    })
                    .collect::<Vec<_>>();
                paginate(&items, page, &files::fingerprint(&tree))
            })
            .await
    }
}
pub(crate) fn check_revision(
    current: Option<&Vec<u8>>,
    expected: &ExpectedRevision,
    path: &str,
) -> R<()> {
    let valid = match (expected, current) {
        (ExpectedRevision::Absent, None) => true,
        (ExpectedRevision::Match(r), Some(b)) => *r == format::revision(b),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(E::RevisionConflict(path.into()))
    }
}

#[derive(Clone)]
pub struct ResourceService(pub(crate) WorkspaceHandle);
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddAsset {
    pub bytes: Vec<u8>,
    pub extension: String,
    pub media_type: String,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSnapshot {
    pub object: KnowledgeObject,
    #[serde(default)]
    pub assets: Vec<AddAsset>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ByteRange {
    pub start: usize,
    pub end: Option<usize>,
}
impl ResourceService {
    pub async fn add_asset(&self, req: AddAsset, cancel: CancellationToken) -> R<AssetRef> {
        self.0
            .call(cancel, move |s, _| {
                s.ensure_writable()?;
                let (asset, bytes) = asset_bytes(req)?;
                files::apply(&s.root, BTreeMap::from([(asset.path.clone(), Some(bytes))]))?;
                Ok(asset)
            })
            .await
    }
    pub async fn read_asset(
        &self,
        asset: AssetRef,
        range: ByteRange,
        cancel: CancellationToken,
    ) -> R<Vec<u8>> {
        self.0
            .call(cancel, move |s, _| {
                if !asset.path.starts_with("assets/") {
                    return Err(E::InvalidPath(asset.path));
                }
                let bytes = std::fs::read(files::safe_path(&s.root, &asset.path)?)?;
                if blake3::hash(&bytes).to_hex().as_str() != asset.hash {
                    return Err(E::InvalidFormat("asset checksum mismatch".into()));
                }
                let end = range.end.unwrap_or(bytes.len());
                bytes
                    .get(range.start..end)
                    .map(|b| b.to_vec())
                    .ok_or_else(|| E::InvalidFormat("invalid byte range".into()))
            })
            .await
    }
    pub async fn remove_asset(
        &self,
        asset: AssetRef,
        cancel: CancellationToken,
    ) -> R<ChangeReceipt> {
        self.0
            .call(cancel, move |s, _| {
                s.ensure_writable()?;
                if !asset.path.starts_with("assets/") {
                    return Err(E::InvalidPath(asset.path));
                }
                let tree = files::scan(&s.root)?;
                let c = Catalog::from_tree(&tree);
                c.require_valid()?;
                if c.objects
                    .values()
                    .any(|(o, _, _)| o.metadata.attachments.iter().any(|a| a.path == asset.path))
                {
                    return Err(E::Referenced(asset.path));
                }
                let changed = files::apply(&s.root, BTreeMap::from([(asset.path, None)]))?;
                Ok(ChangeReceipt {
                    changed,
                    generation: files::fingerprint(&files::scan(&s.root)?),
                })
            })
            .await
    }
    pub async fn import(
        &self,
        mut req: ImportSnapshot,
        cancel: CancellationToken,
    ) -> R<ObjectView> {
        self.0
            .call(cancel, move |s, cancel| {
                s.ensure_writable()?;
                if req.object.metadata.kind != ObjectKind::Resource {
                    return Err(E::InvalidFormat(
                        "resource import requires resource type".into(),
                    ));
                }
                req.object.metadata.id = ObjectId::new(ObjectKind::Resource);
                let mut tree = files::scan(&s.root)?;
                let mut changes = BTreeMap::new();
                for a in req.assets {
                    let (asset, bytes) = asset_bytes(a)?;
                    tree.insert(asset.path.clone(), bytes.clone());
                    changes.insert(asset.path.clone(), Some(bytes));
                    req.object.metadata.attachments.push(asset);
                }
                let path = req.object.metadata.id.path();
                let bytes = format::encode(&req.object.metadata, &req.object.body)?;
                tree.insert(path.clone(), bytes.clone());
                changes.insert(path, Some(bytes));
                let catalog = Catalog::from_tree(&tree);
                catalog.require_valid()?;
                check_cancel(cancel)?;
                files::apply(&s.root, changes)?;
                crate::retrieval::refresh(s, &tree, &CancellationToken::new())?;
                catalog.view(
                    &req.object.metadata.id,
                    ReadView::Full,
                    &s.repository.repository_id,
                )
            })
            .await
    }
}
fn asset_bytes(req: AddAsset) -> R<(AssetRef, Vec<u8>)> {
    if req.bytes.len() > 10 * 1024 * 1024 {
        return Err(E::AssetTooLarge);
    }
    if req.extension.is_empty()
        || req.extension.len() > 12
        || !req.extension.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Err(E::InvalidPath("invalid asset extension".into()));
    }
    let hash = blake3::hash(&req.bytes).to_hex().to_string();
    Ok((
        AssetRef {
            path: format!("assets/{}.{}", hash, req.extension.to_ascii_lowercase()),
            media_type: req.media_type,
            hash,
        },
        req.bytes,
    ))
}
fn merge_patch(target: &mut serde_json::Value, patch: serde_json::Value) {
    if let serde_json::Value::Object(patch) = patch {
        if !target.is_object() {
            *target = serde_json::json!({});
        }
        let map = target.as_object_mut().unwrap();
        for (key, value) in patch {
            if value.is_null() {
                map.remove(&key);
            } else {
                merge_patch(map.entry(key).or_insert(serde_json::Value::Null), value);
            }
        }
    } else {
        *target = patch;
    }
}
fn summary_generator(workspace: &WorkspaceHandle) -> String {
    workspace
        .providers
        .summary
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|g| g.id().to_owned())
        .unwrap_or_default()
}
fn read_view(
    s: &crate::workspace::State,
    catalog: &Catalog,
    id: &ObjectId,
    view: ReadView,
    generator: &str,
) -> R<ObjectView> {
    let level = match view {
        ReadView::Abstract => Some(0),
        ReadView::Overview => Some(1),
        _ => None,
    };
    let mut result = catalog.view(id, view, &s.repository.repository_id)?;
    if let Some(level) = level {
        if let Some(content) =
            crate::retrieval::cached_summary(&s.root, catalog, id, level, generator)?
        {
            result.content = content
                .chars()
                .take(if level == 0 { 256 } else { 4000 })
                .collect();
        }
    }
    Ok(result)
}
