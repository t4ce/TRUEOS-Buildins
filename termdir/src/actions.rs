use std::{io, path::PathBuf};

use crossterm::style::Color;

use crate::{
    graph_view::{GraphView, Sha256Result, Viewport},
    layout::LayoutMode,
    screen::{Frame, Style},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    Reload,
    Parent,
    TreeLayout,
    RadialLayout,
    ToggleLayout,
    Spacing,
    Depth,
    LineStyle,
    Zoom,
    Center,
    Enter,
    Edit,
    Show,
    Play,
    Infer,
    Sha256,
    Zip,
    Rename,
    NewFolder,
    NewFile,
    Delete,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuContext {
    None,
    BlobFile,
    File,
    LocalTextFile,
    ImageFile,
    VideoFile,
    Files,
    ImageFiles,
    Folder,
    ImageFolder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuSection {
    Mount,
    View,
    Action,
    Clip,
}

impl MenuSection {
    const ORDER: [Self; 4] = [Self::Mount, Self::View, Self::Action, Self::Clip];

    fn stepped(self, reverse: bool) -> Self {
        let current = Self::ORDER
            .iter()
            .position(|section| *section == self)
            .unwrap_or(0);
        let next = if reverse {
            (current + Self::ORDER.len() - 1) % Self::ORDER.len()
        } else {
            (current + 1) % Self::ORDER.len()
        };
        Self::ORDER[next]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MenuEntry {
    pub label: &'static str,
    pub command: MenuCommand,
    pub section: MenuSection,
}

// The visual order is the contract from ☰enu_Rework.txt. Local hotkeys are
// assigned after contextual filtering, so every section reuses 🯰..🯹/0..9.
pub const MENU_ENTRIES: [MenuEntry; 18] = [
    MenuEntry {
        label: "center",
        command: MenuCommand::Center,
        section: MenuSection::View,
    },
    MenuEntry {
        label: "zoom",
        command: MenuCommand::Zoom,
        section: MenuSection::View,
    },
    MenuEntry {
        label: "depth",
        command: MenuCommand::Depth,
        section: MenuSection::View,
    },
    MenuEntry {
        label: "space",
        command: MenuCommand::Spacing,
        section: MenuSection::View,
    },
    MenuEntry {
        label: "line",
        command: MenuCommand::LineStyle,
        section: MenuSection::View,
    },
    MenuEntry {
        label: "layout",
        command: MenuCommand::ToggleLayout,
        section: MenuSection::View,
    },
    MenuEntry {
        label: "edit",
        command: MenuCommand::Edit,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "enter",
        command: MenuCommand::Enter,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "show",
        command: MenuCommand::Show,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "ply",
        command: MenuCommand::Play,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "new 🖿",
        command: MenuCommand::NewFolder,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "new 🖹",
        command: MenuCommand::NewFile,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "name",
        command: MenuCommand::Rename,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "del ␡",
        command: MenuCommand::Delete,
        section: MenuSection::Action,
    },
    MenuEntry { label: "infer", command: MenuCommand::Infer, section: MenuSection::Action },
    MenuEntry {
        label: "sha ＃",
        command: MenuCommand::Sha256,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "LZ4 🗜",
        command: MenuCommand::Zip,
        section: MenuSection::Action,
    },
    MenuEntry {
        label: "esc ␛",
        command: MenuCommand::Exit,
        section: MenuSection::Action,
    },
];

#[derive(Clone, Debug)]
pub struct MenuLink {
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug)]
pub struct MountLink {
    pub path: PathBuf,
    pub label: String,
    pub primary: bool,
    pub read_only: bool,
}

impl MountLink {
    pub fn menu_label(&self) -> String {
        let marker = match (self.primary, self.read_only) {
            (true, true) => "★◇",
            (true, false) => "★",
            (false, true) => "◇",
            (false, false) => "",
        };
        format!("{marker}{}", self.label)
    }
}

impl MenuLink {
    pub fn label(&self) -> String {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MenuState {
    pub cursor: usize,
    pub mount_cursor: usize,
    pub clip_cursor: usize,
    pub section: MenuSection,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            cursor: MenuState::index_for_command(MenuCommand::Center).unwrap_or(0),
            mount_cursor: 0,
            clip_cursor: 0,
            section: MenuSection::View,
        }
    }
}

impl MenuState {
    pub fn set_cursor(&mut self, index: usize, context: MenuContext) -> bool {
        if index >= MENU_ENTRIES.len() || !Self::is_visible(index, context) {
            return false;
        }
        let next_section = MENU_ENTRIES[index].section;
        let changed = self.cursor != index || self.section != next_section;
        self.cursor = index;
        self.section = next_section;
        changed
    }

    pub fn set_clip_cursor(&mut self, index: usize, link_count: usize) -> bool {
        if link_count == 0 || index >= link_count.min(10) {
            return false;
        }
        let changed = self.section != MenuSection::Clip || self.clip_cursor != index;
        self.section = MenuSection::Clip;
        self.clip_cursor = index;
        changed
    }

    pub fn set_mount_cursor(&mut self, index: usize, mount_count: usize) -> bool {
        if index >= mount_count.min(10) {
            return false;
        }
        let changed = self.section != MenuSection::Mount || self.mount_cursor != index;
        self.section = MenuSection::Mount;
        self.mount_cursor = index;
        changed
    }

    pub fn ensure_visible(&mut self, context: MenuContext) -> bool {
        if matches!(self.section, MenuSection::Mount | MenuSection::Clip) {
            return false;
        }
        if Self::is_visible(self.cursor, context)
            && MENU_ENTRIES[self.cursor].section == self.section
        {
            return false;
        }

        let next = Self::first_visible_in_section(self.section, context)
            .or_else(|| {
                let mut section = self.section;
                for _ in 0..3 {
                    section = section.stepped(false);
                    if section == MenuSection::Clip {
                        continue;
                    }
                    if let Some(index) = Self::first_visible_in_section(section, context) {
                        return Some(index);
                    }
                }
                None
            })
            .unwrap_or(0);
        let changed = self.cursor != next || self.section != MENU_ENTRIES[next].section;
        self.cursor = next;
        self.section = MENU_ENTRIES[next].section;
        changed
    }

    pub fn cycle_section(&mut self, reverse: bool, context: MenuContext) -> bool {
        let section = self.section.stepped(reverse);
        if matches!(section, MenuSection::Mount | MenuSection::Clip) {
            let changed = self.section != section;
            self.section = section;
            return changed;
        }
        let Some(index) = Self::first_visible_in_section(section, context) else {
            return false;
        };
        let changed = self.section != section || self.cursor != index;
        self.section = section;
        self.cursor = index;
        changed
    }

    pub fn set_section(&mut self, section: MenuSection, context: MenuContext) -> bool {
        if matches!(section, MenuSection::Mount | MenuSection::Clip) {
            let changed = self.section != section;
            self.section = section;
            return changed;
        }
        let Some(index) = Self::first_visible_in_section(section, context) else {
            return false;
        };
        let changed = self.section != section || self.cursor != index;
        self.section = section;
        self.cursor = index;
        changed
    }

    pub fn section_for_header_row(
        row: u16,
        bottom: u16,
        context: MenuContext,
        mount_count: usize,
        link_count: usize,
    ) -> Option<MenuSection> {
        if row == 2 {
            Some(MenuSection::Mount)
        } else if row == Self::view_header_row(mount_count) {
            Some(MenuSection::View)
        } else if row == Self::action_header_row(mount_count) {
            Some(MenuSection::Action)
        } else if Self::clip_header_row(bottom, context, mount_count, link_count) == Some(row) {
            Some(MenuSection::Clip)
        } else {
            None
        }
    }

    pub fn index_for_hotkey(&self, key: char, context: MenuContext) -> Option<usize> {
        if matches!(self.section, MenuSection::Mount | MenuSection::Clip) {
            return None;
        }
        let local = key.to_digit(10)? as usize;
        MENU_ENTRIES
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                entry.section == self.section && Self::is_visible(*index, context)
            })
            .nth(local)
            .map(|(index, _)| index)
    }

    pub fn clip_index_for_hotkey(
        &self,
        key: char,
        link_count: usize,
        visible_count: usize,
    ) -> Option<usize> {
        if self.section != MenuSection::Clip {
            return None;
        }
        let index = key.to_digit(10)? as usize;
        (index < link_count.min(10).min(visible_count)).then_some(index)
    }

    pub fn index_for_command(command: MenuCommand) -> Option<usize> {
        MENU_ENTRIES
            .iter()
            .position(|entry| entry.command == command)
    }

    pub fn is_visible(index: usize, context: MenuContext) -> bool {
        let Some(entry) = MENU_ENTRIES.get(index) else {
            return false;
        };
        match entry.command {
            MenuCommand::Reload
            | MenuCommand::Center
            | MenuCommand::Spacing
            | MenuCommand::Depth
            | MenuCommand::LineStyle
            | MenuCommand::Zoom
            | MenuCommand::ToggleLayout
            | MenuCommand::Exit => true,
            // A digest describes one byte stream. Multi-file selection leaves
            // it unavailable rather than implying a made-up combined hash.
            MenuCommand::Sha256 => matches!(
                context,
                MenuContext::BlobFile | MenuContext::File | MenuContext::LocalTextFile | MenuContext::ImageFile | MenuContext::VideoFile
            ),
            MenuCommand::Infer => context == MenuContext::BlobFile,
            MenuCommand::Zip => context != MenuContext::None,
            MenuCommand::Enter => matches!(context, MenuContext::Folder | MenuContext::ImageFolder),
            MenuCommand::Show => matches!(
                context,
                MenuContext::ImageFile | MenuContext::ImageFiles | MenuContext::ImageFolder
            ),
            MenuCommand::Play => context == MenuContext::VideoFile,
            MenuCommand::Edit => context == MenuContext::LocalTextFile,
            MenuCommand::NewFolder | MenuCommand::NewFile => {
                matches!(
                    context,
                    MenuContext::None | MenuContext::Folder | MenuContext::ImageFolder
                )
            }
            MenuCommand::Delete => {
                matches!(
                    context,
                    MenuContext::BlobFile | MenuContext::File
                        | MenuContext::LocalTextFile
                        | MenuContext::ImageFile
                        | MenuContext::VideoFile
                        | MenuContext::Files
                        | MenuContext::ImageFiles
                        | MenuContext::Folder
                        | MenuContext::ImageFolder
                )
            }
            MenuCommand::Rename => matches!(
                context,
                MenuContext::BlobFile | MenuContext::File
                    | MenuContext::LocalTextFile
                    | MenuContext::ImageFile
                    | MenuContext::VideoFile
                    | MenuContext::Folder
                    | MenuContext::ImageFolder
            ),
            MenuCommand::Parent | MenuCommand::TreeLayout | MenuCommand::RadialLayout => false,
        }
    }

    pub fn visible_mount_count(mount_count: usize) -> usize {
        mount_count.min(10)
    }

    pub fn view_header_row(mount_count: usize) -> u16 {
        4 + Self::visible_mount_count(mount_count) as u16
    }

    pub fn action_header_row(mount_count: usize) -> u16 {
        Self::view_header_row(mount_count) + 8
    }

    pub fn mount_index_for_row(row: u16, mount_count: usize) -> Option<usize> {
        let index = row.checked_sub(3)? as usize;
        (index < Self::visible_mount_count(mount_count)).then_some(index)
    }

    pub fn row_for_index(index: usize, context: MenuContext, mount_count: usize) -> Option<u16> {
        let entry = MENU_ENTRIES.get(index)?;
        if !Self::is_visible(index, context) {
            return None;
        }
        let start = match entry.section {
            MenuSection::Mount => return None,
            MenuSection::View => Self::view_header_row(mount_count) + 1,
            MenuSection::Action => Self::action_header_row(mount_count) + 1,
            MenuSection::Clip => return None,
        };
        let offset = MENU_ENTRIES[..index]
            .iter()
            .enumerate()
            .filter(|(prior, candidate)| {
                candidate.section == entry.section && Self::is_visible(*prior, context)
            })
            .count() as u16;
        Some(start + offset)
    }

    pub fn index_for_row(row: u16, context: MenuContext, mount_count: usize) -> Option<usize> {
        (0..MENU_ENTRIES.len())
            .find(|&index| Self::row_for_index(index, context, mount_count) == Some(row))
    }

    pub fn local_index(index: usize, context: MenuContext) -> Option<usize> {
        let entry = MENU_ENTRIES.get(index)?;
        if !Self::is_visible(index, context) {
            return None;
        }
        Some(
            MENU_ENTRIES[..index]
                .iter()
                .enumerate()
                .filter(|(prior, candidate)| {
                    candidate.section == entry.section && Self::is_visible(*prior, context)
                })
                .count(),
        )
    }

    fn first_visible_in_section(section: MenuSection, context: MenuContext) -> Option<usize> {
        MENU_ENTRIES
            .iter()
            .enumerate()
            .find(|(index, entry)| entry.section == section && Self::is_visible(*index, context))
            .map(|(index, _)| index)
    }

    pub fn action_count(context: MenuContext) -> usize {
        MENU_ENTRIES
            .iter()
            .enumerate()
            .filter(|(index, entry)| {
                entry.section == MenuSection::Action && Self::is_visible(*index, context)
            })
            .count()
    }

    pub fn stats_header_row(context: MenuContext, mount_count: usize) -> Option<u16> {
        if matches!(
            context,
            MenuContext::None | MenuContext::Files | MenuContext::ImageFiles
        ) {
            None
        } else {
            Some(Self::action_header_row(mount_count) + 2 + Self::action_count(context) as u16)
        }
    }

    pub fn fixed_content_bottom(context: MenuContext, mount_count: usize) -> u16 {
        match Self::stats_header_row(context, mount_count) {
            // Stats header + five ordinary values + up to ten wrapped
            // typed-identity rows + one breathing row.
            Some(header) => header + 16,
            // Action list + one breathing row.
            None => Self::action_header_row(mount_count) + 1 + Self::action_count(context) as u16,
        }
    }

    pub fn clip_visible_count(
        bottom: u16,
        context: MenuContext,
        mount_count: usize,
        link_count: usize,
    ) -> usize {
        let fixed = Self::fixed_content_bottom(context, mount_count);
        let capacity = bottom.saturating_sub(fixed.saturating_add(2)) as usize;
        if capacity == 0 {
            return 0;
        }
        let desired = if link_count == 0 {
            1
        } else {
            link_count.min(10)
        };
        desired.min(capacity)
    }

    pub fn clip_header_row(
        bottom: u16,
        context: MenuContext,
        mount_count: usize,
        link_count: usize,
    ) -> Option<u16> {
        let visible = Self::clip_visible_count(bottom, context, mount_count, link_count);
        if visible == 0 {
            None
        } else {
            Some(bottom.saturating_sub(visible as u16).saturating_sub(1))
        }
    }
}

#[derive(Clone, Debug)]
pub enum PendingAction {
    Infer { path: PathBuf, bytes: Vec<u8>, content_type: trueos::content_identity::ContentTypeId, digest: [u8; 32] },
    Move {
        source: usize,
        target: usize,
    },
    MoveNodes {
        sources: Vec<usize>,
        target: usize,
    },
    MoveToPath {
        source: usize,
        target: PathBuf,
    },
    MoveNodesToPath {
        sources: Vec<usize>,
        target: PathBuf,
    },
    MovePath {
        source: PathBuf,
        target: PathBuf,
    },
    MovePaths { sources: Vec<PathBuf>, target: PathBuf },
    Trash {
        source: usize,
    },
    TrashNodes {
        sources: Vec<usize>,
    },
    NewFolder {
        parent: usize,
    },
    NewFile {
        parent: usize,
    },
    Rename {
        source: usize,
    },
    ExtractArchive {
        source: usize,
    },
}

#[derive(Clone, Debug)]
pub enum ModalMode {
    Confirm,
    Input {
        value: String,
        accept_label: &'static str,
    },
}

#[derive(Clone, Debug)]
pub struct Modal {
    pub pending: PendingAction,
    pub title: String,
    pub lines: Vec<String>,
    pub mode: ModalMode,
    pub trash_origin_y: Option<u16>,
}

impl Modal {
    pub fn move_node(graph: &GraphView, source: usize, target: usize) -> Self {
        Self {
            pending: PendingAction::Move { source, target },
            title: "Confirm".to_string(),
            lines: vec![
                format!("Move {}", graph.label(source)),
                format!("into {} ?", graph.label(target)),
            ],
            mode: ModalMode::Confirm,
            trash_origin_y: None,
        }
    }

    pub fn move_to_path(graph: &GraphView, source: usize, target: PathBuf) -> Self {
        Self {
            pending: PendingAction::MoveToPath {
                source,
                target: target.clone(),
            },
            title: "Confirm".to_string(),
            lines: vec![
                format!("Move {}", graph.label(source)),
                format!("into {} ?", target.display()),
            ],
            mode: ModalMode::Confirm,
            trash_origin_y: None,
        }
    }

    pub fn move_nodes(graph: &GraphView, sources: Vec<usize>, target: usize) -> Self {
        let count = sources.len();
        Self {
            pending: PendingAction::MoveNodes { sources, target },
            title: "Confirm".to_string(),
            lines: vec![
                format!("Move {count} files"),
                format!("into {} ?", graph.label(target)),
            ],
            mode: ModalMode::Confirm,
            trash_origin_y: None,
        }
    }

    pub fn move_nodes_to_path(sources: Vec<usize>, target: PathBuf) -> Self {
        let count = sources.len();
        Self {
            pending: PendingAction::MoveNodesToPath {
                sources,
                target: target.clone(),
            },
            title: "Confirm".to_string(),
            lines: vec![
                format!("Move {count} files"),
                format!("into {} ?", target.display()),
            ],
            mode: ModalMode::Confirm,
            trash_origin_y: None,
        }
    }

    pub fn move_path(source: PathBuf, target: PathBuf) -> Self {
        let label = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| source.display().to_string());
        Self {
            pending: PendingAction::MovePath {
                source,
                target: target.clone(),
            },
            title: "Confirm".to_string(),
            lines: vec![
                format!("Move {label}"),
                format!("into {} ?", target.display()),
            ],
            mode: ModalMode::Confirm,
            trash_origin_y: None,
        }
    }

    pub fn move_paths(sources: Vec<PathBuf>, target: PathBuf) -> Self {
        if sources.len() == 1 {
            return Self::move_path(sources[0].clone(), target);
        }
        let count = sources.len();
        Self {
            pending: PendingAction::MovePaths {
                sources,
                target: target.clone(),
            },
            title: "Confirm".to_string(),
            lines: vec![
                format!("Move {count} entities"),
                format!("into {} ?", target.display()),
            ],
            mode: ModalMode::Confirm,
            trash_origin_y: None,
        }
    }

    pub fn trash_node(graph: &GraphView, source: usize) -> Self {
        Self {
            pending: PendingAction::Trash { source },
            title: "Confirm".to_string(),
            lines: vec![
                format!("Move {} to recycle area?", graph.label(source)),
                "Stored in .explorer-trash/".to_string(),
            ],
            mode: ModalMode::Confirm,
            trash_origin_y: None,
        }
    }

    pub fn trash_nodes(sources: Vec<usize>) -> Self {
        let count = sources.len();
        Self {
            pending: PendingAction::TrashNodes { sources },
            title: "Confirm".to_string(),
            lines: vec![
                format!("Move {count} files to recycle?"),
                "Stored in .explorer-trash/".to_string(),
            ],
            mode: ModalMode::Confirm,
            trash_origin_y: None,
        }
    }

    pub fn new_folder(graph: &GraphView, parent: usize) -> Self {
        let parent_name = if parent == 0 {
            graph.root_label()
        } else {
            graph.label(parent)
        };
        Self {
            pending: PendingAction::NewFolder { parent },
            title: "New folder".to_string(),
            lines: vec![format!("Parent: {parent_name}"), "Folder name:".to_string()],
            mode: ModalMode::Input {
                value: "New Folder".to_string(),
                accept_label: "create",
            },
            trash_origin_y: None,
        }
    }

    pub fn new_file(graph: &GraphView, parent: usize) -> Self {
        let parent_name = if parent == 0 {
            graph.root_label()
        } else {
            graph.label(parent)
        };
        Self {
            pending: PendingAction::NewFile { parent },
            title: "New file".to_string(),
            lines: vec![format!("Parent: {parent_name}"), "File name:".to_string()],
            mode: ModalMode::Input {
                value: "New File".to_string(),
                accept_label: "create",
            },
            trash_origin_y: None,
        }
    }

    pub fn extract_archive(source: usize, default_folder: String) -> Self {
        Self {
            pending: PendingAction::ExtractArchive { source },
            title: "Extract archive".to_owned(),
            lines: vec!["Destination folder:".to_owned()],
            mode: ModalMode::Input {
                value: default_folder,
                accept_label: "extract",
            },
            trash_origin_y: None,
        }
    }

    pub fn rename(graph: &GraphView, source: usize) -> Self {
        let current = graph
            .node(source)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        Self {
            pending: PendingAction::Rename { source },
            title: "Name".to_string(),
            lines: vec![format!("Current: {current}"), "New name:".to_string()],
            mode: ModalMode::Input {
                value: current,
                accept_label: "rename",
            },
            trash_origin_y: None,
        }
    }

    pub fn with_origin_y(mut self, y: u16) -> Self {
        self.trash_origin_y = Some(y);
        self
    }

    pub fn is_input(&self) -> bool {
        matches!(self.mode, ModalMode::Input { .. })
    }

    pub fn input_value(&self) -> Option<&str> {
        match &self.mode {
            ModalMode::Input { value, .. } => Some(value.as_str()),
            ModalMode::Confirm => None,
        }
    }

    pub fn push_char(&mut self, ch: char) -> bool {
        let ModalMode::Input { value, .. } = &mut self.mode else {
            return false;
        };
        if ch.is_control() || ch == '/' || ch == '\\' {
            return false;
        }
        if value.chars().count() >= 96 {
            return false;
        }
        value.push(ch);
        true
    }

    pub fn backspace(&mut self) -> bool {
        let ModalMode::Input { value, .. } = &mut self.mode else {
            return false;
        };
        value.pop().is_some()
    }
}

