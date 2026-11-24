//! `TuiController`'s behaviour: provider discovery, provider starts and
//! resumes, the event pump, sends and steers, launch settings and shutdown.
//! The struct and its state stay in `tui.rs`, which the agent socket and the
//! keyboard share.
use super::{
    AgentController, AgentDelivery, AgentDeliveryKind, AgentErrorCode, AgentProtocolError,
    AgentSessionState, AgentStatus, Arc, AtomicU64, BTreeMap, CHANNELS, CellTone,
    CliCollaborationRuntime, CollaborationDraftStateUpdate, CollaborationEventDto,
    CollaborationMode, CollaborationRuntimeEventPush, CollaborationState, CollaborationStatusDto,
    CommandCatalogue, ControlledSession, ControllerState, DiagnosticsSink, EngineEvent,
    EngineEventKind, EngineId, HISTORY_MAX_LIMIT, LaunchDefaults, LaunchOptions, ModelOption,
    Mutex, NewSession, NoticeTone, OrchestrationHostControl, Ordering, PreparedOrchestrationHost,
    ProjectedSendOutcome, PromptHistory, ProviderCommandScope, ProviderProbe, ProviderReadiness,
    PushFrame, QUEUED_FAILURE_LIMIT, QueuedSendFailure, ResumeOptions, RuntimeEventFrame, RwLock,
    SessionRuntime, SessionStatus, SessionSummary, StartFailure, SteerHistoryStore,
    SteerSettledOutcome, TerminalNotification, Transcript, TranscriptCell, TuiController, TuiView,
    VecDeque, activity_order, capabilities_for, capabilities_for_text, catalogue_from,
    context_telemetry_sample, controlled_from_live_summary, controlled_from_summary,
    definitely_unsent, delivery_from_runtime, for_display, merge_registry_summary, mpsc, protocol,
    push_accepted_steer, push_message, push_notice, queued_send_failure, read_lock, ready_session,
    reduce_event, replace_provider_history, require_confirmed, scope_from_setting_sources,
    started_session, watch, write_lock,
};

