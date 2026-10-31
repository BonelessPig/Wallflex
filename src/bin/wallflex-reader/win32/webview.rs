//! The `WebView2` control filling the reader's window: Edge's engine,
//! which comes with Windows 11, so nothing is bundled. It shows the
//! page (`shell.html`), serves the guides folder on a virtual host, and
//! reports the page's requests, link clicks and the Esc key to the
//! window through [`super::handle`].

use std::path::Path;
use std::sync::mpsc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_COLOR, COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY_CORS,
    COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC,
    CreateCoreWebView2EnvironmentWithOptions, ICoreWebView2, ICoreWebView2_3,
    ICoreWebView2Controller, ICoreWebView2Controller2,
    ICoreWebView2Environment, ICoreWebView2EnvironmentOptions,
    ICoreWebView2NavigationStartingEventArgs,
    ICoreWebView2NavigationStartingEventHandler,
};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CoreWebView2EnvironmentOptions,
    CreateCoreWebView2ControllerCompletedHandler,
    CreateCoreWebView2EnvironmentCompletedHandler,
    NavigationStartingEventHandler, NewWindowRequestedEventHandler,
    WebMessageReceivedEventHandler, ZoomFactorChangedEventHandler, take_pwstr,
};
use windows::Win32::Foundation::{E_POINTER, HWND, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::core::{HSTRING, Interface, PWSTR};

use super::Event;
use crate::guides;
use crate::messages::{self, Navigation};

/// The page around the guides.
const SHELL: &str = include_str!("../shell.html");

/// The control and its browser.
pub struct WebView {
    /// Sizes, shows and focuses the control.
    controller: ICoreWebView2Controller,
    /// The browser inside.
    webview: ICoreWebView2,
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

    /// Fits the control to the window's client area, `width` x `height`.
    pub fn fit(&self, width: i32, height: i32) {
        let bounds = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
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
        Navigation::Allow | Navigation::Block => {}
    }
}

/// A Windows error as text.
fn describe(error: &windows::core::Error) -> String {
    format!("{} ({:#010X})", error.message(), error.code().0)
}