pub enum Dispatch {
    Exit,
    Zoom,
    Show(Vec<String>),
    Play(String),
    Edit(PathBuf),
    Modal(Modal),
    Sha256(Sha256Result),
    Status(String),
}

pub struct ActionOutcome {
    pub status: String,
    pub trashed_label: Option<String>,
}

pub fn dispatch_menu(
    command: MenuCommand,
    graph: &mut GraphView,
    selected: Option<usize>,
) -> io::Result<Dispatch> {
    dispatch_menu_selected(command, graph, selected, &[])
}

pub fn dispatch_menu_selected(
    command: MenuCommand,
    graph: &mut GraphView,
    selected: Option<usize>,
    selected_files: &[usize],
) -> io::Result<Dispatch> {
    let selected = single_selected(selected, selected_files);
    match command {
        MenuCommand::Reload => {
            graph.reload()?;
            Ok(Dispatch::Status(format!("MOUNT · {}", graph.root_label())))
        }
        MenuCommand::Parent => {
            if graph.mount_parent()? {
                Ok(Dispatch::Status(format!("MOUNT · {}", graph.root_label())))
            } else {
                Ok(Dispatch::Status(
                    "MOUNT · already at filesystem root".to_string(),
                ))
            }
        }
        MenuCommand::TreeLayout => {
            graph.set_layout_mode(LayoutMode::Tree);
            Ok(Dispatch::Status("VIEW · tree layout".to_string()))
        }
        MenuCommand::RadialLayout => {
            graph.set_layout_mode(LayoutMode::Radial);
            Ok(Dispatch::Status("VIEW · radial layout".to_string()))
        }
        MenuCommand::ToggleLayout => {
            let mode = graph.toggle_layout_mode();
            let label = match mode {
                LayoutMode::Tree => "tree",
                LayoutMode::Radial => "radial",
            };
            Ok(Dispatch::Status(format!("VIEW · {label} layout")))
        }
        MenuCommand::Spacing => {
            let gap = graph.cycle_spacing();
            Ok(Dispatch::Status(format!("VIEW · spacing {gap}")))
        }
        MenuCommand::Depth => {
            let depth = graph.cycle_depth()?;
            Ok(Dispatch::Status(format!("VIEW · depth {depth}")))
        }
        MenuCommand::LineStyle => {
            let style = graph.cycle_line_style();
            Ok(Dispatch::Status(format!("VIEW · line {}", style.label())))
        }
        MenuCommand::Center => {
            graph.center();
            Ok(Dispatch::Status("VIEW · centered".to_string()))
        }
        MenuCommand::Zoom => Ok(Dispatch::Zoom),
        MenuCommand::Enter => match selected {
            Some(source) if graph.node(source).map(|n| n.is_dir).unwrap_or(false) => {
                graph.mount_node(source)?;
                Ok(Dispatch::Status(format!("MOUNT · {}", graph.root_label())))
            }
            _ => Ok(Dispatch::Status(
                "ENTER · select a folder first".to_string(),
            )),
        },
        MenuCommand::Show => match graph.image_paths_for_selection(selected, selected_files) {
            Ok(paths) => Ok(Dispatch::Show(paths)),
            Err(error) => Ok(Dispatch::Status(format!("SHOW FAILED · {error}"))),
        },
        MenuCommand::Play => match selected.and_then(|id| graph.video_path_for_node(id).ok()) {
            Some(path) => Ok(Dispatch::Play(path)),
            None => Ok(Dispatch::Status(
                "PLY FAILED · select one inferred MP4 or MKV file".to_string(),
            )),
        },
        MenuCommand::Edit => match selected.and_then(|id| graph.node(id)) {
            Some(node) if crate::graph_view::is_editable_text_file(node) => Ok(Dispatch::Edit(node.path.clone())),
            _ => Ok(Dispatch::Status("EDIT · select one .txt file".to_string())),
        },
        MenuCommand::Sha256 => match selected {
            Some(source) => match graph.sha256_node(source) {
                Ok(result) => Ok(Dispatch::Sha256(result)),
                Err(err) => Ok(Dispatch::Status(format!("SHA256 FAILED · {err}"))),
            },
            None => Ok(Dispatch::Status(
                "SHA256 · select one file first".to_string(),
            )),
        },
        MenuCommand::Zip => match selected.or_else(|| {
            selected_files
                .first()
                .copied()
                .filter(|_| selected_files.len() == 1)
        }) {
            Some(source) => {
                if let Some(folder) = graph.archive_default_folder(source) {
                    Ok(Dispatch::Modal(Modal::extract_archive(source, folder)))
                } else {
                    Ok(Dispatch::Status(match graph.archive_node(source) {
                        Ok(status) => status,
                        Err(err) => format!("ARCHIVE FAILED · {err}"),
                    }))
                }
            }
            None if !selected_files.is_empty() => Ok(Dispatch::Status(
                match graph.archive_nodes(selected_files) {
                    Ok(status) => status,
                    Err(err) => format!("ARCHIVE FAILED · {err}"),
                },
            )),
            None => Ok(Dispatch::Status(
                "ARCHIVE · select one file or folder first".to_string(),
            )),
        },
        MenuCommand::Rename => match selected {
            Some(source) => Ok(Dispatch::Modal(Modal::rename(graph, source))),
            None => Ok(Dispatch::Status(
                "NAME · select a file or folder first".to_string(),
            )),
        },
        MenuCommand::Infer => {
            let Some(source) = single_selected(selected, selected_files) else {
                return Ok(Dispatch::Status("INFER · select one Blob file".into()));
            };
            match graph.preview_infer(source)? {
                Some((path, bytes, content_type, digest)) => {
                    let info = trueos::content_identity::content_type_info(content_type).unwrap();
                    Ok(Dispatch::Modal(Modal {
                        title: "Apply inferred type?".into(),
                        lines: vec![format!("{}", path.display()), format!("Blob → {} ({})", info.canonical_name, info.mime_type), "Known extension and file bytes agree. Apply this type?".into()],
                        pending: PendingAction::Infer { path, bytes, content_type, digest },
                        mode: ModalMode::Confirm, trash_origin_y: None,
                    }))
                }
                None => Ok(Dispatch::Status("INFER · no matching known extension and byte classification; file unchanged".into())),
            }
        }
        MenuCommand::NewFolder => {
            let parent = selected
                .filter(|id| graph.node(*id).map(|n| n.is_dir).unwrap_or(false))
                .unwrap_or(0);
            Ok(Dispatch::Modal(Modal::new_folder(graph, parent)))
        }
        MenuCommand::NewFile => {
            let parent = selected
                .filter(|id| graph.node(*id).map(|n| n.is_dir).unwrap_or(false))
                .unwrap_or(0);
            Ok(Dispatch::Modal(Modal::new_file(graph, parent)))
        }
        MenuCommand::Delete if selected_files.len() > 1 => {
            Ok(Dispatch::Modal(Modal::trash_nodes(selected_files.to_vec())))
        }
        MenuCommand::Delete => match selected {
            None => Ok(Dispatch::Status(
                "DELETE · select one file/folder first".to_string(),
            )),
            Some(source) => Ok(Dispatch::Modal(Modal::trash_node(graph, source))),
        },
        MenuCommand::Exit => Ok(Dispatch::Exit),
    }
}

