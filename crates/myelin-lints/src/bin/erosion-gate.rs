use myelin_lints::erosion::{parse_budget, scan_workspace};
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = myelin_lints::coverage::workspace_root();
    let path = root.join("erosion-budget.toml");
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) => {
            eprintln!("L2: cannot read `{}`: {error}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let budget = match parse_budget(&source) {
        Ok(budget) => budget,
        Err(error) => {
            eprintln!("L2: malformed `{}`: {error}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let mut errors = scan_workspace(&root, &budget);
    errors.extend(myelin_lints::dependency_direction::scan_dependency_directions(&root));
    if errors.is_empty() {
        eprintln!(
            "L2: erosion + dependency-direction budgets green (soft {}, hard {}, {} shrink-only allowance(s))",
            budget.soft_limit,
            budget.hard_limit,
            budget.over_limit.len()
        );
        ExitCode::SUCCESS
    } else {
        for error in errors {
            eprintln!("{error}");
        }
        ExitCode::FAILURE
    }
}
