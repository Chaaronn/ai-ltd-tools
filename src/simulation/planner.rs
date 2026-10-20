use super::*;
use crate::lab::{library::Library, overview};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

/// Observed alternatives for the same vehicle. Each arm changes exactly one setting.
pub fn settings_alternatives(
    lib: &Library,
    vehicle: u8,
    baseline: &Settings,
    dimension: Option<&str>,
) -> Vec<Arm> {
    let mut candidates = Vec::new();
    for n in lib
        .networks
        .iter()
        .filter(|n| n.save.vehicle == Some(vehicle))
    {
        for r in n.runs.iter().filter(|r| r.start >= n.fork.own_from) {
            let mut choices: Vec<(&str, String, Settings)> = vec![];
            if let Some(v) = r.settings.mutation {
                let mut s = baseline.clone();
                s.mutation_rate = v;
                choices.push(("Mutation", format!("Mutation {v}"), s));
            }
            if let Some(v) = r
                .settings
                .selection
                .and_then(|v| ["best", "tournament", "roulette"].get(v as usize))
            {
                let mut s = baseline.clone();
                s.selection_algorithm = v.to_string();
                choices.push(("Selection", format!("Selection {v}"), s));
            }
            if let Some(v) = r.settings.selection_size {
                let mut s = baseline.clone();
                s.selection_size = v as usize;
                choices.push(("Selection size", format!("Selection size {v}"), s));
            }
            if let Some(v) = r
                .settings
                .crossover
                .and_then(|v| ["none", "single_point", "uniform"].get(v as usize))
            {
                let mut s = baseline.clone();
                s.crossover = v.to_string();
                choices.push(("Crossover", format!("Crossover {v}"), s));
            }
            if let Some(v) = r.settings.weight_decay {
                let mut s = baseline.clone();
                s.weight_decay = v;
                choices.push(("Weight decay", format!("Weight decay {v}"), s));
            }
            if let Some(v) = r.settings.elimination {
                let mut s = baseline.clone();
                s.eliminate_on_wall = v;
                choices.push(("Elimination", format!("Elimination {v}"), s));
            }
            for (dim, name, s) in choices {
                if dimension.is_some_and(|d| d != dim) || s == *baseline || s.validate().is_err() {
                    continue;
                }
                if !candidates.iter().any(|a: &Arm| a.settings == s) {
                    candidates.push(Arm {
                        name,
                        settings: s,
                        design: None,
                    });
                }
            }
        }
    }
    candidates
}

