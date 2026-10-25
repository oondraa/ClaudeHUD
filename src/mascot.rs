//! The pixel-art Claude mascots from `assety/`, rasterized at runtime.

use resvg::{tiny_skia, usvg};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pose {
    Idle,
    Working,
    Done,
}

impl Pose {

    fn svg(self) -> &'static str {
        match self {
            Pose::Idle => include_str!("../assety/claude-idle.svg"),
            Pose::Working => include_str!("../assety/claude-working.svg"),
            Pose::Done => include_str!("../assety/claude-done.svg"),
        }
    }
}

/// A rasterized mascot: straight (non-premultiplied) RGBA.
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Renders a pose at `scale` pixels per SVG unit.
pub fn render(pose: Pose, scale: f32) -> Bitmap {
    let tree = usvg::Tree::from_str(pose.svg(), &usvg::Options::default()).expect("bundled SVG is valid");
    let size = tree.size();
    let (w, h) = ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32);
    let mut pixmap = tiny_skia::Pixmap::new(w, h).expect("non-empty mascot");
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    let rgba = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    Bitmap { width: w, height: h, rgba: rgba.collect() }
}
