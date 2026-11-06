//! Deterministic string/fret chooser for tablature.
//!
//! A Viterbi search over [`simultaneous_groups`]: every group gets a handful of candidate
//! fingerings (distinct string per note, each physically playable), scored by an intrinsic cost
//! (low frets, small span, position marks, agreement with fixed decisions) plus a transition cost
//! (hand movement between consecutive groups). No randomness: ties go to the lower cost, then
//! to the lexicographically lower string numbers.
//!
//! Rule for out-of-range notes: `fret` is relative to the capo; if `inst.fret_for(string, midi,
//! capo)` is `None` the caller shifts `midi` by octaves (-12, +12, -24, +24, ...) until it
//! fits. The solver applies exactly this rule and reports it as a warning.

use std::collections::HashMap;

use super::instrument::pitch_name;
use super::{simultaneous_groups, Assignment, Instrument, NoteEvent, PositionMark};

const MAX_CANDIDATES: usize = 200;
const MAX_RAW: usize = 20_000;
const MAX_SPAN: u8 = 5;
const FIXED_PENALTY: f64 = 100.0;
const TIE_LOOKBACK: usize = 8;

/// One playable choice for one note: (string, fret).
type Opt = (u8, u8);

#[derive(Debug, Clone)]
struct Cand {
    /// (event index into `events`, string, fret, sounding midi after octave adjustment)
    notes: Vec<(usize, u8, u8, i32)>,
    cost: f64,
    span: u8,
    /// Lowest non-zero fret.
    pos: Option<u8>,
    /// String of the highest note.
    top_string: Option<u8>,
}

struct GroupInfo {
    measure_idx: usize,
    onset: i64,
    duration: i64,
    grace: bool,
}

/// Octave-shift `midi` until some string can sound it. Returns (adjusted midi, octaves shifted
/// where negative is down), or `None` if nothing fits within five octaves.
fn adjust_midi(inst: &Instrument, capo: u8, midi: i32) -> Option<(i32, i32)> {
    let playable = |m: i32| (1..=inst.strings()).any(|s| inst.fret_for(s, m, capo).is_some());
    if playable(midi) {
        return Some((midi, 0));
    }
    for k in 1..=5 {
        for sign in [-1, 1] {
            let m = midi + sign * 12 * k;
            if playable(m) {
                return Some((m, sign * k));
            }
        }
    }
    None
}

/// Per-event adjusted pitch (`None` when hopeless) plus warnings for shifted notes.
fn effective_midis(events: &[NoteEvent], inst: &Instrument, capo: u8) -> (Vec<Option<i32>>, Vec<String>) {
    let mut warnings = Vec::new();
    let eff = events
        .iter()
        .map(|e| match adjust_midi(inst, capo, e.midi) {
            Some((m, 0)) => Some(m),
            Some((m, k)) => {
                let dir = if k < 0 { "lower" } else { "higher" };
                let amount = if k.abs() == 1 {
                    "an octave".to_string()
                } else {
                    format!("{} octaves", k.abs())
                };
                warnings.push(format!(
                    "m{}: {} out of range for {}, tabbed {} {}",
                    e.measure,
                    pitch_name(e.midi),
                    inst.name,
                    amount,
                    dir
                ));
                Some(m)
            }
            None => {
                warnings.push(format!(
                    "m{}: {} cannot be played on {}, placed on string 1 fret 0",
                    e.measure,
                    pitch_name(e.midi),
                    inst.name
                ));
                None
            }
        })
        .collect();
    (eff, warnings)
}

fn options(inst: &Instrument, capo: u8, midi: i32) -> Vec<Opt> {
    (1..=inst.strings())
        .filter_map(|s| inst.fret_for(s, midi, capo).map(|f| (s, f)))
        .collect()
}

