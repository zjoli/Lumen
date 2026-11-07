use super::*;
use crate::{
    agent::{AgentMessage, ContentPart, SessionRecord},
    workspace::{WorkspaceError as E, WorkspaceResult as R},
};
use std::collections::{BTreeMap, BTreeSet};

pub fn decode_records(bytes: &[u8]) -> R<Vec<AgentMessage>> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(E::InvalidFormat("incomplete JSONL record".into()));
    }
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|line| {
            serde_json::from_slice::<SessionRecord>(line)
                .map(|r| r.message)
                .map_err(Into::into)
        })
        .collect()
}
pub fn validate_session_file(path: &str, bytes: &[u8]) -> R<()> {
    let parts: Vec<_> = path.split('/').collect();
    if parts.len() != 3 || parts[0] != "sessions" {
        return Err(E::InvalidPath(path.into()));
    }
    crate::workspace::validate_typed_id(parts[1], "session")?;
    if parts[2] == "session.yaml" {
        let meta: SessionMetadata = serde_yaml_ng::from_slice(bytes)?;
        meta.validate()?;
        if meta.id != parts[1] {
            return Err(E::InvalidFormat(
                "session metadata identity mismatch".into(),
            ));
        }
        return Ok(());
    }
    let id = parts[2]
        .strip_suffix(".jsonl")
        .ok_or_else(|| E::InvalidPath(path.into()))?;
    crate::workspace::validate_typed_id(id, "segment")?;
    let records = decode_records(bytes)?;
    for (index, record) in records.iter().enumerate() {
        if let AgentMessage::Custom { kind, content } = record {
            if kind == "lumen.segment_open" {
                let open: SegmentOpen = serde_json::from_value(content.clone())?;
                if !matches!(open.schema_version, 1 | 2) {
                    return Err(E::UnsupportedSchema(open.schema_version));
                }
                if index != 0 || open.session_id != parts[1] || open.segment_id != id {
                    return Err(E::InvalidFormat("invalid segment header".into()));
                }
            }
            if kind == "lumen.run_end" {
                let end: RunEnd = serde_json::from_value(content.clone())?;
                if !matches!(end.schema_version, 1 | 2) {
                    return Err(E::UnsupportedSchema(end.schema_version));
                }
                if index + 1 != records.len() {
                    return Err(E::InvalidFormat("run_end must be final record".into()));
                }
            }
        }
    }
    Ok(())
}

