use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use baseview::{
    Event, EventStatus, PhySize, Window, WindowHandle, WindowHandler, WindowOpenOptions,
    WindowScalePolicy,
};
use copypasta::ClipboardProvider;
use egui::epaint::ClippedShape;
use egui::{pos2, vec2, Pos2, Rect, Rgba, ViewportCommand};
use keyboard_types::{KeyboardEvent, Modifiers};
use raw_window_handle::HasRawWindowHandle;

use crate::{renderer::Renderer, GraphicsConfig};

/// Without input, a repaint request or a deadline, a pass still runs this often, so the
/// application notices state it is not told about (a transport that started, a meter that moved).
const IDLE_PASS_INTERVAL: Duration = Duration::from_millis(50);

/// The window renders at least this often, so a surface the host mapped or showed again gets its
/// contents back without waiting for a change.
const SAFETY_RENDER_INTERVAL: Duration = Duration::from_millis(500);

/// Every frame runs a pass and renders for this long after the window opens: hosts open the
/// editor while the window is still unmapped, and a render into an unmapped surface is lost.
const WARM_UP: Duration = Duration::from_secs(1);

pub struct Queue<'a> {
    bg_color: &'a mut Rgba,
    close_requested: &'a mut bool,
    physical_size: &'a mut PhySize,
}

impl<'a> Queue<'a> {
    pub(crate) fn new(
        bg_color: &'a mut Rgba,
        close_requested: &'a mut bool,
        physical_size: &'a mut PhySize,
    ) -> Self {
        Self {
            bg_color,
            //renderer,
            //repaint_requested,
            close_requested,
            physical_size,
        }
    }

    /// Set the background color.
    pub fn bg_color(&mut self, bg_color: Rgba) {
        *self.bg_color = bg_color;
    }

    /// Set size of the window.
    pub fn resize(&mut self, physical_size: PhySize) {
        *self.physical_size = physical_size;
    }

    /// Close the window.
    pub fn close_window(&mut self) {
        *self.close_requested = true;
    }
}

struct OpenSettings {
    scale_policy: WindowScalePolicy,
    logical_width: f64,
    logical_height: f64,
    title: String,
}

impl OpenSettings {
    fn new(settings: &WindowOpenOptions) -> Self {
        // WindowScalePolicy does not implement copy/clone.
        let scale_policy = match &settings.scale {
            WindowScalePolicy::SystemScaleFactor => WindowScalePolicy::SystemScaleFactor,
            WindowScalePolicy::ScaleFactor(scale) => WindowScalePolicy::ScaleFactor(*scale),
        };

        Self {
            scale_policy,
            logical_width: settings.size.width,
            logical_height: settings.size.height,
            title: settings.title.clone(),
        }
    }
}

/// Handles an egui-baseview application
pub struct EguiWindow<State, U>
where
    State: 'static + Send,
    U: FnMut(&egui::Context, &mut Queue, &mut State),
    U: 'static + Send,
{
    user_state: Option<State>,
    user_update: U,

    egui_ctx: egui::Context,
    viewport_id: egui::ViewportId,
    start_time: Instant,
    egui_input: egui::RawInput,
    pointer_pos_in_points: Option<egui::Pos2>,
    current_cursor_icon: baseview::MouseCursor,

    renderer: Renderer,

    clipboard_ctx: Option<copypasta::ClipboardContext>,

    physical_size: PhySize,
    scale_policy: WindowScalePolicy,
    pixels_per_point: f32,
    points_per_pixel: f32,
    bg_color: Rgba,
    close_requested: bool,
    /// When egui asked for the next pass (a repaint request with a delay).
    repaint_after: Option<Instant>,
    /// Set by egui's repaint callback for a request without a delay, from inside a pass or from
    /// another thread between passes.
    wake: Arc<AtomicBool>,
    last_pass: Option<Instant>,
    /// The shapes of the last render: a pass that draws the same shapes does not render again.
    last_shapes: Vec<ClippedShape>,
    last_render: Option<Instant>,
    /// The next pass renders whatever it draws: after a resize or a focus change.
    render_forced: bool,
}

