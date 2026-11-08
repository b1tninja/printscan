//! Raw SANE frames as a PNG page.
//!
//! A frame is rows of `bytes_per_line` bytes, each starting with `pixels_per_line` samples of
//! `depth` bits (grey, or red, green and blue interleaved). A 1-bit grey frame has 1 for black,
//! the opposite of PNG, so its bits are inverted. 16-bit samples come in the byte order the start
//! reply declared and are written big-endian, as PNG wants. A three-pass colour scan sends one
//! frame per channel, which are interleaved here. A frame whose line count was not known in advance
//! (`lines` -1) is as long as the rows that arrived; a frame that ended short keeps its complete
//! rows. The resolution goes into the PNG's `pHYs`, so the page's density is read from the file.

use crate::protocol::{ByteOrder, Frame, Parameters};

/// One frame as it arrived.
#[derive(Clone, Debug)]
pub(crate) struct RawFrame {
    pub parameters: Parameters,
    pub data: Vec<u8>,
}

/// Pixels per metre for `dpi`.
fn per_metre(dpi: u32) -> u32 {
    (f64::from(dpi) / 0.0254).round() as u32
}

/// The frame's geometry: width, bytes per line, and the rows that are complete.
fn geometry(frame: &RawFrame, samples_per_pixel: u64) -> Result<(u32, usize, usize), String> {
    let p = frame.parameters;
    let width = u32::try_from(p.pixels_per_line).ok().filter(|&w| w > 0).ok_or_else(|| format!("a frame {} pixels wide", p.pixels_per_line))?;
    let stride =
        usize::try_from(p.bytes_per_line).ok().filter(|&s| s > 0).ok_or_else(|| format!("a frame of {} bytes per line", p.bytes_per_line))?;
    let depth = u64::try_from(p.depth).unwrap_or(0);
    let needed = (u64::from(width) * samples_per_pixel * depth).div_ceil(8);
    if needed > stride as u64 {
        return Err(format!("{width} pixels at {depth} bits do not fit in {stride} bytes per line"));
    }
    let arrived = frame.data.len() / stride;
    let rows = match usize::try_from(p.lines) {
        Ok(lines) => lines.min(arrived),
        Err(_) => arrived,
    };
    if rows == 0 {
        return Err("the frame has no complete line".into());
    }
    Ok((width, stride, rows))
}

/// The bytes of one row's samples, `row_bytes` long, in PNG order.
fn push_row(out: &mut Vec<u8>, row: &[u8], depth: i32, order: ByteOrder, invert: bool) {
    match depth {
        16 if order == ByteOrder::Little => {
            for [low, high] in row.as_chunks::<2>().0 {
                out.extend_from_slice(&[*high, *low]);
            }
        }
        _ if invert => out.extend(row.iter().map(|b| !b)),
        _ => out.extend_from_slice(row),
    }
}