pub fn validate_design_change(base: &PreparedInput, other: &PreparedInput) -> Result<()> {
    ensure!(
        base.vehicle == other.vehicle
            && base.scene == other.scene
            && base.network["outputs"] == other.network["outputs"],
        "Designs must preserve vehicle, track and controls"
    );
    let a: Vec<usize> = serde_json::from_value(base.network["shape"].clone())?;
    let b: Vec<usize> = serde_json::from_value(other.network["shape"].clone())?;
    ensure!(
        (3..=8).contains(&b.len()) && b[1..b.len() - 1].iter().all(|n| (2..=32).contains(n)),
        "Generated designs need 1–6 hidden layers, 2–32 neurons each"
    );
    ensure!(
        b.windows(2).map(|w| (w[0] + 1) * w[1]).sum::<usize>() <= 10_000,
        "Generated design exceeds 10,000 parameters"
    );
    let sa = &base.model["sensor_layout"]["sensors"];
    let sb = &other.model["sensor_layout"]["sensors"];
    let sensors_changed = sa != sb;
    if sensors_changed {
        ensure!(a == b, "Sensor changes must preserve architecture");
        let (sa, sb) = (
            sa.as_array()
                .ok_or_else(|| anyhow::anyhow!("Exact sensor descriptors required"))?,
            sb.as_array()
                .ok_or_else(|| anyhow::anyhow!("Exact sensor descriptors required"))?,
        );
        ensure!(sa.len() == sb.len(), "Sensor count must remain fixed");
        let changed: Vec<_> = sa.iter().zip(sb).filter(|(a, b)| a != b).collect();
        ensure!(
            changed.len() == 1
                && changed[0].0["$type"] != "raycast"
                && changed[0].1["$type"] != "raycast",
            "Change one non-raycast sensor"
        );
    } else {
        ensure!(
            base.model == other.model && base.network["inputs"] == other.network["inputs"],
            "Layer changes must preserve sensor parameters"
        );
        let ah = &a[1..a.len() - 1];
        let bh = &b[1..b.len() - 1];
        let width = ah.len() == bh.len() && ah.iter().zip(bh).filter(|(a, b)| a != b).count() == 1;
        let insertion = if ah.len() + 1 == bh.len() {
            (0..bh.len()).any(|i| {
                bh.iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, n)| *n)
                    .eq(ah.iter().copied())
            })
        } else {
            false
        };
        let deletion = if bh.len() + 1 == ah.len() {
            (0..ah.len()).any(|i| {
                ah.iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, n)| *n)
                    .eq(bh.iter().copied())
            })
        } else {
            false
        };
        ensure!(
            width || insertion || deletion,
            "Change one hidden width, insert one hidden layer, or remove one hidden layer"
        );
    }
    Ok(())
}
pub fn design_alternatives(
    input: &PreparedInput,
    baseline: &Settings,
    lib: Option<&Library>,
) -> Vec<Arm> {
    let Some(shape) = input.network["shape"].as_array().map(|a| {
        a.iter()
            .filter_map(Value::as_u64)
            .map(|n| n as usize)
            .collect::<Vec<_>>()
    }) else {
        return vec![];
    };
    let mut out = Vec::new();
    let mut add = |mut design: PreparedInput, name: String| {
        design.network.as_object_mut().unwrap().remove("weights");
        design.network.as_object_mut().unwrap().remove("biases");
        if validate_design_change(input, &design).is_ok()
            && engine::validate_input(&design).is_ok()
            && !out.iter().any(|a: &Arm| {
                a.design
                    .as_ref()
                    .is_some_and(|d| d.network == design.network && d.model == design.model)
            })
        {
            out.push(Arm {
                name,
                settings: baseline.clone(),
                design: Some(design),
            });
        }
    };
    if shape.len() >= 3 {
        for width in [shape[1].saturating_sub(4).max(2), (shape[1] + 4).min(32)] {
            let mut d = input.clone();
            let mut s = shape.clone();
            s[1] = width;
            d.network["shape"] = json!(s);
            add(d, format!("First hidden layer: {width}"));
        }
        let mut d = input.clone();
        let mut s = shape.clone();
        s.insert(2, shape[1]);
        d.network["shape"] = json!(s);
        add(d, "Add one hidden layer".into());
        if shape.len() > 3 {
            let mut d = input.clone();
            let mut s = shape.clone();
            s.remove(1);
            d.network["shape"] = json!(s);
            add(d, "Remove first hidden layer".into());
        }
    }
    if let Some(lib) = lib {
        for n in lib
            .networks
            .iter()
            .filter(|n| n.save.vehicle == Some(input.vehicle))
        {
            if let Ok((raw, _)) = engine::read_gzip(&n.path) {
                for sensor in raw["Sensors"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|s| s["$type"] != "raycast")
                {
                    if let Some(rows) = input.model["sensor_layout"]["sensors"].as_array() {
                        for (i, old) in rows
                            .iter()
                            .enumerate()
                            .filter(|(_, s)| s["$type"] != "raycast")
                        {
                            if old == sensor {
                                continue;
                            }
                            let mut d = input.clone();
                            d.model["sensor_layout"]["sensors"][i] = sensor.clone();
                            add(d, format!("Sensor {} → {}", old["$type"], sensor["$type"]));
                        }
                    }
                }
            }
        }
    }
    out
}

