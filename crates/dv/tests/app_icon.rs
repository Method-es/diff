//! The app icon, `resources/windows/dv.ico` (DESIGN §1.7): `build.rs` embeds it in dv.exe and
//! dv-gui.exe as resource 1, the one gpui loads for its windows (the title bar, the taskbar,
//! Alt Tab) and Explorer shows, and `script/package` gives it to the installer. It is
//! `resources/windows/dv.svg` at the sizes Windows asks for, 16 to 256 px. The Linux app's
//! `resources/linux/dv.png` is the same SVG at 256 px.
//!
//! This test renders the SVG and checks the icon against it. After changing the SVG, write the
//! icon again, and look at it (each size, and on a light and a dark taskbar) with
//!
//! ```text
//! DV_WRITE_ICON=1 cargo test -p dv --test app_icon
//! DV_ICON_PREVIEW=target/tmp/icon cargo test -p dv --test app_icon
//! ```

use std::io::Cursor;
use std::path::PathBuf;

use resvg::{tiny_skia, usvg};

/// The sizes in the icon: the small icon at 100–200% scaling (16, 20, 24, 32), the large one
/// and Explorer's views (32, 40, 48, 64), and the jumbo one (256).
const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];
/// Sizes up to this are stored as bitmaps, which every reader of icons understands; bigger
/// ones as PNG, as Windows' own jumbo icons are.
const LARGEST_BITMAP: u32 = 64;
/// The Linux app's icon: one PNG, the size desktops scale from.
const LINUX_ICON_SIZE: u32 = 256;
/// How far a channel of the stored icon may be from the rendered SVG (a patch release of
/// resvg antialiasing an edge a little differently).
const TOLERANCE: u8 = 8;
const WRITE_VAR: &str = "DV_WRITE_ICON";
const PREVIEW_VAR: &str = "DV_ICON_PREVIEW";
/// The taskbars the preview puts the icon on: Windows 11's light and dark.
const TASKBARS: [(&str, [u8; 3]); 2] =
    [("light", [0xee, 0xee, 0xee]), ("dark", [0x1c, 0x1c, 0x1c])];
fn resources() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/windows")
}

/// A square picture, straight (not premultiplied) RGBA.
#[derive(Clone, Debug, PartialEq)]
struct Picture {
    size: u32,
    rgba: Vec<u8>,
}

impl Picture {
    /// The RGBA values premultiplied by alpha: what the eye sees, so a fully transparent
    /// pixel's colour doesn't count.
    fn premultiplied(&self) -> Vec<u8> {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, a]| {
                let scale = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                [scale(r), scale(g), scale(b), a]
            })
            .collect()
    }
}

/// The SVG at `size` × `size`.
fn render(svg: &[u8], size: u32) -> Picture {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).expect("the SVG parses");
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("a pixmap");
    let scale = size as f32 / tree.size().width();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|px| {
            let px = px.demultiply();
            [px.red(), px.green(), px.blue(), px.alpha()]
        })
        .collect();
    Picture { size, rgba }
}

