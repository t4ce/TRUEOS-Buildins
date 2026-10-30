mod actions;
mod backend;
mod edit_action;
mod native_ui4;
mod chronos;
mod drag_preview;
mod graph_view;
mod layout;
mod menu_view;
mod minimap;
mod screen;

fn archive_log(message: std::fmt::Arguments<'_>) {
    // Host UI tests have no kernel log C ABI. Production records go through
    // log_os, never stdout (which belongs to the terminal renderer).
    #[cfg(not(test))]
    let _ = trueos::logl::log_record(trueos::logl::level::INFO, "apps", message);
    #[cfg(test)]
    let _ = message;
}

use std::{
    collections::VecDeque,
    env,
    io::{self, BufWriter, stdout},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use std::time::Instant;

use std::io::Write;
use backend::Backend;

use actions::{
    Dispatch, MENU_ENTRIES, MenuContext, MenuLink, MenuSection, MenuState, Modal, MountLink,
    PendingAction, dispatch_menu_selected, draw_modal, execute_modal, modal_click,
};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    style::{Color, ResetColor},
    terminal::{self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen},
};
use graph_view::{GraphView, SelectionStats, Sha256Result, Viewport};
use menu_view::{close_button_hit, draw_menu, link_index_for_row};
use minimap::Minimap;
use screen::{Frame, Renderer, Style, terminal_cell_width, text_cell_width};

const DROP_X: u16 = 0;
const DROP_HIT_WIDTH: u16 = 3; // virtual hit area: columns 0, 1, and 2
const FRAME_X: u16 = 2;
const CANVAS_X: u16 = FRAME_X + 1;
const MENU_WIDTH: u16 = 17;
const LOG_ROWS: u16 = 6;
const MIN_WIDTH: u16 = 60;
const MIN_CONTENT_HEIGHT: u16 = 20;
const TICK: Duration = Duration::from_millis(16);
const MAX_EVENT_BATCH: usize = 64;
const FRAME_OUTPUT_BUFFER_CAPACITY: usize = 64 * 1024;
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(70);
const BUILD_ID: &str = "termdir · td · v0.26";
const FALL_TICK: Duration = Duration::from_millis(90);
const ZOOM_LEVELS: [u16; 5] = [75, 100, 125, 150, 200];
const DEFAULT_ZOOM_STEP: usize = 1;
const RULE_TITLE_SEPARATOR: &str = "・・";
const BREADCRUMB_SEPARATOR: &str = "・";
const BREADCRUMB_ROOT_LABEL: &str = "root";

#[derive(Clone)]
struct Config {
    backend: Backend,
    diagnostics: bool,
    initial_path: Option<PathBuf>,
    initial_depth: Option<usize>,
}

impl Config {
    fn from_args() -> io::Result<Self> {
        let mut diagnostics = env::var("EXPLORER_DIAGNOSTICS")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false);

        let directives = launch_directives();
        if let Some(error) = directives.backend_error {
            return Err(io::Error::other(error));
        }
        let mut backend = directives.backend;
        let mut initial_path = directives.browse_path;
        let initial_depth = directives.depth;
        let mut backend_value = false;
        let mut browse_value = false;

        for arg in env::args().skip(1) {
            if backend_value {
                backend = Backend::parse(&arg).map_err(io::Error::other)?;
                backend_value = false;
                continue;
            }
            if browse_value {
                initial_path = Some(PathBuf::from(arg));
                browse_value = false;
                continue;
            }
            match arg.as_str() {
                "--diagnostics" | "--diagnostic" | "--diag" | "-d" => {
                    diagnostics = true;
                }
                "--no-diagnostics" | "--no-diagnostic" => {
                    diagnostics = false;
                }
                "--backend" => backend_value = true,
                _ if arg.starts_with("--backend=") => {
                    backend = Backend::parse(&arg["--backend=".len()..]).map_err(io::Error::other)?;
                }
                "--browse" | "--path" => browse_value = true,
                _ if arg.starts_with("--browse=") => {
                    initial_path = Some(PathBuf::from(&arg["--browse=".len()..]));
                }
                _ if !arg.starts_with('-') => initial_path = Some(PathBuf::from(arg)),
                _ => {}
            }
        }

        // A scoped explorer starts at the primary filesystem root, not its
        // per-app working directory. Explicit browse paths still take priority.
        if initial_path.is_none() && env::var("TRUEOS_FS_SCOPE").as_deref() == Ok("trueosfs") {
            initial_path = Some(PathBuf::from("/"));
        }

        if backend_value {
            return Err(io::Error::other("--backend requires ui4 or terminal"));
        }
        Ok(Self {
            backend,
            diagnostics,
            initial_path,
            initial_depth,
        })
    }
}

#[derive(Default)]
struct LaunchDirectives {
    backend: Backend,
    backend_error: Option<String>,
    browse_path: Option<PathBuf>,
    depth: Option<usize>,
}

fn launch_directives() -> LaunchDirectives {
    let Ok(bytes) = trueos::async_fs::block_on(trueos::async_fs::read_file(b"vFile:launch")) else {
        return LaunchDirectives::default();
    };
    let Ok(script) = String::from_utf8(bytes) else {
        return LaunchDirectives::default();
    };
    launch_directives_from_script(script.as_str())
}

fn launch_directives_from_script(script: &str) -> LaunchDirectives {
    let mut directives = LaunchDirectives::default();
    for line in script.lines().map(str::trim) {
        if line == "backend" || line.starts_with("backend ") {
            match Backend::parse(line.strip_prefix("backend").unwrap().trim()) {
                Ok(backend) => { directives.backend = backend; directives.backend_error = None; }
                Err(error) => directives.backend_error = Some(error),
            }
            continue;
        }
        let browse = line
            .strip_prefix("browse ")
            .map(str::trim)
            .or_else(|| (line == "browse/").then_some("/"));
        if let Some(path) = browse.filter(|path| !path.is_empty()) {
            directives.browse_path = Some(PathBuf::from(path));
            continue;
        }
        if let Some(depth) = line
            .strip_prefix("depth ")
            .map(str::trim)
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|depth| *depth <= 7)
        {
            directives.depth = Some(depth);
        }
    }
    directives
}

fn browse_path_from_script(script: &str) -> Option<PathBuf> {
    launch_directives_from_script(script).browse_path
}

#[cfg(test)]
mod launch_script_tests {
    use super::{
        BREADCRUMB_ROOT_LABEL, BREADCRUMB_SEPARATOR, Sha256Result, breadcrumb_layout,
        breadcrumb_path_at, browse_path_from_script, clip_text_head_cells,
        launch_directives_from_script, sha256_rule_title, sha256_title_for_selection,
        text_cell_width,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn native_chrome_backend_is_explicit_and_invalid_values_fail() {
        use super::Backend;
        assert_eq!(launch_directives_from_script("browse /\n").backend, Backend::Terminal);
        let native = launch_directives_from_script("backend ui4\nbrowse /\ndepth 2");
        assert_eq!(native.backend, Backend::Ui4);
        assert!(native.backend_error.is_none());
        let remote = launch_directives_from_script("backend terminal\n");
        assert_eq!(remote.backend, Backend::Terminal);
        assert!(launch_directives_from_script("backend typo\n").backend_error.is_some());
        assert!(launch_directives_from_script("backend\n").backend_error.is_some());
    }

    #[test]
    fn uses_the_last_nonempty_browse_directive() {
        assert_eq!(
            browse_path_from_script("fs-scope trueosfs\nbrowse /\nbrowse trueosfs:disc4/apps\n"),
            Some(PathBuf::from("trueosfs:disc4/apps"))
        );
    }

    #[test]
    fn ignores_unrelated_or_empty_directives() {
        assert_eq!(
            browse_path_from_script("fs-scope trueosfs\nbrowse   \n"),
            None
        );
    }

    #[test]
    fn parses_td_root_and_depth_defaults() {
        let directives = launch_directives_from_script("fs-scope trueosfs\nbrowse/\ndepth 2\n");
        assert_eq!(directives.browse_path, Some(PathBuf::from("/")));
        assert_eq!(directives.depth, Some(2));
    }

    #[test]
    fn breadcrumb_uses_middle_dots_without_legacy_slashes() {
        let layout = breadcrumb_layout(Path::new("/apps/termdir"), 0, 80).unwrap();
        let labels: Vec<&str> = layout
            .crumbs
            .iter()
            .map(|crumb| crumb.label.as_str())
            .collect();
        assert_eq!(labels, [BREADCRUMB_ROOT_LABEL, "apps", "termdir"]);

        let expected_width = labels
            .iter()
            .map(|label| text_cell_width(label))
            .sum::<usize>()
            + 2 * text_cell_width(BREADCRUMB_SEPARATOR);
        assert_eq!(layout.content_width as usize, expected_width);
        assert!(!labels.iter().any(|label| label.contains('/')));
    }

    #[test]
    fn middle_dot_breadcrumb_widths_drive_positions_and_hit_boundaries() {
        let layout = breadcrumb_layout(Path::new("/apps/termdir"), 0, 80).unwrap();
        let root = &layout.crumbs[0];
        let apps = &layout.crumbs[1];
        let termdir = &layout.crumbs[2];

        assert_eq!(text_cell_width(BREADCRUMB_SEPARATOR), 2);
        assert_eq!(apps.x, root.x + root.width + 2);
        assert_eq!(termdir.x, apps.x + apps.width + 2);

        assert_eq!(
            breadcrumb_path_at(&layout, root.x, false),
            Some(PathBuf::from("/"))
        );
        assert_eq!(
            breadcrumb_path_at(&layout, root.x + root.width - 1, false),
            Some(PathBuf::from("/"))
        );
        assert_eq!(
            breadcrumb_path_at(&layout, root.x + root.width, false),
            None
        );
        assert_eq!(
            breadcrumb_path_at(&layout, apps.x, false),
            Some(PathBuf::from("/apps"))
        );
        assert_eq!(breadcrumb_path_at(&layout, termdir.x, false), None);
        assert_eq!(
            breadcrumb_path_at(&layout, termdir.x + termdir.width - 1, true),
            Some(PathBuf::from("/apps/termdir"))
        );
    }

    #[test]
    fn sha256_bottom_title_keeps_digest_as_the_clipped_prefix() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let title = sha256_rule_title(digest, "selected-file.txt");

        assert_eq!(title, format!("{digest} ・ selected-file.txt"));
        assert_eq!(clip_text_head_cells(&title, 20), digest[..20].to_string());
        assert_eq!(clip_text_head_cells(&title, 80), title);
    }

