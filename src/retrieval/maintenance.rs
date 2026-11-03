use super::*;
use crate::workspace::{Workspace, WorkspaceEvent, WorkspaceEventKind};
use std::sync::Weak;
use tokio::sync::{broadcast, Notify};
pub(crate) fn start_maintenance(
    workspace: Weak<Workspace>,
    mut events: broadcast::Receiver<WorkspaceEvent>,
    notify: Arc<Notify>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                            _=cancel.cancelled()=>break,
                            _=notify.notified()=>(),
                            event=events.recv()=>match event{Ok(event)if matches!(event.kind,WorkspaceEventKind::IndexReady)=>(),Err(broadcast::error::RecvError::Lagged(_))=>(),Err(broadcast::error::RecvError::Closed)=>break,_=>continue}
            ,
                        }
            let Some(w) = workspace.upgrade() else { break };
            let embedder = w
                .providers
                .embedder
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            let summary = w
                .providers
                .summary
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if embedder.is_none() && summary.is_none() {
                continue;
            }
            let work = async {
                if let Some(embedder) = embedder {
                    w.index()
                        .embed_missing(embedder, cancel.child_token())
                        .await?;
                }
                if summary.is_some() {
                    let mut cursor = None;
                    loop {
                        let objects = w
                            .knowledge()
                            .list(
                                ObjectFilter::default(),
                                PageRequest { limit: 100, cursor },
                                cancel.child_token(),
                            )
                            .await?;
                        for object in objects.items {
                            for level in 0..=1 {
                                if level == 0 && object.summary.is_some()
                                    || level == 1
                                        && !matches!(
                                            object.kind,
                                            ObjectKind::Domain
                                                | ObjectKind::Topic
                                                | ObjectKind::Unit
                                        )
                                {
                                    continue;
                                }
                                let request = SummaryRequest {
                                    id: object.id.clone(),
                                    level,
                                    max_chars: None,
                                };
                                if !w
                                    .summaries()
                                    .read(request.clone(), cancel.child_token())
                                    .await?
                                    .generated
                                {
                                    w.summaries()
                                        .generate(request, cancel.child_token())
                                        .await?;
                                }
                            }
                        }
                        cursor = objects.next_cursor;
                        if cursor.is_none() {
                            break;
                        }
                    }
                }
                Ok::<_, E>(())
            };
            let result = tokio::select! {_ =cancel.cancelled()=>break,result=work=>result};
            if let Err(error) = result {
                if !matches!(error, E::Cancelled | E::NeedsResolution | E::WorkspaceBusy) {
                    let _ = w
                        .call(cancel.child_token(), move |s, _| {
                            s.emit(WorkspaceEventKind::IndexFailed, None);
                            Ok(())
                        })
                        .await;
                }
            }
        }
    })
}