fn mark_for(marks: &[PositionMark], measure_idx: usize, onset: i64) -> Option<&PositionMark> {
    marks
        .iter()
        .filter(|m| (m.measure_idx, m.onset) <= (measure_idx, onset))
        .max_by_key(|m| (m.measure_idx, m.onset))
}

#[allow(clippy::too_many_arguments)]
fn enumerate(
    notes: &[(usize, i32, Vec<Opt>)],
    depth: usize,
    used: u64,
    cur: &mut Vec<(usize, u8, u8, i32)>,
    out: &mut Vec<Vec<(usize, u8, u8, i32)>>,
    raw: &mut usize,
) {
    if *raw >= MAX_RAW {
        return;
    }
    if depth == notes.len() {
        *raw += 1;
        out.push(cur.clone());
        return;
    }
    let (ev, midi, opts) = &notes[depth];
    for &(s, f) in opts {
        if used & (1u64 << s) != 0 {
            continue;
        }
        cur.push((*ev, s, f, *midi));
        enumerate(notes, depth + 1, used | (1u64 << s), cur, out, raw);
        cur.pop();
    }
}

fn make_cand(
    notes: Vec<(usize, u8, u8, i32)>,
    mark: Option<&PositionMark>,
    fixed: &HashMap<usize, u8>,
    events: &[NoteEvent],
) -> Cand {
    let fretted: Vec<u8> = notes.iter().map(|n| n.2).filter(|&f| f > 0).collect();
    let min_f = fretted.iter().copied().min();
    let max_f = fretted.iter().copied().max();
    let span = match (min_f, max_f) {
        (Some(a), Some(b)) => b - a,
        _ => 0,
    };
    let mut cost = 0.0;
    for &(ev, s, f, _) in &notes {
        cost += f as f64 * 0.3;
        if f == 0 {
            if fretted.iter().any(|&x| x > 7) {
                cost += 0.5;
            }
        } else if let Some(m) = mark {
            if f < m.fret || f > m.fret.saturating_add(4) {
                cost += 2.0;
            }
        }
        if let Some(&fs) = fixed.get(&events[ev].idx) {
            if fs != s {
                cost += FIXED_PENALTY;
            }
        }
    }
    if span > 3 {
        cost += (span - 3) as f64;
    }
    let top = notes.iter().max_by_key(|n| (n.3, std::cmp::Reverse(n.1)));
    Cand {
        cost,
        span,
        pos: min_f,
        top_string: top.map(|n| n.1),
        notes,
    }
}

fn transition(prev: &Cand, prev_pos: Option<u8>, pg: &GroupInfo, cur: &Cand, cg: &GroupInfo) -> f64 {
    let mut t = 0.0;
    if let (Some(a), Some(b)) = (prev_pos, cur.pos) {
        t += (a as f64 - b as f64).abs();
    }
    if let (Some(a), Some(b)) = (prev.top_string, cur.top_string) {
        if (a as i32 - b as i32).abs() > 2 {
            t += 0.5;
        }
    }
    // Time to shift: a skipped measure, or a new measure that opens with a rest.
    if cg.measure_idx > pg.measure_idx && (cg.measure_idx - pg.measure_idx >= 2 || cg.onset > 0) {
        t *= 0.5;
    }
    if pg.grace || cg.grace {
        t *= 0.5;
    }
    let _ = pg.duration;
    if let ([p], [c]) = (prev.notes.as_slice(), cur.notes.as_slice()) {
        if p.1 == c.1 && (p.3 - c.3).abs() <= 2 {
            t -= 0.2;
        }
    }
    t
}

/// Greedy placement used when a group has no conflict-free fingering (more notes than strings).
fn greedy(notes: &[(usize, i32, Vec<Opt>)]) -> Vec<(usize, u8, u8, i32)> {
    let mut used = 0u64;
    let mut out = Vec::new();
    for (ev, midi, opts) in notes {
        let pick = opts
            .iter()
            .filter(|(s, _)| used & (1u64 << s) == 0)
            .min_by_key(|(s, f)| (*f, *s))
            .or_else(|| opts.iter().min_by_key(|(s, f)| (*f, *s)));
        if let Some(&(s, f)) = pick {
            used |= 1u64 << s;
            out.push((*ev, s, f, *midi));
        }
    }
    out
}

