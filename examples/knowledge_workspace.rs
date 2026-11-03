use lumen::{knowledge::*, retrieval::SearchRequest, workspace::*};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: cargo run --example knowledge_workspace -- /path/to/new/knowledge")?;
    let workspace = WorkspaceService::init(
        InitRequest {
            root,
            author_name: "Learner".into(),
            author_email: "learner@localhost".into(),
        },
        CancellationToken::new(),
    )
    .await?;
    let concept = workspace
        .knowledge()
        .create(
            KnowledgeObject {
                metadata: ObjectMetadata::new(ObjectKind::Concept, "事务隔离性"),
                body: "隔离性描述并发事务之间的可见性约束。".into(),
            },
            CancellationToken::new(),
        )
        .await?;
    let results = workspace
        .retrieval()
        .search(
            SearchRequest {
                query: "事务隔离性".into(),
                ..Default::default()
            },
            CancellationToken::new(),
        )
        .await?;
    println!(
        "created {}\n{}",
        concept.metadata.id,
        serde_json::to_string_pretty(&results)?
    );
    workspace.close(CancellationToken::new()).await?;
    Ok(())
}
