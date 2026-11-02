use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::VecDeque;
use trueos::ui4_scene::output_dimensions;
use trueos::ui4_winit::{Damage, Error, HullTextRun, SceneTarget};
use trueos::{logl, netfs, vshell};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ButtonSource, ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::platform::trueos::{ActiveEventLoopExtTrueOS, WindowExtTrueOS};
use winit::window::{Window, WindowAttributes, WindowId};

mod actual_bg_quad;
mod border_geometry;
mod buttons;
mod chrome;
mod console;
mod css;
mod favicon;
mod images;
mod javascript;
mod javascript_input;
mod layout;
mod links;
#[path = "../xxx/mod.rs"]
mod pink_quad_dbg;
mod scripts;
mod shellio;
mod snapshot;
mod viewport;

#[derive(Default)]
struct Hull {
    window: Option<Box<dyn Window>>,
    target: Option<SceneTarget>,
    pending_publish: bool,
    image_frame_recovery: bool,
    quads_dirty: bool,
    quad_pass: actual_bg_quad::QuadPass,
    background_quads: Vec<layout::BackgroundQuad>,
    canvas_color: [u8; 4],
    border_quads: Vec<layout::BorderQuad>,
    button_boxes: Vec<layout::ButtonBox>,
    borders: Option<pink_quad_dbg::BorderPass>,
    chrome_quads: Option<(pink_quad_dbg::Quads, (u32, u32))>,
    redraw_reason: &'static str,
    frame_counter: shellio::FrameCounter,
    quads: Option<pink_quad_dbg::Quads>,
    text_runs: Vec<layout::TextRun>,
    links: links::Links,
    rectangles: Vec<pink_quad_dbg::Rect>,
    layout_extent: (u32, u32),
    viewport: viewport::Viewport,
    shift_scroll: bool,
    download: Option<u32>,
    html: Option<Vec<u8>>,
    document: Option<rp5::Document>,
    css: css::Css,
    scripts: scripts::Scripts,
    javascript: Option<javascript::Runtime>,
    layout_attempted: bool,
    generation: u64,
    layout_generation: Option<u64>,
    published_generation: Option<u64>,
    images: images::Images,
    snapshot: snapshot::Snapshot,
    page_url: String,
    console: Option<console::Console>,
    queued_urls: VecDeque<QueuedNavigation>,
    pending_submissions: VecDeque<DownloadSubmission>,
}

struct QueuedNavigation {
    url: String,
    new_shell: bool,
}

struct DownloadSubmission {
    url: String,
    filename_hint: Option<String>,
    future: Pin<Box<dyn Future<Output = Result<(), i32>>>>,
}

impl Hull {
    fn open_link(&mut self, url: String, new_shell: bool) {
        if links::download_url(&url) {
            self.submit_download(url, None);
        } else if new_shell {
            self.launch_navigation(url);
        } else {
            self.start_page(url);
        }
    }

    fn launch_navigation(&mut self, url: String) {
        match links::launch_script(&url)
            .map_err(str::to_owned)
            .and_then(|script| {
                vshell::launch_with_destination(
                    "hull",
                    &script,
                    vshell::LaunchDestination::NewShell3,
                )
                .map_err(|code| format!("could not launch a new Hull: {code}"))
            }) {
            Ok(()) => shellio::linef(format_args!("hull: opening {url} in a new Hull")),
            Err(error) => shellio::linef(format_args!("hull: {error}")),
        }
    }

    fn submit_download(&mut self, url: String, filename_hint: Option<String>) {
        self.submit_received(url, filename_hint, None);
    }

    fn submit_received(
        &mut self,
        url: String,
        filename_hint: Option<String>,
        body: Option<Vec<u8>>,
    ) {
        if self.pending_submissions.len() >= 32 {
            shellio::line("hull: Trauma submission queue is full");
            return;
        }
        let action = links::LinkAction::Download { url, filename_hint };
        let mut request = match links::download_request(&action) {
            Ok(request) => request,
            Err(error) => {
                shellio::linef(format_args!("hull: {error}"));
                return;
            }
        };
        let links::LinkAction::Download { url, filename_hint } = action else {
            unreachable!();
        };
        let endpoint: &[u8] = if let Some(body) = body {
            request.push(0);
            request.extend_from_slice(&body);
            b"vFile:hulldownloads/received"
        } else {
            b"vFile:hulldownloads/submit"
        };
        let future =
            Box::pin(async move { trueos::async_fs::write_file(endpoint, &request).await });
        self.pending_submissions.push_back(DownloadSubmission {
            url,
            filename_hint,
            future,
        });
    }