/// Choose a string (and hence fret) for every event. Returns one `Assignment` per event in
/// `events` order (same length), plus warnings.
///
/// `string` 1 is the highest-pitched string; `fret` is relative to the capo. If
/// `inst.fret_for(string, midi, capo)` is `None` for an event's own pitch, the caller shifts
/// `midi` by octaves (-12, +12, -24, +24, ...) until it fits; the solver does the same and
/// warns, e.g. "m7: C7 out of range for guitar, tabbed an octave lower". A note that fits
/// nowhere gets string 1 fret 0 and a warning.
///
/// `fixed` holds decisions that are respected whenever they are physically possible; an
/// infeasible one is ignored with a warning. `marks` are position hints: from each mark until
/// the next, frets in `[mark.fret, mark.fret + 4]` are preferred. Tied continuations
/// (`tie_stop`) copy string and fret from the previous event of equal pitch (within the last 8
/// events).
///
/// Cost model: intrinsic = 0.3 per fret + 1.0 per unit of span above 3 + 0.5 per open string
/// beside notes above fret 7 + 2.0 per fretted note outside the active mark's window + 100
/// per note contradicting a feasible fixed string; transition = |position change| (position =
/// lowest non-zero fret, carried over open groups) + 0.5 when the top note jumps more than two
/// strings, halved across a measure gap or for grace notes, minus 0.2 for a same-string step
/// between single notes. Fingerings with span above 5 are used only if nothing else exists.
pub fn solve(
    events: &[NoteEvent],
    inst: &Instrument,
    capo: u8,
    marks: &[PositionMark],
    fixed: &[Assignment],
) -> (Vec<Assignment>, Vec<String>) {
    let (eff, mut warnings) = effective_midis(events, inst, capo);
    let mut result: Vec<Option<Assignment>> = vec![None; events.len()];

    // Fixed decisions, validated against the adjusted pitch.
    let index_of: HashMap<usize, usize> = events.iter().enumerate().map(|(i, e)| (e.idx, i)).collect();
    let mut fixed_ok: HashMap<usize, u8> = HashMap::new(); // event idx -> string
    let mut fixed_bad: Vec<(usize, u8)> = Vec::new(); // (position in events, string)
    for f in fixed {
        let Some(&i) = index_of.get(&f.idx) else { continue };
        let ok = eff[i].is_some_and(|m| inst.fret_for(f.string, m, capo).is_some());
        if ok {
            fixed_ok.entry(f.idx).or_insert(f.string);
        } else {
            fixed_bad.push((i, f.string));
        }
    }

    // Tie sources (static: same pitch within the last few events).
    let tie_src: Vec<Option<usize>> = events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            if !e.tie_stop {
                return None;
            }
            (i.saturating_sub(TIE_LOOKBACK)..i)
                .rev()
                .find(|&j| events[j].midi == e.midi)
        })
        .collect();

    let groups = simultaneous_groups(events);
    let mut layers: Vec<Vec<Cand>> = Vec::with_capacity(groups.len());
    let mut infos: Vec<GroupInfo> = Vec::with_capacity(groups.len());
    for g in &groups {
        let first = &events[g[0]];
        infos.push(GroupInfo {
            measure_idx: first.measure_idx,
            onset: first.onset,
            duration: g.iter().map(|&i| events[i].duration).max().unwrap_or(0),
            grace: first.grace,
        });
        let mut free: Vec<(usize, i32, Vec<Opt>)> = Vec::new();
        for &i in g {
            if tie_src[i].is_some() {
                continue;
            }
            match eff[i] {
                Some(m) => {
                    let o = options(inst, capo, m);
                    if o.is_empty() {
                        result[i] = Some(Assignment {
                            idx: events[i].idx,
                            string: 1,
                            fret: 0,
                        });
                    } else {
                        free.push((i, m, o));
                    }
                }
                None => {
                    result[i] = Some(Assignment {
                        idx: events[i].idx,
                        string: 1,
                        fret: 0,
                    })
                }
            }
        }
        if free.is_empty() {
            layers.push(vec![Cand {
                notes: vec![],
                cost: 0.0,
                span: 0,
                pos: None,
                top_string: None,
            }]);
            continue;
        }
        free.sort_by_key(|(i, m, o)| (std::cmp::Reverse(*m), o.len(), *i));
        let mark = mark_for(marks, first.measure_idx, first.onset);
        let mut combos = Vec::new();
        enumerate(&free, 0, 0, &mut Vec::new(), &mut combos, &mut 0);
        let mut cands: Vec<Cand> = combos
            .into_iter()
            .map(|c| make_cand(c, mark, &fixed_ok, events))
            .collect();
        if cands.is_empty() {
            warnings.push(format!(
                "m{}: {} notes sounding together exceed what {} can play, some share a string",
                first.measure,
                free.len(),
                inst.name
            ));
            cands.push(make_cand(greedy(&free), mark, &fixed_ok, events));
        }
        if cands.iter().any(|c| c.span <= MAX_SPAN) {
            cands.retain(|c| c.span <= MAX_SPAN);
        }
        cands.sort_by(|a, b| {
            a.cost
                .total_cmp(&b.cost)
                .then_with(|| a.notes.iter().map(|n| n.1).cmp(b.notes.iter().map(|n| n.1)))
        });
        cands.truncate(MAX_CANDIDATES);
        layers.push(cands);
    }

    // Viterbi. node = (total cost, predecessor, effective position)
    let mut table: Vec<Vec<(f64, usize, Option<u8>)>> = Vec::with_capacity(layers.len());
    for (l, layer) in layers.iter().enumerate() {
        let mut row = Vec::with_capacity(layer.len());
        for c in layer {
            if l == 0 {
                row.push((c.cost, 0, c.pos));
                continue;
            }
            let mut best: Option<(f64, usize, Option<u8>)> = None;
            for (pi, p) in layers[l - 1].iter().enumerate() {
                let (pc, _, ppos) = table[l - 1][pi];
                let total = pc + c.cost + transition(p, ppos, &infos[l - 1], c, &infos[l]);
                if best.is_none_or(|b| total < b.0) {
                    best = Some((total, pi, c.pos.or(ppos)));
                }
            }
            row.push(best.unwrap_or((c.cost, 0, c.pos)));
        }
        table.push(row);
    }
    if let Some(last) = table.last() {
        let mut ci = last
            .iter()
            .enumerate()
            .fold(None::<(usize, f64)>, |b, (i, n)| {
                if b.is_none_or(|x| n.0 < x.1) {
                    Some((i, n.0))
                } else {
                    b
                }
            })
            .map_or(0, |x| x.0);
        for l in (0..layers.len()).rev() {
            for &(ev, s, f, _) in &layers[l][ci].notes {
                result[ev] = Some(Assignment {
                    idx: events[ev].idx,
                    string: s,
                    fret: f,
                });
            }
            ci = table[l][ci].1;
        }
    }

    // Tie continuations, in order.
    for (gi, g) in groups.iter().enumerate() {
        let _ = gi;
        for &i in g {
            let Some(src) = tie_src[i] else { continue };
            let taken =
                |res: &[Option<Assignment>], s: u8| g.iter().any(|&k| k != i && res[k].is_some_and(|a| a.string == s));
            let inherited = result[src].filter(|a| !taken(&result, a.string));
            let a = inherited.or_else(|| {
                let m = eff[i]?;
                let (s, f) = options(inst, capo, m)
                    .into_iter()
                    .filter(|&(s, _)| !taken(&result, s))
                    .min_by_key(|&(s, f)| (f, s))?;
                if result[src].is_some() {
                    warnings.push(format!(
                        "m{}: tied {} moved to string {} (string taken)",
                        events[i].measure,
                        pitch_name(m),
                        s
                    ));
                }
                Some(Assignment {
                    idx: events[i].idx,
                    string: s,
                    fret: f,
                })
            });
            result[i] = Some(a.map_or(
                Assignment {
                    idx: events[i].idx,
                    string: 1,
                    fret: 0,
                },
                |a| Assignment {
                    idx: events[i].idx,
                    ..a
                },
            ));
        }
    }

    // Report ignored fixed decisions.
    for (i, s) in fixed_bad {
        let chosen = result[i].map_or(1, |a| a.string);
        warnings.push(format!(
            "m{}: string {} cannot sound {}, chose string {}",
            events[i].measure,
            s,
            pitch_name(eff[i].unwrap_or(events[i].midi)),
            chosen
        ));
    }

    let out = result
        .into_iter()
        .enumerate()
        .map(|(i, a)| {
            a.unwrap_or(Assignment {
                idx: events[i].idx,
                string: 1,
                fret: 0,
            })
        })
        .collect();
    (out, warnings)
}

