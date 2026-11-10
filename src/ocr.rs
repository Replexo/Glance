// Copyright (c) 2026 Replexo
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text recognition with Tesseract
//!
//! Images are read in the background when they are shown. The words that
//! were found come back with their position in the displayed image, so they
//! can be selected on top of it.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

use image::{Rgb, RgbImage};

use crate::deps::*;

/// Languages that are read when their data is installed and the device
/// uses them, with their locale codes
const LANGUAGES_LIST: &[(&str, &str)] = &[
    ("eng", "en"),
    ("spa", "es"),
    ("fra", "fr"),
    ("deu", "de"),
    ("por", "pt"),
    ("rus", "ru"),
    ("pol", "pl"),
    ("tur", "tr"),
];

/// Images with more pixels are not read
const MAX_PIXELS: u64 = 60_000_000;
/// Longer sides are scaled down before reading
const MAX_SIDE: u32 = 4000;
/// Tilts tried for slanted text, in degrees clockwise
const TILTS: [f64; 6] = [10., -10., 20., -20., 30., -30.];
/// Longer sides are scaled down before reading tilted text
const MAX_TILT_SIDE: u32 = 1800;
/// Most Tesseract instances reading at once
const MAX_ENGINES: usize = 4;
/// Mean confidence of a read that needs no other tries
const SURE_CONFIDENCE: f32 = 85.;
/// Half size copies with a shorter side are not read for large text
const MIN_SPARSE_SIDE: u32 = 300;
/// Words from the extra reads are only added from this confidence on,
/// and with at least three letters or digits
const EXTRA_CONFIDENCE: f32 = 65.;
/// Words below this confidence are dropped
const MIN_CONFIDENCE: f32 = 55.;
/// An image counts as having text from this many letters or digits on
const MIN_CHARS: usize = 4;

/// A recognised word in displayed image coordinates
#[derive(Debug, Clone)]
pub struct Word {
    pub text: String,
    /// Box of the word before it is turned by `angle` around its centre
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Clockwise tilt of the word in degrees, 0 for level text
    pub angle: f64,
    /// How sure Tesseract is, from 0 to 100
    pub confidence: f32,
    /// Index of the line the word belongs to
    pub line: usize,
    /// The word starts a new paragraph
    pub paragraph_start: bool,
}

/// Joins words into text with spaces and line breaks
pub fn join_words(words: &[Word]) -> String {
    let mut text = String::new();
    let mut last_line = None;
    for word in words {
        match last_line {
            Some(line) if line == word.line => text.push(' '),
            Some(_) if word.paragraph_start => text.push_str("\n\n"),
            Some(_) => text.push('\n'),
            None => {}
        }
        text.push_str(&word.text);
        last_line = Some(word.line);
    }
    text
}

/// Reads the text in `file` as it is displayed with `rotation` and `mirrored`
///
/// Returns an empty list when the image has no text, or when Tesseract or
/// its language data are missing.
pub async fn recognize(
    file: gio::File,
    rotation: f64,
    mirrored: bool,
    cancelled: Arc<AtomicBool>,
    cancellable: gio::Cancellable,
) -> anyhow::Result<Vec<Word>> {
    let mut loader = glycin::Loader::new(file);
    loader.cancellable(cancellable);
    // Orientation is applied below, the same way the viewer shows it
    loader
        .apply_transformations(false)
        .accepted_memory_formats(glycin::MemoryFormatSelection::R8g8b8a8);
    let mut image = loader.load().await?;
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Vec::new());
    }

    let details = image.details();
    if details.width() as u64 * details.height() as u64 > MAX_PIXELS {
        return Ok(Vec::new());
    }

    let frame = image.next_frame().await?;
    if frame.memory_format() != glycin::MemoryFormat::R8g8b8a8 {
        anyhow::bail!("Unexpected memory format {:?}", frame.memory_format());
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Vec::new());
    }
    let (width, height, stride) = (frame.width(), frame.height(), frame.stride() as usize);
    let buf = frame.buf_slice().to_vec();
    drop(frame);
    drop(image);

    let (reply, result) = async_channel::bounded(1);
    worker()
        .send(Job {
            buf,
            width,
            height,
            stride,
            rotation,
            mirrored,
            cancelled,
            reply,
        })
        .map_err(|_| anyhow::anyhow!("Text recognition thread stopped"))?;

    result
        .recv()
        .await
        .map_err(|_| anyhow::anyhow!("Text recognition thread stopped"))?
}

