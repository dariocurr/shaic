use crate::error::Result;
use shaic_core::config::Config;

use crate::ProjectAction;

pub fn run(action: ProjectAction) -> Result<()> {
    match action {
        ProjectAction::Add { path } => {
            let mut config = Config::load()?;
            config.add_project(path.clone());
            config.save()?;
            println!("registered project {}", path.display());
        }
        ProjectAction::List => {
            let config = Config::load()?;
            for p in &config.projects {
                println!("{}", p.display());
            }
        }
        ProjectAction::Rm { path } => {
            let mut config = Config::load()?;
            config.remove_project(&path);
            config.save()?;
            println!("unregistered project {}", path.display());
        }
    }
    Ok(())
}
