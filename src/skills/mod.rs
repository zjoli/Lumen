use crate::{agent::*, context::*};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Debug, Serialize)]
pub struct SkillMetadata {
    pub id: String,
    pub name: String,
    pub description: String,
    pub revision: String,
    #[serde(skip)]
    pub path: PathBuf,
}
#[derive(Default)]
pub struct SkillRegistry {
    skills: Vec<SkillMetadata>,
    diagnostics: Vec<String>,
}
#[derive(Deserialize)]
struct Frontmatter {
    name: String,
    description: String,
}
impl SkillRegistry {
    pub fn discover(roots: &[(String, PathBuf)]) -> Result<Arc<Self>, AgentError> {
        let mut skills = vec![];
        let mut diagnostics = vec![];
        for (namespace, root) in roots {
            if !root.exists() {
                continue;
            }
            for entry in std::fs::read_dir(root).map_err(skill_error)? {
                let path = entry.map_err(skill_error)?.path().join("SKILL.md");
                if !path.is_file() {
                    continue;
                }
                let body = match std::fs::read_to_string(&path) {
                    Ok(body) => body,
                    Err(error) => {
                        diagnostics.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                };
                let metadata_body = body.replace("\r\n", "\n");
                let front = metadata_body
                    .strip_prefix("---\n")
                    .and_then(|s| s.split_once("\n---"))
                    .map(|p| p.0);
                let Some(front) = front else {
                    diagnostics.push(format!("{}: missing SKILL.md frontmatter", path.display()));
                    continue;
                };
                let info: Frontmatter = match serde_yaml_ng::from_str(front) {
                    Ok(info) => info,
                    Err(error) => {
                        diagnostics.push(format!("{}: {error}", path.display()));
                        continue;
                    }
                };
                if info.name.trim().is_empty() || info.description.trim().is_empty() {
                    diagnostics.push(format!("{}: empty skill name/description", path.display()));
                    continue;
                }
                let folder = path
                    .parent()
                    .and_then(Path::file_name)
                    .unwrap()
                    .to_string_lossy();
                skills.push(SkillMetadata {
                    id: format!("{namespace}:{folder}"),
                    name: info.name,
                    description: info.description,
                    revision: blake3::hash(body.as_bytes()).to_hex().to_string(),
                    path: path.canonicalize().map_err(skill_error)?,
                });
            }
        }
        skills.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Arc::new(Self {
            skills,
            diagnostics,
        }))
    }
    pub fn list(&self) -> &[SkillMetadata] {
        &self.skills
    }
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
    fn resolve(&self, id: &str) -> Result<&SkillMetadata, AgentError> {
        if let Some(skill) = self.skills.iter().find(|skill| skill.id == id) {
            return Ok(skill);
        }
        let matches: Vec<_> = self
            .skills
            .iter()
            .filter(|s| s.id == id || s.name == id)
            .collect();
        match matches.as_slice() {
            [skill] => Ok(skill),
            [] => Err(skill_error("unknown skill")),
            _ => Err(skill_error("ambiguous skill name; use source-qualified ID")),
        }
    }
    pub fn register_tools(self: &Arc<Self>, agent: &mut AgentLoop, session: Arc<SessionRuntime>) {
        for name in [
            "skill_search",
            "skill_load",
            "skill_finish",
            "skill_progress",
            "skill_resource",
        ] {
            agent.register_tool(Arc::new(SkillTool {
                name,
                registry: self.clone(),
                session: session.clone(),
            }));
        }
    }
}
fn skill_error(e: impl std::fmt::Display) -> AgentError {
    AgentError::Context(e.to_string())
}
#[async_trait]
impl AgentPlugin for SkillRegistry {
    async fn before_request(
        &self,
        event: &mut BeforeRequest<'_>,
        _: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        if let BeforeRequest::Collect(collector) = event {
            if self.skills.is_empty() {
                collector.offer(ContextContribution {
                    id: "skill-catalog".into(),
                    region: ContextRegion::Workspace,
                    source: "skill-registry".into(),
                    revision: "empty".into(),
                    content: "Skill catalog is now empty; older listings are inactive.".into(),
                    required: false,
                    retire: true,
                });
                return Ok(());
            }
            let mut catalog = "Available skills. Use skill_search to discover, skill_load to activate, skill_resource for supporting files, skill_progress to record steps, skill_finish when complete.\n".to_owned();
            for skill in &self.skills {
                let line = format!(
                    "{}: {}\n",
                    skill.id,
                    skill.description.chars().take(160).collect::<String>()
                );
                if catalog.len() + line.len() > 4000 {
                    catalog.push_str(
                        "Directory abbreviated; use skill_search for remaining skills.\n",
                    );
                    break;
                }
                catalog.push_str(&line);
            }
            collector.offer(ContextContribution {
                id: "skill-catalog".into(),
                region: ContextRegion::Workspace,
                source: "skill-registry".into(),
                revision: blake3::hash(catalog.as_bytes()).to_hex().to_string(),
                content: catalog,
                required: true,
                retire: false,
            });
        }
        Ok(())
    }
}
struct SkillTool {
    name: &'static str,
    registry: Arc<SkillRegistry>,
    session: Arc<SessionRuntime>,
}
#[async_trait]
impl AgentTool for SkillTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        match self.name {
            "skill_search" => "Find available skills by name or description.",
            "skill_load" => "Load a skill's complete workflow for the current task.",
            "skill_progress" => "Persist the current step, completed steps and notes for an active skill across compaction and restart.",
            "skill_finish" => {
                "Mark a skill inactive; its workflow is removed at the next compaction."
            }
            _ => "Read a bounded UTF-8 supporting resource within a skill directory.",
        }
    }
    fn input_schema(&self) -> Value {
        match self.name {
            "skill_progress" => {
                json!({"type":"object","properties":{"id":{"type":"string"},"step":{"type":"string"},"completed":{"type":"array","items":{"type":"string"}},"notes":{"type":"string"}},"required":["id","step"],"additionalProperties":false})
            }
            "skill_search" => {
                json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false})
            }
            "skill_resource" => {
                json!({"type":"object","properties":{"id":{"type":"string"},"path":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":8192}},"required":["id","path"],"additionalProperties":false})
            }
            _ => {
                json!({"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false})
            }
        }
    }
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolResult, AgentError> {
        if ctx.cancel.is_cancelled() {
            return Err(AgentError::Aborted);
        }
        let value = if self.name == "skill_search" {
            let query = args["query"].as_str().unwrap_or("").to_lowercase();
            json!(self
                .registry
                .skills
                .iter()
                .filter(|s| format!("{} {}", s.name, s.description)
                    .to_lowercase()
                    .contains(&query))
                .take(20)
                .collect::<Vec<_>>())
        } else {
            let skill = self.registry.resolve(args["id"].as_str().unwrap_or(""))?;
            if self.name == "skill_progress" {
                let snapshot = self.session.snapshot().await;
                let skill_id = format!("skill:{}", skill.id);
                let mut active = snapshot.window.pins.contains_key(&skill_id);
                for pending in &snapshot.pending[snapshot.window.control_through..] {
                    if pending.id == skill_id {
                        active = !pending.retire;
                    }
                }
                if !active {
                    return Err(skill_error("load the skill before updating its progress"));
                }
                let content = json!({"skill_id":skill.id,"current_step":args["step"],"completed":args.get("completed").cloned().unwrap_or_else(||json!([])),"notes":args.get("notes").cloned().unwrap_or_else(||json!(""))}).to_string();
                let update = ContextContribution {
                    id: format!("skill-progress:{}", skill.id),
                    region: ContextRegion::Skill,
                    source: "skill-progress".into(),
                    revision: blake3::hash(content.as_bytes()).to_hex().to_string(),
                    content,
                    required: true,
                    retire: false,
                };
                json!({"id":skill.id,"status":"progress saved","lumen_context_updates":[update]})
            } else if self.name == "skill_resource" {
                let root = skill.path.parent().unwrap();
                let resource = root
                    .join(args["path"].as_str().unwrap_or(""))
                    .canonicalize()
                    .map_err(skill_error)?;
                if !resource.starts_with(root) {
                    return Err(skill_error("resource escapes skill directory"));
                }
                let body = std::fs::read_to_string(&resource).map_err(skill_error)?;
                let offset = args["offset"].as_u64().unwrap_or(0) as usize;
                let limit = args["limit"].as_u64().unwrap_or(4096).min(8192) as usize;
                let content: String = body.chars().skip(offset).take(limit).collect();
                json!({"source":resource,"revision":blake3::hash(body.as_bytes()).to_hex().to_string(),"offset":offset,"content":content,"truncated":offset.saturating_add(limit)<body.chars().count()})
            } else {
                let retire = self.name == "skill_finish";
                let content = if retire {
                    format!("Skill {} completed/inactive. Do not continue applying its workflow unless reactivated.", skill.id)
                } else {
                    std::fs::read_to_string(&skill.path).map_err(skill_error)?
                };
                let id = format!("skill:{}", skill.id);
                let revision = blake3::hash(content.as_bytes()).to_hex().to_string();
                let mut updates = vec![ContextContribution {
                    id,
                    region: ContextRegion::Skill,
                    source: skill.path.display().to_string(),
                    revision,
                    content,
                    required: !retire,
                    retire,
                }];
                if retire {
                    updates.push(ContextContribution {
                        id: format!("skill-progress:{}", skill.id),
                        region: ContextRegion::Skill,
                        source: "skill-progress".into(),
                        revision: "inactive".into(),
                        content: format!("Progress for {} is inactive.", skill.id),
                        required: false,
                        retire: true,
                    });
                }
                json!({"id":skill.id,"status":if retire {"inactive"} else {"loaded"},"instructions":"Workflow state is supplied in the next model context.","lumen_context_updates":updates})
            }
        };
        Ok(ToolResult {
            content: vec![ContentPart::Text(value.to_string())],
            is_error: false,
            terminate: false,
        })
    }
}
