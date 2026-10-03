# Gecko

The optional Gecko backend: a headful Firefox driven over WebDriver BiDi,
behind the `gecko` cargo feature. It is the engine that can host WebExtensions,
because Firefox brings its own Wayland window, full-frame-rate rendering,
WebGL, video and WebExtensions.

Firefox owns that window, so the page cannot be drawn inside a TontooUI
window: the browser chrome is a separate, draggable toolbar window. For the
single-window browser use the default WPE engine (see [WPE.md](WPE.md)).

## Lifecycle

```
find_firefox()
  |-- TONTOO_FIREFOX_BIN / FIREFOX_BIN
  |-- managed ESR build (gecko_provision::installed)
  +-- firefox, firefox-esr, firefox-devedition on PATH (smoke-tested)
        |
GeckoPage::launch(options)
  |-- profile_dir(private)  -> <managed>/profile or a temp dir
  |-- write_prefs(port, download_dir, private) -> user.js
  |-- firefox --profile <dir> --new-instance --no-remote
  |            --remote-debugging-port <port> [--kiosk] [--headless]
  |-- ws://127.0.0.1:<port>/session   (retry up to LAUNCH_TIMEOUT)
  |-- session.new + session.subscribe
  +-- browsingContext.getTree -> top-level context
        |
GeckoEngine / GeckoRenderer
  |-- EngineCommand -> run_command() -> BiDi calls -> EngineEvent
  +-- BidiEvent     -> handle_bidi_event() -> EngineEvent
```

## Why a window instead of frames

Firefox cannot render into a foreign surface, so this backend does not
blit anything into TontooUI. Instead:

- Firefox creates its own `xdg_toplevel` and the compositor manages it.
- The engine reports `EngineEvent::EngineWindow { pid, kiosk }` as soon as
  the first window exists. Match `pid` in `CoreWindows::list_windows` to
  find, focus, minimize or close that window from a host app.
- The host draws the browser chrome (address field, tabs, buttons) in its
  own TontooUI window. `examples/tontoo_browser.rs` is exactly that.

> **Note:** Wayland has no protocol for placing a client window at a
> chosen position, and TontooCompositor implements neither
> `xdg_toplevel.set_parent` nor `xdg-foreign-v2`. The toolbar is therefore
> a normal, draggable TontooUI window rather than a child of the Firefox
> window. See [Backend.md](Backend.md) for the frame-based alternative.

## Profile

Persistent sessions share one profile so cookies, logins, history and
installed extensions survive restarts:

| Platform | Path |
|---|---|
| Linux | `~/.local/share/tontoo-webengine/gecko/profile` |
| Windows | `%LOCALAPPDATA%\TontooWebEngine\gecko\profile` |

`TONTOO_FIREFOX_DIR` overrides the managed dir. Private sessions
(`GeckoOptions::private`) get a throwaway directory under the temp dir
that is deleted when Firefox exits.

### Prefs (`user.js`)

Written by [`gecko::profile_prefs`] on every launch. Firefox itself never
edits the file; it uses `prefs.js`.

> **Note:** `remote.active-protocols` must be `3`, not `2`. Firefox only
> registers the `/session` BiDi WebSocket handler when the DevTools layer
> is active too; with BiDi alone the endpoint answers
> `404 Unknown command: /session`. We never speak CDP, we only keep the
> handler registered.

| Pref | Value | Why |
|---|---|---|
| `remote.active-protocols` | `3` | BiDi (2) plus the DevTools layer (1) |
| `remote.enabled` | `true` | Remote agent on loopback |
| `remote.allow-hosts` | `localhost,127.0.0.1` | Loopback only |
| `remote.debugging.port` | launch port | One session per helper |
| `app.update.auto` / `app.update.enabled` | `false` | [`gecko_provision`](Backend.md) owns updates |
| `extensions.autoDisableScopes` | `0` | Keep policy, sideloaded and temporary add-ons alive |
| `extensions.enabledScopes` | `15` | All scopes enabled |
| `extensions.update.enabled` | `true` | Add-ons update themselves from AMO |
| `xpinstall.signatures.required` | `false` | Allow unsigned local packages (see below) |
| `browser.download.dir` | download dir | Downloads land where the helper expects them |
| `browser.download.useDownloadDir` | `true` | No save dialog |
| `browser.tabs.inTitlebar` | `0` | Firefox chrome is hidden; the host draws it |
| `datareporting.*`, `toolkit.telemetry.*` | disabled | No telemetry, no studies |
| `gfx.webrender.*` | unset | Let Firefox pick WebRender or software GL; forcing it breaks hosts without hardware GL |
| `media.autoplay.*` | permissive | Videos play without a user gesture |

> **Note:** `xpinstall.signatures.required` is honoured on ESR and
> DevEdition builds. Official release builds ignore it and then only
> accept AMO-signed extensions. TontooOS ships and provisions ESR for
> exactly this reason.

## Launch arguments

