//! Picks the most representative samples of each label with k-medoids
//! (PAM) over DTW distances.

use std::collections::BTreeMap;

use notein_handwriting::{Label, Template, Trace};

use crate::uji::Sample;

/// A template with its trace rebuilt once for distance computations.
#[derive(Debug, Clone)]
struct Prepared {
    template: Template,
    trace: Trace,
}

/// How samples become templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Extraction {
    /// Each sample is one whole template.
    Whole,
    /// Composed letters split into base + mark templates.
    Decomposed,
}

impl Extraction {
    pub(crate) fn templates(self, samples: &[Sample]) -> Vec<Template> {
        samples
            .iter()
            .flat_map(|sample| match self {
                Self::Whole => Template::whole(sample.glyph, &sample.strokes, sample.line)
                    .into_iter()
                    .collect(),
                Self::Decomposed => Template::extract(sample.glyph, &sample.strokes, sample.line),
            })
            .collect()
    }
}

/// Up to `per_class` medoids per label; `0` keeps every template.
pub(crate) fn medoids(templates: Vec<Template>, per_class: usize) -> Vec<Template> {
    if per_class == 0 {
        return templates;
    }
    let mut classes: BTreeMap<Label, Vec<Prepared>> = BTreeMap::new();
    for template in templates {
        classes.entry(template.label).or_default().push(Prepared {
            trace: template.trace(),
            template,
        });
    }
    let classes: Vec<Vec<Prepared>> = classes.into_values().collect();
    let workers = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    // Classes vary a lot in size (marks pool many letters), so workers
    // pull the next class instead of taking fixed chunks.
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut picked = Vec::new();
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(class) = classes.get(index) else {
                            return picked;
                        };
                        picked.extend(
                            pam(&distances(class), per_class)
                                .into_iter()
                                .map(|member| class[member].template.clone()),
                        );
                    }
                })
            })
            .collect();
        let mut picked: Vec<Template> = handles
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect();
        picked.sort_by_key(|template| template.label);
        picked
    })
}

fn distances(class: &[Prepared]) -> Vec<Vec<f32>> {
    class
        .iter()
        .map(|a| {
            class
                .iter()
                .map(|b| a.trace.distance(&b.trace, f32::INFINITY))
                .collect()
        })
        .collect()
}

/// PAM: greedy BUILD, then SWAP until no swap lowers the total
/// distance of every member to its nearest medoid.
fn pam(distances: &[Vec<f32>], k: usize) -> Vec<usize> {
    let n = distances.len();
    if k >= n {
        return (0..n).collect();
    }
    let cost = |medoids: &[usize]| -> f32 {
        (0..n)
            .map(|member| {
                medoids
                    .iter()
                    .map(|medoid| distances[*medoid][member])
                    .fold(f32::INFINITY, f32::min)
            })
            .sum()
    };
    let mut chosen: Vec<usize> = Vec::with_capacity(k);
    while chosen.len() < k {
        let with = |extra: usize| {
            let mut medoids = chosen.clone();
            medoids.push(extra);
            cost(&medoids)
        };
        let Some(next) = (0..n)
            .filter(|candidate| !chosen.contains(candidate))
            .min_by(|a, b| with(*a).total_cmp(&with(*b)))
        else {
            break;
        };
        chosen.push(next);
    }
    let mut current = cost(&chosen);
    loop {
        let mut improved = false;
        for slot in 0..chosen.len() {
            for candidate in 0..n {
                if chosen.contains(&candidate) {
                    continue;
                }
                let previous = std::mem::replace(&mut chosen[slot], candidate);
                let swapped = cost(&chosen);
                if swapped < current {
                    current = swapped;
                    improved = true;
                } else {
                    chosen[slot] = previous;
                }
            }
        }
        if !improved {
            return chosen;
        }
    }
}