    #[test]
    fn sha256_bottom_title_is_only_relevant_to_its_selected_node() {
        let result = Sha256Result {
            source: 7,
            digest: "a".repeat(64),
            path: PathBuf::from("/apps/selected-file.txt"),
        };

        assert_eq!(
            sha256_title_for_selection(
                Some((7, Path::new("/apps/selected-file.txt"), "selected-file.txt")),
                Some(&result)
            ),
            Some((format!("{} ・ selected-file.txt", result.digest), true))
        );
        assert_eq!(
            sha256_title_for_selection(
                Some((8, Path::new("/apps/other-file.txt"), "other-file.txt")),
                Some(&result)
            ),
            Some(("other-file.txt".to_string(), false))
        );
        assert_eq!(
            sha256_title_for_selection(
                Some((
                    7,
                    Path::new("/other/selected-file.txt"),
                    "selected-file.txt"
                )),
                Some(&result)
            ),
            Some(("selected-file.txt".to_string(), false))
        );
        assert_eq!(sha256_title_for_selection(None, Some(&result)), None);
        assert_eq!(
            sha256_title_for_selection(
                Some((7, Path::new("/apps/selected-file.txt"), "selected-file.txt")),
                None
            ),
            Some(("selected-file.txt".to_string(), false))
        );
    }
}

#[cfg(test)]
fn available_mounts() -> io::Result<Vec<MountLink>> { Ok(Vec::new()) }

#[cfg(not(test))]
fn available_mounts() -> io::Result<Vec<MountLink>> {
    let mounts = trueos::async_fs::block_on(trueos::async_fs::list_mounts()).map_err(|error| {
        io::Error::other(format!(
            "list_mounts failed ({error}); TRUEOSFS scope required"
        ))
    })?;
    Ok(mounts
        .into_iter()
        .map(|mount| MountLink {
            path: PathBuf::from(mount.selector),
            label: mount.label,
            primary: mount.primary,
            read_only: mount.read_only,
        })
        .collect())
}

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;

        if let Err(err) = execute!(
            stdout(),
            EnterAlternateScreen,
            DisableLineWrap,
            EnableMouseCapture,
            Hide
        ) {
            let _ = execute!(
                stdout(),
                ResetColor,
                Show,
                DisableMouseCapture,
                EnableLineWrap,
                LeaveAlternateScreen
            );
            let _ = terminal::disable_raw_mode();
            return Err(err);
        }

        Ok(Self { active: true })
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }

        let screen_result = execute!(
            stdout(),
            ResetColor,
            Show,
            DisableMouseCapture,
            EnableLineWrap,
            LeaveAlternateScreen
        );
        let raw_result = terminal::disable_raw_mode();
        self.active = false;
        screen_result.and(raw_result)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[derive(Clone, Copy)]
struct Press {
    node: usize,
    start_x: u16,
    start_y: u16,
}

#[derive(Clone, PartialEq, Eq)]
struct DragState {
    sources: Vec<usize>,
    x: u16,
    y: u16,
    target: Option<usize>,
    over_trash: bool,
    over_menu: bool,
}

#[derive(Clone, Copy)]
struct ClipPress {
    index: usize,
    start_x: u16,
    start_y: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ClipDragState {
    index: usize,
    x: u16,
    y: u16,
    target: Option<PathBuf>,
    over_clip: bool,
}

#[derive(Clone, Debug)]
struct PendingClipMove {
    source: PathBuf,
    target: PathBuf,
}

#[derive(Clone, Copy)]
struct PanState {
    start_x: u16,
    start_y: u16,
    camera_x: i32,
    camera_y: i32,
}

struct FallingGhost {
    icon: &'static str,
    y: u16,
    next_step: Instant,
}

struct App {
    native_extent: Option<(u32, u32)>,
    local_ui4: bool,
    graph: GraphView,
    selected: Option<usize>,
    /// A toggle-selected set of regular files that all share one immediate
    /// parent. Folder selections continue to use `selected` alone.
    selected_files: Vec<usize>,
    selected_stats: Option<SelectionStats>,
    selected_sha256: Option<Sha256Result>,
    show_paths: Vec<String>,
    path_hover: Option<PathBuf>,
    item_hover: Option<usize>,
    press: Option<Press>,
    drag: Option<DragState>,
    drag_preview: Option<drag_preview::Preview>,
    clip_press: Option<ClipPress>,
    clip_drag: Option<ClipDragState>,
    pending_clip_move: Option<PendingClipMove>,
    pan: Option<PanState>,
    minimap_nav: bool,
    menu: MenuState,
    minimap: Minimap,
    minimap_visible: bool,
    modal: Option<Modal>,
    action_status: Option<String>,
    logs: VecDeque<String>,
    falling: Option<FallingGhost>,
    links: Vec<MenuLink>,
    mounts: Vec<MountLink>,
    zoom_step: usize,
    diagnostics: bool,
    resize_deadline: Option<Instant>,
    should_exit: bool,
    should_shutdown: bool,
    dirty: bool,
}

impl App {
    fn new(config: Config) -> io::Result<Self> {
        let mut graph = if config.backend == Backend::Ui4 {
            GraphView::empty(config.initial_path.as_deref().unwrap_or(Path::new("/")))
        } else {
            match config.initial_path.as_deref() {
                Some(path) => GraphView::from_path(path)?,
                None => GraphView::from_current_dir()?,
            }
        };
        if let Some(depth) = config.initial_depth {
            graph.set_depth_limit(depth)?;
        }
        let mut logs = VecDeque::new();
        logs.push_back(format!("⇝ {BUILD_ID}"));
        logs.push_back(format!("⇝ Mounted {}", graph.root_label()));
        let mounts = match available_mounts() {
            Ok(mounts) => {
                logs.push_back(format!("⇝ {} filesystem root mount(s)", mounts.len()));
                mounts
            }
            Err(error) => {
                logs.push_back(format!("MOUNT FAILED · {error}"));
                Vec::new()
            }
        };
        logs.push_back("⇝ LMB select/drag · MMB/WASD/arrows pan · Home center".to_string());
        logs.push_back("⇝ Tab changes menu segment · 0..9 runs local segment item".to_string());
        logs.push_back("⇝ Esc hides TUI · tui reopens · Ctrl-Q exits app".to_string());
        Ok(Self {
            native_extent: None,
            local_ui4: false,
            graph,
            selected: None,
            selected_files: Vec::new(),
            selected_stats: None,
            selected_sha256: None,
            show_paths: Vec::new(),
            path_hover: None,
            item_hover: None,
            press: None,
            drag: None,
            drag_preview: None,
            clip_press: None,
            clip_drag: None,
            pending_clip_move: None,
            pan: None,
            minimap_nav: false,
            menu: MenuState::default(),
            minimap: Minimap::default(),
            minimap_visible: true,
            modal: None,
            action_status: None,
            logs,
            falling: None,
            links: Vec::new(),
            mounts,
            zoom_step: DEFAULT_ZOOM_STEP,
            diagnostics: config.diagnostics,
            resize_deadline: None,
            should_exit: false,
            should_shutdown: false,
            dirty: true,
        })
    }

    fn size(&self) -> io::Result<(u16, u16)> {
        self.native_extent.map(|(width, height)| Ok(native_ui4::cell_size(width, height, self.zoom_step))).unwrap_or_else(terminal::size)
    }

    fn set_selected(&mut self, selected: Option<usize>) -> bool {
        let changed = self.selected != selected || !self.selected_files.is_empty();
        self.selected = selected;
        self.selected_files.clear();
        self.selected_stats = selected.and_then(|id| self.graph.selection_stats(id));
        self.show_paths = self
            .graph
            .image_paths_for_selection(selected, &[])
            .unwrap_or_default();
        if changed {
            self.action_status = None;
            self.selected_sha256 = None;
            self.mark_dirty();
        }
        changed
    }

    /// Apply a completed primary-button click. Re-clicking a regular file in
    /// the same folder toggles it; selecting a folder or a file in another
    /// folder starts a new selection.
    fn select_node_on_click(&mut self, node_id: usize) -> bool {
        let Some(node) = self.graph.node(node_id) else {
            return self.set_selected(None);
        };
        if node.is_dir {
            return self.set_selected(Some(node_id));
        }
        let parent = node.path.parent().map(Path::to_path_buf);
        let can_toggle = parent.is_some()
            && !self.selected_files.is_empty()
            && self.selected_files.iter().all(|selected| {
                self.graph
                    .node(*selected)
                    .and_then(|node| node.path.parent())
                    == parent.as_deref()
            });
        let mut next = if can_toggle {
            self.selected_files.clone()
        } else {
            Vec::new()
        };
        if let Some(index) = next.iter().position(|selected| *selected == node_id) {
            next.remove(index);
        } else {
            next.push(node_id);
        }
        let primary = next.last().copied();
        let changed = self.selected != primary || self.selected_files != next;
        self.selected = primary;
        self.selected_files = next;
        self.selected_stats = (self.selected_files.len() == 1)
            .then(|| self.selected_files[0])
            .and_then(|id| self.graph.selection_stats(id));
        self.show_paths = self
            .graph
            .image_paths_for_selection(self.selected, self.selected_files.as_slice())
            .unwrap_or_default();
        self.selected_sha256 = None;
        if changed {
            self.mark_dirty();
        }
        changed
    }

