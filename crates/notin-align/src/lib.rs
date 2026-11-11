//! Statistical word alignment between an original-language text (verses
//! as lemma ids) and a translation (verses as tokens).
//!
//! Two IBM Model 1 runs with a diagonal prior, one per direction, are
//! symmetrised with intersection grown along the diagonal. The result
//! tells, for each verse, which translation tokens render each original
//! word, and, for each lemma, how the translation renders it across the
//! whole corpus. Function words are not filtered here; the caller decides
//! by morphology.

mod model;
mod tokenize;

use std::collections::HashMap;

pub use tokenize::{tokenize, tokenize_surface};

/// Book, chapter and verse; the pairing key between both texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VerseKey {
    pub book: u8,
    pub chapter: u16,
    pub verse: u16,
}

/// One verse of the original text as lemma ids in word order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceVerse {
    pub key: VerseKey,
    pub lemmas: Vec<u32>,
}

/// One verse of the translation as normalised tokens in word order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetVerse {
    pub key: VerseKey,
    pub tokens: Vec<String>,
}

/// Training and filtering knobs.
#[derive(Debug, Clone, PartialEq)]
pub struct AlignConfig {
    /// EM rounds per direction.
    pub iterations: u32,
    /// Strength of the diagonal prior (`fast_align`'s lambda).
    pub diagonal_tension: f32,
    /// Links below this posterior are dropped.
    pub min_link_prob: f32,
    /// Renderings seen fewer times than this are dropped.
    pub min_rendering_count: u32,
    /// Renderings kept per lemma.
    pub max_renderings: usize,
}

impl Default for AlignConfig {
    fn default() -> Self {
        Self {
            iterations: 5,
            diagonal_tension: 4.0,
            min_link_prob: 0.3,
            min_rendering_count: 2,
            max_renderings: 3,
        }
    }
}

/// One original word rendered by a contiguous span of translation tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub source_position: u16,
    pub target_start: u16,
    /// Exclusive.
    pub target_end: u16,
    pub prob: f32,
}

/// Links of one paired verse.
#[derive(Debug, Clone, PartialEq)]
pub struct VerseAlignment {
    pub key: VerseKey,
    pub links: Vec<Link>,
}

/// How often a lemma was rendered by `token` (a span joined by spaces).
#[derive(Debug, Clone, PartialEq)]
pub struct Rendering {
    pub token: String,
    pub count: u32,
    /// `count` over all links of the lemma.
    pub prob: f32,
}

/// Result of [`align`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Alignment {
    /// One entry per verse present on both sides, in source order.
    pub verses: Vec<VerseAlignment>,
    /// Renderings per lemma id, sorted by lemma id.
    pub renderings: Vec<(u32, Vec<Rendering>)>,
}

/// Aligns `source` with `target`. Verses are paired by key; verses missing
/// on either side are skipped.
#[must_use]
pub fn align(source: &[SourceVerse], target: &[TargetVerse], config: &AlignConfig) -> Alignment {
    let corpus = Corpus::build(source, target);
    if corpus.pairs.is_empty() {
        return Alignment::default();
    }
    let lemma_vocab = u32::try_from(corpus.lemma_ids.len()).unwrap_or(u32::MAX);
    let token_vocab = u32::try_from(corpus.token_texts.len()).unwrap_or(u32::MAX);
    let (src_sents, tgt_sents): (Vec<_>, Vec<_>) = corpus
        .pairs
        .iter()
        .map(|p| (p.src.clone(), p.tgt.clone()))
        .unzip();
    // Direction A explains each target token by a source lemma.
    let tgt_by_src = model::Trainer::new(
        &src_sents,
        &tgt_sents,
        lemma_vocab,
        config.diagonal_tension,
        config.min_link_prob,
    )
    .run(config.iterations);
    // Direction B explains each source lemma by a target token.
    let src_by_tgt = model::Trainer::new(
        &tgt_sents,
        &src_sents,
        token_vocab,
        config.diagonal_tension,
        config.min_link_prob,
    )
    .run(config.iterations);

    let mut renderings: HashMap<u32, HashMap<String, u32>> = HashMap::new();
    let mut verses = Vec::with_capacity(corpus.pairs.len());
    for (idx, pair) in corpus.pairs.iter().enumerate() {
        let grid = Grid::symmetrise(
            pair.src.len(),
            pair.tgt.len(),
            &tgt_by_src[idx],
            &src_by_tgt[idx],
        );
        let links = grid.spans(config.min_link_prob);
        for link in &links {
            let lemma = corpus.lemma_ids[pair.src[link.source_position as usize] as usize];
            let text = pair.tgt[link.target_start as usize..link.target_end as usize]
                .iter()
                .map(|&t| corpus.token_texts[t as usize].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            *renderings
                .entry(lemma)
                .or_default()
                .entry(text)
                .or_default() += 1;
        }
        verses.push(VerseAlignment {
            key: pair.key,
            links,
        });
    }

    let mut renderings: Vec<(u32, Vec<Rendering>)> = renderings
        .into_iter()
        .filter_map(|(lemma, counts)| {
            let total: u32 = counts.values().sum();
            let mut list: Vec<Rendering> = counts
                .into_iter()
                .filter(|&(_, count)| count >= config.min_rendering_count)
                .map(|(token, count)| Rendering {
                    token,
                    count,
                    #[allow(clippy::cast_precision_loss)]
                    prob: count as f32 / total as f32,
                })
                .collect();
            list.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.token.cmp(&b.token)));
            list.truncate(config.max_renderings);
            (!list.is_empty()).then_some((lemma, list))
        })
        .collect();
    renderings.sort_by_key(|(lemma, _)| *lemma);

    Alignment { verses, renderings }
}