/// An image waiting to be read
struct Job {
    buf: Vec<u8>,
    width: u32,
    height: u32,
    stride: usize,
    rotation: f64,
    mirrored: bool,
    cancelled: Arc<AtomicBool>,
    reply: async_channel::Sender<anyhow::Result<Vec<Word>>>,
}

/// Sends jobs to the thread that reads images, one at a time
///
/// A thread of its own keeps the long running work off GIO's thread pool.
/// Jobs that were cancelled while waiting are skipped.
fn worker() -> &'static mpsc::Sender<Job> {
    static WORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("text-recognition".into())
            .spawn(move || {
                // Set up on the first image
                let mut engines = Engines::default();
                for job in receiver {
                    let result = if job.cancelled.load(Ordering::Relaxed) {
                        Ok(Vec::new())
                    } else {
                        read_job(&mut engines, &job)
                    };
                    let _ = job.reply.send_blocking(result);
                }
            })
            .expect("Failed to start text recognition thread");
        sender
    })
}

/// Tesseract instances that read in parallel, created when first needed
#[derive(Default)]
struct Engines {
    list: Vec<Engine>,
    failed: bool,
}

impl Engines {
    /// Up to `count` engines, or none when Tesseract can't be used
    ///
    /// Missing engines are set up side by side, as loading the language
    /// data takes a while.
    fn get(&mut self, count: usize) -> &mut [Engine] {
        if !self.failed && self.list.is_empty() {
            match Engine::new() {
                Some(engine) => self.list.push(engine),
                None => self.failed = true,
            }
        }
        if !self.failed && self.list.len() < count {
            let new: Vec<Option<Engine>> = std::thread::scope(|scope| {
                let handles = (self.list.len()..count)
                    .map(|_| scope.spawn(Engine::new))
                    .collect::<Vec<_>>();
                handles.into_iter().map(|x| x.join().ok().flatten()).collect()
            });
            self.failed = new.iter().any(Option::is_none);
            self.list.extend(new.into_iter().flatten());
        }
        let count = count.min(self.list.len());
        &mut self.list[..count]
    }
}

fn max_engines() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |x| x.get())
        .clamp(1, MAX_ENGINES)
}

/// One read of a version of the image
#[derive(Clone, Copy)]
enum Pass {
    /// The upright image at full size
    Upright,
    /// The half size image, looking for scattered large text
    Sparse,
    /// The upright image looking for scattered text, with thresholds that
    /// follow the local background, for light text on busy backgrounds
    Local,
    /// The smaller image turned clockwise by a quarter, three quarters or half
    Turn(u32),
    /// The smaller image tilted clockwise by degrees
    Tilt(f64),
}

