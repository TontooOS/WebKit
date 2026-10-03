# Extensions

WebExtensions on TontooOS, **with the optional `gecko` engine only**. Because
the Gecko backend drives a real Firefox with a real profile, every Firefox
add-on works: uBlock Origin, Bitwarden, Vimium, Dark Reader, Return YouTube
Dislike and the rest of AMO. Nothing in TontooWebKit re-implements the
extension system -- it only asks Firefox to install them.

The default WPE engine has no add-on manager, so `install_extension` and
`list_extensions` report an error there. See [WPE.md](WPE.md).

## Two Paths

| Path | API | Needs |
|---|---|---|
| Profile policy | [`GeckoOptions::extensions`], [`write_extension_policy`] | Nothing -- works on every build |
| Live install | [`WebView::install_extension`] | The BiDi `webExtension` module |

> **Note:** Firefox 140 ESR (the build TontooOS provisions) ships **no**
> `webExtension` module: `webExtension.install` and
> `webExtension.listExtensions` never answer, and neither does the
> `webExtension.installed` event. The profile policy path is therefore
> the one that always works; the live path is used when a build has the
> module. `tests/gecko_bidi.rs` accepts either.

## Profile Policy (Recommended)

`write_extension_policy` merges entries into `<profile>/policies.json`
(`3rdparty.Extensions` with `installation_mode: normal_installed`).
Firefox downloads each add-on on startup and keeps it updated itself.

```rust
use webkit::{ExtensionPolicy, GeckoEngine, GeckoOptions, WebView};

let engine = GeckoEngine::launch(GeckoOptions {
    extensions: vec![
        ExtensionPolicy::AddonId("uBlock0@raymondhill.net".into()),
        ExtensionPolicy::AddonId("jid1-MnnxcxisBPnSXQ@jetpack".into()),
    ],
    start_url: Some("https://example.com".into()),
    ..GeckoOptions::default()
})
.expect("Firefox");

let view = WebView::with_engine(
    WebKitConfiguration::new(),
    std::sync::Arc::new(engine),
)
.expect("web view");
```

| Variant | Result |
|---|---|
| `AddonId(id)` | Firefox fetches that AMO add-on and auto-updates it |
| `Url { url }` | Firefox fetches that `.xpi` URL at startup |

Writing the policy twice merges instead of replacing, so a launcher can
add ids over time without dropping what a previous run wrote. Signed AMO
add-ons work on every Firefox build; see
[Signing](#signing) below.

`examples/tontoo_browser.rs` installs the curated TontooOS set by
default and reads `TONTOO_BROWSER_EXTENSIONS` (comma separated ids) to
override it.

## Live Install

```rust
fn install_extension(&self, path: &std::path::Path) -> Result<String, WebKitError>
```

`path` is either a directory containing `manifest.json` or an `.xpi`
archive. The call blocks up to `JS_TIMEOUT`] while pumping engine events
and returns the Firefox extension id (`"uBlock0@raymondhill.net"`).

```rust
let id = view.install_extension(std::path::Path::new(
    "/usr/share/tontoo/extensions/ublock-origin",
))?;
let id = view.install_extension(std::path::Path::new(
    "/tmp/download-ublock-origin.xpi",
))?;
```

| Result | Meaning |
|---|---|
| `Ok(id)` | Installed and enabled |
| `Err(WebKitError::Engine(..))` | Firefox refused it (invalid manifest, missing file, signature) or the build has no `webExtension` module |
| `Err(WebKitError::Engine("timed out .."))` | No answer within `JS_TIMEOUT` (60 s) |

Behind the scenes: `webExtension.install` with
`extensionData { type: "path", path: <absolute path> }`.

## List

```rust
fn list_extensions(&self) -> Result<Vec<String>, WebKitError>
```

Maps `webExtension.listExtensions` and returns the installed extension
ids in Firefox's order. An empty vec means "nothing installed" **or** the
engine cannot answer the module (`MockEngine` reports the same), so pair
it with [`WebView::engine_window`] to tell a browser from a mock.