/// Paired verses in dense vocabulary indices.
struct Corpus {
    pairs: Vec<Pair>,
    /// Dense lemma index → original lemma id.
    lemma_ids: Vec<u32>,
    /// Dense token index → token text.
    token_texts: Vec<String>,
}

struct Pair {
    key: VerseKey,
    src: Vec<u32>,
    tgt: Vec<u32>,
}

impl Corpus {
    fn build(source: &[SourceVerse], target: &[TargetVerse]) -> Self {
        let by_key: HashMap<VerseKey, &TargetVerse> = target.iter().map(|v| (v.key, v)).collect();
        let mut lemma_index: HashMap<u32, u32> = HashMap::new();
        let mut lemma_ids = Vec::new();
        let mut token_index: HashMap<&str, u32> = HashMap::new();
        let mut token_texts = Vec::new();
        let mut pairs = Vec::new();
        for verse in source {
            let Some(tgt) = by_key.get(&verse.key) else {
                continue;
            };
            if verse.lemmas.is_empty() || tgt.tokens.is_empty() {
                continue;
            }
            let src = verse
                .lemmas
                .iter()
                .map(|&lemma| {
                    *lemma_index.entry(lemma).or_insert_with(|| {
                        lemma_ids.push(lemma);
                        u32::try_from(lemma_ids.len() - 1).unwrap_or(u32::MAX)
                    })
                })
                .collect();
            let tgt_ids = tgt
                .tokens
                .iter()
                .map(|token| {
                    *token_index.entry(token.as_str()).or_insert_with(|| {
                        token_texts.push(token.clone());
                        u32::try_from(token_texts.len() - 1).unwrap_or(u32::MAX)
                    })
                })
                .collect();
            pairs.push(Pair {
                key: verse.key,
                src,
                tgt: tgt_ids,
            });
        }
        Self {
            pairs,
            lemma_ids,
            token_texts,
        }
    }
}

/// Dense `n × m` link grid of one verse pair used for symmetrisation.
struct Grid {
    n: usize,
    m: usize,
    aligned: Vec<bool>,
    union: Vec<bool>,
    prob: Vec<f32>,
}

impl Grid {
    const fn at(&self, i: usize, j: usize) -> usize {
        i * self.m + j
    }

