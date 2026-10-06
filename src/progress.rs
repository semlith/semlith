//! How far an index run is, and how long it has left: one account, used by the
//! CLI, the daemon and Semlith Cloud alike.
//!
//! The walk of 0.37.0-rc.4 found every figure on the run card derived from
//! the wrong thing. Time left was bytes over a byte rate, and a PNG moves
//! megabytes for no chunks while a log moves a kilobyte per chunk, so it read
//! 22 h, then 5 h, then 1 h 34 within six percent. The bar's denominator was the
//! rows written so far, which one large file resets. The stages were guessed
//! from the file count. Here the unit is the work itself: chunks to embed,
//! images to embed, files to read, each counted against an expected total that
//! converges as files are chunked.

use std::collections::VecDeque;
use std::path::Path;

use serde::Serialize;

/// What a run is doing now. A phase is an event, not a guess: the engine says
/// when it enters one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// A person's decisions on refused files, applied before the files.
    Decisions,
    /// Finding the files under the roots.
    Walk,
    /// Reading, hashing, parsing and chunking, before anything is embedded.
    #[default]
    Read,
    /// Waiting for an embedding lane to load or compile.
    Lane,
    /// Embedding, with reading going on beside it.
    Embed,
    /// Loading the image model.
    Images,
    /// Waiting for the windows still with the lanes.
    Drain,
    /// Writing the index to disk.
    Save,
    /// Sweeping files gone from disk and recording the run.
    Finalize,
    /// Taking a stopped run's work back out.
    Undo,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decisions => "decisions",
            Self::Walk => "walk",
            Self::Read => "read",
            Self::Lane => "lane",
            Self::Embed => "embed",
            Self::Images => "images",
            Self::Drain => "drain",
            Self::Save => "save",
            Self::Finalize => "finalize",
            Self::Undo => "undo",
        }
    }
}

/// What an image weighs against a chunk until the run has measured both: the
/// image model's time per image over the text model's time per chunk on the
/// owner's M1 (CLIP about 9 images/s on the CPU, granite about 70 chunks/s on
/// the Neural Engine beside it).
pub const IMAGE_UNITS: f64 = 8.0;

/// What reading a file weighs, so a run of unchanged files still moves.
pub const FILE_UNITS: f64 = 0.5;

/// The counts a run's share done is taken from.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Work {
    /// Files handled, of the files found.
    pub files: (u64, u64),
    /// Chunks embedded, of the chunks expected.
    pub chunks: (u64, u64),
    /// Images embedded, of the images found.
    pub images: (u64, u64),
    /// What an image weighs in chunks; [`IMAGE_UNITS`] until measured. See
    /// [`image_units`].
    pub image_weight: f64,
    /// Whether images embed beside the text rather than taking turns with it
    /// ([`crate::accel::images_beside_text`]): the run then takes as long as
    /// the slower of the two, not their sum.
    pub parallel: bool,
}

/// What an image weighs against a chunk on this machine: the lanes' chunks
/// per second over the image model's images per second, once both have been
/// measured or checked here, and [`IMAGE_UNITS`] until then.
pub fn image_units() -> f64 {
    let rates = crate::accel::rates();
    match (crate::accel::expected_rate(), rates.get("clip")) {
        (Some(chunks), Some(clip)) if clip.per_s > 0.0 => (chunks / clip.per_s).clamp(1.0, 200.0),
        _ => IMAGE_UNITS,
    }
}

