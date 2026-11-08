#!/usr/bin/env python3
"""Host checks for the production OS release parser and confirmation state machine."""
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def test_releases():
    main = (ROOT / "os/src/main.rs").read_text()
    model = main[main.index("#[derive(Clone)]\nstruct Disk"):main.index("\nfn main()")]
    source = (ROOT / "os/src/releases.rs").read_text()
    release_type = source[source.index("pub struct Release"):source.index("\nstruct Download")]
    parser = source[source.index("pub fn parse"):]
    tests = r'''
#[cfg(test)] mod tests {
    use super::*;
    fn entry(version: &str) -> serde_json::Value {
        serde_json::json!({"tag_name": format!("v{version}"), "draft":false, "assets":[
            {"name":"Source.zip", "browser_download_url":"https://example.com/source.zip"},
            {"name":format!("TrueOS-{version}.lz4"), "browser_download_url":format!("https://github.com/t4ce/TRUEOS/releases/download/v{version}/TrueOS-{version}.lz4")}
        ]})
    }
    #[test] fn release_order_and_confirmation() {
        let data = serde_json::to_vec(&vec![entry("3"),entry("2"),entry("1"),entry("0")]).unwrap();
        let mut app = App::new(vec![], vec![]);
        app.releases = releases::parse(&data).unwrap();
        assert_eq!(app.releases.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), vec!["v3","v2","v1"]);
        app.selected = 1;
        assert!(app.activate().is_none());
        assert!(matches!(app.screen, Screen::Releases));
        assert!(app.activate().is_none());
        assert!(matches!(app.screen, Screen::Confirm(Action::Release(0))));
        assert!(!app.back());
        app.selected = 2;
        app.activate();
        app.selected = 1;
        assert_eq!(app.activate().unwrap(), "os:update:github:https://github.com/t4ce/TRUEOS/releases/download/v1/TrueOS-1.lz4");
    }
    #[test] fn nightly_and_failure() {
        assert!(releases::parse(br#"{"message":"rate limited"}"#).is_err());
        assert!(releases::parse(b"[]").is_err());
        assert!(releases::parse(b"invalid").is_err());
        let mut app = App::new(vec![],vec![]);
        app.screen = Screen::Releases;
        app.activate();
        assert!(matches!(app.screen, Screen::Confirm(Action::LiveUpdate)));
        app.selected = 1;
        assert_eq!(app.activate().unwrap(), "os:update:live");
    }
    #[test] fn rejects_wrong_assets_and_drafts() {
        let mut wrong = entry("3");
        wrong["assets"][1]["browser_download_url"] = serde_json::json!("https://example.com/TrueOS-3.lz4");
        let mut draft = entry("2");
        draft["draft"] = serde_json::json!(true);
        let data = serde_json::to_vec(&vec![wrong,draft,entry("1")]).unwrap();
        let releases = releases::parse(&data).unwrap();
        assert_eq!(releases.len(),1);
        assert_eq!(releases[0].version,"v1");
    }
}
'''
    with tempfile.TemporaryDirectory(prefix="trueos-os-test-") as directory:
        path = Path(directory)
        (path / "Cargo.toml").write_text('[package]\nname="os-release-tests"\nversion="0.0.0"\nedition="2024"\n[dependencies]\nserde_json="1"\n[workspace]\n')
        (path / "src").mkdir()
        (path / "src/lib.rs").write_text('#![allow(dead_code)]\nextern crate alloc;\nuse alloc::{format,string::String,vec::Vec};\nmod releases { use super::*;\n' + release_type + parser + '\n}\n' + model + tests)
        subprocess.run(["cargo", "test", "--offline", "--quiet", "--target", "x86_64-unknown-linux-gnu"], cwd=path, check=True)


if __name__ == "__main__":
    test_releases()
