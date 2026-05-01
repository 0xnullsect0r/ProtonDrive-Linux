/// protondrive-hv — standalone GTK3/webkit2gtk-4.1 subprocess
/// Loaded as a child process by protondrive-ui when Proton requires
/// Human Verification. Prints {"token":"…","tokenType":"…"} to stdout
/// then exits. Uses a completely separate GTK3 process so it doesn't
/// conflict with the parent's GTK4 instance.
use gtk::prelude::*;
use webkit2gtk::{
    UserContentInjectedFrames, UserContentManagerExt, UserScript,
    UserScriptInjectionTime, WebViewExt,
};
// Bring in ValueExt so .to_json() is available on JSC values
use javascriptcore::ValueExt;

const INTERCEPT_JS: &str = r#"
(function() {
    "use strict";
    var _sent = false;

    function capture(data) {
        if (_sent || !data) return;
        var tok = data.token || data.captcha_token || "";
        if (!tok) return;
        var tt = data.tokenType
            || (data.type === "pm_captcha" ? "captcha"
                : (data.type || "captcha"));
        _sent = true;
        try {
            /* Pass object directly so to_json() in Rust gives clean JSON */
            window.webkit.messageHandlers.hvCapture.postMessage(
                { token: tok, tokenType: tt }
            );
        } catch (e) {}
    }

    /* Make verify.proton.me think we are embedded in a Proton page */
    try {
        Object.defineProperty(window, "parent", {
            configurable: true,
            get: function () {
                return {
                    postMessage: function (data, _origin) { capture(data); }
                };
            }
        });
    } catch (_e) {}

    /* Also catch raw window.message events just in case */
    window.addEventListener("message", function (e) {
        capture(e.data);
    }, true);
})();
"#;

fn main() {
    // Args: protondrive-hv <hv_token> [<methods>]
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: protondrive-hv <hv_token> [captcha]");
        std::process::exit(1);
    }
    let hv_token = &args[1];
    let methods = args.get(2).map(|s| s.as_str()).unwrap_or("captcha");

    let url = format!(
        "https://verify.proton.me/?methods={}&token={}&theme=dark",
        methods, hv_token
    );

    gtk::init().expect("GTK init failed");

    let ucm = webkit2gtk::UserContentManager::new();

    let script = UserScript::new(
        INTERCEPT_JS,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::Start,
        &[],
        &[],
    );
    ucm.add_script(&script);
    ucm.register_script_message_handler("hvCapture");

    // Capture message from JS → print to stdout → quit
    ucm.connect_script_message_received(Some("hvCapture"), move |_ucm, result| {
        if let Some(js_val) = result.js_value() {
            if let Some(json) = js_val.to_json(0) {
                println!("{}", json.as_str().trim());
                let _ = std::io::Write::flush(&mut std::io::stdout());
                gtk::main_quit();
            }
        }
    });

    let wv = webkit2gtk::WebView::with_user_content_manager(&ucm);
    wv.load_uri(&url);

    let win = gtk::Window::new(gtk::WindowType::Toplevel);
    win.set_title("Proton Security Verification");
    win.set_default_size(480, 540);
    win.connect_destroy(|_| gtk::main_quit());
    win.add(&wv);
    win.show_all();

    // 5-minute safety timeout
    glib::timeout_add_seconds(300, || {
        gtk::main_quit();
        glib::ControlFlow::Break
    });

    gtk::main();
}
