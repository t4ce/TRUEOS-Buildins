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
}
