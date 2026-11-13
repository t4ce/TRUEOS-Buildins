//! Feed the actual Apps output to the production Shell3 terminal model.
extern crate alloc;
use crossterm::style::Color;
use trueos_terminal::{Cell, Terminal, TerminalColor};

#[path = "../src/catalog.rs"]
mod catalog;
#[path = "../src/mouse.rs"]
mod mouse;
#[path = "../../vendor/trueos/src/terminal_style.rs"]
mod terminal_style;
const CONTROL_BACKGROUND: Color = Color::Rgb {
    r: terminal_style::CONTROL_BACKGROUND_RGB[0],
    g: terminal_style::CONTROL_BACKGROUND_RGB[1],
    b: terminal_style::CONTROL_BACKGROUND_RGB[2],
};
#[path = "../src/view.rs"]
mod view;

fn apps(count: usize) -> Vec<catalog::OnlineApp> {
    // The test runner may set NO_COLOR; exercise the app's actual colored output.
    crossterm::style::force_color_output(true);
    let html = (0..count).map(|id| format!("<li><a href=\"app{id}.bp\">app{id}.bp</a></li>\n")).collect::<String>();
    let apps = catalog::parse(&html);
    assert_eq!(apps.len(), count);
    apps
}

fn changes(before: &[Cell], after: &[Cell]) -> Vec<usize> {
    before.iter().zip(after).enumerate().filter_map(|(i, (a,b))| (a != b).then_some(i)).collect()
}

#[test]
fn hover_commits_only_four_background_cells_without_clearing_or_rewriting_text() {
    for width in [80,81,82,180] {
        let apps = apps(38);
        let mut view = view::View::default();
        let mut terminal = Terminal::new(width as usize,24);
        let mut bytes = Vec::new();
        view.draw_to(&mut bytes,(width,24),&apps,"",0,"Ready").unwrap();
        terminal.feed(&bytes);
        for selected in 1..38 {
            let before = terminal.cells().to_vec();
            bytes.clear();
            view.draw_to(&mut bytes,(width,24),&apps,"",selected,"Ready").unwrap();
            assert!(!bytes.windows(4).any(|b| b == b"\x1b[2J"));
            assert!(!bytes.windows(3).any(|b| b == b"app"));
            assert!(bytes.len() < 120, "hover must stay bounded: {}",bytes.len());
            for (i, byte) in bytes.iter().enumerate() {
                terminal.feed(core::slice::from_ref(byte));
                if i + 1 < bytes.len() { assert_eq!(terminal.cells(), before); }
            }
            let changed = changes(&before,terminal.cells());
            assert_eq!(changed.len(),4);
            for i in changed {
                assert_eq!(before[i].glyph, ' ');
                assert_eq!(terminal.cells()[i].glyph, ' ');
                assert_eq!(before[i].style.foreground,terminal.cells()[i].style.foreground);
            }
            let highlighted = terminal.cells().iter().filter(|cell| cell.style.background == TerminalColor::Indexed(6)).count();
            assert_eq!(highlighted,2);
        }
        bytes.clear();
        view.draw_to(&mut bytes,(width,24),&apps,"",37,"Ready").unwrap();
        assert!(bytes.is_empty(), "same selection must produce no output");
    }
}

#[test]
fn status_changes_leave_the_list_untouched_and_erase_old_message_tails() {
    let apps = apps(38);
    let mut view = view::View::default();
    let mut terminal = Terminal::new(80,24);
    let mut bytes = Vec::new();
    view.draw_to(&mut bytes,(80,24),&apps,"",0,"Downloading app0…").unwrap();
    terminal.feed(&bytes);
    let before = terminal.cells().to_vec();
    bytes.clear();
    view.draw_to(&mut bytes,(80,24),&apps,"",0,"Ready").unwrap();
    terminal.feed(&bytes);
    assert!(changes(&before,terminal.cells()).iter().all(|i| *i >= 80*23));
    let status = terminal.cells()[80*23..].iter().map(|c| c.glyph).collect::<String>();
    assert_eq!(status.trim_end(),"Ready");
}

#[test]
fn page_search_and_resize_repaint_the_correct_list_with_one_marker() {
    let apps = apps(80);
    let mut view = view::View::default();
    let mut bytes = Vec::new();
    for (size,query,selected) in [((80,24),"",0),((80,24),"",51),((80,24),"79",0),((82,25),"79",0),((82,25),"no-match",0)] {
        bytes.clear();
        view.draw_to(&mut bytes,size,&apps,query,selected,"Ready").unwrap();
        assert!(bytes.windows(4).any(|b| b == b"\x1b[2J"));
        let mut terminal = Terminal::new(size.0 as usize,size.1 as usize);
        terminal.feed(&bytes);
        let highlighted = terminal.cells().iter().filter(|cell| cell.style.background == TerminalColor::Indexed(6)).count();
        assert_eq!(highlighted,if query == "no-match" {0} else {2});
        if query == "79" {
            let row = terminal.cells()[size.0 as usize*5..size.0 as usize*6].iter().map(|c| c.glyph).collect::<String>();
            assert!(row.starts_with(" 79  app79"),"{row}");
        }
    }
}
