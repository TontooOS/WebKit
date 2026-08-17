//! Navigation state and policy decisions, the equivalent of
//! `WKNavigationDelegate` in Apple WebKit.

use webkit6 as wk;

/// A phase in the page load lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationEvent {
    /// The provisional navigation has started.
    Started,
    /// The server redirected the request.
    Redirected,
    /// The navigation was committed; content is arriving.
    Committed,
    /// The page finished loading.
    Finished,
    /// The page failed to load.
    Failed,
}

/// The kind of navigation a policy decision applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    /// A main-frame navigation.
    Navigation,
    /// A new window/tab requested via `window.open` or a target link.
    NewWindow,
    /// A sub-resource response.
    Response,
}

/// Information about a navigation decision.
#[derive(Debug, Clone)]
pub struct NavigationAction {
    /// The target URL, when available.
    pub url: Option<String>,
    /// The policy action this decision belongs to.
    pub action: PolicyAction,
}

/// Trait for reacting to navigation lifecycle and policy decisions.
///
/// Every method has a default implementation, so only the methods you care
/// about need to be overridden.
pub trait WebNavigationDelegate {
    /// The provisional navigation started.
    fn navigation_started(&mut self, _url: Option<&str>) {}

    /// The server redirected the provisional navigation.
    fn navigation_redirected(&mut self, _url: Option<&str>) {}

    /// The navigation was committed and content starts arriving.
    fn navigation_committed(&mut self, _url: Option<&str>) {}

    /// The page finished loading.
    fn navigation_finished(&mut self, _url: Option<&str>) {}

    /// The page failed to load.
    fn navigation_failed(&mut self, _url: Option<&str>, _error: &str) {}

    /// Decide whether a navigation is allowed to proceed.
    ///
    /// Return `true` to allow, `false` to cancel. Defaults to `true`.
    fn decide_policy(&mut self, _url: Option<&str>, _action: PolicyAction) -> bool {
        true
    }
}

/// Default delegate used when the caller does not provide one.
#[derive(Debug, Default)]
pub struct DefaultWebNavigationDelegate;

impl WebNavigationDelegate for DefaultWebNavigationDelegate {}

pub(crate) fn load_event_to_navigation(event: &wk::LoadEvent) -> NavigationEvent {
    match event {
        wk::LoadEvent::Started => NavigationEvent::Started,
        wk::LoadEvent::Redirected => NavigationEvent::Redirected,
        wk::LoadEvent::Committed => NavigationEvent::Committed,
        wk::LoadEvent::Finished => NavigationEvent::Finished,
        _ => NavigationEvent::Failed,
    }
}