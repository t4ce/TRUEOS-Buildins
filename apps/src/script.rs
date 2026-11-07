//! One-shot stream: `pull [--sh3 | --headless] NAME_OR_ID` or `install NAME_OR_ID`.
//! Install downloads, verifies and imports into AppDB without launching the app.
//! Empty lines and # comments are ignored; duplicates intentionally launch twice.
use alloc::{format, string::{String, ToString}, vec::Vec};
#[cfg(test)]
use alloc::vec;
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub selector: String,
    pub install_only: bool,
    pub start_script: String,
    pub new_shell3: bool,
    pub headless: bool,
}
/// `launch NAME -- COMMAND` forwards COMMAND as the downloaded app's start script.
/// `launch --sh3 NAME -- COMMAND` also creates a kernel-owned Shell3 window.
/// Plain `pull NAME` and `launch NAME` retain their existing behavior.
pub fn requests(script: &str) -> Vec<Result<Request, String>> {
    script
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            Some(match line.split_once(char::is_whitespace) {
                Some(("install", selector)) if !selector.trim().is_empty()
                    && !selector.trim().chars().any(char::is_whitespace)
                    && !selector.trim().starts_with('-') => Ok(Request {
                        selector: selector.trim().to_string(), install_only: true,
                        start_script: String::new(), new_shell3: false, headless: false,
                    }),
                Some(("pull" | "launch", value)) if !value.trim().is_empty() => {
                    let value = value.trim();
                    let (new_shell3, value) = match value.strip_prefix("--sh3") {
                        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => (true, rest.trim()),
                        _ => (false, value),
                    };
                    let (headless, value) = match value.strip_prefix("--headless") {
                        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => (true, rest.trim()),
                        _ => (false, value),
                    };
                    let (selector, command) = value
                        .trim()
                        .split_once(" -- ")
                        .map_or((value.trim(), ""), |(selector, command)| {
                            (selector.trim(), command.trim())
                        });
                    if (new_shell3 && headless) || selector.is_empty() || (value.contains(" -- ") && command.is_empty()) {
                        Err(format!("invalid request: {line}"))
                    } else {
                        Ok(Request {
                            selector: selector.to_string(),
                            install_only: false,
                            new_shell3,
                            headless,
                            start_script: if command.is_empty() {
                                String::new()
                            } else {
                                format!("{command}\n")
                            },
                        })
                    }
                }
                _ => Err(format!("invalid request: {line}")),
            })
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn plain(selector: &str) -> Result<Request, String> {
        Ok(Request {
            selector: selector.into(),
            install_only: false,
            start_script: String::new(),
            new_shell3: false, headless: false,
        })
    }
    #[test]
    fn forwards_frog_command_without_consuming_its_coordinates() {
        assert_eq!(
            requests("launch Frog -- weather -74.123456 40.987654\n"),
            vec![Ok(Request {
                selector: "Frog".into(),
                install_only: false,
                start_script: "weather -74.123456 40.987654\n".into(),
                new_shell3: false, headless: false,
            })]
        );
    }
    #[test]
    fn forwards_arbitrary_commands_and_preserves_other_requests() {
        let batch = requests(
            "pull cubes\nlaunch solara -- open https://example.com/a?q=x -- y\npull Frog\n",
        );
        assert_eq!(batch[0], plain("cubes"));
        assert_eq!(
            batch[1].as_ref().unwrap().start_script,
            "open https://example.com/a?q=x -- y\n"
        );
        assert_eq!(batch[2], plain("Frog"));
    }

    #[test]
    fn shell_destination_does_not_change_the_child_script() {
        let batch = requests("launch --sh3 Frog -- weather 13 51\npull --sh3 12\nlaunch --sh3\n");
        assert_eq!(batch[0], Ok(Request { selector: "Frog".into(), install_only: false, start_script: "weather 13 51\n".into(), new_shell3: true, headless: false }));
        assert_eq!(batch[1], Ok(Request { selector: "12".into(), install_only: false, start_script: String::new(), new_shell3: true, headless: false }));
        assert!(batch[2].is_err());
        assert!(!requests("launch Frog -- weather --sh3 51\n")[0].as_ref().unwrap().new_shell3);
    }

    #[test]
    fn drains_multiple_requests_in_order_and_keeps_duplicates() {
        assert_eq!(
            requests("# batch\npull gridpaper\n\nlaunch cubes\r\npull gridpaper\n"),
            vec![plain("gridpaper"), plain("cubes"), plain("gridpaper")]
        );
    }
    #[test]
    fn bad_lines_do_not_discard_later_requests() {
        let requests = requests("pull\nunknown cubes\npull 12\n");
        assert!(requests[0].is_err());
        assert!(requests[1].is_err());
        assert_eq!(requests[2], plain("12"));
    }
    #[test]
    fn headless_autostart_forwards_the_child_script() {
        assert_eq!(requests("pull --headless voxy\n"), vec![Ok(Request {
            selector: "voxy".into(), install_only: false, start_script: String::new(), new_shell3: false, headless: true,
        })]);
        let batch = requests("pull --headless Frog -- weather 13 51\n");
        assert_eq!(batch[0].as_ref().unwrap().start_script, "weather 13 51\n");
        assert!(batch[0].as_ref().unwrap().headless);
        assert!(requests("pull --sh3 --headless voxy\n")[0].is_err());
    }
    #[test]
    fn blank_script_is_an_empty_headless_batch() {
        assert!(requests("\n# empty\n").is_empty());
    }
    #[test]
    fn install_fetches_without_launch_options_or_commands() {
        assert_eq!(requests("install odin-son\n"), vec![Ok(Request {
            selector: "odin-son".into(), install_only: true, start_script: String::new(),
            new_shell3: false, headless: false,
        })]);
        for invalid in ["install", "install --headless odin-son", "install odin-son -- build", "install --sh3"] {
            assert!(requests(invalid)[0].is_err(), "{invalid}");
        }
    }
}
