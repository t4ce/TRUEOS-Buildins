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
#[derive(Default)]
pub struct IdeCli {
    project: Option<String>,
    database: Option<String>,
    compiler: Option<String>,
    entry: Option<String>,
}

#[cfg(feature = "ide")]
impl IdeCli {
    /// Consume one recognized option and its literal UTF-8 value.
    pub fn consume(&mut self, arg: &std::ffi::OsStr, args: &mut impl Iterator<Item = std::ffi::OsString>) -> io::Result<bool> {
        let slot = if arg == "--ide" {
            &mut self.project
        } else if arg == "--ide-db" {
            &mut self.database
        } else if arg == "--ide-compiler" {
            &mut self.compiler
        } else if arg == "--ide-entry" {
            &mut self.entry
        } else {
            return Ok(false);
        };
        if slot.is_some() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                format!("edit {} may only be specified once", arg.to_string_lossy())));
        }
        let value = args.next().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput,
            format!("edit {} requires a value", arg.to_string_lossy())))?;
        let value = value.into_string().map_err(|_| io::Error::new(io::ErrorKind::InvalidInput,
            "edit IDE options require UTF-8 values"))?;
        if matches!(value.as_str(), "--" | "--ide" | "--ide-db" | "--ide-compiler" | "--ide-entry"
            | "-g" | "--goto" | "-h" | "--help" | "-v" | "--version")
        {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                format!("edit {} requires a value before the next option", arg.to_string_lossy())));
        }
        if value.is_empty() || value.contains(['\0', '\r', '\n']) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "edit IDE options require nonempty literal values"));
        }
        *slot = Some(value);
        Ok(true)
    }

    pub fn launch(&self) -> io::Result<Option<IdeLaunch<'_>>> {
        ide_options(self.project.as_deref(), self.database.as_deref(),
            self.compiler.as_deref(), self.entry.as_deref())
    }
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
    ide_options(project, database, compiler, entry)
}

#[cfg(feature = "ide")]
fn ide_options<'a>(project: Option<&'a str>, database: Option<&'a str>, compiler: Option<&'a str>, entry: Option<&'a str>) -> io::Result<Option<IdeLaunch<'a>>> {
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
            "edit IDE options require --ide or ide-project")),
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

    #[cfg(feature = "ide")]
    #[test]
    fn ide_cli_uses_the_same_defaults_and_validation_as_launch_directives() {
        fn parse(args: &[&str]) -> io::Result<IdeCli> {
            let mut options = IdeCli::default();
            let mut args = args.iter().map(std::ffi::OsString::from);
            while let Some(arg) = args.next() {
                assert!(options.consume(&arg, &mut args)?);
            }
            options.launch()?;
            Ok(options)
        }
        let defaults = parse(&["--ide", "example"]).unwrap();
        assert_eq!(defaults.launch().unwrap(), ide_project("ide-project example\n").unwrap());
        let explicit = parse(&["--ide-compiler", "rustc-min", "--ide", "My project",
            "--ide-entry", "src/main.rs", "--ide-db", "/Source DB.redb"]).unwrap();
        assert_eq!(explicit.launch().unwrap(), ide_project("ide-project My project\nide-compiler rustc-min\nide-entry src/main.rs\nide-db /Source DB.redb\n").unwrap());
        for args in [vec!["--ide"], vec!["--ide", ""], vec!["--ide", "one", "--ide", "two"],
            vec!["--ide-db", "only.redb"], vec!["--ide", "--ide-compiler", "rustc-min"],
            vec!["--ide", "p", "--ide-entry", "../main.rs"],
            vec!["--ide", "p", "--ide-compiler", "unknown"],
            vec!["--ide", "p\nide-db injected.redb"]] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
}