```rust
fn firefox_args(
    profile: &Path,
    port: u16,
    kiosk: bool,
    headless: bool,
    private: bool,
    start_url: Option<&str>,
) -> Vec<String>
```

| Flag | Condition | Effect |
|---|---|---|
| `--profile <dir>` | always | Managed or temporary profile |
| `--new-instance --no-remote` | always | Never attach to a running Firefox |
| `--remote-debugging-port <port>` | always | BiDi WebSocket on loopback |
| `--window-size=W,H` | `window_size` set | Initial window size |
| `--kiosk` | `kiosk && !headless` | Chrome-less fullscreen (opt-in, see below) |
| `--headless` | `headless` | No window (tests, servers) |
| `--private-window` | `private` | Ephemeral session |
| start URL | always | `about:blank` when empty |

> **Note:** `kiosk` defaults to **off**. `--kiosk` sends an
> `xdg_toplevel.fullscreen` request; a compositor that confirms it with a
> `0 x 0` size aborts Firefox with
> `xdg_surface buffer (1 x 1) is larger than the configured fullscreen
> state (0 x 0)` (seen on WSLg). TontooCompositor does not implement
> `fullscreen_request` yet, so leave `TONTOO_BROWSER_KIOSK` unset until
> it does and use `TONTOO_BROWSER_SIZE=1200x800` instead.

### When Firefox dies

Every BiDi call goes through an enriching wrapper, so a dead connection
reports the cause instead of `closed connection`:

```text
Firefox killed by signal 6; crash reports in /home/u/.local/share/tontoo-webengine/gecko/profile/crashes
```

`GeckoPage::exit_reason()` exposes the same information, and
`GeckoPage::crash_dir()` the report directory (one subdir per crash with
an `events` file holding the stack and the `MozCrashReason` line).

## BiDi mapping

| Engine operation | BiDi command |
|---|---|
| `load_url` | `browsingContext.navigate { wait: "none" }` |
| `load_html` | `browsingContext.navigate` to a base64 `data:text/html` URL |
| `reload` / bypass cache | `browsingContext.reload { ignoreCache }` |
| `stop` | `window.stop()` (BiDi has no abort command) |
| `go_back` / `go_forward` | `browsingContext.history`, then navigate to the entry |
| `can_go_back` / `can_go_forward` | `browsingContext.history` `currentIndex` |
| `evaluate_javascript` | `script.evaluate { awaitPromise: true }` |
| user scripts | re-injected on `browsingContext.load` |
| mouse / wheel | `input.performActions` pointer and scroll actions |
| keys | `input.performActions` key actions with WebDriver PUA values |
| dialogs | `browsingContext.handleUserPrompt { accept, promptText }` |
| cookies | `storage.getCookies` / `storage.setCookie` / `storage.deleteCookies` |
| clear data | `network.clearData` |
| extensions | `webExtension.install` / `webExtension.listExtensions` |
| tabs | `browsingContext.create` / `close` / `activate` |
| resize | `browsingContext.setViewport` (headless only) |
| shutdown | `browser.close`, then `session.end` |

Subscribed events (`gecko::SUBSCRIBED_EVENTS`):

| BiDi event | Engine event |
|---|---|
| `browsingContext.contextCreated` | `EngineWindow` + `ReadyToShow` |
| `browsingContext.navigationStarted` | `Url`, `Progress(0.1)` |
| `browsingContext.domContentLoaded` | `Progress(0.6)` |
| `browsingContext.load` | `Title`, `Progress(1.0)`, `LoadFinished`, `History` |
| `browsingContext.contextCreated` (child) | `ContextReady` |
| `browsingContext.contextDestroyed` | `ContextClosed` |
| `browsingContext.userPromptOpened` | `ScriptDialog` |
| `browsingContext.downloadWillBegin` | `DownloadStarted` |
| `browsingContext.downloadProgress` | `DownloadProgress` / `DownloadFinished` |
| `script.message` | `ScriptMessage` |
| `log.entryAdded` | `ScriptMessage` (message bridge fallback) |
| `webExtension.installed` | `ExtensionInstalled` |

## BiDi limitations on Firefox 140 ESR

Verified against the provisioned `140.17.0esr` build:

| Command or event | Status |
|---|---|
| `session.new`, `session.subscribe` (one event per call) | works |
| `browsingContext.*` (navigate, history, reload, create, close, activate) | works |
| `script.evaluate` | works |
| `input.performActions` | works |
| `storage.getCookies` / `setCookie` / `deleteCookies` | works |
| `network.clearData` | works |
| `browsingContext.setViewport` | works |
| `browsingContext.userPromptOpened` | works |
| `script.addPreloadScript` | rejected; the message bridge falls back to post-load injection |
| `webExtension.*` (install, listExtensions) | not implemented; use [`write_extension_policy`] |
| `webExtension.installed` event | not implemented |
| `*.downloadWillBegin` / `downloadProgress` events | not implemented under either module |
| multi-event `session.subscribe` | never answers; subscribe one event per call |