    fn drag_sources_for(&self, node_id: usize) -> Vec<usize> {
        if self.selected_files.contains(&node_id) {
            self.selected_files.clone()
        } else {
            vec![node_id]
        }
    }

    fn set_selected_sha256(&mut self, result: Sha256Result) {
        self.selected_sha256 = Some(result);
        self.mark_dirty();
    }

    fn clear_selected_sha256(&mut self) {
        if self.selected_sha256.take().is_some() {
            self.mark_dirty();
        }
    }

    fn set_path_hover(&mut self, path: Option<PathBuf>) -> bool {
        if self.path_hover == path {
            return false;
        }
        self.path_hover = path;
        self.dirty = true;
        true
    }

    fn refresh_terminal_frontend(&mut self) {
        let local = edit_action::is_local_ui4();
        if local != self.local_ui4 {
            self.local_ui4 = local;
            self.mark_dirty();
        }
    }

    fn menu_context(&self) -> MenuContext {
        if self.selected_files.len() > 1 {
            return if self.show_paths.is_empty() {
                MenuContext::Files
            } else {
                MenuContext::ImageFiles
            };
        }
        match self.selected.and_then(|id| self.graph.node(id)) {
            Some(node) if !node.is_dir && !node.is_placeholder && !node.is_removed && !node.is_moved && node.content_type == trueos::content_identity::ContentTypeId::BLOB => MenuContext::BlobFile,
            Some(node) if self.local_ui4 && graph_view::is_editable_text_file(node) => MenuContext::LocalTextFile,
            Some(node) if node.is_dir && !self.show_paths.is_empty() => MenuContext::ImageFolder,
            Some(node) if node.is_dir => MenuContext::Folder,
            Some(_) if !self.show_paths.is_empty() => MenuContext::ImageFile,
            Some(node) if graph_view::is_playable_video_type(node.content_type) => MenuContext::VideoFile,
            Some(_) => MenuContext::File,
            None => MenuContext::None,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.action_status = Some(message.clone());
        archive_log(format_args!("termdir/archive: phase=ui-status message={message:?}"));
        self.logs.push_back(format!("⇝ {message}"));
        while self.logs.len() > LOG_ROWS as usize {
            self.logs.pop_front();
        }
        self.mark_dirty();
    }

    fn mark_dirty(&mut self) {
        let context = self.menu_context();
        self.menu.ensure_visible(context);
        self.dirty = true;
    }

    fn clear_transient(&mut self) {
        self.drag_preview = None;
        self.press = None;
        self.drag = None;
        self.clip_press = None;
        self.clip_drag = None;
        self.pan = None;
        self.minimap_nav = false;
    }

    fn schedule_resize(&mut self) {
        self.resize_deadline = Some(Instant::now() + RESIZE_DEBOUNCE);
    }

    fn update_resize(&mut self, now: Instant) {
        if self
            .resize_deadline
            .map(|deadline| now >= deadline)
            .unwrap_or(false)
        {
            self.resize_deadline = None;
            self.mark_dirty();
        }
    }
}

#[derive(Clone, Copy)]
struct Layout {
    width: u16,
    menu_x: u16,
    separator_y: u16,
    canvas_bottom_y: u16,
    viewport: Viewport,
}

impl Layout {
    fn minimum_height(diagnostics: bool) -> u16 {
        MIN_CONTENT_HEIGHT + if diagnostics { LOG_ROWS } else { 0 }
    }

    fn current(app: &App) -> io::Result<Option<Self>> {
        let diagnostics = app.diagnostics;
        let (width, height) = app.size()?;
        if width < MIN_WIDTH || height < Self::minimum_height(diagnostics) {
            return Ok(None);
        }

        let diagnostic_rows = if diagnostics { LOG_ROWS } else { 0 };
        let menu_x = width - MENU_WIDTH;
        let separator_y = height - diagnostic_rows - 1;
        let canvas_bottom_y = separator_y - 1;
        let viewport = Viewport {
            x: CANVAS_X,
            y: 1,
            width: menu_x.saturating_sub(CANVAS_X),
            height: canvas_bottom_y,
        };

        Ok(Some(Self {
            width,
            menu_x,
            separator_y,
            canvas_bottom_y,
            viewport,
        }))
    }
}

fn run_terminal_session(
    app: &mut App,
    mut first_frame_ready: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    app.refresh_terminal_frontend();
    app.should_exit = false;
    app.should_shutdown = false;
    app.clear_transient();
    app.resize_deadline = None;
    app.dirty = true;

    let mut terminal_guard = TerminalGuard::enter()?;
    let stdout = stdout();
    let mut out = BufWriter::with_capacity(FRAME_OUTPUT_BUFFER_CAPACITY, stdout.lock());
    let mut renderer = Renderer::default();
    let mut first_frame = true;
    let mut archive_modal_presented = false;
    archive_log(format_args!("termdir/archive: phase=session-start markers=v1"));

    let session_result = (|| {
        loop {
            let now = Instant::now();
            app.update_resize(now);
            update_animation(app, now);
            poll_ui4_drop(app)?;
            let archive_scene = app.graph.scene_revision();
            if let Some(status) = app.graph.poll_archive(now) {
                app.set_selected(None);
                app.clear_transient();
                app.log(status);
            }
            if app.graph.scene_revision() != archive_scene {
                app.mark_dirty();
            }

            if app.dirty && app.resize_deadline.is_none() {
                let frame = compose_frame(app)?;
                renderer.present(&mut out, frame)?;
                let archive_modal = app.modal.as_ref().is_some_and(|modal| matches!(modal.pending, PendingAction::ExtractArchive { .. }));
                if archive_modal && !archive_modal_presented {
                    archive_log(format_args!("termdir/archive: phase=modal-presented"));
                }
                archive_modal_presented = archive_modal;
                app.dirty = false;
                if first_frame {
                    first_frame_ready()?;
                    first_frame = false;
                }
            }
            if app.should_exit {
                break;
            }

            if event::poll(TICK)? {
                // Coalesce a bounded burst before repainting. Drag and key-repeat
                // events often arrive in clusters; this renders the newest camera
                // position once instead of every intermediate position.
                for batch_index in 0..MAX_EVENT_BATCH {
                    let terminal_event = event::read()?;
                    handle_event(app, terminal_event)?;
                    if app.should_exit || batch_index + 1 == MAX_EVENT_BATCH {
                        break;
                    }
                    if !event::poll(Duration::from_millis(0))? {
                        break;
                    }
                }
            }
        }
        Ok(())
    })();

    drop(out);
    app.drag_preview = None;
    let restore_result = terminal_guard.restore();
    match session_result {
        Err(error) => {
            let _ = restore_result;
            Err(error)
        }
        Ok(()) => restore_result,
    }
}

fn terminal_lease_io(error: trueos::vshell::TerminalLeaseError) -> io::Error {
    io::Error::new(io::ErrorKind::Other, error.to_string())
}

fn main() -> io::Result<()> {
    let config = Config::from_args()?;
    if config.backend == Backend::Ui4 { return native_ui4::run(config); }
    // Bare-metal input requires the terminal lease to establish the terminal
    // route before the full application performs its VFS-backed initialization.
    // Keep the claim rollback explicit so an initialization error still returns
    // ownership to Shell2 deterministically.
    let mut lease = trueos::vshell::terminal_initial_lease().map_err(terminal_lease_io)?;
    let mut app = match App::new(config) {
        Ok(app) => app,
        Err(error) => {
            let reason = format!("termdir initialization failed after terminal claim: {error}");
            let _ = trueos::vshell::report_exit_reason(reason.as_str());
            let _ = lease.release_to_shell();
            return Err(error);
        }
    };

    loop {
        if let Err(error) = run_terminal_session(&mut app, || {
            lease.acknowledge_ready().map_err(terminal_lease_io)
        }) {
            let reason = format!("termdir terminal session failed: {error}");
            let _ = trueos::vshell::report_exit_reason(reason.as_str());
            let _ = lease.release_to_shell();
            return Err(error);
        }

        if app.should_shutdown {
            let _ = trueos::vshell::report_exit_reason("termdir user exit");
            let _ticket = lease.release_to_shell().map_err(terminal_lease_io)?;
            return Ok(());
        }

        let ticket = lease.release_to_shell().map_err(terminal_lease_io)?;
        lease = ticket.wait_for_reentry().map_err(terminal_lease_io)?;
    }
}

fn handle_event(app: &mut App, event: Event) -> io::Result<()> {
    match event {
        Event::Resize(columns, rows) => {
            // Crossterm emits a resize on lease re-entry, including a move to SSH.
            app.refresh_terminal_frontend();
            if rows <= 12 {
                let _ = trueos::logl::log_record(
                    trueos::logl::level::INFO,
                    "termdir-startup-probe",
                    format_args!("resize-rescue park cols={columns} rows={rows}"),
                );
                app.should_exit = true;
                return Ok(());
            }
            app.schedule_resize();
        }
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            handle_key(app, key.code, key.modifiers)?
        }
        Event::Mouse(mouse) => handle_mouse(app, mouse)?,
        _ => {}
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> io::Result<()> {
    if modifiers.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('q' | 'Q')) {
        app.should_shutdown = true;
        app.should_exit = true;
        return Ok(());
    }

    if app.modal.is_some() {
        let is_input = app
            .modal
            .as_ref()
            .map(|modal| modal.is_input())
            .unwrap_or(false);
        if is_input {
            match code {
                KeyCode::Enter => confirm_modal(app, true)?,
                KeyCode::Esc => confirm_modal(app, false)?,
                KeyCode::Backspace => {
                    let changed = app
                        .modal
                        .as_mut()
                        .map(|modal| modal.backspace())
                        .unwrap_or(false);
                    if changed {
                        app.mark_dirty();
                    }
                }
                KeyCode::Char(ch) => {
                    let changed = app
                        .modal
                        .as_mut()
                        .map(|modal| modal.push_char(ch))
                        .unwrap_or(false);
                    if changed {
                        app.mark_dirty();
                    }
                }
                _ => {}
            }
        } else {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    confirm_modal(app, true)?
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    confirm_modal(app, false)?
                }
                _ => {}
            }
        }
        return Ok(());
    }

