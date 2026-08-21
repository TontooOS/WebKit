# DialogsAndPermissions

JavaScript dialog and permission-request handling for TontooWebKit. Both
are hooks on the `WebViewDelegate` and fail closed: dialogs show no UI by
default and permissions are denied unless the app explicitly grants them.

## Rules

- Both hooks run on the main thread.
- An unhandled dialog (`script_dialog` returns `false`) gets the engine
  default answer: no UI, `confirm` = `false`, `prompt` = `null`.
- The default `permission_request` implementation returns
  `PermissionDecision::Deny`. Nothing is ever granted silently.

## JavaScript Dialogs

### `WebViewDelegate::script_dialog`

```rust
fn script_dialog(&mut self, dialog: &ScriptDialogRef) -> bool
```

Called for `window.alert`, `window.confirm`, `window.prompt` and unload
confirmations. Return `true` when the app handled the dialog; it must then
answer through the methods below. Return `false` to leave it unhandled.

### `ScriptDialogRef`

| Method | Behavior |
|---|---|
| `kind(&self) -> ScriptDialogKind` | Which dialog was requested |
| `message(&self) -> String` | The dialog message text |
| `prompt_default_text(&self) -> Option<String>` | Default text of a `prompt` |
| `set_prompt_text(&self, text: &str)` | Answer a `prompt` with text |
| `set_confirmed(&self, confirmed: bool)` | Answer a `confirm` / before-unload dialog |

### `ScriptDialogKind`

| Variant | Trigger |
|---|---|
| `Alert` | `window.alert(message)` |
| `Confirm` | `window.confirm(message)` |
| `Prompt` | `window.prompt(message, default)` |
| `BeforeUnloadConfirm` | Unload confirmation |

## Permission Requests

### `WebViewDelegate::permission_request`

```rust
fn permission_request(&mut self, kind: PermissionKind) -> PermissionDecision
```

Called when a page requests a permission. Return
`PermissionDecision::Grant` or `PermissionDecision::Deny`.

### `PermissionKind`

| Variant | Request |
|---|---|
| `Camera` | Camera access |
| `Microphone` | Microphone access |
| `CameraAndMicrophone` | Camera and microphone at once |
| `Geolocation` | Geolocation |
| `Notifications` | Notifications |
| `ClipboardRead` | Reading the system clipboard |
| `DeviceInfo` | Listing media devices |
| `PointerLock` | Locking the mouse pointer |
| `MediaKeySystem` | EME media key system (DRM) access |
| `WebsiteDataAccess` | Cross-site website data access |
| `Other` | Any other or unknown permission |

## Usage / Example

```rust,no_run
use webkit::{
    PermissionDecision, PermissionKind, ScriptDialogKind, ScriptDialogRef,
    WebViewDelegate,
};

struct SecureDelegate;

impl WebViewDelegate for SecureDelegate {
    fn script_dialog(&mut self, dialog: &ScriptDialogRef) -> bool {
        match dialog.kind() {
            ScriptDialogKind::Confirm => {
                dialog.set_confirmed(true);
                true
            }
            _ => false,
        }
    }

    fn permission_request(&mut self, kind: PermissionKind) -> PermissionDecision {
        match kind {
            PermissionKind::Notifications => PermissionDecision::Grant,
            _ => PermissionDecision::Deny,
        }
    }
}
```

## Cross References

- [WebView.md](WebView.md) -- installing delegates
- [Navigation.md](Navigation.md) -- navigation delegate
- [Downloads.md](Downloads.md) -- download delegate