impl<State, U> EguiWindow<State, U>
where
    State: 'static + Send,
    U: FnMut(&egui::Context, &mut Queue, &mut State),
    U: 'static + Send,
{
    fn new<B>(
        window: &mut baseview::Window<'_>,
        open_settings: OpenSettings,
        graphics_config: GraphicsConfig,
        mut build: B,
        update: U,
        mut state: State,
    ) -> EguiWindow<State, U>
    where
        B: FnMut(&egui::Context, &mut Queue, &mut State),
        B: 'static + Send,
    {
        let renderer = Renderer::new(window, graphics_config).unwrap_or_else(|err| {
            // TODO: better error log and not panicking, but that's gonna require baseview changes
            log::error!("oops! the gpu backend couldn't initialize! \n {err}");
            panic!("gpu backend failed to initialize: \n {err}")
        });
        let egui_ctx = egui::Context::default();
        let wake = Arc::new(AtomicBool::new(false));
        egui_ctx.set_request_repaint_callback({
            let wake = wake.clone();
            move |info| {
                if info.delay.is_zero() {
                    wake.store(true, Ordering::Relaxed);
                }
            }
        });

        // Assume scale for now until there is an event with a new one.
        let pixels_per_point = match open_settings.scale_policy {
            WindowScalePolicy::ScaleFactor(scale) => scale,
            WindowScalePolicy::SystemScaleFactor => 1.0,
        } as f32;
        let points_per_pixel = pixels_per_point.recip();

        let screen_rect = Rect::from_min_size(
            Pos2::new(0f32, 0f32),
            vec2(
                open_settings.logical_width as f32,
                open_settings.logical_height as f32,
            ),
        );

        let viewport_info = egui::ViewportInfo {
            parent: None,
            title: Some(open_settings.title),
            native_pixels_per_point: Some(pixels_per_point),
            focused: Some(true),
            inner_rect: Some(screen_rect),
            ..Default::default()
        };
        let viewport_id = egui::ViewportId::default();

        let mut egui_input = egui::RawInput {
            max_texture_side: Some(renderer.max_texture_side()),
            screen_rect: Some(screen_rect),
            ..Default::default()
        };
        let _ = egui_input.viewports.insert(viewport_id, viewport_info);

        let mut physical_size = PhySize {
            width: (open_settings.logical_width * pixels_per_point as f64).round() as u32,
            height: (open_settings.logical_height * pixels_per_point as f64).round() as u32,
        };

        let mut bg_color = Rgba::BLACK;
        let mut close_requested = false;
        let mut queue = Queue::new(&mut bg_color, &mut close_requested, &mut physical_size);
        (build)(&egui_ctx, &mut queue, &mut state);

        let clipboard_ctx = match copypasta::ClipboardContext::new() {
            Ok(clipboard_ctx) => Some(clipboard_ctx),
            Err(e) => {
                log::error!("Failed to initialize clipboard: {}", e);
                None
            }
        };

        let start_time = Instant::now();

        Self {
            user_state: Some(state),
            user_update: update,

            egui_ctx,
            viewport_id,
            start_time,
            egui_input,
            pointer_pos_in_points: None,
            current_cursor_icon: baseview::MouseCursor::Default,

            renderer,

            clipboard_ctx,

            physical_size,
            pixels_per_point,
            points_per_pixel,
            scale_policy: open_settings.scale_policy,
            bg_color,
            close_requested,
            repaint_after: None,
            wake,
            last_pass: None,
            last_shapes: Vec::new(),
            last_render: None,
            render_forced: true,
        }
    }

    /// Open a new child window.
    ///
    /// * `parent` - The parent window.
    /// * `settings` - The settings of the window.
    /// * `state` - The initial state of your application.
    /// * `build` - Called once before the first frame. Allows you to do setup code and to
    /// call `ctx.set_fonts()`. Optional.
    /// * `update` - Called before each frame. Here you should update the state of your
    /// application and build the UI.
    pub fn open_parented<P, B>(
        parent: &P,
        #[allow(unused_mut)] mut settings: WindowOpenOptions,
        graphics_config: GraphicsConfig,
        state: State,
        build: B,
        update: U,
    ) -> WindowHandle
    where
        P: HasRawWindowHandle,
        B: FnMut(&egui::Context, &mut Queue, &mut State),
        B: 'static + Send,
    {
        #[cfg(feature = "opengl")]
        if settings.gl_config.is_none() {
            settings.gl_config = Some(Default::default());
        }

        let open_settings = OpenSettings::new(&settings);

        Window::open_parented(
            parent,
            settings,
            move |window: &mut baseview::Window<'_>| -> EguiWindow<State, U> {
                EguiWindow::new(window, open_settings, graphics_config, build, update, state)
            },
        )
    }

    /// Open a new window that blocks the current thread until the window is destroyed.
    ///
    /// * `settings` - The settings of the window.
    /// * `state` - The initial state of your application.
    /// * `build` - Called once before the first frame. Allows you to do setup code and to
    /// call `ctx.set_fonts()`. Optional.
    /// * `update` - Called before each frame. Here you should update the state of your
    /// application and build the UI.
    pub fn open_blocking<B>(
        #[allow(unused_mut)] mut settings: WindowOpenOptions,
        graphics_config: GraphicsConfig,
        state: State,
        build: B,
        update: U,
    ) where
        B: FnMut(&egui::Context, &mut Queue, &mut State),
        B: 'static + Send,
    {
        #[cfg(feature = "opengl")]
        if settings.gl_config.is_none() {
            settings.gl_config = Some(Default::default());
        }

        let open_settings = OpenSettings::new(&settings);

        Window::open_blocking(
            settings,
            move |window: &mut baseview::Window<'_>| -> EguiWindow<State, U> {
                EguiWindow::new(window, open_settings, graphics_config, build, update, state)
            },
        )
    }

    /// Update the pressed key modifiers when a mouse event has sent a new set of modifiers.
    fn update_modifiers(&mut self, modifiers: &Modifiers) {
        self.egui_input.modifiers = translate_modifiers(*modifiers);
    }
}