    match code {
        KeyCode::Esc => app.should_exit = true,
        KeyCode::Tab => cycle_menu_section(app, false),
        KeyCode::BackTab => cycle_menu_section(app, true),
        KeyCode::Home => {
            if let Some(index) = MenuState::index_for_command(actions::MenuCommand::Center) {
                let context = app.menu_context();
                app.menu.set_cursor(index, context);
                invoke_menu(app, index)?;
            }
        }
        KeyCode::Left | KeyCode::Char('a') | KeyCode::Char('A') => {
            if let Some(layout) = Layout::current(app)? {
                if app.graph.pan_clamped(layout.viewport, 2, 0) {
                    app.mark_dirty();
                }
            }
        }
        KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(layout) = Layout::current(app)? {
                if app.graph.pan_clamped(layout.viewport, -2, 0) {
                    app.mark_dirty();
                }
            }
        }
        KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => {
            if let Some(layout) = Layout::current(app)? {
                if app.graph.pan_clamped(layout.viewport, 0, 2) {
                    app.mark_dirty();
                }
            }
        }
        KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
            if let Some(layout) = Layout::current(app)? {
                if app.graph.pan_clamped(layout.viewport, 0, -2) {
                    app.mark_dirty();
                }
            }
        }
        KeyCode::Enter => {
            if let Some(source) = app.selected {
                if app
                    .graph
                    .node(source)
                    .map(|node| node.is_dir)
                    .unwrap_or(false)
                {
                    match app.graph.mount_node(source) {
                        Ok(()) => {
                            app.set_selected(None);
                            app.clear_transient();
                            app.log(format!("MOUNT · {}", app.graph.root_label()));
                        }
                        Err(err) => app.log(format!("ENTER FAILED · {err}")),
                    }
                }
            }
        }
        KeyCode::Delete | KeyCode::Backspace => {
            if app.selected_files.len() > 1 {
                app.modal = Some(Modal::trash_nodes(app.selected_files.clone()));
                app.mark_dirty();
            } else if let Some(source) = app.selected.filter(|id| *id != 0) {
                let mut modal = Modal::trash_node(&app.graph, source);
                if let Some(y) = selected_screen_y(app, source) {
                    modal = modal.with_origin_y(y);
                }
                app.modal = Some(modal);
                app.mark_dirty();
            }
        }
        KeyCode::Char(ch) if ch.is_ascii_digit() => {
            let context = app.menu_context();
            if app.menu.section == MenuSection::Clip {
                let visible = Layout::current(app)
                    .ok()
                    .flatten()
                    .map(|layout| {
                        MenuState::clip_visible_count(
                            layout.separator_y,
                            context,
                            app.mounts.len().saturating_add(1),
                            app.links.len(),
                        )
                    })
                    .unwrap_or(0);
                if let Some(link_index) =
                    app.menu.clip_index_for_hotkey(ch, app.links.len(), visible)
                {
                    app.menu.set_clip_cursor(link_index, app.links.len());
                    open_link(app, link_index)?;
                } else if ch == '0' && app.links.is_empty() && visible > 0 {
                    app.log("CLIP · Empty");
                }
            } else if app.menu.section == MenuSection::Mount {
                let mount_count = app.mounts.len().saturating_add(1);
                if let Some(index) = ch
                    .to_digit(10)
                    .map(|index| index as usize)
                    .filter(|index| *index < MenuState::visible_mount_count(mount_count))
                {
                    app.menu.set_mount_cursor(index, mount_count);
                    open_mount(app, index)?;
                }
            } else if let Some(index) = app.menu.index_for_hotkey(ch, context) {
                app.menu.set_cursor(index, context);
                invoke_menu(app, index)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn cycle_menu_section(app: &mut App, reverse: bool) {
    let context = app.menu_context();
    let mut changed = app.menu.cycle_section(reverse, context);
    if app.menu.section == MenuSection::Clip {
        let clip_visible = Layout::current(app)
            .ok()
            .flatten()
            .and_then(|layout| {
                MenuState::clip_header_row(
                    layout.separator_y,
                    context,
                    app.mounts.len().saturating_add(1),
                    app.links.len(),
                )
            })
            .is_some();
        if !clip_visible {
            changed |= app.menu.cycle_section(reverse, context);
        }
    }
    if changed {
        app.mark_dirty();
    }
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) -> io::Result<()> {
    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        archive_log(format_args!(
            "termdir/archive: phase=mouse-down x={} y={} selected={:?} selected_files={:?} modal={} press={} drag={} pan={}",
            mouse.column, mouse.row, app.selected, app.selected_files,
            app.modal.is_some(), app.press.is_some(), app.drag.is_some(), app.pan.is_some()
        ));
    }
    let Some(layout) = Layout::current(app)? else {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            archive_log(format_args!("termdir/archive: phase=mouse-rejected reason=no-layout"));
        }
        return Ok(());
    };

    if app.modal.is_some() {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            if let Some(yes) = modal_click(layout.viewport, mouse.column, mouse.row) {
                confirm_modal(app, yes)?;
            } else {
                archive_log(format_args!("termdir/archive: phase=modal-click reason=outside-buttons"));
            }
        }
        return Ok(());
    }

    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && close_button_hit(layout.menu_x, MENU_WIDTH, mouse.column, mouse.row)
    {
        app.should_exit = true;
        return Ok(());
    }

    if matches!(mouse.kind, MouseEventKind::Moved) {
        let hover = if mouse.row == 0 {
            breadcrumb_target_at(app, layout, mouse.column)
        } else {
            None
        };
        app.set_path_hover(hover);
        let item_hover = app.graph.hit_test(layout.viewport, mouse.column, mouse.row);
        if app.item_hover != item_hover {
            app.item_hover = item_hover;
            app.mark_dirty();
        }
    }

    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        && map_toggle_hit(mouse.column, mouse.row)
    {
        app.minimap_visible = !app.minimap_visible;
        app.mark_dirty();
        return Ok(());
    }

    if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) && mouse.row == 0 {
        if let Some(target) = breadcrumb_target_at(app, layout, mouse.column) {
            match app.graph.mount_path(&target) {
                Ok(()) => {
                    app.set_selected(None);
                    app.set_path_hover(None);
                    app.clear_transient();
                    app.log(format!("MOUNT · {}", app.graph.root_label()));
                }
                Err(err) => app.log(format!("MOUNT FAILED · {err}")),
            }
            return Ok(());
        }
    }

    // Left-dragging the minimap is a navigation scrub. Mouse-down performs
    // the first jump; subsequent drag events keep mapping the pointer through
    // the same cached minimap world bounds. No minimap scene rebuild is needed.
    if app.minimap_nav {
        match mouse.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(geometry) = minimap_geometry(app, layout) {
                    if let Some((world_x, world_y)) =
                        Minimap::world_at(&app.graph, geometry, mouse.column, mouse.row)
                    {
                        if app.graph.jump_to_world(layout.viewport, world_x, world_y) {
                            app.mark_dirty();
                        }
                    }
                }
                return Ok(());
            }
            MouseEventKind::Up(_) => {
                app.minimap_nav = false;
                return Ok(());
            }
            _ => {}
        }
    }

    if let Some(pan) = app.pan {
        match mouse.kind {
            MouseEventKind::Drag(_) => {
                let dx = mouse.column as i32 - pan.start_x as i32;
                let dy = mouse.row as i32 - pan.start_y as i32;
                let next = (pan.camera_x + dx * 2, pan.camera_y + dy * 2);
                if app
                    .graph
                    .set_camera_clamped(layout.viewport, next.0, next.1)
                {
                    app.mark_dirty();
                }
                return Ok(());
            }
            MouseEventKind::Up(_) => {
                app.pan = None;
                return Ok(());
            }
            _ => {}
        }
    }

    if handle_clip_gesture(app, mouse, layout)? {
        return Ok(());
    }

    if mouse.column >= layout.menu_x
        && app.press.is_none()
        && app.drag.is_none()
        && app.pan.is_none()
    {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            archive_log(format_args!("termdir/archive: phase=menu-hit row={} menu_x={} context={:?}", mouse.row, layout.menu_x, app.menu_context()));
        }
        let context = app.menu_context();
        let mount_count = app.mounts.len().saturating_add(1);
        if let Some(mount_index) = MenuState::mount_index_for_row(mouse.row, mount_count) {
            if app.menu.set_mount_cursor(mount_index, mount_count) {
                app.mark_dirty();
            }
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                open_mount(app, mount_index)?;
            }
        } else if let Some(index) = MenuState::index_for_row(mouse.row, context, mount_count) {
            if app.menu.set_cursor(index, context) {
                app.mark_dirty();
            }
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                invoke_menu(app, index)?;
            }
        } else if let Some(section) = MenuState::section_for_header_row(
            mouse.row,
            layout.separator_y,
            context,
            mount_count,
            app.links.len(),
        ) {
            if app.menu.set_section(section, context) {
                app.mark_dirty();
            }
        } else if let Some(link_index) = link_index_for_row(
            mouse.row,
            layout.separator_y,
            context,
            mount_count,
            app.links.len(),
        ) {
            if app.menu.set_clip_cursor(link_index, app.links.len()) {
                app.mark_dirty();
            }
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                app.clip_press = Some(ClipPress {
                    index: link_index,
                    start_x: mouse.column,
                    start_y: mouse.row,
                });
            }
        }
        return Ok(());
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Middle)
            if layout.viewport.contains(mouse.column, mouse.row) =>
        {
            let (camera_x, camera_y) = app.graph.camera();
            app.pan = Some(PanState {
                start_x: mouse.column,
                start_y: mouse.row,
                camera_x,
                camera_y,
            });
            app.press = None;
            app.drag_preview = None;
            app.drag = None;
        }
        MouseEventKind::Down(MouseButton::Left)
            if layout.viewport.contains(mouse.column, mouse.row) =>
        {
            if let Some(geometry) = minimap_geometry(app, layout) {
                if geometry.contains(mouse.column, mouse.row) {
                    if let Some((world_x, world_y)) =
                        Minimap::world_at(&app.graph, geometry, mouse.column, mouse.row)
                    {
                        if app.graph.jump_to_world(layout.viewport, world_x, world_y) {
                            app.mark_dirty();
                        }
                    }
                    app.minimap_nav = true;
                    app.press = None;
                    app.drag_preview = None;
                    app.drag = None;
                    return Ok(());
                }
            }
            if let Some(node) = app.graph.hit_test(layout.viewport, mouse.column, mouse.row) {
                app.press = Some(Press {
                    node,
                    start_x: mouse.column,
                    start_y: mouse.row,
                });
            } else {
                if app.selected.is_some() {
                    app.set_selected(None);
                    app.mark_dirty();
                }
                app.press = None;
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(press) = app.press {
                if press.node == 0 {
                    return Ok(());
                }
                let moved =
                    mouse.column.abs_diff(press.start_x) + mouse.row.abs_diff(press.start_y);
                if moved >= 1 {
                    let sources = app.drag_sources_for(press.node);
                    let over_menu =
                        mouse.column >= layout.menu_x && mouse.row <= layout.separator_y;
                    let over_trash = !over_menu
                        && is_trash_column(mouse.column)
                        && mouse.row <= layout.separator_y;
                    let over_minimap = !over_menu
                        && !over_trash
                        && minimap_hit(app, layout, mouse.column, mouse.row);
                    let path_target = if over_trash || over_menu || over_minimap || mouse.row != 0 {
                        None
                    } else {
                        breadcrumb_drop_target_at(app, layout, mouse.column, sources[0]).filter(
                            |target| {
                                sources.iter().all(|source| {
                                    app.graph.can_move_to_path(*source, target.as_path())
                                })
                            },
                        )
                    };
                    app.set_path_hover(path_target.clone());
                    let target = if over_trash || over_menu || over_minimap || path_target.is_some()
                    {
                        None
                    } else {
                        app.graph.folder_drop_target_many(
                            layout.viewport,
                            mouse.column,
                            mouse.row,
                            sources.as_slice(),
                        )
                    };
                    let next_drag = DragState {
                        sources,
                        x: mouse.column,
                        y: mouse.row,
                        target,
                        over_trash,
                        over_menu,
                    };
                    if app.drag.as_ref() != Some(&next_drag) {
                        if app.drag.is_none() {
                            let paths: Vec<_> = next_drag.sources.iter().filter_map(|source| app.graph.node(*source).map(|node| node.path.clone())).collect();
                            let label = if paths.len() == 1 {
                                app.graph.label(next_drag.sources[0])
                            } else { format!("{} entities", paths.len()) };
                            app.drag_preview = drag_preview::Preview::begin(&clip_text(&label, 48), &paths);
                        }
                        let repaint = app.drag_preview.is_none() || app.drag.as_ref().is_none_or(|previous| {
                            previous.sources != next_drag.sources || previous.target != next_drag.target
                                || previous.over_menu != next_drag.over_menu || previous.over_trash != next_drag.over_trash
                        });
                        app.drag = Some(next_drag);
                        if repaint { app.mark_dirty(); }
                    }
                }
            }
        }
        MouseEventKind::Up(_) => {
            let delivered_elsewhere = app.drag_preview.take().is_some_and(|preview| preview.finish());
            if delivered_elsewhere {
                app.drag = None;
                app.press = None;
                app.set_path_hover(None);
                app.log("DROP · delivered to another frame");
                return Ok(());
            }
            if let Some(drag) = app.drag.take() {
                let path_target = (mouse.row == 0)
                    .then(|| {
                        breadcrumb_drop_target_at(app, layout, mouse.column, drag.sources[0])
                            .filter(|target| {
                                drag.sources.iter().all(|source| {
                                    app.graph.can_move_to_path(*source, target.as_path())
                                })
                            })
                    })
                    .flatten();
                let dropped_in_menu = drag.over_menu
                    || (mouse.column >= layout.menu_x && mouse.row <= layout.separator_y);
                let dropped_in_trash = !dropped_in_menu
                    && (drag.over_trash
                        || (is_trash_column(mouse.column) && mouse.row <= layout.separator_y));
                if dropped_in_menu {
                    add_links(app, drag.sources.as_slice());
                } else if dropped_in_trash {
                    app.modal = if drag.sources.len() == 1 {
                        Some(
                            Modal::trash_node(&app.graph, drag.sources[0]).with_origin_y(mouse.row),
                        )
                    } else {
                        Some(Modal::trash_nodes(drag.sources))
                    };
                    app.mark_dirty();
                } else if let Some(target) = path_target {
                    app.modal = if drag.sources.len() == 1 {
                        Some(Modal::move_to_path(&app.graph, drag.sources[0], target))
                    } else {
                        Some(Modal::move_nodes_to_path(drag.sources, target))
                    };
                    app.mark_dirty();
                } else if let Some(target) = drag.target {
                    app.modal = if drag.sources.len() == 1 {
                        Some(Modal::move_node(&app.graph, drag.sources[0], target))
                    } else {
                        Some(Modal::move_nodes(&app.graph, drag.sources, target))
                    };
                    app.mark_dirty();
                } else {
                    app.log("MOVE · cancelled · target must be a valid folder");
                }
            } else if let Some(press) = app.press {
                app.select_node_on_click(press.node);
            }
            app.set_path_hover(None);
            app.press = None;
            app.mark_dirty();
        }
        _ => {}
    }

    Ok(())
}

