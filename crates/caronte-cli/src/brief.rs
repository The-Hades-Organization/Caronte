//! `caronte brief <plano>` (CC:279-287): só leitura.

use std::path::Path;

use caronte_core::pack::{resolve_plan, work_of};
use caronte_core::plan::{Settled, linked_agents, resume_prompt, settled};
use hades_core::Env;
use hades_core::fsx::read_text;
use hades_core::output::{HadesError, exit, out};

/// Argumentos a mais são ignorados, como no TS.
pub fn run(args: &[String], env: &Env, cwd: &Path) -> Result<i32, HadesError> {
    let plan_file = resolve_plan(env, args.first().map(String::as_str))?;
    let work = work_of(env, cwd, &plan_file)?;
    let text = read_text(&plan_file).unwrap_or_default();
    let parked: Vec<String> = linked_agents(&work, &text)
        .into_iter()
        .filter(|a| settled(&a.record, &work.reports) == Some(Settled::Parked))
        .map(|a| a.name)
        .collect();
    out(&resume_prompt(&plan_file.to_string_lossy(), &parked));
    Ok(exit::OK)
}
