//! Bounded, duplicate-aware matching on the receiver's monotonic clock.
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    time::{Duration, Instant},
};

#[derive(Default)]
struct SourceStats {
    received: u64,
    unique: u64,
    duplicates: u64,
}
struct Observation {
    times: Vec<Option<Instant>>,
    inserted: Instant,
}
struct PairStats {
    matched: u64,
    baseline_first: u64,
    candidate_first: u64,
    ties: u64,
    late: u64,
    sum: f64,
    min: f64,
    max: f64,
    samples: Vec<f64>,
    random: u64,
}
impl PairStats {
    fn new() -> Self {
        Self {
            matched: 0,
            baseline_first: 0,
            candidate_first: 0,
            ties: 0,
            late: 0,
            sum: 0.,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            samples: Vec::new(),
            random: 0x83f1251da51c747b,
        }
    }
    fn add(&mut self, delta: f64, max_samples: usize) {
        self.matched += 1;
        self.sum += delta;
        self.min = self.min.min(delta);
        self.max = self.max.max(delta);
        if delta > 0. {
            self.baseline_first += 1;
        } else if delta < 0. {
            self.candidate_first += 1;
        } else {
            self.ties += 1;
        }
        if self.samples.len() < max_samples {
            self.samples.push(delta);
        } else {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 7;
            self.random ^= self.random << 17;
            let index = (self.random % self.matched) as usize;
            if index < max_samples {
                self.samples[index] = delta;
            }
        }
    }
    fn report(&self) -> Value {
        if self.matched == 0 {
            return json!({"matched":0,"outside_match_window":self.late,"arrival_difference_us":null});
        }
        let mut samples = self.samples.clone();
        samples.sort_by(f64::total_cmp);
        let q = |pct: usize| samples[(samples.len() - 1) * pct / 100];
        json!({"matched":self.matched,"baseline_first":self.baseline_first,"candidate_first":self.candidate_first,"ties":self.ties,"baseline_first_percent":self.baseline_first as f64*100./self.matched as f64,"candidate_first_percent":self.candidate_first as f64*100./self.matched as f64,"outside_match_window":self.late,"quantile_sample_count":samples.len(),"quantiles_sampled":self.matched>samples.len() as u64,"arrival_difference_us":{"minimum":self.min,"p01":q(1),"p05":q(5),"p50":q(50),"p95":q(95),"p99":q(99),"maximum":self.max,"mean":self.sum/self.matched as f64}})
    }
}

