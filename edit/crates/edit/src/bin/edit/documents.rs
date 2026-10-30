// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::io;
use std::{fs, fs::File};

use edit::buffer::{RcTextBuffer, TextBuffer};
use edit::helpers::{CoordType, Point};
use edit::lsh::{FILE_ASSOCIATIONS, Language, process_file_associations};
use edit::{path, sys};

use crate::apperr;
use crate::settings::Settings;
use crate::state::DisplayablePathBuf;

pub struct Document {
    pub buffer: RcTextBuffer,
    pub path: Option<PathBuf>,
    pub dir: Option<DisplayablePathBuf>,
    pub filename: String,
    pub file_id: Option<sys::FileId>,
    pub new_file_counter: usize,
    pub language_override: Option<Option<&'static Language>>,
    #[cfg(feature = "ide")]
    pub project: Option<crate::ide::ProjectHandle>,
}

impl Document {
    pub fn save(&mut self, new_path: Option<PathBuf>) -> apperr::Result<()> {
        let path = new_path.as_deref().unwrap_or_else(|| self.path.as_ref().unwrap().as_path());
        #[cfg(feature = "ide")]
        if let Some(project) = &self.project {
            let path = path.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput,
                "Project source paths must be UTF-8"))?;
            let mut bytes = Vec::new();
            self.buffer.borrow_mut().write_stream(&mut bytes)?;
            project.borrow_mut().save_file(path, bytes)?;
            self.buffer.borrow_mut().mark_as_clean();
            if let Some(path) = new_path {
                self.set_path(path);
            }
            return Ok(());
        }
        let mut file = DocumentManager::open_for_writing(path)?;

        {
            let mut tb = self.buffer.borrow_mut();
            tb.write_file(&mut file)?;
        }

        if let Ok(id) = sys::file_id(None, path) {
            self.file_id = Some(id);
        }

        if let Some(path) = new_path {
            self.set_path(path);
        }

        Ok(())
    }

    pub fn reread(&mut self, encoding: Option<&'static str>) -> apperr::Result<()> {
        let path = self.path.as_ref().unwrap().as_path();
        #[cfg(feature = "ide")]
        if let Some(project) = &self.project {
            let project = project.borrow();
            let source = project.source.file(path.to_str().unwrap_or_default())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Project source no longer exists"))?;
            self.buffer.borrow_mut().read_stream(&mut source.bytes.as_slice(), encoding, Some(source.bytes.len()))?;
            return Ok(());
        }
        let mut file = DocumentManager::open_for_reading(path)?;

        {
            let mut tb = self.buffer.borrow_mut();
            tb.read_file(&mut file, encoding)?;
        }

        if let Ok(id) = sys::file_id(None, path) {
            self.file_id = Some(id);
        }

        Ok(())
    }

    fn set_path(&mut self, path: PathBuf) {
        let filename = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let dir = path.parent().map(ToOwned::to_owned).unwrap_or_default();

        self.filename = filename;
        self.dir = Some(DisplayablePathBuf::from_path(dir));
        self.path = Some(path);

        self.buffer.borrow_mut().set_ruler(if self.filename == "COMMIT_EDITMSG" { 72 } else { 0 });
        self.update_language();
    }

    pub fn auto_detect_language(&mut self) {
        self.language_override = None;
        self.update_language();
    }

    pub fn override_language(&mut self, lang: Option<&'static Language>) {
        self.language_override = Some(lang);
        self.update_language();
    }

    fn update_language(&mut self) {
        self.buffer.borrow_mut().set_language(self.get_language());
    }

    fn get_language(&self) -> Option<&'static Language> {
        if let Some(lang) = self.language_override {
            return lang;
        }

        if let Some(path) = &self.path {
            let settings = Settings::borrow();
            if let Some(lang) = process_file_associations(&settings.file_associations, path) {
                return Some(lang);
            }
            if let Some(lang) = process_file_associations(FILE_ASSOCIATIONS, path) {
                return Some(lang);
            }
        }

        None
    }

    /// Moves the cursor to a 1-based line/char position.
    /// A negative line counts backwards from the end
    /// of the document, e.g. -1 is the last line.
    pub fn cursor_move_to_goto(&self, goto: Point) {
        let mut tb = self.buffer.borrow_mut();
        let x = goto.x.saturating_sub(1);
        let y = if goto.y < 0 {
            tb.logical_line_count().saturating_add(goto.y)
        } else {
            goto.y.saturating_sub(1)
        };
        tb.cursor_move_to_logical(Point { x, y });
    }
}

