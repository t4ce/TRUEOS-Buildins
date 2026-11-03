#!/usr/bin/env python3
"""End-to-end host workflow checks; no network or TRUEOS runtime."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
import xml.etree.ElementTree as ET

import sandbox


def svg_text(path):
    return " ".join(node.text or "" for node in ET.parse(path).iter("{http://www.w3.org/2000/svg}text"))


class SandboxWorkflow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.executable = sandbox.binary()

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="hull-sandbox-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.capture = self.root / "capture"
        self.capture.mkdir()
        self.out = self.root / "run"

    def page(self, html, resources=None):
        (self.capture / "page.html").write_text(html)
        sandbox.store(self.capture / "capture.json", {
            "version": 1, "url": "https://fixture.test/page.html", "resources": resources or {},
        })

    def run_page(self, *arguments):
        return sandbox.execute(self.executable, self.capture, self.out,
                               ["--turn-ms", "0", *arguments])

    def test_cookie_snapshot_is_fresh_and_saved_only_with_svg(self):
        self.page("""<p>cookies</p><script>
          if(document.cookie!=='')throw Error('old cookie');
          document.cookie='a=one; Path=/'; localStorage.setItem('a','stored');
          document.addEventListener('DOMContentLoaded',()=>Promise.resolve().then(()=>document.cookie='a=ready'));
        </script>""")
        report = self.run_page()
        self.assertEqual(report['scripts'][0]['status'], 'executed')
        self.assertEqual(json.loads((self.out / 'storage.json').read_text()), [['cookie:a', 'ready'], ['localStorage:a', 'stored']])
        self.page("<p>fresh</p><script>if(document.cookie!=='')throw Error('restored')</script>")
        report = self.run_page()
        self.assertEqual(report['scripts'][0]['status'], 'executed')
        self.assertEqual(json.loads((self.out / 'storage.json').read_text()), [])
        self.run_page('--no-svg')
        self.assertFalse((self.out / 'storage.json').exists())

    def test_video_play_records_only_latest_url_without_native_playback(self):
        self.page("""<video id=a src='/old.mp4'></video><video id=b><source src='/new.mp4'></video>
        <script>
          document.getElementById('a').play().catch(()=>{});
          document.getElementById('b').play().catch(()=>{});
        </script>""")
        report = self.run_page()
        self.assertEqual(report['scripts'][0]['status'], 'executed')
        self.assertEqual(len(report['video_requests']), 1)
        self.assertTrue(report['video_requests'][0]['url'].endswith('/new.mp4'))

    def test_first_style_write_reaches_svg_and_later_assignment_is_ignored(self):
        self.page("""<p>styled</p><script>
          const s=document.querySelector('p').style;
          s.color='#123456'; s.color='#abcdef';
          s.width='100px'; s.width='200px';
          if(s.width!=='100px')throw Error('write not frozen');
        </script>""")
        report = self.run_page()
        self.assertEqual(report['scripts'][0]['status'], 'executed')
        svg = (self.out / 'layout.svg').read_text()
        self.assertIn('#123456', svg)
        self.assertNotIn('#abcdef', svg)

    def test_real_dom_css_and_svg_with_independent_script_failures(self):
        (self.capture / "style.css").write_text("#message{color:#123456}")
        self.page("""<link rel=stylesheet href=style.css><p id=message>before</p>
<script>document.querySelector('p').textContent='initialized '+window.innerHeight;console.log('first');</script>
<script>fetch('/unsupported')</script>
<script>
  const markup = <broken>;
