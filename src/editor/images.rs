//! Images in notes.
//!
//! An image added from the editor is decoded and encoded again, so only
//! its pixels survive (no EXIF, location, comments or anything a crafted
//! file could carry), then stored in the item's `assets/` folder and
//! linked with a relative path. That folder travels with the item when it
//! is exported. Remote images go through the same cleaning into a cache.

use std::collections::HashMap;
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Cursor, Read};
use std::path::{Component as PathPart, Path, PathBuf};
use std::rc::Rc;

use freya::prelude::*;
use freya_markdown_editor::prelude::{
    Decorator, EditorColors, EditorController, ImageContext, ImagePicker, ImageRequest,
};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use crate::library::model::ItemId;

use crate::library::bodies::ASSETS_SUBDIR as ASSETS_DIR;
/// Longest side an image is stored at; bigger ones are scaled down.
const MAX_SIDE: u32 = 4096;
/// Largest file read or downloaded, before decoding.
const MAX_BYTES: u64 = 40 * 1024 * 1024;
/// Images kept decoded in memory, so remounting a note does not flash.
const CACHE_LIMIT: usize = 48;

#[derive(Debug)]
pub(crate) enum ImageError {
    Io(io::Error),
    Decode(image::ImageError),
    Download(reqwest::Error),
    TooLarge,
    /// The link points outside the item, or nowhere usable.
    Unsupported,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "image file: {err}"),
            Self::Decode(err) => write!(f, "image decode: {err}"),
            Self::Download(err) => write!(f, "image download: {err}"),
            Self::TooLarge => f.write_str("image is too large"),
            Self::Unsupported => f.write_str("image link is not supported"),
        }
    }
}

impl std::error::Error for ImageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Decode(err) => Some(err),
            Self::Download(err) => Some(err),
            Self::TooLarge | Self::Unsupported => None,
        }
    }
}

impl From<io::Error> for ImageError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<image::ImageError> for ImageError {
    fn from(err: image::ImageError) -> Self {
        Self::Decode(err)
    }
}

impl From<reqwest::Error> for ImageError {
    fn from(err: reqwest::Error) -> Self {
        Self::Download(err)
    }
}

/// An image with nothing but its pixels, ready to store.
pub(crate) struct CleanImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) extension: &'static str,
}

impl CleanImage {
    /// Decode `raw` (PNG, JPEG, WebP or the first frame of a GIF), turn it
    /// upright, cap its size and encode it again.
    pub(crate) fn from_raw(raw: &[u8]) -> Result<Self, ImageError> {
        let reader = ImageReader::new(Cursor::new(raw)).with_guessed_format()?;
        let format = reader.format();
        let mut decoder = reader.into_decoder()?;
        let orientation = decoder.orientation()?;
        let mut image = DynamicImage::from_decoder(decoder)?;
        image.apply_orientation(orientation);
        if image.width().max(image.height()) > MAX_SIDE {
            image = image.resize(MAX_SIDE, MAX_SIDE, image::imageops::FilterType::Lanczos3);
        }
        let mut bytes = Vec::new();
        let photo = format == Some(ImageFormat::Jpeg) && !image.color().has_alpha();
        let extension = if photo {
            let encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(Cursor::new(&mut bytes), 90);
            image.to_rgb8().write_with_encoder(encoder)?;
            "jpg"
        } else {
            image.write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)?;
            "png"
        };
        Ok(Self { bytes, extension })
    }

    /// File name derived from the content, so adding the same image twice
    /// stores it once.
    fn file_name(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.bytes.hash(&mut hasher);
        format!("{:016x}.{}", hasher.finish(), self.extension)
    }
}

/// Where an image link leads.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Source {
    File(PathBuf),
    Remote(String),
}

/// Images of one library item: drawn from its folder and added to it.
#[derive(Clone, Debug)]
pub(crate) struct ItemImages {
    item: ItemId,
    root: PathBuf,
}

impl ItemImages {
    #[must_use]
    pub(crate) fn new(item: ItemId) -> Self {
        Self {
            item,
            root: crate::library::bodies::item_root(item),
        }
    }

    /// Only links inside the item are read from disk: a note someone
    /// shared must not show files of the device it is opened on.
    fn resolve(&self, url: &str) -> Option<Source> {
        if url.starts_with("https://") || url.starts_with("http://") {
            return Some(Source::Remote(url.to_owned()));
        }
        let path = Path::new(url);
        path.components()
            .all(|part| matches!(part, PathPart::Normal(_)))
            .then(|| Source::File(self.root.join(path)))
    }

    /// Picker that stores chosen images in this item.
    #[must_use]
    pub(crate) fn picker(&self) -> ImagePicker {
        let (item, root) = (self.item, self.root.clone());
        ImagePicker::new(move |controller, request| {
            let root = root.clone();
            spawn_in_window(async move {
                if let Err(err) = add_picked(item, &root, controller, request).await {
                    log::warn!("adding an image: {err}");
                }
            });
        })
    }
}