impl Work {
    /// Units done and units in all.
    pub fn units(&self) -> (f64, f64) {
        let (f, ft) = self.files;
        let (c, ct) = self.chunks;
        let (i, it) = self.images;
        let w = if self.image_weight > 0.0 {
            self.image_weight
        } else {
            IMAGE_UNITS
        };
        let files = (f.min(ft) as f64 * FILE_UNITS, ft as f64 * FILE_UNITS);
        if self.parallel {
            // The longer of the two paths is the run's: replay 10 took 751 s
            // where the sum said 1,147.
            let all = (ct as f64).max(it as f64 * w) + files.1;
            let left =
                ((ct - c.min(ct)) as f64).max((it - i.min(it)) as f64 * w) + files.1 - files.0;
            return (all - left, all);
        }
        let done = c.min(ct) as f64 + i.min(it) as f64 * w + files.0;
        let all = ct as f64 + it as f64 * w + files.1;
        (done, all)
    }

    /// The share done, in [0, 1].
    pub fn share(&self) -> f64 {
        let (done, all) = self.units();
        if all <= 0.0 {
            0.0
        } else {
            (done / all).clamp(0.0, 1.0)
        }
    }
}

/// Bytes a chunk takes, by kind of file, for a file the plan never chunked.
/// Measured on the 70-repository bench corpus; a file corrects its own
/// estimate the moment it is chunked, so these only have to be close.
fn bytes_per_chunk(path: &Path) -> Option<u64> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    Some(match ext.as_str() {
        "json" | "jsonl" | "ndjson" | "log" | "csv" | "tsv" => 700,
        "md" | "markdown" | "mdx" | "txt" | "rst" | "adoc" => 900,
        "html" | "htm" | "xml" | "svg" => 1_500,
        "pdf" => 4_000,
        "docx" | "pptx" | "xlsx" | "odt" | "odp" | "ods" | "epub" => 6_000,
        "lock" => 1_200,
        _ => 1_100,
    })
}

/// A file's expected chunks and whether it is an image, from what the plan
/// counted or, when it never saw the file, from its size and kind.
pub fn estimate(path: &Path, bytes: u64, planned: Option<u64>) -> (u64, bool) {
    if crate::image::is_image(path) {
        return (0, true);
    }
    if let Some(n) = planned {
        return (n, false);
    }
    if bytes == 0 {
        return (0, false);
    }
    let per = bytes_per_chunk(path).unwrap_or(1_100);
    (bytes.div_ceil(per).max(1), false)
}

/// How much a shown time left may move in ten seconds, either way.
const STEP: f64 = 1.25;

/// How long the rate is averaged over, in seconds. A mixed corpus changes
/// pace every minute or so (a stretch of images, a dense log, a slice's drain);
/// thirty seconds followed each turn and read it as the whole run's pace.
const TAU: f64 = 90.0;

/// Seconds of measured progress before the measurement replaces the prior.
/// A lane's first minute runs below its rate (models warming, the first
/// windows filling), and twenty seconds of it read as the whole run's pace.
const SETTLE: f64 = 60.0;

/// Time left from a run's units, smoothed so it is worth reading.
///
/// The rate is an exponentially weighted average of units per second over
/// about thirty seconds of the run's own time, and until twenty seconds have
/// been measured it leans on a prior: what this machine's lanes are known to
/// do. A shown figure moves at most a quarter per ten seconds either way, a
/// cap on time rather than on how often someone asks, and holds still while
/// the run is waiting on something that is not its work (a lane loading, the
/// index being written).
#[derive(Debug, Clone, Default)]
pub struct Eta {
    last: Option<(f64, f64)>,
    rate: Option<f64>,
    measured: f64,
    recent: VecDeque<(f64, f64)>,
    shown: Option<(f64, f64)>,
    prior: Option<f64>,
    /// The work left at the last estimate, to tell the work changing from
    /// the rate changing.
    work: Option<f64>,
}

impl Eta {
    /// The rate this machine is expected to manage, in units per second.
    pub fn with_prior(prior: Option<f64>) -> Self {
        Self {
            prior: prior.filter(|r| *r > 0.0),
            ..Self::default()
        }
    }

    pub fn set_prior(&mut self, prior: Option<f64>) {
        self.prior = prior.filter(|r| *r > 0.0);
    }

