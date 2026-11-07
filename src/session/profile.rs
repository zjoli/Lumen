use super::SessionModel;
use crate::workspace::{WorkspaceError, WorkspaceResult};
use std::path::Path;
/// Legacy tuple API. New callers resolve a frozen model with ProviderRegistry.
pub fn resolve_profile(
    model: &SessionModel,
    root: &Path,
) -> WorkspaceResult<Option<(String, String)>> {
    let selection = model
        .selection()
        .map_err(|e| WorkspaceError::Configuration(e.to_string()))?;
    crate::provider::legacy_profile(&selection, root)
        .map_err(|e| WorkspaceError::Configuration(e.to_string()))
}
