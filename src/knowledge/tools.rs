use super::*;
use crate::{
    agent::{AgentError, AgentLoop, AgentTool, ContentPart, ToolContext, ToolResult},
    workspace::WorkspaceHandle,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
pub const KNOWLEDGE_TOOLS: &[&str] = &[
    "knowledge_list",
    "knowledge_resolve",
    "knowledge_read",
    "knowledge_read_many",
    "knowledge_relations",
    "knowledge_search",
    "knowledge_context",
    "knowledge_create",
    "knowledge_update",
    "knowledge_delete",
    "knowledge_apply",
    "resource_import",
    "asset_read",
    "learning_record",
    "learning_events",
    "learning_state",
    "knowledge_history",
    "knowledge_diff",
    "knowledge_read_at",
];
pub fn register_knowledge_tools(agent: &mut AgentLoop, workspace: WorkspaceHandle) {
    for name in KNOWLEDGE_TOOLS {
        agent.register_tool(Arc::new(KnowledgeTool {
            name,
            workspace: workspace.clone(),
        }));
    }
}
pub struct KnowledgeTool {
    pub name: &'static str,
    pub workspace: WorkspaceHandle,
}
fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn id_schema() -> Value {
    json!({"type":"string","pattern":"^(domain|topic|concept|unit|question|resource)_[0-9a-fA-F-]{36}$"})
}
fn page_schema() -> Value {
    schema(
        json!({"limit":{"type":"integer","minimum":1,"maximum":100},"cursor":{"type":["string","null"]}}),
        &[],
    )
}
fn metadata_schema(create: bool) -> Value {
    let ids = json!({"type":"array","items":id_schema()});
    let mut value = schema(
        json!({"schema_version":{"const":1}
        ,"id":id_schema(),"type":{"enum":["domain","topic","concept","unit","question","resource"]}
        ,"title":{"type":"string","minLength":1}
        ,"aliases":{"type":"array","items":{"type":"string"}
        }
        ,"summary":{"type":["string","null"]}
        ,"member_of":ids,"requires":ids,"covers":ids,"tests":ids,"sources":ids,"steps":{"type":"array","items":schema(json!({"kind":{"enum":["read","answer"]}
        ,"target":id_schema()}
        ),&["kind","target"])}
        ,"question_kind":{"enum":["inquiry","assessment",null]}
        ,"origin":{"anyOf":[{"type":"null"}
        ,schema(json!({"uri":{"type":"string"}
        ,"captured_at":{"type":"string"}
        }
        ),&["uri","captured_at"])]}
        ,"attachments":{"type":"array","items":schema(json!({"path":{"type":"string"}
        ,"media_type":{"type":"string"}
        ,"hash":{"type":"string"}
        }
        ),&["path","media_type","hash"])}
        ,"extensions":{"type":"object"}
        }
        ),
        if create {
            &["type", "title"]
        } else {
            &["schema_version", "id", "type", "title"]
        },
    );
    if create {
        value["properties"].as_object_mut().unwrap().remove("id");
        value["properties"]
            .as_object_mut()
            .unwrap()
            .remove("schema_version");
    }
    value
}
fn object_schema(create: bool) -> Value {
    schema(
        json!({"metadata":metadata_schema(create),"body":{"type":"string"}}),
        &["metadata", "body"],
    )
}
fn revision_schema() -> Value {
    json!({"type":"string","pattern":"^[0-9a-f]{64}$"})
}
fn filter_schema() -> Value {
    schema(
        json!({"kinds":{"type":"array","items":{"enum":["domain","topic","concept","unit","question","resource"]}
        }
        ,"scope":{"anyOf":[id_schema(),{"type":"null"}
        ]}
        ,"descendants":{"type":"boolean"}
        }
        ),
        &[],
    )
}
fn search_schema() -> Value {
    schema(
        json!({"query":{"type":"string"}
        ,"mode":{"enum":["lexical","semantic","hybrid"]}
        ,"scope":{"anyOf":[id_schema(),{"type":"null"}
        ]}
        ,"descendants":{"type":"boolean"}
        ,"source_types":{"type":"array","items":{"type":"string"}
        }
        ,"levels":{"type":"array","items":{"enum":[0,1,2]}
        }
        ,"limit":{"type":"integer","minimum":1,"maximum":100}
        ,"rerank":{"type":"boolean"}
        }
        ),
        &["query"],
    )
}
fn view_schema() -> Value {
    json!({"anyOf":[{"enum":["abstract","overview","full"]},schema(json!({"section":{"type":"string"}}),&["section"]) ]})
}
fn read_schema() -> Value {
    schema(json!({"id":id_schema(),"view":view_schema()}), &["id"])
}
fn parse<T: serde::de::DeserializeOwned>(
    value: Value,
) -> Result<T, crate::workspace::WorkspaceError> {
    Ok(serde_json::from_value(value)?)
}
fn field<T: serde::de::DeserializeOwned>(
    args: &Value,
    name: &str,
) -> Result<T, crate::workspace::WorkspaceError> {
    parse(args.get(name).cloned().unwrap_or(Value::Null))
}
fn default_field<T: serde::de::DeserializeOwned + Default>(
    args: &Value,
    name: &str,
) -> Result<T, crate::workspace::WorkspaceError> {
    match args.get(name) {
        Some(v) => parse(v.clone()),
        None => Ok(T::default()),
    }
}
fn create_object(mut value: Value) -> Result<KnowledgeObject, crate::workspace::WorkspaceError> {
    let kind: ObjectKind = parse(value["metadata"]["type"].clone())?;
    value["metadata"]["id"] = json!(ObjectId::new(kind));
    value["metadata"]["schema_version"] = json!(1);
    parse(value)
}
#[async_trait]
impl AgentTool for KnowledgeTool {
    fn name(&self) -> &str {
        self.name
    }
    fn description(&self) -> &str {
        "Read, retrieve, or save local learning knowledge. Writes remain local until the user commits and synchronizes."
    }
    fn input_schema(&self) -> Value {
        match self.name {
            "knowledge_list" => schema(json!({"filter":filter_schema(),"page":page_schema()}), &[]),
            "knowledge_resolve" => schema(json!({"query":{"type":"string"}}), &["query"]),
            "knowledge_read" => read_schema(),
            "knowledge_read_many" => schema(
                json!({"requests":{"type":"array","maxItems":100,"items":read_schema()}}),
                &["requests"],
            ),
            "knowledge_relations" => schema(
                json!({"id":id_schema(),"query":schema(json!({"incoming":{"type":"boolean"}
                ,"kind":{"type":["string","null"]}
                ,"typed":{"type":["boolean","null"]}
                }
                ),&[]),"page":page_schema()}
                ),
                &["id"],
            ),
            "knowledge_search" => search_schema(),
            "knowledge_context" => schema(
                json!({"search":search_schema(),"expansion":schema(json!({"hops":{"type":"integer","minimum":0,"maximum":2}
                ,"max_objects":{"type":"integer","minimum":1,"maximum":100}
                }
                ),&[]),"token_budget":{"type":"integer","minimum":1,"maximum":32000}
                }
                ),
                &["search"],
            ),
            "knowledge_create" => object_schema(true),
            "knowledge_update" => {
                let mut patch = metadata_schema(true);
                patch["required"] = json!([]);
                patch["properties"].as_object_mut().unwrap().remove("type");
                for value in patch["properties"].as_object_mut().unwrap().values_mut() {
                    *value = json!({"anyOf":[value.clone(),{"type":"null"}]});
                }
                json!({"oneOf":[schema(json!({"object":object_schema(false),"expected_revision":revision_schema()}
                ),&["object","expected_revision"]),schema(json!({"id":id_schema(),"metadata_patch":patch,"body":{"type":["string","null"]}
                ,"expected_revision":revision_schema()}
                ),&["id","expected_revision"])]}
                )
            }
            "knowledge_delete" => schema(
                json!({"id":id_schema(),"expected_revision":revision_schema()}),
                &["id", "expected_revision"],
            ),
            "knowledge_apply" => schema(
                json!({"changes":{"type":"array","maxItems":100,"items":{"oneOf":[schema(json!({"operation":{"const":"create"}
                ,"object":object_schema(false)}
                ),&["operation","object"]),schema(json!({"operation":{"const":"update"}
                ,"object":object_schema(false),"expected_revision":revision_schema()}
                ),&["operation","object","expected_revision"]),schema(json!({"operation":{"const":"delete"}
                ,"id":id_schema(),"expected_revision":revision_schema()}
                ),&["operation","id","expected_revision"])]}
                }
                }
                ),
                &["changes"],
            ),
            "resource_import" => schema(
                json!({"object":object_schema(true),"assets":{"type":"array","items":schema(json!({"bytes":{"type":"array","items":{"type":"integer","minimum":0,"maximum":255}
                }
                ,"extension":{"type":"string"}
                ,"media_type":{"type":"string"}
                }
                ),&["bytes","extension","media_type"])}
                }
                ),
                &["object"],
            ),
            "asset_read" => schema(
                json!({"asset":schema(json!({"path":{"type":"string"}
                ,"media_type":{"type":"string"}
                ,"hash":{"type":"string"}
                }
                ),&["path","media_type","hash"]),"range":schema(json!({"start":{"type":"integer","minimum":0}
                ,"end":{"type":["integer","null"],"minimum":0}
                }
                ),&[])}
                ),
                &["asset"],
            ),
            "learning_record" => schema(
                json!({"event_kind":{"enum":["study","answer","review","assessment","correction"]}
                ,"occurred_at":{"type":["string","null"]}
                ,"targets":{"type":"array","minItems":1,"items":id_schema()}
                ,"session_id":{"type":["string","null"]}
                ,"supersedes":{"type":["string","null"]}
                ,"data":{}
                ,"body":{"type":"string"}
                }
                ),
                &["event_kind", "targets"],
            ),
            "learning_events" => schema(
                json!({"filter":schema(json!({"target":{"anyOf":[id_schema(),{"type":"null"}
                ]}
                ,"event_kind":{"enum":["study","answer","review","assessment","correction",null]}
                }
                ),&[]),"page":page_schema()}
                ),
                &[],
            ),
            "learning_state" => schema(
                json!({"ids":{"type":"array","maxItems":100,"items":id_schema()}}),
                &["ids"],
            ),
            "knowledge_history" => schema(
                json!({"object_id":{"anyOf":[id_schema(),{"type":"null"}]},"page":page_schema()}),
                &[],
            ),
            "knowledge_diff" => schema(
                json!({"object_id":{"anyOf":[id_schema(),{"type":"null"}]}}),
                &[],
            ),
            "knowledge_read_at" => schema(
                json!({"id":id_schema(),"commit":{"type":"string"}}),
                &["id", "commit"],
            ),
            _ => schema(json!({}), &[]),
        }
    }
    async fn execute(&self, args: Value, ctx: ToolContext) -> Result<ToolResult, AgentError> {
        // Validate here as well: direct callers need the same contract as ToolRegistry.
        let schema = self.input_schema();
        let validator = jsonschema::validator_for(&schema)
            .map_err(|e| AgentError::InvalidArguments(e.to_string()))?;
        if let Err(e) = validator.validate(&args) {
            return Err(AgentError::InvalidArguments(e.to_string()));
        }
        let c = ctx.cancel;
        let k = self.workspace.knowledge();
        let result:crate::workspace::WorkspaceResult<Value>=async{Ok(match self.name{
            "knowledge_list"=>serde_json::to_value(k.list(default_field(&args,"filter")?,default_field(&args,"page")?,c).await?)?,
            "knowledge_resolve"=>json!(k.resolve(field(&args,"query")?,c).await?),
            "knowledge_read"=>json!(k.read(field(&args,"id")?,default_field(&args,"view")?,c).await?),
            "knowledge_read_many"=>json!(k.read_many(field(&args,"requests")?,c).await?),
            "knowledge_relations"=>json!(k.relations(field(&args,"id")?,default_field(&args,"query")?,default_field(&args,"page")?,c).await?),
            "knowledge_search"=>json!(self.workspace.retrieval().search(parse(args)?,c).await?),
            "knowledge_context"=>json!(self.workspace.retrieval().context(parse(args)?,c).await?),
            "knowledge_create"=>json!(k.create(create_object(args)?,c).await?),
            "knowledge_update"=>if args.get("object").is_some(){json!(k.update(parse(args)?,c).await?)}else{json!(k.patch(parse(args)?,c).await?)},
            "knowledge_delete"=>json!(k.delete(parse(args)?,c).await?),
            "knowledge_apply"=>json!(k.apply(parse(args)?,c).await?),
            "resource_import"=>{let object=create_object(args["object"].clone())?;
json!(self.workspace.resources().import(ImportSnapshot{object,assets:default_field(&args,"assets")?}
,c).await?)}
,
            "asset_read"=>json!({"bytes":self.workspace.resources().read_asset(field(&args,"asset")?,default_field(&args,"range")?,c).await?}),
            "learning_record"=>json!(self.workspace.learning().append(parse(args)?,c).await?),
            "learning_events"=>json!(self.workspace.learning().events(default_field(&args,"filter")?,default_field(&args,"page")?,c).await?),
            "learning_state"=>json!(self.workspace.learning().state(field(&args,"ids")?,c).await?),
            "knowledge_history"=>json!(self.workspace.versions().history(crate::versioning::HistoryQuery{object_id:default_field(&args,"object_id")?}
,default_field(&args,"page")?,c).await?),
            "knowledge_diff"=>json!(self.workspace.versions().diff(crate::versioning::DiffRequest{object_id:default_field(&args,"object_id")?,..Default::default()},c).await?),
            "knowledge_read_at"=>json!(self.workspace.versions().read_at(field(&args,"id")?,field(&args,"commit")?,c).await?),
            _=>return Err(crate::workspace::WorkspaceError::NotFound(self.name.into())),
        })}.await;
        match result {
            Ok(value) => Ok(ToolResult {
                content: vec![ContentPart::Text(value.to_string())],
                is_error: false,
                terminate: false,
            }),
            Err(crate::workspace::WorkspaceError::Cancelled) => Err(AgentError::Aborted),
            Err(e) => Ok(ToolResult {
                content: vec![ContentPart::Text(json!({"error":e}).to_string())],
                is_error: true,
                terminate: false,
            }),
        }
    }
}
