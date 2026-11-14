#!/usr/bin/env python3
"""Exercise Paper/img document producers with the production A4 raster facade.

Only font input and unused native ABI effects are host adapters. This does not
claim GPU presentation, printer discovery, transport, or a physical page.
"""

from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def item(source: str, signature: str) -> str:
    start = source.index(signature)
    opening = source.index("{", start)
    depth, end = 1, opening + 1
    while depth:
        depth += (source[end] == "{") - (source[end] == "}")
        end += 1
    return source[start:end] + "\n"


TESTS = r'''
#[cfg(test)]
mod app_print_tests {
    use super::*;
    use print2d::a4::{self, Page};
    fn pixel(page: &Page, x: u32, y: u32) -> &[u8] {
        let offset = (y as usize * a4::WIDTH as usize + x as usize) * 3;
        &page.rgb()[offset..offset+3]
    }
    fn at(page: &Page, x: f32, y: f32) -> &[u8] {
        pixel(page, a4::mm_to_px(x), a4::mm_to_px(y))
    }
    fn render(editor: &Editor, scale: u32) -> Page {
        paper_printing::render_page(editor, scale, |_,_| panic!("empty document needs no glyph")).unwrap()
    }
    #[test]
    fn a4_grid_keeps_five_mm_spacing_with_no_accumulated_pixel_drift() {
        let page=render(&Editor::new(1),1);
        assert_eq!(page.rgb().len(),2480*3508*3);
        for column in 0..=39 {
            assert_eq!(at(&page,7.5+column as f32*5.0,13.5),[190;3]);
        }
        for row in 0..=55 {
            assert_eq!(at(&page,9.5,11.0+row as f32*5.0),[190;3]);
        }
        assert_eq!(at(&page,10.0,13.5),[255;3]);
        assert_eq!(at(&page,2.0,13.5),[255;3]);
        let encoded=(a4::mm_to_px(202.5)-a4::mm_to_px(7.5)) as f32*25.4/300.0;
        assert!((encoded-195.0).abs()<25.4/300.0);
    }
    #[test]
    fn linepaper_and_whitepaper_keep_their_distinct_ruling_and_cell_spacing() {
        let lined=render(&Editor::with_mode(2,Mode::Linepaper),2);
        assert_eq!(at(&lined,9.5,13.5),[190;3]);
        assert_eq!(at(&lined,7.5,16.0),[255;3]);
        assert_eq!(at(&lined,9.5,23.5),[190;3]);
        let white=render(&Editor::with_mode(2,Mode::Whitepaper),2);
        assert!(white.rgb().iter().all(|byte|*byte==255));
    }
    #[test]
    fn compact_sheets_keep_their_physical_size_centered_on_a4() {
        let settings=script_settings("size 190x90\nstep 2").unwrap();
        let editor=Editor::with_settings(settings);
        let page=render(&editor,2);
        assert_eq!(at(&page,10.0,106.0),[190;3]);
        assert_eq!(at(&page,20.0,106.0),[190;3]);
        assert_eq!(at(&page,15.0,106.0),[255;3]);
        assert_eq!(at(&page,15.0,103.5),[190;3]);
        assert_eq!(at(&page,15.0,193.5),[190;3]);
        assert_eq!(at(&page,15.0,98.0),[255;3]);
    }
    #[test]
    fn typed_document_prints_actual_cells_once_per_glyph_without_cursor_or_em_border_erasure() {
        let mut editor=Editor::new(1);
        editor.type_char('A');editor.type_char('A');editor.cursor=2;
        let mut calls=0;
        let page=paper_printing::render_page(&editor,1,|scale,ch| {
            calls+=1;assert_eq!((scale,ch),(1,'A'));
            let mut bytes=2u32.to_le_bytes().to_vec();bytes.extend_from_slice(&2u32.to_le_bytes());
            bytes.extend_from_slice(&[0,0,0,255,255,255,255,255,255,255,255,255,255,255,255,255]);Ok(bytes)
        }).unwrap();
        assert_eq!(calls,1);
        assert_eq!(at(&page,9.5,12.0),[0;3]);
        assert_eq!(at(&page,14.5,12.0),[0;3]);
        assert_eq!(at(&page,20.0,13.5),[255;3]);
        assert_eq!(at(&page,10.0,16.0),[190;3]);
        assert!(paper_printing::render_page(&editor,1,|_,_|Ok(vec![0;7])).is_err());
    }
    #[test]
    fn img_prints_full_source_centered_with_ten_mm_margins_and_correct_alpha() {
        let image=Image{width:2,height:1,rgba:vec![255,0,0,255,0,0,255,128]};
        let page=img_printing::render_page(&image).unwrap();
        let margin=a4::mm_to_px(10.0);let width=a4::WIDTH-2*margin;
        let height=width/2;let top=(a4::HEIGHT-height)/2;
        assert_eq!(pixel(&page,margin+10,top+10),[255,0,0]);
        assert_eq!(pixel(&page,margin+width-10,top+10),[127,127,255]);
        assert_eq!(pixel(&page,margin-1,top+10),[255;3]);
        assert_eq!(pixel(&page,margin+10,top-1),[255;3]);
        assert_eq!(image.rgba,[255,0,0,255,0,0,255,128]);
    }
    #[test]
    fn portrait_image_fits_height_without_stretching_and_invalid_sources_fail() {
        let image=Image{width:1,height:2,rgba:vec![0,0,0,255,0,0,0,255]};
        let page=img_printing::render_page(&image).unwrap();
        let margin=a4::mm_to_px(10.0);let height=a4::HEIGHT-2*margin;
        let left=(a4::WIDTH-height/2)/2;
        assert_eq!(pixel(&page,left,margin),[0;3]);
        assert_eq!(pixel(&page,left-1,margin),[255;3]);
        assert!(img_printing::render_page(&Image{width:0,height:2,rgba:vec![]}).is_err());
    }
    #[test]
    fn explicit_control_p_prints_but_plain_letters_and_existing_named_keys_do_not() {
        for modifiers in [1,16,17] {
            for codepoint in [16,80,112] {
                let event=input::TrueosKeyboardOutputEvent{kind:1,modifiers,codepoint};
                assert!(paper_print_shortcut(&event));assert!(img_print_shortcut(&event));
            }
        }
        for event in [input::TrueosKeyboardOutputEvent{kind:1,modifiers:0,codepoint:112},
            input::TrueosKeyboardOutputEvent{kind:2,modifiers:0,codepoint:0},
            input::TrueosKeyboardOutputEvent{kind:1,modifiers:1,codepoint:113}] {
            assert!(!paper_print_shortcut(&event));assert!(!img_print_shortcut(&event));
        }
    }
}
'''