/// The card's stage and measure rank the observed alternatives; never invent a bucket midpoint.
pub fn for_card(
    lib: &Library,
    spec: &mut JobSpec,
    card: &crate::lab::gather::Card,
) -> Result<Vec<Arm>> {
    let parts: Vec<_> = card.id.split(':').collect();
    ensure!(
        parts.first() == Some(&"setting") && parts.len() >= 3,
        "This question needs a different experiment protocol"
    );
    let dim = if parts[2] == "Parents kept" {
        "Selection size"
    } else {
        parts[2]
    };
    ensure!(
        !["Population", "Rewards"].contains(&dim),
        "Population is fixed at 1,200; reward experiments are not settings comparisons"
    );
    let measure = match spec.measure {
        Measure::Lap => crate::lab::compare::Measure::Lap,
        Measure::Rate => crate::lab::compare::Measure::Rate,
        Measure::Survival => crate::lab::compare::Measure::Survival,
    };
    let stage = crate::lab::compare::STAGES
        .iter()
        .position(|s| *s == parts[1])
        .unwrap_or(0);
    // Match the displayed question before filtering for same-vehicle values.
    let book = overview::playbook(lib, None, measure, lib.min_side.max(1));
    let pick = book
        .get(stage)
        .and_then(|b| b.picks.iter().find(|p| p.dimension == dim))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "This card no longer has a supported comparison; refresh Data gathering"
            )
        })?;
    let runner = pick
        .runner_up
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("This question has no alternative"))?;
    let vehicle = spec.input.vehicle;
    let runs_of = move |any: bool| {
        lib.networks
            .iter()
            .filter(move |n| any || n.save.vehicle == Some(vehicle))
            .flat_map(|n| n.runs.iter().filter(move |r| r.start >= n.fork.own_from))
    };
    let runs = || runs_of(false);
    // The most common value trained with inside a compared range, applied to `base`: from this
    // vehicle's runs, or any vehicle's when this one never used that range.
    let value_in = |label: &str, base: &Settings| -> Option<Settings> {
        for any in [false, true] {
            let mut counts: Vec<(Settings, usize)> = vec![];
            for r in runs_of(any).filter(|r| overview::dim_label(dim, r).as_deref() == Some(label)) {
                let Some(s) = with_value(dim, base, r) else { continue };
                match counts.iter_mut().find(|(c, _)| *c == s) {
                    Some(c) => c.1 += 1,
                    None => counts.push((s, 1)),
                }
            }
            if let Some((s, _)) = counts.into_iter().filter(|(s, _)| s.validate().is_ok() && s != base).max_by_key(|(_, n)| *n) {
                return Some(s);
            }
        }
        None
    };
    let current = runs()
        .find(|r| matches_setting(dim, &spec.baseline, r))
        .and_then(|r| overview::dim_label(dim, r));
    let arm_settings = if current.as_deref() == Some(pick.best.label.as_str()) {
        value_in(&runner.label, &spec.baseline)
    } else if current.as_deref() == Some(runner.label.as_str()) {
        value_in(&pick.best.label, &spec.baseline)
    } else {
        // The network's own value is in neither compared range: test the card's two options
        // against each other, with the current advice's alternative as the baseline.
        let base = value_in(&runner.label, &spec.baseline)
            .ok_or_else(|| anyhow::anyhow!("No run of this vehicle has used {} {}", dim.to_lowercase(), runner.label))?;
        let arm = value_in(&pick.best.label, &base);
        spec.input.notes.push(format!(
            "This network's {} is in neither of the card's ranges ({} vs {}), so the baseline uses {} and the alternative {}.",
            dim.to_lowercase(),
            pick.best.label,
            runner.label,
            setting_text(dim, &base),
            arm.as_ref().map_or("?".into(), |a| setting_text(dim, a)),
        ));
        spec.baseline = base;
        arm
    };
    let settings = arm_settings
        .filter(|s| *s != spec.baseline)
        .ok_or_else(|| anyhow::anyhow!("None of your saves has used a different {} in the card's other range, so there's no value to test", dim.to_lowercase()))?;
    Ok(vec![Arm {
        name: setting_text(dim, &settings),
        settings,
        design: None,
    }])
}

