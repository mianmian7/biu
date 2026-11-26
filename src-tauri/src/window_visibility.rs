use tauri::{Emitter, EventTarget, Runtime, WebviewWindow};

pub fn set_main_window_visibility<R: Runtime>(window: &WebviewWindow<R>, visible: bool) {
    let main = window.clone();
    if let Err(err) = window.run_on_main_thread(move || {
        let result = if visible {
            let _ = main.unminimize();
            main.show()
        } else {
            main.hide()
        };
        if let Err(err) = result {
            log::warn!("[window] failed to set main visibility to {visible}: {err}");
            return;
        }
        if visible {
            let _ = main.set_focus();
        }
        if let Err(err) = main.emit_to(
            EventTarget::webview_window(main.label()),
            "biu:window-visibility",
            serde_json::json!({ "visible": visible }),
        ) {
            log::warn!("[window] failed to emit main visibility: {err}");
        }
        update_memory_usage(&main, visible);
    }) {
        log::warn!("[window] failed to dispatch main visibility update: {err}");
    }
}

#[cfg(target_os = "windows")]
fn update_memory_usage<R: Runtime>(window: &WebviewWindow<R>, visible: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    };
    use windows::core::Interface;

    if let Err(err) = window.with_webview(move |webview| unsafe {
        let level = if visible {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        } else {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        };
        let result = webview
            .controller()
            .CoreWebView2()
            .and_then(|core| core.cast::<ICoreWebView2_19>())
            .and_then(|core| core.SetMemoryUsageTargetLevel(level));
        if let Err(err) = result {
            log::debug!("[window] WebView2 memory target unavailable: {err}");
        }
    }) {
        log::debug!("[window] failed to access WebView2: {err}");
    }
}

#[cfg(target_os = "linux")]
fn update_memory_usage<R: Runtime>(_window: &WebviewWindow<R>, visible: bool) {
    if !visible {
        trim_heap();
    }
}

#[cfg(target_os = "linux")]
pub fn trim_heap() {
    #[cfg(target_env = "gnu")]
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(target_os = "linux")]
pub fn configure_webkit_cache<R: Runtime>(webview: tauri::Webview<R>) {
    use webkit2gtk::{CacheModel, WebContextExt, WebViewExt};

    if let Err(err) = webview.with_webview(|webview| {
        if let Some(context) = webview.inner().context() {
            context.set_cache_model(CacheModel::DocumentViewer);
        }
    }) {
        log::debug!("[window] failed to configure WebKitGTK cache: {err}");
    }
}

#[cfg(target_os = "macos")]
fn update_memory_usage<R: Runtime>(window: &WebviewWindow<R>, visible: bool) {
    use objc2_foundation::{NSDate, NSSet};
    use objc2_web_kit::{WKWebView, WKWebsiteDataTypeMemoryCache};

    if visible {
        return;
    }
    if let Err(err) = window.with_webview(|webview| unsafe {
        // Tauri supplies a live WKWebView on the main thread.
        let webview = &*webview.inner().cast::<WKWebView>();
        let store = webview.configuration().websiteDataStore();
        let types = NSSet::from_slice(&[WKWebsiteDataTypeMemoryCache]);
        let completed = block2::RcBlock::new(|| {});
        store.removeDataOfTypes_modifiedSince_completionHandler(
            &types,
            &NSDate::distantPast(),
            &completed,
        );
    }) {
        log::debug!("[window] failed to access WKWebView memory cache: {err}");
    }
}
