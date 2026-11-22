//! Per-stage wall time and peak memory for a CLI run. The pipeline announces every step through the stage hook; a sampler thread reads the process's resident size and its
//! `phys_footprint` (the number iOS's jetsam is expressed in) every `SAMPLE_EVERY` (`task_info`, the kernel's own numbers) and keeps the largest of each per step, and what each
//! was when the step ended (what the stage left behind). `ru_maxrss` (the process high-water mark) is also read at every boundary, and a step that raised it has that as its resident
//! peak: the kernel's maximum is exact whatever the sampling, so a spike between two samples is not lost there. The same holds for the footprint: the kernel's footprint high-water mark
//! (`sys::mem::footprint_peak`, TASK_VM_INFO's `ledger_phys_footprint_peak`) is read at every boundary, and a step that raised it has it as its footprint peak (review r4 B4).

use ibuilder_pipeline::stage::{module_counter, set_stage_hook};
use ibuilder_pipeline::sys::mem::{footprint_peak, mem_now};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Segment {
    pub label: String,
    pub wall_s: f64,
    /// the largest resident size sampled while this step ran (every `SAMPLE_EVERY` and at both of its ends), or the process's high-water mark when this step raised it
    pub peak_resident: u64,
    /// the process's high-water mark (ru_maxrss) when the step ended
    pub high_water_at_end: u64,
    /// the largest `phys_footprint` sampled while this step ran, or the kernel's footprint high-water mark when this step raised it (0 where the kernel has no such number)
    pub peak_footprint: u64,
    /// the resident size and the footprint when the step ended: what it left behind for the next one
    pub resident_at_end: u64,
    pub footprint_at_end: u64,
}

/// How often the sampler reads the memory: the peaks that matter (a module's rewrite, the split's archives, the translation) last seconds, and the kernel keeps the exact resident
/// high-water mark itself. It was 5 ms: 200 samples a second, ~85,000 kernel calls over a 141 s build (review r4 §6.3).
/// Known limit (review r5 P16, accepted): a step shorter than this, or a spike between two samples, that does NOT raise `ru_maxrss` / the footprint high-water mark has only the values read at
/// its two ends (and any sample that fell inside it) as its peaks: such a peak is below the process's earlier maximum, so it can be under-reported for that step, never for the run.
pub const SAMPLE_EVERY: Duration = Duration::from_millis(100);

struct State {
    label: String,
    started: Instant,
    max_resident: u64,
    max_footprint: u64,
    /// the process high-water mark when this step started: a step that ends above it raised it
    peak_at_start: u64,
    /// the kernel's footprint high-water mark when this step started (0 where there is none)
    fp_peak_at_start: u64,
    done: Vec<Segment>,
}

impl State {
    /// The step that ends now, with the numbers read at its end.
    fn close(&self, res: u64, peak: u64, fp: u64, fp_peak: u64) -> Segment {
        let sampled = self.max_resident.max(res);
        let sampled_fp = self.max_footprint.max(fp);
        Segment {
            label: self.label.clone(),
            wall_s: self.started.elapsed().as_secs_f64(),
            peak_resident: if peak > self.peak_at_start {
                sampled.max(peak)
            } else {
                sampled
            },
            high_water_at_end: peak,
            peak_footprint: if fp_peak > self.fp_peak_at_start {
                sampled_fp.max(fp_peak)
            } else {
                sampled_fp
            },
            resident_at_end: res,
            footprint_at_end: fp,
        }
    }
}

pub struct Meter {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    sampler: Option<std::thread::JoinHandle<()>>,
    t0: Instant,
}