/// Runs `passes` on as many engines as there are, returning the words of
/// each pass in the coordinates of the image it read
fn run(
    engines: &mut [Engine],
    passes: &[Pass],
    rgb: &RgbImage,
    small: &RgbImage,
    cancelled: &Arc<AtomicBool>,
) -> Vec<(Pass, Vec<Word>, (f64, f64))> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(passes.len()));

    std::thread::scope(|scope| {
        for engine in engines.iter_mut() {
            let (next, results) = (&next, &results);
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&pass) = passes.get(index) else {
                        break;
                    };
                    if cancelled.load(Ordering::Relaxed) {
                        break;
                    }

                    let image = match pass {
                        Pass::Upright | Pass::Local => std::borrow::Cow::Borrowed(rgb),
                        Pass::Sparse => std::borrow::Cow::Owned(image::imageops::resize(
                            rgb,
                            rgb.width() / 2,
                            rgb.height() / 2,
                            image::imageops::FilterType::Triangle,
                        )),
                        Pass::Turn(90) => std::borrow::Cow::Owned(image::imageops::rotate90(small)),
                        Pass::Turn(270) => {
                            std::borrow::Cow::Owned(image::imageops::rotate270(small))
                        }
                        Pass::Turn(_) => std::borrow::Cow::Owned(image::imageops::rotate180(small)),
                        Pass::Tilt(tilt) => std::borrow::Cow::Owned(tilt_gray(small, tilt)),
                    };
                    let words = match pass {
                        Pass::Sparse => engine.read_with(&image, cancelled, PSM_SPARSE_TEXT),
                        Pass::Local => {
                            engine.set_thresholding(THRESHOLD_SAUVOLA);
                            let words = engine.read_with(&image, cancelled, PSM_SPARSE_TEXT);
                            engine.set_thresholding(THRESHOLD_OTSU);
                            words
                        }
                        _ => engine.read_with(&image, cancelled, PSM_AUTO),
                    };
                    let size = (image.width() as f64, image.height() as f64);
                    results.lock().unwrap().push((index, pass, words, size));
                }
            });
        }
    });

    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|x| x.0);
    results
        .into_iter()
        .map(|(_, pass, words, size)| (pass, words, size))
        .collect()
}

fn read_job(engines: &mut Engines, job: &Job) -> anyhow::Result<Vec<Word>> {
    let rgb = to_rgb(&job.buf, job.width, job.height, job.stride)?;
    let (rgb, scale) = limit_size(rgb);
    let rgb = orient(rgb, job.rotation, job.mirrored);
    let (width, height) = (rgb.width() as f64, rgb.height() as f64);
    let (small, small_scale) = shrink(&rgb, MAX_TILT_SIDE);
    let small_size = (small.width() as f64, small.height() as f64);

    let engines = engines.get(max_engines());
    if engines.is_empty() {
        return Ok(Vec::new());
    }

    // Upright text first, together with a read of a half size copy that
    // finds large text, like signs and logos, often skipped at full size.
    // Sideways or upside down text, for example in a rotated photo, is read
    // from a smaller copy turned clockwise by a quarter, three quarters or
    // half, and tilted text, as in photos of signs, from it tilted back. A
    // confident read ends the search, otherwise the best read wins. Words
    // keep their tilt. Passes of a step run in parallel.
    let mut best: Option<(f32, Vec<Word>)> = None;
    let mut extra = Vec::new();
    let mut sure = false;

    let mut first = vec![Pass::Upright, Pass::Local];
    if rgb.width().min(rgb.height()) / 2 >= MIN_SPARSE_SIDE {
        first.push(Pass::Sparse);
    }
    let turns = [Pass::Turn(90), Pass::Turn(270), Pass::Turn(180)];
    let tilts = TILTS.map(Pass::Tilt);

    for step in [&first[..], &turns[..], &tilts[..]] {
        if sure {
            break;
        }
        for (pass, mut found, size) in run(engines, step, &rgb, &small, &job.cancelled) {
            if job.cancelled.load(Ordering::Relaxed) {
                return Ok(Vec::new());
            }

            match pass {
                Pass::Sparse | Pass::Local => {
                    let (fx, fy) = (width / size.0, height / size.1);
                    for word in &mut found {
                        word.x *= fx;
                        word.y *= fy;
                        word.width *= fx;
                        word.height *= fy;
                    }
                    extra.push(found);
                    continue;
                }
                Pass::Upright => {}
                Pass::Turn(turn) => {
                    for word in &mut found {
                        turn_back(word, turn, small_size.0, small_size.1);
                        unscale(word, small_scale);
                    }
                }
                Pass::Tilt(tilt) => {
                    let from = (size.0 / 2., size.1 / 2.);
                    let to = (small_size.0 / 2., small_size.1 / 2.);
                    for word in &mut found {
                        tilt_back(word, tilt, from, to);
                        unscale(word, small_scale);
                    }
                }
            }

            if count_chars(&found) < MIN_CHARS {
                continue;
            }
            let (score, is_sure) = rate(&found);
            // Only a level read can end the search before the tilts
            sure |= is_sure && !matches!(pass, Pass::Tilt(_));
            if best.as_ref().is_none_or(|(most, _)| score > *most) {
                best = Some((score, found));
            }
        }
    }
    if job.cancelled.load(Ordering::Relaxed) {
        return Ok(Vec::new());
    }

    let mut words = best.map(|(_, words)| words).unwrap_or_default();
    for extra in extra {
        merge(&mut words, extra);
    }
    if count_chars(&words) < MIN_CHARS {
        words.clear();
    }
    for word in &mut words {
        unscale(word, scale);
    }

    Ok(words)
}