def main():
    facade = (ROOT / "vendor/trueos/src/lib.rs").read_text()
    input_constants = "\n".join(re.findall(r"    pub const KEYBOARD_[^;]+;", facade))
    library = (ROOT / "paper/src/lib.rs").read_text().replace("#![no_std]\n", "")
    printing = (ROOT / "vendor/trueos-v/src/vprint2d.rs").read_text()
    a4 = (ROOT / "vendor/trueos-v/src/vprint2d/a4.rs").read_text()
    a4 = a4.split('#[cfg(test)]\n#[path = "a4_tests.rs"]', 1)[0]
    with tempfile.TemporaryDirectory(prefix="trueos-a4-apps-") as temporary:
        work = Path(temporary)
        module_path = re.search(r'#\[path\s*=\s*"([^"\n]+)"\]\s*pub mod a4;', printing)
        a4_target = work / (module_path.group(1) if module_path else "print2d/a4.rs")
        a4_target.parent.mkdir(parents=True)
        a4_target.write_text(a4)
        (work / "print2d.rs").write_text(printing)
        source = '''extern crate self as trueos;
extern crate self as paper;
pub mod print2d;
pub mod vfs_async {
 pub async fn read_file(_: &[u8])->Result<Vec<u8>,i32>{panic!("no native font access")}
 pub fn block_on<F:core::future::Future>(_:F)->F::Output{panic!("no native font access")}
}
pub mod input { @CONSTANTS@
 pub struct TrueosKeyboardOutputEvent {pub kind:u8,pub modifiers:u8,pub codepoint:u32}
}
@LIBRARY@
#[path=@PAPER_PRINT@] mod paper_printing;
struct Image {width:u32,height:u32,rgba:Vec<u8>}
#[path=@IMG_PRINT@] mod img_printing;
@PAPER_SHORTCUT@
@IMG_SHORTCUT@
'''
        import json
        source = source.replace("@CONSTANTS@", input_constants).replace("@LIBRARY@", library)
        source = source.replace("@PAPER_PRINT@", json.dumps(str(ROOT / "paper/src/printing.rs")))
        source = source.replace("@IMG_PRINT@", json.dumps(str(ROOT / "img/src/printing.rs")))
        for app in ("paper", "img"):
            function = item((ROOT / f"{app}/src/main.rs").read_text(), "fn print_shortcut(")
            function = function.replace("fn print_shortcut(", f"fn {app}_print_shortcut(")
            source = source.replace(f"@{app.upper()}_SHORTCUT@", function)
        (work / "actual.rs").write_text(source + TESTS)
        for label, optimization in (("debug", "0"), ("optimized", "3")):
            binary = work / label
            subprocess.run(["rustc", "--edition=2024", "--test", "-Adead_code", "-C",
                            "opt-level=" + optimization, str(work / "actual.rs"), "-o", str(binary)],
                           check=True, cwd="/var/tmp")
            print("a4-app-pages profile=" + label, flush=True)
            subprocess.run([str(binary), "app_print_tests::"], check=True)


if __name__ == "__main__":
    main()
