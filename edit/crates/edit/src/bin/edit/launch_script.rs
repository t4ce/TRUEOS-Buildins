// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Edit's one-shot TRUEOS launch directives. Paths are literal, never shell input.
use std::io;

pub fn open_paths(script: &str) -> io::Result<Vec<&str>> {
    let mut paths = Vec::new();
    for line in script.lines() {
        let Some(path) = line.strip_prefix("open ") else { continue };
        if path.is_empty() || path.contains(['\0', '\r']) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "edit open requires a file path"));
        }
        paths.push(path);
    }
    Ok(paths)
}

/// Project names and database paths are literal, never shell arguments.
#[cfg(feature = "ide")]
#[derive(Debug, PartialEq, Eq)]
pub struct IdeLaunch<'a> {
    pub project: &'a str,
    pub database: &'a str,
    pub backend: trueos_ide::protocol::CompilerBackend,
    pub entry: &'a str,
}

#[cfg(feature = "ide")]
pub fn ide_project(script: &str) -> io::Result<Option<IdeLaunch<'_>>> {
    let mut project = None;
    let mut database = None;
    let mut compiler = None;
    let mut entry = None;
    for line in script.lines() {
        let (value, slot) = if let Some(value) = line.strip_prefix("ide-project ") {
            (value, &mut project)
        } else if let Some(value) = line.strip_prefix("ide-db ") {
            (value, &mut database)
        } else if let Some(value) = line.strip_prefix("ide-compiler ") {
            (value, &mut compiler)
        } else if let Some(value) = line.strip_prefix("ide-entry ") {
            (value, &mut entry)
        } else if matches!(line, "ide-project" | "ide-db" | "ide-compiler" | "ide-entry") {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "edit IDE directive requires a value"));
        } else {
            continue;
        };
        if value.is_empty() || value.contains(['\0', '\r']) || slot.is_some() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "edit requires one nonempty value per IDE directive"));
        }
        *slot = Some(value);
    }
    use trueos_ide::protocol::CompilerBackend;
    let backend = match compiler {
        None | Some("odin-son") => CompilerBackend::OdinSon,
        Some("rustc-min") => CompilerBackend::RustcMin,
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unknown IDE compiler")),
    };
    let entry_path = entry.unwrap_or(match backend { CompilerBackend::OdinSon => "main.odin", CompilerBackend::RustcMin => "main.rs" });
    if !trueos_ide::workspace::valid_source_path(entry_path) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "IDE entry must be a relative project source path"));
    }
    match project {
        Some(project) => Ok(Some(IdeLaunch { project, database: database.unwrap_or(trueos_ide::IDE_DATABASE_PATH), backend, entry: entry_path })),
        None if database.is_some() || compiler.is_some() || entry.is_some() => Err(io::Error::new(io::ErrorKind::InvalidInput,
            "edit IDE options require ide-project")),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edit_runscript_opens_literal_paths_without_parsing_options() {
        assert_eq!(open_paths("fs-scope trueosfs\nopen trueosfs:disc7/My notes/a; --name.txt\n").unwrap(),
            ["trueosfs:disc7/My notes/a; --name.txt"]);
        assert_eq!(open_paths("open /notes/end space .txt \n").unwrap(), ["/notes/end space .txt "]);
        assert!(open_paths("fs-scope trueosfs\n").unwrap().is_empty());
        assert!(open_paths("open \n").is_err());
        assert!(open_paths("open a\0b").is_err());
    }

    #[cfg(feature = "ide")]
    #[test]
    fn ide_project_and_database_are_literal_and_unambiguous() {
        assert_eq!(ide_project("fs-scope trueosfs\nide-project My Project\nide-db trueosfs:disc7/Source DB.redb\nopen lib.rs\n").unwrap(),
            Some(IdeLaunch { project: "My Project", database: "trueosfs:disc7/Source DB.redb", backend: trueos_ide::protocol::CompilerBackend::OdinSon, entry: "main.odin" }));
        assert_eq!(ide_project("ide-project example\n").unwrap().unwrap().database, trueos_ide::IDE_DATABASE_PATH);
        let rust = ide_project("ide-project rust\nide-compiler rustc-min\n").unwrap().unwrap();
        assert_eq!(rust.entry, "main.rs");
        assert_eq!(rust.backend, trueos_ide::protocol::CompilerBackend::RustcMin);
        assert_eq!(ide_project("open main.rs\n").unwrap(), None);
        for script in ["ide-project \n", "ide-project\n", "ide-project one\nide-project two\n",
            "ide-project a\0b\n", "ide-db only.redb\n", "ide-project p\nide-db one\nide-db two\n",
            "ide-project p\nide-entry ../escape.rs\n", "ide-project p\nide-compiler unknown\n",
            "ide-compiler rustc-min\n"] {
            assert!(ide_project(script).is_err(), "{script:?}");
        }
    }
}