/// "stage 2: rewriting the modules (1/3: main)" -> "2 rewrite main"; other steps keep their first words.
pub fn label(k: u32, text: &str) -> String {
    if let Some((_, _, m)) = module_counter(text) {
        return format!("{k} rewrite {m}");
    }
    if k == 3 {
        // stage 3 announces "shaders: reading the game's shaders (..)", "shaders: translating for this iPhone (n/N shaders)" (the progress lines only change the count) and
        // "shaders: the library for <id> is already here": the split and the build are two steps of the table
        let what = if text.contains("reading") {
            "split"
        } else if text.contains("translating") {
            "build"
        } else if text.contains("already here") {
            "already built"
        } else {
            return format!(
                "3 {}",
                text.split(['(', ':'])
                    .next()
                    .unwrap_or(text)
                    .split_whitespace()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        };
        return format!("3 shaders {what}");
    }
    let words: Vec<&str> = text
        .split(['(', ':'])
        .next()
        .unwrap_or(text)
        .split_whitespace()
        .take(3)
        .collect();
    format!("{k} {}", words.join(" "))
}

impl Meter {
    pub fn start() -> Meter {
        let (res, peak, fp) = mem_now();
        let fp_peak = footprint_peak().unwrap_or(0);
        let state = Arc::new(Mutex::new(State {
            label: "0 start".into(),
            started: Instant::now(),
            max_resident: res,
            max_footprint: fp,
            peak_at_start: peak,
            fp_peak_at_start: fp_peak,
            done: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, st2) = (state.clone(), stop.clone());
        let sampler = std::thread::Builder::new()
            .name("meter".into())
            .spawn(move || {
                while !st2.load(Ordering::Relaxed) {
                    let (res, _, fp) = mem_now();
                    {
                        let mut s = s2.lock().unwrap();
                        s.max_resident = s.max_resident.max(res);
                        s.max_footprint = s.max_footprint.max(fp);
                    }
                    std::thread::park_timeout(SAMPLE_EVERY); // `finish` unparks it: the run does not wait a period for the sampler to notice
                }
            })
            .ok();
        let s3 = state.clone();
        set_stage_hook(Some(Arc::new(move |k, _n, text| {
            let l = label(k, text);
            let mut s = s3.lock().unwrap();
            if s.label == l {
                return;
            }
            let (res, peak, fp) = mem_now();
            let fp_peak = footprint_peak().unwrap_or(0);
            let seg = s.close(res, peak, fp, fp_peak);
            s.done.push(seg);
            s.label = l;
            s.started = Instant::now();
            s.max_resident = res;
            s.max_footprint = fp;
            s.peak_at_start = peak;
            s.fp_peak_at_start = fp_peak;
        })));
        Meter {
            state,
            stop,
            sampler,
            t0: Instant::now(),
        }
    }

    /// Close the last step and return every step (the first "0 start" step is dropped when it is empty).
    pub fn finish(mut self) -> (Vec<Segment>, f64, u64) {
        set_stage_hook(None);
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.sampler.take() {
            h.thread().unpark();
            let _ = h.join();
        }
        let (res, peak, fp) = mem_now();
        let fp_peak = footprint_peak().unwrap_or(0);
        let mut s = self.state.lock().unwrap();
        let last = s.close(res, peak, fp, fp_peak);
        s.done.push(last);
        let segs: Vec<Segment> = s
            .done
            .iter()
            .filter(|x| !(x.label == "0 start" && x.wall_s < 0.001))
            .cloned()
            .collect();
        (segs, self.t0.elapsed().as_secs_f64(), peak)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(max_resident: u64, peak_at_start: u64) -> State {
        State {
            label: "2 rewrite main".into(),
            started: Instant::now(),
            max_resident,
            max_footprint: 7,
            peak_at_start,
            fp_peak_at_start: 50,
            done: Vec::new(),
        }
    }

    /// The sampled peak is kept; a step that raised the process high-water mark has that as its peak even when no sample saw it (a spike between two samples); a step that did
    /// not raise it keeps what was sampled (the high-water mark is an earlier step's).
    #[test]
    fn a_step_that_raised_the_high_water_mark_has_it_as_its_peak() {
        let seg = state(500, 900).close(400, 1200, 9, 50);
        assert_eq!(
            seg.peak_resident, 1200,
            "the kernel's maximum, raised in this step"
        );
        assert_eq!(
            (
                seg.high_water_at_end,
                seg.peak_footprint,
                seg.resident_at_end,
                seg.footprint_at_end
            ),
            (1200, 9, 400, 9)
        );
        let seg = state(500, 1200).close(400, 1200, 3, 50);
        assert_eq!(
            seg.peak_resident, 500,
            "the high-water mark is an earlier step's: the samples say"
        );
        assert_eq!(seg.peak_footprint, 7);
        let seg = state(500, 1200).close(600, 1200, 3, 50);
        assert_eq!(seg.peak_resident, 600, "the end sample counts");
    }

    /// Review r4 B4: the footprint peak of a step that raised the kernel's footprint high-water mark is that mark (a spike between two samples is not lost); one that did not keeps
    /// what was sampled.
    #[test]
    fn a_step_that_raised_the_footprint_high_water_mark_has_it_as_its_footprint_peak() {
        let seg = state(500, 1200).close(400, 1200, 6, 900);
        assert_eq!(
            seg.peak_footprint, 900,
            "the kernel's footprint peak, raised in this step (sampled: 7)"
        );
        let seg = state(500, 1200).close(400, 1200, 6, 50);
        assert_eq!(seg.peak_footprint, 7, "not raised here: the samples say");
        let seg = state(500, 1200).close(400, 1200, 6, 0);
        assert_eq!(seg.peak_footprint, 7, "no kernel number: the samples say");
        // and for real: a 96 MiB footprint spike that ends before the next sample is still the step's peak
        let d = ibuilder_pipeline::testutil::TempDir::new("meter-spike");
        let m = Meter::start();
        step(d.path(), 2, "spike step");
        {
            // an anonymous mapping, unmapped when it is dropped: the footprint falls again before the step ends (an allocator may keep a freed big block), and only the kernel's
            // high-water mark still knows the spike
            let mut big = memmap2::MmapMut::map_anon(96 << 20).expect("an anonymous mapping");
            for i in (0..big.len()).step_by(4096) {
                big[i] = 1; // touched: in the footprint
            }
            std::hint::black_box(&big);
            drop(big);
        }
        step(d.path(), 4, "after it");
        let (segs, _, _) = m.finish();
        if footprint_peak().is_some() {
            let spike = segs
                .iter()
                .find(|s| s.label.starts_with("2 spike"))
                .expect("the spike's step");
            assert!(
                spike.peak_footprint >= spike.footprint_at_end + (64 << 20),
                "{spike:?}"
            );
        }
    }

    /// A stage line, as the pipeline writes it (the meter hears it through the stage hook).
    fn step(docs: &std::path::Path, k: u32, text: &str) {
        ibuilder_pipeline::stage::write_stage(
            &ibuilder_pipeline::layout::Docs::new(docs),
            k,
            5,
            text,
            None,
        );
    }

    /// The sampler is woken by `finish`: a run is not held up a whole period, and a step's peak is never below what was read at its end.
    #[test]
    fn finish_does_not_wait_for_the_sampling_period() {
        let m = Meter::start();
        std::thread::sleep(Duration::from_millis(5));
        let t = Instant::now();
        let (segs, _, peak) = m.finish();
        assert!(t.elapsed() < SAMPLE_EVERY, "finish took {:?}", t.elapsed());
        assert!(!segs.is_empty() || peak > 0);
        for s in &segs {
            assert!(
                s.peak_resident >= s.resident_at_end && s.peak_footprint >= s.footprint_at_end,
                "{s:?}"
            );
        }
    }

    #[test]
    fn labels() {
        assert_eq!(
            label(2, "rewriting the modules (1/3: main)"),
            "2 rewrite main"
        );
        assert_eq!(
            label(4, "signing the framework in-process (zsign, no codesign)"),
            "4 signing the framework"
        );
        assert_eq!(
            label(1, "deriving the profile from the game in Documents"),
            "1 deriving the profile"
        );
        assert_eq!(
            label(3, "shaders: reading the game's shaders (0100DFA027504000)"),
            "3 shaders split"
        );
        assert_eq!(
            label(3, "shaders: translating for this iPhone (0/21990 shaders)"),
            "3 shaders build"
        );
        assert_eq!(
            label(
                3,
                "shaders: translating for this iPhone (1400/21990 shaders)"
            ),
            "3 shaders build",
            "the progress count does not start a new step"
        );
        assert_eq!(
            label(
                3,
                "shaders: the library for 0100DFA027504000 is already here"
            ),
            "3 shaders already built"
        );
        assert_eq!(
            label(3, "shader post-rewrites (/m)"),
            "3 shader post-rewrites"
        );
    }
}