#[derive(Default)]
pub struct DocumentManager {
    list: Vec<Document>,
    #[cfg(feature = "ide")]
    project: Option<crate::ide::ProjectHandle>,
}

impl DocumentManager {
    #[inline]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[inline]
    pub fn active(&self) -> Option<&Document> {
        self.list.last()
    }

    #[inline]
    pub fn active_mut(&mut self) -> Option<&mut Document> {
        self.list.last_mut()
    }

    pub fn update_active<F: FnMut(&Document) -> bool>(&mut self, mut func: F) -> bool {
        let Some(idx) = self.list.iter().rposition(&mut func) else {
            return false;
        };

        // Already active (= last) document matched? Nothing to do.
        if idx == self.list.len() - 1 {
            return false;
        }

        // Otherwise, move the matched document to the end of the list so it becomes active.
        // Uses unsafe, because `rotate_left()` is horrendously bad with -Copt-level=s
        // (it's really almost comical) and I just don't tolerate that.
        // If I'm dead and you're looking to rewrite this use `list.push(list.remove(idx))`.
        unsafe {
            let beg = self.list.as_mut_ptr();
            let doc = beg.add(idx);
            let last = beg.add(self.list.len() - 1);
            let amount = self.list.len() - idx - 1;
            let mut temp = std::mem::MaybeUninit::<Document>::uninit();

            // Make a backup of the document
            std::ptr::copy_nonoverlapping(doc, temp.as_mut_ptr(), 1);
            // Shift the rest to the front
            std::ptr::copy(doc.add(1), doc, amount);
            // Move the backup to the end
            std::ptr::copy_nonoverlapping(temp.as_ptr(), last, 1);
        }

        true
    }

    pub fn remove_active(&mut self) {
        self.list.pop();
    }

    pub fn add_untitled(&mut self) -> apperr::Result<&mut Document> {
        let buffer = Self::create_buffer()?;
        let mut doc = Document {
            buffer,
            path: None,
            dir: Default::default(),
            filename: Default::default(),
            file_id: None,
            new_file_counter: 0,
            language_override: None,
            #[cfg(feature = "ide")]
            project: self.project.clone(),
        };
        self.gen_untitled_name(&mut doc);

        // In the future this could use push_mut, but it's unstable right now. As usual.
        self.list.push(doc);
        Ok(self.list.last_mut().unwrap())
    }

    pub fn gen_untitled_name(&self, doc: &mut Document) {
        let mut new_file_counter = 0;
        for doc in &self.list {
            new_file_counter = new_file_counter.max(doc.new_file_counter);
        }
        new_file_counter += 1;

        doc.filename = format!("Untitled-{new_file_counter}.txt");
        doc.new_file_counter = new_file_counter;
    }

    pub fn add_file_path(&mut self, path: &Path) -> apperr::Result<&mut Document> {
        #[cfg(feature = "ide")]
        if let Some(project) = self.project.clone()
            && !path.is_absolute()
            && project.borrow().source.file(path.to_str().unwrap_or_default()).is_some()
        {
            return self.add_project_path(project, path);
        }
        let path = path::normalize(path);

        let mut file = match File::open(&path) {
            Ok(file) => Some(file),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };

        let file_id = if file.is_some() { Some(sys::file_id(file.as_ref(), &path)?) } else { None };

        // Check if the file is already open.
        if file_id.is_some() && self.update_active(|doc| doc.file_id == file_id) {
            let doc = self.active_mut().unwrap();
            return Ok(doc);
        }

        let buffer = Self::create_buffer()?;
        {
            if let Some(file) = &mut file {
                let mut tb = buffer.borrow_mut();
                tb.read_file(file, None)?;
            }
        }

        let mut doc = Document {
            buffer,
            path: None,
            dir: None,
            filename: Default::default(),
            file_id,
            new_file_counter: 0,
            language_override: None,
            #[cfg(feature = "ide")]
            project: None,
        };
        doc.set_path(path);

        if let Some(active) = self.active()
            && active.path.is_none()
            && active.file_id.is_none()
            && !active.buffer.borrow().is_dirty()
        {
            // If the current document is a pristine Untitled document with no
            // name and no ID, replace it with the new document.
            self.remove_active();
        }

        self.list.push(doc);
        Ok(self.list.last_mut().unwrap())
    }

