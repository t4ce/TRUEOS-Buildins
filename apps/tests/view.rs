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
            assert!(row.starts_with("  app79"),"{row}");
        }
    }
}

#[test]
fn search_matches_names_without_interpreting_numeric_ids() {
    let apps = apps(25);
    assert_eq!(view::visible(&apps,"1"),vec![1,10,11,12,13,14,15,16,17,18,19,21]);
    assert_eq!(view::visible(&apps,"APP2"),vec![2,20,21,22,23,24]);
}

#[test]
fn fetched_names_and_legend_share_gray_and_leave_other_names_unchanged() {
    let apps = apps(2);
    let mut view = view::View::default();
    view.set_fetched(["app1.bp".into()].into_iter().collect());
    let mut bytes = Vec::new();
    view.draw_to(&mut bytes,(80,24),&apps,"",0,"Ready").unwrap();
    let mut terminal = Terminal::new(80,24);
    terminal.feed(&bytes);
    let fetched_x = mouse::cell(80,1).0 as usize + 2;
    let cells = terminal.cells();
    assert_eq!(cells[5*80+fetched_x].glyph,'a');
    assert_eq!(cells[5*80+fetched_x].style.foreground,TerminalColor::Indexed(7));
    assert_eq!(cells[22*80].style.foreground,TerminalColor::Indexed(7));
    assert_eq!(cells[5*80+2].style.foreground,TerminalColor::Default);
    assert_eq!(cells[22*80..22*80+7].iter().map(|c| c.glyph).collect::<String>(),"fetched");
    // Shell3 currently ignores bold weight; ensure the app still emits it for
    // terminals that render it, for both the name and the legend.
    assert_eq!(bytes.windows(4).filter(|b| *b == b"\x1b[1m").count(),2);
}

#[test]
fn hover_hash_is_centered_and_loses_characters_in_the_middle_on_narrow_windows() {
    let mut apps = apps(2);
    apps[0].sha256 = "daffe2fc8d3bab24bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbba04a1c264b0e8c55".into();
    apps[1].sha256 = "c".repeat(64);
    for width in [20,27,40,60,80] {
        let mut view = view::View::default();
        let mut terminal = Terminal::new(width as usize,24);
        let mut bytes = Vec::new();
        view.draw_to(&mut bytes,(width,24),&apps,"",0,"Ready").unwrap();
        terminal.feed(&bytes);
        let header = terminal.cells()[..width as usize].iter().map(|c| c.glyph).collect::<String>();
        assert!(header.starts_with("TRUEOS APPS"));
        let available = width.saturating_sub(26).min(33) as usize;
        if available > 0 {
            let hash = header.chars().skip((width as usize-available)/2).take(available).collect::<String>();
            assert_eq!(hash.chars().count(),available);
            assert!(hash.contains('…'));
            if width >= 60 { assert_eq!(hash,"daffe2fc8d3bab24…a04a1c264b0e8c55"); }
        }
        let before = terminal.cells().to_vec();
        bytes.clear();
        view.draw_to(&mut bytes,(width,24),&apps,"",1,"Ready").unwrap();
        assert!(!bytes.windows(4).any(|b| b == b"\x1b[2J"));
        for (i,byte) in bytes.iter().enumerate() {
            terminal.feed(core::slice::from_ref(byte));
            if i+1 < bytes.len() { assert_eq!(terminal.cells(),before); }
        }
        assert!(changes(&before,terminal.cells()).iter().all(|i| *i < width as usize || *i / width as usize == 5));
    }
}