/// Divides the box of `word` by `scale`
fn unscale(word: &mut Word, scale: f64) {
    word.x /= scale;
    word.y /= scale;
    word.width /= scale;
    word.height /= scale;
}

/// Turns `image` clockwise by `degrees` onto a larger white canvas
fn tilt_gray(rgb: &RgbImage, degrees: f64) -> RgbImage {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let (w, h) = (rgb.width() as f64, rgb.height() as f64);
    let out_w = (w * cos.abs() + h * sin.abs()).ceil();
    let out_h = (w * sin.abs() + h * cos.abs()).ceil();

    RgbImage::from_fn(out_w as u32, out_h as u32, |x, y| {
        // The source pixel, by turning back counter-clockwise
        let dx = x as f64 + 0.5 - out_w / 2.;
        let dy = y as f64 + 0.5 - out_h / 2.;
        let sx = dx * cos + dy * sin + w / 2.;
        let sy = -dx * sin + dy * cos + h / 2.;
        // Smooth edges read better than jagged ones
        let (sx, sy) = (sx - 0.5, sy - 0.5);
        let (x0, y0) = (sx.floor(), sy.floor());
        let (fx, fy) = (sx - x0, sy - y0);
        let pixel = |x: f64, y: f64, c: usize| {
            if x >= 0. && y >= 0. && x < w && y < h {
                rgb.get_pixel(x as u32, y as u32).0[c] as f64
            } else {
                255.
            }
        };
        Rgb(std::array::from_fn(|c| {
            let top = pixel(x0, y0, c) * (1. - fx) + pixel(x0 + 1., y0, c) * fx;
            let bottom = pixel(x0, y0 + 1., c) * (1. - fx) + pixel(x0 + 1., y0 + 1., c) * fx;
            (top * (1. - fy) + bottom * fy).round() as u8
        }))
    })
}

/// Moves a word read from an image tilted clockwise by `degrees` back onto
/// the untilted image, where `from` and `to` are the two image centres
fn tilt_back(word: &mut Word, degrees: f64, from: (f64, f64), to: (f64, f64)) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let dx = word.x + word.width / 2. - from.0;
    let dy = word.y + word.height / 2. - from.1;
    let cx = dx * cos + dy * sin + to.0;
    let cy = -dx * sin + dy * cos + to.1;
    word.x = cx - word.width / 2.;
    word.y = cy - word.height / 2.;
    word.angle = -degrees;
}

/// A copy with the long side at most `max_side`, and the factor used
fn shrink(rgb: &RgbImage, max_side: u32) -> (RgbImage, f64) {
    let long_side = rgb.width().max(rgb.height());
    if long_side <= max_side {
        return (rgb.clone(), 1.);
    }
    let scale = max_side as f64 / long_side as f64;
    let width = ((rgb.width() as f64 * scale).round() as u32).max(1);
    let height = ((rgb.height() as f64 * scale).round() as u32).max(1);
    let small = image::imageops::resize(rgb, width, height, image::imageops::FilterType::Triangle);
    (small, scale)
}