fn single_selected(selected: Option<usize>, selected_files: &[usize]) -> Option<usize> {
    match selected_files {
        [] => selected,
        [source] => Some(*source),
        _ => None,
    }
}

pub fn execute_modal(modal: Modal, graph: &mut GraphView) -> io::Result<ActionOutcome> {
    let pending = modal.pending.clone();
    match pending {
        PendingAction::Infer { path, bytes, content_type, digest } => {
            graph.apply_infer(&path, &bytes, content_type, digest)?;
            Ok(ActionOutcome { status: "INFER · type applied".into(), trashed_label: None })
        }
        PendingAction::Move { source, target } => {
            let status = graph.move_node(source, target)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
        PendingAction::MoveNodes { sources, target } => {
            let status = graph.move_nodes(sources.as_slice(), target)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
        PendingAction::MoveToPath { source, target } => {
            let status = graph.move_node_to_path(source, &target)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
        PendingAction::MoveNodesToPath { sources, target } => {
            let status = graph.move_nodes_to_path(sources.as_slice(), &target)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
        PendingAction::MovePath { source, target } => {
            let status = graph.move_path(&source, &target)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
        PendingAction::MovePaths { sources, target } => {
            let status = graph.move_paths(&sources, &target)?;
            Ok(ActionOutcome { status, trashed_label: None })
        }
        PendingAction::Trash { source } => {
            let label = graph.label(source);
            let status = graph.trash_node(source)?;
            Ok(ActionOutcome {
                status,
                trashed_label: Some(label),
            })
        }
        PendingAction::TrashNodes { sources } => {
            let status = graph.trash_nodes(sources.as_slice())?;
            Ok(ActionOutcome {
                status,
                trashed_label: Some(format!("{} files", sources.len())),
            })
        }
        PendingAction::NewFolder { parent } => {
            let name = modal.input_value().unwrap_or("").trim().to_string();
            let status = graph.create_folder(parent, &name)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
        PendingAction::NewFile { parent } => {
            let name = modal.input_value().unwrap_or("").trim().to_string();
            let status = graph.create_file(parent, &name)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
        PendingAction::ExtractArchive { source } => {
            let name = modal.input_value().unwrap_or("").trim();
            let status = graph.extract_archive_node(source, name)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
        PendingAction::Rename { source } => {
            let name = modal.input_value().unwrap_or("").trim().to_string();
            let status = graph.rename_node(source, &name)?;
            Ok(ActionOutcome {
                status,
                trashed_label: None,
            })
        }
    }
}

#[derive(Clone, Copy)]
struct ModalGeometry {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    yes_x0: u16,
    yes_x1: u16,
    no_x0: u16,
    no_x1: u16,
    button_y: u16,
}

pub fn modal_click(viewport: Viewport, x: u16, y: u16) -> Option<bool> {
    let g = modal_geometry(viewport);
    if y != g.button_y {
        return None;
    }
    if x >= g.yes_x0 && x <= g.yes_x1 {
        Some(true)
    } else if x >= g.no_x0 && x <= g.no_x1 {
        Some(false)
    } else {
        None
    }
}

pub fn draw_modal(frame: &mut Frame, modal: &Modal, viewport: Viewport) {
    let g = modal_geometry(viewport);
    let inner = g.width.saturating_sub(2) as usize;
    let modal_style = Style::new(Color::Black, Color::Grey);
    let button_style = Style::new(Color::Black, Color::White);

    frame.fill_rect(g.x, g.y, g.width, g.height, ' ', modal_style);

    let title = clip_text(&modal.title, inner.saturating_sub(10));
    let notch = format!("╗ {title} ╔");
    let top_fill = inner.saturating_sub(notch.chars().count());
    let left_fill = top_fill / 2;
    let right_fill = top_fill - left_fill;
    let top = format!(
        "╒{}{}{}╕",
        "═".repeat(left_fill),
        notch,
        "═".repeat(right_fill)
    );
    print_at(
        frame,
        g.x,
        g.y,
        &fit_exact(&top, g.width as usize),
        modal_style,
    );

    let notch_width = notch.chars().count().max(4);
    let cap = format!("╚{}╝", "═".repeat(notch_width.saturating_sub(2)));
    let cap_space = inner.saturating_sub(cap.chars().count());
    let cap_left = cap_space / 2;
    let cap_right = cap_space - cap_left;
    let second = format!("│{}{}{}│", " ".repeat(cap_left), cap, " ".repeat(cap_right));
    print_at(
        frame,
        g.x,
        g.y + 1,
        &fit_exact(&second, g.width as usize),
        modal_style,
    );

    let mut content = vec![String::new(), String::new(), String::new()];
    for (i, line) in modal.lines.iter().take(3).enumerate() {
        content[i] = line.clone();
    }
    if let ModalMode::Input { value, .. } = &modal.mode {
        content[2] = format!("> {value}▏");
    }
    for (i, line) in content.iter().enumerate() {
        let clipped = clip_text_tail(line, inner.saturating_sub(2));
        let body = format!(" {:<width$} ", clipped, width = inner.saturating_sub(2));
        print_at(
            frame,
            g.x,
            g.y + 2 + i as u16,
            &fit_exact(&format!("│{body}│"), g.width as usize),
            modal_style,
        );
    }

    let left_half = (inner - 1) / 2;
    let right_half = inner - 1 - left_half;
    let divider = format!("├{}┬{}┤", "╌".repeat(left_half), "╌".repeat(right_half));
    print_at(
        frame,
        g.x,
        g.y + 5,
        &fit_exact(&divider, g.width as usize),
        modal_style,
    );

    let (yes_label, no_label) = match &modal.mode {
        ModalMode::Confirm => ("⫸ yes (Y)".to_string(), "⫸ no (N)".to_string()),
        ModalMode::Input { accept_label, .. } => {
            (format!("⫸ {accept_label}"), "⫸ cancel (esc)".to_string())
        }
    };
    let buttons = format!(
        "╞{}╪{}╡",
        fill_center(left_half, &yes_label, '═'),
        fill_center(right_half, &no_label, '═')
    );
    print_at(
        frame,
        g.x,
        g.button_y,
        &fit_exact(&buttons, g.width as usize),
        button_style,
    );

    let bottom = format!("╘{}╧{}╛", "═".repeat(left_half), "═".repeat(right_half));
    print_at(
        frame,
        g.x,
        g.y + 7,
        &fit_exact(&bottom, g.width as usize),
        modal_style,
    );
}

fn modal_geometry(viewport: Viewport) -> ModalGeometry {
    let width = viewport.width.saturating_sub(4).min(46).max(24);
    let height = 8;
    let x = viewport.x + viewport.width.saturating_sub(width) / 2;
    let y = viewport.y + viewport.height.saturating_sub(height) / 2;
    let inner = width.saturating_sub(2);
    let left_half = (inner - 1) / 2;
    ModalGeometry {
        x,
        y,
        width,
        height,
        yes_x0: x + 1,
        yes_x1: x + left_half,
        no_x0: x + left_half + 2,
        no_x1: x + width - 2,
        button_y: y + 6,
    }
}

fn fill_center(width: usize, label: &str, fill: char) -> String {
    if width == 0 {
        return String::new();
    }
    let visible = clip_text(label, width);
    let used = visible.chars().count();
    let pad = width.saturating_sub(used);
    let left = pad / 2;
    let right = pad - left;
    format!(
        "{}{}{}",
        fill.to_string().repeat(left),
        visible,
        fill.to_string().repeat(right)
    )
}

fn fit_exact(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count == width {
        return text.to_string();
    }
    if count > width {
        return text.chars().take(width).collect();
    }
    format!("{}{}", text, " ".repeat(width - count))
}

fn menu_text(frame: &mut Frame, x: u16, y: u16, text: &str) {
    print_at(frame, x, y, text, Style::default());
}

fn clip_text(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    if max <= 3 {
        return text.chars().take(max).collect();
    }
    let mut clipped: String = text.chars().take(max - 3).collect();
    clipped.push_str("...");
    clipped
}

fn clip_text_tail(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }
    let tail: String = text
        .chars()
        .rev()
        .take(max - 1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

fn print_at(frame: &mut Frame, x: u16, y: u16, text: &str, style: Style) {
    frame.put_str(x, y, text, style);
}

#[cfg(test)]
mod tests {
    use super::{MenuCommand, MenuContext, MenuState, Modal, PendingAction};

    #[test]
    fn infer_is_only_visible_for_one_blob_file() {
        let infer = MenuState::index_for_command(MenuCommand::Infer).unwrap();
        assert!(MenuState::is_visible(infer, MenuContext::BlobFile));
        for context in [MenuContext::None, MenuContext::File, MenuContext::LocalTextFile,
            MenuContext::ImageFile, MenuContext::VideoFile, MenuContext::Files,
            MenuContext::ImageFiles, MenuContext::Folder, MenuContext::ImageFolder] {
            assert!(!MenuState::is_visible(infer, context));
        }
    }

    #[test]
    fn edit_is_the_first_action_only_for_one_local_txt_file() {
        let edit = MenuState::index_for_command(MenuCommand::Edit).unwrap();
        assert_eq!(MenuState::local_index(edit, MenuContext::LocalTextFile), Some(0));
        let row = MenuState::row_for_index(edit, MenuContext::LocalTextFile, 1).unwrap();
        assert_eq!(MenuState::index_for_row(row, MenuContext::LocalTextFile, 1), Some(edit));
        for context in [MenuContext::None, MenuContext::File, MenuContext::Files,
            MenuContext::ImageFile, MenuContext::ImageFiles, MenuContext::VideoFile,
            MenuContext::Folder, MenuContext::ImageFolder] {
            assert!(!MenuState::is_visible(edit, context));
        }
        for command in [MenuCommand::Rename, MenuCommand::Delete, MenuCommand::Sha256, MenuCommand::Zip] {
            assert!(MenuState::is_visible(MenuState::index_for_command(command).unwrap(), MenuContext::LocalTextFile));
        }
    }

    #[test]
    fn extraction_modal_prefills_an_editable_folder_name() {
        let modal = Modal::extract_archive(7, "assets".into());
        assert!(modal.is_input());
        assert_eq!(modal.input_value(), Some("assets"));
        assert!(matches!(
            modal.pending,
            PendingAction::ExtractArchive { source: 7 }
        ));
    }

    #[test]
    fn name_action_is_visible_and_clickable_for_files_and_folders() {
        let index = MenuState::index_for_command(MenuCommand::Rename).unwrap();
        for context in [MenuContext::File, MenuContext::Folder] {
            assert!(MenuState::is_visible(index, context));
            let row = MenuState::row_for_index(index, context, 1).unwrap();
            assert_eq!(MenuState::index_for_row(row, context, 1), Some(index));
        }
        assert!(!MenuState::is_visible(index, MenuContext::None));
    }

    #[test]
    fn file_group_exposes_only_group_safe_actions() {
        let delete = MenuState::index_for_command(MenuCommand::Delete).unwrap();
        let archive = MenuState::index_for_command(MenuCommand::Zip).unwrap();
        let sha = MenuState::index_for_command(MenuCommand::Sha256).unwrap();
        let rename = MenuState::index_for_command(MenuCommand::Rename).unwrap();

        assert!(MenuState::is_visible(delete, MenuContext::Files));
        assert!(MenuState::is_visible(archive, MenuContext::Files));
        assert!(!MenuState::is_visible(sha, MenuContext::Files));
        assert!(!MenuState::is_visible(rename, MenuContext::Files));
        assert_eq!(MenuState::stats_header_row(MenuContext::Files, 1), None);
    }

    #[test]
    fn show_is_only_visible_for_typed_image_selections() {
        let show = MenuState::index_for_command(MenuCommand::Show).unwrap();
        for context in [
            MenuContext::ImageFile,
            MenuContext::ImageFiles,
            MenuContext::ImageFolder,
        ] {
            assert!(MenuState::is_visible(show, context));
        }
        for context in [
            MenuContext::None,
            MenuContext::File,
            MenuContext::Files,
            MenuContext::Folder,
        ] {
            assert!(!MenuState::is_visible(show, context));
        }
    }

    #[test]
    fn play_is_only_visible_for_one_video_file() {
        let play = MenuState::index_for_command(MenuCommand::Play).unwrap();
        for context in [
            MenuContext::None,
            MenuContext::File,
            MenuContext::Files,
            MenuContext::ImageFile,
            MenuContext::ImageFiles,
            MenuContext::Folder,
            MenuContext::ImageFolder,
        ] {
            assert!(!MenuState::is_visible(play, context));
        }
        assert!(MenuState::is_visible(play, MenuContext::VideoFile));
    }

    #[test]
    fn group_recycle_confirmation_names_the_file_count() {
        let modal = Modal::trash_nodes(vec![4, 9, 16]);
        assert_eq!(modal.lines[0], "Move 3 files to recycle?");
        assert!(matches!(modal.pending, PendingAction::TrashNodes { .. }));
    }
}