fn handle_clip_gesture(app: &mut App, mouse: MouseEvent, layout: Layout) -> io::Result<bool> {
    let Some(press) = app.clip_press else {
        return Ok(false);
    };

    match mouse.kind {
        MouseEventKind::Drag(MouseButton::Left) => {
            let moved = mouse.column.abs_diff(press.start_x) + mouse.row.abs_diff(press.start_y);
            if moved == 0 {
                return Ok(true);
            }
            let Some(link) = app.links.get(press.index).cloned() else {
                app.clip_press = None;
                app.drag_preview = None;
                app.clip_drag = None;
                return Ok(true);
            };
            let target = clip_drop_target_at(app, layout, mouse.column, mouse.row, &link.path);
            app.set_path_hover((mouse.row == 0).then(|| target.clone()).flatten());
            let next = ClipDragState {
                index: press.index,
                x: mouse.column,
                y: mouse.row,
                target,
                over_clip: clip_area_hit(app, layout, mouse.column, mouse.row),
            };
            if app.clip_drag.as_ref() != Some(&next) {
                if app.clip_drag.is_none() {
                    let label = link.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| link.path.display().to_string());
                    app.drag_preview = drag_preview::Preview::begin(&clip_text(&label, 48), &[link.path.clone()]);
                }
                let repaint = app.drag_preview.is_none() || app.clip_drag.as_ref().is_none_or(|previous| {
                    previous.target != next.target || previous.over_clip != next.over_clip || previous.index != next.index
                });
                app.clip_drag = Some(next);
                if repaint { app.mark_dirty(); }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if app.drag_preview.take().is_some_and(|preview| preview.finish()) {
                app.clip_drag = None;
                app.clip_press = None;
                app.set_path_hover(None);
                app.log("DROP · delivered to another frame");
                return Ok(true);
            }
            app.clip_press = None;
            if let Some(mut drag) = app.clip_drag.take() {
                let Some(link) = app.links.get(drag.index).cloned() else {
                    app.set_path_hover(None);
                    app.mark_dirty();
                    return Ok(true);
                };
                drag.target = clip_drop_target_at(app, layout, mouse.column, mouse.row, &link.path);
                drag.over_clip = clip_area_hit(app, layout, mouse.column, mouse.row);
                if let Some(target) = drag.target {
                    app.pending_clip_move = Some(PendingClipMove {
                        source: link.path.clone(),
                        target: target.clone(),
                    });
                    app.modal = Some(Modal::move_path(link.path, target));
                    app.mark_dirty();
                } else if !drag.over_clip {
                    let removed = app.links.remove(drag.index);
                    app.menu.set_clip_cursor(
                        drag.index.min(app.links.len().saturating_sub(1)),
                        app.links.len(),
                    );
                    app.log(format!("UNLINK · {}", removed.path.display()));
                } else {
                    app.mark_dirty();
                }
            } else {
                open_link(app, press.index)?;
            }
            app.set_path_hover(None);
        }
        _ => {}
    }
    Ok(true)
}

fn poll_ui4_drop(app: &mut App) -> io::Result<()> {
    let Some((x, y, paths)) = drag_preview::take_drop() else {
        return Ok(());
    };
    if app.modal.is_some() {
        app.log("DROP · finish the current action first");
        return Ok(());
    }
    let Some(layout) = Layout::current(app)? else {
        return Ok(());
    };
    let target = clip_drop_target_at(app, layout, x, y, &paths[0]);
    if let Some(target) = target.filter(|target| {
        paths
            .iter()
            .all(|source| app.graph.can_move_path(source, target))
    }) {
        app.pending_clip_move = None;
        app.modal = Some(Modal::move_paths(paths, target));
        app.mark_dirty();
    } else {
        app.log("DROP · target must be a valid folder");
    }
    Ok(())
}

fn clip_area_hit(app: &App, layout: Layout, x: u16, y: u16) -> bool {
    if x < layout.menu_x || y > layout.separator_y {
        return false;
    }
    let context = app.menu_context();
    let mount_count = app.mounts.len().saturating_add(1);
    MenuState::clip_header_row(layout.separator_y, context, mount_count, app.links.len())
        .map(|header| y >= header)
        .unwrap_or(false)
}

