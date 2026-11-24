//! The launch-settings commands: `/cwd`, `/engine`, `/model`, `/effort`,
//! `/fast` and access, their pickers and setters, and the access cycle key.
use super::{
    CwdScan, PaletteAction, PaletteKind, Path, PathBuf, PickerItem, TuiApp, canonical_directory,
    effort_display_name, engine_display_name, expand_home, for_display, read_lock, write_lock,
};

impl TuiApp {
    pub(super) async fn cwd_command(&mut self, argument: &str) {
        if self.controller.active_key().is_some() {
            self.notice = Some("Use /new before choosing a different launch directory.".into());
            return;
        }
        let launch = self.controller.launch();
        let typed = if argument.is_empty() {
            PathBuf::from(&launch.cwd)
        } else {
            expand_home(argument)
        };
        let resolved = if typed.is_absolute() {
            typed
        } else {
            Path::new(&launch.cwd).join(typed)
        };
        // A stat, a canonicalize and a directory read: seconds on an evicted
        // or network folder, so they run off the worker that draws.
        let typed_argument = !argument.is_empty();
        let scanned = tokio::task::spawn_blocking(move || {
            if typed_argument && let Ok(path) = canonical_directory(&resolved) {
                return Ok(CwdScan::Directory(path));
            }
            let absolute = std::path::absolute(&resolved).unwrap_or(resolved);
            let (directory, prefix) = if typed_argument {
                (
                    absolute
                        .parent()
                        .map_or_else(PathBuf::new, Path::to_path_buf),
                    absolute
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or_default()
                        .to_ascii_lowercase(),
                )
            } else {
                (absolute, String::new())
            };
            let entries = std::fs::read_dir(&directory)?;
            let mut candidates = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .starts_with(&prefix)
                })
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
            Ok(CwdScan::Candidates(directory, candidates))
        })
        .await
        .unwrap_or_else(|_| Err(std::io::Error::other("the directory scan stopped")));
        let (directory, candidates) = match scanned {
            Ok(CwdScan::Directory(path)) => {
                self.set_cwd(path).await;
                return;
            }
            Ok(CwdScan::Candidates(directory, candidates)) => (directory, candidates),
            Err(error) => {
                self.notice = Some(format!("Directory completion failed: {error}"));
                return;
            }
        };
        if candidates.is_empty() {
            self.notice = Some(format!(
                "No directory completion matches {}.",
                if argument.is_empty() {
                    directory.to_string_lossy().into_owned()
                } else {
                    argument.to_owned()
                }
            ));
            return;
        }
        let items = candidates
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| {
                PickerItem::new(
                    format!("cwd:{index}"),
                    for_display(
                        candidate
                            .file_name()
                            .and_then(|value| value.to_str())
                            .unwrap_or_default(),
                    ),
                    Some(for_display(&candidate.to_string_lossy())),
                    None,
                    PaletteAction::SetCwd(candidate.to_string_lossy().into_owned()),
                    None,
                )
            })
            .collect();
        self.open_picker(PaletteKind::Cwd, "Launch directory", items, "");
    }

    pub(super) async fn set_cwd(&mut self, candidate: PathBuf) {
        match canonical_directory(&candidate) {
            Ok(path) => {
                let cwd = path.to_string_lossy().into_owned();
                write_lock(&self.controller.launch).cwd.clone_from(&cwd);
                self.pending_access_confirmation = None;
                self.controller.refresh_launch_commands().await;
                self.notice = Some(cwd);
            }
            Err(error) => self.notice = Some(error),
        }
    }

    pub(super) async fn engine_command(&mut self, argument: &str) {
        if self.controller.active_key().is_some() {
            self.notice = Some("Use /new before choosing a launch engine.".into());
            return;
        }
        if !argument.is_empty() {
            if self.controller.probe(argument).is_none() {
                self.notice = Some("Choose an engine from the detected roster.".into());
                return;
            }
            self.set_engine(argument.to_owned()).await;
            return;
        }
        let items = self
            .controller
            .probes()
            .into_iter()
            .map(|probe| {
                let ready = probe.info.is_some();
                let label = probe
                    .info
                    .as_ref()
                    .and_then(|info| info.display_name.clone())
                    .unwrap_or_else(|| probe.engine.to_string());
                let description = probe.info.as_ref().map_or_else(
                    || probe.error.unwrap_or_else(|| "unavailable".into()),
                    |info| format!("available · {}", info.version),
                );
                PickerItem::new(
                    format!("engine:{}", probe.engine),
                    for_display(&label),
                    Some(for_display(&description)),
                    Some(if ready { "ready" } else { "down" }.into()),
                    PaletteAction::SetEngine(probe.engine.to_string()),
                    None,
                )
            })
            .collect();
        self.open_picker(PaletteKind::Engine, "Engines", items, "");
    }

    pub(super) async fn set_engine(&mut self, engine: String) {
        {
            let mut launch = write_lock(&self.controller.launch);
            launch.engine.clone_from(&engine);
            launch.model = None;
            launch.effort = None;
            launch.fast_mode = false;
            launch.access = self.config.get().access_mode_for_launch(&engine);
        }
        self.pending_access_confirmation = None;
        self.config.remember_engine(engine.clone());
        self.save_config().await;
        self.controller.refresh_launch_commands().await;
        self.notice = Some(engine);
    }

    pub(super) fn open_model_picker(&mut self) {
        let items = if let Some(key) = self.controller.active_key() {
            let engine = read_lock(&self.controller.state)
                .sessions
                .get(&key)
                .map_or_else(
                    || self.controller.launch().engine,
                    |row| row.state.engine.clone(),
                );
            std::iter::once(PickerItem::new(
                "model:default".into(),
                "Provider default".into(),
                None,
                None,
                PaletteAction::SetModel {
                    engine: engine.clone(),
                    model: None,
                },
                None,
            ))
            .chain(self.controller.active_models().into_iter().enumerate().map(
                move |(index, model)| {
                    PickerItem::new(
                        format!("model:{index}"),
                        for_display(&model.display_name),
                        model.description.as_deref().map(for_display),
                        None,
                        PaletteAction::SetModel {
                            engine: engine.clone(),
                            model: Some(model.value),
                        },
                        None,
                    )
                },
            ))
            .collect()
        } else {
            self.controller
                .probes()
                .into_iter()
                .flat_map(|probe| {
                    let engine = probe.engine.to_string();
                    let name = probe
                        .info
                        .as_ref()
                        .and_then(|info| info.display_name.clone())
                        .unwrap_or_else(|| engine_display_name(&engine));
                    let mut rows = vec![PickerItem::new(
                        format!("model:{engine}:default"),
                        format!("{} · Provider default", for_display(&name)),
                        None,
                        None,
                        PaletteAction::SetModel {
                            engine: engine.clone(),
                            model: None,
                        },
                        None,
                    )];
                    rows.extend(
                        probe
                            .info
                            .and_then(|info| info.model_catalog)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|model| {
                                !model.value.trim().is_empty() && model.value != "default"
                            })
                            .enumerate()
                            .map(|(index, model)| {
                                PickerItem::new(
                                    format!("model:{engine}:{index}"),
                                    format!(
                                        "{} · {}",
                                        for_display(&name),
                                        for_display(&model.display_name)
                                    ),
                                    model.description.as_deref().map(for_display),
                                    None,
                                    PaletteAction::SetModel {
                                        engine: engine.clone(),
                                        model: Some(model.value),
                                    },
                                    None,
                                )
                            }),
                    );
                    rows
                })
                .collect()
        };
        self.open_picker(PaletteKind::Models, "Models", items, "");
    }

    pub(super) async fn set_model(&mut self, model: Option<String>) {
        if let Some(key) = self.controller.active_key() {
            let models = self.controller.active_models();
            if model
                .as_ref()
                .is_some_and(|value| !models.iter().any(|row| row.value == *value))
            {
                self.notice = Some("Choose a model from the live provider catalogue.".into());
                return;
            }
            self.notice = self
                .controller
                .guarded_setting(
                    &key,
                    "model change",
                    self.controller.runtime.set_model(&key, model.clone()),
                )
                .await;
            if self.notice.is_some() {
                return;
            }
            if let Some(session) = write_lock(&self.controller.state).sessions.get_mut(&key) {
                session.state.model.clone_from(&model);
            }
            write_lock(&self.controller.launch).model.clone_from(&model);
            self.config.remember_model(model.clone());
            self.save_config().await;
            self.notice = Some(model.unwrap_or_else(|| "Provider default".into()));
            return;
        }
        let engine = if let Some(value) = &model {
            let launch_engine = self.controller.launch().engine;
            if self
                .controller
                .model_catalogue(&launch_engine)
                .iter()
                .any(|row| row.value == *value)
            {
                Some(launch_engine)
            } else {
                self.controller
                    .probes()
                    .into_iter()
                    .find(|probe| {
                        probe
                            .info
                            .as_ref()
                            .and_then(|info| info.model_catalog.as_ref())
                            .is_some_and(|models| models.iter().any(|row| row.value == *value))
                    })
                    .map(|probe| probe.engine.to_string())
            }
        } else {
            Some(self.controller.launch().engine)
        };
        let Some(engine) = engine else {
            self.notice = Some("Choose a model from the live provider catalogue.".into());
            return;
        };
        if engine != self.controller.launch().engine {
            self.set_engine(engine).await;
        }
        write_lock(&self.controller.launch).model.clone_from(&model);
        if !self.controller.supports_fast_mode() {
            write_lock(&self.controller.launch).fast_mode = false;
        }
        self.config.remember_model(model);
        self.save_config().await;
        self.pending_access_confirmation = None;
        self.controller.refresh_launch_commands().await;
    }

    pub(super) async fn set_model_for_engine(&mut self, engine: String, model: Option<String>) {
        if self.controller.active_key().is_none() && engine != self.controller.launch().engine {
            self.set_engine(engine).await;
        }
        self.set_model(model).await;
    }

    pub(super) fn open_effort_picker(&mut self) {
        let items =
            std::iter::once(PickerItem::new(
                "effort:default".into(),
                "Provider default".into(),
                None,
                None,
                PaletteAction::SetEffort(None),
                None,
            ))
            .chain(self.controller.effort_levels().into_iter().enumerate().map(
                |(index, effort)| {
                    PickerItem::new(
                        format!("effort:{index}"),
                        effort_display_name(&effort),
                        None,
                        None,
                        PaletteAction::SetEffort(Some(effort)),
                        None,
                    )
                },
            ))
            .collect();
        self.open_picker(PaletteKind::Effort, "Effort", items, "");
    }

    pub(super) async fn set_effort(&mut self, effort: Option<String>) {
        if effort
            .as_ref()
            .is_some_and(|value| !self.controller.effort_levels().contains(value))
        {
            self.notice = Some("Choose an effort from the live model catalogue.".into());
            return;
        }
        if let Some(key) = self.controller.active_key() {
            self.notice = self
                .controller
                .guarded_setting(
                    &key,
                    "effort change",
                    self.controller.runtime.set_effort(&key, effort.clone()),
                )
                .await;
            if self.notice.is_some() {
                return;
            }
            if let Some(session) = write_lock(&self.controller.state).sessions.get_mut(&key) {
                session.effort.clone_from(&effort);
            }
        }
        write_lock(&self.controller.launch)
            .effort
            .clone_from(&effort);
        self.config.remember_effort(effort.clone());
        self.save_config().await;
        self.pending_access_confirmation = None;
        self.notice = Some(effort.unwrap_or_else(|| "Provider default".into()));
    }

    pub(super) fn open_fast_picker(&mut self) {
        if !self.controller.supports_fast_mode() {
            self.notice =
                Some("The exact selected model does not advertise fast-mode support.".into());
            return;
        }
        let enabled = self.controller.launch().fast_mode;
        let items = vec![
            PickerItem::new(
                "fast:on".into(),
                "On".into(),
                Some("Ask the provider for faster output".into()),
                enabled.then(|| "current".into()),
                PaletteAction::SetFast(true),
                None,
            ),
            PickerItem::new(
                "fast:off".into(),
                "Off".into(),
                Some("Use normal provider mode".into()),
                (!enabled).then(|| "current".into()),
                PaletteAction::SetFast(false),
                None,
            ),
        ];
        self.open_picker(PaletteKind::Fast, "Fast mode", items, "");
    }

    pub(super) async fn set_fast_mode(&mut self, enabled: bool) {
        if !self.controller.supports_fast_mode() {
            write_lock(&self.controller.launch).fast_mode = false;
            self.notice =
                Some("The exact selected model does not advertise fast-mode support.".into());
            return;
        }
        if let Some(key) = self.controller.active_key() {
            self.notice = self
                .controller
                .guarded_setting(
                    &key,
                    "fast-mode change",
                    self.controller.runtime.set_fast_mode(&key, enabled),
                )
                .await;
            if self.notice.is_some() {
                return;
            }
            if let Some(session) = write_lock(&self.controller.state).sessions.get_mut(&key) {
                session.fast_mode = enabled;
            }
            self.notice = Some(
                if enabled {
                    "The adapter accepted fast mode on."
                } else {
                    "The adapter accepted fast mode off."
                }
                .into(),
            );
        } else {
            self.notice = Some(if enabled { "On" } else { "Off" }.into());
        }
        write_lock(&self.controller.launch).fast_mode = enabled;
    }

    pub(super) fn open_access_picker(&mut self) {
        let engine = self.controller.view().engine.unwrap_or_default();
        let items = self
            .controller
            .permission_modes()
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                PickerItem::new(
                    format!("access:{index}"),
                    for_display(&mode),
                    Some(
                        if crate::config::is_restorable_access_mode(&engine, &mode) {
                            "asking-or-stricter; safe to remember"
                        } else {
                            "explicit confirmation required"
                        }
                        .into(),
                    ),
                    None,
                    PaletteAction::SetAccess(mode),
                    None,
                )
            })
            .collect();
        self.open_picker(PaletteKind::Access, "Access mode", items, "");
    }

    pub(super) async fn set_access(&mut self, mode: String) {
        let engine = self.controller.view().engine.unwrap_or_default();
        if !self.controller.permission_modes().contains(&mode) {
            self.notice = Some("Choose a mode reported by this provider.".into());
            return;
        }
        let confirmation = (engine.clone(), mode.clone(), self.controller.active_key());
        if !crate::config::is_restorable_access_mode(&engine, &mode)
            && self.pending_access_confirmation.as_ref() != Some(&confirmation)
        {
            self.pending_access_confirmation = Some(confirmation);
            self.notice = Some(format!(
                "Select or enter {mode} once more. It is not auto-restorable and may allow actions without asking."
            ));
            return;
        }
        self.pending_access_confirmation = None;
        if let Some(key) = self.controller.active_key() {
            self.notice = self
                .controller
                .guarded_setting(
                    &key,
                    "permission change",
                    self.controller
                        .runtime
                        .set_permission_mode(&key, mode.clone()),
                )
                .await;
            if self.notice.is_some() {
                return;
            }
            if let Some(session) = write_lock(&self.controller.state).sessions.get_mut(&key) {
                session.access = Some(mode.clone());
            }
        }
        write_lock(&self.controller.launch).access = Some(mode.clone());
        self.config.remember_access_mode(&engine, &mode);
        self.save_config().await;
        self.notice = Some(mode);
    }

    pub(super) async fn cycle_access(&mut self) {
        let modes = self.controller.permission_modes();
        if modes.is_empty() {
            let engine = self.controller.view().engine.unwrap_or_default();
            self.notice = Some(format!(
                "{engine} did not report any modes to cycle through."
            ));
            return;
        }
        let current = self.controller.view().access;
        let index = modes
            .iter()
            .position(|mode| Some(mode.as_str()) == current.as_deref())
            .unwrap_or(0);
        self.set_access(modes[(index + 1) % modes.len()].clone())
            .await;
    }
}