impl<State, U> WindowHandler for EguiWindow<State, U>
where
    State: 'static + Send,
    U: FnMut(&egui::Context, &mut Queue, &mut State),
    U: 'static + Send,
{
    fn on_frame(&mut self, window: &mut Window) {
        let Some(state) = &mut self.user_state else {
            return;
        };

        // A pass runs for input, when egui asked for one, during the warm-up and at the idle
        // rate.
        let now = Instant::now();
        let warming_up = now.duration_since(self.start_time) < WARM_UP;
        let due = !self.egui_input.events.is_empty()
            || self.wake.swap(false, Ordering::Relaxed)
            || self.repaint_after.is_some_and(|t| now >= t)
            || warming_up
            || self
                .last_pass
                .is_none_or(|t| now.duration_since(t) >= IDLE_PASS_INTERVAL);
        if !due {
            return;
        }
        self.last_pass = Some(now);

        self.egui_input.time = Some(self.start_time.elapsed().as_secs_f64());
        self.egui_input.screen_rect = Some(calculate_screen_rect(
            self.physical_size,
            self.points_per_pixel,
        ));

        self.egui_ctx.begin_pass(self.egui_input.take());

        //let mut repaint_requested = false;
        let mut queue = Queue::new(
            &mut self.bg_color,
            &mut self.close_requested,
            &mut self.physical_size,
        );

        (self.user_update)(&self.egui_ctx, &mut queue, state);

        if self.close_requested {
            window.close();
        }

        // Prevent data from being allocated every frame by storing this
        // in a member field.
        let mut full_output = self.egui_ctx.end_pass();

        let Some(viewport_output) = full_output.viewport_output.get(&self.viewport_id) else {
            // The main window was closed by egui.
            window.close();
            return;
        };

        for command in viewport_output.commands.iter() {
            match command {
                ViewportCommand::Close => {
                    window.close();
                }
                ViewportCommand::InnerSize(size) => window.resize(baseview::Size {
                    width: size.x.max(1.0) as f64,
                    height: size.y.max(1.0) as f64,
                }),
                _ => {}
            }
        }

        // egui asks for the next pass after at most `repaint_delay` (`None` when it asks for
        // none). A request without a delay reaches `wake` through the callback.
        self.repaint_after = now.checked_add(viewport_output.repaint_delay);

        // Render when the pass drew something else than the last render shows or changed a
        // texture, and whatever it drew when forced, during the warm-up and at the safety rate.
        let changed =
            full_output.shapes != self.last_shapes || !full_output.textures_delta.is_empty();
        let forced = self.render_forced
            || warming_up
            || self
                .last_render
                .is_none_or(|t| now.duration_since(t) >= SAFETY_RENDER_INTERVAL);
        if changed || forced {
            self.last_shapes = full_output.shapes.clone();
            self.renderer.render(
                #[cfg(feature = "opengl")]
                window,
                self.bg_color,
                self.physical_size,
                self.pixels_per_point,
                &mut self.egui_ctx,
                &mut full_output,
            );
            self.render_forced = false;
            self.last_render = Some(now);
        }

        for command in full_output.platform_output.commands {
            match command {
                egui::OutputCommand::CopyText(text) => {
                    if let Some(clipboard_ctx) = &mut self.clipboard_ctx {
                        if let Err(err) = clipboard_ctx.set_contents(text) {
                            log::error!("Copy/Cut error: {}", err);
                        }
                    }
                }
                egui::OutputCommand::CopyImage(_) => {
                    log::warn!("Copying images is not supported in egui_baseview.");
                }
                egui::OutputCommand::OpenUrl(open_url) => {
                    if let Err(err) = open::that_detached(&open_url.url) {
                        log::error!("Open error: {}", err);
                    }
                }
            }
        }

        let cursor_icon =
            crate::translate::translate_cursor_icon(full_output.platform_output.cursor_icon);
        if self.current_cursor_icon != cursor_icon {
            self.current_cursor_icon = cursor_icon;

            // TODO: Set mouse cursor for MacOS once baseview supports it.
            #[cfg(not(target_os = "macos"))]
            window.set_mouse_cursor(cursor_icon);
        }

        // A temporary workaround for keyboard input not working sometimes in Windows.
        // See https://github.com/BillyDM/egui-baseview/issues/20
        #[cfg(feature = "windows_keyboard_workaround")]
        {
            #[cfg(target_os = "windows")]
            {
                if !full_output.platform_output.events.is_empty()
                    || full_output.platform_output.ime.is_some()
                {
                    window.focus();
                }
            }
        }
    }

    fn on_event(&mut self, _window: &mut Window, event: Event) -> EventStatus {
        match &event {
            baseview::Event::Mouse(event) => match event {
                baseview::MouseEvent::CursorMoved {
                    position,
                    modifiers,
                } => {
                    self.update_modifiers(modifiers);

                    let pos = pos2(position.x as f32, position.y as f32);
                    self.pointer_pos_in_points = Some(pos);
                    self.egui_input.events.push(egui::Event::PointerMoved(pos));
                }
                baseview::MouseEvent::ButtonPressed { button, modifiers } => {
                    self.update_modifiers(modifiers);

                    if let Some(pos) = self.pointer_pos_in_points {
                        if let Some(button) = crate::translate::translate_mouse_button(*button) {
                            self.egui_input.events.push(egui::Event::PointerButton {
                                pos,
                                button,
                                pressed: true,
                                modifiers: self.egui_input.modifiers,
                            });
                        }
                    }
                }
                baseview::MouseEvent::ButtonReleased { button, modifiers } => {
                    self.update_modifiers(modifiers);

                    if let Some(pos) = self.pointer_pos_in_points {
                        if let Some(button) = crate::translate::translate_mouse_button(*button) {
                            self.egui_input.events.push(egui::Event::PointerButton {
                                pos,
                                button,
                                pressed: false,
                                modifiers: self.egui_input.modifiers,
                            });
                        }
                    }
                }
                baseview::MouseEvent::WheelScrolled {
                    delta: scroll_delta,
                    modifiers,
                } => {
                    self.update_modifiers(modifiers);

                    #[allow(unused_mut)]
                    let (unit, mut delta) = match scroll_delta {
                        baseview::ScrollDelta::Lines { x, y } => {
                            (egui::MouseWheelUnit::Line, egui::vec2(*x, *y))
                        }

                        baseview::ScrollDelta::Pixels { x, y } => (
                            egui::MouseWheelUnit::Point,
                            egui::vec2(*x, *y) * self.points_per_pixel,
                        ),
                    };

                    if cfg!(target_os = "macos") {
                        // This is still buggy in winit despite
                        // https://github.com/rust-windowing/winit/issues/1695 being closed
                        //
                        // TODO: See if this is an issue in baseview as well.
                        delta.x *= -1.0;
                    }

                    self.egui_input.events.push(egui::Event::MouseWheel {
                        unit,
                        delta,
                        modifiers: self.egui_input.modifiers,
                    });
                }
                baseview::MouseEvent::CursorLeft => {
                    self.pointer_pos_in_points = None;
                    self.egui_input.events.push(egui::Event::PointerGone);
                }
                _ => {}
            },
            baseview::Event::Keyboard(event) => {
                let pressed = event.state == keyboard_types::KeyState::Down;

                // The event carries the modifier state after the key, so a modifier key's own
                // press and release are included.
                self.egui_input.modifiers = translate_modifiers(event.modifiers);

                if let Some(key) = crate::translate::translate_virtual_key(&event.key) {
                    self.egui_input.events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: event.repeat,
                        modifiers: self.egui_input.modifiers,
                    });
                }

                if pressed {
                    match typed(self.egui_input.modifiers, event) {
                        Typed::Nothing => {}
                        Typed::Cut => self.egui_input.events.push(egui::Event::Cut),
                        Typed::Copy => self.egui_input.events.push(egui::Event::Copy),
                        Typed::Paste => {
                            if let Some(clipboard_ctx) = &mut self.clipboard_ctx {
                                match clipboard_ctx.get_contents() {
                                    Ok(contents) => {
                                        self.egui_input.events.push(egui::Event::Paste(contents))
                                    }
                                    Err(err) => {
                                        log::error!("Paste error: {}", err);
                                    }
                                }
                            }
                        }
                        Typed::Text(text) => self.egui_input.events.push(egui::Event::Text(text)),
                    }
                }
            }
            baseview::Event::Window(event) => match event {
                baseview::WindowEvent::Resized(window_info) => {
                    self.pixels_per_point = match self.scale_policy {
                        WindowScalePolicy::ScaleFactor(scale) => scale,
                        WindowScalePolicy::SystemScaleFactor => window_info.scale(),
                    } as f32;
                    self.points_per_pixel = self.pixels_per_point.recip();

                    self.physical_size = window_info.physical_size();

                    let screen_rect =
                        calculate_screen_rect(self.physical_size, self.points_per_pixel);

                    self.egui_input.screen_rect = Some(screen_rect);

                    let viewport_info = self
                        .egui_input
                        .viewports
                        .get_mut(&self.viewport_id)
                        .unwrap();
                    viewport_info.native_pixels_per_point = Some(self.pixels_per_point);
                    viewport_info.inner_rect = Some(screen_rect);

                    // The surface changed: render on the next frame.
                    self.repaint_after = Some(Instant::now());
                    self.render_forced = true;
                }
                baseview::WindowEvent::Focused => {
                    // The host may have shown the window again: render on the next frame.
                    self.render_forced = true;
                    self.egui_input
                        .events
                        .push(egui::Event::WindowFocused(true));
                    self.egui_input
                        .viewports
                        .get_mut(&self.viewport_id)
                        .unwrap()
                        .focused = Some(true);
                }
                baseview::WindowEvent::Unfocused => {
                    self.egui_input
                        .events
                        .push(egui::Event::WindowFocused(false));
                    self.egui_input
                        .viewports
                        .get_mut(&self.viewport_id)
                        .unwrap()
                        .focused = Some(false);
                }
                baseview::WindowEvent::WillClose => {}
            },
        }

        EventStatus::Captured
    }
}