/// An ICO file of `pictures`: 32-bit bitmaps (with their AND mask) up to
/// [`LARGEST_BITMAP`], PNG above.
fn encode_ico(pictures: &[Picture]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = pictures
        .iter()
        .map(|picture| {
            if picture.size <= LARGEST_BITMAP {
                encode_bitmap(picture)
            } else {
                encode_png(picture)
            }
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // an icon
    out.extend_from_slice(&(pictures.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pictures.len();
    for (picture, image) in pictures.iter().zip(&images) {
        // 256 is written as 0.
        let side = if picture.size >= 256 {
            0
        } else {
            picture.size as u8
        };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(image.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.len();
    }
    for image in images {
        out.extend_from_slice(&image);
    }
    out
}

/// Bytes in one row of the 1-bit AND mask, padded to 32 bits.
fn mask_stride(size: u32) -> usize {
    size.div_ceil(32) as usize * 4
}

/// A BITMAPINFOHEADER, the BGRA rows bottom up, then the AND mask (set where the pixel is
/// fully transparent, for readers that ignore alpha).
fn encode_bitmap(picture: &Picture) -> Vec<u8> {
    let size = picture.size;
    let stride = mask_stride(size);
    let mut out = Vec::new();
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    // The height covers the colours and the mask.
    out.extend_from_slice(&(2 * size as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    let image_size = size as usize * size as usize * 4 + stride * size as usize;
    out.extend_from_slice(&(image_size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]); // resolution and palette
    for y in (0..size).rev() {
        for x in 0..size {
            let px = &picture.rgba[((y * size + x) * 4) as usize..][..4];
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    for y in (0..size).rev() {
        let mut row = vec![0u8; stride];
        for x in 0..size {
            if picture.rgba[((y * size + x) * 4 + 3) as usize] == 0 {
                row[(x / 8) as usize] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

fn encode_png(picture: &Picture) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, picture.size, picture.size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header().expect("a PNG header");
    writer
        .write_image_data(&picture.rgba)
        .expect("the PNG's pixels");
    writer.finish().expect("the PNG's end");
    out
}

/// The pictures of an ICO file, in directory order.
fn decode_ico(bytes: &[u8]) -> Vec<Picture> {
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
    assert_eq!((u16_at(0), u16_at(2)), (0, 1), "an ICO header");
    (0..u16_at(4) as usize)
        .map(|entry| {
            let at = 6 + 16 * entry;
            let size = match bytes[at] {
                0 => 256,
                side => u32::from(side),
            };
            assert_eq!(bytes[at + 1], bytes[at], "a square picture");
            assert_eq!(u16_at(at + 6), 32, "32 bits per pixel");
            let len = u32_at(at + 8) as usize;
            let offset = u32_at(at + 12) as usize;
            let image = &bytes[offset..offset + len];
            let picture = if image.starts_with(b"\x89PNG") {
                decode_png(image)
            } else {
                decode_bitmap(image, size)
            };
            assert_eq!(picture.size, size, "entry {entry}");
            picture
        })
        .collect()
}

fn decode_bitmap(image: &[u8], size: u32) -> Picture {
    let i32_at = |at: usize| i32::from_le_bytes(image[at..at + 4].try_into().expect("4 bytes"));
    assert_eq!(i32_at(0), 40, "a BITMAPINFOHEADER");
    assert_eq!((i32_at(4), i32_at(8)), (size as i32, 2 * size as i32));
    let pixels = &image[40..];
    let mut rgba = vec![0; (size * size * 4) as usize];
    for row in 0..size {
        let y = size - 1 - row;
        for x in 0..size {
            let px = &pixels[((row * size + x) * 4) as usize..][..4];
            let at = ((y * size + x) * 4) as usize;
            rgba[at..at + 4].copy_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    let mask = &pixels[(size * size * 4) as usize..];
    assert_eq!(mask.len(), mask_stride(size) * size as usize, "an AND mask");
    Picture { size, rgba }
}

fn decode_png(image: &[u8]) -> Picture {
    let decoder = png::Decoder::new(Cursor::new(image));
    let mut reader = decoder.read_info().expect("a PNG");
    let mut rgba = vec![0; reader.output_buffer_size().expect("a buffer size")];
    let info = reader.next_frame(&mut rgba).expect("the PNG's pixels");
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(info.width, info.height);
    rgba.truncate(info.buffer_size());
    Picture {
        size: info.width,
        rgba,
    }
}

fn largest_difference(a: &Picture, b: &Picture) -> u8 {
    a.premultiplied()
        .iter()
        .zip(b.premultiplied())
        .map(|(a, b)| a.abs_diff(b))
        .max()
        .unwrap_or(0)
}

#[test]
fn the_icon_is_the_mark_at_every_size() {
    let svg = std::fs::read(resources().join("dv.svg")).expect("reading dv.svg");
    let rendered: Vec<Picture> = SIZES.iter().map(|&size| render(&svg, size)).collect();
    let path = resources().join("dv.ico");
    if std::env::var_os(WRITE_VAR).is_some() {
        std::fs::write(&path, encode_ico(&rendered)).expect("writing dv.ico");
    }
    let stored = decode_ico(&std::fs::read(&path).expect("reading dv.ico"));
    let sizes: Vec<u32> = stored.iter().map(|picture| picture.size).collect();
    assert_eq!(sizes, SIZES, "{WRITE_VAR}=1 writes the icon again");
    for (stored, rendered) in stored.iter().zip(&rendered) {
        let difference = largest_difference(stored, rendered);
        assert!(
            difference <= TOLERANCE,
            "the {0}×{0} icon is {difference} off the SVG: {WRITE_VAR}=1 writes it again",
            stored.size
        );
    }
}

/// The Linux app's icon, `resources/linux/dv.png`: the SVG at 256 px, which `script/package
/// --linux-app` gives the AppImage (its `.desktop` entry and the dock).
#[test]
fn the_linux_icon_is_the_mark() {
    let svg = std::fs::read(resources().join("dv.svg")).expect("reading dv.svg");
    let rendered = render(&svg, LINUX_ICON_SIZE);
    let path = resources().join("../linux/dv.png");
    if std::env::var_os(WRITE_VAR).is_some() {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("making its folder");
        std::fs::write(&path, encode_png(&rendered)).expect("writing dv.png");
    }
    let stored = decode_png(&std::fs::read(&path).expect("reading dv.png"));
    assert_eq!(
        stored.size, LINUX_ICON_SIZE,
        "{WRITE_VAR}=1 writes it again"
    );
    let difference = largest_difference(&stored, &rendered);
    assert!(
        difference <= TOLERANCE,
        "the Linux icon is {difference} off the SVG: {WRITE_VAR}=1 writes it again"
    );
}

#[test]
fn icons_round_trip_through_bitmaps_and_png() {
    let svg = std::fs::read(resources().join("dv.svg")).expect("reading dv.svg");
    let pictures: Vec<Picture> = [16, 48, 256]
        .iter()
        .map(|&size| render(&svg, size))
        .collect();
    let decoded = decode_ico(&encode_ico(&pictures));
    assert_eq!(decoded, pictures);
}

#[test]
fn transparent_pixels_are_masked_in_bitmaps() {
    // A 2×2 picture: one opaque pixel, three transparent ones.
    let mut rgba = vec![0; 16];
    rgba[..4].copy_from_slice(&[227, 168, 87, 255]);
    let bitmap = encode_bitmap(&Picture { size: 2, rgba });
    let mask = &bitmap[40 + 16..];
    // Rows bottom up: the bottom row is all transparent, the top row's second pixel is.
    assert_eq!(mask, [0b1100_0000, 0, 0, 0, 0b0100_0000, 0, 0, 0]);
}

#[test]
fn the_icon_is_a_split_diff_on_the_accent_tile() {
    let svg = std::fs::read(resources().join("dv.svg")).expect("reading dv.svg");
    let picture = render(&svg, 256);
    let at = |x: u32, y: u32| &picture.rgba[((y * 256 + x) * 4) as usize..][..4];
    assert_eq!(at(2, 2)[3], 0, "the corners are transparent");
    assert_eq!(at(128, 30)[3], 0xff, "the tile is opaque");
    // Two panes of the code surface, the old side's changed line red, the new side's green.
    assert_eq!(at(80, 195), [0x15, 0x14, 0x1b, 0xff]);
    assert_eq!(at(172, 195), [0x15, 0x14, 0x1b, 0xff]);
    assert_eq!(at(82, 128), [0xff, 0x67, 0x67, 0xff]);
    assert_eq!(at(172, 128), [0x61, 0xff, 0xca, 0xff]);
}

/// With `DV_ICON_PREVIEW=<folder>`: each size as a PNG, and each on a light and a dark
/// taskbar, to look at.
#[test]
fn previews_are_written_when_asked() {
    let Some(dir) = std::env::var_os(PREVIEW_VAR) else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("making the preview folder");
    let svg = std::fs::read(resources().join("dv.svg")).expect("reading dv.svg");
    for size in SIZES {
        let picture = render(&svg, size);
        std::fs::write(dir.join(format!("dv-{size}.png")), encode_png(&picture))
            .expect("writing a preview");
        for (name, [r, g, b]) in TASKBARS {
            // The icon centred on a square of the taskbar's colour, then scaled up 8 times
            // (pixels kept square) so the small sizes can be read.
            let pad = size / 2;
            let side = size + 2 * pad;
            let mut rgba = Vec::with_capacity((side * side * 4) as usize);
            for y in 0..side {
                for x in 0..side {
                    let inside = (pad..pad + size).contains(&x) && (pad..pad + size).contains(&y);
                    let [fr, fg, fb, fa] = if inside {
                        let at = (((y - pad) * size + (x - pad)) * 4) as usize;
                        [
                            picture.rgba[at],
                            picture.rgba[at + 1],
                            picture.rgba[at + 2],
                            picture.rgba[at + 3],
                        ]
                    } else {
                        [0, 0, 0, 0]
                    };
                    let mix = |f: u8, b: u8| {
                        ((u16::from(f) * u16::from(fa) + u16::from(b) * (255 - u16::from(fa)))
                            / 255) as u8
                    };
                    rgba.extend_from_slice(&[mix(fr, r), mix(fg, g), mix(fb, b), 255]);
                }
            }
            let zoom = if size <= 64 { 8 } else { 1 };
            let big = side * zoom;
            let mut scaled = Vec::with_capacity((big * big * 4) as usize);
            for y in 0..big {
                for x in 0..big {
                    let at = (((y / zoom) * side + x / zoom) * 4) as usize;
                    scaled.extend_from_slice(&rgba[at..at + 4]);
                }
            }
            let preview = Picture {
                size: big,
                rgba: scaled,
            };
            std::fs::write(
                dir.join(format!("dv-{size}-on-{name}.png")),
                encode_png(&preview),
            )
            .expect("writing a preview");
        }
    }
}
