use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::graphics::{Area, image};

#[derive(Clone, PartialEq)]
pub struct Image {
    source: Source,
    pub(crate) fit: Fit,

    // the size it is shrunk to cover when decoded, none keeps every pixel
    pub(crate) thumbnail: Option<(u32, u32)>,

    // how far the decoded copy is blurred, in its own pixels
    pub(crate) blur: u32,
}

#[derive(Clone, PartialEq)]
enum Source {
    File(PathBuf),
    Owned(Arc<image::Bitmap>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    // fills the rectangle exactly, bending the image's shape to match
    Stretch,

    // fills the whole rectangle, cutting off what spills past the edges
    Cover,

    // shows the whole image, leaving the rest of the rectangle uncovered
    Contain,
}

impl Image {
    /// Own validated, straight-alpha RGBA pixels without a file-cache entry.
    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        Some(Self::owned(image::Bitmap::from_rgba(
            width, height, pixels,
        )?))
    }

    /// Decode a file synchronously. Call from background work, not drawing.
    pub fn read_owned(path: impl AsRef<Path>) -> Option<Self> {
        Some(Self::owned(image::read(path.as_ref())?))
    }

    fn owned(bitmap: image::Bitmap) -> Self {
        Self {
            source: Source::Owned(Arc::new(bitmap)),
            fit: Fit::Contain,
            thumbnail: None,
            blur: 0,
        }
    }

    pub(crate) fn bitmap(&self) -> Option<Arc<image::Bitmap>> {
        match &self.source {
            Source::File(path) => image::load(path, self.thumbnail, self.blur),
            Source::Owned(bitmap) => Some(Arc::clone(bitmap)),
        }
    }

    pub fn cover(path: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::File(path.into()),
            fit: Fit::Cover,
            thumbnail: None,
            blur: 0,
        }
    }

    pub fn contain(path: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::File(path.into()),
            fit: Fit::Contain,
            thumbnail: None,
            blur: 0,
        }
    }

    pub fn stretch(path: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::File(path.into()),
            fit: Fit::Stretch,
            thumbnail: None,
            blur: 0,
        }
    }

    /*
     * keeps a small copy big enough to cover width by height, for pictures
     * shown far smaller than their file, like a list of wallpapers
     */
    pub fn thumbnail(mut self, width: u32, height: u32) -> Self {
        self.thumbnail = Some((width, height));
        if let Source::Owned(bitmap) = &self.source {
            self.source = Source::Owned(Arc::new(image::prepare(
                (**bitmap).clone(),
                self.thumbnail,
                0,
            )));
        }

        self
    }

    /*
     * blurs the decoded copy once, radius counted in its own pixels; with a
     * small thumbnail stretched far bigger, like a blurred backdrop, it
     * looks smooth instead of blocky and costs nothing per frame
     */
    pub fn blurred(mut self, radius: u32) -> Self {
        self.blur = radius;
        if let Source::Owned(bitmap) = &self.source {
            self.source = Source::Owned(Arc::new(image::prepare((**bitmap).clone(), None, radius)));
        }

        self
    }

    /*
     * starts decoding the file if nothing asked for it yet, and says whether
     * it is ready to draw; a file that can't be read never becomes ready
     */
    pub fn loaded(path: impl AsRef<Path>) -> bool {
        let path = path.as_ref();

        image::load(path, None, 0).is_some()
    }
}

impl Fit {
    pub fn place(self, area: Area, image_width: f32, image_height: f32) -> Area {
        let horizontal_scale = area.width / image_width;
        let vertical_scale = area.height / image_height;

        let scale = match self {
            Fit::Stretch => return area,
            Fit::Cover => f32::max(horizontal_scale, vertical_scale),
            Fit::Contain => f32::min(horizontal_scale, vertical_scale),
        };

        let width = image_width * scale;
        let height = image_height * scale;

        let x = area.x + (area.width - width) / 2.0;
        let y = area.y + (area.height - height) / 2.0;

        Area::new(x, y, width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn owned_rgba_rejects_invalid_buffers() {
        assert!(Image::from_rgba(0, 1, vec![]).is_none());
        assert!(Image::from_rgba(u32::MAX, u32::MAX, vec![]).is_none());
        assert!(Image::from_rgba(1, 1, vec![1, 2, 3]).is_none());
        let image = Image::from_rgba(1, 1, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(&*image.bitmap().unwrap().pixels, &[1, 2, 3, 4]);
        let same = Image::from_rgba(1, 1, vec![1, 2, 3, 4]).unwrap();
        assert!(same == image);
        assert!(Image::from_rgba(1, 1, vec![1, 2, 3, 5]).unwrap() != image);
    }

    #[test]
    fn owned_images_release_replaced_pixels() {
        let image = Image::from_rgba(1, 1, vec![1, 2, 3, 4]).unwrap();
        let bitmap = image.bitmap().unwrap();
        let weak = Arc::downgrade(&bitmap);
        let clone = image.clone();
        drop(image);
        drop(bitmap);
        assert!(weak.upgrade().is_some());
        drop(clone);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn owned_file_read_bypasses_path_cache() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let folder = std::env::temp_dir().join(format!(
            "amane-owned-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&folder).unwrap();
        let png = folder.join("changing.png");
        let write_png = |pixel: [u8; 4]| {
            let mut encoder = png::Encoder::new(fs::File::create(&png).unwrap(), 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixel)
                .unwrap();
        };
        write_png([1, 2, 3, 4]);
        let first = Image::read_owned(&png).unwrap();
        write_png([5, 6, 7, 8]);
        let second = Image::read_owned(&png).unwrap();
        assert_eq!(&*first.bitmap().unwrap().pixels, &[1, 2, 3, 4]);
        assert_eq!(&*second.bitmap().unwrap().pixels, &[5, 6, 7, 8]);
        assert!(first != second);
        let svg = folder.join("icon.svg");
        fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red"/></svg>"#).unwrap();
        let bitmap = Image::read_owned(&svg).unwrap().bitmap().unwrap();
        assert_eq!((bitmap.width(), bitmap.height()), (256, 256));
        assert_eq!(&bitmap.pixels[..4], &[255, 0, 0, 255]);
        assert!(Image::read_owned(folder.join("missing")).is_none());
        fs::remove_dir_all(folder).unwrap();
    }
}
