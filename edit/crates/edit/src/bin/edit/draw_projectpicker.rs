// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Project selection uses the same modal/list controls as Edit's file picker.

use std::path::Path;

use edit::framebuffer::IndexedColor;
use edit::helpers::*;
use edit::input::vk;
use edit::tui::*;
use trueos_ide::{IDE_DATABASE_PATH, ProjectSummary};

use crate::ide::{self, IdeSession, ProjectOpen};
use crate::state::{State, error_log_add};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Mode { #[default] None, Open, New }

#[derive(Default)]
pub struct ProjectPicker {
    mode: Mode,
    projects: Option<Vec<ProjectSummary>>,
    selected: Option<String>,
    name: String,
}

impl ProjectPicker {
    pub fn show_open(&mut self) { *self = Self { mode: Mode::Open, ..Self::default() }; }
    pub fn show_new(&mut self) { *self = Self { mode: Mode::New, ..Self::default() }; }
}

pub fn draw_project_picker(ctx: &mut Context, state: &mut State) {
    if state.wants_close_project {
        state.wants_close_project = false;
        let result = state.ide.as_ref().map_or(Ok(()), |session| session.check_project_change(&state.documents));
        if let Err(error) = result {
            error_log_add(ctx, state, error.into());
        } else {
            state.documents.close_project();
            state.ide = None;
            if state.documents.len() == 0 {
                if let Err(error) = state.documents.add_untitled() { error_log_add(ctx, state, error); }
            }
            ctx.needs_rerender();
        }
    }

    let mode = state.project_picker.mode;
    if mode == Mode::None { return; }

    if mode == Mode::Open && state.project_picker.projects.is_none() {
        let projects = match &state.ide {
            Some(session) => session.list_projects(),
            None => ide::list_database_projects(Path::new(IDE_DATABASE_PATH)),
        };
        match projects {
            Ok(projects) => {
                let current = state.ide.as_ref().map(IdeSession::name);
                state.project_picker.selected = current.filter(|name| projects.iter().any(|project| &project.name == name))
                    .or_else(|| projects.first().map(|project| project.name.clone()));
                state.project_picker.projects = Some(projects);
            }
            Err(error) => {
                state.project_picker = ProjectPicker::default();
                error_log_add(ctx, state, error.into());
                return;
            }
        }
    }

    let width = (ctx.size().width - 20).max(10);
    let height = (ctx.size().height - 10).max(10);
    let mut activate = false;
    let mut done = false;
    let mut new = false;
    ctx.modal_begin("project-picker", if mode == Mode::New { "New Project" } else { "Open Project" });
    ctx.attr_intrinsic_size(Size { width, height: if mode == Mode::New { 6 } else { height } });
    {
        let contains_focus = ctx.contains_focus();
        if mode == Mode::New {
            ctx.block_begin("name");
            ctx.attr_padding(Rect::two(1, 1));
            ctx.inherit_focus();
            ctx.label("label", "Project name:");
            ctx.editline("name", &mut state.project_picker.name);
            ctx.focus_on_first_present();
            ctx.block_end();
            if contains_focus && ctx.consume_shortcut(vk::RETURN) { activate = true; }
        } else {
            ctx.scrollarea_begin("projects", Size { width: 0, height: height - 2 });
            ctx.attr_background_rgba(ctx.indexed_alpha(IndexedColor::Black, 1, 4));
            ctx.inherit_focus();
            ctx.list_begin("projects");
            ctx.focus_on_first_present();
            for project in state.project_picker.projects.as_deref().unwrap_or_default() {
                match ctx.list_item(state.project_picker.selected.as_ref() == Some(&project.name), &project.name) {
                    ListSelection::Unchanged => {},
                    ListSelection::Selected => state.project_picker.selected = Some(project.name.clone()),
                    ListSelection::Activated => {
                        state.project_picker.selected = Some(project.name.clone());
                        activate = true;
                    },
                }
                ctx.attr_overflow(Overflow::TruncateTail);
            }
            ctx.list_end();
            if state.project_picker.projects.as_ref().is_some_and(Vec::is_empty) {
                ctx.label("empty", "No projects yet. Choose New Project to create one.");
            }
            ctx.scrollarea_end();
        }
        ctx.table_begin("choices");
        ctx.attr_padding(Rect::two(0, 1));
        ctx.table_set_cell_gap(Size { width: 2, height: 0 });
        ctx.inherit_focus();
        ctx.table_next_row();
        if mode == Mode::New {
            activate |= ctx.button("create", "Create", ButtonStyle::default());
        } else {
            if state.project_picker.selected.is_some() {
                activate |= ctx.button("open", "Open", ButtonStyle::default());
            }
            new = ctx.button("new", "New Project…", ButtonStyle::default());
        }
        done |= ctx.button("cancel", "Cancel", ButtonStyle::default());
        ctx.table_end();
    }
    done |= ctx.modal_end();

    if activate && !done {
        let name = if mode == Mode::New {
            Some(state.project_picker.name.trim().to_owned())
        } else {
            state.project_picker.selected.clone()
        };
        if let Some(name) = name {
            let request = if mode == Mode::New { ProjectOpen::New } else { ProjectOpen::Existing };
            let result = match state.ide.as_mut() {
                Some(session) => session.change_project(&mut state.documents, &name, request),
                None => open_project(state, &name, request),
            };
            match result {
                Ok(()) => { done = true; ctx.needs_rerender(); },
                Err(error) => error_log_add(ctx, state, error),
            }
        }
    }
    if done { state.project_picker = ProjectPicker::default(); }
    else if new { state.project_picker.show_new(); ctx.needs_rerender(); }
}

fn open_project(state: &mut State, name: &str, mode: ProjectOpen) -> crate::apperr::Result<()> {
    let session = IdeSession::open_project(Path::new(IDE_DATABASE_PATH), name, mode)?;
    state.documents.load_project(session.project.clone())?;
    session.focus_entry(&mut state.documents);
    state.ide = Some(session);
    Ok(())
}
