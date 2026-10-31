//! Camera frames shrunk for the preview and read for QR codes.

use istmo_camera::{CameraFrame, PixelFormat};

/// Longest side kept from each frame. Enough for a code held a hand
/// away, small enough to read every frame.
const MAX_SIDE: u32 = 960;

/// One frame shrunk to at most [`MAX_SIDE`]: RGBA for the preview and
/// luma for the decoder.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Picture {
    pub(super) width: u32,
    pub(super) height: u32,
    /// Clockwise turn that shows the picture upright.
    pub(super) rotation: u16,
    pub(super) rgba: Vec<u8>,
    luma: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Picture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rotation", &self.rotation)
            .finish_non_exhaustive()
    }
}

/// Rec. 601 luma of an sRGB pixel.
fn luma(r: u8, g: u8, b: u8) -> u8 {
    let weighted = u32::from(r) * 77 + u32::from(g) * 150 + u32::from(b) * 29;
    u8::try_from(weighted >> 8).unwrap_or(u8::MAX)
}

impl Picture {
    /// Shrink a packed RGBA or BGRA frame. `None` for other formats or
    /// a buffer shorter than its planes say.
    #[sismo::function("scan::raster")]
    pub(super) fn of(frame: &CameraFrame) -> Option<Self> {
        let bgra = match frame.pixel_format {
            PixelFormat::Rgba8 => false,
            PixelFormat::Bgra8 => true,
            PixelFormat::Nv21 | PixelFormat::Yuv420 | PixelFormat::Jpeg(_) => return None,
        };
        let (width, height) = (frame.resolution.width, frame.resolution.height);
        let plane = frame.planes.first();
        let offset = usize::try_from(plane.map_or(0, |plane| plane.offset)).ok()?;
        let stride = usize::try_from(plane.map_or(width * 4, |plane| plane.row_stride)).ok()?;
        let step = width.max(height).div_ceil(MAX_SIDE).max(1);
        let (out_width, out_height) = (width / step, height / step);
        if out_width == 0 || out_height == 0 {
            return None;
        }
        let pixels = usize::try_from(out_width * out_height).ok()?;
        let mut rgba = Vec::with_capacity(pixels * 4);
        let mut luma_plane = Vec::with_capacity(pixels);
        let step = usize::try_from(step).ok()?;
        for row in 0..usize::try_from(out_height).ok()? {
            let start = offset + row * step * stride;
            for column in 0..usize::try_from(out_width).ok()? {
                let at = start + column * step * 4;
                let &[first, g, third, _] = frame.bytes.get(at..at + 4)? else {
                    return None;
                };
                let (r, b) = if bgra { (third, first) } else { (first, third) };
                rgba.extend_from_slice(&[r, g, b, u8::MAX]);
                luma_plane.push(luma(r, g, b));
            }
        }
        Some(Self {
            width: out_width,
            height: out_height,
            rotation: frame.rotation_degrees,
            rgba,
            luma: luma_plane,
        })
    }

    /// Text of the first QR code found in the picture.
    #[sismo::function("scan::decode")]
    pub(super) fn read_qr(&self) -> Option<String> {
        let width = usize::try_from(self.width).ok()?;
        let height = usize::try_from(self.height).ok()?;
        let mut image = {
            sismo::scope!("scan::decode::prepare");
            rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
                self.luma.get(y * width + x).copied().unwrap_or(u8::MAX)
            })
        };
        let grids = {
            sismo::scope!("scan::decode::detect_grids");
            image.detect_grids()
        };
        sismo::counter!("scan.grids", grids.len());
        sismo::scope!("scan::decode::grids");
        grids
            .into_iter()
            .find_map(|grid| grid.decode().ok().map(|(_, text)| text))
    }
}

#[cfg(test)]
mod tests {
    use istmo_camera::{FramePlane, Resolution};

    use super::*;

    /// `text` as a QR code, `module` pixels per module and a four
    /// module quiet zone, in `format` with `pad` spare bytes per row.
    fn qr_frame(text: &str, module: u32, format: PixelFormat, pad: u32) -> CameraFrame {
        let code = qrcode::QrCode::new(text.as_bytes()).expect("encodable");
        let modules = u32::try_from(code.width()).expect("small code");
        let colors = code.to_colors();
        let side = (modules + 8) * module;
        let stride = side * 4 + pad;
        let mut bytes = vec![u8::MAX; usize::try_from(stride * side).expect("fits")];
        for y in 0..side {
            for x in 0..side {
                let (mx, my) = ((x / module).wrapping_sub(4), (y / module).wrapping_sub(4));
                let dark = mx < modules
                    && my < modules
                    && colors[usize::try_from(my * modules + mx).expect("fits")]
                        == qrcode::Color::Dark;
                if dark {
                    let at = usize::try_from(y * stride + x * 4).expect("fits");
                    bytes[at..at + 3].fill(0);
                }
            }
        }
        CameraFrame {
            seq: 0,
            timestamp_ns: 0,
            resolution: Resolution {
                width: side,
                height: side,
            },
            rotation_degrees: 90,
            pixel_format: format,
            planes: vec![FramePlane {
                offset: 0,
                row_stride: stride,
                pixel_stride: 4,
                width: side,
                height: side,
            }],
            bytes,
            dropped_before: 0,
        }
    }

    #[test]
    fn reads_a_code_from_rgba() {
        let frame = qr_frame("notein://pair/abc123", 6, PixelFormat::Rgba8, 0);
        let picture = Picture::of(&frame).expect("rgba frame");
        assert_eq!(picture.rotation, 90);
        assert_eq!(picture.read_qr().as_deref(), Some("notein://pair/abc123"));
    }

    #[test]
    fn reads_a_code_from_padded_bgra_shrunk_below_the_limit() {
        // 8 px modules over ~33 modules: wider than MAX_SIDE, so shrunk.
        let frame = qr_frame("notein://join/xyz", 32, PixelFormat::Bgra8, 12);
        assert!(frame.resolution.width > MAX_SIDE);
        let picture = Picture::of(&frame).expect("bgra frame");
        assert!(picture.width <= MAX_SIDE);
        assert_eq!(picture.read_qr().as_deref(), Some("notein://join/xyz"));
    }

    #[test]
    fn blank_frame_reads_nothing() {
        let mut frame = qr_frame("x", 4, PixelFormat::Rgba8, 0);
        frame.bytes.fill(u8::MAX);
        assert_eq!(Picture::of(&frame).and_then(|p| p.read_qr()), None);
    }

    #[test]
    fn yuv_and_short_buffers_are_skipped() {
        let mut frame = qr_frame("x", 4, PixelFormat::Nv21, 0);
        assert!(Picture::of(&frame).is_none());
        frame.pixel_format = PixelFormat::Rgba8;
        frame.bytes.truncate(10);
        assert!(Picture::of(&frame).is_none());
    }

    #[test]
    fn luma_spans_the_range() {
        assert_eq!(luma(0, 0, 0), 0);
        assert_eq!(luma(255, 255, 255), 255);
    }
}