fn clip_drop_target_at(
    app: &App,
    layout: Layout,
    x: u16,
    y: u16,
    source: &Path,
) -> Option<PathBuf> {
    if x >= layout.menu_x || is_trash_column(x) || minimap_hit(app, layout, x, y) {
        return None;
    }
    if y == 0 {
        let map_width = text_cell_width("🗺  ") as u16;
        let crumbs = breadcrumb_layout(app.graph.root_path(), map_width, layout.menu_x)?;
        return breadcrumb_path_at(&crumbs, x, true)
            .filter(|target| app.graph.can_move_path(source, target));
    }
    if !layout.viewport.contains(x, y) {
        return None;
    }
    let target = app.graph.hit_test(layout.viewport, x, y)?;
    let node = app.graph.node(target)?;
    (node.is_dir && app.graph.can_move_path(source, &node.path)).then(|| node.path.clone())
}

fn is_trash_column(x: u16) -> bool {
    x < DROP_HIT_WIDTH
}

fn minimap_geometry(app: &App, layout: Layout) -> Option<minimap::MinimapGeometry> {
    if !app.minimap_visible {
        return None;
    }
    let (width, height) = app.size().ok()?;
    Minimap::geometry(width, height, layout.viewport)
}

fn minimap_hit(app: &App, layout: Layout, x: u16, y: u16) -> bool {
    minimap_geometry(app, layout)
        .map(|geometry| geometry.contains(x, y))
        .unwrap_or(false)
}

fn add_links(app: &mut App, node_ids: &[usize]) {
    let mut added = 0usize;
    for node_id in node_ids {
        let Some(node) = app.graph.node(*node_id) else {
            continue;
        };
        if node.is_placeholder || node.is_removed || node.is_moved {
            continue;
        }
        let link = MenuLink {
            path: node.path.clone(),
            is_dir: node.is_dir,
        };
        app.links.retain(|existing| existing.path != link.path);
        app.links.push(link);
        added += 1;
    }
    if added == 0 {
        return;
    }
    // The Clip menu remains numbered 0..9, but never silently drops members
    // of a selection merely because more than one local page is pinned.
    app.menu
        .set_clip_cursor(app.links.len().saturating_sub(1).min(9), app.links.len());
    if added == 1 {
        app.log("LINK · pinned");
    } else {
        app.log(format!("LINK · pinned {added} files"));
    }
}

fn open_link(app: &mut App, index: usize) -> io::Result<()> {
    let Some(link) = app.links.get(index).cloned() else {
        return Ok(());
    };

    let result: io::Result<String> = if link.is_dir {
        match app.graph.mount_path(&link.path) {
            Ok(()) => {
                app.set_selected(None);
                Ok(format!("LINK · enter {}", link.path.display()))
            }
            Err(err) => Err(err),
        }
    } else if let Some(parent) = link.path.parent() {
        match app.graph.mount_path(parent) {
            Ok(()) => {
                let selected = app.graph.find_node_by_path(&link.path);
                app.set_selected(selected);
                Ok(format!("LINK · {}", link.path.display()))
            }
            Err(err) => Err(err),
        }
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "link has no parent",
        ))
    };

    match result {
        Ok(status) => {
            // Mounting through a file link may reuse the same numeric node id
            // after reload, so clear by path transition rather than id alone.
            app.clear_selected_sha256();
            app.clear_transient();
            app.log(status);
        }
        Err(err) => app.log(format!("LINK FAILED · {err}")),
    }
    Ok(())
}

fn open_mount(app: &mut App, index: usize) -> io::Result<()> {
    let result: io::Result<String> = if index == 0 {
        available_mounts().and_then(|mounts| {
            app.mounts = mounts;
            app.graph.reload()?;
            Ok(format!("MOUNT · refreshed {} root(s)", app.mounts.len()))
        })
    } else {
        let Some(mount) = app.mounts.get(index - 1).cloned() else {
            return Ok(());
        };
        app.graph
            .mount_path(&mount.path)
            .map(|()| format!("MOUNT · {}", mount.label))
    };

    match result {
        Ok(status) => {
            app.set_selected(None);
            app.clear_transient();
            app.log(status);
        }
        Err(err) => app.log(format!("MOUNT FAILED · {err}")),
    }
    Ok(())
}

fn invoke_menu(app: &mut App, index: usize) -> io::Result<()> {
    let entry = MENU_ENTRIES[index];
    archive_log(format_args!("termdir/archive: phase=dispatch command={:?} selected={:?} files={:?}", entry.command, app.selected, app.selected_files));
    if entry.command == actions::MenuCommand::Sha256 {
        // A new SHA request supersedes the previous digest, including when it
        // fails or no item is selected.
        app.clear_selected_sha256();
    }
    match dispatch_menu_selected(
        entry.command,
        &mut app.graph,
        app.selected,
        app.selected_files.as_slice(),
    )? {
        Dispatch::Exit => app.should_exit = true,
        Dispatch::Zoom => cycle_terminal_zoom(app)?,
        Dispatch::Show(paths) => match trueos::vshell::open_images(paths.as_slice()) {
            Ok(()) => app.log(format!("SHOW · opening {} image source(s)", paths.len())),
            Err(error) => app.log(format!("SHOW FAILED · host request code={error}")),
        },
        Dispatch::Play(path) => match trueos::vshell::play_video_qualified(&path) {
            Ok(()) => app.log("PLY · opening looping video"),
            Err(error) => app.log(format!("PLY FAILED · host request code={error}")),
        },
        Dispatch::Edit(path) => {
            // Recheck the live route before launching; never open a desktop
            // window on behalf of a remote terminal.
            app.refresh_terminal_frontend();
            if !app.local_ui4 {
                app.log("EDIT · separate Shell3 window requires local UI4");
            } else {
                match edit_action::open(&path) {
                    Ok(()) => app.log("EDIT · requested editor in a new Shell3 window"),
                    Err(error) => app.log(format!("EDIT FAILED · {error}")),
                }
            }
        }
        Dispatch::Modal(mut modal) => {
            archive_log(format_args!("termdir/archive: phase=modal-open action={:?} value={:?}", modal.pending, modal.input_value()));
            if modal.trash_origin_y.is_none() {
                let trash_source = match &modal.pending {
                    PendingAction::Trash { source } => Some(*source),
                    _ => None,
                };
                if let Some(source) = trash_source {
                    if let Some(y) = selected_screen_y(app, source) {
                        modal = modal.with_origin_y(y);
                    }
                }
            }
            app.modal = Some(modal);
            app.mark_dirty();
        }
        Dispatch::Sha256(result) => {
            let status = format!("SHA256 · {} · {}", result.digest, result.path.display());
            if app.selected == Some(result.source) {
                app.set_selected_sha256(result);
            }
            app.log(status);
        }
        Dispatch::Status(status) => {
            if matches!(
                entry.command,
                actions::MenuCommand::Reload
                    | actions::MenuCommand::Parent
                    | actions::MenuCommand::Enter
                    | actions::MenuCommand::Depth
                    | actions::MenuCommand::Zip
            ) {
                app.set_selected(None);
                app.clear_transient();
            }
            app.log(status);
        }
    }
    Ok(())
}

fn cycle_terminal_zoom(app: &mut App) -> io::Result<()> {
    let next = (app.zoom_step + 1) % ZOOM_LEVELS.len();
    let percent = ZOOM_LEVELS[next];
    if app.native_extent.is_none() {
        let mut output = stdout().lock();
        write!(output, "\x1b]777;terminal_zoom={percent}\x07")?;
        output.flush()?;
    }
    app.zoom_step = next;
    app.log(format!("VIEW · zoom {percent}%"));
    Ok(())
}

fn selected_screen_y(app: &App, source: usize) -> Option<u16> {
    let layout = Layout::current(app).ok().flatten()?;
    app.graph.screen_y(source, layout.viewport)
}

fn confirm_modal(app: &mut App, yes: bool) -> io::Result<()> {
    let Some(modal) = app.modal.take() else {
        return Ok(());
    };
    archive_log(format_args!("termdir/archive: phase=modal-confirm yes={yes} action={:?} value={:?}", modal.pending, modal.input_value()));

    if !yes {
        app.pending_clip_move = None;
        app.log(if modal.is_input() {
            "INPUT · cancelled"
        } else {
            "CONFIRM · no · cancelled"
        });
        return Ok(());
    }

    if modal.is_input() && modal.input_value().map(str::trim).unwrap_or("").is_empty() {
        app.modal = Some(modal);
        app.log("INPUT · name cannot be empty");
        return Ok(());
    }

    let retry_modal = if modal.is_input() {
        Some(modal.clone())
    } else {
        None
    };
    let falling_seed = match &modal.pending {
        PendingAction::Trash { source } => app
            .graph
            .node(*source)
            .map(|node| (if node.is_dir { "🖿" } else { "🖹" }, modal.trash_origin_y)),
        _ => None,
    };
    let pending_clip_move = app.pending_clip_move.take();

    match execute_modal(modal, &mut app.graph) {
        Ok(outcome) => {
            if let Some(clip_move) = pending_clip_move {
                if let Some(name) = clip_move.source.file_name() {
                    let destination = clip_move.target.join(name);
                    if let Some(link) = app
                        .links
                        .iter_mut()
                        .find(|link| link.path == clip_move.source)
                    {
                        link.path = destination;
                    }
                }
            }
            app.set_selected(None);
            app.clear_transient();
            app.log(outcome.status);
            if outcome.trashed_label.is_some() {
                if let Some((icon, origin_y)) = falling_seed {
                    let y = origin_y.unwrap_or(1);
                    app.falling = Some(FallingGhost {
                        icon,
                        y,
                        next_step: Instant::now() + FALL_TICK,
                    });
                    app.mark_dirty();
                }
            }
        }
        Err(err) => {
            if retry_modal.is_some() {
                app.modal = retry_modal;
            }
            app.log(format!("ACTION FAILED · {err}"));
        }
    }
    Ok(())
}

