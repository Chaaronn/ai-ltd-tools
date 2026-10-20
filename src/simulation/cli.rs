use super::*;
use anyhow::{Context, Result, bail};
use storage::{Control, Store};
pub fn run(args: &[String]) -> Result<()> {
    let store = match std::env::var_os("CATS_SIMULATION_ROOT") {
        Some(p) => Store::at(p.into()),
        None => Store::default_location()?,
    };
    match args.first().map(String::as_str) {
        Some("prepare") => {
            println!("{}", serde_json::to_string_pretty(&prepare(&args[1..])?)?);
        }
        Some("smoke") => {
            let spec = JobSpec::smoke()?;
            let id = store.create(&spec)?;
            println!("Job {id}");
            println!(
                "{}",
                serde_json::to_string_pretty(&worker::run(&store, &id)?)?
            );
        }
        Some("create") => {
            let spec: JobSpec =
                serde_json::from_slice(&std::fs::read(args.get(1).context("create SPEC.json")?)?)?;
            println!("{}", store.create(&spec)?);
        }
        Some("template") => {
            println!("{}", serde_json::to_string_pretty(&JobSpec::smoke()?)?);
        }
        Some("start" | "resume") => {
            let id = args.get(1).context("Missing job ID")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&worker::run(&store, id)?)?
            );
        }
        Some("pause" | "cancel") => {
            store.control(
                args.get(1).context("Missing job ID")?,
                if args[0] == "pause" {
                    Control::Pause
                } else {
                    Control::Cancel
                },
            )?;
        }
        Some("inspect") => {
            let id = args.get(1).context("Missing job ID")?;
            let spec = store.spec(id)?;
            let p = store.progress(id)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"spec":spec,"progress":p,"results":analysis::comparisons(&spec,&p.observations)})
                )?
            );
        }
        Some("list") => {
            for id in store.list()? {
                let p = store.progress(&id)?;
                println!("{id} {:?} {}", p.status, store.spec(&id)?.title);
            }
        }
        _ => bail!(
            "simulation: prepare [--save FILE] (--track FILE | --generated-seed N) [--mode fresh|saved|design] [--time-limit SECONDS] [--mutation RATE] | template | create SPEC.json | start ID | resume ID | pause ID | cancel ID | inspect ID | list | smoke"
        ),
    }
    Ok(())
}

fn prepare(args:&[String])->Result<JobSpec> {
    let mut save=None;let mut track=None;let mut generated=None;let mut time=None;let mut mutation=None;let mut mode=CampaignMode::Fresh;
    let mut i=0;
    while i<args.len(){
        let value=||args.get(i+1).context("Missing option value");
        match args[i].as_str(){
            "--origin-grip"=>{i+=1;continue;} // accepted for old scripts: grip offsets are always (0,0)
            "--save"=>save=Some(value()?.clone()),
            "--track"=>track=Some(value()?.clone()),
            "--generated-seed"=>generated=Some(value()?.parse::<i64>()?),
            "--time-limit"=>time=Some(value()?.parse::<f64>()?),
            "--mutation"=>mutation=Some(value()?.parse::<f64>()?),
            "--mode"=>mode=match value()?.as_str(){"fresh"=>CampaignMode::Fresh,"saved"=>CampaignMode::SavedWeights,"design"=>CampaignMode::Designs,_=>bail!("Mode must be fresh, saved or design")},
            unknown=>bail!("Unknown prepare option {unknown}"),
        }i+=2;
    }
    anyhow::ensure!(track.is_some()!=generated.is_some(),"Choose either --track or --generated-seed");
    let (mut input,baseline,inferred)=if let Some(path)=save{let path=std::path::Path::new(&path);let source=engine::read_gzip(path)?.1;engine::from_save(path,&source)?}else{anyhow::ensure!(mode!=CampaignMode::SavedWeights,"Saved mode requires --save");(engine::preset(1,generated.unwrap_or(1729))?,Settings::default(),None)};
    engine::set_track(&mut input,track.as_deref().map(std::path::Path::new),generated.unwrap_or(1729))?;
    let mut spec=JobSpec::new(input,baseline,time.or(inferred).context("Supply --time-limit: source has no effective generation limit")?);spec.mode=mode;
    if mode==CampaignMode::SavedWeights{spec.title="Experiment initialized from saved weights".into();spec.stages=vec![0];}
    if mode==CampaignMode::Designs{spec.title="Whole-training design comparison".into();spec.alternatives=planner::design_alternatives(&spec.input,&spec.baseline,None);spec.alternatives.truncate(3);}
    else {let mut settings=spec.baseline.clone();settings.mutation_rate=mutation.context("Supply --mutation for the single-setting alternative")?;spec.alternatives=vec![Arm{name:format!("Mutation {}",settings.mutation_rate),settings,design:None}];}
    spec.validate()?;Ok(spec)
}