    #[cfg(feature = "ide")]
    pub fn load_project(&mut self, project: crate::ide::ProjectHandle) -> apperr::Result<()> {
        let prepared = Self::prepare_project(project.clone(), &project.borrow().source)?;
        self.replace_project(prepared);
        Ok(())
    }

    /// Build every buffer before replacing the current project. A load error
    /// must leave the current documents and their undo histories intact.
    #[cfg(feature = "ide")]
    pub fn prepare_project(project: crate::ide::ProjectHandle, source: &trueos_ide::LoadedSource) -> apperr::Result<Self> {
        let mut prepared = Self { list: Vec::new(), project: Some(project.clone()) };
        for file in &source.files {
            let buffer = Self::create_buffer()?;
            buffer.borrow_mut().read_stream(&mut file.bytes.as_slice(), None, Some(file.bytes.len()))?;
            let mut doc = Document {
                buffer, path: None, dir: None, filename: String::new(), file_id: None,
                new_file_counter: 0, language_override: None, project: Some(project.clone()),
            };
            doc.set_path(PathBuf::from(&file.path));
            prepared.list.push(doc);
        }
        Ok(prepared)
    }

    #[cfg(feature = "ide")]
    pub fn replace_project(&mut self, mut prepared: Self) {
        self.close_project();
        if self.active().is_some_and(|doc| doc.path.is_none() && !doc.buffer.borrow().is_dirty()) {
            self.remove_active();
        }
        self.list.append(&mut prepared.list);
        self.project = prepared.project;
    }

    #[cfg(feature = "ide")]
    pub fn close_project(&mut self) {
        self.list.retain(|doc| doc.project.is_none());
        self.project = None;
    }

