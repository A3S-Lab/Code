//! Browser runtime lifecycle API for headless web search.
//!
//! This is a stable A3S Code entry point over the browser management surface
//! provided by a3s-search. Status checks are read-only; install, update, and
//! repair are explicit host operations and are never triggered by inspection.

pub use a3s_search::browser_management::{
    browser_status, browser_statuses, install_browser, repair_browser, update_browser,
    BrowserInstallSource, BrowserRuntimeStatus, ManagedBrowser,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_statuses_cover_all_managed_runtimes() {
        let statuses = browser_statuses();
        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].browser, ManagedBrowser::Chrome);
        assert_eq!(statuses[1].browser, ManagedBrowser::Lightpanda);
    }
}