    /// Intersection of the 1-best links of both directions, grown along
    /// the diagonal (grow-diag) over the union of every candidate either
    /// direction proposed. A point proposed by both keeps the mean of
    /// the two posteriors.
    fn symmetrise(
        n: usize,
        m: usize,
        tgt_by_src: &[model::Posterior],
        src_by_tgt: &[model::Posterior],
    ) -> Self {
        let mut grid = Self {
            n,
            m,
            aligned: vec![false; n * m],
            union: vec![false; n * m],
            prob: vec![0.0; n * m],
        };
        let mut seen_in_a = vec![false; n * m];
        for (j, post) in tgt_by_src.iter().enumerate() {
            for &(i, p) in &post.candidates {
                let k = grid.at(usize::from(i), j);
                seen_in_a[k] = true;
                grid.union[k] = true;
                grid.prob[k] = p;
            }
        }
        for (i, post) in src_by_tgt.iter().enumerate() {
            for &(j, p) in &post.candidates {
                let k = grid.at(i, usize::from(j));
                if seen_in_a[k] {
                    grid.prob[k] = f32::midpoint(grid.prob[k], p);
                } else {
                    grid.union[k] = true;
                    grid.prob[k] = p;
                }
            }
        }
        for (j, post) in tgt_by_src.iter().enumerate() {
            let Some(i) = post.best else { continue };
            if src_by_tgt[usize::from(i)].best == u16::try_from(j).ok() {
                let k = grid.at(usize::from(i), j);
                grid.aligned[k] = true;
                grid.union[k] = true;
            }
        }
        grid.grow_diagonal();
        grid
    }

    /// Adds union points next to an aligned point while the source word or
    /// the target token is still unlinked, always the most probable
    /// candidate first so the outcome does not depend on scan order.
    fn grow_diagonal(&mut self) {
        let mut src_covered = vec![false; self.n];
        let mut tgt_covered = vec![false; self.m];
        let mut frontier: Vec<usize> = Vec::new();
        for k in 0..self.n * self.m {
            if self.aligned[k] {
                let (i, j) = (k / self.m, k % self.m);
                src_covered[i] = true;
                tgt_covered[j] = true;
                self.push_neighbours(i, j, &mut frontier);
            }
        }
        loop {
            let mut best: Option<(usize, f32)> = None;
            for &k in &frontier {
                let (i, j) = (k / self.m, k % self.m);
                let unlinked_side = !src_covered[i] || !tgt_covered[j];
                let eligible = self.union[k] && !self.aligned[k] && unlinked_side;
                if eligible && best.is_none_or(|(_, p)| self.prob[k] > p) {
                    best = Some((k, self.prob[k]));
                }
            }
            let Some((k, _)) = best else { break };
            let (i, j) = (k / self.m, k % self.m);
            self.aligned[k] = true;
            src_covered[i] = true;
            tgt_covered[j] = true;
            self.push_neighbours(i, j, &mut frontier);
        }
    }

    fn push_neighbours(&self, i: usize, j: usize, frontier: &mut Vec<usize>) {
        for (di, dj) in NEIGHBOURS {
            let Some(ni) = i.checked_add_signed(di).filter(|&ni| ni < self.n) else {
                continue;
            };
            let Some(nj) = j.checked_add_signed(dj).filter(|&nj| nj < self.m) else {
                continue;
            };
            let k = self.at(ni, nj);
            if self.union[k] && !self.aligned[k] && !frontier.contains(&k) {
                frontier.push(k);
            }
        }
    }

    /// Links above `min_prob`, with adjacent target tokens of the same
    /// source position merged into one span.
    fn spans(&self, min_prob: f32) -> Vec<Link> {
        let mut links = Vec::new();
        for i in 0..self.n {
            let Ok(source_position) = u16::try_from(i) else {
                continue;
            };
            let mut run: Option<(usize, usize, f32, u32)> = None;
            for j in 0..=self.m {
                let hit = j < self.m && {
                    let k = self.at(i, j);
                    self.aligned[k] && self.prob[k] >= min_prob
                };
                match (hit, run.as_mut()) {
                    (true, Some((_, end, sum, count))) if *end == j => {
                        *end = j + 1;
                        *sum += self.prob[self.at(i, j)];
                        *count += 1;
                    }
                    (true, _) => {
                        if let Some(done) = run.take() {
                            links.push(Self::link(source_position, done));
                        }
                        run = Some((j, j + 1, self.prob[self.at(i, j)], 1));
                    }
                    (false, Some(_)) => {
                        if let Some(done) = run.take() {
                            links.push(Self::link(source_position, done));
                        }
                    }
                    (false, None) => {}
                }
            }
        }
        links
    }

    #[allow(clippy::cast_precision_loss)]
    fn link(source_position: u16, (start, end, sum, count): (usize, usize, f32, u32)) -> Link {
        Link {
            source_position,
            target_start: u16::try_from(start).unwrap_or(u16::MAX),
            target_end: u16::try_from(end).unwrap_or(u16::MAX),
            prob: sum / count as f32,
        }
    }
}

const NEIGHBOURS: [(isize, isize); 8] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (-1, 1),
    (1, -1),
    (1, 1),
];