/// Ask for a file, clean it off the UI thread, store it and link it.
#[allow(clippy::future_not_send, reason = "runs on the UI executor")]
async fn add_picked(
    item: ItemId,
    root: &Path,
    controller: EditorController,
    request: ImageRequest,
) -> Result<(), ImageError> {
    let picked = crate::files::pick(
        "Imagen",
        &["png", "jpg", "jpeg", "webp", "gif"],
        &["image/png", "image/jpeg", "image/webp", "image/gif"],
    )
    .await;
    let picked = match picked {
        Ok(Some(picked)) => picked,
        Ok(None) => return Ok(()),
        Err(err) => {
            log::warn!("image picker: {err}");
            return Ok(());
        }
    };
    let root = root.to_owned();
    let (url, name) = off_thread(move || {
        let clean = CleanImage::from_raw(&picked.bytes)?;
        let name = clean.file_name();
        let dir = root.join(ASSETS_DIR);
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&name);
        if !path.exists() {
            let partial = dir.join(format!("{name}.tmp"));
            std::fs::write(&partial, &clean.bytes)?;
            std::fs::rename(&partial, &path)?;
        }
        Ok((format!("{ASSETS_DIR}/{name}"), name))
    })
    .await?;
    crate::sync::asset_added(item, &name);
    match request {
        ImageRequest::Insert => controller.insert_image(&url, ""),
        ImageRequest::Replace(slot) => controller.set_image_url(slot, &url),
    }
    Ok(())
}

/// Run `work` on its own thread and wait for it without blocking the UI.
#[allow(clippy::future_not_send, reason = "runs on the UI executor")]
async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ImageError> + Send + 'static,
) -> Result<T, ImageError> {
    let (done, result) = flume::bounded(1);
    std::thread::Builder::new()
        .name("notein-image".to_owned())
        .spawn(move || {
            let _ = done.send(work());
        })?;
    result
        .recv_async()
        .await
        .unwrap_or_else(|_| Err(ImageError::Unsupported))
}

impl Decorator for ItemImages {
    fn render_image(&self, image: &ImageContext<'_>) -> Option<Element> {
        Some(
            NoteImage {
                item: self.item,
                source: self.resolve(image.url),
                name: if image.alt.is_empty() {
                    image.url.rsplit('/').next().unwrap_or(image.url).to_owned()
                } else {
                    image.alt.to_owned()
                },
                max_width: image.max_width,
                zoom: image.theme.zoom,
                colors: image.theme.colors.clone(),
            }
            .into_element(),
        )
    }
}

/// Pixels of a loaded image and their size.
#[derive(Clone, PartialEq)]
struct Loaded {
    key: u64,
    bytes: Bytes,
    width: u32,
    height: u32,
}

#[derive(Clone, PartialEq)]
enum Load {
    Loading,
    Ready(Rc<Loaded>),
    Failed,
}

thread_local! {
    static LOADED: std::cell::RefCell<HashMap<Source, Rc<Loaded>>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Bytes of `source`, cleaned when they come from the network.
fn fetch(source: &Source) -> Result<Vec<u8>, ImageError> {
    match source {
        Source::File(path) => {
            if std::fs::metadata(path)?.len() > MAX_BYTES {
                return Err(ImageError::TooLarge);
            }
            Ok(std::fs::read(path)?)
        }
        Source::Remote(url) => {
            let mut hasher = DefaultHasher::new();
            url.hash(&mut hasher);
            let cached = istmo::path::cache_dir()
                .join("note-images")
                .join(format!("{:016x}", hasher.finish()));
            if let Ok(bytes) = std::fs::read(&cached) {
                return Ok(bytes);
            }
            let response = reqwest::blocking::get(url)?.error_for_status()?;
            let mut raw = Vec::new();
            response.take(MAX_BYTES + 1).read_to_end(&mut raw)?;
            if raw.len() as u64 > MAX_BYTES {
                return Err(ImageError::TooLarge);
            }
            let clean = CleanImage::from_raw(&raw)?;
            if let Some(dir) = cached.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&cached, &clean.bytes)?;
            Ok(clean.bytes)
        }
    }
}

fn load(source: &Source) -> Result<Loaded, ImageError> {
    let bytes = fetch(source)?;
    let (width, height) = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()?
        .into_dimensions()?;
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    bytes.len().hash(&mut hasher);
    Ok(Loaded {
        key: hasher.finish(),
        bytes: Bytes::from(bytes),
        width,
        height,
    })
}

/// An image of a note, loading in the background.
#[derive(Clone, PartialEq)]
struct NoteImage {
    /// Whose files to wait for when the image is not here yet.
    item: ItemId,
    source: Option<Source>,
    name: String,
    max_width: f32,
    zoom: f32,
    colors: EditorColors,
}

impl NoteImage {
    /// Width it is drawn at, never wider than the text.
    fn width(&self, natural: f32) -> f32 {
        let room = if self.max_width > 0. {
            self.max_width
        } else {
            480. * self.zoom
        };
        (natural * self.zoom).min(room)
    }

