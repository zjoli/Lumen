use super::*;
use crate::agent::{AgentMessage, SessionRecord};
use serde::{Deserialize, Serialize};
use std::io::Write;
#[derive(Clone)]
pub struct SessionArchive(pub(crate) WorkspaceHandle);
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SegmentInfo {
    pub session_id: String,
    pub segment_id: String,
    pub path: String,
}
pub type SegmentHandle = SegmentInfo;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub segments: Vec<SegmentInfo>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub info: SegmentInfo,
    pub messages: Vec<AgentMessage>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionTranscript {
    pub session_id: String,
    pub segments: Vec<TranscriptSegment>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SessionFilter {
    pub session_id: Option<String>,
}
pub(crate) fn validate_typed_id(id: &str, kind: &str) -> WorkspaceResult<()> {
    let u = id
        .strip_prefix(&format!("{kind}_"))
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .ok_or_else(|| WorkspaceError::InvalidFormat(format!("invalid {kind} ID")))?;
    if u.get_version_num() != 7 {
        return Err(WorkspaceError::InvalidFormat(format!("invalid {kind} ID")));
    }
    Ok(())
}
fn segment_info(path: &str) -> WorkspaceResult<SegmentInfo> {
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0] != "sessions" {
        return Err(WorkspaceError::InvalidPath(path.into()));
    }
    validate_typed_id(parts[1], "session")?;
    let segment = parts[2]
        .strip_suffix(".jsonl")
        .ok_or_else(|| WorkspaceError::InvalidPath(path.into()))?;
    validate_typed_id(segment, "segment")?;
    Ok(SegmentInfo {
        session_id: parts[1].into(),
        segment_id: segment.into(),
        path: path.into(),
    })
}
fn parse(bytes: &[u8]) -> WorkspaceResult<Vec<AgentMessage>> {
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(WorkspaceError::InvalidFormat(
            "incomplete JSONL record".into(),
        ));
    }
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|line| Ok(serde_json::from_slice::<SessionRecord>(line)?.message))
        .collect()
}
pub(crate) fn validate_segment(path: &str, bytes: &[u8]) -> WorkspaceResult<()> {
    segment_info(path)?;
    parse(bytes)?;
    Ok(())
}
fn find_segment(root: &Path, id: &str, open: bool) -> WorkspaceResult<SegmentInfo> {
    validate_typed_id(id, "segment")?;
    for path in segment_paths(root)? {
        let name = if open {
            format!("{id}.jsonl.open")
        } else {
            format!("{id}.jsonl")
        };
        if path.ends_with(&format!("/{name}")) {
            let closed = path.strip_suffix(".open").unwrap_or(&path);
            return segment_info(closed);
        }
    }
    Err(WorkspaceError::NotFound(id.into()))
}
fn segment_paths(root: &Path) -> WorkspaceResult<Vec<String>> {
    let dir = files::safe_path(root, "sessions")?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in fs::read_dir(dir)? {
        let p = entry?.path();
        if fs::symlink_metadata(&p)?.file_type().is_symlink() {
            return Err(WorkspaceError::InvalidPath(p.display().to_string()));
        }
        if !p.is_dir() {
            return Err(WorkspaceError::InvalidFormat(
                "invalid sessions directory".into(),
            ));
        }
        for entry in fs::read_dir(p)? {
            let p = entry?.path();
            let rel = p
                .strip_prefix(root)
                .map_err(|_| WorkspaceError::InvalidPath(p.display().to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            files::safe_path(root, &rel)?;
            out.push(rel);
        }
    }
    out.sort();
    Ok(out)
}
impl SessionArchive {
    pub async fn begin(
        &self,
        session_id: String,
        cancel: CancellationToken,
    ) -> WorkspaceResult<SegmentHandle> {
        self.0
            .call(cancel, move |s, _| {
                s.ensure_writable()?;
                validate_typed_id(&session_id, "session")?;
                let id = format!("segment_{}", uuid::Uuid::now_v7());
                let lease = segment_lease(&s.root, &id)?;
                fs2::FileExt::try_lock_exclusive(&lease)
                    .map_err(|_| WorkspaceError::WorkspaceBusy)?;
                let path = format!("sessions/{session_id}/{id}.jsonl");
                files::atomic_write(&s.root, &format!("{path}.open"), b"")?;
                s.segment_locks.insert(id.clone(), lease);
                Ok(SegmentInfo {
                    session_id,
                    segment_id: id,
                    path,
                })
            })
            .await
    }
    pub async fn append(
        &self,
        segment_id: String,
        message: AgentMessage,
        cancel: CancellationToken,
    ) -> WorkspaceResult<()> {
        self.0
            .call(cancel, move |s, _| {
                s.ensure_writable()?;
                if !s.segment_locks.contains_key(&segment_id) {
                    return Err(WorkspaceError::WorkspaceBusy);
                }
                let info = find_segment(&s.root, &segment_id, true)?;
                let path = files::safe_path(&s.root, &format!("{}.open", info.path))?;
                let mut file = fs::OpenOptions::new().append(true).open(path)?;
                let mut line = serde_json::to_vec(&SessionRecord { message })?;
                line.push(b'\n');
                file.write_all(&line)?;
                file.sync_data()?;
                Ok(())
            })
            .await
    }
    pub async fn finish(
        &self,
        segment_id: String,
        cancel: CancellationToken,
    ) -> WorkspaceResult<SegmentInfo> {
        self.0
            .call(cancel, move |s, _| {
                if !s.segment_locks.contains_key(&segment_id) {
                    return Err(WorkspaceError::WorkspaceBusy);
                }
                let info = find_segment(&s.root, &segment_id, true)?;
                let open = files::safe_path(&s.root, &format!("{}.open", info.path))?;
                let bytes = fs::read(&open)?;
                validate_segment(&info.path, &bytes)?;
                fs::rename(open, files::safe_path(&s.root, &info.path)?)?;
                s.segment_locks.remove(&segment_id);
                Ok(info)
            })
            .await
    }
    pub async fn list(
        &self,
        filter: SessionFilter,
        page: PageRequest,
        cancel: CancellationToken,
    ) -> WorkspaceResult<Page<SessionInfo>> {
        self.0
            .call(cancel, move |s, _| {
                let tree = files::scan(&s.root)?;
                let mut groups = std::collections::BTreeMap::<String, Vec<SegmentInfo>>::new();
                for (path, bytes) in &tree {
                    if path.starts_with("sessions/") {
                        validate_segment(path, bytes)?;
                        let info = segment_info(path)?;
                        if filter
                            .session_id
                            .as_ref()
                            .map(|id| *id == info.session_id)
                            .unwrap_or(true)
                        {
                            groups
                                .entry(info.session_id.clone())
                                .or_default()
                                .push(info)
                        }
                    }
                }
                let items = groups
                    .into_iter()
                    .map(|(session_id, segments)| SessionInfo {
                        session_id,
                        segments,
                    })
                    .collect::<Vec<_>>();
                paginate(&items, page, &files::fingerprint(&tree))
            })
            .await
    }
    pub async fn load(
        &self,
        session_id: String,
        cancel: CancellationToken,
    ) -> WorkspaceResult<SessionTranscript> {
        self.0
            .call(cancel, move |s, _| {
                validate_typed_id(&session_id, "session")?;
                let mut segments = vec![];
                for (path, bytes) in files::scan(&s.root)? {
                    if path.starts_with(&format!("sessions/{session_id}/")) {
                        let info = segment_info(&path)?;
                        segments.push(TranscriptSegment {
                            info,
                            messages: parse(&bytes)?,
                        })
                    }
                }
                Ok(SessionTranscript {
                    session_id,
                    segments,
                })
            })
            .await
    }
}
pub(crate) fn recover_segments(root: &Path) -> WorkspaceResult<usize> {
    let mut count = 0;
    for path in segment_paths(root)? {
        if !path.ends_with(".jsonl.open") {
            continue;
        }
        let open = files::safe_path(root, &path)?;
        let bytes = fs::read(&open)?;
        let end = bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let closed = path.strip_suffix(".open").unwrap();
        let info = segment_info(closed)?;
        let lease = segment_lease(root, &info.segment_id)?;
        if fs2::FileExt::try_lock_exclusive(&lease).is_err() {
            continue;
        }
        validate_segment(closed, &bytes[..end])?;
        if end != bytes.len() {
            files::atomic_write(
                root,
                &format!(".lumen/recovered-{}.jsonl", uuid::Uuid::now_v7()),
                &bytes,
            )?;
            files::atomic_write(root, &path, &bytes[..end])?
        }
        fs::rename(open, files::safe_path(root, closed)?)?;
        count += 1;
    }
    Ok(count)
}
fn segment_lease(root: &Path, id: &str) -> WorkspaceResult<fs::File> {
    validate_typed_id(id, "segment")?;
    Ok(fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(files::safe_path(root, &format!(".lumen/locks/{id}.lock"))?)?)
}
pub(crate) fn finish_all(s: &mut State) -> WorkspaceResult<usize> {
    let ids = s.segment_locks.keys().cloned().collect::<Vec<_>>();
    for id in &ids {
        let info = find_segment(&s.root, id, true)?;
        let path = format!("{}.open", info.path);
        let bytes = fs::read(files::safe_path(&s.root, &path)?)?;
        validate_segment(&info.path, &bytes)?;
        fs::rename(
            files::safe_path(&s.root, &path)?,
            files::safe_path(&s.root, &info.path)?,
        )?;
        s.segment_locks.remove(id);
    }
    Ok(ids.len() + recover_segments(&s.root)?)
}
