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
/// Small tilts tried for slightly slanted or curved lines, as in photos
/// of screens and pages, in degrees clockwise
const LEANS: [f64; 2] = [3., -3.];
/// Lines sloping more than this, in degrees, are read again with `LEANS`
const MAX_SLOPE: f64 = 1.;
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
    /// The upright image tilted clockwise by a few degrees
    Lean(f64),
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
                        Pass::Lean(tilt) => std::borrow::Cow::Owned(tilt_gray(rgb, tilt)),
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
    //
    // Slightly slanted or curved lines, as in photos of screens, lose
    // words in the upright read, so when its lines slope it is read again
    // tilted a little both ways, and the words the best of these reads
    // missed are taken from the others.
    let mut reads: Vec<(f32, Vec<Word>, bool)> = Vec::new();
    let mut extra = Vec::new();
    let mut sure = false;
    let mut sloped = false;

    let mut first = vec![Pass::Upright, Pass::Local];
    if rgb.width().min(rgb.height()) / 2 >= MIN_SPARSE_SIDE {
        first.push(Pass::Sparse);
    }
    let leans = LEANS.map(Pass::Lean);
    let turns = [Pass::Turn(90), Pass::Turn(270), Pass::Turn(180)];
    let tilts = TILTS.map(Pass::Tilt);

    for step in [&first[..], &leans[..], &turns[..], &tilts[..]] {
        let is_lean = matches!(step[0], Pass::Lean(_));
        if (is_lean && !sloped) || (!is_lean && sure) {
            continue;
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
                Pass::Upright => sloped = slopes(&found),
                Pass::Lean(tilt) => {
                    let from = (size.0 / 2., size.1 / 2.);
                    for word in &mut found {
                        tilt_back(word, tilt, from, (width / 2., height / 2.));
                    }
                }
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
            reads.push((score, found, matches!(pass, Pass::Upright | Pass::Lean(_))));
        }
    }
    if job.cancelled.load(Ordering::Relaxed) {
        return Ok(Vec::new());
    }

    let best = (0..reads.len()).max_by(|&a, &b| reads[a].0.total_cmp(&reads[b].0));
    let (mut words, level) = best.map_or((Vec::new(), false), |x| {
        let (_, words, level) = reads.swap_remove(x);
        (words, level)
    });
    // Other level reads only fill gaps in a level read
    let others = reads
        .into_iter()
        .filter(|x| level && x.2)
        .map(|x| x.1);
    for extra in others.chain(extra) {
        merge(&mut words, extra);
    }
    join_lines(&mut words);
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
        // Sharp, smooth edges read best, so pixels are mixed bicubically.
        // Bilinear mixing blurs small text and loses many words.
        let (sx, sy) = (sx - 0.5, sy - 0.5);
        let (x0, y0) = (sx.floor(), sy.floor());
        let (wx, wy) = (cubic_weights(sx - x0), cubic_weights(sy - y0));
        let pixel = |x: f64, y: f64, c: usize| {
            if x >= 0. && y >= 0. && x < w && y < h {
                rgb.get_pixel(x as u32, y as u32).0[c] as f64
            } else {
                255.
            }
        };
        Rgb(std::array::from_fn(|c| {
            let mut sum = 0.;
            for (j, wy) in wy.iter().enumerate() {
                let y = y0 + j as f64 - 1.;
                for (i, wx) in wx.iter().enumerate() {
                    sum += pixel(x0 + i as f64 - 1., y, c) * wx * wy;
                }
            }
            sum.round().clamp(0., 255.) as u8
        }))
    })
}

/// Catmull-Rom weights of the four pixels around a point `t` past the second
fn cubic_weights(t: f64) -> [f64; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        (-t3 + 2. * t2 - t) / 2.,
        (3. * t3 - 5. * t2 + 2.) / 2.,
        (-3. * t3 + 4. * t2 + t) / 2.,
        (t3 - t2) / 2.,
    ]
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

/// Whether a long line of `words` slopes more than `MAX_SLOPE`
fn slopes(words: &[Word]) -> bool {
    words.chunk_by(|a, b| a.line == b.line).any(|line| {
        if line.len() < 4 {
            return false;
        }
        let points = line
            .iter()
            .map(|w| (w.x + w.width / 2., w.y + w.height / 2.))
            .collect::<Vec<_>>();
        let n = points.len() as f64;
        let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
        let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;
        let (mut xy, mut xx) = (0., 0.);
        for (x, y) in &points {
            xy += (x - mean_x) * (y - mean_y);
            xx += (x - mean_x).powi(2);
        }
        // Short lines slope by chance
        let height = line.iter().map(|w| w.height).sum::<f64>() / n;
        xx > 0. && xx.sqrt() > height * 8. && (xy / xx).atan().to_degrees().abs() > MAX_SLOPE
    })
}