    #[cfg(feature = "ide")]
    pub fn save_project_file_as(&mut self, path: PathBuf) -> apperr::Result<()> {
        let doc = self.active_mut().ok_or_else(|| io::Error::other("No document is open"))?;
        let project = doc.project.as_ref().ok_or_else(|| io::Error::other("This document is not part of a project"))?;
        if doc.path.as_deref() != Some(path.as_path())
            && project.borrow().source.file(path.to_str().unwrap_or_default()).is_some()
        {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists,
                "A project file with that name already exists; open that file to edit it").into());
        }
        doc.save(Some(path))
    }

    #[cfg(feature = "ide")]
    fn add_project_path(&mut self, project: crate::ide::ProjectHandle, path: &Path) -> apperr::Result<&mut Document> {
        let existing = self.list.iter().position(|doc| doc.project.as_ref().is_some_and(|handle|
            std::rc::Rc::ptr_eq(handle, &project)) && doc.path.as_deref() == Some(path));
        if let Some(index) = existing {
            let doc = self.list.remove(index);
            self.list.push(doc);
            return Ok(self.list.last_mut().unwrap());
        }
        let buffer = Self::create_buffer()?;
        {
            let project = project.borrow();
            let source = project.source.file(path.to_str().unwrap_or_default())
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Project source does not exist"))?;
            buffer.borrow_mut().read_stream(&mut source.bytes.as_slice(), None, Some(source.bytes.len()))?;
        }
        let mut doc = Document {
            buffer, path: None, dir: None, filename: String::new(), file_id: None,
            new_file_counter: 0, language_override: None, project: Some(project),
        };
        doc.set_path(path.to_path_buf());
        self.list.push(doc);
        Ok(self.list.last_mut().unwrap())
    }

    #[cfg(feature = "ide")]
    pub fn save_project_documents(&mut self) -> apperr::Result<()> {
        let mut buffers = Vec::new();
        for doc in &self.list {
            if doc.project.is_some() && doc.buffer.borrow().is_dirty() {
                let path = doc.path.as_deref().and_then(Path::to_str)
                    .ok_or_else(|| io::Error::other("Save new project documents before compiling"))?;
                let mut bytes = Vec::new();
                doc.buffer.borrow_mut().write_stream(&mut bytes)?;
                buffers.push((path.to_owned(), bytes));
            }
        }
        if !buffers.is_empty() {
            self.project.as_ref().ok_or_else(|| io::Error::other("No IDE project is open"))?
                .borrow_mut().save_buffers(buffers)?;
            for doc in &self.list {
                if doc.project.is_some() {
                    doc.buffer.borrow_mut().mark_as_clean();
                }
            }
        }
        Ok(())
    }

    #[cfg(feature = "ide")]
    pub fn open_project_paths(&self) -> Vec<String> {
        self.list.iter().filter(|doc| doc.project.is_some())
            .filter_map(|doc| doc.path.as_deref().and_then(Path::to_str).map(str::to_owned)).collect()
    }

    #[cfg(feature = "ide")]
    pub fn has_dirty_project_documents(&self) -> bool {
        self.list.iter().any(|doc| doc.project.is_some() && doc.buffer.borrow().is_dirty())
    }

    #[cfg(feature = "ide")]
    pub fn active_project_path(&self) -> Option<String> {
        self.active().filter(|doc| doc.project.is_some())
            .and_then(|doc| doc.path.as_deref()).and_then(Path::to_str).map(str::to_owned)
    }

    #[cfg(feature = "ide")]
    pub fn selected_project_text(&self) -> String {
        self.active().filter(|doc| doc.project.is_some())
            .and_then(|doc| doc.buffer.borrow_mut().extract_user_selection(false))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default()
    }

    #[cfg(feature = "ide")]
    pub fn project_text(&self, path: &str) -> io::Result<Option<String>> {
        if let Some(doc) = self.list.iter().find(|doc| doc.project.is_some()
            && doc.path.as_deref() == Some(Path::new(path)))
        {
            let buffer = doc.buffer.borrow();
            let mut bytes = Vec::new();
            let mut offset = 0;
            loop {
                let chunk = buffer.read_forward(offset);
                if chunk.is_empty() { break; }
                bytes.extend_from_slice(chunk);
                offset += chunk.len();
            }
            return String::from_utf8(bytes).map(Some)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
        }
        let Some(project) = &self.project else { return Ok(None) };
        let project = project.borrow();
        project.source.file(path).map(|file| String::from_utf8(file.bytes.clone())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))).transpose()
    }

    #[cfg(feature = "ide")]
    pub fn write_project_text(&mut self, path: &str, revision: i64, expected: Option<&str>, text: &str) -> apperr::Result<i64> {
        if !trueos_ide::workspace::valid_source_path(path) || path.trim() != path
            || path.chars().any(char::is_control) || path.len() > 4096
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid project source path").into());
        }
        let project = self.project.clone().ok_or_else(|| io::Error::other("No IDE project is open"))?;
        if project.borrow().source.revision.id != revision {
            return Err(io::Error::other("Project revision changed; read the file again before writing").into());
        }
        let current = self.project_text(path)?;
        if let Some(expected) = expected {
            if current.as_deref().unwrap_or_default() != expected {
                return Err(io::Error::other("File contents changed; read the file again before writing").into());
            }
        } else if current.is_some() {
            return Err(io::Error::other("Existing files require expected_text before writing").into());
        }
        if current.is_none() {
            let buffer = Self::create_buffer()?;
            {
                let mut buffer = buffer.borrow_mut();
                buffer.set_insert_final_newline(false);
                buffer.write_raw(text.as_bytes());
                buffer.mark_as_dirty();
            }
            let mut doc = Document {
                buffer, path: None, dir: None, filename: String::new(), file_id: None,
                new_file_counter: 0, language_override: None, project: Some(project.clone()),
            };
            doc.set_path(PathBuf::from(path));
            self.list.push(doc);
        } else {
            let doc = self.add_project_path(project.clone(), Path::new(path))?;
            {
                let mut buffer = doc.buffer.borrow_mut();
                buffer.select_all();
                buffer.write_raw(text.as_bytes());
            }
        }
        self.save_project_documents()?;
        let revision = project.borrow().source.revision.id;
        Ok(revision)
    }

    pub fn reflow_all(&self) {
        for doc in &self.list {
            let mut tb = doc.buffer.borrow_mut();
            tb.reflow();
        }
    }

    pub fn open_for_reading(path: &Path) -> apperr::Result<File> {
        File::open(path).map_err(apperr::Error::from)
    }

    pub fn open_for_writing(path: &Path) -> apperr::Result<File> {
        // Error handling for directory creation and file writing

        // It is worth doing an existence check because it is significantly
        // faster than calling mkdir() and letting it fail (at least on Windows).
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            fs::create_dir_all(parent)?;
        }

        File::create(path).map_err(apperr::Error::from)
    }

    fn create_buffer() -> apperr::Result<RcTextBuffer> {
        let buffer = TextBuffer::new_rc(false)?;
        {
            let mut tb = buffer.borrow_mut();
            tb.set_insert_final_newline(true); // As mandated by POSIX.
            tb.set_margin_enabled(true);
            tb.set_line_highlight_enabled(true);
        }
        Ok(buffer)
    }
}

