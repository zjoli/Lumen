use super::*;
use crate::workspace::{WorkspaceError as E, WorkspaceResult as R};
pub fn history_page(
    messages: &[HistoryMessage],
    before: Option<&str>,
    limit: usize,
) -> R<HistoryPage> {
    let end = match before {
        None => messages.len(),
        Some(key) => messages
            .iter()
            .position(|m| history_key(&m.record_ref) == key)
            .ok_or(E::CursorExpired)?,
    };
    let start = end.saturating_sub(limit.clamp(1, 200));
    Ok(HistoryPage {
        messages: messages[start..end].to_vec(),
        before: if start > 0 {
            Some(history_key(&messages[start].record_ref))
        } else {
            None
        },
    })
}
pub fn history_key(reference: &RecordRef) -> String {
    format!(
        "{}:{}:{}",
        reference.segment_id, reference.line, reference.hash
    )
}
