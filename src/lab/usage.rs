//! Input usage across the library: which ray angles and sensors networks lean on. Usage is the
//! sensitivity estimate from `netviz` (each network's current weights), so it reads "what the
//! networks rely on", not "what makes them fast".

use std::collections::BTreeMap;

use super::compare::{self, NSTAGES};
use super::format::{sensor_name, OUTPUTS};
use super::library::Library;
use super::netviz;
use super::runs;

/// Usage of one ray angle (or one sensor) across networks, as multiples of an even share.
#[derive(Clone, Debug, Default)]
pub struct Usage {
    /// (network, × even for all outputs, × even for steering, × even for pace outputs, stage)
    pub nets: Vec<(String, f64, Option<f64>, Option<f64>, Option<usize>)>,
}

impl Usage {
    fn med(v: Vec<f64>) -> Option<f64> {
        let mut v = v;
        runs::median(&mut v)
    }
    pub fn all(&self) -> Option<f64> {
        Self::med(self.nets.iter().map(|x| x.1).collect())
    }
    pub fn steer(&self) -> Option<f64> {
        Self::med(self.nets.iter().filter_map(|x| x.2).collect())
    }
    pub fn pace(&self) -> Option<f64> {
        Self::med(self.nets.iter().filter_map(|x| x.3).collect())
    }
    pub fn at_stage(&self, s: usize) -> (Option<f64>, usize) {
        let v: Vec<f64> = self.nets.iter().filter(|x| x.4 == Some(s)).map(|x| x.1).collect();
        let n = v.len();
        (Self::med(v), n)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Profile {
    /// Ray angle (degrees, negative = left) -> usage relative to an even share among that
    /// network's rays.
    pub rays: BTreeMap<i32, Usage>,
    /// Sensor name -> usage relative to an even share among all of that network's inputs.
    pub sensors: BTreeMap<String, Usage>,
    pub networks: usize,
}

/// One entry per network with its own history and saved weights. `track` keeps networks whose
/// latest run is on that track.
pub fn profile(lib: &Library, track: Option<&str>) -> Profile {
    let mut p = Profile::default();
    for n in lib.networks.iter().filter(|n| n.gens() > n.fork.own_from && !super::keeper::is_keeper_copy(&n.file)) {
        let save = &n.save;
        let Some(w) = save.weights.as_ref() else { continue };
        let Some(last) = n.runs.last() else { continue };
        if track.is_some_and(|t| last.track != t) {
            continue;
        }
        let sens = netviz::sensitivity(w, &netviz::is_ray(save));
        let all = netviz::sensitivity_all(&sens);
        if all.is_empty() {
            continue;
        }
        let steer: Vec<usize> = save.output_order.iter().enumerate().filter(|(_, o)| OUTPUTS.get(**o as usize) == Some(&"Steer")).map(|(j, _)| j).collect();
        let pace: Vec<usize> = (0..save.output_order.len().min(sens.len())).filter(|j| !steer.contains(j)).collect();
        let avg = |rows: &[usize]| -> Option<Vec<f64>> {
            (!rows.is_empty()).then(|| (0..all.len()).map(|i| rows.iter().map(|&j| sens[j][i]).sum::<f64>() / rows.len() as f64).collect())
        };
        let us = avg(&steer);
        let up = avg(&pace);
        let stage = compare::stage_index(last.stage).filter(|&s| s < NSTAGES);
        let rays: Vec<(usize, i32)> = (0..all.len()).filter_map(|i| save.rays.get(i).copied().flatten().map(|(d, _)| (i, d.round() as i32))).collect();
        // relative to an even share among the rays (so networks with more sensors compare fairly)
        let rel = |u: &[f64], idx: &[usize]| -> Vec<f64> {
            let tot: f64 = idx.iter().map(|&i| u[i]).sum();
            let even = tot / idx.len().max(1) as f64;
            idx.iter().map(|&i| if even > 0.0 { u[i] / even } else { 0.0 }).collect()
        };
        let idx: Vec<usize> = rays.iter().map(|x| x.0).collect();
        if !idx.is_empty() {
            let a = rel(&all, &idx);
            let s = us.as_ref().map(|u| rel(u, &idx));
            let pc = up.as_ref().map(|u| rel(u, &idx));
            for (k, (_, deg)) in rays.iter().enumerate() {
                p.rays.entry(*deg).or_default().nets.push((n.file.clone(), a[k], s.as_ref().map(|v| v[k]), pc.as_ref().map(|v| v[k]), stage));
            }
        }
        let even = 1.0 / all.len() as f64;
        for i in (0..all.len()).filter(|i| !idx.contains(i)) {
            let name = save.arch.sensors.get(i).map(|t| sensor_name(t)).unwrap_or_else(|| format!("input {i}"));
            p.sensors.entry(name).or_default().nets.push((n.file.clone(), all[i] / even, us.as_ref().map(|u| u[i] / even), up.as_ref().map(|u| u[i] / even), stage));
        }
        p.networks += 1;
    }
    p
}