/// Parse a filename in the form of "filename:line:char".
/// Returns the filename and the [`Document::cursor_move_to_goto`] coordinates.
pub fn parse_filename_goto(path: &Path) -> (&Path, Option<Point>) {
    fn parse(s: &[u8]) -> Option<CoordType> {
        let (negative, digits) = match s {
            [b'-', rest @ ..] => (true, rest),
            _ => (false, s),
        };
        if digits.is_empty() {
            return None;
        }

        let mut num: CoordType = 0;
        for &b in digits {
            if !b.is_ascii_digit() {
                return None;
            }
            let digit = (b - b'0') as CoordType;
            num = num.checked_mul(10)?.checked_add(digit)?;
        }
        Some(if negative { -num } else { num })
    }

    fn find_colon_rev(bytes: &[u8], offset: usize) -> Option<usize> {
        (0..offset.min(bytes.len())).rev().find(|&i| bytes[i] == b':')
    }

    let bytes = path.as_os_str().as_encoded_bytes();
    let colend = match find_colon_rev(bytes, bytes.len()) {
        // Reject filenames that would result in an empty filename after stripping off the :line:char suffix.
        // For instance, a filename like ":123:456" will not be processed by this function.
        Some(colend) if colend > 0 => colend,
        _ => return (path, None),
    };

    let last = match parse(&bytes[colend + 1..]) {
        Some(last) => last,
        None => return (path, None),
    };
    let mut len = colend;
    let mut goto = Point { x: 1, y: last };

    // Counting backwards is only supported for lines,
    // so a negative `last` rules out a char position.
    if last >= 0
        && let Some(colbeg) = find_colon_rev(bytes, colend)
        // Same here: Don't allow empty filenames.
        && colbeg != 0
        && let Some(first) = parse(&bytes[colbeg + 1..colend])
    {
        len = colbeg;
        goto = Point { x: last, y: first };
    }

    // Strip off the :line:char suffix.
    let path = &bytes[..len];
    let path = unsafe { OsStr::from_encoded_bytes_unchecked(path) };
    let path = Path::new(path);
    (path, Some(goto))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_last_numbers() {
        fn parse(s: &str) -> (&str, Option<Point>) {
            let (p, g) = parse_filename_goto(Path::new(s));
            (p.to_str().unwrap(), g)
        }

        assert_eq!(parse("123"), ("123", None));
        assert_eq!(parse("abc"), ("abc", None));
        assert_eq!(parse(":123"), (":123", None));
        assert_eq!(parse("abc:123"), ("abc", Some(Point { x: 1, y: 123 })));
        assert_eq!(parse("45:123"), ("45", Some(Point { x: 1, y: 123 })));
        assert_eq!(parse(":45:123"), (":45", Some(Point { x: 1, y: 123 })));
        assert_eq!(parse("abc:45:123"), ("abc", Some(Point { x: 123, y: 45 })));
        assert_eq!(parse("abc:def:123"), ("abc:def", Some(Point { x: 1, y: 123 })));
        assert_eq!(parse("1:2:3"), ("1", Some(Point { x: 3, y: 2 })));
        assert_eq!(parse("::3"), (":", Some(Point { x: 1, y: 3 })));
        assert_eq!(parse("1::3"), ("1:", Some(Point { x: 1, y: 3 })));
        assert_eq!(parse(""), ("", None));
        assert_eq!(parse(":"), (":", None));
        assert_eq!(parse("::"), ("::", None));
        assert_eq!(parse("a:1"), ("a", Some(Point { x: 1, y: 1 })));
        assert_eq!(parse("1:a"), ("1:a", None));
        assert_eq!(parse("file.txt:10"), ("file.txt", Some(Point { x: 1, y: 10 })));
        assert_eq!(parse("file.txt:10:5"), ("file.txt", Some(Point { x: 5, y: 10 })));
        assert_eq!(parse("file.txt:-1"), ("file.txt", Some(Point { x: 1, y: -1 })));
        assert_eq!(parse("file.txt:-10:5"), ("file.txt", Some(Point { x: 5, y: -10 })));
        assert_eq!(parse("file.txt:10:-5"), ("file.txt:10", Some(Point { x: 1, y: -5 })));
        assert_eq!(parse("file.txt:-"), ("file.txt:-", None));
    }
}