/// Letters weighted by confidence, and whether the read is clearly right
fn rate(words: &[Word]) -> (f32, bool) {
    let chars = count_chars(words) as f32;
    let score = words
        .iter()
        .map(|w| w.text.chars().filter(|c| c.is_alphanumeric()).count() as f32 * w.confidence)
        .sum::<f32>()
        / 100.;
    let mean = if chars > 0. { score * 100. / chars } else { 0. };
    (score, mean >= SURE_CONFIDENCE)
}

/// Adds the words of `extra` that are not already in `words`
fn merge(words: &mut Vec<Word>, extra: Vec<Word>) {
    let first_line = words.iter().map(|w| w.line + 1).max().unwrap_or(0);
    let bounds = |w: &Word| {
        let (sin, cos) = w.angle.to_radians().sin_cos();
        let half_w = (w.width * cos.abs() + w.height * sin.abs()) / 2.;
        let half_h = (w.width * sin.abs() + w.height * cos.abs()) / 2.;
        let (cx, cy) = (w.x + w.width / 2., w.y + w.height / 2.);
        (cx - half_w, cy - half_h, cx + half_w, cy + half_h)
    };
    let overlaps = |a: &Word, b: &Word| {
        let (a, b) = (bounds(a), bounds(b));
        let w = a.2.min(b.2) - a.0.max(b.0);
        let h = a.3.min(b.3) - a.1.max(b.1);
        let smaller = ((a.2 - a.0) * (a.3 - a.1)).min((b.2 - b.0) * (b.3 - b.1));
        w > 0. && h > 0. && w * h > smaller * 0.3
    };

    let new: Vec<Word> = extra
        .into_iter()
        .filter(|e| {
            e.confidence >= EXTRA_CONFIDENCE
                && count_chars(std::slice::from_ref(e)) >= 3
                && !words.iter().any(|w| overlaps(w, e))
        })
        .collect();
    words.extend(new.into_iter().map(|mut word| {
        word.line += first_line;
        word
    }));
}

fn count_chars(words: &[Word]) -> usize {
    words
        .iter()
        .map(|w| w.text.chars().filter(|c| c.is_alphanumeric()).count())
        .sum()
}

/// Moves a box read from the image turned clockwise by `turn` degrees back
/// onto the unturned image of `width` × `height`
fn turn_back(word: &mut Word, turn: u32, width: f64, height: f64) {
    let (x, y, w, h) = (word.x, word.y, word.width, word.height);
    (word.x, word.y, word.width, word.height) = match turn {
        90 => (y, height - x - w, h, w),
        270 => (width - y - h, x, h, w),
        180 => (width - x - w, height - y - h, w, h),
        _ => (x, y, w, h),
    };
}

/// Converts strided RGBA to RGB, putting transparent parts on white
///
/// Tesseract finds more text in colour than in grey, as it picks a
/// threshold for each channel.
fn to_rgb(buf: &[u8], width: u32, height: u32, stride: usize) -> anyhow::Result<RgbImage> {
    let row_len = width as usize * 4;
    if height > 0 && buf.len() < (height as usize - 1) * stride + row_len {
        anyhow::bail!("Frame buffer too small");
    }

    let mut out = Vec::with_capacity(width as usize * height as usize * 3);
    for row in buf.chunks(stride).take(height as usize) {
        for pixel in row[..row_len].chunks_exact(4) {
            let a = pixel[3] as u32;
            for &c in &pixel[..3] {
                out.push(((c as u32 * a + 255 * (255 - a)) / 255) as u8);
            }
        }
    }
    RgbImage::from_raw(width, height, out).ok_or_else(|| anyhow::anyhow!("Frame buffer too small"))
}