/// The egui modifiers of a baseview modifier set. On macOS the command key is Cmd (`META`) and
/// Control stays a plain modifier; elsewhere Control is the command key.
fn translate_modifiers(modifiers: Modifiers) -> egui::Modifiers {
    translate_modifiers_for(modifiers, cfg!(target_os = "macos"))
}

fn translate_modifiers_for(modifiers: Modifiers, macos: bool) -> egui::Modifiers {
    let ctrl = modifiers.contains(Modifiers::CONTROL);
    let meta = modifiers.contains(Modifiers::META);
    egui::Modifiers {
        alt: modifiers.contains(Modifiers::ALT),
        ctrl,
        shift: modifiers.contains(Modifiers::SHIFT),
        mac_cmd: macos && meta,
        command: if macos { meta } else { ctrl },
    }
}

/// What a key press types, besides its `egui::Event::Key`.
#[derive(Debug, PartialEq)]
enum Typed {
    Nothing,
    Cut,
    Copy,
    Paste,
    Text(String),
}

/// A letter with the command key (or Control) is a shortcut, not text.
fn typed(modifiers: egui::Modifiers, event: &KeyboardEvent) -> Typed {
    if is_cut_command(modifiers, event.code) {
        Typed::Cut
    } else if is_copy_command(modifiers, event.code) {
        Typed::Copy
    } else if is_paste_command(modifiers, event.code) {
        Typed::Paste
    } else if let keyboard_types::Key::Character(written) = &event.key {
        if modifiers.ctrl || modifiers.command {
            Typed::Nothing
        } else {
            Typed::Text(written.clone())
        }
    } else {
        Typed::Nothing
    }
}