The driver therefore logs unsupported events once and keeps going: an
unavailable event never fails a launch. Downloads still work -- Firefox
writes them into `browser.download.dir` -- but the
`DownloadDelegate` round-trip is only raised on builds with the download
events. See [Downloads.md](Downloads.md).

## Extensions

[`ExtensionPolicy`] entries are merged into `<profile>/policies.json`
before launch, which is how Firefox installs add-ons on every build; see
[Extensions.md](Extensions.md). Live installation through
[`WebView::install_extension`] additionally needs the BiDi
`webExtension` module.

## Script Messages

[`gecko::MESSAGE_BOOTSTRAP`] is injected as a BiDi preload script and
again after every load. It defines two page APIs:

```js
window.tontoo.postMessage("channel", { any: "json" });
window.webkit.messageHandlers.channel.postMessage({ any: "json" });
```

Messages reach the host through the `script.message` BiDi event. When a
build does not support `script.addPreloadScript` (older ESR), the
bootstrap falls back to a `console.debug("__tontoo__" + json)` prefix
that `gecko::parse_console_message` decodes from `log.entryAdded`.
Handlers registered with
[`WebKitConfiguration::message_handler`](ScriptMessages.md) are invoked
from `WebView::apply_event` either way.

## Key Values

[`gecko::bidi_key_value`] maps engine key names to the WebDriver PUA
code points: `Enter` U+E007, `Tab` U+E004, `Backspace` U+E003,
`Delete` U+E017, `Escape` U+E00C, `Space` U+E00D, `ArrowLeft` U+E012,
`ArrowUp` U+E013, `ArrowRight` U+E014, `ArrowDown` U+E015, `Home` U+E011,
`End` U+E010, `PageUp` U+E00E, `PageDown` U+E00F, `Insert` U+E016,
`Shift` U+E008, `Control` U+E009, `Alt` U+E00A. Unknown names are typed
as literal text.

## Firefox Auto-Update

[`gecko_provision`](Backend.md) downloads and smoke-tests a Firefox ESR
build when no binary exists, then refreshes it at most once a day in the
background. The running build keeps serving; a new one activates on the
next start.

| Env var | Meaning |
|---|---|
| `TONTOO_FIREFOX_BIN` / `FIREFOX_BIN` | Firefox binary override |
| `TONTOO_FIREFOX_DIR` | Managed install dir override |
| `TONTOO_FIREFOX_CHANNEL` | `esr` (default) or `release` |
| `TONTOO_FIREFOX_AUTOUPDATE=0` | Disable all network provisioning |
| `TONTOO_WEBENGINE_RENDERER` | Helper default: `auto`, `mock` or `gecko` |
| `TONTOO_WEBENGINE_DEBUG=1` | BiDi setup markers and reader heartbeats on stderr |
| `TONTOO_BROWSER_HOME` | Start URL of `tontoo_browser` |
| `TONTOO_BROWSER_EXTENSIONS` | Comma separated AMO add-on ids of `tontoo_browser` |
| `TONTOO_BROWSER_PRIVATE` | `1` starts an ephemeral private profile |
| `TONTOO_BROWSER_HEADLESS` | `1` runs Firefox without a window |
| `TONTOO_BROWSER_SIZE` | Firefox window size, e.g. `1200x800` |
| `TONTOO_BROWSER_KIOSK` | `1` asks for chrome-less fullscreen (off by default) |
| `TONTOO_E2E_TIMEOUT_SECS` | Per-step budget of `tests/gecko_bidi.rs` (default 90) |

Provisioning is Linux-only: Mozilla ships a tarball there, while Windows
and macOS distribute installers and `.dmg` images that do not belong in a
managed directory. On those platforms the system Firefox is used.

## Usage / Example

```rust,no_run
use std::sync::Arc;
use webkit::{ExtensionPolicy, GeckoEngine, GeckoOptions, WebKitConfiguration, WebView};

let options = GeckoOptions {
    kiosk: false,
    headless: false,
    private: false,
    download_dir: std::env::temp_dir().join("tontoo-downloads"),
    window_size: Some((1200, 800)),
    start_url: Some("https://example.com".to_string()),
    provision: true,
    extensions: vec![ExtensionPolicy::AddonId("uBlock0@raymondhill.net".into())],
};
let engine = GeckoEngine::launch(options).expect("Firefox failed to start");

let view = WebView::with_engine(
    WebKitConfiguration::new().start_url("https://example.com"),
    Arc::new(engine),
).expect("web view");

// Pump once per UI frame, then read state.
view.pump_events();
let _title = view.title();
let _url = view.url();
let _progress = view.estimated_progress();
```

Without the helper, in one call:

```bash
cargo run --example tontoo_browser
```

## Cross References

- [Extensions.md](Extensions.md) -- WebExtensions over the same session
- [Backend.md](Backend.md) -- engine trait, helper protocol, cargo features
- [WebView.md](WebView.md) -- the view API used above
- [ScriptMessages.md](ScriptMessages.md) -- page-to-host message bridge
- [Cookies.md](Cookies.md) -- `storage.*` cookie mapping
