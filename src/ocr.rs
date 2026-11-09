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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};

use image::{GrayImage, Luma};

use crate::deps::*;

/// Languages that are read when their data is installed, with their locale codes
const LANGUAGES: &[(&str, &str)] = &[
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
/// Words below this confidence are dropped
const MIN_CONFIDENCE: f32 = 55.;
/// An image counts as having text from this many letters or digits on
const MIN_CHARS: usize = 4;

/// A recognised word in displayed image coordinates
#[derive(Debug, Clone)]
pub struct Word {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
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
                let mut engine: Option<Option<Engine>> = None;
                for job in receiver {
                    let result = if job.cancelled.load(Ordering::Relaxed) {
                        Ok(Vec::new())
                    } else {
                        let engine = engine.get_or_insert_with(Engine::new);
                        read_job(engine.as_ref(), &job)
                    };
                    let _ = job.reply.send_blocking(result);
                }
            })
            .expect("Failed to start text recognition thread");
        sender
    })
}

fn read_job(engine: Option<&Engine>, job: &Job) -> anyhow::Result<Vec<Word>> {
    let Some(engine) = engine else {
        return Ok(Vec::new());
    };

    let gray = to_gray(&job.buf, job.width, job.height, job.stride)?;
    let gray = orient(gray, job.rotation, job.mirrored);
    let (gray, scale) = limit_size(gray);

    let mut words = engine.read(&gray, &job.cancelled);
    for word in &mut words {
        word.x /= scale;
        word.y /= scale;
        word.width /= scale;
        word.height /= scale;
    }

    let chars: usize = words
        .iter()
        .map(|w| w.text.chars().filter(|c| c.is_alphanumeric()).count())
        .sum();
    if chars < MIN_CHARS {
        words.clear();
    }

    Ok(words)
}

/// Converts strided RGBA to grey, putting transparent parts on white
fn to_gray(buf: &[u8], width: u32, height: u32, stride: usize) -> anyhow::Result<GrayImage> {
    let row_len = width as usize * 4;
    if height > 0 && buf.len() < (height as usize - 1) * stride + row_len {
        anyhow::bail!("Frame buffer too small");
    }

    Ok(GrayImage::from_fn(width, height, |x, y| {
        let offset = y as usize * stride + x as usize * 4;
        let [r, g, b, a] = [
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ];
        let luma = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
        let a = a as u32;
        Luma([((luma * a + 255 * (255 - a)) / 255) as u8])
    }))
}

/// Applies the viewer's mirroring and counter-clockwise rotation
fn orient(gray: GrayImage, rotation: f64, mirrored: bool) -> GrayImage {
    let gray = if mirrored {
        image::imageops::flip_horizontal(&gray)
    } else {
        gray
    };

    match (rotation.round() as i64).rem_euclid(360) {
        90 => image::imageops::rotate270(&gray),
        180 => image::imageops::rotate180(&gray),
        270 => image::imageops::rotate90(&gray),
        _ => gray,
    }
}

/// Scales very large images down and returns the factor
fn limit_size(gray: GrayImage) -> (GrayImage, f64) {
    let long_side = gray.width().max(gray.height());
    if long_side <= MAX_SIDE {
        return (gray, 1.);
    }

    let scale = MAX_SIDE as f64 / long_side as f64;
    let width = ((gray.width() as f64 * scale).round() as u32).max(1);
    let height = ((gray.height() as f64 * scale).round() as u32).max(1);
    let gray = image::imageops::resize(&gray, width, height, image::imageops::FilterType::Triangle);
    (gray, scale)
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
/// Lives on the worker thread only.
struct Engine(*mut TessBaseAPI);

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

        unsafe {
            let handle = TessBaseAPICreate();
            if handle.is_null() {
                return None;
            }
            let engine = Self(handle);

            // English first, to ask which other languages are installed
            let eng = CString::new("eng").unwrap();
            if TessBaseAPIInit2(handle, datapath_ptr, eng.as_ptr(), OEM_LSTM_ONLY) != 0 {
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

            // The first language reads best, so the device's language goes first
            let device = glib::language_names()
                .first()
                .map(|x| {
                    x.split(['_', '.', '@'])
                        .next()
                        .unwrap_or_default()
                        .to_string()
                })
                .unwrap_or_default();
            let mut languages = LANGUAGES
                .iter()
                .filter(|(lang, _)| installed.iter().any(|x| x == lang))
                .collect::<Vec<_>>();
            languages.sort_by_key(|(_, locale)| *locale != device);
            let languages = languages
                .iter()
                .map(|(lang, _)| *lang)
                .collect::<Vec<_>>()
                .join("+");
            tracing::debug!("Text recognition languages: {languages}");

            if !languages.is_empty() && languages != "eng" {
                let languages = CString::new(languages).unwrap();
                TessBaseAPIEnd(handle);
                if TessBaseAPIInit2(handle, datapath_ptr, languages.as_ptr(), OEM_LSTM_ONLY) != 0 {
                    tracing::warn!("Text recognition is off: language data failed to load");
                    return None;
                }
            }

            // Tesseract's notes about resolution and empty pages are not useful here
            let name = CString::new("debug_file").unwrap();
            let value = CString::new("/dev/null").unwrap();
            TessBaseAPISetVariable(handle, name.as_ptr(), value.as_ptr());

            Some(engine)
        }
    }

    /// Returns the words found in `gray`, or none when cancelled
    fn read(&self, gray: &GrayImage, cancelled: &Arc<AtomicBool>) -> Vec<Word> {
        let mut words = Vec::new();

        unsafe {
            let handle = self.0;

            TessBaseAPISetImage(
                handle,
                gray.as_raw().as_ptr(),
                gray.width() as c_int,
                gray.height() as c_int,
                1,
                gray.width() as c_int,
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

                        if has_box && !word.is_empty() && confidence >= MIN_CONFIDENCE {
                            let paragraph_start = new_paragraph
                                && words.last().is_some_and(|w: &Word| w.line != line);
                            words.push(Word {
                                text: word,
                                x: left as f64,
                                y: top as f64,
                                width: (right - left) as f64,
                                height: (bottom - top) as f64,
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