fn is_cut_command(modifiers: egui::Modifiers, keycode: keyboard_types::Code) -> bool {
    (modifiers.command && keycode == keyboard_types::Code::KeyX)
        || (cfg!(target_os = "windows")
            && modifiers.shift
            && keycode == keyboard_types::Code::Delete)
}

fn is_copy_command(modifiers: egui::Modifiers, keycode: keyboard_types::Code) -> bool {
    (modifiers.command && keycode == keyboard_types::Code::KeyC)
        || (cfg!(target_os = "windows")
            && modifiers.ctrl
            && keycode == keyboard_types::Code::Insert)
}

fn is_paste_command(modifiers: egui::Modifiers, keycode: keyboard_types::Code) -> bool {
    (modifiers.command && keycode == keyboard_types::Code::KeyV)
        || (cfg!(target_os = "windows")
            && modifiers.shift
            && keycode == keyboard_types::Code::Insert)
}

/// Calculate screen rectangle in logical size.
fn calculate_screen_rect(physical_size: PhySize, points_per_pixel: f32) -> Rect {
    let logical_size = (
        physical_size.width as f32 * points_per_pixel,
        physical_size.height as f32 * points_per_pixel,
    );
    Rect::from_min_size(Pos2::new(0f32, 0f32), vec2(logical_size.0, logical_size.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyboard_types::{Code, Key, KeyState, Location};

    fn press(code: Code, key: &str, modifiers: Modifiers) -> KeyboardEvent {
        KeyboardEvent {
            state: KeyState::Down,
            key: Key::Character(key.into()),
            code,
            location: Location::Standard,
            modifiers,
            repeat: false,
            is_composing: false,
        }
    }

    #[test]
    fn cmd_is_the_command_key_on_macos() {
        let cmd = translate_modifiers_for(Modifiers::META, true);
        assert!(cmd.command && cmd.mac_cmd && !cmd.ctrl);
        let ctrl = translate_modifiers_for(Modifiers::CONTROL, true);
        assert!(ctrl.ctrl && !ctrl.command && !ctrl.mac_cmd);
    }

    #[test]
    fn ctrl_is_the_command_key_elsewhere() {
        let ctrl = translate_modifiers_for(Modifiers::CONTROL | Modifiers::SHIFT, false);
        assert!(ctrl.ctrl && ctrl.command && ctrl.shift && !ctrl.mac_cmd);
        assert_eq!(
            translate_modifiers_for(Modifiers::META, false),
            egui::Modifiers::NONE
        );
    }

    #[test]
    fn letters_with_the_command_key_are_shortcuts() {
        let cmd = translate_modifiers_for(Modifiers::META, true);
        assert_eq!(
            typed(cmd, &press(Code::KeyA, "a", Modifiers::META)),
            Typed::Nothing
        );
        assert_eq!(
            typed(cmd, &press(Code::KeyX, "x", Modifiers::META)),
            Typed::Cut
        );
        assert_eq!(
            typed(cmd, &press(Code::KeyC, "c", Modifiers::META)),
            Typed::Copy
        );
        assert_eq!(
            typed(cmd, &press(Code::KeyV, "v", Modifiers::META)),
            Typed::Paste
        );
        let ctrl = translate_modifiers_for(Modifiers::CONTROL, false);
        assert_eq!(
            typed(ctrl, &press(Code::KeyV, "v", Modifiers::CONTROL)),
            Typed::Paste
        );
        let none = translate_modifiers_for(Modifiers::empty(), true);
        assert_eq!(
            typed(none, &press(Code::KeyA, "a", Modifiers::empty())),
            Typed::Text("a".into())
        );
    }

    #[test]
    fn letters_translate_in_both_cases() {
        let key = |s: &str| crate::translate::translate_virtual_key(&Key::Character(s.into()));
        assert_eq!(key("z"), Some(egui::Key::Z));
        assert_eq!(key("Z"), Some(egui::Key::Z));
    }
}
