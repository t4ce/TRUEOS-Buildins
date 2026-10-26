use crate::{actions, minimap};

use std::{
    collections::VecDeque,
    env,
    io::{self, stdout},
    path::{Component, Path, PathBuf, MAIN_SEPARATOR},
    time::{Duration, Instant},
};

use crate::actions::{
    dispatch_menu, draw_modal, execute_modal, modal_click, Dispatch, MenuContext, MenuLink,
    MenuSection, MenuState, Modal, PendingAction, MENU_ENTRIES,
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
use crate::graph_view::{GraphView, SelectionStats, Viewport};
use crate::menu_view::{close_button_hit, draw_menu, link_index_for_row};
use crate::minimap::Minimap;
use crate::screen::{terminal_cell_width, text_cell_width, Frame, Renderer, Style};

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
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(70);
const BUILD_ID: &str = "v0.22 · half-screen overscroll + map scrub";
const FALL_TICK: Duration = Duration::from_millis(90);

#[derive(Clone, Copy)]
struct Config {
    diagnostics: bool,
}

impl Config {
    fn from_args() -> Self {
        let mut diagnostics = env::var("EXPLORER_DIAGNOSTICS")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false);

        for arg in env::args().skip(1) {
            match arg.as_str() {
                "--diagnostics" | "--diagnostic" | "--diag" | "-d" => {
                    diagnostics = true;
                }
                "--no-diagnostics" | "--no-diagnostic" => {
                    diagnostics = false;
                }
                _ => {}
            }
        }

        Self { diagnostics }
    }
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct DragState {
    node: usize,
    x: u16,
    y: u16,
    target: Option<usize>,
    over_trash: bool,
    over_menu: bool,
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
    graph: GraphView,
    selected: Option<usize>,
    selected_stats: Option<SelectionStats>,
    path_hover: Option<PathBuf>,
    press: Option<Press>,
    drag: Option<DragState>,
    pan: Option<PanState>,
    minimap_nav: bool,
    menu: MenuState,
    minimap: Minimap,
    minimap_visible: bool,
    modal: Option<Modal>,
    logs: VecDeque<String>,
    falling: Option<FallingGhost>,
    links: Vec<MenuLink>,
    diagnostics: bool,
    resize_deadline: Option<Instant>,
    should_exit: bool,
    should_shutdown: bool,
    dirty: bool,
}

impl App {
    fn new(config: Config) -> io::Result<Self> {
        let graph = GraphView::from_current_dir()?;
        let mut logs = VecDeque::new();
        logs.push_back(format!("⇝ {BUILD_ID}"));
        logs.push_back(format!("⇝ Mounted {}", graph.root_label()));
        logs.push_back("⇝ LMB select/drag · MMB/WASD/arrows pan · Home center".to_string());
        logs.push_back("⇝ Tab changes menu segment · 0..9 runs local segment item".to_string());
        logs.push_back("⇝ Esc hides TUI · vmx_tui reopens · Ctrl-Q exits app".to_string());
        Ok(Self {
            graph,
            selected: None,
            selected_stats: None,
            path_hover: None,
            press: None,
            drag: None,
            pan: None,
            minimap_nav: false,
            menu: MenuState::default(),
            minimap: Minimap::default(),
            minimap_visible: true,
            modal: None,
            logs,
            falling: None,
            links: Vec::new(),
            diagnostics: config.diagnostics,
            resize_deadline: None,
            should_exit: false,
            should_shutdown: false,
            dirty: true,
        })
    }

    fn set_selected(&mut self, selected: Option<usize>) -> bool {
        let changed = self.selected != selected;
        self.selected = selected;
        self.selected_stats = selected.and_then(|id| self.graph.selection_stats(id));
        if changed {
            self.mark_dirty();
        }
        changed
    }

    fn set_path_hover(&mut self, path: Option<PathBuf>) -> bool {
        if self.path_hover == path {
            return false;
        }
        self.path_hover = path;
        self.dirty = true;
        true
    }

    fn menu_context(&self) -> MenuContext {
        match self.selected.and_then(|id| self.graph.node(id)) {
            Some(node) if node.is_dir => MenuContext::Folder,
            Some(_) => MenuContext::File,
            None => MenuContext::None,
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.logs.push_back(format!("⇝ {}", message.into()));
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
        self.press = None;
        self.drag = None;
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

    fn current(diagnostics: bool) -> io::Result<Option<Self>> {
        let (width, height) = terminal::size()?;
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
    app.should_exit = false;
    app.should_shutdown = false;
    app.resize_deadline = None;
    app.dirty = true;

    let mut terminal_guard = TerminalGuard::enter()?;
    let mut out = stdout();
    let mut renderer = Renderer::default();
    let mut first_frame = true;
    let session_result = (|| {
        loop {
            let now = Instant::now();
            app.update_resize(now);
            update_animation(app, now);

            if app.dirty && app.resize_deadline.is_none() {
                let frame = compose_frame(app)?;
                renderer.present(&mut out, frame)?;
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
                    handle_event(app, event::read()?)?;
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

fn trueos_main() -> io::Result<()> {
    let config = Config::from_args();
    let mut app = App::new(config)?;
    let mut lease = trueos::vshell::terminal_initial_lease().map_err(terminal_lease_io)?;

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

pub fn run() -> io::Result<()> {
    trueos_main()
}

fn handle_event(app: &mut App, event: Event) -> io::Result<()> {
    match event {
        Event::Resize(_, _) => app.schedule_resize(),
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
            if let Some(layout) = Layout::current(app.diagnostics)? {
                if app.graph.pan_clamped(layout.viewport, 2, 0) {
                    app.mark_dirty();
                }
            }
        }
        KeyCode::Right | KeyCode::Char('d') | KeyCode::Char('D') => {
            if let Some(layout) = Layout::current(app.diagnostics)? {
                if app.graph.pan_clamped(layout.viewport, -2, 0) {
                    app.mark_dirty();
                }
            }
        }
        KeyCode::Up | KeyCode::Char('w') | KeyCode::Char('W') => {
            if let Some(layout) = Layout::current(app.diagnostics)? {
                if app.graph.pan_clamped(layout.viewport, 0, 2) {
                    app.mark_dirty();
                }
            }
        }
        KeyCode::Down | KeyCode::Char('s') | KeyCode::Char('S') => {
            if let Some(layout) = Layout::current(app.diagnostics)? {
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
            if let Some(source) = app.selected.filter(|id| *id != 0) {
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
                let visible = Layout::current(app.diagnostics)
                    .ok()
                    .flatten()
                    .map(|layout| {
                        MenuState::clip_visible_count(layout.separator_y, context, app.links.len())
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
        let clip_visible = Layout::current(app.diagnostics)
            .ok()
            .flatten()
            .and_then(|layout| {
                MenuState::clip_header_row(layout.separator_y, context, app.links.len())
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
    let Some(layout) = Layout::current(app.diagnostics)? else {
        return Ok(());
    };

    if app.modal.is_some() {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            if let Some(yes) = modal_click(layout.viewport, mouse.column, mouse.row) {
                confirm_modal(app, yes)?;
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

    if mouse.column >= layout.menu_x
        && app.press.is_none()
        && app.drag.is_none()
        && app.pan.is_none()
    {
        let context = app.menu_context();
        if let Some(index) = MenuState::index_for_row(mouse.row, context) {
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
            app.links.len(),
        ) {
            if app.menu.set_section(section, context) {
                app.mark_dirty();
            }
        } else if let Some(link_index) =
            link_index_for_row(mouse.row, layout.separator_y, context, app.links.len())
        {
            if app.menu.set_clip_cursor(link_index, app.links.len()) {
                app.mark_dirty();
            }
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                open_link(app, link_index)?;
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
                    app.drag = None;
                    return Ok(());
                }
            }
            if let Some(node) = app.graph.hit_test(layout.viewport, mouse.column, mouse.row) {
                if app.selected != Some(node) {
                    app.set_selected(Some(node));
                    app.mark_dirty();
                }
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
                    let over_menu =
                        mouse.column >= layout.menu_x && mouse.row <= layout.separator_y;
                    let over_trash = !over_menu
                        && is_trash_column(mouse.column)
                        && mouse.row <= layout.separator_y;
                    let over_minimap = !over_menu
                        && !over_trash
                        && minimap_hit(app, layout, mouse.column, mouse.row);
                    let target = if over_trash || over_menu || over_minimap {
                        None
                    } else {
                        app.graph.folder_drop_target(
                            layout.viewport,
                            mouse.column,
                            mouse.row,
                            press.node,
                        )
                    };
                    let next_drag = DragState {
                        node: press.node,
                        x: mouse.column,
                        y: mouse.row,
                        target,
                        over_trash,
                        over_menu,
                    };
                    if app.drag != Some(next_drag) {
                        app.drag = Some(next_drag);
                        app.mark_dirty();
                    }
                }
            }
        }
        MouseEventKind::Up(_) => {
            if let Some(drag) = app.drag.take() {
                let dropped_in_menu = drag.over_menu
                    || (mouse.column >= layout.menu_x && mouse.row <= layout.separator_y);
                let dropped_in_trash = !dropped_in_menu
                    && (drag.over_trash
                        || (is_trash_column(mouse.column) && mouse.row <= layout.separator_y));
                if dropped_in_menu {
                    add_link(app, drag.node);
                } else if dropped_in_trash {
                    app.modal =
                        Some(Modal::trash_node(&app.graph, drag.node).with_origin_y(mouse.row));
                    app.mark_dirty();
                } else if let Some(target) = drag.target {
                    app.modal = Some(Modal::move_node(&app.graph, drag.node, target));
                    app.mark_dirty();
                } else {
                    app.log("MOVE · cancelled · target must be a valid folder");
                }
            }
            app.press = None;
            app.mark_dirty();
        }
        _ => {}
    }

    Ok(())
}

fn is_trash_column(x: u16) -> bool {
    x < DROP_HIT_WIDTH
}

fn minimap_geometry(app: &App, layout: Layout) -> Option<minimap::MinimapGeometry> {
    if !app.minimap_visible {
        return None;
    }
    let (width, height) = terminal::size().ok()?;
    Minimap::geometry(width, height, layout.viewport)
}

fn minimap_hit(app: &App, layout: Layout, x: u16, y: u16) -> bool {
    minimap_geometry(app, layout)
        .map(|geometry| geometry.contains(x, y))
        .unwrap_or(false)
}

fn add_link(app: &mut App, node_id: usize) {
    let Some(node) = app.graph.node(node_id) else {
        return;
    };
    if node.is_placeholder {
        return;
    }
    let link = MenuLink {
        path: node.path.clone(),
        is_dir: node.is_dir,
    };
    let status = format!("LINK · {}", link.path.display());
    app.links.retain(|existing| existing.path != link.path);
    app.links.push(link);
    if app.links.len() > 10 {
        app.links.remove(0);
    }
    let newest = app.links.len().saturating_sub(1);
    app.menu.set_clip_cursor(newest, app.links.len());
    app.log(status);
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
            app.clear_transient();
            app.log(status);
        }
        Err(err) => app.log(format!("LINK FAILED · {err}")),
    }
    Ok(())
}

fn invoke_menu(app: &mut App, index: usize) -> io::Result<()> {
    let entry = MENU_ENTRIES[index];
    match dispatch_menu(entry.command, &mut app.graph, app.selected)? {
        Dispatch::Exit => app.should_exit = true,
        Dispatch::Modal(mut modal) => {
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
        Dispatch::Status(status) => {
            if matches!(
                entry.command,
                actions::MenuCommand::Reload
                    | actions::MenuCommand::Parent
                    | actions::MenuCommand::Enter
                    | actions::MenuCommand::Depth
            ) {
                app.set_selected(None);
                app.clear_transient();
            }
            app.log(status);
        }
    }
    Ok(())
}

fn selected_screen_y(app: &App, source: usize) -> Option<u16> {
    let layout = Layout::current(app.diagnostics).ok().flatten()?;
    app.graph.screen_y(source, layout.viewport)
}

fn confirm_modal(app: &mut App, yes: bool) -> io::Result<()> {
    let Some(modal) = app.modal.take() else {
        return Ok(());
    };

    if !yes {
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

    match execute_modal(modal, &mut app.graph) {
        Ok(outcome) => {
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
    let stop_y = Layout::current(app.diagnostics)
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
    let (width, height) = terminal::size()?;
    let mut frame = Frame::new(width, height);

    let Some(layout) = Layout::current(app.diagnostics)? else {
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

    let drag_id = app.drag.map(|drag| drag.node);
    let drop_target = app.drag.and_then(|drag| drag.target);
    app.graph.render(
        &mut frame,
        layout.viewport,
        app.selected,
        drag_id,
        drop_target,
    );

    if app.minimap_visible {
        app.minimap.draw(&mut frame, &app.graph, layout.viewport);
    }

    if let Some(drag) = app.drag {
        draw_drag_box(&mut frame, app, drag, layout);
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
        &app.links,
        app.selected_stats.as_ref(),
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
    crumbs
        .crumbs
        .into_iter()
        .find(|crumb| !crumb.is_last && x >= crumb.x && x < crumb.x.saturating_add(crumb.width))
        .map(|crumb| crumb.path)
}

fn breadcrumb_layout(path: &Path, start_x: u16, end_x: u16) -> Option<BreadcrumbLayout> {
    let width = end_x.saturating_sub(start_x) as usize;
    if width < 8 {
        return None;
    }
    let max_content = width.saturating_sub(6);
    if max_content == 0 {
        return None;
    }

    let raw = path_components(path);
    if raw.is_empty() {
        return None;
    }
    let separator = MAIN_SEPARATOR.to_string();

    let content_width_for = |from: usize| -> usize {
        let mut used = if from > 0 { 2 } else { 0 }; // …/
        for i in from..raw.len() {
            if i > from && raw[i - 1].0 != separator {
                used += 1;
            }
            used += text_cell_width(&raw[i].0);
        }
        used
    };

    let mut from = 0usize;
    while from + 1 < raw.len() && content_width_for(from) > max_content {
        from += 1;
    }

    let prefix = (from > 0).then(|| format!("…{MAIN_SEPARATOR}"));
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

    let block_width = content_width.saturating_add(6).min(width);
    let block_x = start_x + ((width - block_width) / 2) as u16;
    let content_x = block_x.saturating_add(3);
    let mut cursor = content_x;
    if let Some(prefix) = &prefix {
        cursor = cursor.saturating_add(text_cell_width(prefix) as u16);
    }

    let mut crumbs = Vec::new();
    for (local, source_index) in (from..raw.len()).enumerate() {
        if local > 0 && raw[source_index - 1].0 != separator {
            cursor = cursor.saturating_add(1);
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
                let root = MAIN_SEPARATOR.to_string();
                current.push(Path::new(&root));
                out.push((root, current.clone()));
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
    // Leave the two patterned cells on each side untouched so 🞃🞁/🞁🞃
    // always continues from absolute column parity rather than from title
    // string length.
    print_at(frame, layout.block_x + 2, 0, " ", Style::default());
    print_at(
        frame,
        layout.content_x + layout.content_width,
        0,
        " ",
        Style::default(),
    );

    let mut cursor = layout.content_x;
    if let Some(prefix) = &layout.prefix {
        print_at(frame, cursor, 0, prefix, Style::default());
        cursor += text_cell_width(prefix) as u16;
    }

    for (index, crumb) in layout.crumbs.iter().enumerate() {
        if index > 0 {
            let previous = &layout.crumbs[index - 1];
            if previous.label != MAIN_SEPARATOR.to_string() {
                print_at(
                    frame,
                    cursor,
                    0,
                    &MAIN_SEPARATOR.to_string(),
                    Style::default(),
                );
                cursor += 1;
            }
        }

        let mut style = Style::default();
        if crumb.is_last {
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
    if let Some((y, icon)) = app.drag.filter(|drag| drag.over_trash).and_then(|drag| {
        app.graph.node(drag.node).map(|node| {
            (
                drag.y.clamp(1, layout.canvas_bottom_y),
                if node.is_dir { "🖿" } else { "🖹" },
            )
        })
    }) {
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

fn draw_drag_box(frame: &mut Frame, app: &App, drag: DragState, layout: Layout) {
    let Some(node) = app.graph.node(drag.node) else {
        return;
    };

    let icon = if node.is_dir { "🖿" } else { "🖹" };
    let name = node.name.clone();
    let inner = name.chars().count().clamp(4, 18);
    let visible = clip_text(&name, inner);
    let box_width = inner + 2;

    let mut x = drag.x.saturating_add(1);
    if x + box_width as u16 >= layout.menu_x {
        x = drag.x.saturating_sub(box_width as u16);
    }
    x = x.max(CANVAS_X);
    let y = drag
        .y
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
    let selected_name = app
        .selected
        .and_then(|id| app.graph.node(id))
        .filter(|node| !node.is_placeholder)
        .map(|node| node.name.clone())
        .filter(|name| !name.is_empty());

    match selected_name {
        Some(name) => draw_rule_title(frame, FRAME_X, y, menu_x, &name, "🞁", "🞃"),
        None => draw_pattern(frame, FRAME_X, y, menu_x, "🞁", "🞃"),
    }
}

fn draw_rule_title(
    frame: &mut Frame,
    start_x: u16,
    y: u16,
    end_x: u16,
    title: &str,
    first: &'static str,
    second: &'static str,
) {
    let width = end_x.saturating_sub(start_x) as usize;
    draw_pattern(frame, start_x, y, end_x, first, second);
    if width < 7 || title.is_empty() {
        return;
    }

    let title_width = width.saturating_sub(6).max(1);
    let visible = clip_text_tail_cells(title, title_width);
    let visible_width = text_cell_width(&visible);
    let block_width = visible_width.saturating_add(6).min(width);
    let block_x = start_x + ((width - block_width) / 2) as u16;
    let text_x = block_x.saturating_add(3);

    // The two cells on either side remain the original alternating pattern.
    // Only the inner spaces and text are overpainted, so the sequence cannot
    // produce doubled 🞁🞁 or 🞃🞃 at title boundaries.
    print_at(frame, block_x + 2, y, " ", Style::default());
    print_at(frame, text_x, y, &visible, Style::default());
    print_at(
        frame,
        text_x.saturating_add(visible_width as u16),
        y,
        " ",
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