    fn poll_download_submission(&mut self) {
        let Some(submission) = self.pending_submissions.front_mut() else {
            return;
        };
        let mut context = Context::from_waker(Waker::noop());
        let result = match submission.future.as_mut().poll(&mut context) {
            Poll::Pending => return,
            Poll::Ready(result) => result,
        };
        let submission = self.pending_submissions.pop_front().unwrap();
        match result {
            Ok(()) => shellio::linef(format_args!(
                "hull: download accepted by Trauma: {}{}",
                submission.url,
                submission
                    .filename_hint
                    .as_deref()
                    .map(|hint| format!(" as {hint}"))
                    .unwrap_or_default()
            )),
            Err(code) => shellio::linef(format_args!(
                "hull: download submission failed ({code}): {}",
                submission.url
            )),
        }
    }

    fn create_window(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (width, height) = output_dimensions().map_err(|error| format!("{error:?}"))?;
        let (content_width, content_height) = chrome::INITIAL_CONTENT;
        let frame_height = content_height + chrome::HEIGHT;
        let attributes = WindowAttributes::default()
            .with_surface_size(PhysicalSize::new(content_width, frame_height))
            .with_position(PhysicalPosition::new(
                ((i64::from(width) - i64::from(content_width)) / 2) as i32,
                ((i64::from(height) - i64::from(frame_height)) / 2) as i32,
            ));
        let window = event_loop.create_streaming_window(attributes)?;
        javascript_input::enable(window.as_ref());
        let size = window.surface_size();
        self.target = Some(
            SceneTarget::for_window(window.trueos_window_id(), size.width, size.height)
                .map_err(|error| format!("{error:?}"))?,
        );
        self.viewport
            .resize_with_origin(size.width, size.height, 0, chrome::HEIGHT);
        self.quads_dirty = true;
        self.redraw_reason = "initial";
        window.request_redraw();
        self.window = Some(window);
        shellio::line("hull: enter an HTTPS URL to download, parse and display it");
        shellio::line(
            "hull: scroll with wheel, arrows or Page Up/Down; Home/End; Shift+wheel pans sideways; F2 toggles actualBgQuad / pinkQuadDbg",
        );
        Ok(())
    }

    fn start_page(&mut self, url: String) {
        let download = netfs::fetch_bytes_happy_eyeballs(url.as_bytes());
        match download {
            Ok(operation) => {
                if let Some(window) = &self.window {
                    javascript_input::discard(window.as_ref());
                }
                self.links = links::Links::default();
                self.button_boxes.clear();
                if let Some(target) = &mut self.target {
                    let _ = target.set_button_regions(&[]);
                }
                self.html = None;
                self.document = None;
                self.css = css::Css::default();
                self.scripts = scripts::Scripts::default();
                self.layout_attempted = false;
                self.generation = self.generation.wrapping_add(1).max(1);
                self.layout_generation = None;
                self.published_generation = None;
                self.snapshot = snapshot::Snapshot::default();
                if let Some(target) = &self.target {
                    self.images.reset(target.render_target(), self.generation);
                    self.images.start_favicon(&url);
                }
                // A pending old frame must not publish after its assets retire.
                self.pending_publish = false;
                self.quads_dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                self.page_url = url;
                if let Some(javascript) = &mut self.javascript {
                    javascript.replace_page(self.generation, self.page_url.clone(), Vec::new());
                }
                self.download = Some(operation);
                shellio::linef(format_args!("hull: downloading {}", self.page_url));
            }
            Err(code) => {
                shellio::linef(format_args!("hull: download start failed: {code}"));
            }
        }
    }

    fn poll_javascript_input(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        // Drain remaining copies: raw routed records are authoritative. Standard
        // keyboard/pointer callbacks continue to drive Hull's existing UI only.
        let events = javascript_input::drain(window.as_ref());
        if let Some(javascript) = &mut self.javascript {
            for event in events {
                javascript_input::forward(javascript, self.generation, event);
            }
        }
    }