pub struct Comparison {
    names: Vec<String>,
    window: Duration,
    max_keys: usize,
    max_samples: usize,
    pending: HashMap<[u8; 32], Observation>,
    order: VecDeque<([u8; 32], Instant)>,
    sources: Vec<SourceStats>,
    pairs: Vec<PairStats>,
    pub capacity_evictions: u64,
}
impl Comparison {
    pub fn new(names: Vec<String>, window: Duration, max_keys: usize, max_samples: usize) -> Self {
        assert!(names.len() >= 2 && max_keys > 0 && max_samples > 0);
        let count = names.len();
        Self {
            names,
            window,
            max_keys,
            max_samples,
            pending: HashMap::new(),
            order: VecDeque::new(),
            sources: (0..count).map(|_| SourceStats::default()).collect(),
            pairs: (1..count).map(|_| PairStats::new()).collect(),
            capacity_evictions: 0,
        }
    }
    pub fn observe(&mut self, key: [u8; 32], source: usize, stamp: Instant) {
        self.expire(Instant::now());
        self.sources[source].received += 1;
        if !self.pending.contains_key(&key) {
            if self.pending.len() >= self.max_keys {
                if let Some((old, _)) = self.order.pop_front() {
                    self.pending.remove(&old);
                    self.capacity_evictions += 1;
                }
            }
            let inserted = Instant::now();
            self.order.push_back((key, inserted));
            self.pending.insert(
                key,
                Observation {
                    times: vec![None; self.names.len()],
                    inserted,
                },
            );
        }
        let observation = self.pending.get_mut(&key).unwrap();
        if observation.times[source].is_some() {
            self.sources[source].duplicates += 1;
            return;
        }
        self.sources[source].unique += 1;
        observation.times[source] = Some(stamp);
        let candidates: Vec<usize> = if source == 0 {
            (1..self.names.len()).collect()
        } else {
            vec![source]
        };
        for candidate in candidates {
            if let (Some(base), Some(other)) = (observation.times[0], observation.times[candidate])
            {
                let (duration, sign) = if other >= base {
                    (other.duration_since(base), 1.)
                } else {
                    (base.duration_since(other), -1.)
                };
                if duration > self.window {
                    self.pairs[candidate - 1].late += 1;
                } else {
                    self.pairs[candidate - 1]
                        .add(sign * duration.as_secs_f64() * 1_000_000., self.max_samples);
                }
            }
        }
    }
    pub fn contains(&self, key: &[u8; 32]) -> bool {
        self.pending.contains_key(key)
    }
    pub fn expire(&mut self, now: Instant) {
        while self
            .order
            .front()
            .is_some_and(|(_, t)| now.saturating_duration_since(*t) > self.window)
        {
            let (key, inserted) = self.order.pop_front().unwrap();
            if self
                .pending
                .get(&key)
                .is_some_and(|o| o.inserted == inserted)
            {
                self.pending.remove(&key);
            }
        }
    }
    pub fn report(&self) -> Value {
        let sources:Vec<_>=self.sources.iter().enumerate().map(|(i,s)|json!({"name":self.names[i],"received":s.received,"unique_within_window":s.unique,"duplicates_within_window":s.duplicates})).collect();
        let pairs: Vec<_> = self
            .pairs
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut v = p.report();
                v["baseline"] = json!(self.names[0]);
                v["candidate"] = json!(self.names[i + 1]);
                v["baseline_without_match"] =
                    json!(self.sources[0].unique.saturating_sub(p.matched));
                v["candidate_without_match"] =
                    json!(self.sources[i + 1].unique.saturating_sub(p.matched));
                v
            })
            .collect();
        json!({"clock":"single receiver monotonic Instant, captured immediately after stream/socket read","delta_convention":"candidate minus baseline; positive means baseline arrived first","match_window_seconds":self.window.as_secs_f64(),"pending_keys":self.pending.len(),"capacity_evictions":self.capacity_evictions,"capacity_sufficient":self.capacity_evictions==0,"sources":sources,"comparisons":pairs,"unmatched_is_not_packet_loss":true})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn matcher() -> Comparison {
        Comparison::new(
            vec!["a".into(), "b".into()],
            Duration::from_secs(5),
            10,
            100,
        )
    }
    #[test]
    fn first_arrival_wins_and_duplicates_do_not_rematch() {
        let mut m = matcher();
        let t = Instant::now();
        m.observe([1; 32], 1, t + Duration::from_micros(100));
        m.observe([1; 32], 0, t);
        m.observe([1; 32], 0, t + Duration::from_secs(1));
        let r = m.report();
        assert_eq!(r["comparisons"][0]["matched"], 1);
        assert_eq!(r["comparisons"][0]["baseline_first"], 1);
        assert_eq!(r["sources"][0]["duplicates_within_window"], 1);
    }
    #[test]
    fn sign_is_negative_when_candidate_arrives_first() {
        let mut m = matcher();
        let t = Instant::now();
        m.observe([2; 32], 0, t + Duration::from_micros(200));
        m.observe([2; 32], 1, t);
        assert!(
            m.report()["comparisons"][0]["arrival_difference_us"]["p50"]
                .as_f64()
                .unwrap()
                < 0.
        );
    }
    #[test]
    fn bounded_capacity_is_visible() {
        let mut m = Comparison::new(vec!["a".into(), "b".into()], Duration::from_secs(5), 1, 10);
        let t = Instant::now();
        m.observe([1; 32], 0, t);
        m.observe([2; 32], 0, t);
        assert_eq!(m.capacity_evictions, 1);
        assert_eq!(m.report()["pending_keys"], 1);
    }
    #[test]
    fn out_of_window_pairs_are_not_speed_samples() {
        let mut m = matcher();
        let t = Instant::now();
        m.observe([1; 32], 0, t);
        m.observe([1; 32], 1, t + Duration::from_secs(6));
        assert_eq!(m.report()["comparisons"][0]["matched"], 0);
        assert_eq!(m.report()["comparisons"][0]["outside_match_window"], 1);
    }
}