</script>
<script>console.log('continued')</script>""", {
            "https://fixture.test/style.css": {"status": "ok", "path": "style.css", "kind": "css"},
        })
        report = self.run_page()
        self.assertEqual([item["status"] for item in report["scripts"]],
                         ["executed", "failed", "failed", "executed"])
        self.assertIn("fetch", report["scripts"][1]["error"])
        self.assertIn("SyntaxError", report["scripts"][2]["error"])
        source = (self.out / report["scripts"][2]["file"]).read_text()
        self.assertIn("<broken>", source.splitlines()[1])
        svg = (self.out / "layout.svg").read_text()
        self.assertEqual(svg_text(self.out / "layout.svg"), "initialized 720")
        self.assertIn('#123456', svg)
        self.assertIn('continued', (self.out / "console.log").read_text())
        self.assertEqual(report["css_requests"], ["https://fixture.test/style.css"])
        self.assertTrue((self.out / "trace.jsonl").exists())

    def test_external_sources_only_run_with_explicit_capture_experiment(self):
        (self.capture / "library.js").write_text("function library(){document.querySelector('p').textContent='library ready'}")
        self.page("<p>before</p><script src=library.js></script><script>library()</script>", {
            "https://fixture.test/library.js": {"status": "ok", "path": "library.js", "kind": "js"},
        })
        report = self.run_page()
        self.assertEqual([item["status"] for item in report["scripts"]], ["skipped", "failed"])
        report = self.run_page("--external")
        self.assertEqual([item["status"] for item in report["scripts"]], ["executed", "executed"])
        self.assertEqual(svg_text(self.out / "layout.svg"), "library ready")
        self.assertTrue(report["experiments"])

    def test_native_jquery_initialization_reaches_the_svg_snapshot(self):
        self.page("""<style>p.ready{color:#13579b}</style><p id=message>before</p>
<script>
  $('#message').text('native ready').addClass('ready').attr('data-native','yes');
  console.log($.param({status:'native ready',items:[1,2]}));
</script>""")
        report = self.run_page()
        self.assertEqual([item["status"] for item in report["scripts"]], ["executed"])
        self.assertEqual(svg_text(self.out / "layout.svg"), "native ready")
        self.assertIn('#13579b', (self.out / "layout.svg").read_text())
        self.assertIn('status=native%20ready', (self.out / "console.log").read_text())

    def test_script_created_stylesheet_participates_in_first_snapshot(self):
        (self.capture / "late.css").write_text("p{color:#2468ac}")
        self.page("""<p id=message>before</p><script>
const link=document.createElement('link');link.rel='stylesheet';link.href='late.css';
document.head.appendChild(link);document.querySelector('p').textContent='linked stylesheet';
if(document.getElementsByTagName('link')[0]!==link)throw Error('inserted link missing');
</script>""", {
            "https://fixture.test/late.css": {"status":"ok","path":"late.css","kind":"css"},
        })
        report = self.run_page()
        self.assertEqual([item["status"] for item in report["scripts"]], ["executed"])
        self.assertEqual(svg_text(self.out / "layout.svg"), "linked stylesheet")
        self.assertIn('#2468ac', (self.out / "layout.svg").read_text())
        self.assertEqual(report["css_requests"], ["https://fixture.test/late.css"])

    def test_larger_css_and_js_are_admitted_and_dom_ready_runs_before_svg(self):
        (self.capture / "large.css").write_text("/*" + " " * (600 * 1024) + "*/p{color:#2468ac}")
        (self.capture / "large.js").write_text("/*" + " " * (1200 * 1024) + "*/" + """
document.addEventListener('DOMContentLoaded',function(event){
  if(event.target!==document||document.readyState!=='interactive')throw Error('ready state');
  document.querySelector('p').textContent='DOMContentLoaded';
  Promise.resolve().then(()=>document.querySelector('p').textContent+=' promise');
});""")
        self.page("<link rel=stylesheet href=large.css><p>before</p><script src=large.js></script>", {
            "https://fixture.test/large.css":{"status":"ok","path":"large.css","kind":"css"},
            "https://fixture.test/large.js":{"status":"ok","path":"large.js","kind":"js"},
        })
        report = self.run_page("--modules")
        self.assertEqual(report["scripts"][0]["status"], "executed")
        self.assertEqual(svg_text(self.out / "layout.svg"), "DOMContentLoaded promise")
        self.assertIn('#2468ac', (self.out / "layout.svg").read_text())

    def test_modules_use_captured_fallback_and_render_the_result(self):
        (self.capture / "fallback.js").write_text("export const answer='CDN fallback ready';")
        self.page("<p>before</p><script>import('demo@1.0.0').then(m=>document.querySelector('p').textContent=m.answer)</script>", {
            "https://cdn.jsdelivr.net/npm/demo@1.0.0/+esm": {"status":"ok","path":"fallback.js","kind":"module"},
        })
        report = self.run_page("--modules")
        self.assertEqual(svg_text(self.out / "layout.svg"), "CDN fallback ready")
        self.assertTrue(any(e["status"] == "fallback" for e in report["resources"]))
        self.assertEqual(report["css_requests"], [])
        self.assertIn("esm.sh/demo@1.0.0", (self.out / "console.log").read_text())

    def test_svelte_shaped_current_script_and_dynamic_graph_are_offline(self):
        (self.capture / "start.js").write_text("export function init(){globalThis.initialized=true}export function start(app,element){if(!initialized||element.id!=='container')throw Error('bootstrap');document.querySelector('p').textContent=app.answer}")
        (self.capture / "app.js").write_text("export const answer='module bootstrap ready';")
        self.page("""<div id=container><p>before</p><script>{
const element=document.currentScript.parentElement;
import('/_app/start.js').then(async kit=>{kit.init();const app=await import('/_app/app.js');kit.start(app,element)});
}</script></div>""", {
            "https://fixture.test/_app/start.js": {"status":"ok","path":"start.js","kind":"module"},
            "https://fixture.test/_app/app.js": {"status":"ok","path":"app.js","kind":"module"},
        })
        report = self.run_page("--modules")
        self.assertEqual(svg_text(self.out / "layout.svg"), "module bootstrap ready")
        self.assertFalse(any("esm.sh" in e["url"] for e in report["resources"]))

    def test_native_url_is_available_in_a_dynamic_module(self):
        (self.capture / "url-module.js").write_text("export function resolve(){const u=new URL('./asset.js',import.meta.url);u.searchParams.set('q','native URL');return u.href}")
        self.page("<p>before</p><script>import('./url-module.js').then(m=>document.querySelector('p').textContent=m.resolve())</script>", {
            "https://fixture.test/url-module.js": {"status":"ok","path":"url-module.js","kind":"module"},
        })
        report = self.run_page("--modules")
        self.assertEqual(svg_text(self.out / "layout.svg"), "https://fixture.test/asset.js?q=native+URL")
        self.assertEqual(report["job_errors"], [])

    def test_capture_module_graph_uses_the_parser_without_executing_bodies(self):
        from unittest.mock import patch
        html = b"<p>before</p><script type=module>import 'demo@1.0.0';</script>"
        bodies = {
            "https://fixture.test/page": html,
            "https://esm.sh/demo@1.0.0?target=es2022&bundle": b"export {answer} from './answer.js';globalThis.captureMustNotExecute=1;",
            "https://esm.sh/answer.js": b"throw Error('capture executed module body');export const answer=42;",
        }
        def fetch(url, limit, **kwargs):
            self.assertLessEqual(len(bodies[url]), limit)
            return bodies[url], url, "text/javascript"
        args = sandbox.parser().parse_args(["capture", "https://fixture.test/page", str(self.capture), "--no-css", "--module", "demo@1.0.0"])
        with patch.object(sandbox, "fetch", side_effect=fetch):
            sandbox.capture_page(args, self.executable)
        manifest = json.loads((self.capture / "capture.json").read_text())
        self.assertEqual(manifest["resources"]["https://esm.sh/answer.js"]["module_status"], "ready")
        self.assertTrue(all(value["status"] == "ok" for value in manifest["resources"].values()))

    def test_replacement_isolation_static_baseline_and_js_only_cleanup(self):
        self.page("<p>before</p><script>throw Error('first')</script><script>throw Error('second')</script>")
        report = self.run_page("--no-js")
        self.assertIn(">before</text>", (self.out / "layout.svg").read_text())
        node = report["scripts"][1]["nodes"][0]
        replacement = self.root / "replacement.js"
        replacement.write_text("document.querySelector('p').textContent='replacement'")
        report = self.run_page("--only-node", str(node), "--replace", f"{node}={replacement}")
        self.assertEqual([item["status"] for item in report["scripts"]], ["excluded", "executed"])
        self.assertIn(">replacement</text>", (self.out / "layout.svg").read_text())
        self.run_page("--no-svg")
        self.assertFalse((self.out / "layout.svg").exists())

    def test_ui_poll_delay_does_not_gate_native_worker_progress(self):
        self.page("<p>ready</p>" + "<script>globalThis.count=(globalThis.count||0)+1</script>" * 30)
        for turn_ms in ("20", "0"):
            report = self.run_page("--turn-ms", turn_ms, "--no-svg")
            self.assertFalse(report["initialization"]["capped"])
            self.assertEqual(report["initialization"]["remaining_sources"], 0)
            self.assertTrue(all(item["status"] == "executed" for item in report["scripts"]))

    def test_import_and_show_need_no_live_network_and_resources_cannot_escape(self):
        html = self.root / "saved.html"
        html.write_text("<p>saved</p><script>\nconst value = <bad>;\n</script>")
        imported = self.root / "imported"
        subprocess.run([sys.executable, str(sandbox.HERE / "sandbox.py"), "capture",
                        "https://fixture.test/page.html", str(imported), "--html", str(html), "--no-css"],
                       check=True, capture_output=True, text=True)
        inventory = json.loads((imported / "inspection/scripts.json").read_text())
        node = inventory[0]["nodes"][0]
        shown = subprocess.run([sys.executable, str(sandbox.HERE / "sandbox.py"), "show", str(imported),
                                "--node", str(node), "--line", "2"], check=True, capture_output=True, text=True)
        self.assertIn("<bad>", shown.stdout)
        self.page("<p>safe</p>", {"https://fixture.test/secret.css": {
            "status": "ok", "path": "../saved.html", "kind": "css",
        }})
        rejected = subprocess.run([str(self.executable), "--input", str(self.capture), "--out", str(self.out)],
                                  capture_output=True, text=True)
        self.assertNotEqual(rejected.returncode, 0)
        self.assertIn("escapes capture directory", rejected.stderr)

    def test_watch_replays_an_edited_replacement_without_restarting_the_host(self):
        self.page("<p>before</p><script>throw Error('replace me')</script>")
        inventory = self.run_page("--no-js")["scripts"]
        node = inventory[0]["nodes"][0]
        replacement = self.root / "working.js"
        replacement.write_text("document.querySelector('p').textContent='first'")
        watched_out = self.root / "watched"
        log = self.root / "watch.log"
        with log.open("w") as output:
            process = subprocess.Popen([sys.executable, str(sandbox.HERE / "sandbox.py"), "watch",
                                        str(self.capture), "--out", str(watched_out), "--turn-ms", "0",
                                        "--replace", f"{node}={replacement}"], stdout=output, stderr=output)
            try:
                deadline = time.monotonic() + 15
                while "Watching captured" not in log.read_text():
                    self.assertIsNone(process.poll(), log.read_text())
                    self.assertLess(time.monotonic(), deadline, log.read_text())
                    time.sleep(0.05)
                self.assertEqual(svg_text(watched_out / "layout.svg"), "first")
                replacement.write_text("document.querySelector('p').textContent='second'")
                while True:
                    self.assertIsNone(process.poll(), log.read_text())
                    self.assertLess(time.monotonic(), deadline, log.read_text())
                    try:
                        if svg_text(watched_out / "layout.svg") == "second":
                            break
                    except (FileNotFoundError, ET.ParseError):
                        pass
                    time.sleep(0.05)
            finally:
                process.terminate()
                process.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