/// Adds the words of `extra` that are not already in `words`
///
/// A longer, confident word replaces the shorter pieces it covers, as
/// parts of slanted lines are often read as fragments. Added words go
/// next to their neighbours on the same line, so copied text keeps its order.
fn merge(words: &mut Vec<Word>, extra: Vec<Word>) {
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

    for word in extra {
        let chars = count_chars(std::slice::from_ref(&word));
        if word.confidence < EXTRA_CONFIDENCE || chars < 3 {
            continue;
        }
        let covered = (0..words.len())
            .filter(|&i| overlaps(&words[i], &word))
            .collect::<Vec<_>>();
        if !covered.is_empty() {
            let pieces = covered.iter().map(|&i| &words[i]).cloned().collect::<Vec<_>>();
            let lowest = pieces.iter().map(|w| w.confidence).fold(f32::MAX, f32::min);
            if chars <= count_chars(&pieces) || word.confidence + 10. < lowest {
                continue;
            }
            // Takes the place of the first piece
            let mut word = word;
            word.line = pieces[0].line;
            word.paragraph_start = pieces[0].paragraph_start;
            let at = covered[0];
            for &i in covered.iter().rev() {
                words.remove(i);
            }
            words.insert(at, word);
            continue;
        }
        insert_in_line(words, word);
    }
}

/// Inserts `word` after the nearest word to its left on the same line,
/// or before the nearest to its right, or else on a new last line
fn insert_in_line(words: &mut Vec<Word>, mut word: Word) {
    let centre = |w: &Word| (w.x + w.width / 2., w.y + w.height / 2.);
    let (x, y) = centre(&word);
    let same_line = |w: &Word| (centre(w).1 - y).abs() < w.height.max(word.height) / 2.;

    let left = (0..words.len())
        .filter(|&i| same_line(&words[i]) && centre(&words[i]).0 < x)
        .max_by(|&a, &b| centre(&words[a]).0.total_cmp(&centre(&words[b]).0));
    let right = (0..words.len())
        .filter(|&i| same_line(&words[i]) && centre(&words[i]).0 >= x)
        .min_by(|&a, &b| centre(&words[a]).0.total_cmp(&centre(&words[b]).0));

    if let Some(i) = left {
        word.line = words[i].line;
        word.paragraph_start = false;
        words.insert(i + 1, word);
    } else if let Some(i) = right {
        word.line = words[i].line;
        word.paragraph_start = words[i].paragraph_start;
        words[i].paragraph_start = false;
        words.insert(i, word);
    } else {
        word.line = words.iter().map(|w| w.line + 1).max().unwrap_or(0);
        words.push(word);
    }
}

/// Joins line pieces that continue each other on the same line
///
/// Reads of slanted lines often split a line into pieces, which then copy
/// as separate lines. Pieces far apart, as in columns, stay apart.
fn join_lines(words: &mut Vec<Word>) {
    let mut pieces: Vec<Vec<Word>> = Vec::new();
    for word in words.drain(..) {
        match pieces.iter_mut().find(|x| x[0].line == word.line) {
            Some(piece) => piece.push(word),
            None => pieces.push(vec![word]),
        }
    }

    let centre_y = |w: &Word| w.y + w.height / 2.;
    let continues = |a: &[Word], b: &[Word]| {
        let (last, first) = (&a[a.len() - 1], &b[0]);
        let height = last.height.max(first.height);
        let gap = first.x - (last.x + last.width);
        gap > -height / 2. && gap < height * 3. && (centre_y(first) - centre_y(last)).abs() < height / 2.
    };
    let mut next: Vec<Option<usize>> = vec![None; pieces.len()];
    let mut has_previous = vec![false; pieces.len()];
    for a in 0..pieces.len() {
        let closest = (0..pieces.len())
            .filter(|&b| b != a && !has_previous[b] && continues(&pieces[a], &pieces[b]))
            .min_by(|&x, &y| pieces[x][0].x.total_cmp(&pieces[y][0].x));
        if let Some(b) = closest {
            next[a] = Some(b);
            has_previous[b] = true;
        }
    }

    let mut taken = vec![false; pieces.len()];
    let mut line = 0;
    // The first word of the last line, as slanted lines drift up or down
    let mut line_start = None;
    // The distance between the last two lines
    let mut pitch: Option<f64> = None;
    for start in 0..pieces.len() {
        if has_previous[start] || taken[start] {
            continue;
        }
        let mut at = Some(start);
        let mut first = true;
        // Joined pieces were often paragraphs of their own, so only a gap
        // clearly wider than the one between the last lines starts one
        let above = line_start.map(|i: usize| (centre_y(&words[i]), words[i].height));
        line_start = Some(words.len());
        while let Some(i) = at.filter(|&i| !taken[i]) {
            taken[i] = true;
            for mut word in std::mem::take(&mut pieces[i]) {
                if first && let Some((y, height)) = above {
                    let gap = centre_y(&word) - y;
                    let usual = pitch.unwrap_or(height.max(word.height) * 2.5);
                    word.paragraph_start &= gap > usual * 1.5;
                    pitch = Some(gap);
                }
                word.paragraph_start &= first;
                first = false;
                word.line = line;
                words.push(word);
            }
            at = next[i];
        }
        line += 1;
    }
    // Pieces left in a loop
    for piece in pieces.into_iter().filter(|x| !x.is_empty()) {
        for mut word in piece {
            word.line = line;
            words.push(word);
        }
        line += 1;
    }
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



