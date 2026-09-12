//! The portal, compiled into the binary.
//!
//! Every byte the page needs is an `include_bytes!` below — the HTML, the one
//! stylesheet, the one script, and the five IBM Plex faces the design uses.
//! There is no build step, no bundler and no npm, and the server's
//! Content-Security-Policy allows no origin but `'self'`, so a page that
//! reached for a CDN would simply fail rather than quietly work on a machine
//! with a network and break on one without.
//!
//! That is the property the release is actually selling: the portal loads with
//! the cable unplugged, and there is no configuration in which it does not.

use crate::http::Response;

const INDEX: &[u8] = include_bytes!("index.html");
const STYLE: &[u8] = include_bytes!("style.css");
const APP: &[u8] = include_bytes!("app.js");

/// IBM Plex Sans and Mono, latin subsets, under the SIL Open Font License 1.1.
/// The licence travels with them rather than being referenced from somewhere.
const SANS_400: &[u8] = include_bytes!("fonts/IBMPlexSans-Regular.woff2");
const SANS_500: &[u8] = include_bytes!("fonts/IBMPlexSans-Medium.woff2");
const SANS_600: &[u8] = include_bytes!("fonts/IBMPlexSans-SemiBold.woff2");
const MONO_400: &[u8] = include_bytes!("fonts/IBMPlexMono-Regular.woff2");
const MONO_500: &[u8] = include_bytes!("fonts/IBMPlexMono-Medium.woff2");
const FONT_LICENSE: &[u8] = include_bytes!("fonts/LICENSE.txt");

/// The mark, in both themes. SVG rather than the design's PNGs: the same
/// artwork at a twentieth of the bytes, and sharp at 26px in a topbar and 38px
/// on the welcome screen without shipping two rasters of each.
const LOGO: &[u8] = include_bytes!("logo.svg");
const LOGO_DARK: &[u8] = include_bytes!("logo-dark.svg");

/// The tab and home-screen icons. Served from the binary like everything else,
/// so a browser asking for `/favicon.ico` is answered without a network round
/// trip and without the page carrying a data URI of its own.
const FAVICON_ICO: &[u8] = include_bytes!("icons/favicon.ico");
const FAVICON_16: &[u8] = include_bytes!("icons/favicon-16x16.png");
const FAVICON_32: &[u8] = include_bytes!("icons/favicon-32x32.png");
const APPLE_TOUCH: &[u8] = include_bytes!("icons/apple-touch-icon.png");
const ANDROID_192: &[u8] = include_bytes!("icons/android-chrome-192x192.png");
const ANDROID_512: &[u8] = include_bytes!("icons/android-chrome-512x512.png");
const WEBMANIFEST: &[u8] = include_bytes!("icons/site.webmanifest");

/// Everything served from the binary, as `(route, media type, bytes)`.
///
/// One table rather than a match arm each, so [`asset`] and the size assertion
/// in the tests below read the same list and cannot disagree about what ships.
const ASSETS: &[(&str, &str, &[u8])] = &[
    ("/style.css", "text/css; charset=utf-8", STYLE),
    ("/app.js", "text/javascript; charset=utf-8", APP),
    ("/logo.svg", "image/svg+xml", LOGO),
    ("/logo-dark.svg", "image/svg+xml", LOGO_DARK),
    ("/favicon.ico", "image/x-icon", FAVICON_ICO),
    ("/icons/favicon-16x16.png", "image/png", FAVICON_16),
    ("/icons/favicon-32x32.png", "image/png", FAVICON_32),
    ("/icons/apple-touch-icon.png", "image/png", APPLE_TOUCH),
    (
        "/icons/android-chrome-192x192.png",
        "image/png",
        ANDROID_192,
    ),
    (
        "/icons/android-chrome-512x512.png",
        "image/png",
        ANDROID_512,
    ),
    (
        "/icons/site.webmanifest",
        "application/manifest+json",
        WEBMANIFEST,
    ),
    ("/fonts/IBMPlexSans-Regular.woff2", "font/woff2", SANS_400),
    ("/fonts/IBMPlexSans-Medium.woff2", "font/woff2", SANS_500),
    ("/fonts/IBMPlexSans-SemiBold.woff2", "font/woff2", SANS_600),
    ("/fonts/IBMPlexMono-Regular.woff2", "font/woff2", MONO_400),
    ("/fonts/IBMPlexMono-Medium.woff2", "font/woff2", MONO_500),
    (
        "/fonts/LICENSE.txt",
        "text/plain; charset=utf-8",
        FONT_LICENSE,
    ),
];

/// The page itself.
pub fn page() -> Response {
    Response::asset("text/html; charset=utf-8", INDEX)
}

/// A static file, if the path names one.
pub fn asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    ASSETS
        .iter()
        .find(|(route, _, _)| *route == path)
        .map(|(_, kind, bytes)| (*kind, *bytes))
}

/// Every route the portal itself serves, for the parity test.
pub fn routes() -> Vec<&'static str> {
    let mut out = vec!["/"];
    out.extend(ASSETS.iter().map(|(route, _, _)| *route));
    out
}

/// What the embedded assets cost, uncompressed.
pub fn weight() -> usize {
    INDEX.len()
        + ASSETS
            .iter()
            .map(|(_, _, bytes)| bytes.len())
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract budgets the embedded assets at under 1 MiB uncompressed,
    /// because they are added to every `semlith mcp` process's resident set
    /// whether or not anyone opens a browser.
    #[test]
    fn the_embedded_assets_stay_inside_their_budget() {
        let total = weight();
        assert!(
            total < 1024 * 1024,
            "the portal now weighs {total} bytes, over the 1 MiB budget"
        );
    }

    /// A page that reached for a CDN would fail under the server's CSP rather
    /// than quietly work on a machine with a network. Better to fail here.
    #[test]
    fn nothing_in_the_portal_points_at_another_origin() {
        // Only what a browser parses: the licence text is prose served for a
        // person to read, and the URLs in it are attribution, not loads.
        let parsed: &[(&str, &[u8])] = &[("/", INDEX), ("/style.css", STYLE), ("/app.js", APP)];
        for (route, bytes) in parsed {
            // The SVG namespace is an identifier, not an address: nothing is
            // ever fetched from it, and every inline SVG has to name it.
            let text = String::from_utf8_lossy(bytes).replace("http://www.w3.org/2000/svg", "");
            for needle in ["http://", "https://", "//cdn", "fonts.googleapis", "unpkg"] {
                assert!(
                    !text.contains(needle),
                    "{route} contains {needle:?}, which the CSP forbids loading"
                );
            }
        }
    }

    #[test]
    fn every_asset_the_page_references_is_served() {
        let html = String::from_utf8_lossy(INDEX);
        for reference in ["style.css", "app.js"] {
            assert!(
                html.contains(reference),
                "the page does not load {reference}"
            );
            assert!(
                asset(&format!("/{reference}")).is_some(),
                "{reference} is referenced but not served"
            );
        }
        let css = String::from_utf8_lossy(STYLE);
        for (route, _, _) in ASSETS.iter().filter(|(r, _, _)| r.ends_with(".woff2")) {
            let file = route.trim_start_matches('/');
            assert!(css.contains(file), "{file} ships but no @font-face uses it");
        }
    }
}