fn update_animation(app: &mut App, now: Instant) {
    // Do not query terminal geometry on every idle event-loop turn. On TrueOS
    // terminal::size() is a real platform boundary; only the falling-trash
    // animation needs a fresh layout while it is active.
    if app.falling.is_none() {
        return;
    }

    let stop_y = Layout::current(app)
        .ok()
        .flatten()
        // The paper bin occupies separator_y. The icon may reach exactly one
        // row above it, then disappears on the following animation tick.
        .map(|layout| layout.separator_y.saturating_sub(1).max(1))
        .unwrap_or(1);

    let mut changed = false;
    let mut finished = false;
    if let Some(falling) = app.falling.as_mut() {
        if now >= falling.next_step {
            changed = true;
            if falling.y >= stop_y {
                finished = true;
            } else {
                falling.y = falling.y.saturating_add(1).min(stop_y);
                falling.next_step = now + FALL_TICK;
            }
        }
    }

    if finished {
        app.falling = None;
    }
    if changed {
        app.mark_dirty();
    }
}

fn compose_frame(app: &mut App) -> io::Result<Frame> {
    let (width, height) = app.size()?;
    let mut frame = Frame::new(width, height);

    let Some(layout) = Layout::current(app)? else {
        print_at(&mut frame, 0, 0, "terminal too small", Style::default());
        print_at(
            &mut frame,
            0,
            1,
            &format!(
                "need at least {MIN_WIDTH}x{}; got {width}x{height}",
                Layout::minimum_height(app.diagnostics)
            ),
            Style::default(),
        );
        return Ok(frame);
    };

    // Resize changes the viewport and therefore the legal camera rectangle.
    // Clamp once before composing so a formerly valid camera can never reveal
    // empty space after the terminal becomes larger.
    app.graph.clamp_camera(layout.viewport);

    draw_top_rule(&mut frame, app, layout.menu_x);
    draw_canvas_frame(&mut frame, layout.canvas_bottom_y);
    draw_dropzone(&mut frame, app, layout);

    let drag_sources = app
        .drag
        .as_ref()
        .map(|drag| drag.sources.as_slice())
        .unwrap_or(&[]);
    let drop_target = app.drag.as_ref().and_then(|drag| drag.target).or_else(|| {
        app.clip_drag
            .as_ref()
            .and_then(|drag| drag.target.as_deref())
            .and_then(|path| app.graph.find_node_by_path(path))
    });
    app.graph.render_many(
        &mut frame,
        layout.viewport,
        app.selected,
        app.selected_files.as_slice(),
        app.item_hover,
        drag_sources,
        drop_target,
    );

    if app.minimap_visible {
        app.minimap.draw(&mut frame, &app.graph, layout.viewport);
    }

    if app.native_extent.is_some() {
        let text = "not ready";
        print_at(&mut frame,
            layout.viewport.x + layout.viewport.width.saturating_sub(text.len() as u16) / 2,
            layout.viewport.y + layout.viewport.height / 2,
            text, Style::new(Color::DarkGrey, Color::Reset));
    }

    if let Some(drag) = app.drag.as_ref().filter(|_| app.drag_preview.is_none()) {
        draw_drag_box(&mut frame, app, drag, layout);
    }
    if let Some(drag) = app.clip_drag.as_ref().filter(|_| app.drag_preview.is_none()) {
        draw_clip_drag_box(&mut frame, app, drag, layout);
    }
    if let Some(falling) = &app.falling {
        draw_falling(&mut frame, falling, layout);
    }

    draw_menu(
        &mut frame,
        layout.menu_x,
        layout.separator_y,
        app.menu,
        MENU_WIDTH,
        app.menu_context(),
        &app.mounts,
        &app.links,
        app.selected_stats.as_ref(),
        app.zoom_step,
        app.graph.depth_limit(),
        app.graph.spacing_level(),
        app.graph.line_style(),
        app.graph.layout_mode(),
    );
    draw_bottom_rule(&mut frame, app, layout.menu_x, layout.separator_y);

    if app.diagnostics {
        draw_logs(&mut frame, app, layout.separator_y + 1, layout.width);
    }

    if let Some(modal) = &app.modal {
        draw_modal(&mut frame, modal, layout.viewport);
    }

    Ok(frame)
}

#[derive(Clone, Debug)]
struct BreadcrumbCrumb {
    label: String,
    path: PathBuf,
    x: u16,
    width: u16,
    is_last: bool,
}

#[derive(Clone, Debug)]
struct BreadcrumbLayout {
    block_x: u16,
    content_x: u16,
    content_width: u16,
    prefix: Option<String>,
    crumbs: Vec<BreadcrumbCrumb>,
}

fn map_toggle_hit(x: u16, y: u16) -> bool {
    y == 0 && x < text_cell_width("🗺  ") as u16
}

fn draw_top_rule(frame: &mut Frame, app: &App, menu_x: u16) {
    // World-map owns the physical top-left. Two cells of air to its right keep
    // the breadcrumb detached without consuming a leading drop-rail column.
    let map_cluster = "🗺  ";
    let map_width = text_cell_width(map_cluster) as u16;

    print_at(frame, 0, 0, map_cluster, Style::default());
    draw_pattern(frame, map_width, 0, menu_x, "🞃", "🞁");
    if let Some(layout) = breadcrumb_layout(app.graph.root_path(), map_width, menu_x) {
        draw_breadcrumb(frame, app, &layout);
    }
}

fn breadcrumb_target_at(app: &App, layout: Layout, x: u16) -> Option<PathBuf> {
    let map_width = text_cell_width("🗺  ") as u16;
    let crumbs = breadcrumb_layout(app.graph.root_path(), map_width, layout.menu_x)?;
    breadcrumb_path_at(&crumbs, x, false)
}

fn breadcrumb_drop_target_at(app: &App, layout: Layout, x: u16, source: usize) -> Option<PathBuf> {
    let map_width = text_cell_width("🗺  ") as u16;
    let crumbs = breadcrumb_layout(app.graph.root_path(), map_width, layout.menu_x)?;
    breadcrumb_path_at(&crumbs, x, true).filter(|path| app.graph.can_move_to_path(source, path))
}

fn breadcrumb_path_at(layout: &BreadcrumbLayout, x: u16, include_last: bool) -> Option<PathBuf> {
    layout
        .crumbs
        .iter()
        .find(|crumb| {
            (include_last || !crumb.is_last)
                && x >= crumb.x
                && x < crumb.x.saturating_add(crumb.width)
        })
        .map(|crumb| crumb.path.clone())
}

fn breadcrumb_layout(path: &Path, start_x: u16, end_x: u16) -> Option<BreadcrumbLayout> {
    let width = end_x.saturating_sub(start_x) as usize;
    if width < 8 {
        return None;
    }
    let separator_width = text_cell_width(RULE_TITLE_SEPARATOR);
    let separator_pair_width = separator_width.saturating_mul(2);
    let max_content = width.saturating_sub(separator_pair_width);
    if max_content == 0 {
        return None;
    }

    let raw = path_components(path);
    if raw.is_empty() {
        return None;
    }
    let breadcrumb_separator_width = text_cell_width(BREADCRUMB_SEPARATOR);

    let content_width_for = |from: usize| -> usize {
        let mut used = if from > 0 {
            text_cell_width("…") + breadcrumb_separator_width
        } else {
            0
        };
        for i in from..raw.len() {
            if i > from {
                used += breadcrumb_separator_width;
            }
            used += text_cell_width(&raw[i].0);
        }
        used
    };

    let mut from = 0usize;
    while from + 1 < raw.len() && content_width_for(from) > max_content {
        from += 1;
    }

    let prefix = (from > 0).then(|| format!("…{BREADCRUMB_SEPARATOR}"));
    let mut labels: Vec<String> = raw[from..].iter().map(|(label, _)| label.clone()).collect();
    let mut content_width = content_width_for(from);
    if content_width > max_content {
        // A single very long final directory name: preserve its tail and keep
        // the final crumb bold/non-clickable.
        let prefix_width = prefix.as_deref().map(text_cell_width).unwrap_or(0);
        let budget = max_content.saturating_sub(prefix_width);
        if let Some(last) = labels.last_mut() {
            *last = clip_text_tail_cells(last, budget);
        }
        content_width = prefix_width + labels.last().map(|s| text_cell_width(s)).unwrap_or(0);
    }

    let block_width = content_width
        .saturating_add(separator_pair_width)
        .min(width);
    let block_x = start_x + ((width - block_width) / 2) as u16;
    let content_x = block_x.saturating_add(separator_width as u16);
    let mut cursor = content_x;
    if let Some(prefix) = &prefix {
        cursor = cursor.saturating_add(text_cell_width(prefix) as u16);
    }

    let mut crumbs = Vec::new();
    for (local, source_index) in (from..raw.len()).enumerate() {
        if local > 0 {
            cursor = cursor.saturating_add(breadcrumb_separator_width as u16);
        }
        let label = labels.get(local).cloned().unwrap_or_default();
        let label_width = text_cell_width(&label) as u16;
        crumbs.push(BreadcrumbCrumb {
            label,
            path: raw[source_index].1.clone(),
            x: cursor,
            width: label_width,
            is_last: source_index + 1 == raw.len(),
        });
        cursor = cursor.saturating_add(label_width);
    }

    Some(BreadcrumbLayout {
        block_x,
        content_x,
        content_width: content_width as u16,
        prefix,
        crumbs,
    })
}

fn path_components(path: &Path) -> Vec<(String, PathBuf)> {
    let mut current = PathBuf::new();
    let mut out = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                current.push(prefix.as_os_str());
                out.push((
                    prefix.as_os_str().to_string_lossy().into_owned(),
                    current.clone(),
                ));
            }
            Component::RootDir => {
                current.push(Path::new("/"));
                out.push((BREADCRUMB_ROOT_LABEL.to_string(), current.clone()));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                current.push("..");
                out.push(("..".to_string(), current.clone()));
            }
            Component::Normal(name) => {
                current.push(name);
                out.push((name.to_string_lossy().into_owned(), current.clone()));
            }
        }
    }
    out
}

