//! Writer-independent evaluation: templates picked from the dataset's
//! `trn` writers, scored on the unseen `tst` writers.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use notein_handwriting::{Candidate, Recognizer};

use crate::select::{Extraction, medoids};
use crate::uji::Sample;

/// Extent weights compared per run; `None` is shape only.
const EXTENT_WEIGHTS: [Option<f32>; 2] = [None, Some(1.0)];
const MARK_WEIGHTS: [f32; 2] = [0.25, 0.5];

/// Folds classes a writer cannot be told apart by shape alone: case,
/// and zero against `o`.
fn fold(glyph: char) -> char {
    match glyph.to_lowercase().next().unwrap_or(glyph) {
        '0' => 'o',
        lower => lower,
    }
}

const fn is_accented(glyph: char) -> bool {
    matches!(
        glyph,
        'á' | 'é' | 'í' | 'ó' | 'ú' | 'ü' | 'ñ' | 'Á' | 'É' | 'Í' | 'Ó' | 'Ú' | 'Ü' | 'Ñ'
    )
}

#[derive(Debug, Default)]
struct Tally {
    total: usize,
    top1: usize,
    top3: usize,
    folded: usize,
    accented: usize,
    accented_top1: usize,
    confusions: BTreeMap<(char, char), usize>,
    elapsed: Duration,
}

impl Tally {
    fn record(&mut self, truth: char, ranked: &[Candidate], elapsed: Duration) {
        self.total += 1;
        self.elapsed += elapsed;
        self.top3 += usize::from(ranked.iter().any(|candidate| candidate.glyph == truth));
        let best = ranked.first().map(|candidate| candidate.glyph);
        let hit = best == Some(truth);
        self.top1 += usize::from(hit);
        self.folded += usize::from(best.map(fold) == Some(fold(truth)));
        if is_accented(truth) {
            self.accented += 1;
            self.accented_top1 += usize::from(hit);
        }
        if let Some(best) = best.filter(|best| fold(*best) != fold(truth)) {
            *self.confusions.entry((truth, best)).or_default() += 1;
        }
    }

    #[allow(clippy::cast_precision_loss, reason = "sample counts are small")]
    fn rate(hits: usize, total: usize) -> f64 {
        100.0 * hits as f64 / total.max(1) as f64
    }

    #[allow(clippy::cast_precision_loss, reason = "sample counts are small")]
    fn print(&self, label: &str) {
        println!(
            "   {label:<26} top-1 {:5.1}%  top-3 {:5.1}%  case-folded {:5.1}%  accented {:5.1}%  {:.2} ms/char",
            Self::rate(self.top1, self.total),
            Self::rate(self.top3, self.total),
            Self::rate(self.folded, self.total),
            Self::rate(self.accented_top1, self.accented),
            self.elapsed.as_secs_f64() * 1000.0 / self.total.max(1) as f64
        );
    }

    fn print_confusions(&self) {
        let mut confusions: Vec<_> = self.confusions.iter().collect();
        confusions.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<String> = confusions
            .iter()
            .take(20)
            .map(|((truth, guess), count)| format!("{truth}→{guess} {count}"))
            .collect();
        println!("   top confusions beyond case: {}", top.join(", "));
    }
}

fn score(recognizer: &Recognizer, test: &[Sample], with_line: bool) -> Tally {
    let mut tally = Tally::default();
    for sample in test {
        let started = Instant::now();
        let ranked = recognizer.recognize(&sample.strokes, with_line.then_some(sample.line), 3);
        tally.record(sample.glyph, &ranked, started.elapsed());
    }
    tally
}

pub(crate) fn run(train: &[Sample], test: &[Sample], per_class: &[usize]) {
    for &k in per_class {
        for extraction in [Extraction::Whole, Extraction::Decomposed] {
            let started = Instant::now();
            let chosen = medoids(extraction.templates(train), k);
            println!(
                "── {k} per class, {extraction:?}: {} templates (picked in {:.1}s)",
                chosen.len(),
                started.elapsed().as_secs_f64()
            );
            let mark_weights: &[f32] = match extraction {
                Extraction::Whole => &[Recognizer::MARK_WEIGHT],
                Extraction::Decomposed => &MARK_WEIGHTS,
            };
            for &mark_weight in mark_weights {
                for extent_weight in EXTENT_WEIGHTS {
                    let mut recognizer = Recognizer::default()
                        .with_mark_weight(mark_weight)
                        .with_extent_weight(extent_weight.unwrap_or(0.0));
                    for template in &chosen {
                        recognizer.insert(template);
                    }
                    let tally = score(&recognizer, test, extent_weight.is_some());
                    let label = format!(
                        "mark ×{mark_weight} {}",
                        extent_weight.map_or_else(
                            || "shape".to_owned(),
                            |weight| format!("extent ×{weight}")
                        )
                    );
                    tally.print(&label);
                    if extent_weight.is_none()
                        && mark_weight.to_bits() == Recognizer::MARK_WEIGHT.to_bits()
                    {
                        tally.print_confusions();
                    }
                }
            }
        }
    }
}
