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

/// The size icons are taken at before shrinking: comfortably above any row
/// size, so the filter has detail to work with. 256 looks the same and makes
/// the shell hold larger images.
pub(crate) const SOURCE_SIZE: u32 = 128;

/// Shrinks `icon` to `size` square with a Lanczos-3 filter, one axis at a
/// time, in premultiplied space so edges do not darken. Sharper than
/// averaging blocks, which leaves visible steps at non-integer ratios.
pub(crate) fn downscale(icon: &Icon, size: u32) -> Icon {
    if icon.width <= size && icon.height <= size {
        return icon.clone();
    }
    let channels: Vec<[f32; 4]> = icon
        .pixels
        .iter()
        .map(|&p| std::array::from_fn(|c| ((p >> (c * 8)) & 0xff) as f32))
        .collect();
    let (w, h) = (icon.width as usize, icon.height as usize);
    let n = size as usize;
    // Rows: w x h -> n x h.
    let across = taps(w, n);
    let mut rows = vec![[0f32; 4]; n * h];
    for y in 0..h {
        for (x, taps) in across.iter().enumerate() {
            rows[y * n + x] = weigh(taps, |j| channels[y * w + j]);
        }
    }
    // Columns: n x h -> n x n.
    let down = taps(h, n);
    let mut pixels = Vec::with_capacity(n * n);
    for taps in &down {
        for x in 0..n {
            let px = weigh(taps, |j| rows[j * n + x]);
            pixels.push(px.iter().enumerate().fold(0u32, |acc, (c, v)| {
                acc | ((v.round().clamp(0.0, 255.0) as u32) << (c * 8))
            }));
        }
    }
    Icon {
        width: size,
        height: size,
        pixels,
    }
}

fn weigh(taps: &[(usize, f32)], sample: impl Fn(usize) -> [f32; 4]) -> [f32; 4] {
    let mut acc = [0f32; 4];
    for &(j, w) in taps {
        let px = sample(j);
        for c in 0..4 {
            acc[c] += px[c] * w;
        }
    }
    acc
}

/// Lanczos-3 source taps and normalized weights for each of `out_len`
/// outputs along an axis of `len` samples.
fn taps(len: usize, out_len: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = len as f32 / out_len as f32;
    let support = 3.0 * scale;
    let lanczos = |x: f32| {
        if x == 0.0 {
            1.0
        } else if x.abs() >= 3.0 {
            0.0
        } else {
            let px = std::f32::consts::PI * x;
            3.0 * px.sin() * (px / 3.0).sin() / (px * px)
        }
    };
    (0..out_len)
        .map(|i| {
            let center = (i as f32 + 0.5) * scale;
            let first = (center - support).floor().max(0.0) as usize;
            let last = ((center + support).ceil() as usize).min(len);
            let mut taps: Vec<(usize, f32)> = (first..last)
                .map(|j| (j, lanczos((j as f32 + 0.5 - center) / scale)))
                .collect();
            let total: f32 = taps.iter().map(|(_, w)| w).sum();
            for (_, w) in &mut taps {
                *w /= total;
            }
            taps
        })
        .collect()
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
                    // crops) icons poorly. A larger image filtered down is crisp.
                    #[cfg(windows)]
                    let icon = crate::sys::app_icon(&id, SOURCE_SIZE.max(size))
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
    fn downscale_keeps_flat_color_and_alpha() {
        let icon = Icon {
            width: 64,
            height: 64,
            pixels: vec![0x8040_2010; 64 * 64],
        };
        let small = downscale(&icon, 30);
        assert_eq!((small.width, small.height), (30, 30));
        assert!(small.pixels.iter().all(|&p| p == 0x8040_2010));
    }

    #[test]
    fn downscale_keeps_a_hard_edge_where_it_was() {
        // Left half opaque white, right half transparent.
        let pixels = (0..64 * 64)
            .map(|i| if i % 64 < 32 { 0xffff_ffff } else { 0 })
            .collect();
        let icon = Icon {
            width: 64,
            height: 64,
            pixels,
        };
        let small = downscale(&icon, 16);
        let row = &small.pixels[8 * 16..9 * 16];
        assert_eq!(row[0] >> 24, 255);
        assert_eq!(row[15] >> 24, 0);
        // Rows are not mixed into columns or the other way round.
        assert!(small.pixels.chunks(16).all(|r| r == row));
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