/// Applies the viewer's mirroring and counter-clockwise rotation
fn orient(rgb: RgbImage, rotation: f64, mirrored: bool) -> RgbImage {
    let rgb = if mirrored {
        image::imageops::flip_horizontal(&rgb)
    } else {
        rgb
    };

    match (rotation.round() as i64).rem_euclid(360) {
        90 => image::imageops::rotate270(&rgb),
        180 => image::imageops::rotate180(&rgb),
        270 => image::imageops::rotate90(&rgb),
        _ => rgb,
    }
}

/// Scales very large images down and returns the factor
fn limit_size(rgb: RgbImage) -> (RgbImage, f64) {
    let long_side = rgb.width().max(rgb.height());
    if long_side <= MAX_SIDE {
        return (rgb, 1.);
    }

    let scale = MAX_SIDE as f64 / long_side as f64;
    let width = ((rgb.width() as f64 * scale).round() as u32).max(1);
    let height = ((rgb.height() as f64 * scale).round() as u32).max(1);
    let rgb = image::imageops::resize(&rgb, width, height, image::imageops::FilterType::Triangle);
    (rgb, scale)
}

// Tesseract C API

#[repr(C)]
struct TessBaseAPI {
    _private: [u8; 0],
}
#[repr(C)]
struct TessResultIterator {
    _private: [u8; 0],
}
#[repr(C)]
struct TessPageIterator {
    _private: [u8; 0],
}
#[repr(C)]
struct EtextDesc {
    _private: [u8; 0],
}

const RIL_PARA: c_int = 1;
const RIL_TEXTLINE: c_int = 2;
const RIL_WORD: c_int = 3;
const OEM_LSTM_ONLY: c_int = 1;
const PSM_AUTO: c_int = 3;
const PSM_SPARSE_TEXT: c_int = 11;
const THRESHOLD_OTSU: &CStr = c"0";
const THRESHOLD_SAUVOLA: &CStr = c"2";

type CancelFunc = unsafe extern "C" fn(cancel_this: *mut c_void, words: c_int) -> bool;

unsafe extern "C" {
    fn TessBaseAPICreate() -> *mut TessBaseAPI;
    fn TessBaseAPIDelete(handle: *mut TessBaseAPI);
    fn TessBaseAPIInit2(
        handle: *mut TessBaseAPI,
        datapath: *const c_char,
        language: *const c_char,
        oem: c_int,
    ) -> c_int;
    fn TessBaseAPIEnd(handle: *mut TessBaseAPI);
    fn TessBaseAPISetPageSegMode(handle: *mut TessBaseAPI, mode: c_int);
    fn TessBaseAPISetVariable(
        handle: *mut TessBaseAPI,
        name: *const c_char,
        value: *const c_char,
    ) -> c_int;
    fn TessBaseAPIGetAvailableLanguagesAsVector(handle: *const TessBaseAPI) -> *mut *mut c_char;
    fn TessDeleteTextArray(arr: *mut *mut c_char);
    fn TessDeleteText(text: *const c_char);
    fn TessBaseAPISetImage(
        handle: *mut TessBaseAPI,
        imagedata: *const u8,
        width: c_int,
        height: c_int,
        bytes_per_pixel: c_int,
        bytes_per_line: c_int,
    );
    fn TessBaseAPIRecognize(handle: *mut TessBaseAPI, monitor: *mut EtextDesc) -> c_int;
    fn TessBaseAPIClear(handle: *mut TessBaseAPI);
    fn TessBaseAPIGetIterator(handle: *mut TessBaseAPI) -> *mut TessResultIterator;
    fn TessResultIteratorDelete(handle: *mut TessResultIterator);
    fn TessResultIteratorGetPageIterator(handle: *mut TessResultIterator) -> *mut TessPageIterator;
    fn TessResultIteratorGetUTF8Text(
        handle: *const TessResultIterator,
        level: c_int,
    ) -> *mut c_char;
    fn TessResultIteratorConfidence(handle: *const TessResultIterator, level: c_int) -> f32;
    fn TessPageIteratorNext(handle: *mut TessPageIterator, level: c_int) -> c_int;
    fn TessPageIteratorIsAtBeginningOf(handle: *const TessPageIterator, level: c_int) -> c_int;
    fn TessPageIteratorBoundingBox(
        handle: *const TessPageIterator,
        level: c_int,
        left: *mut c_int,
        top: *mut c_int,
        right: *mut c_int,
        bottom: *mut c_int,
    ) -> c_int;
    fn TessMonitorCreate() -> *mut EtextDesc;
    fn TessMonitorDelete(monitor: *mut EtextDesc);
    fn TessMonitorSetCancelFunc(monitor: *mut EtextDesc, func: CancelFunc);
    fn TessMonitorSetCancelThis(monitor: *mut EtextDesc, cancel_this: *mut c_void);
}

