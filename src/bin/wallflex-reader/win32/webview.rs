//! The `WebView2` control filling the reader's window: Edge's engine,
//! which comes with Windows 11, so nothing is bundled. It shows the
//! page (`shell.html`), serves the guides folder on a virtual host, and
//! reports the page's requests, link clicks and the Esc key to the
//! window through [`super::handle`].

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_COLOR, COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
    COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2_2,
    ICoreWebView2_3, ICoreWebView2Controller, ICoreWebView2Controller2,
    ICoreWebView2Environment, ICoreWebView2EnvironmentOptions,
    ICoreWebView2NavigationStartingEventArgs,
    ICoreWebView2NavigationStartingEventHandler,
};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
    DOMContentLoadedEventHandler, NavigationStartingEventHandler,
    NewWindowRequestedEventHandler, WebMessageReceivedEventHandler,
    ZoomFactorChangedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::{HSTRING, Interface, PWSTR};

use super::Event;
use crate::claude;
use crate::guides;
use crate::messages::{self, Navigation};

/// The page around the guides.
const SHELL: &str = include_str!("../shell.html");

thread_local! {
    /// The Claude view's control, between the browser making it and the
    /// window picking it up.
    static PENDING: RefCell<Option<ICoreWebView2Controller>> =
        const { RefCell::new(None) };
}

/// The control and its browser.
pub struct WebView {
    /// Sizes, shows and focuses the control.
    controller: ICoreWebView2Controller,
    /// The browser inside.
    webview: ICoreWebView2,
    /// Starts more views (the Claude page) in the same browser.
    environment: ICoreWebView2Environment,
}

/// A second control for claude.ai, over part of the window. It has no
/// link to the page: it only navigates, and its popups and outside
/// links go to the default browser.
pub struct Site {
    /// Sizes, shows and zooms the control.
    controller: ICoreWebView2Controller,
    /// The browser inside.
    webview: ICoreWebView2,
    /// The script that gives claude.ai the reading style (empty: none
    /// yet), run again as each page loads.
    look: Rc<RefCell<String>>,
}

impl WebView {
    /// Creates the control in `parent`, keeping the browser's data in
    /// `data_dir` and serving `folder` (the guides) on
    /// [`guides::HOST`]. `background` shows until the page draws.
    ///
    /// # Errors
    ///
    /// Why `WebView2` couldn't start (most likely it isn't installed).
    pub fn create(
        parent: HWND,
        data_dir: &Path,
        folder: &Path,
        background: COREWEBVIEW2_COLOR,
    ) -> Result<Self, String> {
        let environment = environment(data_dir)?;
        let controller = controller(&environment, parent)?;
        // SAFETY: COM calls on live interfaces, on the thread that made
        // them; the strings outlive the calls.
        let webview = unsafe {
            let webview =
                controller.CoreWebView2().map_err(|e| describe(&e))?;
            let settings = webview.Settings().map_err(|e| describe(&e))?;
            settings
                .SetIsStatusBarEnabled(false)
                .map_err(|e| describe(&e))?;
            settings
                .SetAreDevToolsEnabled(cfg!(debug_assertions))
                .map_err(|e| describe(&e))?;
            if let Ok(controller) =
                controller.cast::<ICoreWebView2Controller2>()
            {
                controller
                    .SetDefaultBackgroundColor(background)
                    .map_err(|e| describe(&e))?;
            }
            webview
                .cast::<ICoreWebView2_3>()
                .map_err(|e| describe(&e))?
                .SetVirtualHostNameToFolderMapping(
                    &HSTRING::from(guides::HOST),
                    &HSTRING::from(folder.as_os_str()),
                    COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
                )
                .map_err(|e| describe(&e))?;
            webview
        };
        let view = Self {
            controller,
            webview,
            environment,
        };
        view.listen()?;
        // SAFETY: as above.
        unsafe {
            view.webview
                .NavigateToString(&HSTRING::from(SHELL))
                .map_err(|e| describe(&e))?;
        }
        Ok(view)
    }

