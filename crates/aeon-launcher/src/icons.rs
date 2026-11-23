//! App icons for the launcher's list, extracted by the shell on a background
//! thread: a Store app's icon can take tens of milliseconds to produce.

use std::sync::mpsc::{self, Sender};

/// A square icon as premultiplied `0xAARRGGBB` pixels, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// Handle to the icon thread. Dropping it stops the thread.
pub struct Icons {
    requests: Sender<(String, u32)>,
}

impl Icons {
    /// `on_loaded(app id, icon)` is called from the icon thread for every
    /// request, with `None` when the shell has no icon for the app.
    pub fn spawn(on_loaded: impl Fn(String, Option<Icon>) + Send + 'static) -> Self {
        let (requests, queue) = mpsc::channel::<(String, u32)>();
        std::thread::Builder::new()
            .name("icons".into())
            .spawn(move || {
                #[cfg(windows)]
                crate::sys::init_com();
                for (id, size) in queue {
                    #[cfg(windows)]
                    let icon = crate::sys::app_icon(&id, size);
                    #[cfg(not(windows))]
                    let icon = {
                        let _ = size;
                        None
                    };
                    on_loaded(id, icon);
                }
            })
            .expect("spawning the icon thread");
        Self { requests }
    }

    /// Asks for an app's icon at `size` pixels. The caller keeps track of what
    /// it already asked for.
    pub fn request(&self, id: &str, size: u32) {
        let _ = self.requests.send((id.to_owned(), size));
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn extracts_an_installed_apps_icon() {
        // Every Windows install lists a few apps; the first one must have an
        // icon the shell can draw.
        let apps = crate::apps::load();
        let Some(app) = apps.first() else {
            panic!("no apps to take an icon from");
        };
        let (tx, rx) = mpsc::channel();
        let icons = Icons::spawn(move |id, icon| {
            let _ = tx.send((id, icon));
        });
        icons.request(&app.id, 32);
        let (id, icon) = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("an answer");
        assert_eq!(id, app.id);
        let icon = icon.unwrap_or_else(|| panic!("no icon for {}", app.name));
        assert!(icon.width >= 16 && icon.height >= 16);
        assert_eq!(icon.pixels.len(), (icon.width * icon.height) as usize);
        assert!(
            icon.pixels.iter().any(|p| p >> 24 != 0),
            "fully transparent"
        );
    }
}
