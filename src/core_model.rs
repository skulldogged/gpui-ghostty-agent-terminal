use std::{
    fmt,
    path::{Path, PathBuf},
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLaunch {
    pub working_directory: PathBuf,
}

impl TerminalLaunch {
    pub fn shell(working_directory: impl Into<PathBuf>) -> Self {
        Self {
            working_directory: working_directory.into(),
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
    pub name_is_custom: bool,
    pub directory: PathBuf,
    pub tabs: Vec<TabSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabSnapshot {
    pub id: TabId,
    pub name: String,
    pub name_is_custom: bool,
    pub layout: PaneLayout,
}

pub(crate) fn default_space_name(directory: &Path) -> String {
    directory
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .unwrap_or(directory)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Terminal")
        .to_owned()
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
    CloseTab {
        tab_id: TabId,
    },
    CloseSpace {
        space_id: SpaceId,
    },
    ClosePane {
        pane_id: PaneId,
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
        Self::with_id_namespace(1)
    }

    pub(crate) fn with_id_namespace(first_id: u64) -> Self {
        assert_ne!(first_id, 0, "Core resource IDs must be nonzero");
        Self {
            snapshot: CoreSnapshot {
                revision: 0,
                spaces: Vec::new(),
                terminal_sessions: Vec::new(),
            },
            next_id: first_id,
        }
    }

    pub fn snapshot(&self) -> CoreSnapshot {
        self.snapshot.clone()
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
                    name_is_custom: false,
                    directory,
                    tabs: vec![TabSnapshot {
                        id: tab_id,
                        name: "Terminal".into(),
                        name_is_custom: false,
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
                space.name_is_custom = true;
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
                        name_is_custom: false,
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
                let tab = find_tab_mut(&mut self.snapshot.spaces, tab_id)?;
                tab.name = name;
                tab.name_is_custom = true;
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
            CoreCommand::CloseTab { tab_id } => {
                let (space_index, tab_index) = find_tab_location(&self.snapshot.spaces, tab_id)?;
                let tab = self.snapshot.spaces[space_index].tabs.remove(tab_index);
                let mut terminal_session_ids = Vec::new();
                collect_terminal_session_ids(&tab.layout, &mut terminal_session_ids);
                if self.snapshot.spaces[space_index].tabs.is_empty() {
                    self.snapshot.spaces.remove(space_index);
                }
                self.snapshot
                    .terminal_sessions
                    .retain(|session| !terminal_session_ids.contains(&session.id));
                let effects = terminal_session_ids
                    .into_iter()
                    .map(|terminal_session_id| CoreEffect::StopTerminal {
                        terminal_session_id,
                    })
                    .collect();
                Ok((effects, CreatedResource::None))
            }
            CoreCommand::CloseSpace { space_id } => {
                let space_index = self
                    .snapshot
                    .spaces
                    .iter()
                    .position(|space| space.id == space_id)
                    .ok_or_else(|| not_found(ResourceKind::Space, space_id.as_u64()))?;
                let space = self.snapshot.spaces.remove(space_index);
                let mut terminal_session_ids = Vec::new();
                for tab in &space.tabs {
                    collect_terminal_session_ids(&tab.layout, &mut terminal_session_ids);
                }
                self.snapshot
                    .terminal_sessions
                    .retain(|session| !terminal_session_ids.contains(&session.id));
                let effects = terminal_session_ids
                    .into_iter()
                    .map(|terminal_session_id| CoreEffect::StopTerminal {
                        terminal_session_id,
                    })
                    .collect();
                Ok((effects, CreatedResource::None))
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

fn collect_terminal_session_ids(
    layout: &PaneLayout,
    terminal_session_ids: &mut Vec<TerminalSessionId>,
) {
    match layout {
        PaneLayout::Pane(pane) => terminal_session_ids.push(pane.terminal_session_id),
        PaneLayout::Split(split) => {
            collect_terminal_session_ids(&split.first, terminal_session_ids);
            collect_terminal_session_ids(&split.second, terminal_session_ids);
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
        assert!(!commit.snapshot.spaces[0].name_is_custom);
        assert_eq!(commit.snapshot.spaces[0].tabs[0].id, tab_id);
        assert!(!commit.snapshot.spaces[0].tabs[0].name_is_custom);
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
    fn closing_a_tab_stops_its_terminals_once_and_preserves_unrelated_resources() {
        let mut model = model_with_space();
        let source_space_id = model.snapshot().spaces[0].id;
        let tab_id = model.snapshot().spaces[0].tabs[0].id;
        let first_pane_id = pane_ids(&model.snapshot())[0];
        let first_terminal_session_id = session_for_pane(&model.snapshot(), first_pane_id);
        let split = model
            .apply(
                1,
                CoreCommand::SplitPane {
                    pane_id: first_pane_id,
                    axis: SplitAxis::Horizontal,
                    placement: SplitPlacement::After,
                    ratio: SplitRatio::EQUAL,
                },
            )
            .expect("split Tab before closing it");
        let CreatedResource::Pane {
            terminal_session_id: second_terminal_session_id,
            ..
        } = split.created
        else {
            panic!("split must identify its Terminal Session");
        };
        let sibling = model
            .apply(
                2,
                CoreCommand::CreateTab {
                    space_id: source_space_id,
                    name: "Sibling".into(),
                },
            )
            .expect("create sibling Tab");
        let CreatedResource::Tab {
            tab_id: sibling_tab_id,
            terminal_session_id: sibling_terminal_session_id,
            ..
        } = sibling.created
        else {
            panic!("Tab creation must identify its resources");
        };
        let unrelated = model
            .apply(
                3,
                CoreCommand::CreateSpace {
                    name: "Unrelated".into(),
                    directory: "/work/unrelated".into(),
                },
            )
            .expect("create unrelated Space");
        let CreatedResource::Space {
            space_id: unrelated_space_id,
            terminal_session_id: unrelated_terminal_session_id,
            ..
        } = unrelated.created
        else {
            panic!("Space creation must identify its resources");
        };

        let closed = model
            .apply(4, CoreCommand::CloseTab { tab_id })
            .expect("close Tab");

        assert_eq!(closed.revision, 5);
        assert_eq!(closed.snapshot.spaces.len(), 2);
        assert_eq!(closed.snapshot.spaces[0].id, source_space_id);
        assert_eq!(closed.snapshot.spaces[0].tabs.len(), 1);
        assert_eq!(closed.snapshot.spaces[0].tabs[0].id, sibling_tab_id);
        assert_eq!(closed.snapshot.spaces[1].id, unrelated_space_id);
        assert_eq!(
            closed
                .snapshot
                .terminal_sessions
                .iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![sibling_terminal_session_id, unrelated_terminal_session_id]
        );
        assert_eq!(
            closed.effects,
            vec![
                CoreEffect::StopTerminal {
                    terminal_session_id: first_terminal_session_id,
                },
                CoreEffect::StopTerminal {
                    terminal_session_id: second_terminal_session_id,
                },
            ]
        );
    }

    #[test]
    fn closing_an_unknown_tab_is_rejected_without_changing_the_hierarchy() {
        let mut model = model_with_space();
        let before = model.snapshot();

        assert_eq!(
            model
                .apply(
                    before.revision,
                    CoreCommand::CloseTab {
                        tab_id: TabId::from_u64(u64::MAX),
                    },
                )
                .expect_err("unknown Tab must be rejected"),
            CoreModelError::NotFound {
                kind: ResourceKind::Tab,
                id: u64::MAX,
            }
        );
        assert_eq!(model.snapshot(), before);
    }

    #[test]
    fn closing_a_space_stops_its_terminals_and_preserves_other_spaces() {
        let mut model = model_with_space();
        let closed_space_id = model.snapshot().spaces[0].id;
        let closed_terminal_session_id = model.snapshot().terminal_sessions[0].id;
        let unrelated = model
            .apply(
                1,
                CoreCommand::CreateSpace {
                    name: "Unrelated".into(),
                    directory: "/work/unrelated".into(),
                },
            )
            .expect("create unrelated Space");
        let CreatedResource::Space {
            space_id: unrelated_space_id,
            terminal_session_id: unrelated_terminal_session_id,
            ..
        } = unrelated.created
        else {
            panic!("Space creation must identify its resources");
        };

        let closed = model
            .apply(
                2,
                CoreCommand::CloseSpace {
                    space_id: closed_space_id,
                },
            )
            .expect("close Space");

        assert_eq!(closed.revision, 3);
        assert_eq!(closed.snapshot.spaces.len(), 1);
        assert_eq!(closed.snapshot.spaces[0].id, unrelated_space_id);
        assert_eq!(closed.snapshot.terminal_sessions.len(), 1);
        assert_eq!(
            closed.snapshot.terminal_sessions[0].id,
            unrelated_terminal_session_id
        );
        assert_eq!(
            closed.effects,
            vec![CoreEffect::StopTerminal {
                terminal_session_id: closed_terminal_session_id,
            }]
        );
    }

    #[test]
    fn closing_an_unknown_space_is_rejected_without_changing_the_hierarchy() {
        let mut model = model_with_space();
        let before = model.snapshot();

        assert_eq!(
            model
                .apply(
                    before.revision,
                    CoreCommand::CloseSpace {
                        space_id: SpaceId::from_u64(u64::MAX),
                    },
                )
                .expect_err("unknown Space must be rejected"),
            CoreModelError::NotFound {
                kind: ResourceKind::Space,
                id: u64::MAX,
            }
        );
        assert_eq!(model.snapshot(), before);
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
    fn tab_order_names_and_split_ratio_are_revisioned() {
        let mut model = model_with_space();
        let space_id = model.snapshot().spaces[0].id;
        let original_tab = model.snapshot().spaces[0].tabs[0].id;
        let original_pane = pane_ids(&model.snapshot())[0];
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
        let final_commit = model
            .apply(5, CoreCommand::ResizeSplit { split_id, ratio })
            .expect("resize split");

        assert_eq!(final_commit.revision, 6);
        assert_eq!(final_commit.snapshot.spaces[0].tabs[0].id, tab_id);
        let renamed_tab = find_tab(&final_commit.snapshot, original_tab);
        assert_eq!(renamed_tab.name, "Shell");
        assert!(renamed_tab.name_is_custom);
        assert_eq!(
            find_split_snapshot(&final_commit.snapshot, split_id).ratio,
            ratio
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