/// `base` with one setting taken from an observed run.
fn with_value(dim: &str, base: &Settings, r: &crate::lab::runs::Run) -> Option<Settings> {
    let mut s = base.clone();
    match dim {
        "Mutation" => s.mutation_rate = r.settings.mutation?,
        "Selection" => s.selection_algorithm = ["best", "tournament", "roulette"].get(r.settings.selection? as usize)?.to_string(),
        "Selection size" => s.selection_size = r.settings.selection_size? as usize,
        "Crossover" => s.crossover = ["none", "single_point", "uniform"].get(r.settings.crossover? as usize)?.to_string(),
        "Weight decay" => s.weight_decay = r.settings.weight_decay?,
        "Elimination" => s.eliminate_on_wall = r.settings.elimination?,
        _ => return None,
    }
    Some(s)
}

/// "Mutation 0.05", "Selection size 10", ... for one setting.
fn setting_text(dim: &str, s: &Settings) -> String {
    match dim {
        "Mutation" => format!("Mutation {}", (s.mutation_rate * 10000.0).round() / 10000.0),
        "Selection" => format!("Selection {}", s.selection_algorithm),
        "Selection size" => format!("Selection size {}", s.selection_size),
        "Crossover" => format!("Crossover {}", s.crossover.replace('_', " ")),
        "Weight decay" => format!("Weight decay {}", s.weight_decay),
        "Elimination" => format!("Elimination {}", if s.eliminate_on_wall { "on" } else { "off" }),
        _ => dim.to_string(),
    }
}

fn matches_setting(dim: &str, settings: &Settings, run: &crate::lab::runs::Run) -> bool {
    match dim {
        "Mutation" => run.settings.mutation == Some(settings.mutation_rate),
        "Selection" => run
            .settings
            .selection
            .and_then(|v| ["best", "tournament", "roulette"].get(v as usize))
            .is_some_and(|v| *v == settings.selection_algorithm),
        "Selection size" => {
            run.settings.selection_size.map(|v| v as usize) == Some(settings.selection_size)
        }
        "Crossover" => run
            .settings
            .crossover
            .and_then(|v| ["none", "single_point", "uniform"].get(v as usize))
            .is_some_and(|v| *v == settings.crossover),
        "Weight decay" => run.settings.weight_decay == Some(settings.weight_decay),
        "Elimination" => run.settings.elimination == Some(settings.eliminate_on_wall),
        _ => false,
    }
}

fn same_vehicle(lib: &Library, vehicle: u8) -> Library {
    let mut scoped = lib.clone();
    // Preserve ancestry indices, but remove other vehicles' observations.
    scoped.networks = lib
        .networks
        .iter()
        .map(|n| {
            let mut n = (**n).clone();
            if n.save.vehicle != Some(vehicle) {
                n.runs.clear();
                n.verdicts.clear();
            }
            std::sync::Arc::new(n)
        })
        .collect();
    scoped
}

/// Rank unresolved playbook dimensions at the selected stage, then observed values.
pub fn explore_settings(lib: &Library, spec: &JobSpec) -> Vec<Arm> {
    let scoped = same_vehicle(lib, spec.input.vehicle);
    let measure = match spec.measure {
        Measure::Lap => crate::lab::compare::Measure::Lap,
        Measure::Rate => crate::lab::compare::Measure::Rate,
        Measure::Survival => crate::lab::compare::Measure::Survival,
    };
    let stage = match spec.stages.first().copied().unwrap_or(0) {
        0..200 => 0,
        200..1000 => 1,
        1000..5000 => 2,
        _ => 3,
    };
    let book = overview::playbook(&scoped, None, measure, 10);
    let mut dimensions = vec![
        "Mutation",
        "Selection",
        "Selection size",
        "Crossover",
        "Weight decay",
        "Elimination",
    ];
    dimensions.sort_by_key(|dimension| {
        book.get(stage)
            .and_then(|b| b.picks.iter().find(|p| p.dimension == *dimension))
            .map_or(0, |p| match p.strength {
                overview::Strength::Weak => 1,
                overview::Strength::Some => 2,
                _ => 3,
            })
    });
    let mut out = vec![];
    for dimension in dimensions {
        let candidates =
            settings_alternatives(&scoped, spec.input.vehicle, &spec.baseline, Some(dimension));
        out.extend(candidates);
    }
    out
}