/// Frets and strings for assignments already made (e.g. by a model): recompute each fret from
/// string + pitch, dropping (and warning about) any assignment whose string cannot sound the
/// note. Proposals are matched to events by `idx`; unknown `idx` values are dropped with a
/// warning, as are repeated ones. Out-of-range pitches follow the octave rule of [`solve`].
pub fn check_assignments(
    events: &[NoteEvent],
    inst: &Instrument,
    capo: u8,
    proposed: &[Assignment],
) -> (Vec<Assignment>, Vec<String>) {
    let (eff, _) = effective_midis(events, inst, capo);
    let index_of: HashMap<usize, usize> = events.iter().enumerate().map(|(i, e)| (e.idx, i)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    for p in proposed {
        let Some(&i) = index_of.get(&p.idx) else {
            warnings.push(format!("note {}: unknown note index, assignment dropped", p.idx));
            continue;
        };
        if !seen.insert(p.idx) {
            warnings.push(format!(
                "m{}: duplicate assignment for note {}, ignored",
                events[i].measure, p.idx
            ));
            continue;
        }
        let midi = eff[i].unwrap_or(events[i].midi);
        match inst.fret_for(p.string, midi, capo) {
            Some(fret) => out.push(Assignment {
                idx: p.idx,
                string: p.string,
                fret,
            }),
            None => warnings.push(format!(
                "m{}: string {} cannot sound {}, assignment dropped",
                events[i].measure,
                p.string,
                pitch_name(midi)
            )),
        }
    }
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guitar() -> Instrument {
        Instrument {
            name: "guitar".into(),
            tuning: vec![40, 45, 50, 55, 59, 64],
            octave_change: -1,
            max_fret: 20,
            tuning_label: "standard".into(),
        }
    }

    fn ev(idx: usize, measure_idx: usize, onset: i64, midi: i32) -> NoteEvent {
        NoteEvent {
            idx,
            measure_idx,
            measure: (measure_idx + 1).to_string(),
            onset,
            duration: 4,
            midi,
            chord: false,
            grace: false,
            tie_stop: false,
            voice: "1".into(),
            string_hint: None,
            fingering: None,
        }
    }

    fn sf(a: &Assignment) -> (u8, u8) {
        (a.string, a.fret)
    }

    #[test]
    fn open_a_major_chord() {
        let evs: Vec<_> = [45, 52, 57, 61]
            .iter()
            .enumerate()
            .map(|(i, &m)| ev(i, 0, 0, m))
            .collect();
        let (a, w) = solve(&evs, &guitar(), 0, &[], &[]);
        assert!(w.is_empty(), "{w:?}");
        let got: Vec<_> = a.iter().map(sf).collect();
        assert_eq!(got, vec![(5, 0), (4, 2), (3, 2), (2, 2)]);
    }

    #[test]
    fn single_notes_prefer_low_frets() {
        let (a, _) = solve(&[ev(0, 0, 0, 40)], &guitar(), 0, &[], &[]);
        assert_eq!(sf(&a[0]), (6, 0));
        let (a, _) = solve(&[ev(0, 0, 0, 52)], &guitar(), 0, &[], &[]);
        assert_eq!(sf(&a[0]), (4, 2));
    }

    #[test]
    fn position_mark_keeps_hand_up_the_neck() {
        let mut evs: Vec<NoteEvent> = [71, 72, 73, 74]
            .iter()
            .enumerate()
            .map(|(i, &m)| ev(i, 0, i as i64 * 4, m))
            .collect();
        for k in 0..6 {
            evs.push(ev(4 + k, 2, k as i64 * 4, if k % 2 == 0 { 66 } else { 67 }));
        }
        let mark = [PositionMark {
            measure_idx: 0,
            onset: 0,
            fret: 7,
        }];
        let (with, _) = solve(&evs, &guitar(), 0, &mark, &[]);
        assert!(with[4..].iter().all(|a| a.fret >= 7), "{with:?}");
        let (without, _) = solve(&evs, &guitar(), 0, &[], &[]);
        assert!(without[4..].iter().all(|a| a.fret <= 3), "{without:?}");
    }

    #[test]
    fn fixed_assignments() {
        let evs = [ev(0, 0, 0, 64), ev(1, 0, 4, 40)];
        let fixed = [
            Assignment {
                idx: 0,
                string: 2,
                fret: 5,
            },
            Assignment {
                idx: 1,
                string: 1,
                fret: 0,
            },
        ];
        let (a, w) = solve(&evs, &guitar(), 0, &[], &fixed);
        assert_eq!(sf(&a[0]), (2, 5));
        assert_eq!(sf(&a[1]), (6, 0));
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("string 1 cannot sound E2"), "{w:?}");
    }

    #[test]
    fn tie_keeps_string_and_fret() {
        let mut evs = vec![ev(0, 0, 0, 64), ev(1, 0, 4, 59), ev(2, 1, 0, 64)];
        evs[2].tie_stop = true;
        let fixed = [Assignment {
            idx: 0,
            string: 2,
            fret: 5,
        }];
        let (a, _) = solve(&evs, &guitar(), 0, &[], &fixed);
        assert_eq!(sf(&a[0]), (2, 5));
        assert_eq!(sf(&a[2]), (2, 5));
    }

    #[test]
    fn check_assignments_corrects_and_drops() {
        let evs = [ev(0, 0, 0, 64), ev(1, 0, 4, 40), ev(2, 0, 8, 52)];
        let proposed = [
            Assignment {
                idx: 0,
                string: 2,
                fret: 9,
            },
            Assignment {
                idx: 1,
                string: 1,
                fret: 0,
            },
            Assignment {
                idx: 9,
                string: 1,
                fret: 0,
            },
        ];
        let (a, w) = check_assignments(&evs, &guitar(), 0, &proposed);
        assert_eq!(
            a,
            vec![Assignment {
                idx: 0,
                string: 2,
                fret: 5
            }]
        );
        assert_eq!(w.len(), 2, "{w:?}");
    }

    #[test]
    fn out_of_range_note_is_shifted() {
        let (a, w) = solve(&[ev(0, 6, 0, 96)], &guitar(), 0, &[], &[]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("m7: C7 out of range for guitar, tabbed an octave lower"),
            "{w:?}"
        );
        assert_eq!(guitar().fret_for(a[0].string, 84, 0), Some(a[0].fret));
    }
}