impl TuiController {
    /// Builds the single runtime/store owner shared by the TUI and agent socket.
    ///
    /// # Errors
    ///
    /// Returns an error if the persisted session registry cannot be read.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn new(
        runtime: SessionRuntime,
        collaboration: Option<CliCollaborationRuntime>,
        launch: LaunchDefaults,
        prompt_history: PromptHistory,
        steer_history: SteerHistoryStore,
        diagnostics: DiagnosticsSink,
        prepared_orchestration: PreparedOrchestrationHost,
    ) -> Result<Self, String> {
        let summaries = runtime
            .cached_sessions()
            .await
            .map_err(|error| error.message)?;
        let sessions = summaries
            .into_iter()
            .map(|summary| {
                let key = format!("{}#{}", summary.engine, summary.session_id);
                (key.clone(), controlled_from_summary(key, summary))
            })
            .collect();
        let orchestration =
            Some(OrchestrationHostControl::start(prepared_orchestration, &runtime).await?);
        let probes = runtime
            .registered_engines()
            .into_iter()
            .map(|engine| {
                (
                    engine.to_string(),
                    ProviderProbe {
                        engine,
                        info: None,
                        error: Some("Discovering provider…".into()),
                    },
                )
            })
            .collect();
        let (revision, _) = watch::channel(0);
        let controller = Self {
            runtime,
            collaboration,
            orchestration,
            diagnostics: Some(diagnostics),
            state: Arc::new(RwLock::new(ControllerState {
                sessions,
                active: None,
            })),
            launch: Arc::new(RwLock::new(launch)),
            probes: Arc::new(RwLock::new(probes)),
            launch_commands: Arc::new(RwLock::new(CommandCatalogue::default())),
            launch_command_revision: Arc::new(AtomicU64::new(0)),
            prompt_history,
            steer_history,
            notifications: Arc::new(RwLock::new(VecDeque::new())),
            queued_failures: Arc::new(RwLock::new(VecDeque::new())),
            draft_revision: Arc::new(Mutex::new(0)),
            revision,
        };
        if let Some(collaboration) = &controller.collaboration
            && let Some(pushes) = collaboration.take_pushes().await
        {
            let push_controller = controller.clone();
            tokio::spawn(async move {
                push_controller.consume_collaboration_pushes(pushes).await;
            });
        }
        Ok(controller)
    }

    /// Provider discovery is independent of first paint. Never replace a row
    /// that the UI/socket changed while its initial listing was in flight.
    pub(super) async fn discover_providers(&self) {
        let initial = read_lock(&self.state)
            .sessions
            .iter()
            .map(|(key, row)| (key.clone(), row.state.clone()))
            .collect();
        let probes = async {
            let probes = self
                .runtime
                .probe_all_providers()
                .await
                .into_iter()
                .map(|readiness| {
                    let probe = match readiness {
                        ProviderReadiness::Ready { engine, info } => ProviderProbe {
                            engine,
                            info: Some(*info),
                            error: None,
                        },
                        ProviderReadiness::Unavailable { engine, error } => ProviderProbe {
                            engine,
                            info: None,
                            error: Some(for_display(&error.message)),
                        },
                    };
                    (probe.engine.to_string(), probe)
                })
                .collect();
            *write_lock(&self.probes) = probes;
            self.changed();
        };
        let sessions = async {
            if let Ok(summaries) = self.runtime.list_sessions().await {
                self.merge_discovered_sessions(&initial, summaries);
            }
        };
        tokio::join!(probes, sessions, self.refresh_launch_commands());
    }

    pub(super) fn merge_discovered_sessions(
        &self,
        initial: &BTreeMap<String, AgentSessionState>,
        summaries: Vec<SessionSummary>,
    ) {
        let mut state = write_lock(&self.state);
        for summary in summaries {
            let key = format!("{}#{}", summary.engine, summary.session_id);
            if let Some(row) = state.sessions.get_mut(&key) {
                if !row.state.live && initial.get(&key) == Some(&row.state) {
                    merge_registry_summary(row, summary, false);
                }
            } else if !initial.contains_key(&key) {
                state
                    .sessions
                    .insert(key.clone(), controlled_from_summary(key, summary));
            }
        }
        drop(state);
        self.changed();
    }

    async fn consume_collaboration_pushes(&self, mut pushes: mpsc::Receiver<PushFrame>) {
        while let Some(frame) = pushes.recv().await {
            if frame.operation == CHANNELS.registry {
                if let Ok(summaries) = frame.payload.decode::<Vec<SessionSummary>>() {
                    self.merge_collaboration_registry(summaries).await;
                }
                continue;
            }
            if frame.operation == CHANNELS.event {
                if let Ok(payload) = frame.payload.decode::<CollaborationRuntimeEventPush>() {
                    // The runtime registers a collaboration target before its
                    // pump emits, so the registry alone knows an unseen key;
                    // provider discovery would only rescan every store.
                    if !read_lock(&self.state).sessions.contains_key(&payload.key)
                        && let Ok(summaries) = self.runtime.cached_sessions().await
                    {
                        self.merge_collaboration_registry(summaries).await;
                    }
                    self.apply_event(&payload.key, &payload.event);
                }
                continue;
            }
            if frame.operation == CHANNELS.collaboration_event
                && let Ok(event) = frame.payload.decode::<CollaborationEventDto>()
            {
                self.apply_collaboration_status(&event.status);
            }
        }
    }

    async fn merge_collaboration_registry(&self, summaries: Vec<SessionSummary>) {
        let mut projected = Vec::with_capacity(summaries.len());
        for summary in summaries {
            let key = format!("{}#{}", summary.engine, summary.session_id);
            let locked = if let Some(collaboration) = &self.collaboration {
                collaboration.is_locked_session(&key).await
            } else {
                false
            };
            projected.push((key, summary, locked));
        }
        let mut state = write_lock(&self.state);
        for (key, summary, locked) in projected {
            if let Some(row) = state.sessions.get_mut(&key) {
                merge_registry_summary(row, summary, locked);
            } else if locked {
                state
                    .sessions
                    .insert(key.clone(), controlled_from_live_summary(key, summary));
            }
        }
        drop(state);
        self.changed();
    }

    fn apply_collaboration_status(&self, status: &CollaborationStatusDto) {
        if status.receipt.mode == CollaborationMode::Handoff
            && status.receipt.state == CollaborationState::Completed
            && read_lock(&self.state).active.as_deref() == Some(&status.source_session_key)
            && let Some(target) = status.target_session_key.as_deref()
            && read_lock(&self.state).sessions.contains_key(target)
        {
            self.set_active(Some(target.to_owned()));
            return;
        }
        self.changed();
    }

    pub(super) fn active_key(&self) -> Option<String> {
        read_lock(&self.state).active.clone()
    }

    pub(super) fn set_active(&self, key: Option<String>) {
        // Copy only what the launch defaults take: cloning the whole row would
        // also share its transcript, which an event landing meanwhile would
        // then have to deep-copy.
        let selected = key.as_ref().and_then(|key| {
            let state = read_lock(&self.state);
            let session = state.sessions.get(key)?;
            Some(LaunchDefaults {
                engine: session.state.engine.clone(),
                cwd: session.state.cwd.clone(),
                model: session.state.model.clone(),
                effort: session.effort.clone(),
                access: session.access.clone(),
                scope: scope_from_setting_sources(session.setting_sources.as_deref()),
                fast_mode: session.fast_mode,
            })
        });
        write_lock(&self.state).active = key;
        if let Some(selected) = selected {
            *write_lock(&self.launch) = selected;
        }
        self.changed();
    }

    pub(super) fn launch(&self) -> LaunchDefaults {
        read_lock(&self.launch).clone()
    }

    /// The session `--continue` opens: the most recently active one in the
    /// launch directory, as 0.1.169 chose it. Activity compares as RFC 3339
    /// instants, falling back to text order when either does not parse or
    /// both name the same instant; a tie keeps the earlier listed session.
    pub(super) fn continue_target(&self) -> Option<String> {
        let cwd = read_lock(&self.launch).cwd.clone();
        let state = read_lock(&self.state);
        state
            .sessions
            .values()
            .filter(|row| row.state.cwd == cwd)
            .reduce(|newest, row| {
                if activity_order(&row.last_activity, &newest.last_activity).is_gt() {
                    row
                } else {
                    newest
                }
            })
            .map(|row| row.state.key.clone())
    }

    fn launch_commands(&self) -> CommandCatalogue {
        read_lock(&self.launch_commands).clone()
    }

    pub(super) fn probe(&self, engine: &str) -> Option<ProviderProbe> {
        read_lock(&self.probes).get(engine).cloned()
    }

    pub(super) fn probes(&self) -> Vec<ProviderProbe> {
        read_lock(&self.probes).values().cloned().collect()
    }

    pub(super) fn model_catalogue(&self, engine: &str) -> Vec<ModelOption> {
        self.probe(engine)
            .and_then(|probe| probe.info)
            .and_then(|info| info.model_catalog)
            .unwrap_or_default()
    }

    pub(super) fn active_models(&self) -> Vec<ModelOption> {
        let state = read_lock(&self.state);
        let Some(key) = &state.active else {
            return self.model_catalogue(&read_lock(&self.launch).engine);
        };
        let Some(session) = state.sessions.get(key) else {
            return Vec::new();
        };
        if session.models.is_empty() {
            self.model_catalogue(&session.state.engine)
        } else {
            session.models.clone()
        }
    }

    pub(super) fn effort_levels(&self) -> Vec<String> {
        let launch = self.launch();
        let models = self.active_models();
        if let Some(levels) = models
            .iter()
            .find(|model| Some(model.value.as_str()) == launch.model.as_deref())
            .map(|model| model.effort_levels.clone())
            .filter(|levels| !levels.is_empty())
        {
            return levels;
        }
        self.probe(&launch.engine)
            .and_then(|probe| probe.info)
            .map(|info| info.capabilities.effort_levels)
            .unwrap_or_default()
    }

    pub(super) fn permission_modes(&self) -> Vec<String> {
        let engine = self
            .active_key()
            .and_then(|key| read_lock(&self.state).sessions.get(&key).cloned())
            .map_or_else(|| self.launch().engine, |session| session.state.engine);
        self.probe(&engine)
            .and_then(|probe| probe.info)
            .map(|info| info.capabilities.permission_modes)
            .unwrap_or_default()
    }

    pub(super) fn supports_fast_mode(&self) -> bool {
        let launch = self.launch();
        let model = self.active_key().and_then(|key| {
            read_lock(&self.state)
                .sessions
                .get(&key)
                .and_then(|session| session.state.model.clone())
        });
        let model = model.or(launch.model);
        let models = self.active_models();
        model.map_or_else(
            || {
                models
                    .iter()
                    .find(|model| model.is_default == Some(true))
                    .is_some_and(|model| model.supports_fast_mode == Some(true))
            },
            |value| {
                models
                    .iter()
                    .find(|model| model.value == value)
                    .is_some_and(|model| model.supports_fast_mode == Some(true))
            },
        )
    }

    pub(super) fn command_catalogue(&self) -> CommandCatalogue {
        let state = read_lock(&self.state);
        state
            .active
            .as_ref()
            .and_then(|key| state.sessions.get(key))
            .map_or_else(
                || self.launch_commands(),
                |session| session.commands.clone(),
            )
    }

    pub(super) async fn refresh_launch_commands(&self) {
        let revision = self.launch_command_revision.fetch_add(1, Ordering::AcqRel) + 1;
        let launch = self.launch();
        let Ok(engine) = EngineId::new(launch.engine) else {
            return;
        };
        *write_lock(&self.launch_commands) = CommandCatalogue::default();
        let result = self
            .runtime
            .list_provider_commands(ProviderCommandScope {
                engine,
                cwd: launch.cwd,
                model: launch.model,
                setting_sources: launch
                    .scope
                    .setting_sources()
                    .map(|values| values.into_iter().map(str::to_owned).collect()),
            })
            .await;
        if self.launch_command_revision.load(Ordering::Acquire) == revision {
            *write_lock(&self.launch_commands) = catalogue_from(result);
            self.changed();
        }
    }

    async fn refresh_session_commands(&self, key: &str) {
        let scope = read_lock(&self.state).sessions.get(key).map(|session| {
            (
                session.state.engine.clone(),
                session.state.cwd.clone(),
                session.state.model.clone(),
                session.setting_sources.clone(),
            )
        });
        let Some((engine, cwd, model, setting_sources)) = scope else {
            return;
        };
        let Ok(engine) = EngineId::new(engine) else {
            return;
        };
        let result = self
            .runtime
            .list_provider_commands(ProviderCommandScope {
                engine,
                cwd,
                model,
                setting_sources,
            })
            .await;
        let mut state = write_lock(&self.state);
        let Some(session) = state.sessions.get_mut(key) else {
            return;
        };
        session.commands = catalogue_from(result);
        drop(state);
        self.changed();
    }

    pub(super) fn changed(&self) {
        self.revision
            .send_modify(|value| *value = value.saturating_add(1));
    }

    pub(super) async fn open_new(
        &self,
        request: NewSession,
    ) -> Result<AgentSessionState, AgentProtocolError> {
        self.open_new_classified(request)
            .await
            .map_err(|failure| failure.error)
    }

    /// `open_new`, telling a start that never reached the provider (its
    /// prompt can go back to the composer) from one whose prompt may have.
    pub(super) async fn open_new_classified(
        &self,
        NewSession {
            engine,
            cwd,
            prompt,
            model,
            effort,
            access,
            fast_mode,
            scope,
            name,
            task,
        }: NewSession,
    ) -> Result<AgentSessionState, StartFailure> {
        let engine_id = EngineId::new(engine.clone()).map_err(|error| {
            StartFailure::unsent(AgentProtocolError::new(
                AgentErrorCode::InvalidRequest,
                error.to_string(),
            ))
        })?;
        let setting_sources = scope
            .setting_sources()
            .map(|sources| sources.into_iter().map(str::to_owned).collect());
        let opened = self
            .runtime
            .start_session(
                &engine_id,
                LaunchOptions {
                    cwd: cwd.clone(),
                    prompt: prompt.clone(),
                    model: model.clone(),
                    effort: effort.clone(),
                    permission_mode: access.clone(),
                    fast_mode: fast_mode.then_some(true),
                    name: name.clone(),
                    setting_sources: setting_sources.clone(),
                    ..LaunchOptions::default()
                },
            )
            .await;
        let runtime_session = started_session(opened)?;
        // The provider holds the prompt from here on: any later failure
        // leaves its outcome unknown.
        if let Some(collaboration) = &self.collaboration {
            collaboration
                .activate_session(&runtime_session.key)
                .and_then(|()| collaboration.observe_registry_change(&runtime_session.key))
                .map_err(|message| {
                    StartFailure::unknown(protocol(AgentErrorCode::Failed, message))
                })?;
        }
        let session = AgentSessionState {
            key: runtime_session.key.clone(),
            engine,
            session_id: runtime_session.session_id,
            status: if prompt.is_empty() {
                AgentStatus::Idle
            } else {
                AgentStatus::Working
            },
            cwd,
            title: name.unwrap_or_else(|| "New session".into()),
            model,
            task,
            capabilities: Some(capabilities_for(&engine_id)),
            live: true,
        };
        let transcript = if prompt.is_empty() {
            Transcript::default()
        } else {
            Transcript::new(vec![TranscriptCell::Message {
                role: "user",
                markdown: for_display(&prompt),
            }])
        };
        write_lock(&self.state).sessions.insert(
            session.key.clone(),
            ControlledSession {
                state: session.clone(),
                transcript,
                queued: VecDeque::new(),
                pending_approval: None,
                effort,
                access,
                fast_mode,
                models: Vec::new(),
                commands: self.launch_commands(),
                setting_sources,
                disallowed_tools: None,
                context: None,
                previous_context: None,
                last_activity: chrono::Utc::now().to_rfc3339(),
                telemetry_startup_recorded: false,
                telemetry_pending: false,
                telemetry_recorded: None,
            },
        );
        self.set_active(Some(session.key.clone()));
        if !prompt.is_empty() {
            self.record_prompt(&engine_id, &session.cwd, &prompt).await;
        }
        self.spawn_event_pump(&session.key)
            .await
            .map_err(StartFailure::unknown)?;
        Ok(session)
    }

    pub(super) async fn resume_key(
        &self,
        key: &str,
    ) -> Result<AgentSessionState, AgentProtocolError> {
        let current = self.agent_session(key)?;
        if current.live {
            return Ok(current);
        }
        if let Some(collaboration) = &self.collaboration {
            collaboration
                .guard_session_mutation(key, "resume", false)
                .await
                .map_err(|message| protocol(AgentErrorCode::Blocked, message))?;
        }
        let engine = EngineId::new(current.engine.clone()).map_err(|error| {
            AgentProtocolError::new(AgentErrorCode::InvalidRequest, error.to_string())
        })?;
        self.refresh_history(key).await?;
        // Before the event pump starts (0.1.169's order): a late answer must
        // not overwrite the catalogue the live session announces itself.
        self.refresh_session_commands(key).await;
        let resume = read_lock(&self.state)
            .sessions
            .get(key)
            .map(|row| ResumeOptions {
                setting_sources: row.setting_sources.clone(),
                disallowed_tools: row.disallowed_tools.clone(),
                model: row.state.model.clone(),
                effort: row.effort.clone(),
                ..ResumeOptions::default()
            })
            .ok_or_else(|| {
                AgentProtocolError::new(AgentErrorCode::NotFound, "Agent session was not found.")
            })?;
        let opened = self
            .runtime
            .resume_session(
                &engine,
                current.session_id.clone(),
                current.cwd.clone(),
                resume,
            )
            .await;
        let runtime_session = ready_session(opened)?;
        if let Some(collaboration) = &self.collaboration {
            collaboration
                .activate_session(&runtime_session.key)
                .and_then(|()| collaboration.observe_registry_change(&runtime_session.key))
                .map_err(|message| protocol(AgentErrorCode::Failed, message))?;
        }
        {
            let mut state = write_lock(&self.state);
            let row = state.sessions.get_mut(key).ok_or_else(|| {
                AgentProtocolError::new(AgentErrorCode::NotFound, "Agent session was not found.")
            })?;
            row.state.live = true;
            row.state.status = AgentStatus::Idle;
        }
        self.spawn_event_pump(&runtime_session.key).await?;
        self.changed();
        self.agent_session(&runtime_session.key)
    }

    async fn refresh_history(&self, key: &str) -> Result<(), AgentProtocolError> {
        let history = self
            .runtime
            .read_provider_history(key, HISTORY_MAX_LIMIT)
            .await
            .map_err(|error| AgentProtocolError::new(AgentErrorCode::Failed, error.message))?;
        let mut state = write_lock(&self.state);
        let row = state.sessions.get_mut(key).ok_or_else(|| {
            AgentProtocolError::new(AgentErrorCode::NotFound, "Agent session was not found.")
        })?;
        replace_provider_history(row, history);
        drop(state);
        self.changed();
        Ok(())
    }

    async fn spawn_event_pump(&self, key: &str) -> Result<(), AgentProtocolError> {
        let stream = self
            .runtime
            .take_session_events(key)
            .await
            .map_err(|error| AgentProtocolError::new(AgentErrorCode::Failed, error.message))?;
        let controller = self.clone();
        let key = key.to_owned();
        tokio::spawn(async move {
            controller.consume_events(key, stream).await;
        });
        Ok(())
    }

    async fn consume_events(
        &self,
        key: String,
        mut stream: interface_engine::BoundedEventStream<RuntimeEventFrame>,
    ) {
        loop {
            match stream.recv().await {
                Ok(Some(RuntimeEventFrame::Event { value })) => {
                    let became_idle = matches!(
                        value.event.event,
                        EngineEventKind::StatusChanged {
                            status: SessionStatus::Idle
                        }
                    );
                    if let Some(collaboration) = &self.collaboration
                        && let Err(error) = collaboration.observe_event(&key, &value.event)
                    {
                        self.apply_event(
                            &key,
                            &EngineEvent {
                                at: None,
                                event: EngineEventKind::Notice {
                                    tone: NoticeTone::Warning,
                                    message: error,
                                },
                            },
                        );
                    }
                    self.apply_event(&key, &value.event);
                    if became_idle {
                        self.send_next_queued(&key).await;
                    }
                }
                Ok(Some(RuntimeEventFrame::Terminal {
                    reason, message, ..
                })) => {
                    let mut state = write_lock(&self.state);
                    if let Some(row) = state.sessions.get_mut(&key) {
                        row.state.live = false;
                        row.state.status =
                            if matches!(reason, interface_runtime::RuntimeEventTermination::Closed)
                            {
                                AgentStatus::NotLive
                            } else {
                                AgentStatus::Failed
                            };
                        if let Some(message) = message {
                            push_notice(row, &message, CellTone::Error);
                        }
                    }
                    drop(state);
                    self.changed();
                    break;
                }
                Ok(None) => break,
                Err(error) => {
                    let mut state = write_lock(&self.state);
                    if let Some(row) = state.sessions.get_mut(&key) {
                        row.state.status = AgentStatus::Failed;
                        row.state.live = false;
                        push_notice(row, &error.to_string(), CellTone::Error);
                    }
                    drop(state);
                    self.changed();
                    break;
                }
            }
        }
    }

    fn apply_event(&self, key: &str, event: &EngineEvent) {
        let mut state = write_lock(&self.state);
        let Some(row) = state.sessions.get_mut(key) else {
            return;
        };
        let before = row.state.status;
        let title = row.state.title.clone();
        reduce_event(row, event);
        let telemetry = context_telemetry_sample(key, row, event, before);
        let notification = match &event.event {
            EngineEventKind::ApprovalRequested { summary, .. } => Some(TerminalNotification {
                title: "iface approval".into(),
                body: for_display(summary),
            }),
            EngineEventKind::StatusChanged {
                status: SessionStatus::Idle,
            } if matches!(before, AgentStatus::Working | AgentStatus::Blocked) => {
                Some(TerminalNotification {
                    title: "iface turn complete".into(),
                    body: for_display(&title),
                })
            }
            _ => None,
        };
        drop(state);
        if let (Some(diagnostics), Some(telemetry)) = (&self.diagnostics, telemetry) {
            diagnostics.record_context(telemetry);
        }
        if let Some(notification) = notification {
            let mut notifications = write_lock(&self.notifications);
            if notifications.len() == 32 {
                notifications.pop_front();
            }
            notifications.push_back(notification);
        }
        self.changed();
    }

    pub(super) fn take_notifications(&self) -> Vec<TerminalNotification> {
        write_lock(&self.notifications).drain(..).collect()
    }

    pub(super) fn take_queued_send_failures(&self) -> Vec<QueuedSendFailure> {
        write_lock(&self.queued_failures).drain(..).collect()
    }

    async fn send_next_queued(&self, key: &str) {
        let next = write_lock(&self.state)
            .sessions
            .get_mut(key)
            .and_then(|row| row.queued.pop_front());
        if let Some(text) = next {
            let delivery = self.send_now(key, text.clone()).await;
            // An auto-send that did not reach the provider must never vanish:
            // the prompt is handed back to the UI so the user keeps the text.
            if let Some(failure) = queued_send_failure(key, text, &delivery) {
                let mut failures = write_lock(&self.queued_failures);
                if failures.len() == QUEUED_FAILURE_LIMIT {
                    failures.pop_front();
                }
                failures.push_back(failure);
                drop(failures);
                self.changed();
            }
            let queue_empty = read_lock(&self.state)
                .sessions
                .get(key)
                .is_none_or(|row| row.queued.is_empty());
            if queue_empty {
                let _ = self.project_draft(key, true);
            }
        }
    }

    /// Records an unsent queued prompt in the session transcript so the text
    /// stays copyable. Used whenever restoring it to the composer would either
    /// clobber the user's own draft or risk a second send.
    pub(super) fn note_unsent_queued_prompt(&self, failure: &QueuedSendFailure) {
        let mut state = write_lock(&self.state);
        if let Some(row) = state.sessions.get_mut(&failure.key) {
            push_notice(row, &failure.transcript_notice(), CellTone::Warning);
        }
        drop(state);
        self.changed();
    }

    pub(super) async fn send_now(&self, key: &str, text: String) -> AgentDelivery {
        let prompt = text.clone();
        let delivery = self.projected_send(key, text).await;
        if delivery.kind == AgentDeliveryKind::AdapterAccepted {
            if let Some(row) = write_lock(&self.state).sessions.get_mut(key) {
                row.state.status = AgentStatus::Working;
                push_message(row, "user", &prompt);
            }
            self.changed();
            let identity = {
                let state = read_lock(&self.state);
                state.sessions.get(key).map(|row| {
                    (
                        EngineId::new(row.state.engine.clone()),
                        row.state.cwd.clone(),
                    )
                })
            };
            if let Some((Ok(engine), cwd)) = identity {
                self.record_prompt(&engine, &cwd, &prompt).await;
            }
        }
        delivery
    }

    async fn projected_send(&self, key: &str, text: String) -> AgentDelivery {
        if let Some(collaboration) = &self.collaboration {
            if let Err(message) = collaboration
                .guard_session_mutation(key, "send", false)
                .await
            {
                return definitely_unsent(message);
            }
            if let Err(message) = collaboration.user_send_started(key) {
                return definitely_unsent(message);
            }
        }
        let delivery = delivery_from_runtime(self.runtime.send(key, text.clone()).await);
        if let Some(collaboration) = &self.collaboration {
            let outcome = match delivery.kind {
                AgentDeliveryKind::AdapterAccepted => ProjectedSendOutcome::AdapterAccepted,
                AgentDeliveryKind::DefinitelyUnsent => ProjectedSendOutcome::DefinitelyUnsent,
                AgentDeliveryKind::OutcomeUnknown => ProjectedSendOutcome::OutcomeUnknown,
            };
            if let Err(message) = collaboration.user_send_settled(key, &text, outcome) {
                return AgentDelivery {
                    kind: AgentDeliveryKind::OutcomeUnknown,
                    message: Some(message),
                    queued: false,
                };
            }
        }
        delivery
    }

    pub(super) fn project_draft(&self, key: &str, empty: bool) -> Result<(), String> {
        let Some(collaboration) = &self.collaboration else {
            return Ok(());
        };
        // Number and apply under one lock. A keystroke, the event pump
        // draining a queue and a socket send can all project at once, and a
        // revision applied after a higher one is refused as stale.
        let mut revision = self
            .draft_revision
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *revision += 1;
        collaboration.set_draft_state(CollaborationDraftStateUpdate {
            source_session_key: key.to_owned(),
            revision: *revision,
            empty,
        })
    }

    pub(super) async fn record_prompt(&self, engine: &EngineId, cwd: &str, text: &str) {
        let at = interface_contracts::canonical_timestamp(chrono::Utc::now());
        let _ = self.prompt_history.append(&at, engine, cwd, text).await;
    }

    pub(super) async fn setting_effect(
        &self,
        operation: impl std::future::Future<Output = interface_runtime::RuntimeDelivery>,
    ) -> Result<(), String> {
        require_confirmed(delivery_from_runtime(operation.await))
    }

    /// Runs one session setting change behind the collaboration lock. Returns
    /// the refusal to show, or `None` when the adapter confirmed the change.
    pub(super) async fn guarded_setting(
        &self,
        key: &str,
        action: &str,
        operation: impl std::future::Future<Output = interface_runtime::RuntimeDelivery>,
    ) -> Option<String> {
        if let Some(collaboration) = &self.collaboration
            && let Err(error) = collaboration
                .guard_session_mutation(key, action, false)
                .await
        {
            return Some(error);
        }
        self.setting_effect(operation).await.err()
    }

    pub(super) fn view(&self) -> TuiView {
        let state = read_lock(&self.state);
        let active = state
            .active
            .as_ref()
            .and_then(|key| state.sessions.get(key));
        let launch = read_lock(&self.launch);
        TuiView {
            engine: Some(
                active.map_or_else(|| launch.engine.clone(), |row| row.state.engine.clone()),
            ),
            model: active
                .and_then(|row| row.state.model.clone())
                .or_else(|| launch.model.clone()),
            cwd: Some(active.map_or_else(|| launch.cwd.clone(), |row| row.state.cwd.clone())),
            status: active.map(|row| row.state.status),
            effort: active
                .and_then(|row| row.effort.clone())
                .or_else(|| launch.effort.clone()),
            access: active
                .and_then(|row| row.access.clone())
                .or_else(|| launch.access.clone()),
            fast_mode: active.map_or(launch.fast_mode, |row| row.fast_mode),
            context: active.and_then(|row| row.context.clone()),
            previous_context: active.and_then(|row| row.previous_context.clone()),
        }
    }

    /// Clones the active session's transcript handle.
    ///
    /// Deliberately separate from [`Self::view`]: holding this `Arc` across a
    /// draw would make every `Arc::make_mut` push deep-clone the transcript, so
    /// only the paths that actually read cells take it.
    pub(super) fn transcript_snapshot(&self) -> Arc<Vec<TranscriptCell>> {
        let state = read_lock(&self.state);
        state
            .active
            .as_ref()
            .and_then(|key| state.sessions.get(key))
            .map_or_else(Arc::default, |row| row.transcript.snapshot())
    }

    /// Runs `read` on the active session's transcript under one short read
    /// lock. The layout copies only the cells it must restyle; no `Arc` of the
    /// cells outlives the lock, so a concurrent edit never clones them. `read`
    /// must stay synchronous and take no other lock (state before launch).
    pub(super) fn with_active_transcript<T>(
        &self,
        read: impl FnOnce(Option<&Transcript>) -> T,
    ) -> T {
        let state = read_lock(&self.state);
        read(
            state
                .active
                .as_ref()
                .and_then(|key| state.sessions.get(key))
                .map(|row| &row.transcript),
        )
    }

    pub(super) async fn steer_with_history(
        &self,
        key: &str,
        text: String,
    ) -> Result<(AgentDelivery, Option<String>), AgentProtocolError> {
        let state = self.agent_session(key)?;
        if !state.live {
            return Err(protocol(
                AgentErrorCode::NotLive,
                "Agent session is not live.",
            ));
        }
        if state.status != AgentStatus::Working || !capabilities_for_text(&state.engine).steer {
            return Err(protocol(
                AgentErrorCode::Unsupported,
                "Agent session does not support mid-turn steering.",
            ));
        }
        let engine = EngineId::new(state.engine.clone()).map_err(|error| {
            AgentProtocolError::new(AgentErrorCode::InvalidRequest, error.to_string())
        })?;
        // The Pending row's fsync runs on the blocking pool, and is awaited:
        // it is durable before any provider contact.
        let history = self.steer_history.clone();
        let (history_key, prompt, model) = (key.to_owned(), text.clone(), state.model.clone());
        let record = tokio::task::spawn_blocking(move || {
            history.begin(&history_key, &engine, model.as_deref(), &prompt, None)
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|recorded| recorded)
        .map_err(|_| {
            protocol(
                AgentErrorCode::Failed,
                "iface could not record this steering attempt, so it stopped before provider contact.",
            )
        })?;
        let delivery = self.projected_send(key, text.clone()).await;
        if delivery.kind == AgentDeliveryKind::AdapterAccepted {
            push_accepted_steer(self, key, &state, &text).await;
        }
        let outcome = match delivery.kind {
            AgentDeliveryKind::AdapterAccepted => SteerSettledOutcome::AdapterAccepted,
            AgentDeliveryKind::DefinitelyUnsent => SteerSettledOutcome::DefinitelyUnsent,
            AgentDeliveryKind::OutcomeUnknown => SteerSettledOutcome::OutcomeUnknown,
        };
        let detail = (delivery.kind != AgentDeliveryKind::AdapterAccepted)
            .then(|| delivery.message.clone())
            .flatten();
        let history = self.steer_history.clone();
        let settled = tokio::task::spawn_blocking(move || {
            history.settle(&record.id, outcome, None, detail.as_deref())
        })
        .await;
        let warning = (!matches!(settled, Ok(Ok(_)))).then(|| {
            "The provider outcome was handled, but its local steering record is still marked pending."
                .to_owned()
        });
        Ok((delivery, warning))
    }

    pub(super) async fn shutdown(&self) {
        self.prompt_history.flush().await;
        self.steer_history.flush();
        if let Some(orchestration) = &self.orchestration {
            let _ = orchestration.close().await;
        }
        if let Some(collaboration) = &self.collaboration {
            let _ = collaboration.quiesce_and_close().await;
        }
        let _ = self.runtime.shutdown().await;
        if let Some(collaboration) = &self.collaboration {
            let _ = collaboration.drain().await;
        }
    }
}
