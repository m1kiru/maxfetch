use std::cmp::Ordering;

use crate::model::Candidate;

pub fn compare_quality(a: &Candidate, b: &Candidate) -> Ordering {
    match (a.quality.lossless, b.quality.lossless) {
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    if a.quality.lossless {
        cmp_opt_desc(a.quality.bit_depth, b.quality.bit_depth)
            .then_with(|| cmp_opt_desc(a.quality.sample_rate, b.quality.sample_rate))
    } else {
        cmp_opt_desc(a.quality.bitrate_kbps, b.quality.bitrate_kbps)
            .then_with(|| cmp_opt_desc(a.quality.sample_rate, b.quality.sample_rate))
    }
}

fn cmp_opt_desc<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    match (a, b) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => Ordering::Equal,
    }
}

pub fn rank_candidates(candidates: &[Candidate]) -> Vec<Candidate> {
    let has_lossless = candidates.iter().any(|c| c.quality.lossless);
    let mut kept: Vec<Candidate> = candidates
        .iter()
        .filter(|c| !has_lossless || c.quality.lossless)
        .cloned()
        .collect();
    kept.sort_by(|a, b| compare_quality(b, a));
    kept
}