    fn poll_console(&mut self, event_loop: &dyn ActiveEventLoop) {
        let Some(console) = &mut self.console else {
            return;
        };
        let commands = match console.poll() {
            Ok(commands) => commands,
            Err(error) => {
                logl::log(logl::level::ERROR, format_args!("hull: console: {error}"));
                event_loop.exit();
                return;
            }
        };
        for command in commands {
            match command {
                console::Command::Url(url) if self.queued_urls.len() < 16 => {
                    self.queued_urls.push_back(QueuedNavigation {
                        url,
                        new_shell: false,
                    })
                }
                console::Command::Url(_) => shellio::line("hull: URL queue is full"),
                console::Command::Scripts => self.list_scripts(),
                console::Command::Run | console::Command::Script(_)
                    if self.document.is_none()
                        || self.download.is_some()
                        || !self.queued_urls.is_empty() =>
                {
                    shellio::line(
                        "hull: wait for the page to finish loading before running scripts",
                    );
                }
                console::Command::Run => {
                    let result = self
                        .javascript
                        .as_mut()
                        .ok_or("JavaScript is unavailable")
                        .and_then(|js| js.run());
                    match result {
                        Ok(()) => shellio::line("hull: page scripts requested"),
                        Err(error) => shellio::linef(format_args!("hull: {error}")),
                    }
                }
                console::Command::Script(source) => {
                    let result = self
                        .javascript
                        .as_mut()
                        .ok_or("JavaScript is unavailable")
                        .and_then(|js| js.execute_pasted(source));
                    if let Err(error) = result {
                        shellio::linef(format_args!("hull: {error}"));
                    }
                }
                console::Command::Exit => {
                    event_loop.exit();
                    break;
                }
            }
        }
    }

    fn list_scripts(&self) {
        if self.document.is_none() {
            shellio::line("hull: no page is loaded");
            return;
        }
        shellio::linef(format_args!(
            "hull: {} collected script sources — {}",
            self.scripts.sources.len(),
            self.page_url
        ));
        for (index, source) in self.scripts.sources.iter().enumerate() {
            let kind = &source.kind;
            if let Some(bytes) = &source.bytes {
                shellio::linef(format_args!(
                    "  {}: inline nodes={:?} type={} bytes={} {}",
                    index + 1,
                    source.nodes,
                    kind,
                    bytes.len(),
                    source.origin
                ));
            } else {
                shellio::linef(format_args!(
                    "  {}: external nodes={:?} type={} not fetched {}",
                    index + 1,
                    source.nodes,
                    kind,
                    source.origin
                ));
            }
        }
        shellio::line(
            "hull: :run requests collected inline JavaScript; :paste runs your own source with :end",
        );
    }

    fn page_pending(&self) -> bool {
        self.download.is_some()
            || self.css.is_pending()
            || (self.document.is_some()
                && (!self.layout_attempted
                    || self
                        .layout_generation
                        .is_some_and(|generation| self.published_generation != Some(generation))))
    }

    fn poll_download(&mut self) {
        let Some(operation) = self.download else {
            return;
        };
        match netfs::fetch_bytes_result_len(operation) {
            Err(-8) => return, // Kernel operation is still pending.
            Err(code) => logl::log(
                logl::level::ERROR,
                format_args!("hull: download failed: {code}"),
            ),
            Ok(len) if len > 4 * 1024 * 1024 => logl::log(
                logl::level::ERROR,
                format_args!("hull: HTML exceeds 4 MiB page budget"),
            ),
            Ok(_) => match netfs::fetch_bytes_read(operation) {
                Ok(bytes) => {
                    logl::log(
                        logl::level::INFO,
                        format_args!("hull: downloaded {} bytes={}", self.page_url, bytes.len()),
                    );
                    if links::binary_download(&bytes) {
                        // Undeclared binary responses are handed off with their received bytes.
                        // Never feed executable/archive payloads to the HTML parser.
                        let _ = netfs::fetch_bytes_discard(operation);
                        self.download = None;
                        self.submit_received(self.page_url.clone(), None, Some(bytes));
                        return;
                    }
                    let document = rp5::parse(&String::from_utf8_lossy(&bytes));
                    logl::log(
                        logl::level::INFO,
                        format_args!(
                            "hull: rp5 parsed nodes={} errors={}",
                            document.nodes.len(),
                            document.errors.len(),
                        ),
                    );
                    self.css.parse_document(&document, &self.page_url);
                    self.images.collect_favicon(&document, &self.page_url);
                    self.scripts.extract(&document, &self.page_url);
                    if let Some(javascript) = &mut self.javascript {
                        javascript.replace_page(
                            self.generation,
                            self.page_url.clone(),
                            self.scripts.sources.clone(),
                        );
                    }
                    self.document = Some(document);
                    self.html = Some(bytes);
                }
                Err(code) => logl::log(
                    logl::level::ERROR,
                    format_args!("hull: download read failed: {code}"),
                ),
            },
        }
        let _ = netfs::fetch_bytes_discard(operation);
        self.download = None;
    }
}

