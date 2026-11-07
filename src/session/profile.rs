use super::SessionModel;
use crate::workspace::{files, WorkspaceError as E, WorkspaceResult as R};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalProfile {
    endpoint: String,
    api_key_env: String,
}
/// Resolve connection secrets at send time; neither the profile nor metadata stores a key.
pub fn resolve_profile(model: &SessionModel, root: &Path) -> R<Option<(String, String)>> {
    let path = files::safe_path(root, ".lumen/model-profiles.yaml")?;
    let profiles: BTreeMap<String, LocalProfile> = if path.is_file() {
        serde_yaml_ng::from_slice(&std::fs::read(path)?)?
    } else {
        BTreeMap::new()
    };
    if let Some(profile) = profiles.get(&model.profile_ref) {
        let key = std::env::var(&profile.api_key_env).map_err(|_| {
            E::Configuration(format!(
                "model profile {} requires environment variable {}",
                model.profile_ref, profile.api_key_env
            ))
        })?;
        if profile.endpoint.is_empty() || key.is_empty() {
            return Err(E::Configuration(
                "model profile endpoint/key is empty".into(),
            ));
        }
        return Ok(Some((profile.endpoint.clone(), key)));
    }
    if model.profile_ref != "default" {
        return Err(E::Configuration(format!(
            "missing local model profile: {}",
            model.profile_ref
        )));
    }
    match(std::env::var("LUMEN_MODEL_ENDPOINT"),std::env::var("LUMEN_MODEL_KEY")){
        (Ok(endpoint),Ok(key)) if !endpoint.is_empty()&&!key.is_empty()=>Ok(Some((endpoint,key))),
        _ if model.model=="mock"=>Ok(None),
        _=>Err(E::Configuration("model credentials unavailable; history remains readable; configure the session profile before sending".into())),
    }
}
