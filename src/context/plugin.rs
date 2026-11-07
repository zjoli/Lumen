use super::*;
use crate::agent::*;
use async_trait::async_trait;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ContextConfig {
    pub context_tokens: usize,
    pub output_tokens: usize,
    pub tool_reserve: usize,
    pub safety_tokens: usize,
    pub soft_percent: usize,
    pub target_percent: usize,
    pub summary_tokens: usize,
    pub tool_result_bytes: usize,
}
impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            context_tokens: 128_000,
            output_tokens: 4096,
            tool_reserve: 8192,
            safety_tokens: 4096,
            soft_percent: 80,
            target_percent: 55,
            summary_tokens: 8000,
            tool_result_bytes: 8192,
        }
    }
}
impl ContextConfig {
    pub fn for_capacity(context_tokens: usize) -> Self {
        let mut config = Self::default();
        config.context_tokens = context_tokens;
        config.output_tokens = config.output_tokens.min((context_tokens / 8).max(1));
        config.tool_reserve = config.tool_reserve.min(context_tokens / 16);
        config.safety_tokens = config.safety_tokens.min(context_tokens / 32);
        config.summary_tokens = config.summary_tokens.min((context_tokens / 8).max(256));
        config
    }
    pub fn input_budget(&self, provider: &dyn ModelProvider) -> Result<usize, AgentError> {
        if self.soft_percent == 0
            || self.soft_percent > 100
            || self.target_percent == 0
            || self.target_percent >= self.soft_percent
            || self.summary_tokens < 256
            || self.output_tokens == 0
            || self.tool_result_bytes < 256
        {
            return Err(AgentError::Context(
                "invalid context window settings".into(),
            ));
        }
        self.context_tokens
            .min(provider.capabilities().context_tokens)
            .checked_sub(
                self.output_tokens
                    .min(provider.capabilities().max_output_tokens)
                    .saturating_add(self.tool_reserve)
                    .saturating_add(self.safety_tokens),
            )
            .filter(|b| *b > 256)
            .ok_or_else(|| {
                AgentError::Context("context capacity cannot cover reserved space".into())
            })
    }
}

