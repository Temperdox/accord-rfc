//! Auto-grant the webview's media (microphone / camera) permission so the app
//! never shows a per-page consent popup for its own first-party UI - the same
//! approach Electron apps (e.g. Discord desktop) use to approve their own
//! `getUserMedia`. The OS-level mic/camera privacy controls still apply, so the
//! user keeps real control; this only suppresses the in-webview Chromium/WebKit
//! prompt.
//!
//! Trust boundary: only our trusted top-level UI calls `getUserMedia`. Sandboxed
//! content (`src/sandbox`, `<iframe sandbox="allow-scripts">` with no
//! `allow-same-origin` and no `allow="microphone"`) cannot reach media capture,
//! so auto-granting does not widen what embedded content can do.
//!
//! Per platform this hooks the native webview behind `WebviewWindow::with_webview`:
//! WebView2 `PermissionRequested` (Windows), WebKitGTK `permission-request`
//! (Linux). macOS (WKWebView) needs a `WKUIDelegate` and the Info.plist usage
//! strings; that path is not wired yet (see the bottom of this file).

use tauri::WebviewWindow;

/// Install a media-permission auto-grant handler on `window`'s native webview.
/// Best-effort and idempotent at the call site; failures are logged, not fatal.
pub fn auto_grant_media(window: &WebviewWindow) {
    let res = window.with_webview(|webview| {
        #[cfg(windows)]
        windows_impl::grant(&webview);
        #[cfg(target_os = "linux")]
        linux_impl::grant(&webview);
        #[cfg(target_os = "macos")]
        {
            // WKWebView auto-grant (WKUIDelegate) is not wired yet; the Info.plist
            // usage strings ship so the OS permission can be granted at least.
            let _ = &webview;
        }
    });
    if let Err(e) = res {
        tracing::warn!("could not install webview media-permission handler: {e}");
    }
}

#[cfg(windows)]
mod windows_impl {
    use tauri::webview::PlatformWebview;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA,
        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    };
    use webview2_com::PermissionRequestedEventHandler;

    /// Approve microphone + camera `PermissionRequested` events on the WebView2
    /// core, so capture starts without the browser-style consent popup.
    pub fn grant(webview: &PlatformWebview) {
        // SAFETY: runs on the UI thread during setup; the WebView2 COM objects
        // are valid for the lifetime of the window.
        unsafe {
            let core = match webview.controller().CoreWebView2() {
                Ok(core) => core,
                Err(e) => {
                    tracing::warn!("WebView2 core unavailable for media auto-grant: {e}");
                    return;
                }
            };
            let handler = PermissionRequestedEventHandler::create(Box::new(|_sender, args| {
                if let Some(args) = args {
                    let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                    args.PermissionKind(&mut kind)?;
                    if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE
                        || kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA
                    {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                    }
                }
                Ok(())
            }));
            let mut token: i64 = 0;
            if let Err(e) = core.add_PermissionRequested(&handler, &mut token) {
                tracing::warn!("add_PermissionRequested failed: {e}");
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_impl {
    use tauri::webview::PlatformWebview;
    use webkit2gtk::prelude::*;

    /// Auto-allow WebKitGTK user-media (mic/camera) permission requests.
    pub fn grant(webview: &PlatformWebview) {
        let wv = webview.inner();
        wv.connect_permission_request(|_wv, req| {
            if req.is::<webkit2gtk::UserMediaPermissionRequest>() {
                req.allow();
                true // handled
            } else {
                false // let the default handler decide other permissions
            }
        });
    }
}