/// An initialised Tesseract instance, kept for later images
///
/// Each instance is used by one thread at a time.
struct Engine(*mut TessBaseAPI);

// SAFETY: Tesseract instances share no state and may move between threads
unsafe impl Send for Engine {}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            TessBaseAPIEnd(self.0);
            TessBaseAPIDelete(self.0);
        }
    }
}

impl Engine {
    fn new() -> Option<Self> {
        let datapath = data_path();
        let datapath_ptr = datapath.as_ref().map_or(std::ptr::null(), |x| x.as_ptr());
        let languages = languages(datapath_ptr)?;

        unsafe {
            let handle = TessBaseAPICreate();
            if handle.is_null() {
                return None;
            }
            let engine = Self(handle);

            if TessBaseAPIInit2(handle, datapath_ptr, languages.as_ptr(), OEM_LSTM_ONLY) != 0 {
                tracing::warn!("Text recognition is off: language data failed to load");
                return None;
            }

            Some(engine)
        }
    }

    /// Picks how Tesseract separates text from background
    fn set_thresholding(&mut self, method: &CStr) {
        unsafe {
            TessBaseAPISetVariable(self.0, c"thresholding_method".as_ptr(), method.as_ptr());
        }
    }

    /// Returns the words found in `rgb` with a page segmentation mode,
    /// or none when cancelled
    fn read_with(&self, rgb: &RgbImage, cancelled: &Arc<AtomicBool>, mode: c_int) -> Vec<Word> {
        let mut words = Vec::new();

        unsafe {
            let handle = self.0;
            TessBaseAPISetPageSegMode(handle, mode);

            TessBaseAPISetImage(
                handle,
                rgb.as_raw().as_ptr(),
                rgb.width() as c_int,
                rgb.height() as c_int,
                3,
                rgb.width() as c_int * 3,
            );

            let monitor = TessMonitorCreate();
            TessMonitorSetCancelFunc(monitor, is_cancelled);
            TessMonitorSetCancelThis(monitor, Arc::as_ptr(cancelled) as *mut c_void);
            let result = TessBaseAPIRecognize(handle, monitor);
            TessMonitorDelete(monitor);

            if result != 0 || cancelled.load(Ordering::Relaxed) {
                TessBaseAPIClear(handle);
                return Vec::new();
            }

            let iter = TessBaseAPIGetIterator(handle);
            if !iter.is_null() {
                let page = TessResultIteratorGetPageIterator(iter);
                let mut line = 0;
                let mut first = true;
                loop {
                    let new_line = TessPageIteratorIsAtBeginningOf(page, RIL_TEXTLINE) != 0;
                    let new_paragraph = TessPageIteratorIsAtBeginningOf(page, RIL_PARA) != 0;
                    if new_line && !first {
                        line += 1;
                    }
                    first = false;

                    let text = TessResultIteratorGetUTF8Text(iter, RIL_WORD);
                    if !text.is_null() {
                        let word = CStr::from_ptr(text).to_string_lossy().trim().to_string();
                        TessDeleteText(text);

                        let confidence = TessResultIteratorConfidence(iter, RIL_WORD);
                        let (mut left, mut top, mut right, mut bottom) = (0, 0, 0, 0);
                        let has_box = TessPageIteratorBoundingBox(
                            page,
                            RIL_WORD,
                            &mut left,
                            &mut top,
                            &mut right,
                            &mut bottom,
                        ) != 0;

                        // Lone symbols are mostly noise from icons and edges
                        let has_text = word.chars().any(char::is_alphanumeric);
                        if has_box && has_text && confidence >= MIN_CONFIDENCE {
                            let paragraph_start = new_paragraph
                                && words.last().is_some_and(|w: &Word| w.line != line);
                            words.push(Word {
                                text: word,
                                x: left as f64,
                                y: top as f64,
                                width: (right - left) as f64,
                                height: (bottom - top) as f64,
                                angle: 0.,
                                confidence,
                                line,
                                paragraph_start,
                            });
                        }
                    }

                    if TessPageIteratorNext(page, RIL_WORD) == 0 {
                        break;
                    }
                }
                TessResultIteratorDelete(iter);
            }

            TessBaseAPIClear(handle);
        }

        words
    }
}