pub fn replay(session_id: &str, segments: &[StoredSegment]) -> R<ReplayReport> {
    let mut report = ReplayReport {
        records: vec![],
        history: vec![],
        health: SessionHealth::Complete,
        diagnostics: vec![],
        head: None,
        legacy_prefix: vec![],
        last_stop: None,
        updated_at: None,
    };
    let mut legacy = vec![];
    let mut modern = BTreeMap::new();
    let mut decoded = BTreeMap::new();
    for segment in segments {
        let records = match decode_records(&segment.bytes) {
            Ok(r) => r,
            Err(e) => {
                report.health = SessionHealth::Corrupt;
                report.diagnostics.push(format!("{}: {e}", segment.id));
                // Read-only diagnostics can show the valid prefix; corruption never
                // gets skipped to fabricate a writable history.
                let mut prefix = vec![];
                for line in segment
                    .bytes
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                {
                    match serde_json::from_slice::<SessionRecord>(line) {
                        Ok(record) => prefix.push(record.message),
                        Err(_) => break,
                    }
                }
                prefix
            }
        };
        if let Some(AgentMessage::Custom { kind, content }) = records
            .first()
            .filter(|m| matches!(m,AgentMessage::Custom{kind,..}if kind=="lumen.segment_open"))
        {
            let _ = kind;
            match serde_json::from_value::<SegmentOpen>(content.clone()) {
                Ok(open)
                    if matches!(open.schema_version, 1 | 2)
                        && open.session_id == session_id
                        && open.segment_id == segment.id =>
                {
                    modern.insert(segment.id.clone(), open);
                }
                _ => {
                    legacy.push(segment.id.clone());
                    report.health = SessionHealth::Unsupported;
                    report
                        .diagnostics
                        .push(format!("{}: invalid or unsupported header", segment.id));
                }
            }
        } else {
            legacy.push(segment.id.clone());
        }
        decoded.insert(segment.id.clone(), records);
    }
    legacy.sort();
    let mut ordered = legacy.clone();
    let hashes: BTreeMap<_, _> = segments
        .iter()
        .map(|s| (s.id.clone(), content_hash(&s.bytes)))
        .collect();
    report.legacy_prefix = legacy
        .iter()
        .map(|id| SegmentParent {
            segment_id: id.clone(),
            hash: hashes[id].clone(),
        })
        .collect();
    let mut children: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    for (id, open) in &modern {
        let parent = open.parent.as_ref().map(|p| p.segment_id.clone());
        let valid = if let Some(p) = &open.parent {
            hashes.get(&p.segment_id) == Some(&p.hash)
        } else {
            open.legacy_prefix
                .iter()
                .map(|p| (&p.segment_id, &p.hash))
                .eq(report
                    .legacy_prefix
                    .iter()
                    .map(|p| (&p.segment_id, &p.hash)))
        };
        if !valid {
            report.health = SessionHealth::Diverged;
            report
                .diagnostics
                .push(format!("{id}: parent or legacy prefix mismatch"));
        }
        children.entry(parent).or_default().push(id.clone());
    }
    let mut parent = None;
    let mut visited = BTreeSet::new();
    while let Some(next) = children.get(&parent) {
        if next.len() != 1 {
            report.health = SessionHealth::Diverged;
            report
                .diagnostics
                .push("multiple continuation segments share the same parent".into());
            break;
        }
        let id = next[0].clone();
        if !visited.insert(id.clone()) {
            report.health = SessionHealth::Diverged;
            break;
        }
        ordered.push(id.clone());
        parent = Some(id);
    }
    if visited.len() != modern.len() {
        report.health = SessionHealth::Diverged;
        report.diagnostics.push(
            "history contains disconnected or diverged segments; continuation disabled".into(),
        );
    }
    // Diverged histories remain browsable, but never feed a fabricated chain to the runtime.
    if report.health == SessionHealth::Diverged {
        for id in modern.keys() {
            if !ordered.contains(id) {
                ordered.push(id.clone());
            }
        }
    }
    for id in ordered {
        let Some(records) = decoded.remove(&id) else {
            continue;
        };
        let mut run_id = None;
        let mut ended = false;
        for (line, message) in records.into_iter().enumerate() {
            if ended {
                report.health = SessionHealth::Corrupt;
                report
                    .diagnostics
                    .push(format!("{id}: records follow run_end"));
            }
            if let AgentMessage::Custom { kind, content } = &message {
                if kind == "lumen.segment_open" {
                    if line != 0 {
                        report.health = SessionHealth::Corrupt;
                        report
                            .diagnostics
                            .push(format!("{id}: header is not the first record"));
                    }
                    let open: SegmentOpen = match serde_json::from_value(content.clone()) {
                        Ok(open) => open,
                        Err(error) => {
                            report.health = SessionHealth::Unsupported;
                            report.diagnostics.push(format!("{id}: {error}"));
                            continue;
                        }
                    };
                    run_id = Some(open.run_id);
                    report.updated_at = Some(open.started_at);
                    continue;
                }
                if kind == "lumen.run_end" {
                    let end: RunEnd = match serde_json::from_value(content.clone()) {
                        Ok(end) => end,
                        Err(error) => {
                            report.health = SessionHealth::Unsupported;
                            report.diagnostics.push(format!("{id}: {error}"));
                            continue;
                        }
                    };
                    if !matches!(end.schema_version, 1 | 2) || run_id.as_ref() != Some(&end.run_id)
                    {
                        report.health = SessionHealth::Corrupt;
                        report.diagnostics.push("run_end identity mismatch".into());
                    }
                    report.last_stop = Some(end.reason);
                    report.updated_at = Some(end.ended_at);
                    ended = true;
                    continue;
                }
            }
            let visible = match &message {
                AgentMessage::User(parts) => Some(("user", parts)),
                AgentMessage::Assistant(a) => Some(("assistant", &a.content)),
                _ => None,
            };
            if let Some((role, parts)) = visible {
                let text = parts
                    .iter()
                    .map(|ContentPart::Text(s)| s.as_str())
                    .collect::<String>();
                if !text.is_empty() {
                    report.history.push(HistoryMessage {
                        record_ref: RecordRef {
                            segment_id: id.clone(),
                            line,
                            hash: content_hash(&serde_json::to_vec(&message)?),
                        },
                        role: role.into(),
                        text,
                    });
                }
            }
            if matches!(&message,AgentMessage::Assistant(a) if a.provider_replay.as_ref().is_some_and(|r|r.schema_version != 1))
            {
                report.health = SessionHealth::Unsupported;
                report
                    .diagnostics
                    .push(format!("{id}: unsupported native replay schema"));
            }
            report.records.push(message);
        }
        if run_id.is_some() && !ended && report.health == SessionHealth::Complete {
            report.health = SessionHealth::Interrupted;
            report.diagnostics.push(format!("{id}: interrupted run"));
        }
        report.head = Some(SegmentParent {
            segment_id: id.clone(),
            hash: hashes[&id].clone(),
        });
    }
    if matches!(
        report.health,
        SessionHealth::Complete | SessionHealth::Interrupted
    ) {
        if let Err(error) = SessionRuntime::from_records(report.records.clone(), None) {
            report.health = SessionHealth::Corrupt;
            report
                .diagnostics
                .push(format!("context replay failed: {error}"));
        }
    }
    Ok(report)
}