    /// Hooks up the events the window needs.
    fn listen(&self) -> Result<(), String> {
        let mut token = 0_i64;
        // SAFETY: COM calls on live interfaces; `token` is a valid place
        // for each call to write. The handlers live as long as the
        // control, which drops them when it closes.
        unsafe {
            self.webview
                .add_WebMessageReceived(
                    &WebMessageReceivedEventHandler::create(Box::new(
                        |_, args| {
                            let Some(args) = args else {
                                return Ok(());
                            };
                            let mut json = PWSTR::null();
                            args.WebMessageAsJson(&raw mut json)?;
                            let json = take_pwstr(json);
                            if let Some(request) =
                                messages::parse_request(&json)
                            {
                                super::handle(Event::Request(request));
                            }
                            Ok(())
                        },
                    )),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
            self.webview
                .add_NavigationStarting(
                    &navigation_handler(true),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
            self.webview
                .add_FrameNavigationStarting(
                    &navigation_handler(false),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
            self.webview
                .add_NewWindowRequested(
                    &NewWindowRequestedEventHandler::create(Box::new(
                        |_, args| {
                            let Some(args) = args else {
                                return Ok(());
                            };
                            args.SetHandled(true)?;
                            let mut uri = PWSTR::null();
                            args.Uri(&raw mut uri)?;
                            follow(messages::judge(&take_pwstr(uri), true));
                            Ok(())
                        },
                    )),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
            self.controller
                .add_ZoomFactorChanged(
                    &ZoomFactorChangedEventHandler::create(Box::new(
                        |controller, _| {
                            let Some(controller) = controller else {
                                return Ok(());
                            };
                            let mut zoom = 1.0_f64;
                            controller.ZoomFactor(&raw mut zoom)?;
                            super::handle(Event::Zoomed(zoom));
                            Ok(())
                        },
                    )),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
            self.controller
                .add_AcceleratorKeyPressed(
                    &AcceleratorKeyPressedEventHandler::create(Box::new(
                        |_, args| {
                            let Some(args) = args else {
                                return Ok(());
                            };
                            let mut kind =
                                COREWEBVIEW2_KEY_EVENT_KIND::default();
                            let mut key = 0_u32;
                            args.KeyEventKind(&raw mut kind)?;
                            args.VirtualKey(&raw mut key)?;
                            let escape = u32::from(VK_ESCAPE.0);
                            if kind == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                                && key == escape
                            {
                                args.SetHandled(true)?;
                                super::handle(Event::Escape);
                            }
                            Ok(())
                        },
                    )),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
        }
        Ok(())
    }

    /// Sends the page a message (JSON).
    pub fn post(&self, json: &str) {
        // SAFETY: COM call on a live interface; the string outlives it.
        // A lost message only leaves the page as it was.
        let _ =
            unsafe { self.webview.PostWebMessageAsJson(&HSTRING::from(json)) };
    }

    /// Fits the control to `bounds` in the window's client area.
    pub fn fit(&self, bounds: RECT) {
        // SAFETY: COM calls on a live interface. If they fail the page
        // keeps its old size until the next move.
        unsafe {
            let _ = self.controller.SetBounds(bounds);
            let _ = self.controller.NotifyParentWindowPositionChanged();
        }
    }

    /// Shows or hides the control (hidden, the browser does less).
    pub fn set_visible(&self, visible: bool) {
        // SAFETY: COM call on a live interface; failing changes nothing
        // that matters.
        let _ = unsafe { self.controller.SetIsVisible(visible) };
    }

    /// Zooms the page (1 = its usual size at the monitor's scaling).
    pub fn set_zoom(&self, zoom: f64) {
        // SAFETY: COM call on a live interface; failing leaves the zoom.
        let _ = unsafe { self.controller.SetZoomFactor(zoom) };
    }

    /// Gives the page the keyboard.
    pub fn focus(&self) {
        // SAFETY: as above.
        let _ = unsafe {
            self.controller
                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
        };
    }

    /// Starts the Claude view in `parent`, hidden until placed.
    ///
    /// # Errors
    ///
    /// Why the control couldn't be made.
    pub fn begin_site(&self, parent: HWND, notify: HWND) -> Result<(), String> {
        let handler = CreateCoreWebView2ControllerCompletedHandler::create(
            Box::new(move |result, controller| {
                result?;
                PENDING.with_borrow_mut(|slot| *slot = controller);
                // SAFETY: posting to our own window; plain values. If
                // it fails the Claude page just stays empty.
                let _ = unsafe {
                    PostMessageW(
                        Some(notify),
                        super::window::WM_SITE_READY,
                        WPARAM(0),
                        LPARAM(0),
                    )
                };
                Ok(())
            }),
        );
        // SAFETY: COM call on a live interface; the handler outlives it.
        unsafe {
            self.environment
                .CreateCoreWebView2Controller(parent, &handler)
                .map_err(|e| describe(&e))
        }
    }

    /// Finishes the view started by [`Self::begin_site`], once its
    /// window has been told it is ready.
    ///
    /// # Errors
    ///
    /// Why the view couldn't be set up.
    pub fn finish_site() -> Result<Site, String> {
        let controller = PENDING
            .with_borrow_mut(Option::take)
            .ok_or_else(|| "The Claude view didn't start.".to_owned())?;
        // SAFETY: COM calls on live interfaces, on the thread that made
        // them.
        let webview = unsafe {
            controller.SetIsVisible(false).map_err(|e| describe(&e))?;
            let webview =
                controller.CoreWebView2().map_err(|e| describe(&e))?;
            let settings = webview.Settings().map_err(|e| describe(&e))?;
            settings
                .SetIsStatusBarEnabled(false)
                .map_err(|e| describe(&e))?;
            settings
                .SetAreDevToolsEnabled(cfg!(debug_assertions))
                .map_err(|e| describe(&e))?;
            webview
        };
        let site = Site {
            controller,
            webview,
            look: Rc::new(RefCell::new(String::new())),
        };
        site.listen()?;
        site.keep_look()?;
        Ok(site)
    }

    /// Closes the browser.
    pub fn close(&self) {
        // SAFETY: as above; it's closing anyway.
        let _ = unsafe { self.controller.Close() };
    }
}

impl Site {
    /// Keeps the view on Claude's hosts: other addresses, and popups,
    /// open in the default browser (Claude's own popups, such as
    /// sign-in, load in the view).
    fn listen(&self) -> Result<(), String> {
        let mut token = 0_i64;
        // SAFETY: COM calls on live interfaces; `token` is a valid place
        // for each call to write. The handlers live as long as the
        // control, which drops them when it closes.
        unsafe {
            self.webview
                .add_NavigationStarting(
                    &NavigationStartingEventHandler::create(Box::new(
                        |_, args| {
                            let Some(args) = args else {
                                return Ok(());
                            };
                            let uri = uri_of(&args)?;
                            let lower = uri.to_ascii_lowercase();
                            if lower.starts_with("https:")
                                || lower.starts_with("about:")
                            {
                                return Ok(());
                            }
                            args.SetCancel(true)?;
                            follow(messages::judge(&uri, true));
                            Ok(())
                        },
                    )),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
            self.webview
                .add_NewWindowRequested(
                    &NewWindowRequestedEventHandler::create(Box::new(
                        |view, args| {
                            let (Some(view), Some(args)) = (view, args) else {
                                return Ok(());
                            };
                            args.SetHandled(true)?;
                            let mut uri = PWSTR::null();
                            args.Uri(&raw mut uri)?;
                            let uri = take_pwstr(uri);
                            if claude::is_claude_host(&uri) {
                                view.Navigate(&HSTRING::from(uri))?;
                            } else {
                                follow(messages::judge(&uri, true));
                            }
                            Ok(())
                        },
                    )),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
        }
        Ok(())
    }

    /// Styles every page Claude loads with the current look, once its
    /// document is there (Claude's own navigations within a page keep
    /// the stylesheet already added).
    fn keep_look(&self) -> Result<(), String> {
        let look = Rc::clone(&self.look);
        let mut token = 0_i64;
        // SAFETY: COM calls on live interfaces; `token` is a valid place
        // to write. The handler lives as long as the control.
        unsafe {
            self.webview
                .cast::<ICoreWebView2_2>()
                .map_err(|e| describe(&e))?
                .add_DOMContentLoaded(
                    &DOMContentLoadedEventHandler::create(Box::new(
                        move |view, _| {
                            let script = look.borrow();
                            if let Some(view) = view
                                && !script.is_empty()
                            {
                                view.ExecuteScript(
                                    &HSTRING::from(script.as_str()),
                                    None,
                                )?;
                            }
                            Ok(())
                        },
                    )),
                    &raw mut token,
                )
                .map_err(|e| describe(&e))?;
        }
        Ok(())
    }

    /// Uses `script` (from [`crate::claude::style_script`]) to style
    /// the page now and every page after.
    pub fn restyle(&self, script: String) {
        // SAFETY: COM call on a live interface; the string outlives it.
        // If it fails the page keeps its look until the next load.
        let _ = unsafe {
            self.webview.ExecuteScript(&HSTRING::from(&script), None)
        };
        *self.look.borrow_mut() = script;
    }

    /// Shows the page at `url`.
    pub fn navigate(&self, url: &str) {
        // SAFETY: COM call on a live interface; the string outlives it.
        // If it fails the view keeps what it showed.
        let _ = unsafe { self.webview.Navigate(&HSTRING::from(url)) };
    }

    /// The address shown now (claude.ai's own page changes included);
    /// `None` if it can't be read.
    pub fn source(&self) -> Option<String> {
        let mut uri = PWSTR::null();
        // SAFETY: `uri` is a valid place for the call to write; the
        // string it gets is ours to free, which `take_pwstr` does.
        unsafe { self.webview.Source(&raw mut uri) }.ok()?;
        Some(take_pwstr(uri))
    }

    /// Fits the control to `bounds` in the window's client area.
    pub fn fit(&self, bounds: RECT) {
        // SAFETY: COM calls on a live interface. If they fail the view
        // keeps its old place until the next move.
        unsafe {
            let _ = self.controller.SetBounds(bounds);
            let _ = self.controller.NotifyParentWindowPositionChanged();
        }
    }

    /// Shows or hides the control.
    pub fn set_visible(&self, visible: bool) {
        // SAFETY: COM call on a live interface; failing changes nothing
        // that matters.
        let _ = unsafe { self.controller.SetIsVisible(visible) };
    }

    /// Gives the view the keyboard.
    pub fn focus(&self) {
        // SAFETY: as above.
        let _ = unsafe {
            self.controller
                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
        };
    }

    /// Zooms the page like the reader's own.
    pub fn set_zoom(&self, zoom: f64) {
        // SAFETY: COM call on a live interface; failing leaves the zoom.
        let _ = unsafe { self.controller.SetZoomFactor(zoom) };
    }

    /// Closes the browser.
    pub fn close(&self) {
        // SAFETY: as above; it's closing anyway.
        let _ = unsafe { self.controller.Close() };
    }
}

/// Starts `WebView2`'s browser, keeping its data in `data_dir`.
fn environment(data_dir: &Path) -> Result<ICoreWebView2Environment, String> {
    let data_dir = HSTRING::from(data_dir.as_os_str());
    let options: ICoreWebView2EnvironmentOptions =
        CoreWebView2EnvironmentOptions::default().into();
    let (sender, receiver) = mpsc::channel();
    CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            // SAFETY: the strings and options outlive the call, which
            // copies what it needs.
            unsafe {
                CreateCoreWebView2EnvironmentWithOptions(
                    windows::core::PCWSTR::null(),
                    &data_dir,
                    &options,
                    &handler,
                )
            }
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |result, environment| {
            result?;
            // The receiver waits below, so sending can't fail.
            let _ = sender.send(environment.ok_or(E_POINTER));
            Ok(())
        }),
    )
    .map_err(|error| format!("{error:?}"))?;
    receiver
        .recv()
        .map_err(|error| error.to_string())?
        .map_err(|error| describe(&error.into()))
}

/// Creates the control inside `parent`.
fn controller(
    environment: &ICoreWebView2Environment,
    parent: HWND,
) -> Result<ICoreWebView2Controller, String> {
    let environment = environment.clone();
    let (sender, receiver) = mpsc::channel();
    CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            // SAFETY: `parent` is our live window; COM call on a live
            // interface.
            unsafe {
                environment.CreateCoreWebView2Controller(parent, &handler)
            }
            .map_err(webview2_com::Error::WindowsError)
        }),
        Box::new(move |result, controller| {
            result?;
            // The receiver waits below, so sending can't fail.
            let _ = sender.send(controller.ok_or(E_POINTER));
            Ok(())
        }),
    )
    .map_err(|error| format!("{error:?}"))?;
    receiver
        .recv()
        .map_err(|error| error.to_string())?
        .map_err(|error| describe(&error.into()))
}

/// A handler that lets the page and the guides load and stops anything
/// else, for the whole page (`top`) or the guide's frame.
fn navigation_handler(
    top: bool,
) -> ICoreWebView2NavigationStartingEventHandler {
    NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let verdict = messages::judge(&uri_of(&args)?, top);
        if verdict != Navigation::Allow {
            // SAFETY: COM call on the live event arguments.
            unsafe { args.SetCancel(true)? };
            follow(verdict);
        }
        Ok(())
    }))
}

/// The address a navigation is going to.
fn uri_of(
    args: &ICoreWebView2NavigationStartingEventArgs,
) -> windows::core::Result<String> {
    let mut uri = PWSTR::null();
    // SAFETY: `uri` is a valid place for the call to write; the string
    // it gets is ours to free, which `take_pwstr` does.
    unsafe { args.Uri(&raw mut uri)? };
    Ok(take_pwstr(uri))
}

/// Acts on a stopped navigation: shows a guide or opens a link.
fn follow(verdict: Navigation) {
    match verdict {
        Navigation::Guide(name) => super::handle(Event::Open(name)),
        Navigation::External(url) => super::open_external(&url),
        Navigation::Read(url) => super::handle(Event::OpenWeb(url)),
        Navigation::Back => super::handle(Event::WebBack),
        Navigation::Save => super::handle(Event::WebSave),
        Navigation::Allow | Navigation::Block => {}
    }
}

/// A Windows error as text.
fn describe(error: &windows::core::Error) -> String {
    format!("{} ({:#010X})", error.message(), error.code().0)
}
