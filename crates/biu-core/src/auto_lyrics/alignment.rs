use super::AudioFeatures;

#[derive(Debug, Clone, Copy, Default)]
pub struct GlobalOffset {
    pub offset_secs: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct LineAlignment {
    pub time: f64,
    pub confidence: f64,
    pub uncertain: bool,
}

fn dot(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

pub fn estimate_global_offset(
    reference: &AudioFeatures,
    target: &AudioFeatures,
    max_search_secs: Option<f64>,
    anchors: &[f64],
) -> GlobalOffset {
    let frame_rate = reference.fps;
    let window = (reference.duration_secs.mul_add(0.25, 0.0).min(12.0) * frame_rate) as usize;
    let max_target = max_search_secs.map_or(target.cens.len(), |seconds| {
        ((seconds * frame_rate) as usize).min(target.cens.len())
    });
    if window < 2
        || reference.cens.len() < window
        || max_target < window
        || target.fps != reference.fps
    {
        return GlobalOffset::default();
    }
    let scan = |start_secs: f64, earliest| {
        let reference_frame =
            ((start_secs * frame_rate) as usize).min(reference.cens.len() - window);
        let reference_window = &reference.cens[reference_frame..reference_frame + window];
        let (frame, similarity) =
            scan_window(reference_window, &target.cens[..max_target], earliest);
        GlobalOffset {
            offset_secs: (frame as f64 - reference_frame as f64) / frame_rate,
            confidence: f64::from(similarity.clamp(0.0, 1.0)),
        }
    };
    let intro = scan(1.0, true);
    let votes = std::iter::once(intro)
        .chain(anchors.iter().take(3).map(|sec| scan(sec - 0.5, false)))
        .collect::<Vec<_>>();
    let mut best_count = 1;
    let mut best_sum = 0.0;
    let mut best = intro;
    for vote in &votes {
        let (count, score, offset) = votes
            .iter()
            .filter(|other| (vote.offset_secs - other.offset_secs).abs() < 0.3)
            .fold((0, 0.0, 0.0), |(count, score, offset), other| {
                (
                    count + 1,
                    score + other.confidence,
                    offset + other.offset_secs,
                )
            });
        if count > best_count || (count >= 2 && count == best_count && score > best_sum) {
            best_count = count;
            best_sum = score;
            best = GlobalOffset {
                offset_secs: offset / f64::from(count),
                confidence: score / f64::from(count),
            };
        }
    }
    best
}

fn scan_window(reference: &[[f32; 12]], target: &[[f32; 12]], earliest: bool) -> (usize, f32) {
    let mut best = (0, -1.0);
    let mut scores = Vec::new();
    for frame in (0..=target.len() - reference.len()).step_by(2) {
        let score = similarity(reference, &target[frame..]);
        if score > best.1 {
            best = (frame, score);
        }
        if earliest {
            scores.push(score);
        }
    }
    if earliest {
        let peaks = || {
            scores
                .windows(3)
                .enumerate()
                .filter(|(_, w)| w[1] > 0.65 && w[1] > w[0] && w[1] > w[2])
        };
        if let Some(top) = peaks().map(|(_, w)| w[1]).reduce(f32::max)
            && let Some((index, window)) = peaks().find(|(_, w)| w[1] >= top * 0.88)
        {
            return ((index + 1) * 2, window[1]);
        }
    }
    best
}

fn similarity(reference: &[[f32; 12]], target: &[[f32; 12]]) -> f32 {
    reference
        .iter()
        .zip(target)
        .step_by(2)
        .map(|(a, b)| dot(a, b))
        .sum::<f32>()
        / reference.len().div_ceil(2) as f32
}

pub(super) fn consensus_reference(studios: &[AudioFeatures]) -> usize {
    for (index, reference) in studios.iter().enumerate() {
        for other in studios.iter().skip(index + 1) {
            let window =
                (reference.duration_secs.mul_add(0.25, 0.0).min(12.0) * reference.fps) as usize;
            if window < 2 || reference.cens.len() < window || other.cens.len() < window {
                continue;
            }
            let mut lags = [0.0; 5];
            let mut scores = 0.0;
            for (i, fraction) in [0.1, 0.3, 0.5, 0.7, 0.9].into_iter().enumerate() {
                let start = ((reference.cens.len() as f64 * fraction) as usize)
                    .min(reference.cens.len() - window);
                let reference = &reference.cens[start..start + window];
                let mut best = (0, -1.0);
                for frame in 0..=other.cens.len() - window {
                    let value = similarity(reference, &other.cens[frame..]);
                    if value > best.1 {
                        best = (frame, value);
                    }
                }
                lags[i] = (best.0 as f64 - start as f64) / other.fps;
                scores += best.1;
            }
            lags.sort_by(f64::total_cmp);
            if lags[2].abs() < 0.3 && lags[4] - lags[0] < 0.3 && scores / 5.0 >= 0.8 {
                return index;
            }
        }
    }
    0
}

/// Affine gaps allow pauses in live performances without displacing all subsequent lines.
pub(super) fn align_lines(
    reference: &AudioFeatures,
    target: &AudioFeatures,
    times: &[f64],
    coarse: GlobalOffset,
) -> Vec<LineAlignment> {
    let rows = reference.cens.len() / 2;
    let cols = target.cens.len() / 2;
    if rows < 2 || cols < 2 || rows.saturating_mul(cols) > 48_000_000 {
        return times
            .iter()
            .map(|time| LineAlignment {
                time: (time + coarse.offset_secs).max(0.0),
                confidence: coarse.confidence.min(0.49),
                uncertain: true,
            })
            .collect();
    }
    const INF: f32 = 1e9;
    let mut traceback = vec![0u8; rows * cols];
    let mut prev = vec![[0.0, 0.0, INF]; cols];
    let mut curr = vec![[INF; 3]; cols];
    for (row, source) in reference
        .cens
        .iter()
        .step_by(2)
        .take(rows)
        .enumerate()
        .skip(1)
    {
        curr[0] = [INF, INF, prev[0][2] + 0.4];
        for (col, target) in target.cens.iter().step_by(2).take(cols).enumerate().skip(1) {
            let [m, horizontal, vertical] = prev[col - 1];
            let diag = if horizontal < m && horizontal <= vertical {
                1
            } else if vertical < m {
                2
            } else {
                0
            };
            let live_open = curr[col - 1][0] + 0.25;
            let live_extend = curr[col - 1][1] + 0.015;
            let studio_open = prev[col][0] + 0.4;
            let studio_extend = prev[col][2] + 0.28;
            curr[col] = [
                prev[col - 1][diag] + 1.0 - dot(source, target),
                live_open.min(live_extend),
                studio_open.min(studio_extend),
            ];
            traceback[row * cols + col] = diag as u8
                | (u8::from(live_extend < live_open) << 2)
                | (u8::from(studio_extend < studio_open) << 3);
        }
        std::mem::swap(&mut curr, &mut prev);
    }
    let (mut row, mut col) = (rows - 1, cols - 1);
    // After the last matched source frame, trailing live audio is free.
    col = (1..=col)
        .min_by(|&a, &b| {
            prev[a][0]
                .min(prev[a][2])
                .total_cmp(&prev[b][0].min(prev[b][2]))
        })
        .unwrap();
    let mut mode = if prev[col][2] < prev[col][0] { 2 } else { 0 };
    let mut mapping = vec![None; rows];
    while row > 0 && col > 0 {
        let direction = traceback[row * cols + col];
        match mode {
            0 => {
                mapping[row] = Some((col * 2, false));
                mode = direction & 3;
                row -= 1;
                col -= 1;
            }
            1 => {
                mode = (direction >> 2) & 1;
                col -= 1;
            }
            _ => {
                mapping[row] = Some((col * 2, true));
                mode = 2 * ((direction >> 3) & 1);
                row -= 1;
            }
        }
    }
    let mut aligned = times
        .iter()
        .map(|&time| {
            let index = ((time * reference.fps / 2.0) as usize).min(rows - 1);
            let (frame, in_gap) = mapping[index].unwrap_or((
                ((time + coarse.offset_secs).max(0.0) * target.fps) as usize,
                true,
            ));
            let frame = frame.min(target.cens.len() - 1);
            let similarity =
                f64::from(dot(&reference.cens[index * 2], &target.cens[frame])).clamp(0.0, 1.0);
            let confidence = if in_gap {
                (similarity * 0.5).min(0.3)
            } else {
                similarity
            };
            LineAlignment {
                time: frame as f64 / target.fps,
                confidence,
                uncertain: in_gap || confidence < 0.65,
            }
        })
        .collect::<Vec<_>>();
    smooth_outliers(times, &mut aligned);
    aligned
}

fn smooth_outliers(times: &[f64], lines: &mut [LineAlignment]) {
    let offsets = lines
        .iter()
        .zip(times)
        .map(|(line, time)| line.time - time)
        .collect::<Vec<_>>();
    let median = |window: &[f64]| {
        let mut values = [0.0; 3];
        values[..window.len()].copy_from_slice(window);
        let values = &mut values[..window.len()];
        values.sort_by(f64::total_cmp);
        (values[values.len() / 2] + values[(values.len() - 1) / 2]) / 2.0
    };
    for i in 1..lines.len().saturating_sub(1) {
        let before = median(&offsets[i.saturating_sub(3)..i]);
        let after = median(&offsets[i + 1..(i + 4).min(lines.len())]);
        let (left, right) = ((offsets[i] - before).abs(), (offsets[i] - after).abs());
        if left > 0.25 && right > 0.25 {
            lines[i].time = (times[i] + if left < right { before } else { after }).max(0.0);
        }
    }
}

pub(super) fn verify_score(lines: &[LineAlignment]) -> f64 {
    if lines.is_empty() {
        return 0.0;
    }
    let count = lines.len() as f64;
    (lines.iter().map(|line| line.confidence).sum::<f64>() / count)
        * (lines.iter().filter(|line| !line.uncertain).count() as f64 / count)
}