    /// Note `done` units at `at` seconds of the run's active time.
    pub fn observe(&mut self, at: f64, done: f64) {
        if let Some((t, d)) = self.last {
            let dt = at - t;
            if dt < 1.0 {
                return;
            }
            let moved = (done - d).max(0.0);
            let now = moved / dt;
            if moved > 0.0 {
                self.measured += dt;
            }
            let w = 1.0 - (-dt / TAU).exp();
            self.rate = Some(match self.rate {
                Some(r) if moved > 0.0 || self.measured > 0.0 => r + w * (now - r),
                Some(r) => r,
                // Seeded with what this machine is known to manage, so a slow
                // first minute (lanes warming, the first slice's drain) moves
                // the figure by its weight rather than becoming it.
                None if moved > 0.0 => self.prior.map_or(now, |p| p + w * (now - p)),
                None => return self.last = Some((at, done)),
            });
            self.recent.push_back((at, now));
            while self.recent.front().is_some_and(|(t0, _)| at - t0 > 60.0) {
                self.recent.pop_front();
            }
        }
        self.last = Some((at, done));
    }

    fn rate(&self) -> Option<f64> {
        let measured = self.rate.filter(|r| *r > 0.0);
        match (measured, self.prior) {
            (Some(m), Some(p)) if self.measured < SETTLE => {
                let k = self.measured / SETTLE;
                Some(p + k * (m - p))
            }
            (Some(m), _) => Some(m),
            (None, p) => p,
        }
    }

    /// Milliseconds left for `remaining` units at `at`, and a low and high
    /// bound from the last minute's spread. `hold` keeps the shown figure
    /// where it is.
    ///
    /// The cap is on how the rate moves the figure, not on how the work does:
    /// when the work left grows or shrinks by more than the cap allows (a walk
    /// finishing, a decision letting files in) the figure follows the work,
    /// so a first reading taken before the total was known is never what the
    /// rest of the run is held to.
    pub fn left(&mut self, at: f64, remaining: f64, hold: bool) -> Option<(u64, (u64, u64))> {
        let rate = self.rate()?;
        let raw = remaining.max(0.0) / rate;
        let rescoped = self
            .work
            .is_some_and(|w| w > 0.0 && ((remaining / w).ln().abs() > STEP.ln()));
        if rescoped {
            self.shown = None;
        }
        self.work = Some(remaining.max(1.0));
        let value = match self.shown {
            Some((t, shown)) => {
                let dt = (at - t).max(0.0);
                if hold {
                    shown
                } else {
                    let counted = (shown - dt).max(0.0);
                    let f = STEP.powf(dt / 10.0);
                    raw.clamp(counted / f, counted.max(1.0) * f)
                }
            }
            None => raw,
        };
        self.shown = Some((at, value));
        let rates: Vec<f64> = self
            .recent
            .iter()
            .map(|(_, r)| *r)
            .filter(|r| *r > 0.0)
            .collect();
        let (lo, hi) = if self.measured >= SETTLE && rates.len() >= 3 {
            let fast = rates.iter().cloned().fold(f64::MIN, f64::max);
            let slow = rates.iter().cloned().fold(f64::MAX, f64::min);
            ((remaining / fast).min(value), (remaining / slow).max(value))
        } else {
            (value * 0.7, value * 1.5)
        };
        let ms = |s: f64| (s * 1000.0).round() as u64;
        Some((ms(value), (ms(lo), ms(hi))))
    }
}

