use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::PathBuf,
};

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u64);

        impl $name {
            pub const fn as_u64(self) -> u64 {
                self.0
            }

            pub(crate) const fn from_u64(value: u64) -> Self {
                Self(value)
            }
        }
    };
}

opaque_id!(SpaceId);
opaque_id!(TabId);
opaque_id!(PaneId);
opaque_id!(SplitId);
opaque_id!(TerminalSessionId);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitPlacement {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitRatio(u16);

impl SplitRatio {
    pub const MIN_PARTS: u16 = 100;
    pub const MAX_PARTS: u16 = 900;
    pub const EQUAL: Self = Self(500);

    pub fn new(parts_per_thousand: u16) -> Result<Self, CoreModelError> {
        if !(Self::MIN_PARTS..=Self::MAX_PARTS).contains(&parts_per_thousand) {
            return Err(CoreModelError::InvalidSplitRatio(parts_per_thousand));
        }
        Ok(Self(parts_per_thousand))
    }

    pub const fn parts_per_thousand(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreDisposition {
    Relaunch,
    RemainEnded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLaunch {
    pub working_directory: PathBuf,
    pub restore_disposition: RestoreDisposition,
}

impl TerminalLaunch {
    pub fn shell(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: working_directory.into(),
            restore_disposition: RestoreDisposition::Relaunch,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreSnapshot {
    pub revision: u64,
    pub spaces: Vec<SpaceSnapshot>,
    pub terminal_sessions: Vec<TerminalSessionSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceSnapshot {
    pub id: SpaceId,
    pub name: String,
    pub directory: PathBuf,
    pub tabs: Vec<TabSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabSnapshot {
    pub id: TabId,
    pub name: String,
    pub layout: PaneLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneLayout {
    Pane(PaneSnapshot),
    Split(SplitSnapshot),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSnapshot {
    pub id: PaneId,
    pub terminal_session_id: TerminalSessionId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitSnapshot {
    pub id: SplitId,
    pub axis: SplitAxis,
    pub ratio: SplitRatio,
    pub first: Box<PaneLayout>,
    pub second: Box<PaneLayout>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalSessionSnapshot {
    pub id: TerminalSessionId,
    pub launch: TerminalLaunch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistedCoreLayout {
    pub next_id: u64,
    pub spaces: Vec<PersistedSpace>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistedSpace {
    pub id: SpaceId,
    pub name: String,
    pub directory: PathBuf,
    pub tabs: Vec<PersistedTab>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistedTab {
    pub id: TabId,
    pub name: String,
    pub layout: PersistedPaneLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PersistedPaneLayout {
    Pane(PersistedPane),
    Split(PersistedSplit),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistedPane {
    pub id: PaneId,
    pub launch: TerminalLaunch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersistedSplit {
    pub id: SplitId,
    pub axis: SplitAxis,
    pub ratio: SplitRatio,
    pub first: Box<PersistedPaneLayout>,
    pub second: Box<PersistedPaneLayout>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CoreRestoreError(&'static str);

impl fmt::Display for CoreRestoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid Core snapshot: {}", self.0)
    }
}

impl std::error::Error for CoreRestoreError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreCommand {
    CreateSpace {
        name: String,
        directory: PathBuf,
    },
    RenameSpace {
        space_id: SpaceId,
        name: String,
    },
    CreateTab {
        space_id: SpaceId,
        name: String,
    },
    RenameTab {
        tab_id: TabId,
        name: String,
    },
    ReorderTab {
        tab_id: TabId,
        index: usize,
    },
    SplitPane {
        pane_id: PaneId,
        axis: SplitAxis,
        placement: SplitPlacement,
        ratio: SplitRatio,
    },
    MovePane {
        pane_id: PaneId,
        target_pane_id: PaneId,
        axis: SplitAxis,
        placement: SplitPlacement,
        ratio: SplitRatio,
    },
    ResizeSplit {
        split_id: SplitId,
        ratio: SplitRatio,
    },
    ClosePane {
        pane_id: PaneId,
    },
    SetRestoreDisposition {
        terminal_session_id: TerminalSessionId,
        disposition: RestoreDisposition,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreEffect {
    LaunchTerminal {
        terminal_session_id: TerminalSessionId,
        launch: TerminalLaunch,
    },
    StopTerminal {
        terminal_session_id: TerminalSessionId,
    },
    RestoreEndedTerminal {
        terminal_session_id: TerminalSessionId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreatedResource {
    None,
    Space {
        space_id: SpaceId,
        tab_id: TabId,
        pane_id: PaneId,
        terminal_session_id: TerminalSessionId,
    },
    Tab {
        tab_id: TabId,
        pane_id: PaneId,
        terminal_session_id: TerminalSessionId,
    },
    Pane {
        pane_id: PaneId,
        split_id: SplitId,
        terminal_session_id: TerminalSessionId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreCommit {
    pub revision: u64,
    pub snapshot: CoreSnapshot,
    pub effects: Vec<CoreEffect>,
    pub created: CreatedResource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Space,
    Tab,
    Pane,
    Split,
    TerminalSession,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoreModelError {
    StaleRevision { expected: u64, actual: u64 },
    NotFound { kind: ResourceKind, id: u64 },
    InvalidName,
    InvalidDirectory,
    InvalidSplitRatio(u16),
    TabIndexOutOfBounds { index: usize, tab_count: usize },
    CannotMovePaneOntoItself,
}

impl fmt::Display for CoreModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleRevision { expected, actual } => {
                write!(
                    formatter,
                    "stale Core revision {expected}; current revision is {actual}"
                )
            }
            Self::NotFound { kind, id } => write!(formatter, "{kind:?} {id} does not exist"),
            Self::InvalidName => formatter.write_str("names must contain 1 to 120 characters"),
            Self::InvalidDirectory => formatter.write_str("a Space directory cannot be empty"),
            Self::InvalidSplitRatio(ratio) => write!(
                formatter,
                "split ratio {ratio} is outside {}..={}",
                SplitRatio::MIN_PARTS,
                SplitRatio::MAX_PARTS
            ),
            Self::TabIndexOutOfBounds { index, tab_count } => {
                write!(formatter, "Tab index {index} is outside 0..{tab_count}")
            }
            Self::CannotMovePaneOntoItself => {
                formatter.write_str("a Pane cannot be moved relative to itself")
            }
        }
    }
}

impl std::error::Error for CoreModelError {}

pub struct CoreModel {
    snapshot: CoreSnapshot,
    next_id: u64,
}

impl Default for CoreModel {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreModel {
    pub fn new() -> Self {
        Self {
            snapshot: CoreSnapshot {
                revision: 0,
                spaces: Vec::new(),
                terminal_sessions: Vec::new(),
            },
            next_id: 1,
        }
    }

    pub fn snapshot(&self) -> CoreSnapshot {
        self.snapshot.clone()
    }

    pub(crate) fn persisted_layout(&self) -> Result<PersistedCoreLayout, CoreRestoreError> {
        let launches = self
            .snapshot
            .terminal_sessions
            .iter()
            .map(|session| (session.id, &session.launch))
            .collect::<HashMap<_, _>>();
        let spaces = self
            .snapshot
            .spaces
            .iter()
            .map(|space| {
                Ok(PersistedSpace {
                    id: space.id,
                    name: space.name.clone(),
                    directory: space.directory.clone(),
                    tabs: space
                        .tabs
                        .iter()
                        .map(|tab| {
                            Ok(PersistedTab {
                                id: tab.id,
                                name: tab.name.clone(),
                                layout: persist_pane_layout(&tab.layout, &launches)?,
                            })
                        })
                        .collect::<Result<Vec<_>, CoreRestoreError>>()?,
                })
            })
            .collect::<Result<Vec<_>, CoreRestoreError>>()?;
        Ok(PersistedCoreLayout {
            next_id: self.next_id,
            spaces,
        })
    }

    pub(crate) fn restore_layout(
        layout: PersistedCoreLayout,
    ) -> Result<(Self, Vec<CoreEffect>), CoreRestoreError> {
        let next_id = validate_persisted_layout(&layout)?;
        let mut model = Self {
            snapshot: CoreSnapshot {
                revision: 0,
                spaces: Vec::with_capacity(layout.spaces.len()),
                terminal_sessions: Vec::new(),
            },
            next_id,
        };
        let mut effects = Vec::new();
        for space in layout.spaces {
            let mut tabs = Vec::with_capacity(space.tabs.len());
            for tab in space.tabs {
                tabs.push(TabSnapshot {
                    id: tab.id,
                    name: tab.name,
                    layout: model.restore_pane_layout(tab.layout, &mut effects),
                });
            }
            model.snapshot.spaces.push(SpaceSnapshot {
                id: space.id,
                name: space.name,
                directory: space.directory,
                tabs,
            });
        }
        Ok((model, effects))
    }

    pub fn apply(
        &mut self,
        expected_revision: u64,
        command: CoreCommand,
    ) -> Result<CoreCommit, CoreModelError> {
        if expected_revision != self.snapshot.revision {
            return Err(CoreModelError::StaleRevision {
                expected: expected_revision,
                actual: self.snapshot.revision,
            });
        }

        let previous_snapshot = self.snapshot.clone();
        let previous_next_id = self.next_id;
        match self.apply_command(command) {
            Ok((effects, created)) => {
                self.snapshot.revision = self.snapshot.revision.saturating_add(1);
                Ok(CoreCommit {
                    revision: self.snapshot.revision,
                    snapshot: self.snapshot.clone(),
                    effects,
                    created,
                })
            }
            Err(error) => {
                self.snapshot = previous_snapshot;
                self.next_id = previous_next_id;
                Err(error)
            }
        }
    }

    fn apply_command(
        &mut self,
        command: CoreCommand,
    ) -> Result<(Vec<CoreEffect>, CreatedResource), CoreModelError> {
        match command {
            CoreCommand::CreateSpace { name, directory } => {
                validate_name(&name)?;
                validate_directory(&directory)?;
                let space_id = self.next_space_id();
                let tab_id = self.next_tab_id();
                let pane_id = self.next_pane_id();
                let terminal_session_id = self.next_terminal_session_id();
                let launch = TerminalLaunch::shell(directory.clone());
                self.snapshot.spaces.push(SpaceSnapshot {
                    id: space_id,
                    name,
                    directory,
                    tabs: vec![TabSnapshot {
                        id: tab_id,
                        name: "Terminal".into(),
                        layout: PaneLayout::Pane(PaneSnapshot {
                            id: pane_id,
                            terminal_session_id,
                        }),
                    }],
                });
                self.snapshot
                    .terminal_sessions
                    .push(TerminalSessionSnapshot {
                        id: terminal_session_id,
                        launch: launch.clone(),
                    });
                Ok((
                    vec![CoreEffect::LaunchTerminal {
                        terminal_session_id,
                        launch,
                    }],
                    CreatedResource::Space {
                        space_id,
                        tab_id,
                        pane_id,
                        terminal_session_id,
                    },
                ))
            }
            CoreCommand::RenameSpace { space_id, name } => {
                validate_name(&name)?;
                let space = self
                    .snapshot
                    .spaces
                    .iter_mut()
                    .find(|space| space.id == space_id)
                    .ok_or_else(|| not_found(ResourceKind::Space, space_id.as_u64()))?;
                space.name = name;
                Ok((Vec::new(), CreatedResource::None))
            }
            CoreCommand::CreateTab { space_id, name } => {
                validate_name(&name)?;
                let directory = self
                    .snapshot
                    .spaces
                    .iter()
                    .find(|space| space.id == space_id)
                    .map(|space| space.directory.clone())
                    .ok_or_else(|| not_found(ResourceKind::Space, space_id.as_u64()))?;
                let tab_id = self.next_tab_id();
                let pane_id = self.next_pane_id();
                let terminal_session_id = self.next_terminal_session_id();
                let launch = TerminalLaunch::shell(directory);
                self.snapshot
                    .spaces
                    .iter_mut()
                    .find(|space| space.id == space_id)
                    .expect("Space was validated before allocating IDs")
                    .tabs
                    .push(TabSnapshot {
                        id: tab_id,
                        name,
                        layout: PaneLayout::Pane(PaneSnapshot {
                            id: pane_id,
                            terminal_session_id,
                        }),
                    });
                self.snapshot
                    .terminal_sessions
                    .push(TerminalSessionSnapshot {
                        id: terminal_session_id,
                        launch: launch.clone(),
                    });
                Ok((
                    vec![CoreEffect::LaunchTerminal {
                        terminal_session_id,
                        launch,
                    }],
                    CreatedResource::Tab {
                        tab_id,
                        pane_id,
                        terminal_session_id,
                    },
                ))
            }
            CoreCommand::RenameTab { tab_id, name } => {
                validate_name(&name)?;
                find_tab_mut(&mut self.snapshot.spaces, tab_id)?.name = name;
                Ok((Vec::new(), CreatedResource::None))
            }
            CoreCommand::ReorderTab { tab_id, index } => {
                let (space_index, tab_index) = find_tab_location(&self.snapshot.spaces, tab_id)?;
                let tab_count = self.snapshot.spaces[space_index].tabs.len();
                if index >= tab_count {
                    return Err(CoreModelError::TabIndexOutOfBounds { index, tab_count });
                }
                let tab = self.snapshot.spaces[space_index].tabs.remove(tab_index);
                self.snapshot.spaces[space_index].tabs.insert(index, tab);
                Ok((Vec::new(), CreatedResource::None))
            }
            CoreCommand::SplitPane {
                pane_id,
                axis,
                placement,
                ratio,
            } => {
                let (_, tab_id) = find_pane_location(&self.snapshot.spaces, pane_id)?;
                let directory = find_space_for_tab(&self.snapshot.spaces, tab_id)
                    .expect("Pane location always identifies its owning Space")
                    .directory
                    .clone();
                let new_pane_id = self.next_pane_id();
                let terminal_session_id = self.next_terminal_session_id();
                let split_id = self.next_split_id();
                let launch = TerminalLaunch::shell(directory);
                let new_pane = PaneSnapshot {
                    id: new_pane_id,
                    terminal_session_id,
                };
                let inserted = replace_pane_with_split(
                    &mut find_tab_mut(&mut self.snapshot.spaces, tab_id)?.layout,
                    pane_id,
                    split_id,
                    axis,
                    ratio,
                    placement,
                    new_pane,
                );
                debug_assert!(inserted);
                self.snapshot
                    .terminal_sessions
                    .push(TerminalSessionSnapshot {
                        id: terminal_session_id,
                        launch: launch.clone(),
                    });
                Ok((
                    vec![CoreEffect::LaunchTerminal {
                        terminal_session_id,
                        launch,
                    }],
                    CreatedResource::Pane {
                        pane_id: new_pane_id,
                        split_id,
                        terminal_session_id,
                    },
                ))
            }
            CoreCommand::MovePane {
                pane_id,
                target_pane_id,
                axis,
                placement,
                ratio,
            } => {
                if pane_id == target_pane_id {
                    return Err(CoreModelError::CannotMovePaneOntoItself);
                }
                let (_, source_tab_id) = find_pane_location(&self.snapshot.spaces, pane_id)?;
                find_pane_location(&self.snapshot.spaces, target_pane_id)?;
                let (pane, source_tab_empty) =
                    detach_pane_from_tab(&mut self.snapshot.spaces, source_tab_id, pane_id)?;
                if source_tab_empty {
                    remove_tab_and_empty_space(&mut self.snapshot.spaces, source_tab_id);
                }
                let (_, target_tab_id) = find_pane_location(&self.snapshot.spaces, target_pane_id)?;
                let split_id = self.next_split_id();
                let inserted = replace_pane_with_split(
                    &mut find_tab_mut(&mut self.snapshot.spaces, target_tab_id)?.layout,
                    target_pane_id,
                    split_id,
                    axis,
                    ratio,
                    placement,
                    pane,
                );
                debug_assert!(inserted);
                Ok((Vec::new(), CreatedResource::None))
            }
            CoreCommand::ResizeSplit { split_id, ratio } => {
                let split = self
                    .snapshot
                    .spaces
                    .iter_mut()
                    .flat_map(|space| &mut space.tabs)
                    .find_map(|tab| find_split_mut(&mut tab.layout, split_id))
                    .ok_or_else(|| not_found(ResourceKind::Split, split_id.as_u64()))?;
                split.ratio = ratio;
                Ok((Vec::new(), CreatedResource::None))
            }
            CoreCommand::ClosePane { pane_id } => {
                let (_, tab_id) = find_pane_location(&self.snapshot.spaces, pane_id)?;
                let (pane, tab_empty) =
                    detach_pane_from_tab(&mut self.snapshot.spaces, tab_id, pane_id)?;
                if tab_empty {
                    remove_tab_and_empty_space(&mut self.snapshot.spaces, tab_id);
                }
                self.snapshot
                    .terminal_sessions
                    .retain(|session| session.id != pane.terminal_session_id);
                Ok((
                    vec![CoreEffect::StopTerminal {
                        terminal_session_id: pane.terminal_session_id,
                    }],
                    CreatedResource::None,
                ))
            }
            CoreCommand::SetRestoreDisposition {
                terminal_session_id,
                disposition,
            } => {
                let session = self
                    .snapshot
                    .terminal_sessions
                    .iter_mut()
                    .find(|session| session.id == terminal_session_id)
                    .ok_or_else(|| {
                        not_found(ResourceKind::TerminalSession, terminal_session_id.as_u64())
                    })?;
                session.launch.restore_disposition = disposition;
                Ok((Vec::new(), CreatedResource::None))
            }
        }
    }

    fn next_raw_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("Core IDs exhausted");
        id
    }

    fn next_space_id(&mut self) -> SpaceId {
        SpaceId::from_u64(self.next_raw_id())
    }

    fn next_tab_id(&mut self) -> TabId {
        TabId::from_u64(self.next_raw_id())
    }

    fn next_pane_id(&mut self) -> PaneId {
        PaneId::from_u64(self.next_raw_id())
    }

    fn next_split_id(&mut self) -> SplitId {
        SplitId::from_u64(self.next_raw_id())
    }

    fn next_terminal_session_id(&mut self) -> TerminalSessionId {
        TerminalSessionId::from_u64(self.next_raw_id())
    }

    fn restore_pane_layout(
        &mut self,
        layout: PersistedPaneLayout,
        effects: &mut Vec<CoreEffect>,
    ) -> PaneLayout {
        match layout {
            PersistedPaneLayout::Pane(pane) => {
                let terminal_session_id = self.next_terminal_session_id();
                let effect = match pane.launch.restore_disposition {
                    RestoreDisposition::Relaunch => CoreEffect::LaunchTerminal {
                        terminal_session_id,
                        launch: pane.launch.clone(),
                    },
                    RestoreDisposition::RemainEnded => CoreEffect::RestoreEndedTerminal {
                        terminal_session_id,
                    },
                };
                self.snapshot
                    .terminal_sessions
                    .push(TerminalSessionSnapshot {
                        id: terminal_session_id,
                        launch: pane.launch,
                    });
                effects.push(effect);
                PaneLayout::Pane(PaneSnapshot {
                    id: pane.id,
                    terminal_session_id,
                })
            }
            PersistedPaneLayout::Split(split) => PaneLayout::Split(SplitSnapshot {
                id: split.id,
                axis: split.axis,
                ratio: split.ratio,
                first: Box::new(self.restore_pane_layout(*split.first, effects)),
                second: Box::new(self.restore_pane_layout(*split.second, effects)),
            }),
        }
    }
}

fn persist_pane_layout(
    layout: &PaneLayout,
    launches: &HashMap<TerminalSessionId, &TerminalLaunch>,
) -> Result<PersistedPaneLayout, CoreRestoreError> {
    match layout {
        PaneLayout::Pane(pane) => Ok(PersistedPaneLayout::Pane(PersistedPane {
            id: pane.id,
            launch: (*launches
                .get(&pane.terminal_session_id)
                .ok_or(CoreRestoreError(
                    "Pane references a missing Terminal Session",
                ))?)
            .clone(),
        })),
        PaneLayout::Split(split) => Ok(PersistedPaneLayout::Split(PersistedSplit {
            id: split.id,
            axis: split.axis,
            ratio: split.ratio,
            first: Box::new(persist_pane_layout(&split.first, launches)?),
            second: Box::new(persist_pane_layout(&split.second, launches)?),
        })),
    }
}

fn validate_persisted_layout(layout: &PersistedCoreLayout) -> Result<u64, CoreRestoreError> {
    let mut ids = HashSet::new();
    let mut greatest_id = 0_u64;
    for space in &layout.spaces {
        validate_persisted_id(space.id.as_u64(), &mut ids, &mut greatest_id)?;
        validate_name(&space.name).map_err(|_| CoreRestoreError("Space name is invalid"))?;
        validate_directory(&space.directory)
            .map_err(|_| CoreRestoreError("Space directory is empty"))?;
        if space.tabs.is_empty() {
            return Err(CoreRestoreError("Space has no Tabs"));
        }
        for tab in &space.tabs {
            validate_persisted_id(tab.id.as_u64(), &mut ids, &mut greatest_id)?;
            validate_name(&tab.name).map_err(|_| CoreRestoreError("Tab name is invalid"))?;
            validate_persisted_pane_layout(&tab.layout, &mut ids, &mut greatest_id, 0)?;
        }
    }
    let minimum_next = greatest_id
        .checked_add(1)
        .ok_or(CoreRestoreError("Core IDs are exhausted"))?;
    if layout.next_id < minimum_next {
        return Err(CoreRestoreError("Core ID allocation watermark is stale"));
    }
    Ok(layout.next_id)
}

fn validate_persisted_pane_layout(
    layout: &PersistedPaneLayout,
    ids: &mut HashSet<u64>,
    greatest_id: &mut u64,
    depth: usize,
) -> Result<(), CoreRestoreError> {
    if depth > 256 {
        return Err(CoreRestoreError("Pane split nesting exceeds 256 levels"));
    }
    match layout {
        PersistedPaneLayout::Pane(pane) => {
            validate_persisted_id(pane.id.as_u64(), ids, greatest_id)?;
            validate_directory(&pane.launch.working_directory)
                .map_err(|_| CoreRestoreError("Pane launch directory is empty"))
        }
        PersistedPaneLayout::Split(split) => {
            validate_persisted_id(split.id.as_u64(), ids, greatest_id)?;
            SplitRatio::new(split.ratio.parts_per_thousand())
                .map_err(|_| CoreRestoreError("split ratio is invalid"))?;
            validate_persisted_pane_layout(&split.first, ids, greatest_id, depth + 1)?;
            validate_persisted_pane_layout(&split.second, ids, greatest_id, depth + 1)
        }
    }
}

fn validate_persisted_id(
    id: u64,
    ids: &mut HashSet<u64>,
    greatest_id: &mut u64,
) -> Result<(), CoreRestoreError> {
    if id == 0 {
        return Err(CoreRestoreError("Core ID is zero"));
    }
    if !ids.insert(id) {
        return Err(CoreRestoreError("Core ID is duplicated"));
    }
    *greatest_id = (*greatest_id).max(id);
    Ok(())
}

fn validate_name(name: &str) -> Result<(), CoreModelError> {
    let characters = name.chars().count();
    if name.trim().is_empty() || characters > 120 {
        return Err(CoreModelError::InvalidName);
    }
    Ok(())
}

fn validate_directory(directory: &std::path::Path) -> Result<(), CoreModelError> {
    if directory.as_os_str().is_empty() {
        return Err(CoreModelError::InvalidDirectory);
    }
    Ok(())
}

fn not_found(kind: ResourceKind, id: u64) -> CoreModelError {
    CoreModelError::NotFound { kind, id }
}

fn find_tab_location(
    spaces: &[SpaceSnapshot],
    tab_id: TabId,
) -> Result<(usize, usize), CoreModelError> {
    spaces
        .iter()
        .enumerate()
        .find_map(|(space_index, space)| {
            space
                .tabs
                .iter()
                .position(|tab| tab.id == tab_id)
                .map(|tab_index| (space_index, tab_index))
        })
        .ok_or_else(|| not_found(ResourceKind::Tab, tab_id.as_u64()))
}

fn find_tab_mut(
    spaces: &mut [SpaceSnapshot],
    tab_id: TabId,
) -> Result<&mut TabSnapshot, CoreModelError> {
    spaces
        .iter_mut()
        .flat_map(|space| &mut space.tabs)
        .find(|tab| tab.id == tab_id)
        .ok_or_else(|| not_found(ResourceKind::Tab, tab_id.as_u64()))
}

fn find_space_for_tab(spaces: &[SpaceSnapshot], tab_id: TabId) -> Option<&SpaceSnapshot> {
    spaces
        .iter()
        .find(|space| space.tabs.iter().any(|tab| tab.id == tab_id))
}

fn find_pane_location(
    spaces: &[SpaceSnapshot],
    pane_id: PaneId,
) -> Result<(SpaceId, TabId), CoreModelError> {
    spaces
        .iter()
        .find_map(|space| {
            space
                .tabs
                .iter()
                .find(|tab| layout_contains_pane(&tab.layout, pane_id))
                .map(|tab| (space.id, tab.id))
        })
        .ok_or_else(|| not_found(ResourceKind::Pane, pane_id.as_u64()))
}

fn layout_contains_pane(layout: &PaneLayout, pane_id: PaneId) -> bool {
    match layout {
        PaneLayout::Pane(pane) => pane.id == pane_id,
        PaneLayout::Split(split) => {
            layout_contains_pane(&split.first, pane_id)
                || layout_contains_pane(&split.second, pane_id)
        }
    }
}

fn replace_pane_with_split(
    layout: &mut PaneLayout,
    pane_id: PaneId,
    split_id: SplitId,
    axis: SplitAxis,
    ratio: SplitRatio,
    placement: SplitPlacement,
    new_pane: PaneSnapshot,
) -> bool {
    match layout {
        PaneLayout::Pane(pane) if pane.id == pane_id => {
            let existing = pane.clone();
            let (first, second) = match placement {
                SplitPlacement::Before => (new_pane, existing),
                SplitPlacement::After => (existing, new_pane),
            };
            *layout = PaneLayout::Split(SplitSnapshot {
                id: split_id,
                axis,
                ratio,
                first: Box::new(PaneLayout::Pane(first)),
                second: Box::new(PaneLayout::Pane(second)),
            });
            true
        }
        PaneLayout::Pane(_) => false,
        PaneLayout::Split(split) => {
            if replace_pane_with_split(
                &mut split.first,
                pane_id,
                split_id,
                axis,
                ratio,
                placement,
                new_pane.clone(),
            ) {
                true
            } else {
                replace_pane_with_split(
                    &mut split.second,
                    pane_id,
                    split_id,
                    axis,
                    ratio,
                    placement,
                    new_pane,
                )
            }
        }
    }
}

enum RemovePane {
    NotFound(PaneLayout),
    Removed {
        remaining: Option<PaneLayout>,
        pane: PaneSnapshot,
    },
}

fn remove_pane(layout: PaneLayout, pane_id: PaneId) -> RemovePane {
    match layout {
        PaneLayout::Pane(pane) if pane.id == pane_id => RemovePane::Removed {
            remaining: None,
            pane,
        },
        PaneLayout::Pane(pane) => RemovePane::NotFound(PaneLayout::Pane(pane)),
        PaneLayout::Split(mut split) => {
            let first = *split.first;
            match remove_pane(first, pane_id) {
                RemovePane::Removed { remaining, pane } => {
                    let second = *split.second;
                    let remaining = match remaining {
                        None => Some(second),
                        Some(first) => {
                            split.first = Box::new(first);
                            split.second = Box::new(second);
                            Some(PaneLayout::Split(split))
                        }
                    };
                    RemovePane::Removed { remaining, pane }
                }
                RemovePane::NotFound(first) => {
                    let second = *split.second;
                    match remove_pane(second, pane_id) {
                        RemovePane::Removed { remaining, pane } => {
                            let remaining = match remaining {
                                None => Some(first),
                                Some(second) => {
                                    split.first = Box::new(first);
                                    split.second = Box::new(second);
                                    Some(PaneLayout::Split(split))
                                }
                            };
                            RemovePane::Removed { remaining, pane }
                        }
                        RemovePane::NotFound(second) => {
                            split.first = Box::new(first);
                            split.second = Box::new(second);
                            RemovePane::NotFound(PaneLayout::Split(split))
                        }
                    }
                }
            }
        }
    }
}

fn detach_pane_from_tab(
    spaces: &mut [SpaceSnapshot],
    tab_id: TabId,
    pane_id: PaneId,
) -> Result<(PaneSnapshot, bool), CoreModelError> {
    let tab = find_tab_mut(spaces, tab_id)?;
    match remove_pane(tab.layout.clone(), pane_id) {
        RemovePane::NotFound(_) => Err(not_found(ResourceKind::Pane, pane_id.as_u64())),
        RemovePane::Removed { remaining, pane } => {
            let empty = remaining.is_none();
            if let Some(remaining) = remaining {
                tab.layout = remaining;
            }
            Ok((pane, empty))
        }
    }
}

fn remove_tab_and_empty_space(spaces: &mut Vec<SpaceSnapshot>, tab_id: TabId) {
    let Some(space_index) = spaces
        .iter()
        .position(|space| space.tabs.iter().any(|tab| tab.id == tab_id))
    else {
        return;
    };
    let tab_index = spaces[space_index]
        .tabs
        .iter()
        .position(|tab| tab.id == tab_id)
        .expect("located Space contains the Tab");
    spaces[space_index].tabs.remove(tab_index);
    if spaces[space_index].tabs.is_empty() {
        spaces.remove(space_index);
    }
}

fn find_split_mut(layout: &mut PaneLayout, split_id: SplitId) -> Option<&mut SplitSnapshot> {
    match layout {
        PaneLayout::Pane(_) => None,
        PaneLayout::Split(split) => {
            if split.id == split_id {
                return Some(split);
            }
            if let Some(found) = find_split_mut(&mut split.first, split_id) {
                return Some(found);
            }
            find_split_mut(&mut split.second, split_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creating_a_space_commits_one_complete_initial_hierarchy() {
        let mut model = CoreModel::new();

        let commit = model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Project".into(),
                    directory: "/work/project".into(),
                },
            )
            .expect("create Space");

        let CreatedResource::Space {
            space_id,
            tab_id,
            pane_id,
            terminal_session_id,
        } = commit.created
        else {
            panic!("Space creation must identify every created resource");
        };
        assert_eq!(commit.revision, 1);
        assert_eq!(commit.snapshot.spaces[0].id, space_id);
        assert_eq!(commit.snapshot.spaces[0].tabs[0].id, tab_id);
        assert_eq!(
            commit.snapshot.spaces[0].tabs[0].layout,
            PaneLayout::Pane(PaneSnapshot {
                id: pane_id,
                terminal_session_id,
            })
        );
        assert_eq!(
            commit.effects,
            vec![CoreEffect::LaunchTerminal {
                terminal_session_id,
                launch: TerminalLaunch::shell("/work/project"),
            }]
        );
    }

    #[test]
    fn rejected_commands_change_neither_state_nor_id_allocation() {
        let mut rejected = CoreModel::new();
        let before = rejected.snapshot();
        assert_eq!(
            rejected
                .apply(
                    1,
                    CoreCommand::CreateSpace {
                        name: "Project".into(),
                        directory: "/work/project".into(),
                    },
                )
                .expect_err("stale command must fail"),
            CoreModelError::StaleRevision {
                expected: 1,
                actual: 0,
            }
        );
        assert_eq!(rejected.snapshot(), before);

        let rejected_commit = rejected
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Project".into(),
                    directory: "/work/project".into(),
                },
            )
            .expect("create after rejection");
        let mut fresh = CoreModel::new();
        let fresh_commit = fresh
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Project".into(),
                    directory: "/work/project".into(),
                },
            )
            .expect("create in fresh model");
        assert_eq!(rejected_commit.created, fresh_commit.created);
    }

    #[test]
    fn splitting_and_moving_a_pane_preserves_its_terminal_session() {
        let mut model = model_with_space();
        let first_pane = pane_ids(&model.snapshot())[0];
        let split = model
            .apply(
                1,
                CoreCommand::SplitPane {
                    pane_id: first_pane,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("split Pane");
        let CreatedResource::Pane {
            pane_id: moved_pane,
            terminal_session_id,
            ..
        } = split.created
        else {
            panic!("split must create a Pane");
        };
        let tab = model
            .apply(
                2,
                CoreCommand::CreateTab {
                    space_id: model.snapshot().spaces[0].id,
                    name: "Second".into(),
                },
            )
            .expect("create destination Tab");
        let CreatedResource::Tab {
            pane_id: target_pane,
            ..
        } = tab.created
        else {
            panic!("Tab creation must create its initial Pane");
        };

        let moved = model
            .apply(
                3,
                CoreCommand::MovePane {
                    pane_id: moved_pane,
                    target_pane_id: target_pane,
                    axis: SplitAxis::Vertical,
                    placement: SplitPlacement::Before,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("move Pane");

        assert!(moved.effects.is_empty());
        assert_eq!(
            session_for_pane(&moved.snapshot, moved_pane),
            terminal_session_id
        );
        assert_eq!(pane_ids(&moved.snapshot).len(), 3);
        assert_eq!(
            moved
                .snapshot
                .terminal_sessions
                .iter()
                .filter(|session| session.id == terminal_session_id)
                .count(),
            1
        );
    }

    #[test]
    fn close_collapses_the_split_and_removes_the_terminal_session() {
        let mut model = model_with_space();
        let original = pane_ids(&model.snapshot())[0];
        let split = model
            .apply(
                1,
                CoreCommand::SplitPane {
                    pane_id: original,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("split Pane");
        let CreatedResource::Pane {
            pane_id,
            terminal_session_id,
            ..
        } = split.created
        else {
            panic!("split must create a Pane");
        };

        let closed = model
            .apply(2, CoreCommand::ClosePane { pane_id })
            .expect("close Pane");

        assert_eq!(pane_ids(&closed.snapshot), vec![original]);
        assert!(
            closed
                .snapshot
                .terminal_sessions
                .iter()
                .all(|session| session.id != terminal_session_id)
        );
        assert_eq!(
            closed.effects,
            vec![CoreEffect::StopTerminal {
                terminal_session_id,
            }]
        );
    }

    #[test]
    fn moving_the_only_pane_out_of_a_space_removes_the_empty_space() {
        let mut model = model_with_space();
        let source_space = model.snapshot().spaces[0].id;
        let moved_pane = pane_ids(&model.snapshot())[0];
        let moved_session = session_for_pane(&model.snapshot(), moved_pane);
        let destination = model
            .apply(
                1,
                CoreCommand::CreateSpace {
                    name: "Destination".into(),
                    directory: "/work/destination".into(),
                },
            )
            .expect("create destination Space");
        let CreatedResource::Space {
            pane_id: target_pane,
            ..
        } = destination.created
        else {
            panic!("Space creation must return its initial Pane");
        };

        let moved = model
            .apply(
                2,
                CoreCommand::MovePane {
                    pane_id: moved_pane,
                    target_pane_id: target_pane,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("move final Pane out of source Space");

        assert!(
            moved
                .snapshot
                .spaces
                .iter()
                .all(|space| space.id != source_space)
        );
        assert_eq!(session_for_pane(&moved.snapshot, moved_pane), moved_session);
        assert!(moved.effects.is_empty());
    }

    #[test]
    fn closing_the_last_pane_removes_its_empty_tab_and_space() {
        let mut model = model_with_space();
        let pane_id = pane_ids(&model.snapshot())[0];

        let closed = model
            .apply(1, CoreCommand::ClosePane { pane_id })
            .expect("close final Pane");

        assert!(closed.snapshot.spaces.is_empty());
        assert!(closed.snapshot.terminal_sessions.is_empty());
    }

    #[test]
    fn tab_order_names_split_ratio_and_restore_disposition_are_revisioned() {
        let mut model = model_with_space();
        let space_id = model.snapshot().spaces[0].id;
        let original_tab = model.snapshot().spaces[0].tabs[0].id;
        let original_pane = pane_ids(&model.snapshot())[0];
        let original_session = session_for_pane(&model.snapshot(), original_pane);
        let second = model
            .apply(
                1,
                CoreCommand::CreateTab {
                    space_id,
                    name: "Second".into(),
                },
            )
            .expect("create second Tab");
        let CreatedResource::Tab { tab_id, .. } = second.created else {
            panic!("Tab creation must return its ID");
        };
        model
            .apply(2, CoreCommand::ReorderTab { tab_id, index: 0 })
            .expect("reorder Tab");
        model
            .apply(
                3,
                CoreCommand::RenameTab {
                    tab_id: original_tab,
                    name: "Shell".into(),
                },
            )
            .expect("rename Tab");
        let split = model
            .apply(
                4,
                CoreCommand::SplitPane {
                    pane_id: original_pane,
                    axis: SplitAxis::Vertical,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("split Pane");
        let CreatedResource::Pane { split_id, .. } = split.created else {
            panic!("split must return its ID");
        };
        let ratio = SplitRatio::new(650).expect("valid ratio");
        model
            .apply(5, CoreCommand::ResizeSplit { split_id, ratio })
            .expect("resize split");
        let final_commit = model
            .apply(
                6,
                CoreCommand::SetRestoreDisposition {
                    terminal_session_id: original_session,
                    disposition: RestoreDisposition::RemainEnded,
                },
            )
            .expect("change Restore Disposition");

        assert_eq!(final_commit.revision, 7);
        assert_eq!(final_commit.snapshot.spaces[0].tabs[0].id, tab_id);
        assert_eq!(find_tab(&final_commit.snapshot, original_tab).name, "Shell");
        assert_eq!(
            find_split_snapshot(&final_commit.snapshot, split_id).ratio,
            ratio
        );
        assert_eq!(
            final_commit
                .snapshot
                .terminal_sessions
                .iter()
                .find(|session| session.id == original_session)
                .expect("session remains present")
                .launch
                .restore_disposition,
            RestoreDisposition::RemainEnded
        );
    }

    #[test]
    fn invalid_move_and_ratio_are_structured_rejections() {
        assert_eq!(
            SplitRatio::new(99).expect_err("too-small ratio must fail"),
            CoreModelError::InvalidSplitRatio(99)
        );
        let mut model = model_with_space();
        let pane_id = pane_ids(&model.snapshot())[0];
        let before = model.snapshot();
        assert_eq!(
            model
                .apply(
                    1,
                    CoreCommand::MovePane {
                        pane_id,
                        target_pane_id: pane_id,
                        axis: SplitAxis::Horizontal,
                        placement: SplitPlacement::After,
                        ratio: SplitRatio::EQUAL,
                    },
                )
                .expect_err("self move must fail"),
            CoreModelError::CannotMovePaneOntoItself
        );
        assert_eq!(model.snapshot(), before);
    }

    #[test]
    fn cold_restore_preserves_structure_but_allocates_new_terminal_sessions() {
        let mut model = model_with_space();
        let initial = model.snapshot();
        let first_terminal = initial.terminal_sessions[0].id;
        let split = model
            .apply(
                initial.revision,
                CoreCommand::SplitPane {
                    pane_id: pane_ids(&initial)[0],
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::new(650).expect("valid ratio"),
                },
            )
            .expect("split Pane");
        model
            .apply(
                split.revision,
                CoreCommand::SetRestoreDisposition {
                    terminal_session_id: first_terminal,
                    disposition: RestoreDisposition::RemainEnded,
                },
            )
            .expect("mark first Terminal Session ended");
        let persisted = model.persisted_layout().expect("capture layout");
        let old_terminal_ids = model
            .snapshot()
            .terminal_sessions
            .into_iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();

        let (restored, effects) =
            CoreModel::restore_layout(persisted.clone()).expect("restore layout");
        let restored_snapshot = restored.snapshot();
        let new_terminal_ids = restored_snapshot
            .terminal_sessions
            .iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();

        let restored_layout = restored.persisted_layout().unwrap();
        assert_eq!(restored_layout.spaces, persisted.spaces);
        assert!(restored_layout.next_id > persisted.next_id);
        assert!(old_terminal_ids.is_disjoint(&new_terminal_ids));
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(effect, CoreEffect::LaunchTerminal { .. }))
                .count(),
            1
        );
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(effect, CoreEffect::RestoreEndedTerminal { .. }))
                .count(),
            1
        );
    }

    fn model_with_space() -> CoreModel {
        let mut model = CoreModel::new();
        model
            .apply(
                0,
                CoreCommand::CreateSpace {
                    name: "Project".into(),
                    directory: "/work/project".into(),
                },
            )
            .expect("create test Space");
        model
    }

    fn pane_ids(snapshot: &CoreSnapshot) -> Vec<PaneId> {
        let mut panes = Vec::new();
        for tab in snapshot.spaces.iter().flat_map(|space| &space.tabs) {
            collect_panes(&tab.layout, &mut panes);
        }
        panes
    }

    fn collect_panes(layout: &PaneLayout, panes: &mut Vec<PaneId>) {
        match layout {
            PaneLayout::Pane(pane) => panes.push(pane.id),
            PaneLayout::Split(split) => {
                collect_panes(&split.first, panes);
                collect_panes(&split.second, panes);
            }
        }
    }

    fn session_for_pane(snapshot: &CoreSnapshot, pane_id: PaneId) -> TerminalSessionId {
        snapshot
            .spaces
            .iter()
            .flat_map(|space| &space.tabs)
            .find_map(|tab| find_pane(&tab.layout, pane_id))
            .expect("Pane exists")
            .terminal_session_id
    }

    fn find_pane(layout: &PaneLayout, pane_id: PaneId) -> Option<&PaneSnapshot> {
        match layout {
            PaneLayout::Pane(pane) if pane.id == pane_id => Some(pane),
            PaneLayout::Pane(_) => None,
            PaneLayout::Split(split) => {
                find_pane(&split.first, pane_id).or_else(|| find_pane(&split.second, pane_id))
            }
        }
    }

    fn find_tab(snapshot: &CoreSnapshot, tab_id: TabId) -> &TabSnapshot {
        snapshot
            .spaces
            .iter()
            .flat_map(|space| &space.tabs)
            .find(|tab| tab.id == tab_id)
            .expect("Tab exists")
    }

    fn find_split_snapshot(snapshot: &CoreSnapshot, split_id: SplitId) -> &SplitSnapshot {
        snapshot
            .spaces
            .iter()
            .flat_map(|space| &space.tabs)
            .find_map(|tab| find_split_in_layout(&tab.layout, split_id))
            .expect("split exists")
    }

    fn find_split_in_layout(layout: &PaneLayout, split_id: SplitId) -> Option<&SplitSnapshot> {
        match layout {
            PaneLayout::Pane(_) => None,
            PaneLayout::Split(split) if split.id == split_id => Some(split),
            PaneLayout::Split(split) => find_split_in_layout(&split.first, split_id)
                .or_else(|| find_split_in_layout(&split.second, split_id)),
        }
    }
}