fn draw_breadcrumb(frame: &mut Frame, app: &App, layout: &BreadcrumbLayout) {
    // The wide centered middle dots bridge the text and Braille rule.
    print_at(
        frame,
        layout.block_x,
        0,
        RULE_TITLE_SEPARATOR,
        Style::default(),
    );
    print_at(
        frame,
        layout.content_x + layout.content_width,
        0,
        RULE_TITLE_SEPARATOR,
        Style::default(),
    );

    let mut cursor = layout.content_x;
    if let Some(prefix) = &layout.prefix {
        print_at(frame, cursor, 0, prefix, Style::default());
        cursor += text_cell_width(prefix) as u16;
    }

    for (index, crumb) in layout.crumbs.iter().enumerate() {
        if index > 0 {
            print_at(frame, cursor, 0, BREADCRUMB_SEPARATOR, Style::default());
            cursor += text_cell_width(BREADCRUMB_SEPARATOR) as u16;
        }

        let mut style = Style::default();
        if app.drag.is_some() && app.path_hover.as_ref() == Some(&crumb.path) {
            style = Style::new(Color::Black, Color::Cyan);
        } else if crumb.is_last {
            style = style.bold();
        } else if app.path_hover.as_ref() == Some(&crumb.path) {
            style = style.underline();
        }
        print_at(frame, cursor, 0, &crumb.label, style);
        cursor += crumb.width;
    }
}

fn draw_canvas_frame(frame: &mut Frame, bottom_y: u16) {
    for y in 1..=bottom_y {
        let edge = if y == 1 {
            "⎛"
        } else if y == bottom_y {
            "⎝"
        } else {
            "⎜"
        };
        print_at(frame, FRAME_X, y, edge, Style::default());
    }
}

fn draw_dropzone(frame: &mut Frame, app: &App, layout: Layout) {
    // The delete target remains a virtual 3-column hit area. Hover does not
    // repaint boxes/backgrounds: only the dragged file/folder glyph appears.
    if let Some((y, icon)) = app
        .drag
        .as_ref()
        .filter(|drag| drag.over_trash)
        .and_then(|drag| {
            app.graph.node(drag.sources[0]).map(|node| {
                (
                    drag.y.clamp(1, layout.canvas_bottom_y),
                    if node.is_dir { "🖿" } else { "🖹" },
                )
            })
        })
    {
        print_at(
            frame,
            DROP_X,
            y,
            icon,
            Style::new(Color::Grey, Color::Reset),
        );
    }

    print_at(
        frame,
        DROP_X,
        layout.separator_y,
        "🗑",
        Style::new(Color::DarkGrey, Color::Reset),
    );
}

fn draw_drag_box(frame: &mut Frame, app: &App, drag: &DragState, layout: Layout) {
    let Some(node) = app.graph.node(drag.sources[0]) else {
        return;
    };
    let name = if drag.sources.len() == 1 {
        node.name.as_str().to_string()
    } else {
        format!("{} files", drag.sources.len())
    };
    draw_named_drag_box(
        frame,
        drag.x,
        drag.y,
        name.as_str(),
        drag.sources.len() == 1 && node.is_dir,
        layout,
    );
}

fn draw_clip_drag_box(frame: &mut Frame, app: &App, drag: &ClipDragState, layout: Layout) {
    let Some(link) = app.links.get(drag.index) else {
        return;
    };
    let name = link
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| link.path.display().to_string());
    draw_named_drag_box(frame, drag.x, drag.y, &name, link.is_dir, layout);
}

fn draw_named_drag_box(
    frame: &mut Frame,
    drag_x: u16,
    drag_y: u16,
    name: &str,
    is_dir: bool,
    layout: Layout,
) {
    let icon = if is_dir { "🖿" } else { "🖹" };
    let inner = name.chars().count().clamp(4, 18);
    let visible = clip_text(name, inner);
    let box_width = inner + 2;

    let mut x = drag_x.saturating_add(1);
    if x + box_width as u16 >= layout.menu_x {
        x = drag_x.saturating_sub(box_width as u16);
    }
    x = x.max(CANVAS_X);
    let y = drag_y
        .saturating_add(1)
        .min(layout.canvas_bottom_y.saturating_sub(2));

    let top_dashes = inner.saturating_sub(2);
    print_at(
        frame,
        x,
        y,
        &format!("  ╾{}╮", "─".repeat(top_dashes)),
        Style::default(),
    );
    print_at(
        frame,
        x,
        y + 1,
        &format!("╿{:<width$}│", visible, width = inner),
        Style::default(),
    );
    print_at(
        frame,
        x,
        y + 2,
        &format!("╰{}{}", "─".repeat(inner), icon),
        Style::default(),
    );
}

fn draw_falling(frame: &mut Frame, falling: &FallingGhost, layout: Layout) {
    let last_visible_y = layout.separator_y.saturating_sub(2).max(1);
    let y = falling.y.clamp(1, last_visible_y);
    print_at(
        frame,
        DROP_X,
        y,
        falling.icon,
        Style::new(Color::Grey, Color::Reset),
    );
}

fn draw_bottom_rule(frame: &mut Frame, app: &App, menu_x: u16, y: u16) {
    // Actions must remain observable with the optional diagnostic panel off.
    if let Some(status) = &app.action_status {
        draw_rule_title(frame, FRAME_X, y, menu_x, status, "🞁", "🞃", true);
        return;
    }
    if app.selected_files.len() > 1 {
        draw_rule_title(
            frame,
            FRAME_X,
            y,
            menu_x,
            &format!("{} files", app.selected_files.len()),
            "🞁",
            "🞃",
            false,
        );
        return;
    }
    let selected = app.selected.and_then(|id| {
        app.graph
            .node(id)
            .filter(|node| !node.is_placeholder)
            .map(|node| (id, node.path.as_path(), node.name.as_str()))
    });
    let title = sha256_title_for_selection(selected, app.selected_sha256.as_ref());

    match title {
        Some((title, preserve_prefix)) => {
            draw_rule_title(frame, FRAME_X, y, menu_x, &title, "🞁", "🞃", preserve_prefix)
        }
        None => draw_pattern(frame, FRAME_X, y, menu_x, "🞁", "🞃"),
    }
}

fn sha256_title_for_selection(
    selected: Option<(usize, &Path, &str)>,
    result: Option<&Sha256Result>,
) -> Option<(String, bool)> {
    let (selected_id, selected_path, name) = selected?;
    if name.is_empty() {
        return None;
    }
    result
        .filter(|result| result.source == selected_id && result.path == selected_path)
        .map(|result| (sha256_rule_title(&result.digest, name), true))
        .or_else(|| Some((name.to_string(), false)))
}

fn sha256_rule_title(digest: &str, name: &str) -> String {
    format!("{digest} {BREADCRUMB_SEPARATOR} {name}")
}

fn draw_rule_title(
    frame: &mut Frame,
    start_x: u16,
    y: u16,
    end_x: u16,
    title: &str,
    first: &'static str,
    second: &'static str,
    preserve_prefix: bool,
) {
    let width = end_x.saturating_sub(start_x) as usize;
    draw_pattern(frame, start_x, y, end_x, first, second);
    let separator_width = text_cell_width(RULE_TITLE_SEPARATOR);
    let separator_pair_width = separator_width.saturating_mul(2);
    if width <= separator_pair_width || title.is_empty() {
        return;
    }

    let title_width = width.saturating_sub(separator_pair_width).max(1);
    // SHA-256 titles begin with the digest. Keep that leading identity under
    // clipping; ordinary filenames retain their established tail clipping.
    let visible = if preserve_prefix {
        clip_text_head_cells(title, title_width)
    } else {
        clip_text_tail_cells(title, title_width)
    };
    let visible_width = text_cell_width(&visible);
    let block_width = visible_width
        .saturating_add(separator_pair_width)
        .min(width);
    let block_x = start_x + ((width - block_width) / 2) as u16;
    let text_x = block_x.saturating_add(separator_width as u16);

    print_at(frame, block_x, y, RULE_TITLE_SEPARATOR, Style::default());
    print_at(frame, text_x, y, &visible, Style::default());
    print_at(
        frame,
        text_x.saturating_add(visible_width as u16),
        y,
        RULE_TITLE_SEPARATOR,
        Style::default(),
    );
}

fn draw_logs(frame: &mut Frame, app: &App, start_y: u16, width: u16) {
    for row in 0..LOG_ROWS {
        let text = app.logs.get(row as usize).map(String::as_str).unwrap_or("");
        let clipped = clip_text(text, width as usize);
        print_at(frame, 0, start_y + row, &clipped, Style::default());
    }
}

fn draw_pattern(
    frame: &mut Frame,
    start_x: u16,
    y: u16,
    end_x: u16,
    first: &'static str,
    second: &'static str,
) {
    for x in start_x..end_x {
        // Absolute-column parity keeps every independently drawn segment on
        // the same 🞁/🞃 phase, including around overpainted titles.
        let glyph = if x % 2 == 0 { first } else { second };
        print_at(frame, x, y, glyph, Style::default());
    }
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

fn clip_text_tail_cells(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text_cell_width(text) <= max {
        return text.to_string();
    }
    if max == 1 {
        return "…".to_string();
    }

    let budget = max - 1;
    let mut tail = Vec::new();
    let mut used = 0usize;
    for ch in text.chars().rev() {
        let width = terminal_cell_width(ch) as usize;
        if used + width > budget {
            break;
        }
        tail.push(ch);
        used += width;
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

fn clip_text_head_cells(text: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text_cell_width(text) <= max {
        return text.to_string();
    }

    let mut head = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let width = terminal_cell_width(ch) as usize;
        if used + width > max {
            break;
        }
        head.push(ch);
        used += width;
    }
    head
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