/// A count as a person reads it, with thousands separated: 15,666.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {

    #[test]
    fn counts_are_grouped_by_thousands() {
        assert_eq!(super::grouped(0), "0");
        assert_eq!(super::grouped(999), "999");
        assert_eq!(super::grouped(5_000), "5,000");
        assert_eq!(super::grouped(1_234_567), "1,234,567");
    }

    use super::*;
    use std::path::PathBuf;

    #[test]
    fn units_count_files_chunks_and_images() {
        let w = Work {
            files: (10, 100),
            chunks: (500, 1_000),
            images: (2, 4),
            image_weight: IMAGE_UNITS,
            parallel: false,
        };
        let (done, all) = w.units();
        assert_eq!(done, 500.0 + 16.0 + 5.0);
        assert_eq!(all, 1_000.0 + 32.0 + 50.0);
        assert!(w.share() < 0.5);
    }

    /// Images beside the text: the run is as long as the longer path.
    #[test]
    fn parallel_work_is_the_longer_path() {
        let mut w = Work {
            files: (0, 0),
            chunks: (500, 1_000),
            images: (10, 100),
            image_weight: 8.0,
            parallel: true,
        };
        // All of it is the text's 1,000; left is the images' 90 x 8 = 720,
        // more than the text's 500.
        assert_eq!(w.units(), (280.0, 1_000.0));
        w.images = (100, 100);
        // Images done; the text's 500 of 1,000 is what is left.
        assert_eq!(w.units(), (500.0, 1_000.0));
    }

    #[test]
    fn a_planned_count_wins_and_an_image_is_an_image() {
        assert_eq!(
            estimate(&PathBuf::from("a.rs"), 11_000, Some(4)),
            (4, false)
        );
        assert_eq!(estimate(&PathBuf::from("a.rs"), 11_000, None), (10, false));
        assert_eq!(
            estimate(&PathBuf::from("a.jsonl"), 7_000, None),
            (10, false)
        );
        assert_eq!(
            estimate(&PathBuf::from("shot.png"), 3_000_000, None),
            (0, true)
        );
        assert_eq!(estimate(&PathBuf::from("empty.md"), 0, None), (0, false));
    }

    /// The walk's shape: a steady rate with one file that adds thousands of
    /// chunks at once. Every shown figure moves at most a quarter in ten
    /// seconds, and it lands near the truth once the rate has settled.
    #[test]
    fn the_shown_time_moves_at_most_a_quarter_in_ten_seconds() {
        let mut eta = Eta::with_prior(Some(100.0));
        let mut shown = Vec::new();
        let mut expected = 60_000.0;
        for s in 0..600 {
            let t = s as f64;
            let done = t * 100.0;
            if s == 120 {
                expected += 30_000.0;
            }
            eta.observe(t, done);
            if let Some((ms, (lo, hi))) = eta.left(t, expected - done, false) {
                assert!(lo <= ms && ms <= hi, "{lo} {ms} {hi}");
                shown.push((t, ms as f64 / 1000.0));
            }
        }
        // The work grew by half at 120 s: the figure follows the work there,
        // and the cap holds everywhere else.
        for pair in shown
            .windows(11)
            .filter(|p| !(p[0].0 < 120.0 && p[10].0 >= 120.0))
        {
            let (t0, a) = pair[0];
            let (t1, b) = pair[10];
            // Never up by more than a quarter, never down by more than a
            // quarter beyond the ten seconds that passed.
            let counted = (a - (t1 - t0)).max(1.0);
            assert!(
                b <= a * STEP * 1.0001 && b >= counted / STEP / 1.0001,
                "{a} -> {b}"
            );
        }
        let (_, last) = *shown.last().unwrap();
        let truth = (expected - 599.0 * 100.0) / 100.0;
        assert!((last - truth).abs() / truth < 0.05, "{last} vs {truth}");
    }

    #[test]
    fn a_hold_keeps_the_figure_and_no_prior_and_no_measure_says_nothing() {
        let mut eta = Eta::default();
        assert_eq!(eta.left(0.0, 1_000.0, false), None);
        let mut eta = Eta::with_prior(Some(10.0));
        let (first, _) = eta.left(0.0, 1_000.0, false).unwrap();
        let (held, _) = eta.left(30.0, 1_000.0, true).unwrap();
        assert_eq!(first, held);
    }
}
