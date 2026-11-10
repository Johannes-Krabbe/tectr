//! Images pasted into prompts. Each is a file named by the SHA-256 of its bytes, so adding the
//! same image twice keeps one copy and an ID means the same image on every device. Events and
//! drafts only refer to them, which keeps transcripts small to store and to send.
//!
//! A big image also gets a small copy beside it, `<id>.thumb`, for the thumbnails and previews
//! clients show, so a client on a slow link doesn't fetch the whole image to draw a tile.
//! Files nothing refers to any more are removed by [`Images::collect`].

use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use protocol::{IMAGE_TYPES, ImageId, MAX_IMAGE_BYTES, THUMBNAIL_EDGE};
use sha2::{Digest, Sha256};

/// Anything bigger isn't decoded. Real screenshots and photos are well inside it.
const MAX_EDGE: u32 = 16_384;
/// How long a file nothing refers to is kept. A client adds an image a moment before a draft
/// or a prompt names it, and the new topic form keeps its images to itself until it starts.
pub const UNREFERENCED_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

const THUMB: &str = "thumb";

#[derive(Clone)]
pub struct Images {
    dir: PathBuf,
}

/// An image's bytes in base64, and their type.
#[derive(Debug, PartialEq, Eq)]
pub struct Encoded {
    pub data: String,
    pub mime: String,
}

impl Images {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("images"),
        }
    }

    /// Keeps the image and returns its ID. The error says what's wrong in words for a person.
    /// It decodes the image, so this is work for a blocking thread.
    pub fn add(&self, mime: &str, data: &str) -> Result<ImageId> {
        if !IMAGE_TYPES.contains(&mime) {
            bail!("{mime} isn't an image type agents take");
        }
        let bytes = STANDARD
            .decode(data)
            .context("the image isn't valid base64")?;
        if bytes.is_empty() {
            bail!("the image is empty");
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            bail!(
                "the image is {} MB, and the most is {} MB",
                bytes.len().div_ceil(1024 * 1024),
                MAX_IMAGE_BYTES / 1024 / 1024
            );
        }
        // An agent sent something it can't read fails the whole turn, so it's caught here.
        let (format, image) = decode(&bytes)?;
        if format.to_mime_type() != mime {
            bail!("the image is a {}, not a {mime}", format.to_mime_type());
        }
        let id = ImageId(hex(&Sha256::digest(&bytes)));
        let path = self.dir.join(&id.0);
        if path.exists() {
            // Adding it again starts its grace period over.
            touch(&path)?;
        } else {
            std::fs::create_dir_all(&self.dir)
                .with_context(|| format!("failed to create {}", self.dir.display()))?;
            self.write(&path, &bytes)?;
        }
        let thumb = thumb_path(&path);
        if !thumb.exists()
            && let Some(small) = thumbnail(&image)?
        {
            self.write(&thumb, &small)?;
        }
        Ok(id)
    }

    /// The image, or its small copy, which for an image already small is the image itself.
    /// Making a missing small copy decodes the image, so this is work for a blocking thread.
    pub fn read(&self, id: &ImageId, small: bool) -> Result<Encoded> {
        let path = self
            .path(id)
            .with_context(|| format!("there's no image {id}"))?;
        let bytes = std::fs::read(&path).with_context(|| format!("there's no image {id}"))?;
        if !small {
            return encoded(bytes);
        }
        let thumb = thumb_path(&path);
        if let Ok(small) = std::fs::read(&thumb) {
            return encoded(small);
        }
        // One kept before thumbnails existed.
        let (_, image) = decode(&bytes)?;
        match thumbnail(&image)? {
            Some(small) => {
                self.write(&thumb, &small)?;
                encoded(small)
            }
            None => encoded(bytes),
        }
    }

    pub fn has(&self, id: &ImageId) -> bool {
        self.path(id).is_some_and(|path| path.is_file())
    }

    /// Removes every file that isn't an image in `keep` or its small copy and hasn't changed
    /// since `before`, which also clears what a crash left half written. Returns how many it
    /// removed.
    pub fn collect(&self, keep: &HashSet<ImageId>, before: SystemTime) -> Result<usize> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error).context("failed to list images"),
        };
        let mut removed = 0;
        for entry in entries {
            let entry = entry.context("failed to list images")?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let (id, rest) = match name.split_once('.') {
                Some((id, rest)) => (id, Some(rest)),
                None => (&*name, None),
            };
            let whole = rest.is_none() || rest == Some(THUMB);
            if whole && keep.contains(&ImageId::from(id)) {
                continue;
            }
            let recent = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .map_or(true, |modified| modified > before);
            if recent {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(error) => tracing::warn!(file = %name, "failed to remove an image: {error}"),
            }
        }
        Ok(removed)
    }

    /// The ID comes from a client and becomes a file name, so only a hash is one.
    fn path(&self, id: &ImageId) -> Option<PathBuf> {
        (id.0.len() == 64 && id.0.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| self.dir.join(&id.0))
    }

    /// Written aside and moved into place, so a crash never leaves half a file under the name
    /// of a whole one.
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        let partial = self.dir.join(format!(
            "{}.{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            crate::new_id()
        ));
        std::fs::write(&partial, bytes)
            .with_context(|| format!("failed to write {}", partial.display()))?;
        std::fs::rename(&partial, path)
            .with_context(|| format!("failed to write {}", path.display()))
    }
}

