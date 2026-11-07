// SPDX-License-Identifier: GPL-3.0-or-later
//! `feline-login`: Feline's login window.
//!
//! Logging in inside the Roblox app needs an Android WebView (the login
//! page's security checks run in one), which Feline doesn't have. So you log
//! in here instead: Roblox's own website in a WebKitGTK window. As soon as
//! the site sets the `.ROBLOSECURITY` session cookie, the window writes that
//! cookie's value to standard output and closes. `feline-runtime --login`
//! runs this program and saves the cookie (see feline-runtime's login.rs).
//!
//! What it keeps: nothing. The browser session is ephemeral (no cookies,
//! cache or history on disk), and the cookie only goes to the pipe. It
//! refuses to print it to a terminal, where it would stay in the scrollback.
//! The window only goes to Roblox's own website over HTTPS; links elsewhere
//! are blocked (pages it embeds, such as the captcha, still load).

use std::cell::Cell;
use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::rc::Rc;

use webkit6::prelude::*;
use webkit6::{gio, glib, gtk};

const LOGIN_URL: &str = "https://www.roblox.com/login";
const COOKIE_URL: &str = "https://www.roblox.com/";
const COOKIE_NAME: &str = ".ROBLOSECURITY";

/// Roblox's website: `roblox.com` and its subdomains, over HTTPS.
fn is_roblox(uri: &str) -> bool {
    let Some(rest) = uri.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    // No user info (`user@host`) tricks.
    if host.contains('@') {
        return false;
    }
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    host == "roblox.com" || host.ends_with(".roblox.com")
}

fn main() -> ExitCode {
    if std::io::stdout().is_terminal() {
        eprintln!(
            "feline-login: this prints your session cookie, so it won't write it to a terminal.\n\
             Run `feline-runtime --login` instead, which saves it for you."
        );
        return ExitCode::FAILURE;
    }
    if let Err(e) = gtk::init() {
        eprintln!("feline-login: couldn't open a window: {e}");
        return ExitCode::FAILURE;
    }

    let main_loop = glib::MainLoop::new(None, false);
    let logged_in = Rc::new(Cell::new(false));

    // Nothing of the browser session is kept on disk.
    let session = webkit6::NetworkSession::new_ephemeral();
    let view = webkit6::WebView::builder().network_session(&session).build();

    // Only Roblox's website in the window itself. Navigation decisions also
    // come for embedded frames (the captcha is one), which must load, so
    // the page's own address is checked as it loads instead.
    view.connect_load_changed(|view, event| {
        if !matches!(
            event,
            webkit6::LoadEvent::Started | webkit6::LoadEvent::Redirected
        ) {
            return;
        }
        if let Some(uri) = view.uri()
            && !is_roblox(&uri)
            && uri != "about:blank"
        {
            eprintln!(
                "feline-login: not leaving Roblox's website (for {})",
                uri.split(['?', '#']).next().unwrap_or("")
            );
            view.stop_loading();
            view.load_uri(LOGIN_URL);
        }
    });
    // No pop-up windows: a link meant for one opens here if it's Roblox's.
    view.connect_create(|view, action| {
        if let Some(uri) = action.clone().request().and_then(|r| r.uri())
            && is_roblox(&uri)
        {
            view.load_uri(&uri);
        }
        None
    });

    let window = gtk::Window::builder()
        .title("Log in to Roblox – Feline")
        .default_width(520)
        .default_height(760)
        .child(&view)
        .build();

    {
        let main_loop = main_loop.clone();
        window.connect_close_request(move |_| {
            main_loop.quit();
            glib::Propagation::Proceed
        });
    }

    // Each time the site changes its cookies, look for the session one.
    if let Some(cookies) = session.cookie_manager() {
        let main_loop = main_loop.clone();
        let logged_in = logged_in.clone();
        let window = window.clone();
        cookies.connect_changed(move |manager| {
            let main_loop = main_loop.clone();
            let logged_in = logged_in.clone();
            let window = window.clone();
            manager.cookies(COOKIE_URL, None::<&gio::Cancellable>, move |result| {
                if logged_in.get() {
                    return;
                }
                let Ok(list) = result else { return };
                for mut cookie in list {
                    let is_session = cookie.name().as_deref() == Some(COOKIE_NAME)
                        && cookie.domain_matches("www.roblox.com");
                    let Some(value) = cookie.value().filter(|v| is_session && !v.is_empty())
                    else {
                        continue;
                    };
                    let mut out = std::io::stdout().lock();
                    if out
                        .write_all(value.as_bytes())
                        .and_then(|()| out.write_all(b"\n"))
                        .and_then(|()| out.flush())
                        .is_err()
                    {
                        eprintln!("feline-login: couldn't hand over the login");
                        return;
                    }
                    logged_in.set(true);
                    window.close();
                    main_loop.quit();
                    return;
                }
            });
        });
    }

    view.load_uri(LOGIN_URL);
    window.present();
    main_loop.run();

    if logged_in.get() {
        ExitCode::SUCCESS
    } else {
        eprintln!("feline-login: closed without logging in");
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::is_roblox;

    #[test]
    fn only_roblox_over_https() {
        assert!(is_roblox("https://www.roblox.com/login"));
        assert!(is_roblox("https://roblox.com/"));
        assert!(is_roblox("https://auth.roblox.com/v2/login?x=1"));
        assert!(is_roblox("https://WWW.Roblox.com"));
        assert!(!is_roblox("http://www.roblox.com/login"));
        assert!(!is_roblox("https://roblox.com.evil.test/"));
        assert!(!is_roblox("https://evilroblox.com/"));
        assert!(!is_roblox("https://www.roblox.com@evil.test/"));
        assert!(!is_roblox("https://evil.test/?https://www.roblox.com"));
    }
}
