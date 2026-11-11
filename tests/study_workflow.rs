use lumen::{agent::*, knowledge::*, study::*, workspace::*};
use serde_json::json;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn c() -> CancellationToken {
    CancellationToken::new()
}
async fn fixture() -> (tempfile::TempDir, WorkspaceHandle, StudyService) {
    let dir = tempfile::tempdir().unwrap();
    let workspace = WorkspaceService::init(
        InitRequest {
            root: dir.path().join("knowledge"),
            author_name: "Learner".into(),
            author_email: "learner@example.test".into(),
        },
        c(),
    )
    .await
    .unwrap();
    (dir, workspace.clone(), StudyService(workspace))
}
fn unit(title: &str) -> PlanUnit {
    PlanUnit {
        id: None,
        expected_revision: None,
        title: title.into(),
        body: format!("目标：理解{title}。练习：解释并举例。验收：独立解释边界条件。"),
        minutes: 30,
    }
}
fn model(text: String) -> Arc<dyn ModelProvider> {
    Arc::new(MockProvider::new(vec![AssistantMessage {
        content: vec![ContentPart::Text(text)],
        ..Default::default()
    }]))
}

#[tokio::test]
async fn goal_route_evidence_reorder_remove_and_restart_form_a_complete_workflow() {
    let (dir, w, study) = fixture().await;
    let goal = study
        .create("学习事务".into(), "能解释事务隔离级别".into(), c())
        .await
        .unwrap();
    let goal = study
        .set(
            goal.id.clone(),
            "baseline",
            "了解 SQL".into(),
            goal.revision,
            c(),
        )
        .await
        .unwrap();
    let goal = study
        .set(
            goal.id.clone(),
            "weekly_minutes",
            "240".into(),
            goal.revision,
            c(),
        )
        .await
        .unwrap();
    assert_eq!(goal.settings.baseline, "了解 SQL");
    assert_eq!(goal.settings.weekly_minutes, 240);
    let progress = study
        .apply_plan(
            goal.id.clone(),
            PlanDraft {
                units: vec![unit("原子性"), unit("隔离性"), unit("并发异常")],
            },
            goal.revision,
            c(),
        )
        .await
        .unwrap();
    assert_eq!(progress.completed, 0);
    assert_eq!(progress.total_minutes, 90);
    let first = progress.units[0].id.clone();
    let second = progress.units[1].id.clone();
    let third = progress.units[2].id.clone();
    let record = study
        .mark(first.clone(), true, "完成练习".into(), c())
        .await
        .unwrap();
    let progress = study.progress(goal.id.clone(), c()).await.unwrap();
    assert_eq!(progress.completed, 1);
    assert_eq!(progress.next, Some(second.clone()));
    assert!(progress.units[0].evidence_ids.contains(&record.metadata.id));
    study
        .review(goal.id.clone(), "隔离性仍需复习".into(), c())
        .await
        .unwrap();
    let (revision, mut draft) = study.draft(goal.id.clone(), c()).await.unwrap();
    let last = draft.units.pop().unwrap();
    draft.units.insert(0, last);
    let progress = study
        .apply_plan(goal.id.clone(), draft, revision, c())
        .await
        .unwrap();
    assert_eq!(progress.next, Some(third.clone()));
    assert_eq!(progress.completed, 1);
    let (revision, mut draft) = study.draft(goal.id.clone(), c()).await.unwrap();
    draft.units.retain(|u| u.id.as_ref() != Some(&first));
    study
        .apply_plan(goal.id.clone(), draft, revision, c())
        .await
        .unwrap();
    // Removing a route entry must not erase notes or evidence.
    assert!(w
        .knowledge()
        .read(first.clone(), ReadView::Full, c())
        .await
        .is_ok());
    assert_eq!(
        w.learning().state(vec![first.clone()], c()).await.unwrap()[0]
            .outcome
            .as_deref(),
        Some("completed")
    );
    let active = w
        .knowledge()
        .read(second.clone(), ReadView::Full, c())
        .await
        .unwrap();
    assert!(w
        .knowledge()
        .delete(
            DeleteObject {
                id: second.clone(),
                expected_revision: active.revision
            },
            c()
        )
        .await
        .is_err());
    assert!(
        w.validate(ValidationScope::WorkingTree, c())
            .await
            .unwrap()
            .valid
    );
    w.close(c()).await.unwrap();
    let w = WorkspaceService::open(dir.path().join("knowledge"), c())
        .await
        .unwrap();
    let restored = StudyService(w.clone())
        .progress(goal.id, c())
        .await
        .unwrap();
    assert_eq!(
        restored
            .units
            .iter()
            .map(|u| u.id.clone())
            .collect::<Vec<_>>(),
        vec![third, second]
    );
    assert_eq!(restored.goal.settings.weekly_minutes, 240);
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn invalid_routes_stale_revisions_and_cancellation_never_partially_write() {
    let (_dir, w, study) = fixture().await;
    let goal = study
        .create("目标".into(), "说明".into(), c())
        .await
        .unwrap();
    let initial = goal.revision.clone();
    let mut bad = unit("错误单元");
    bad.minutes = 0;
    assert!(study
        .apply_plan(
            goal.id.clone(),
            PlanDraft {
                units: vec![unit("有效单元"), bad]
            },
            initial.clone(),
            c()
        )
        .await
        .is_err());
    assert_eq!(
        study.get(goal.id.clone(), c()).await.unwrap().revision,
        initial
    );
    assert_eq!(
        w.knowledge()
            .list(
                ObjectFilter {
                    kinds: vec![ObjectKind::Unit],
                    ..Default::default()
                },
                PageRequest::default(),
                c()
            )
            .await
            .unwrap()
            .items
            .len(),
        0
    );
    let changed = study
        .set(
            goal.id.clone(),
            "description",
            "修改后的说明".into(),
            initial.clone(),
            c(),
        )
        .await
        .unwrap();
    assert!(matches!(
        study
            .apply_plan(
                goal.id.clone(),
                PlanDraft {
                    units: vec![unit("旧路线")]
                },
                initial,
                c()
            )
            .await,
        Err(WorkspaceError::RevisionConflict(_))
    ));
    let cancel = c();
    cancel.cancel();
    assert!(study
        .apply_plan(
            goal.id.clone(),
            PlanDraft {
                units: vec![unit("取消的路线")]
            },
            changed.revision.clone(),
            cancel
        )
        .await
        .is_err());
    assert_eq!(
        study.get(goal.id.clone(), c()).await.unwrap().revision,
        changed.revision
    );
    assert!(study
        .set(goal.id, "weekly_minutes", "0".into(), changed.revision, c())
        .await
        .is_err());
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn unit_edits_after_route_read_are_not_overwritten_by_stale_drafts() {
    let (_dir, w, study) = fixture().await;
    let goal = study
        .create("单元编辑".into(), "说明".into(), c())
        .await
        .unwrap();
    let plan = study
        .apply_plan(
            goal.id.clone(),
            PlanDraft {
                units: vec![unit("初始内容")],
            },
            goal.revision,
            c(),
        )
        .await
        .unwrap();
    let (revision, draft) = study.draft(goal.id.clone(), c()).await.unwrap();
    let current = w
        .knowledge()
        .read(plan.units[0].id.clone(), ReadView::Full, c())
        .await
        .unwrap();
    w.knowledge()
        .patch(
            PatchObject {
                id: current.metadata.id,
                metadata_patch: Default::default(),
                body: Some("用户在生成期间修改的内容".into()),
                expected_revision: current.revision,
            },
            c(),
        )
        .await
        .unwrap();
    assert!(matches!(
        study.apply_plan(goal.id, draft, revision, c()).await,
        Err(WorkspaceError::RevisionConflict(_))
    ));
    assert_eq!(
        w.knowledge()
            .read(plan.units[0].id.clone(), ReadView::Full, c())
            .await
            .unwrap()
            .content,
        "用户在生成期间修改的内容"
    );
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn generated_plan_requires_valid_output_and_retains_completed_units() {
    let (_dir, w, study) = fixture().await;
    let goal = study
        .create("Rust".into(), "理解所有权".into(), c())
        .await
        .unwrap();
    let json =
        json!({"units":[{"title":"所有权","body":"解释移动与借用，并完成练习","minutes":40}]})
            .to_string();
    let plan = study
        .generate(goal.id.clone(), model(json), 4096, c())
        .await
        .unwrap();
    assert_eq!(plan.units.len(), 1);
    let unit_id = plan.units[0].id.clone();
    study
        .mark(unit_id.clone(), true, "已完成".into(), c())
        .await
        .unwrap();
    let revision = plan.goal.revision;
    for output in [
        "not JSON".to_string(),
        json!({"units":[]}).to_string(),
        json!({"units":[{"title":"覆盖已完成单元","body":"不保留原 ID","minutes":30}]}).to_string(),
    ] {
        assert!(study
            .generate(goal.id.clone(), model(output), 4096, c())
            .await
            .is_err());
        assert_eq!(
            study.get(goal.id.clone(), c()).await.unwrap().revision,
            revision
        );
    }
    let json=json!({"units":[{"id":unit_id,"title":"所有权","body":"保留学习成果","minutes":40},{"title":"生命周期","body":"解释生命周期并完成练习","minutes":50}]}).to_string();
    let updated = study
        .generate(goal.id, model(json), 4096, c())
        .await
        .unwrap();
    assert_eq!(updated.completed, 1);
    assert_eq!(updated.units.len(), 2);
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn assessment_persists_answer_feedback_and_evidence_without_inventing_progress() {
    let (_dir, w, study) = fixture().await;
    let goal = study
        .create("隔离性".into(), "解释隔离级别".into(), c())
        .await
        .unwrap();
    let plan = study
        .apply_plan(
            goal.id.clone(),
            PlanDraft {
                units: vec![unit("隔离级别")],
            },
            goal.revision,
            c(),
        )
        .await
        .unwrap();
    let id = plan.units[0].id.clone();
    assert!(study
        .assess(
            id.clone(),
            "答案".into(),
            model("{\"score\":101,\"feedback\":\"bad\"}".into()),
            4096,
            c()
        )
        .await
        .is_err());
    assert_eq!(
        study
            .progress(goal.id.clone(), c())
            .await
            .unwrap()
            .completed,
        0
    );
    let event = study
        .assess(
            id.clone(),
            "读已提交禁止脏读".into(),
            model(json!({"score":65,"feedback":"请进一步解释不可重复读与幻读"}).to_string()),
            4096,
            c(),
        )
        .await
        .unwrap();
    assert!(event.body.contains("读已提交禁止脏读"));
    assert_eq!(event.metadata.data["outcome"], "needs_practice");
    assert_eq!(
        study
            .progress(goal.id.clone(), c())
            .await
            .unwrap()
            .completed,
        0
    );
    study
        .assess(
            id.clone(),
            "完整解释与例子".into(),
            model(json!({"score":90,"feedback":"解释正确，补充边界练习"}).to_string()),
            4096,
            c(),
        )
        .await
        .unwrap();
    assert_eq!(
        study
            .progress(goal.id.clone(), c())
            .await
            .unwrap()
            .completed,
        1
    );
    study
        .mark(id, false, "主动安排复习".into(), c())
        .await
        .unwrap();
    assert_eq!(study.progress(goal.id, c()).await.unwrap().completed, 0);
    w.close(c()).await.unwrap();
}

struct EditingProvider {
    study: StudyService,
    id: ObjectId,
}
#[async_trait::async_trait]
impl ModelProvider for EditingProvider {
    async fn stream(
        &self,
        _: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError> {
        let goal = self
            .study
            .get(self.id.clone(), cancel.clone())
            .await
            .unwrap();
        self.study
            .set(
                goal.id,
                "baseline",
                "模型运行期间用户更新的基础".into(),
                goal.revision,
                cancel.clone(),
            )
            .await
            .unwrap();
        MockProvider::new(vec![AssistantMessage {
            content: vec![ContentPart::Text(
                json!({"units":[{"title":"过期路线","body":"不能覆盖新目标","minutes":20}]})
                    .to_string(),
            )],
            ..Default::default()
        }])
        .stream(ModelRequest::default(), cancel)
        .await
    }
}
#[tokio::test]
async fn goal_changes_during_model_generation_are_preserved() {
    let (_dir, w, study) = fixture().await;
    let goal = study
        .create("并发编辑".into(), "说明".into(), c())
        .await
        .unwrap();
    let provider = Arc::new(EditingProvider {
        study: study.clone(),
        id: goal.id.clone(),
    });
    assert!(matches!(
        study.generate(goal.id.clone(), provider, 4096, c()).await,
        Err(WorkspaceError::RevisionConflict(_))
    ));
    let restored = study.get(goal.id, c()).await.unwrap();
    assert_eq!(restored.settings.baseline, "模型运行期间用户更新的基础");
    assert!(restored.settings.units.is_empty());
    w.close(c()).await.unwrap();
}

struct UnfinishedModel {
    mode: u8,
    body: String,
}
#[async_trait::async_trait]
impl ModelProvider for UnfinishedModel {
    async fn stream(
        &self,
        _: ModelRequest,
        _: CancellationToken,
    ) -> Result<ModelEventStream, ProviderError> {
        let mut events = vec![Ok(ModelEvent::TextDelta(self.body.clone()))];
        if self.mode > 0 {
            let mut response = ModelResponse::from(AssistantMessage {
                content: vec![ContentPart::Text(self.body.clone())],
                ..Default::default()
            });
            response.finish_reason = match self.mode {
                1 => ModelFinishReason::OutputLimit,
                2 => ModelFinishReason::Refusal,
                _ => ModelFinishReason::ToolCalls,
            };
            events.push(Ok(ModelEvent::completed(response)));
        }
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}
#[tokio::test]
async fn partial_output_limit_and_refusal_never_commit_generated_routes() {
    let (_dir, w, study) = fixture().await;
    let goal = study
        .create("可靠生成".into(), "说明".into(), c())
        .await
        .unwrap();
    let body = json!({"units":[{"title":"不能提交","body":"部分结果","minutes":30}]}).to_string();
    for mode in 0..4 {
        assert!(study
            .generate(
                goal.id.clone(),
                Arc::new(UnfinishedModel {
                    mode,
                    body: body.clone()
                }),
                4096,
                c()
            )
            .await
            .is_err());
        assert_eq!(
            study.get(goal.id.clone(), c()).await.unwrap().revision,
            goal.revision
        );
    }
    w.close(c()).await.unwrap();
}

#[tokio::test]
async fn agent_can_apply_a_route_using_registered_tools_and_cas_revision() {
    let (_dir, w, study) = fixture().await;
    let goal = study
        .create("工具路线".into(), "说明".into(), c())
        .await
        .unwrap();
    let provider = MockProvider::new(vec![
        AssistantMessage {
            tool_calls: vec![ToolCall {
                id: "plan".into(),
                name: "plan_apply".into(),
                arguments: json!({"id":goal.id,"expected_revision":goal.revision,"units":[{"title":"工具创建的单元","body":"独立完成解释与练习","minutes":25}]}),
            }],
            ..Default::default()
        },
        AssistantMessage {
            content: vec![ContentPart::Text("路线已保存".into())],
            ..Default::default()
        },
    ]);
    let mut agent = AgentLoop::new(Arc::new(provider));
    register_study_tools(&mut agent, w.clone());
    let mut state = AgentState {
        messages: vec![],
        turn_count: 0,
    };
    let result = agent
        .run(vec![ContentPart::Text("创建路线".into())], &mut state, c())
        .await
        .unwrap();
    assert_eq!(result.reason, StopReason::Completed);
    assert!(state
        .messages
        .iter()
        .any(|m| matches!(m,AgentMessage::ToolResult(r) if !r.is_error)));
    assert_eq!(
        study.progress(goal.id, c()).await.unwrap().units[0].title,
        "工具创建的单元"
    );
    w.close(c()).await.unwrap();
}
