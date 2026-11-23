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

/// Shrinks `icon` to `size` square by averaging each target pixel's area of
/// the source, in premultiplied space so edges do not darken.
pub(crate) fn downscale(icon: &Icon, size: u32) -> Icon {
    if icon.width <= size && icon.height <= size {
        return icon.clone();
    }
    let (sw, sh) = (icon.width as f32, icon.height as f32);
    let mut pixels = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        let (y0, y1) = (
            y as f32 * sh / size as f32,
            (y + 1) as f32 * sh / size as f32,
        );
        for x in 0..size {
            let (x0, x1) = (
                x as f32 * sw / size as f32,
                (x + 1) as f32 * sw / size as f32,
            );
            let mut sum = [0f32; 4];
            let mut total = 0f32;
            for sy in y0.floor() as u32..(y1.ceil() as u32).min(icon.height) {
                let wy = (sy as f32 + 1.0).min(y1) - (sy as f32).max(y0);
                for sx in x0.floor() as u32..(x1.ceil() as u32).min(icon.width) {
                    let w = wy * ((sx as f32 + 1.0).min(x1) - (sx as f32).max(x0));
                    let p = icon.pixels[(sy * icon.width + sx) as usize];
                    for (c, slot) in sum.iter_mut().enumerate() {
                        *slot += ((p >> (c * 8)) & 0xff) as f32 * w;
                    }
                    total += w;
                }
            }
            let pixel = sum.iter().enumerate().fold(0u32, |acc, (c, v)| {
                acc | (((v / total).round() as u32).min(255) << (c * 8))
            });
            pixels.push(pixel);
        }
    }
    Icon {
        width: size,
        height: size,
        pixels,
    }
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
                    // Asked for at small sizes, the shell scales (and sometimes
                    // crops) icons poorly. A larger one averaged down is crisp.
                    #[cfg(windows)]
                    let icon = crate::sys::app_icon(&id, (size * 2).clamp(64, 256))
                        .map(|icon| downscale(&icon, size));
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

#[cfg(test)]
mod resample_tests {
    use super::*;

    #[test]
    fn downscale_averages_areas() {
        // A 4x4 icon: left half opaque white, right half transparent.
        let pixels = (0..16)
            .map(|i| if i % 4 < 2 { 0xffff_ffff } else { 0 })
            .collect();
        let icon = Icon {
            width: 4,
            height: 4,
            pixels,
        };
        let half = downscale(&icon, 2);
        assert_eq!(half.pixels, [0xffff_ffff, 0, 0xffff_ffff, 0]);
        let one = downscale(&icon, 1);
        assert_eq!(one.pixels, [0x8080_8080]);
    }

    #[test]
    fn small_icons_are_left_alone() {
        let icon = Icon {
            width: 16,
            height: 16,
            pixels: vec![1; 256],
        };
        assert_eq!(downscale(&icon, 20), icon);
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
