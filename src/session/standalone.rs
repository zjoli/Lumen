use super::*;
use crate::{
    agent::{AgentMessage, SessionRecord},
    workspace::{WorkspaceError as E, WorkspaceResult as R},
};
use std::{collections::BTreeMap, path::Path};
impl SessionManager {
    pub async fn import_legacy(&mut self, path: &Path) -> R<Vec<String>> {
        let bytes = std::fs::read(path)?;
        let groups = legacy_groups(&bytes)?;
        let mut imported = vec![];
        for (old_id, records) in groups {
            let mut body = vec![];
            for message in &records {
                serde_json::to_writer(
                    &mut body,
                    &SessionRecord {
                        message: message.clone(),
                    },
                )?;
                body.push(b'\n');
            }
            let digest = content_hash(&body);
            let id = old_id
                .filter(|id| crate::workspace::validate_typed_id(id, "session").is_ok())
                .unwrap_or_else(|| format!("session_{}", stable_uuid(&digest)));
            let segment_id = format!("segment_{}", stable_uuid(&format!("segment:{digest}")));
            let loaded = self.repository().load(&id).await;
            if let Ok(loaded) = loaded {
                if loaded
                    .segments
                    .iter()
                    .any(|segment| segment.id == segment_id && segment.bytes == body)
                {
                    imported.push(id);
                    continue;
                }
                return Err(E::RevisionConflict(format!(
                    "legacy import would overwrite existing session {id}"
                )));
            } else if !matches!(loaded, Err(E::NotFound(_))) {
                loaded?;
            }
            SessionRuntime::from_records(records.clone(), None)
                .map_err(|error| E::SessionUnavailable(format!("legacy {id}: {error}")))?;
            let title = records
                .iter()
                .find_map(|m| {
                    if let AgentMessage::User(parts) = m {
                        Some(
                            parts
                                .iter()
                                .map(|crate::agent::ContentPart::Text(s)| s.as_str())
                                .collect::<String>()
                                .chars()
                                .take(60)
                                .collect::<String>(),
                        )
                    } else {
                        None
                    }
                })
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "导入会话".into());
            let mut metadata = SessionMetadata::new(
                Some(title),
                SessionModel::default(),
                SessionContext::default(),
            );
            metadata.id = id.clone();
            metadata.title_source = "imported".into();
            self.repository()
                .publish_import(&metadata, &segment_id, &body)
                .await?;
            imported.push(id);
        }
        Ok(imported)
    }
}
fn legacy_groups(bytes: &[u8]) -> R<Vec<(Option<String>, Vec<AgentMessage>)>> {
    let records = decode_records(bytes)?;
    let mut groups: Vec<(Option<String>, Vec<AgentMessage>)> = vec![];
    let mut id = None;
    let mut group = vec![];
    for record in records {
        if let AgentMessage::Custom { kind, content } = &record {
            if kind == "lumen.session_start" {
                if !group.is_empty() {
                    groups.push((id.take(), std::mem::take(&mut group)));
                }
                id = content["session_id"].as_str().map(str::to_owned);
                continue;
            }
        }
        group.push(record);
    }
    if !group.is_empty() {
        groups.push((id, group));
    }
    Ok(groups)
}
/// Inspect every legacy session without changing the source or publishing imported state.
pub fn preview_legacy(path: &Path) -> R<Vec<SessionView>> {
    let bytes = std::fs::read(path)?;
    let mut views = vec![];
    for (old_id, records) in legacy_groups(&bytes)? {
        let mut body = vec![];
        for record in &records {
            serde_json::to_writer(
                &mut body,
                &SessionRecord {
                    message: record.clone(),
                },
            )?;
            body.push(b'\n');
        }
        let id = old_id
            .filter(|id| crate::workspace::validate_typed_id(id, "session").is_ok())
            .unwrap_or_else(|| format!("session_{}", stable_uuid(&content_hash(&body))));
        let report = replay(
            &id,
            &[StoredSegment {
                id: format!("segment_{}", stable_uuid(&content_hash(&body))),
                bytes: body,
            }],
        )?;
        let title = report
            .history
            .first()
            .map(|message| message.text.chars().take(60).collect())
            .unwrap_or_else(|| "旧会话".into());
        let mut metadata = SessionMetadata::new(
            Some(title),
            SessionModel::default(),
            SessionContext::default(),
        );
        metadata.id = id.clone();
        metadata.title_source = "imported".into();
        views.push(SessionView {
            info: ManagedSessionInfo {
                identity: SessionIdentity {
                    store_id: format!("legacy:{}", content_hash(&bytes)),
                    session_id: id,
                },
                updated_at: metadata.created_at.clone(),
                metadata,
                revision: content_hash(&bytes),
                message_count: report.history.len(),
                health: report.health,
                diagnostics: report.diagnostics,
                last_stop: report.last_stop,
                legacy: true,
            },
            history: history_page(&report.history, None, 200)?,
            ui: Default::default(),
            writable: false,
            activation: 0,
        });
    }
    Ok(views)
}
fn stable_uuid(value: &str) -> uuid::Uuid {
    let digest = blake3::hash(value.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}
impl SessionRepository {
    pub(crate) async fn publish_import(
        &self,
        metadata: &SessionMetadata,
        segment: &str,
        body: &[u8],
    ) -> R<()> {
        metadata.validate()?;
        let _lease = self.lease(&metadata.id)?;
        let mut entries = BTreeMap::new();
        entries.insert(
            format!("sessions/{}/session.yaml", metadata.id),
            Some(serde_yaml_ng::to_string(metadata)?.into_bytes()),
        );
        entries.insert(
            format!("sessions/{}/{segment}.jsonl", metadata.id),
            Some(body.to_vec()),
        );
        self.publish_entries(entries).await
    }
}