## Unsolicited Installs

When the user installs an add-on from Firefox' own `about:addons` page,
the BiDi event `webExtension.installed` would arrive unprompted and
surface through the delegate:

```rust
use webkit::{DefaultWebViewDelegate, WebViewDelegate};

struct Watcher;
impl WebViewDelegate for Watcher {
    fn extension_installed(&mut self, id: &str) {
        println!("extension ready: {id}");
    }
}
```

Firefox 140 ESR has no such event, so this callback only fires on builds
with the `webExtension` module. Installations the host requested answer
their own call and are never reported twice.

## Signing

| Build | Sideloaded unsigned `.xpi` | AMO-signed add-on |
|---|---|---|
| ESR (default), DevEdition | yes, `xpinstall.signatures.required = false` | yes |
| Official release | **no** | yes |
| Nightly | yes (temporary install only) | yes |

TontooOS provisions and defaults to **ESR**, so `ExtensionPolicy` ids
work everywhere and local packages can be installed live through
`install_extension`.

## Profiles

Extensions live in the Firefox profile, not in a TontooWebKit database:

| Session | Profile | Extensions |
|---|---|---|
| Normal | `~/.local/share/tontoo-webengine/gecko/profile` | Persistent, installed at startup, updated from AMO |
| Private (`GeckoOptions::private`) | temp dir, deleted on exit | Policy entries still apply |
| Headless test | temp dir | None |

Prefs that keep the add-on system open for a daily-driver OS:

| Pref | Value | Effect |
|---|---|---|
| `extensions.autoDisableScopes` | `0` | Nothing is auto-disabled |
| `extensions.enabledScopes` | `15` | Profile, policy, sideloaded and temporary scopes |
| `extensions.update.enabled` | `true` | Firefox updates add-ons from AMO |
| `extensions.update.autoUpdateDefault` | `true` | Update without asking |
| `xpinstall.signatures.required` | `false` | Unsigned local packages (ESR/DevEdition) |

## Permissions

Extension permissions are Firefox' own: the add-on asks through its
`permissions` or `optional_permissions` in `manifest.json`, and Firefox
shows its prompt. TontooWebKit never pre-grants anything:

- `WebViewDelegate::permission_request` defaults to
  `PermissionDecision::Deny` for page-level permissions (camera,
  geolocation, notifications).
- The Gecko renderer leaves `EngineCommand::PermissionAnswer` unanswered,
  so nothing is granted silently on the engine side either.

## Where extensions come from on TontooOS

| Source | How |
|---|---|
| AMO / addons.mozilla.org | `ExtensionPolicy::AddonId` (default) or Firefox' own updater |
| `.xpi` URL | `ExtensionPolicy::Url { url }` |
| Local `.xpi` / folder | `WebView::install_extension` (needs the BiDi module) |
| Interactive | Firefox `about:addons` |
| Enterprise policy | Hand-written `<profile>/policies.json` before launch |

## Usage / Example

```rust,no_run
use webkit::{ExtensionPolicy, GeckoEngine, GeckoOptions, WebKitConfiguration, WebView};

let engine = GeckoEngine::launch(GeckoOptions {
    // A fresh private session still installs the policy entries.
    private: true,
    extensions: vec![ExtensionPolicy::AddonId("uBlock0@raymondhill.net".into())],
    start_url: Some("https://addons.mozilla.org/".to_string()),
    ..GeckoOptions::default()
})
.expect("Firefox");

let view = WebView::with_engine(
    WebKitConfiguration::new().start_url("https://addons.mozilla.org/"),
    std::sync::Arc::new(engine),
)
.expect("web view");
```

## Cross References

- [Gecko.md](Gecko.md) -- the Firefox session, profile and prefs
- [DialogsAndPermissions.md](DialogsAndPermissions.md) -- page permission prompts
- [DataStore.md](DataStore.md) -- clearing cookies, cache and site data