/// The languages to read, like `deu+eng`, worked out once
///
/// Each extra language slows reading down a lot, so only the device's
/// language is read, with English. The first language reads best, so the
/// device's language goes first.
fn languages(datapath: *const c_char) -> Option<&'static CStr> {
    static LANGUAGES: OnceLock<Option<CString>> = OnceLock::new();
    LANGUAGES
        .get_or_init(|| unsafe {
            let handle = TessBaseAPICreate();
            if handle.is_null() {
                return None;
            }
            // English first, to ask which other languages are installed
            let engine = Engine(handle);
            if TessBaseAPIInit2(handle, datapath, c"eng".as_ptr(), OEM_LSTM_ONLY) != 0 {
                tracing::warn!("Text recognition is off: no Tesseract language data found");
                return None;
            }

            let mut installed = Vec::new();
            let list = TessBaseAPIGetAvailableLanguagesAsVector(handle);
            if !list.is_null() {
                let mut item = list;
                while !(*item).is_null() {
                    installed.push(CStr::from_ptr(*item).to_string_lossy().into_owned());
                    item = item.add(1);
                }
                TessDeleteTextArray(list);
            }
            // Tesseract's notes about resolution and empty pages are not
            // useful here. The setting is shared by all instances.
            TessBaseAPISetVariable(handle, c"debug_file".as_ptr(), c"/dev/null".as_ptr());
            drop(engine);

            let device = glib::language_names()
                .first()
                .map(|x| {
                    x.split(['_', '.', '@'])
                        .next()
                        .unwrap_or_default()
                        .to_string()
                })
                .unwrap_or_default();
            let mut languages = LANGUAGES_LIST
                .iter()
                .filter(|(lang, locale)| {
                    (*locale == device || *lang == "eng") && installed.iter().any(|x| x == lang)
                })
                .collect::<Vec<_>>();
            languages.sort_by_key(|(_, locale)| *locale != device);
            let languages = languages
                .iter()
                .map(|(lang, _)| *lang)
                .collect::<Vec<_>>()
                .join("+");
            tracing::debug!("Text recognition languages: {languages}");
            CString::new(languages).ok()
        })
        .as_deref()
}

/// Language data installed next to the app, as in the Flatpak
///
/// `None` leaves the choice to Tesseract, which reads `TESSDATA_PREFIX` or
/// uses the folder it was built with.
fn data_path() -> Option<CString> {
    if std::env::var_os("TESSDATA_PREFIX").is_some() {
        return None;
    }

    let path = std::path::Path::new(crate::config::LOCALEDIR)
        .parent()?
        .join("tessdata");
    path.is_dir()
        .then(|| CString::new(path.into_os_string().into_encoded_bytes()).ok())
        .flatten()
}

unsafe extern "C" fn is_cancelled(cancel_this: *mut c_void, _words: c_int) -> bool {
    unsafe { (*(cancel_this as *const AtomicBool)).load(Ordering::Relaxed) }
}