    fn failed(&self) -> Element {
        let colors = &self.colors;
        rect()
            .padding((8. * self.zoom, 12. * self.zoom))
            .corner_radius(8. * self.zoom)
            .background(colors.code_background)
            .horizontal()
            .spacing(8. * self.zoom)
            .cross_align(Alignment::center())
            .child(
                SvgViewer::new(freya::icons::lucide::image_off())
                    .color(colors.muted)
                    .width(Size::px(16. * self.zoom))
                    .height(Size::px(16. * self.zoom)),
            )
            .child(
                rect()
                    .child(
                        label()
                            .font_size(13. * self.zoom)
                            .color(colors.text)
                            .max_lines(1)
                            .text(self.name.clone()),
                    )
                    .child(
                        label()
                            .font_size(11.5 * self.zoom)
                            .color(colors.muted)
                            .text("No se pudo cargar la imagen"),
                    ),
            )
            .into_element()
    }
}

impl Component for NoteImage {
    fn render(&self) -> impl IntoElement {
        let source = self.source.clone();
        let cached = source
            .as_ref()
            .and_then(|source| LOADED.with(|loaded| loaded.borrow().get(source).cloned()));
        let mut state = use_state(|| match (&source, cached) {
            (_, Some(loaded)) => Load::Ready(loaded),
            (Some(_), None) => Load::Loading,
            (None, None) => Load::Failed,
        });
        let item = self.item;
        use_hook(move || {
            let Some(source) = source else {
                return;
            };
            spawn(async move {
                // An image someone just added may still be on its way: try
                // again whenever more of the item's files arrive.
                let arrivals = crate::plugins::content::watch_assets(item);
                loop {
                    if matches!(*state.peek(), Load::Ready(_)) {
                        return;
                    }
                    let wanted = source.clone();
                    match off_thread(move || load(&wanted)).await {
                        Ok(loaded) => {
                            let loaded = Rc::new(loaded);
                            LOADED.with(|cache| {
                                let mut cache = cache.borrow_mut();
                                if cache.len() >= CACHE_LIMIT {
                                    cache.clear();
                                }
                                cache.insert(source, loaded.clone());
                            });
                            state.set(Load::Ready(loaded));
                            return;
                        }
                        Err(err) => {
                            log::warn!("{err}");
                            state.set(Load::Failed);
                        }
                    }
                    if arrivals.recv_async().await.is_err() {
                        return;
                    }
                }
            });
        });

        match &*state.read() {
            Load::Ready(loaded) => {
                #[allow(clippy::cast_precision_loss, reason = "pixel sizes")]
                let (natural, ratio) = (
                    loaded.width as f32,
                    loaded.height as f32 / loaded.width.max(1) as f32,
                );
                let width = self.width(natural);
                rect()
                    .corner_radius(8. * self.zoom)
                    .overflow(Overflow::Clip)
                    .child(
                        ImageViewer::new((loaded.key, loaded.bytes.clone()))
                            .width(Size::px(width))
                            .height(Size::px(width * ratio)),
                    )
                    .into_element()
            }
            Load::Loading => rect()
                .width(Size::px(self.width(320.)))
                .height(Size::px(180. * self.zoom))
                .corner_radius(8. * self.zoom)
                .background(self.colors.code_background)
                .center()
                .child(
                    SvgViewer::new(freya::icons::lucide::image())
                        .color(self.colors.muted.with_a(120))
                        .width(Size::px(22. * self.zoom))
                        .height(Size::px(22. * self.zoom)),
                )
                .into_element(),
            Load::Failed => self.failed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_links_inside_the_item_are_read() {
        let images = ItemImages {
            item: ItemId([1; 16]),
            root: PathBuf::from("/items/a"),
        };
        assert_eq!(
            images.resolve("assets/x.png"),
            Some(Source::File(PathBuf::from("/items/a/assets/x.png")))
        );
        assert_eq!(images.resolve("../b/doc.loro"), None);
        assert_eq!(images.resolve("/etc/passwd"), None);
        assert!(matches!(
            images.resolve("https://example.com/a.png"),
            Some(Source::Remote(_))
        ));
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in data {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn cleaning_drops_everything_but_pixels() {
        let mut raw = Vec::new();
        DynamicImage::new_rgba8(3, 2)
            .write_to(&mut Cursor::new(&mut raw), ImageFormat::Png)
            .expect("encode");
        // A text chunk before the end of the file.
        let body = b"tEXtnote\0hello";
        let mut chunk = u32::try_from(body.len() - 4)
            .expect("len")
            .to_be_bytes()
            .to_vec();
        chunk.extend_from_slice(body);
        chunk.extend_from_slice(&crc32(body).to_be_bytes());
        let end = raw.len() - 12;
        raw.splice(end..end, chunk);
        assert!(raw.windows(4).any(|window| window == b"tEXt"));
        let clean = CleanImage::from_raw(&raw).expect("clean");
        assert_eq!(clean.extension, "png");
        assert!(!clean.bytes.windows(4).any(|window| window == b"tEXt"));
    }
}