fn thumb_path(path: &Path) -> PathBuf {
    path.with_extension(THUMB)
}

fn decode(bytes: &[u8]) -> Result<(ImageFormat, DynamicImage)> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("agents couldn't read that image")?;
    let format = reader.format().context("agents couldn't read that image")?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    reader.limits(limits);
    let image = reader.decode().context("agents couldn't read that image")?;
    Ok((format, image))
}

/// A small copy, or `None` when the image is small already. One with transparency stays a
/// PNG, because JPEG would turn it black.
fn thumbnail(image: &DynamicImage) -> Result<Option<Vec<u8>>> {
    if image.width().max(image.height()) <= THUMBNAIL_EDGE {
        return Ok(None);
    }
    let small = image.thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE);
    let mut out = Cursor::new(Vec::new());
    if small.color().has_alpha() {
        small.write_to(&mut out, ImageFormat::Png)?;
    } else {
        JpegEncoder::new_with_quality(&mut out, 80).encode_image(&small.to_rgb8())?;
    }
    Ok(Some(out.into_inner()))
}

fn encoded(bytes: Vec<u8>) -> Result<Encoded> {
    let format = image::guess_format(&bytes).context("the stored image is damaged")?;
    Ok(Encoded {
        mime: format.to_mime_type().to_owned(),
        data: STANDARD.encode(bytes),
    })
}

