use super::types::*;
use crate::workspace::{WorkspaceError, WorkspaceResult};
use serde::{de::DeserializeOwned, Serialize};

pub fn decode<T: DeserializeOwned>(text: &str) -> WorkspaceResult<(T, String)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or_else(|| WorkspaceError::InvalidFormat("missing frontmatter".into()))?;
    let end = text
        .split_inclusive('\n')
        .scan(0usize, |pos, line| {
            let at = *pos;
            *pos += line.len();
            Some((at, line))
        })
        .find(|(_, line)| line.trim_end_matches(['\r', '\n']) == "---")
        .map(|(at, line)| (at, at + line.len()))
        .ok_or_else(|| WorkspaceError::InvalidFormat("unclosed frontmatter".into()))?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text[..end.0])?;
    reject_tags(&value)?;
    let metadata = serde_yaml_ng::from_value(value)?;
    let rest = &text[end.1..];
    let body = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
        .unwrap_or(rest);
    Ok((metadata, body.to_string()))
}
fn reject_tags(v: &serde_yaml_ng::Value) -> WorkspaceResult<()> {
    use serde_yaml_ng::Value::*;
    match v {
        Tagged(_) => {
            return Err(WorkspaceError::InvalidFormat(
                "YAML tags are not allowed".into(),
            ))
        }
        Sequence(a) => {
            for v in a {
                reject_tags(v)?
            }
        }
        Mapping(m) => {
            for (k, v) in m {
                reject_tags(k)?;
                reject_tags(v)?
            }
        }
        _ => (),
    }
    Ok(())
}
pub fn encode<T: Serialize>(metadata: &T, body: &str) -> WorkspaceResult<Vec<u8>> {
    Ok(format!(
        "---\n{}---\n\n{}",
        serde_yaml_ng::to_string(metadata)?,
        body
    )
    .into_bytes())
}
pub fn parse_object(text: &str) -> WorkspaceResult<KnowledgeObject> {
    let (metadata, body): (ObjectMetadata, String) = decode(text)?;
    if metadata.schema_version != 1 {
        return Err(WorkspaceError::UnsupportedSchema(metadata.schema_version));
    }
    if metadata.id.kind() != metadata.kind || metadata.title.trim().is_empty() {
        return Err(WorkspaceError::InvalidFormat(
            "ID/type mismatch or empty title".into(),
        ));
    }
    Ok(KnowledgeObject { metadata, body })
}
pub fn revision(bytes: &[u8]) -> Revision {
    Revision(blake3::hash(bytes).to_hex().to_string())
}