/// Encode `samples` as a PNG.
fn png(width: u32, height: u32, color: png::ColorType, depth: i32, dpi: (u32, u32), samples: &[u8]) -> Result<Vec<u8>, String> {
    let bit_depth = match depth {
        1 => png::BitDepth::One,
        8 => png::BitDepth::Eight,
        16 => png::BitDepth::Sixteen,
        other => return Err(format!("{other}-bit samples are not supported")),
    };
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(color);
    encoder.set_depth(bit_depth);
    encoder.set_pixel_dims(Some(png::PixelDimensions { xppu: per_metre(dpi.0), yppu: per_metre(dpi.1), unit: png::Unit::Meter }));
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(samples).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

/// A one-frame image (grey or interleaved colour).
fn single(frame: &RawFrame, order: ByteOrder, dpi: (u32, u32)) -> Result<Vec<u8>, String> {
    let p = frame.parameters;
    let (channels, color) = match p.format {
        Frame::Gray => (1, png::ColorType::Grayscale),
        Frame::Rgb => (3, png::ColorType::Rgb),
        other => return Err(format!("a single {other:?} frame is not an image")),
    };
    if p.depth == 1 && channels == 3 {
        return Err("1-bit colour frames are not supported".into());
    }
    if !matches!(p.depth, 1 | 8 | 16) {
        return Err(format!("{}-bit samples are not supported", p.depth));
    }
    let (width, stride, rows) = geometry(frame, channels)?;
    let row_bytes = (u64::from(width) * channels * u64::try_from(p.depth).unwrap_or(8)).div_ceil(8) as usize;
    let mut samples = Vec::with_capacity(row_bytes * rows);
    for row in frame.data.chunks_exact(stride).take(rows) {
        push_row(&mut samples, &row[..row_bytes], p.depth, order, p.depth == 1);
    }
    png(width, rows as u32, color, p.depth, dpi, &samples)
}

/// Three frames, one per channel, as one colour image.
fn three_pass(frames: &[RawFrame], order: ByteOrder, dpi: (u32, u32)) -> Result<Vec<u8>, String> {
    let channel = |which: Frame| frames.iter().find(|f| f.parameters.format == which).ok_or_else(|| format!("the {which:?} frame is missing"));
    let planes = [channel(Frame::Red)?, channel(Frame::Green)?, channel(Frame::Blue)?];
    let first = planes[0].parameters;
    if planes.iter().any(|f| f.parameters.pixels_per_line != first.pixels_per_line || f.parameters.depth != first.depth) {
        return Err("the three colour frames differ in size or depth".into());
    }
    if !matches!(first.depth, 8 | 16) {
        return Err(format!("{}-bit three-pass colour is not supported", first.depth));
    }
    let mut shape = Vec::new();
    for plane in planes {
        shape.push(geometry(plane, 1)?);
    }
    let width = shape[0].0;
    let rows = shape.iter().map(|s| s.2).min().unwrap_or(0);
    let sample = if first.depth == 16 { 2 } else { 1 };
    let mut samples = Vec::with_capacity(width as usize * 3 * sample * rows);
    for y in 0..rows {
        for x in 0..width as usize {
            for (plane, &(_, stride, _)) in planes.iter().zip(&shape) {
                let at = y * stride + x * sample;
                push_row(&mut samples, &plane.data[at..at + sample], first.depth, order, false);
            }
        }
    }
    png(width, rows as u32, png::ColorType::Rgb, first.depth, dpi, &samples)
}

/// The frames of one image as a PNG at `dpi` (horizontal, vertical).
pub(crate) fn encode(frames: &[RawFrame], order: ByteOrder, dpi: (u32, u32)) -> Result<Vec<u8>, String> {
    match frames {
        [] => Err("the scan delivered no frame".into()),
        [frame] => single(frame, order, dpi),
        _ => three_pass(frames, order, dpi),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(format: Frame, last: bool, bpl: i32, ppl: i32, lines: i32, depth: i32, data: Vec<u8>) -> RawFrame {
        RawFrame { parameters: Parameters::new(format, last, bpl, ppl, lines, depth), data }
    }

    /// `(width, height, colour, depth, pixels, pixels per metre x)` of a PNG.
    fn decode(bytes: &[u8]) -> (u32, u32, png::ColorType, png::BitDepth, Vec<u8>, u32) {
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        let ppm = reader.info().pixel_dims.map_or(0, |d| d.xppu);
        (info.width, info.height, info.color_type, info.bit_depth, buf, ppm)
    }

    #[test]
    fn grey_rows_drop_their_padding() {
        // 3 pixels in 4 bytes per line, 2 lines.
        let f = frame(Frame::Gray, true, 4, 3, 2, 8, vec![1, 2, 3, 99, 4, 5, 6, 99]);
        let (w, h, color, _, pixels, ppm) = decode(&encode(&[f], ByteOrder::Big, (300, 300)).unwrap());
        assert_eq!((w, h, color), (3, 2, png::ColorType::Grayscale));
        assert_eq!(pixels, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(ppm, 11811);
    }

    #[test]
    fn lineart_is_inverted() {
        // SANE: bit set = black. 0b1000_0000 is one black pixel then white.
        let f = frame(Frame::Gray, true, 1, 8, 1, 1, vec![0b1000_0000]);
        let (_, _, _, depth, pixels, _) = decode(&encode(&[f], ByteOrder::Big, (300, 300)).unwrap());
        assert_eq!(depth, png::BitDepth::One);
        assert_eq!(pixels, vec![0b0111_1111]);
    }

    #[test]
    fn sixteen_bit_little_endian_is_swapped() {
        let f = frame(Frame::Gray, true, 4, 2, 1, 16, vec![0x34, 0x12, 0xcd, 0xab]);
        let (_, _, _, _, pixels, _) = decode(&encode(&[f], ByteOrder::Little, (100, 100)).unwrap());
        assert_eq!(pixels, vec![0x12, 0x34, 0xab, 0xcd]);
        let f = frame(Frame::Gray, true, 4, 2, 1, 16, vec![0x12, 0x34, 0xab, 0xcd]);
        let (_, _, _, _, pixels, _) = decode(&encode(&[f], ByteOrder::Big, (100, 100)).unwrap());
        assert_eq!(pixels, vec![0x12, 0x34, 0xab, 0xcd]);
    }

    #[test]
    fn unknown_and_short_line_counts() {
        // lines -1: as many as arrived; the half line at the end is dropped.
        let f = frame(Frame::Gray, true, 2, 2, -1, 8, vec![1, 2, 3, 4, 5]);
        assert_eq!(decode(&encode(&[f], ByteOrder::Big, (100, 100)).unwrap()).1, 2);
        // 10 lines announced, 1 arrived.
        let f = frame(Frame::Gray, true, 2, 2, 10, 8, vec![1, 2]);
        assert_eq!(decode(&encode(&[f], ByteOrder::Big, (100, 100)).unwrap()).1, 1);
        let f = frame(Frame::Gray, true, 2, 2, 10, 8, vec![1]);
        assert!(encode(&[f], ByteOrder::Big, (100, 100)).is_err());
    }

    #[test]
    fn three_passes_are_interleaved() {
        let frames = [
            frame(Frame::Red, false, 2, 2, 1, 8, vec![10, 11]),
            frame(Frame::Green, false, 2, 2, 1, 8, vec![20, 21]),
            frame(Frame::Blue, true, 2, 2, 1, 8, vec![30, 31]),
        ];
        let (w, h, color, _, pixels, _) = decode(&encode(&frames, ByteOrder::Big, (100, 100)).unwrap());
        assert_eq!((w, h, color), (2, 1, png::ColorType::Rgb));
        assert_eq!(pixels, vec![10, 20, 30, 11, 21, 31]);
        assert!(encode(&frames[..2], ByteOrder::Big, (100, 100)).is_err());
    }

    #[test]
    fn hostile_geometry_is_refused() {
        for f in [
            frame(Frame::Gray, true, 1, 8, 1, 8, vec![0; 8]),
            frame(Frame::Gray, true, 0, 1, 1, 8, vec![0]),
            frame(Frame::Gray, true, 1, -1, 1, 8, vec![0]),
            frame(Frame::Gray, true, 1, 1, 1, 4, vec![0]),
            frame(Frame::Rgb, true, 1, 8, 1, 1, vec![0; 3]),
            frame(Frame::Other(9), true, 1, 1, 1, 8, vec![0]),
        ] {
            assert!(encode(&[f], ByteOrder::Big, (100, 100)).is_err());
        }
    }
}