fn touch(path: &Path) -> Result<()> {
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(SystemTime::now()))
        .with_context(|| format!("failed to update {}", path.display()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    use super::*;

    fn png(image: DynamicImage) -> String {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).unwrap();
        STANDARD.encode(out.into_inner())
    }

    fn opaque(width: u32, height: u32) -> String {
        png(RgbImage::from_pixel(width, height, Rgb([10, 200, 30])).into())
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir.join("images"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn size(encoded: &Encoded) -> (u32, u32) {
        let bytes = STANDARD.decode(&encoded.data).unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        (image.width(), image.height())
    }

    #[test]
    fn the_same_image_gets_the_same_id_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let images = Images::new(dir.path());
        let data = opaque(4, 3);
        let id = images.add("image/png", &data).unwrap();
        assert_eq!(images.add("image/png", &data).unwrap(), id);
        let read = images.read(&id, false).unwrap();
        assert_eq!(read.data, data);
        assert_eq!(read.mime, "image/png");
        assert!(images.has(&id));
        // A small one is its own thumbnail.
        assert_eq!(images.read(&id, true).unwrap(), read);
        assert_eq!(files(dir.path()), [id.0]);
    }

    #[test]
    fn a_big_image_gets_a_small_copy() {
        let dir = tempfile::tempdir().unwrap();
        let images = Images::new(dir.path());
        let id = images.add("image/png", &opaque(2000, 1000)).unwrap();
        let small = images.read(&id, true).unwrap();
        assert_eq!(small.mime, "image/jpeg");
        assert_eq!(size(&small), (640, 320));
        assert_eq!(size(&images.read(&id, false).unwrap()), (2000, 1000));
        assert_eq!(files(dir.path()), [id.0.clone(), format!("{}.thumb", id.0)]);

        let clear = png(RgbaImage::from_pixel(1000, 2000, Rgba([0, 0, 0, 0])).into());
        let id = images.add("image/png", &clear).unwrap();
        let small = images.read(&id, true).unwrap();
        assert_eq!(small.mime, "image/png");
        assert_eq!(size(&small), (320, 640));
    }

    #[test]
    fn a_missing_small_copy_is_made_when_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let images = Images::new(dir.path());
        let id = images.add("image/png", &opaque(1000, 1000)).unwrap();
        std::fs::remove_file(dir.path().join("images").join(format!("{}.thumb", id.0))).unwrap();
        assert_eq!(size(&images.read(&id, true).unwrap()), (640, 640));
        assert_eq!(files(dir.path()).len(), 2);
    }

    #[test]
    fn what_agents_cant_take_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let images = Images::new(dir.path());
        assert!(images.add("image/tiff", &opaque(4, 3)).is_err());
        assert!(
            images.add("image/jpeg", &opaque(4, 3)).is_err(),
            "it says what it isn't"
        );
        assert!(images.add("image/png", "not base64!").is_err());
        assert!(images.add("image/png", "").is_err());
        let fake = STANDARD.encode(b"\x89PNG\r\n\x1a\nnot really");
        assert!(images.add("image/png", &fake).is_err());
        let huge = STANDARD.encode(vec![0u8; MAX_IMAGE_BYTES + 1]);
        assert!(images.add("image/png", &huge).is_err());
    }

    #[test]
    fn an_id_cant_reach_outside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tectr.db"), "secret").unwrap();
        let images = Images::new(dir.path());
        for id in ["../tectr.db", "", &"a".repeat(63)] {
            assert!(images.read(&ImageId::from(id), false).is_err());
            assert!(!images.has(&ImageId::from(id)));
        }
    }

    #[test]
    fn only_old_files_nothing_names_are_collected() {
        let dir = tempfile::tempdir().unwrap();
        let images = Images::new(dir.path());
        let kept = images.add("image/png", &opaque(1000, 500)).unwrap();
        let dropped = images.add("image/png", &opaque(1000, 501)).unwrap();
        let fresh = images.add("image/png", &opaque(4, 4)).unwrap();
        let folder = dir.path().join("images");
        std::fs::write(folder.join(format!("{}.half", kept.0)), "crashed").unwrap();
        let old = SystemTime::now() - UNREFERENCED_GRACE - Duration::from_secs(60);
        for name in files(dir.path()) {
            if !name.starts_with(&fresh.0) {
                let file = std::fs::File::options()
                    .write(true)
                    .open(folder.join(&name))
                    .unwrap();
                file.set_modified(old).unwrap();
            }
        }

        let keep = HashSet::from([kept.clone()]);
        let before = SystemTime::now() - UNREFERENCED_GRACE;
        assert_eq!(images.collect(&keep, before).unwrap(), 3);
        let mut left = vec![kept.0.clone(), format!("{}.thumb", kept.0), fresh.0.clone()];
        left.sort();
        assert_eq!(files(dir.path()), left);
        assert!(!images.has(&dropped));

        // Adding one again starts its grace period over.
        let again = Images::new(dir.path());
        let file = std::fs::File::options()
            .write(true)
            .open(folder.join(&fresh.0))
            .unwrap();
        file.set_modified(old).unwrap();
        again.add("image/png", &opaque(4, 4)).unwrap();
        assert_eq!(again.collect(&HashSet::new(), before).unwrap(), 2);
        assert_eq!(files(dir.path()), [fresh.0]);
    }
}