impl Drop for Hull {
    fn drop(&mut self) {
        if let Some(operation) = self.download.take() {
            let _ = netfs::fetch_bytes_discard(operation);
        }
    }
}

impl ApplicationHandler for Hull {
    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        self.poll_javascript_input();
        if let Some(javascript) = &mut self.javascript {
            for report in javascript.poll() {
                match report {
                    javascript::Report::Online => logl::log(
                        logl::level::INFO,
                        format_args!("hull: QuickJS task online; page execution is explicit"),
                    ),
                    javascript::Report::Page {
                        generation,
                        sources,
                    } => logl::log(
                        logl::level::INFO,
                        format_args!("hull: QuickJS page={generation} stored={sources} dormant"),
                    ),
                    javascript::Report::Evaluated {
                        generation,
                        origin,
                        node,
                        result,
                    } => match result {
                        Ok(()) => logl::log(
                            logl::level::INFO,
                            format_args!(
                                "hull: QuickJS page={generation} node={node} evaluated={origin}"
                            ),
                        ),
                        Err(error) => {
                            shellio::linef(format_args!("[error] node={node} {origin}: {error}"));
                            logl::log(
                                logl::level::ERROR,
                                format_args!(
                                    "hull: QuickJS page={generation} node={node} {origin}: {error}"
                                ),
                            );
                        }
                    },
                    javascript::Report::Skipped {
                        generation,
                        origin,
                        reason,
                    } => {
                        shellio::linef(format_args!("hull: skipped {origin}: {reason}"));
                        logl::log(
                            logl::level::INFO,
                            format_args!("hull: QuickJS page={generation} {origin}: {reason}"),
                        );
                    }
                    javascript::Report::JobError { generation, error } => {
                        shellio::linef(format_args!("[error] JavaScript job: {error}"));
                        logl::log(
                            logl::level::ERROR,
                            format_args!("hull: QuickJS page={generation} job: {error}"),
                        );
                    }
                    javascript::Report::Input {
                        generation,
                        kind,
                        result,
                    } => {
                        if let Err(error) = result {
                            shellio::linef(format_args!("[error] input={kind}: {error}"));
                            logl::log(
                                logl::level::ERROR,
                                format_args!(
                                    "hull: QuickJS page={generation} input={kind}: {error}"
                                ),
                            );
                        }
                    }
                    javascript::Report::Console { generation, batch } => {
                        if generation == self.generation {
                            for message in batch.messages {
                                shellio::linef(format_args!(
                                    "[{}] {}",
                                    message.level.as_str(),
                                    message.text
                                ));
                            }
                            if batch.dropped != 0 {
                                shellio::linef(format_args!(
                                    "[warn] console dropped {} messages",
                                    batch.dropped
                                ));
                            }
                        }
                    }
                    javascript::Report::Pasted { generation, result } => {
                        if generation == self.generation {
                            match result {
                                Ok(()) => shellio::line("hull: pasted script completed"),
                                Err(error) => {
                                    shellio::linef(format_args!("[error] pasted script: {error}"))
                                }
                            }
                        }
                    }
                }
            }
        }
        self.poll_console(event_loop);
        self.poll_download_submission();
        self.poll_download();
        self.css.poll_downloads();
        // Images and persistence can only begin after this navigation's initial
        // layout and text have successfully published. Neither gates navigation.
        if self.images.poll() {
            self.quads_dirty = true;
            self.redraw_reason = "image";
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        if self.published_generation == Some(self.generation) {
            self.snapshot.poll();
        }
        if !self.layout_attempted && self.document.is_some() && !self.css.is_pending() {
            self.layout_attempted = true;
            self.css.freeze();
            let started = std::time::Instant::now();
            let result = layout::compute(
                self.document.as_ref().unwrap(),
                &self.css,
                self.viewport.extent(),
            )
            .map_err(|error| error.to_string())
            .and_then(|layout| {
                logl::log(
                    logl::level::INFO,
                    format_args!(
                        "hull: layout complete boxes={} images={} elapsed_ms={}",
                        layout.line_rects.len(),
                        layout.image_boxes.len(),
                        started.elapsed().as_millis()
                    ),
                );
                self.images.collect(
                    self.document.as_ref().unwrap(),
                    &self.page_url,
                    layout.image_boxes,
                );
                self.snapshot.prepare(&self.page_url, layout.svg);
                self.layout_generation = Some(self.generation);
                self.layout_extent = layout.line_extent;
                self.links = links::Links::new(
                    self.document.as_ref().unwrap(),
                    layout.link_boxes,
                    &self.page_url,
                );
                self.text_runs = layout.text_runs;
                self.rectangles = layout.line_rects;
                self.background_quads = layout.background_quads;
                self.canvas_color = layout.canvas_color;
                self.border_quads = layout.border_quads;
                self.button_boxes = layout.button_boxes;
                // CSS overflow can extend beyond the SVG's declared viewBox.
                // Keep that overflow reachable without allocating a page-sized frame.
                let mut right = self.layout_extent.0 as f32;
                let mut bottom = self.layout_extent.1 as f32;
                for &[x, y, width, height] in &self.rectangles {
                    if [x, y, width, height].iter().all(|value| value.is_finite()) {
                        right = right.max(x + width);
                        bottom = bottom.max(y + height);
                    }
                }
                for run in &self.text_runs {
                    right = right.max(run.x + run.advance_px);
                    bottom = bottom.max(run.baseline_y + run.size_px * 0.25 + 1.0);
                }
                self.viewport
                    .set_document(right.ceil() as u32, bottom.ceil() as u32);
                self.quads_dirty = true;
                self.redraw_reason = "layout";
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                Ok(())
            });
            if let Err(error) = result {
                logl::log(
                    logl::level::ERROR,
                    format_args!("hull: layout display failed: {error}"),
                );
            }
        }
        if self.window.is_some() && !self.page_pending() {
            if let Some(navigation) = self.queued_urls.pop_front() {
                self.open_link(navigation.url, navigation.new_shell);
            }
        }
        // ShellIO input does not wake Winit's window-event wait. Keep polling
        // cooperatively even after the current page has finished.
        trueos::vsys::sleep_ms(20);
        event_loop.set_control_flow(ControlFlow::Poll);
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(error) = self.create_window(event_loop) {
                logl::log(logl::level::ERROR, format_args!("hull: {error}"));
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &dyn ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if !self.window.as_ref().is_some_and(|window| window.id() == id) {
            return;
        }
        // A supplemental state callback must not overtake the raw input that
        // Winit already captured before issuing this callback.
        let inputs =
            javascript_input::before_supplemental(self.window.as_ref().unwrap().as_ref(), &event);
        if let Some(javascript) = &mut self.javascript {
            for input in inputs {
                javascript_input::forward(javascript, self.generation, input);
            }
        }
        if self.target.is_none() {
            return;
        }
        let result = match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                event_loop.exit();
                Ok(())
            }
            WindowEvent::SurfaceResized(size) => (|| {
                if size.width == 0 || size.height == 0 {
                    return Ok(());
                }
                self.pending_publish = false;
                self.links.cancel();
                self.quads_dirty = true;
                self.redraw_reason = "resize";
                self.viewport
                    .resize_with_origin(size.width, size.height, 0, chrome::HEIGHT);
                self.frame_counter.resizing();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                self.target
                    .as_mut()
                    .unwrap()
                    .set_extent(size.width, size.height)
            })(),
            WindowEvent::PointerMoved {
                device_id,
                position,
                ..
            } => {
                self.links.moved(
                    device_id.map(|device| device.into_raw() as u64),
                    position.x,
                    position.y,
                );
                Ok(())
            }
            WindowEvent::PointerButton {
                device_id,
                state,
                position,
                button: ButtonSource::Mouse(MouseButton::Left),
                ..
            } => {
                if self.published_generation == Some(self.generation) {
                    if let Some(action) = self.links.button(
                        device_id.map(|device| device.into_raw() as u64),
                        state == ElementState::Pressed,
                        position.x,
                        position.y,
                        &self.viewport,
                    ) {
                        match action {
                            links::LinkAction::Navigate(url) => self.launch_navigation(url),
                            links::LinkAction::Download { url, filename_hint } => {
                                self.submit_download(url, filename_hint);
                            }
                        }
                    }
                }
                Ok(())
            }
            WindowEvent::Focused(false) | WindowEvent::PointerLeft { .. } => {
                self.links.cancel();
                Ok(())
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.shift_scroll = modifiers.state().shift_key();
                Ok(())
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (mut dx, mut dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (-f64::from(x) * 48.0, -f64::from(y) * 48.0)
                    }
                    MouseScrollDelta::PixelDelta(position) => (-position.x, -position.y),
                    _ => (0.0, 0.0),
                };
                if self.shift_scroll && dx == 0.0 {
                    dx = dy;
                    dy = 0.0;
                }
                if self.viewport.scroll_by(dx, dy) {
                    self.links.cancel();
                    self.quads_dirty = true;
                    self.redraw_reason = "scroll";
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                Ok(())
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let changed = match &event.logical_key {
                    Key::Named(NamedKey::F2) => {
                        if event.repeat {
                            return;
                        }
                        self.quad_pass.toggle();
                        shellio::linef(format_args!("hull: quad pass {}", self.quad_pass.name()));
                        true
                    }
                    Key::Named(NamedKey::ArrowUp) => self.viewport.scroll_by(0.0, -48.0),
                    Key::Named(NamedKey::ArrowDown) => self.viewport.scroll_by(0.0, 48.0),
                    Key::Named(NamedKey::ArrowLeft) => self.viewport.scroll_by(-48.0, 0.0),
                    Key::Named(NamedKey::ArrowRight) => self.viewport.scroll_by(48.0, 0.0),
                    Key::Named(NamedKey::PageUp) => self.viewport.page(-1),
                    Key::Named(NamedKey::PageDown) => self.viewport.page(1),
                    Key::Named(NamedKey::Home) => self.viewport.home(),
                    Key::Named(NamedKey::End) => self.viewport.end(),
                    _ => false,
                };
                if changed {
                    self.links.cancel();
                    self.quads_dirty = true;
                    self.redraw_reason = if matches!(event.logical_key, Key::Named(NamedKey::F2)) {
                        "quad-pass"
                    } else {
                        "scroll"
                    };
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                Ok(())
            }
            WindowEvent::RedrawRequested => (|| {
                let window = self.window.as_ref().unwrap();
                let target = self.target.as_mut().unwrap();
                // Moving the retained UI4 frame is compositor work. A generic
                // redraw notification does not invalidate our content.
                if !self.quads_dirty && !self.pending_publish {
                    return Ok(());
                }
                if !self.pending_publish && self.quads_dirty {
                    self.frame_counter.attempt();
                    let size = window.surface_size();
                    let visible_rectangles: Vec<_> = self
                        .rectangles
                        .iter()
                        .filter_map(|&rect| self.viewport.clip_rect(rect))
                        .collect();
                    if self.image_frame_recovery {
                        images::recover_frame(target)?;
                    } else if self.layout_generation == Some(self.generation)
                        && self.quad_pass == actual_bg_quad::QuadPass::ActualBgQuad
                    {
                        actual_bg_quad::draw(
                            &self.background_quads,
                            self.canvas_color,
                            &self.viewport,
                            target,
                        )?;
                    } else if self.layout_generation == Some(self.generation) {
                        if self.quads.is_none() {
                            self.quads = Some(pink_quad_dbg::Quads::new().map_err(|code| {
                                if code == trueos::vgpu::ERR_BUSY {
                                    Error::Busy
                                } else {
                                    Error::Unknown(code)
                                }
                            })?);
                        }
                        let quads = self.quads.as_mut().unwrap();
                        quads
                            .set_layout(size.width, size.height, &visible_rectangles)
                            .map_err(|code| {
                                if code == trueos::vgpu::ERR_BUSY {
                                    Error::Busy
                                } else {
                                    Error::Unknown(code)
                                }
                            })?;
                        quads.draw(target)?;
                    } else {
                        target.begin(u32::MAX)?;
                    }
                    if !self.image_frame_recovery && self.layout_generation == Some(self.generation)
                    {
                        let ui_error = |code| {
                            if code == trueos::vgpu::ERR_BUSY {
                                Error::Busy
                            } else {
                                Error::Unknown(code)
                            }
                        };
                        if self.borders.is_none() {
                            self.borders =
                                Some(pink_quad_dbg::BorderPass::new().map_err(ui_error)?);
                        }
                        let borders = self.borders.as_mut().unwrap();
                        borders
                            .set_borders(&self.border_quads, &self.viewport)
                            .map_err(ui_error)?;
                        borders.draw_borders(target)?;
                    }
                    if !self.image_frame_recovery && self.images.draw(&self.viewport, target) {
                        // The failed scene may have canceled its lease. Do not
                        // submit text or publish until recovery checks it again.
                        self.image_frame_recovery = true;
                        self.quads_dirty = true;
                        return Err(Error::Busy);
                    }
                    if self.layout_generation == Some(self.generation) && !self.text_runs.is_empty()
                    {
                        // Document pixels remain document pixels. Cull and trim
                        // offscreen runs before transport or font preparation.
                        let runs: Vec<_> = self
                            .text_runs
                            .iter()
                            .filter_map(|run| {
                                if run.color[3] == 0 {
                                    return None;
                                }
                                let visible = self.viewport.text_slice(
                                    &run.text,
                                    run.x,
                                    run.baseline_y,
                                    run.size_px,
                                    run.advance_px,
                                )?;
                                Some(HullTextRun {
                                    text: visible.text,
                                    x: visible.x,
                                    baseline_y: visible.baseline_y,
                                    size_px: visible.size_px,
                                    advance_px: visible.advance_px,
                                    rgba: u32::from_le_bytes(run.color),
                                })
                            })
                            .collect();
                        match target.draw_hull_text_runs(&runs) {
                            Err(Error::InvalidState) if self.image_frame_recovery => {
                                return Err(Error::Busy);
                            }
                            result => result?,
                        }
                        let (x, y) = self.viewport.offset();
                        let (doc_width, doc_height) = self.viewport.document_extent();
                        let (viewport_width, viewport_height) = self.viewport.extent();
                        shellio::linef(format_args!(
                            "hull: viewport={}x{} document={}x{} scroll={},{} rects={}/{} text_runs={}/{} glyphs={} scale=1",
                            viewport_width,
                            viewport_height,
                            doc_width,
                            doc_height,
                            x,
                            y,
                            visible_rectangles.len(),
                            self.rectangles.len(),
                            runs.len(),
                            self.text_runs.len(),
                            runs.iter()
                                .map(|run| run.text.chars().count())
                                .sum::<usize>(),
                        ));
                    }
                    // Native LOAD_COLOR triangles accept the CPU-authored page
                    // text lease. Their release then authorizes a favicon sprite
                    // overlay; issuing sprites directly after text is invalid.
                    let ui_error = |code| {
                        if code == trueos::vgpu::ERR_BUSY {
                            Error::Busy
                        } else {
                            Error::Unknown(code)
                        }
                    };
                    if self.chrome_quads.is_none() {
                        self.chrome_quads =
                            Some((pink_quad_dbg::Quads::new().map_err(ui_error)?, (0, 0)));
                    }
                    let (chrome_pass, extent) = self.chrome_quads.as_mut().unwrap();
                    if *extent != (size.width, size.height) {
                        chrome_pass
                            .set_colored_rectangles(
                                size.width,
                                size.height,
                                &chrome::rectangles(size.width, size.height),
                            )
                            .map_err(ui_error)?;
                        *extent = (size.width, size.height);
                    }
                    if let Err(error) = chrome_pass.draw_overlay(target) {
                        if error == Error::Busy {
                            self.image_frame_recovery = true;
                        }
                        return Err(error);
                    }
                    let icon = self.images.favicon().and_then(|icon| {
                        chrome::icon_quad(
                            size.width,
                            size.height,
                            icon.sprite_id,
                            icon.width,
                            icon.height,
                        )
                    });
                    if let Some(icon) = icon {
                        if let Err(error) = target.draw_sprite_quads(&[icon]) {
                            self.images.disable_favicon();
                            self.image_frame_recovery = true;
                            logl::log(
                                logl::level::ERROR,
                                format_args!("hull: favicon paint failed: {error:?}"),
                            );
                            return Err(Error::Busy);
                        }
                    } else {
                        let letter = chrome::domain_letter(&self.page_url);
                        if let Some(run) = chrome::letter_run(size.width, size.height, &letter) {
                            target.draw_hull_text_runs(&[run])?;
                        }
                    }
                    self.frame_counter
                        .produced(self.redraw_reason, size.width, size.height, true);
                    self.quads_dirty = false;
                    self.pending_publish = true;
                }
                window.pre_present_notify();
                let size = window.surface_size();
                self.frame_counter.publishing();
                match target.publish(Damage::full(size.width, size.height)) {
                    Err(Error::InvalidState) if self.image_frame_recovery => {
                        self.pending_publish = false;
                        self.quads_dirty = true;
                        return Err(Error::Busy);
                    }
                    result => result?,
                }
                // Update hit regions only with the published geometry, never
                // on cursor movement. A partial scroll clip retains real edges.
                let regions = buttons::regions(&self.button_boxes, &self.viewport);
                if let Err(error) = target.set_button_regions(&regions) {
                    logl::log(
                        logl::level::ERROR,
                        format_args!("hull: button outlines: {error:?}"),
                    );
                }
                self.frame_counter.published();
                self.pending_publish = false;
                if self.image_frame_recovery {
                    // Recovery first publishes a safe text frame. Repaint the
                    // full page next so an icon failure cannot leave CSS and
                    // page images absent until the next scroll or resize.
                    self.quads_dirty = true;
                    self.redraw_reason = "recovery";
                }
                self.image_frame_recovery = false;
                if self.layout_generation == Some(self.generation) {
                    self.published_generation = Some(self.generation);
                    self.images.published();
                }
                if self.quads_dirty {
                    window.request_redraw();
                }
                Ok(())
            })(),
            _ => Ok(()),
        };
        match result {
            Ok(()) => {}
            Err(Error::Busy) => {
                self.frame_counter.busy();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            Err(error) => {
                logl::log(logl::level::ERROR, format_args!("hull: {error:?}"));
                event_loop.exit();
            }
        }
    }
}

fn main() {
    let javascript = match javascript::Runtime::start() {
        Ok(runtime) => runtime,
        Err(error) => {
            logl::log(
                logl::level::ERROR,
                format_args!("hull: QuickJS task initialization: {error}"),
            );
            return;
        }
    };
    let mut app = Hull::default();
    app.javascript = Some(javascript);
    if let Ok(bytes) = trueos::async_fs::block_on(trueos::async_fs::read_file(b"vFile:launch")) {
        let startup = std::str::from_utf8(&bytes)
            .map_err(|_| "startup script must be UTF-8")
            .and_then(links::startup_url);
        match startup {
            Ok(Some(url)) => app.queued_urls.push_back(QueuedNavigation {
                url,
                new_shell: false,
            }),
            Ok(None) => {}
            Err(error) => {
                shellio::linef(format_args!("hull: {error}"));
            }
        }
    }
    app.console = match console::Console::start() {
        Ok(Some(console)) => Some(console),
        Ok(None) => {
            // Unattended/Text launches have no terminal lease. Use the same
            // terminal-capable destination as native hyperlink navigation;
            // the replacement instance owns both console and browser window.
            let script = app
                .queued_urls
                .front()
                .map(|navigation| format!("open {}\n", navigation.url))
                .unwrap_or_default();
            if let Err(code) = vshell::launch_with_destination(
                "hull",
                &script,
                vshell::LaunchDestination::NewShell3,
            ) {
                logl::log(
                    logl::level::ERROR,
                    format_args!("hull: terminal launch failed: {code}"),
                );
            }
            return;
        }
        Err(error) => {
            logl::log(
                logl::level::ERROR,
                format_args!("hull: console initialization: {error}"),
            );
            return;
        }
    };
    let result = EventLoop::new().and_then(|event_loop| event_loop.run_app(app));
    if let Err(error) = result {
        logl::log(logl::level::ERROR, format_args!("hull: {error}"));
    }
}