pub struct ContextWindowPlugin {
    runtime: Arc<SessionRuntime>,
    provider: Arc<dyn ModelProvider>,
    compactor: Arc<dyn CompactionService>,
    config: ContextConfig,
}
impl ContextWindowPlugin {
    pub fn new(
        runtime: Arc<SessionRuntime>,
        provider: Arc<dyn ModelProvider>,
        config: ContextConfig,
    ) -> Self {
        Self {
            runtime,
            compactor: Arc::new(ModelCompactionService::new(provider.clone())),
            provider,
            config,
        }
    }
    pub fn with_compactor(mut self, compactor: Arc<dyn CompactionService>) -> Self {
        self.compactor = compactor;
        self
    }
    fn prepare(
        &self,
        messages: Vec<AgentMessage>,
        tools: &[ToolDeclaration],
    ) -> Result<PreparedRequest, AgentError> {
        self.provider.prepare(ModelRequest {
            messages,
            tools: tools.to_vec(),
            max_output_tokens: self
                .config
                .output_tokens
                .min(self.provider.capabilities().max_output_tokens),
        })
    }
    async fn assemble(
        &self,
        assembly: &mut ContextAssembly,
        ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        let budget = self.config.input_budget(self.provider.as_ref())?;
        let snapshot = self.runtime.snapshot().await;
        let old = &snapshot.window;
        let config_key = blake3::hash(
            &serde_json::to_vec(&(
                &self.provider.configuration_key(),
                &assembly.tools,
                self.config.output_tokens,
            ))
            .unwrap(),
        )
        .to_hex()
        .to_string();
        let mut next = old.clone();
        for region in next.regions.values_mut() {
            if *region == ContextRegion::Current {
                *region = ContextRegion::Recent;
            }
        }
        let mut reset = old.config != config_key;
        if reset {
            next.projection = snapshot
                .messages
                .iter()
                .filter(|m| !matches!(m, AgentMessage::Custom { .. }))
                .cloned()
                .collect();
            // Restore required loaded skills/state even across provider changes.
            next.projection.extend(next.pins.values().cloned());
        } else {
            next.projection.extend(
                snapshot.messages[old.through..]
                    .iter()
                    .filter(|m| !matches!(m, AgentMessage::Custom { .. }))
                    .cloned(),
            );
        }
        next.through = snapshot.messages.len();
        next.config = config_key;
        for message in &snapshot.messages[if reset { 0 } else { old.through }..] {
            next.regions.entry(message_key(message)).or_insert(
                if matches!(message, AgentMessage::System(_)) {
                    ContextRegion::Core
                } else {
                    ContextRegion::Current
                },
            );
        }
        let mut committed_summary = None;
        let mut required_items = vec![];
        let mut optional_items = vec![];
        let mut items = snapshot.pending[old.control_through..].to_vec();
        assembly
            .collector
            .items
            .sort_by(|a, b| (&a.id, &a.revision).cmp(&(&b.id, &b.revision)));
        items.append(&mut assembly.collector.items);
        next.keys.append(&mut assembly.collector.marks);
        for item in items {
            let key = item.key();
            let message = item.message();
            next.regions.insert(message_key(&message), item.region);
            if item.retire {
                if !next.pins.contains_key(&item.id) {
                    continue;
                }
                next.pins.remove(&item.id);
                required_items.push((key, message));
            } else if item.required {
                if next.pins.get(&item.id) == Some(&message) {
                    continue;
                }
                next.pins.insert(item.id.clone(), message.clone());
                required_items.push((key, message));
            } else {
                optional_items.push((key, message));
            }
        }
        for (key, message) in &required_items {
            next.keys.insert(key.clone());
            next.projection.push(message.clone());
        }
        next.control_through = snapshot.pending.len();
        // Validate before inserting references after pending tool calls.
        validate_tool_groups(&next.projection)?;
        let mut prepared = self.prepare(next.projection.clone(), &assembly.tools)?;
        let force = self.runtime.take_compaction();
        if force
            || prepared.estimated_input_tokens()
                >= budget.saturating_mul(self.config.soft_percent) / 100
        {
            ctx.events.push(AgentEvent::ContextCompactionStart);
            let original = next.projection.clone();
            match self
                .compact(
                    &next,
                    &snapshot.messages,
                    &assembly.tools,
                    budget,
                    ctx.cancel.clone(),
                )
                .await
            {
                Ok((projection, summary, usage)) => {
                    for usage in usage {
                        ctx.events.push(AgentEvent::CompactionUsageReported(usage));
                    }
                    next.projection = projection;
                    for message in &next.projection {
                        if is_original_user(message, &snapshot.messages)
                            && !snapshot.messages[old.through..].contains(message)
                        {
                            next.regions
                                .insert(message_key(message), ContextRegion::Task);
                        }
                    }
                    let summary_message = AgentMessage::User(vec![ContentPart::Text(format!("Historical summary (derived, preserve original source authority):\n{summary}"))]);
                    next.regions
                        .insert(message_key(&summary_message), ContextRegion::Summary);
                    reset = true;
                    prepared = self.prepare(next.projection.clone(), &assembly.tools)?;
                    // End event is emitted only after the durable commit below.
                    committed_summary = Some(summary);
                }
                Err(AgentError::Aborted) => {
                    if force {
                        self.runtime.request_compaction();
                    }
                    return Err(AgentError::Aborted);
                }
                Err(error) => {
                    next.projection = original;
                    ctx.events.push(AgentEvent::ContextDegraded {
                        reason: format!("compaction failed: {error}"),
                    });
                }
            }
        }
        if prepared.estimated_input_tokens() > budget {
            // Never commit a partially loaded required skill or a dropped user request.
            return Err(AgentError::Context(format!("input requires approximately {} tokens, budget {budget}; reduce source/input ranges", prepared.estimated_input_tokens())));
        }
        // Optional retrieval uses only remaining capacity. Already committed context stays stable.
        for (key, message) in optional_items {
            // Deduplicate against the current window, not everything ever seen. A source
            // removed by compaction/config changes can be needed again for a new question.
            if next.projection.contains(&message) {
                continue;
            }
            let mut candidate = next.projection.clone();
            candidate.push(message);
            let encoded = self.prepare(candidate.clone(), &assembly.tools)?;
            if encoded.estimated_input_tokens() <= budget {
                next.projection = candidate;
                next.keys.insert(key);
                prepared = encoded;
            } else {
                ctx.events.push(AgentEvent::ContextDegraded {
                    reason: "optional reference omitted: input budget".into(),
                });
            }
        }
        if ctx.cancel.is_cancelled() {
            return Err(AgentError::Aborted);
        }
        next.epoch = old.epoch + u64::from(reset);
        next.manifest = prepared.fingerprint();
        next.report.clear();
        for message in &next.projection {
            let region = next
                .regions
                .get(&message_key(message))
                .copied()
                .unwrap_or(ContextRegion::Recent);
            let name = match region {
                ContextRegion::Core => "A_core",
                ContextRegion::Workspace => "B_workspace",
                ContextRegion::Task => "C_task",
                ContextRegion::Summary => "D_summary",
                ContextRegion::Skill => "E_skills",
                ContextRegion::Recent => "F_recent",
                ContextRegion::Current => "G_current",
            };
            *next.report.entry(name.into()).or_default() +=
                serde_json::to_vec(message).unwrap().len();
        }
        *next.report.entry("A_core".into()).or_default() +=
            serde_json::to_vec(&assembly.tools).unwrap().len();
        next.report
            .insert("input_estimate".into(), prepared.estimated_input_tokens());
        next.report.insert("input_budget".into(), budget);
        next.report
            .insert("H_output".into(), prepared.request().max_output_tokens);
        next.report
            .insert("H_tool_growth".into(), self.config.tool_reserve);
        next.report
            .insert("H_safety".into(), self.config.safety_tokens);
        let visible: std::collections::BTreeSet<_> =
            next.projection.iter().map(message_key).collect();
        next.regions.retain(|key, _| visible.contains(key));
        self.runtime.commit(old, next.clone(), reset).await?;
        if let Some(summary) = committed_summary {
            ctx.events
                .push(AgentEvent::ContextCompactionEnd { summary });
        }
        ctx.events.push(AgentEvent::ContextPrepared {
            epoch: next.epoch,
            input_tokens: prepared.estimated_input_tokens(),
            input_budget: budget,
            manifest: next.manifest,
        });
        assembly.prepared = Some(prepared);
        Ok(())
    }
    async fn compact(
        &self,
        state: &WindowState,
        raw: &[AgentMessage],
        tools: &[ToolDeclaration],
        budget: usize,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(Vec<AgentMessage>, String, Vec<TokenUsage>), AgentError> {
        let messages = &state.projection;
        let user_boundaries: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter_map(|(i, m)| is_original_user(m, raw).then_some(i))
            .collect();
        let split = user_boundaries.iter().rev().nth(1).copied().unwrap_or(0);
        if split == 0 {
            return Err(AgentError::Context(
                "no completed history available to compact".into(),
            ));
        }
        validate_tool_groups(&messages[..split])?;
        validate_tool_groups(&messages[split..])?;
        let source = messages[..split].to_vec();
        let mut retained: Vec<_> = messages[..split]
            .iter()
            .filter(|message| {
                matches!(message, AgentMessage::System(_))
                    || is_original_user(message, raw)
                    || state.pins.values().any(|pin| pin == *message)
            })
            .cloned()
            .collect();
        retained.extend_from_slice(&messages[split..]);
        let retained_size = self.prepare(retained, tools)?.estimated_input_tokens();
        let target = budget.saturating_mul(self.config.target_percent) / 100;
        let summary_budget = self
            .config
            .summary_tokens
            .min(target.saturating_sub(retained_size).max(256));
        let mut summary = None;
        for attempt in 0..2 {
            let limit = (summary_budget / (attempt + 1))
                .max(256)
                .min(self.config.summary_tokens);
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                self.compactor.summarize(
                    ModelRequest {
                        messages: source.clone(),
                        tools: tools.to_vec(),
                        max_output_tokens: limit,
                    },
                    limit,
                    cancel.clone(),
                ),
            )
            .await;
            match result {
                Ok(Ok(output))
                    if !output.text.trim().is_empty() && estimate_tokens(&output.text) <= limit =>
                {
                    summary = Some(output);
                    break;
                }
                Ok(Err(AgentError::Aborted)) => return Err(AgentError::Aborted),
                _ => (),
            }
        }
        let output = summary
            .ok_or_else(|| AgentError::Context("summary failed after two attempts".into()))?;
        let summary = output.text;
        let mut projection = vec![];
        // Conservative first version: preserve original user requests verbatim, in order.
        // This avoids guessing which prose sentences are hard constraints.
        for message in &messages[..split] {
            if matches!(message, AgentMessage::System(_))
                || is_original_user(message, raw)
                || state.pins.values().any(|p| p == message)
            {
                projection.push(message.clone());
            }
        }
        projection.push(AgentMessage::User(vec![ContentPart::Text(format!(
            "Historical summary (derived, preserve original source authority):\n{summary}"
        ))]));
        projection.extend_from_slice(&messages[split..]);
        validate_tool_groups(&projection)?;
        let before = self
            .prepare(messages.to_vec(), tools)?
            .estimated_input_tokens();
        let after = self
            .prepare(projection.clone(), tools)?
            .estimated_input_tokens();
        if after > budget || after.saturating_add((budget / 20).min(1000)) >= before {
            return Err(AgentError::Context(
                "summary did not release enough space".into(),
            ));
        }
        Ok((projection, summary, output.usage))
    }
}
fn is_original_user(message: &AgentMessage, raw: &[AgentMessage]) -> bool {
    matches!(message, AgentMessage::User(_)) && raw.iter().any(|m| m == message)
}
fn message_key(message: &AgentMessage) -> String {
    blake3::hash(&serde_json::to_vec(message).unwrap())
        .to_hex()
        .to_string()
}
#[async_trait]
impl AgentPlugin for ContextWindowPlugin {
    fn role(&self) -> PluginRole {
        PluginRole::ContextCoordinator
    }
    fn context_session(&self) -> Option<Arc<SessionRuntime>> {
        Some(self.runtime.clone())
    }
    async fn before_request(
        &self,
        event: &mut BeforeRequest<'_>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        if let BeforeRequest::Assemble(assembly) = event {
            self.assemble(assembly, ctx).await
        } else {
            Err(AgentError::Context(
                "coordinator requires assembly phase".into(),
            ))
        }
    }
    async fn after_tool_call(
        &self,
        event: &mut AfterToolCall<'_>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<(), AgentError> {
        if !event.original.is_error
            && matches!(
                event.call.name.as_str(),
                "skill_load" | "skill_finish" | "skill_progress"
            )
        {
            let text: String = event
                .original
                .content
                .iter()
                .map(|ContentPart::Text(s)| s.as_str())
                .collect();
            let mut envelope: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| AgentError::Context(format!("invalid skill result envelope: {e}")))?;
            if let Some(updates) = envelope
                .as_object_mut()
                .and_then(|object| object.remove("lumen_context_updates"))
            {
                let updates: Vec<ContextContribution> = serde_json::from_value(updates)
                    .map_err(|e| AgentError::Context(e.to_string()))?;
                // Durable lifecycle changes run outside the cancellable tool future.
                self.runtime.offer_batch(updates).await?;
                event.result.content = vec![ContentPart::Text(envelope.to_string())];
            }
        }
        let calls = ctx
            .state
            .messages
            .iter()
            .rev()
            .find_map(|m| {
                if let AgentMessage::Assistant(a) = m {
                    Some(a.tool_calls.len())
                } else {
                    None
                }
            })
            .unwrap_or(1)
            .max(1);
        let limit = self
            .config
            .tool_result_bytes
            .min(self.config.tool_reserve / calls)
            .max(256);
        let text: String = event
            .result
            .content
            .iter()
            .map(|ContentPart::Text(s)| s.as_str())
            .collect();
        if text.len() > limit || event.original != event.result {
            let artifact = self.runtime.save_artifact(event.original).await?;
            if text.len() > limit {
                let mut end = limit.saturating_sub(200).min(text.len());
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                event.result.content = vec![ContentPart::Text(format!("{}\n[Output abbreviated; full original artifact={artifact}. Use context_artifact_read with offset/limit.]", &text[..end]))];
            }
        }
        Ok(())
    }
}
